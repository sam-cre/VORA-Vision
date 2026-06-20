//! Lexicon-based news sentiment.
//!
//! Finnhub's quantitative news-sentiment endpoint is premium-only, but its
//! `company-news` headline feed is free. This module scores those headlines with
//! a curated financial-sentiment word list (in the spirit of the Loughran-McDonald
//! finance dictionary, trimmed to the vocabulary that actually shows up in market
//! headlines). It is deterministic, dependency-free, and fully transparent — no
//! model, no API cost.
//!
//! Beyond raw word counts it applies the corrections that separate a toy scorer
//! from a defensible one:
//!   - **De-duplication** of syndicated reprints, so one story carried by 30
//!     outlets counts once instead of 30×.
//!   - **Negation handling** ("fails to beat", "no growth") flips polarity.
//!   - **Recency weighting** (7-day half-life) so a stale headline fades.
//!   - **Evidence shrinkage** toward neutral, so a single strongly-worded day
//!     can't produce a confident ±1.0.
//!
//! It is a *live-only* signal: historical headline archives can't be reconstructed
//! point-in-time without bias or cost, so the backtest leaves news sentiment as
//! `None` (handled by the coverage-aware engine), exactly as before.

use std::collections::HashSet;

/// A headline plus how many days ago it was published (for recency weighting).
#[derive(Debug, Clone)]
pub struct Headline {
    pub text: String,
    pub age_days: f64,
}

/// Recency half-life: a headline `HALF_LIFE_DAYS` old carries half the weight.
const HALF_LIFE_DAYS: f64 = 7.0;
/// Shrinkage prior strength — net polarity is pulled toward 0 until evidence
/// (recency-weighted sentiment-word hits) clears this bar.
const SHRINK_K: f64 = 3.0;
/// Minimum unweighted sentiment-word hits required to report a score at all.
const MIN_HITS: u32 = 2;

/// Positive finance-headline terms (and common inflections).
const POSITIVE: &[&str] = &[
    "beat", "beats", "beating", "surpass", "surpassed", "surpasses", "surge", "surged",
    "surges", "soar", "soared", "soars", "jump", "jumped", "jumps", "rally", "rallied",
    "gain", "gains", "gained", "rise", "rises", "rose", "climb", "climbed", "climbs",
    "record", "records", "upgrade", "upgraded", "upgrades", "outperform", "outperformed",
    "strong", "strength", "growth", "grow", "grew", "growing", "profit", "profits",
    "profitable", "boost", "boosted", "boosts", "raise", "raised", "raises", "top",
    "topped", "tops", "exceed", "exceeded", "exceeds", "bullish", "optimistic",
    "breakthrough", "expansion", "expand", "expanded", "approval", "approved", "win",
    "wins", "won", "award", "awarded", "dividend", "buyback", "rebound", "rebounded",
    "accelerate", "accelerated", "momentum", "milestone", "robust", "upbeat", "rallies",
    "outperforms", "surging", "soaring", "highs", "outpaces", "outpaced",
];

/// Negative finance-headline terms (and common inflections).
const NEGATIVE: &[&str] = &[
    "miss", "misses", "missed", "missing", "plunge", "plunged", "plunges", "plummet",
    "plummeted", "plummets", "drop", "dropped", "drops", "fall", "falls", "fell",
    "falling", "decline", "declined", "declines", "declining", "slump", "slumped",
    "tumble", "tumbled", "tumbles", "sink", "sank", "sinks", "downgrade", "downgraded",
    "downgrades", "underperform", "underperformed", "weak", "weakness", "weaker", "loss",
    "losses", "lose", "lost", "losing", "cut", "cuts", "slash", "slashed", "warn",
    "warning", "warned", "warns", "lawsuit", "sue", "sued", "suing", "probe",
    "investigation", "investigate", "fraud", "recall", "recalled", "bankruptcy",
    "bankrupt", "default", "defaulted", "layoff", "layoffs", "fired", "firing", "scandal",
    "halt", "halted", "halts", "fear", "fears", "concern", "concerns", "risk", "risks",
    "risky", "bearish", "pessimistic", "downturn", "recession", "crisis", "crash",
    "crashed", "slowdown", "struggle", "struggled", "struggles", "disappoint",
    "disappointed", "disappointing", "shortfall", "deficit", "delay", "delayed", "delays",
    "suspend", "suspended", "breach", "penalty", "fine", "fined", "selloff", "crackdown",
    "subpoena", "downsize", "writedown", "impairment", "glut", "oversupply", "headwind",
    "headwinds", "sinking", "tumbling", "plunging", "slashing",
];

/// Negators that flip the polarity of a sentiment word appearing shortly after.
const NEGATORS: &[&str] = &[
    "not", "no", "never", "without", "fails", "fail", "failed", "lacks", "lack", "cannot",
    "isnt", "arent", "wont", "doesnt", "didnt", "fewer",
];

/// Score a batch of headlines into a sentiment value in `[-1.0, +1.0]`.
///
/// Returns `None` when there isn't enough signal (fewer than [`MIN_HITS`]
/// sentiment-bearing words across all *distinct* headlines), so the engine falls
/// back to its market-wide Fear & Greed proxy rather than reporting noise.
pub fn score_headlines(headlines: &[Headline]) -> Option<f64> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut weighted_pos = 0.0f64;
    let mut weighted_neg = 0.0f64;
    let mut hits: u32 = 0;

    for hl in headlines {
        let lower = hl.text.to_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() {
            continue;
        }

        // De-duplicate: collapse identical (normalized) headlines from many outlets.
        let key = tokens.join(" ");
        if !seen.insert(key) {
            continue;
        }

        let recency_w = 0.5f64.powf(hl.age_days.max(0.0) / HALF_LIFE_DAYS);

        // Walk tokens, flipping polarity when a negator appeared within 3 tokens.
        let mut neg_window = 0u8;
        for tok in &tokens {
            let mut polarity = if POSITIVE.contains(tok) {
                1i32
            } else if NEGATIVE.contains(tok) {
                -1
            } else {
                0
            };

            if polarity != 0 {
                if neg_window > 0 {
                    polarity = -polarity;
                }
                hits += 1;
                if polarity > 0 {
                    weighted_pos += recency_w;
                } else {
                    weighted_neg += recency_w;
                }
            }

            if NEGATORS.contains(tok) {
                neg_window = 3;
            } else if neg_window > 0 {
                neg_window -= 1;
            }
        }
    }

    if hits < MIN_HITS {
        return None;
    }
    let evidence = weighted_pos + weighted_neg;
    if evidence <= 0.0 {
        return None;
    }
    // Net polarity, then shrink toward neutral when evidence is thin.
    let ratio = (weighted_pos - weighted_neg) / evidence;
    let shrunk = ratio * (evidence / (evidence + SHRINK_K));
    Some(shrunk.clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(text: &str, age_days: f64) -> Headline {
        Headline { text: text.to_string(), age_days }
    }

    #[test]
    fn test_bullish_headlines_score_positive() {
        let news = vec![
            h("Acme beats earnings estimates, shares surge to record high", 1.0),
            h("Analysts upgrade Acme on strong revenue growth", 2.0),
        ];
        let score = score_headlines(&news).unwrap();
        assert!(score > 0.3, "expected positive, got {}", score);
    }

    #[test]
    fn test_bearish_headlines_score_negative() {
        let news = vec![
            h("Acme misses estimates, stock plunges on weak guidance", 1.0),
            h("Acme faces lawsuit and product recall amid fraud probe", 2.0),
        ];
        let score = score_headlines(&news).unwrap();
        assert!(score < -0.3, "expected negative, got {}", score);
    }

    #[test]
    fn test_negation_flips_polarity() {
        // "fails to beat" and "no growth" should read negative, not positive.
        let news = vec![
            h("Acme fails to beat estimates and shows no growth", 1.0),
            h("Acme lacks momentum and cannot top guidance", 1.0),
        ];
        let score = score_headlines(&news).unwrap();
        assert!(score < 0.0, "negation should yield negative, got {}", score);
    }

    #[test]
    fn test_duplicates_do_not_inflate() {
        // The same syndicated headline 30×.
        let dup: Vec<Headline> = (0..30)
            .map(|_| h("Acme beats estimates and raises guidance", 1.0))
            .collect();
        let single = vec![h("Acme beats estimates and raises guidance", 1.0)];
        let s_dup = score_headlines(&dup).unwrap();
        let s_single = score_headlines(&single).unwrap();
        assert!((s_dup - s_single).abs() < 1e-9, "dups inflated: {} vs {}", s_dup, s_single);
    }

    #[test]
    fn test_recency_weighting() {
        // Fresh negative news outweighs stale positive news.
        let news = vec![
            h("Acme beats estimates, shares surge", 60.0), // old, heavily decayed
            h("Acme plunges on fraud probe and weak guidance", 0.0), // today
        ];
        let score = score_headlines(&news).unwrap();
        assert!(score < 0.0, "recent negative should dominate, got {}", score);
    }

    #[test]
    fn test_shrinkage_scales_with_evidence() {
        // More independent confirming headlines → larger magnitude (more confident).
        let thin = vec![h("Acme beats estimates", 0.0)]; // 1 hit only
        // thin has a single hit → below MIN_HITS → None
        assert!(score_headlines(&thin).is_none());

        let some = vec![
            h("Acme beats estimates and raises guidance", 0.0),
            h("Acme shares surge on strong growth", 0.0),
        ];
        let many: Vec<Headline> = (0..8)
            .map(|i| h(&format!("Acme beats estimates with strong growth number {}", i), 0.0))
            .collect();
        let s_some = score_headlines(&some).unwrap();
        let s_many = score_headlines(&many).unwrap();
        assert!(s_many > s_some, "more evidence should be more confident: {} vs {}", s_many, s_some);
    }

    #[test]
    fn test_no_signal_returns_none() {
        let news = vec![h("Acme to host investor day in the third quarter", 1.0)];
        assert!(score_headlines(&news).is_none());
        assert!(score_headlines(&[]).is_none());
    }
}
