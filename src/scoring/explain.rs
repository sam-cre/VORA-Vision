//! Plain-English explanation of a result: *why* the signal is what it is, and
//! *what would flip it*. All derived deterministically from the category scores —
//! no model, no extra data. This turns the composite number into a second opinion
//! a person can reason about and disagree with.

use crate::models::{AnalysisResult, CategoryScore, Signal};
use crate::scoring::engine::composite_from_categories;
use crate::scoring::signal::{get_signal, thresholds};

/// One category's signed influence on the composite, with a representative
/// human-readable detail (the engine's first note for that category, if any).
#[derive(Debug, Clone)]
pub struct Factor {
    pub name: String,
    /// True marginal effect on the 0-100 composite, in points: how far the score
    /// would fall (positive) or rise (negative) if this category reverted to a
    /// neutral 50, holding the others fixed. Computed with the same renormalization
    /// the live score uses, so it's exact rather than a proxy.
    pub contribution: f64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Explanation {
    /// One-line summary of why the signal landed where it did.
    pub headline: String,
    /// Positive contributors, strongest first.
    pub drivers: Vec<Factor>,
    /// Negative contributors, most-negative first.
    pub detractors: Vec<Factor>,
    /// "What would flip this signal" sentence.
    pub flip: String,
}

fn signal_word(s: Signal) -> &'static str {
    match s {
        Signal::Buy => "BUY",
        Signal::Hold => "HOLD",
        Signal::Sell => "SELL",
    }
}

fn category_list(result: &AnalysisResult) -> [(&'static str, CategoryScore); 5] {
    [
        ("Valuation", result.valuation.clone()),
        ("Fundamentals", result.fundamentals.clone()),
        ("Macro", result.macro_env.clone()),
        ("Sentiment", result.sentiment.clone()),
        ("Risk", result.risk.clone()),
    ]
}

fn factors(result: &AnalysisResult) -> Vec<Factor> {
    let cats = category_list(result);
    let base: Vec<CategoryScore> = cats.iter().map(|(_, c)| c.clone()).collect();
    let base_refs: Vec<&CategoryScore> = base.iter().collect();
    let full = composite_from_categories(&base_refs);

    cats.iter()
        .enumerate()
        .map(|(i, (name, c))| {
            let mut hypo = base.clone();
            hypo[i].raw_score = 50.0;
            let hypo_refs: Vec<&CategoryScore> = hypo.iter().collect();
            let neutralized = composite_from_categories(&hypo_refs);
            Factor {
                name: name.to_string(),
                contribution: full - neutralized, // marginal points on the composite
                detail: c.notes.first().cloned(),
            }
        })
        .collect()
}

/// Find the load-bearing category: the one whose reverting to neutral (50) would
/// flip the signal, choosing the most influential such category. Returns its name
/// and the signal the result would become.
fn load_bearing(result: &AnalysisResult) -> Option<(String, Signal)> {
    let cats = category_list(result);
    let names: Vec<&str> = cats.iter().map(|(n, _)| *n).collect();
    let base: Vec<CategoryScore> = cats.iter().map(|(_, c)| c.clone()).collect();

    let mut best: Option<(String, Signal, f64)> = None;
    for i in 0..base.len() {
        let mut hypo = base.clone();
        hypo[i].raw_score = 50.0;
        let refs: Vec<&CategoryScore> = hypo.iter().collect();
        let new_comp = (composite_from_categories(&refs) * 10.0).round() / 10.0;
        let new_sig = get_signal(new_comp, result.horizon);
        if new_sig != result.signal {
            let influence = ((base[i].raw_score - 50.0) * base[i].weight * base[i].coverage).abs();
            if best.as_ref().map_or(true, |(_, _, inf)| influence > *inf) {
                best = Some((names[i].to_string(), new_sig, influence));
            }
        }
    }
    best.map(|(name, sig, _)| (name, sig))
}

pub fn explain(result: &AnalysisResult) -> Explanation {
    let all = factors(result);

    let mut drivers: Vec<Factor> = all.iter().filter(|f| f.contribution > 0.1).cloned().collect();
    drivers.sort_by(|a, b| b.contribution.partial_cmp(&a.contribution).unwrap_or(std::cmp::Ordering::Equal));

    let mut detractors: Vec<Factor> = all.iter().filter(|f| f.contribution < -0.1).cloned().collect();
    detractors.sort_by(|a, b| a.contribution.partial_cmp(&b.contribution).unwrap_or(std::cmp::Ordering::Equal));

    // Headline narrative.
    let sig = signal_word(result.signal);
    let top_drivers = drivers.iter().take(2).map(|f| f.name.clone()).collect::<Vec<_>>().join(" and ");
    let top_detractor = detractors.first().map(|f| f.name.clone());
    let headline = match (drivers.is_empty(), &top_detractor) {
        (false, Some(d)) => format!("{} — strongest support from {}; biggest drag from {}.", sig, top_drivers, d),
        (false, None) => format!("{} — strongest support from {}, with nothing materially dragging it down.", sig, top_drivers),
        (true, Some(d)) => format!("{} — no category is clearly bullish; {} is the biggest drag.", sig, d),
        (true, None) => format!("{} — every category sits near neutral; little conviction either way.", sig),
    };

    // Sensitivity: distance to thresholds + the load-bearing category.
    let (buy_t, sell_t) = thresholds(result.horizon);
    let score = result.composite_score;
    let flip = match result.signal {
        Signal::Buy => {
            let lead = match load_bearing(result) {
                Some((name, new_sig)) => format!(
                    " Main support is {}; if it weakened to neutral this would become a {}.",
                    name, signal_word(new_sig)
                ),
                None => " No single category reverting to neutral would change the call — broadly supported.".to_string(),
            };
            format!("Sits {:.1} pts above the BUY line ({:.0}).{}", score - buy_t, buy_t, lead)
        }
        Signal::Sell => {
            let lead = match load_bearing(result) {
                Some((name, new_sig)) => format!(
                    " Main drag is {}; if it recovered to neutral this would become a {}.",
                    name, signal_word(new_sig)
                ),
                None => " No single category reverting to neutral would change the call.".to_string(),
            };
            format!("Sits {:.1} pts below the SELL line ({:.0}).{}", sell_t - score, sell_t, lead)
        }
        Signal::Hold => {
            // A HOLD whose raw score sits outside the band can only be a coverage-floored
            // call (get_signal would otherwise have returned a direction) — explain that
            // rather than printing a negative "distance to BUY".
            if score >= buy_t {
                format!(
                    "Scores {:.1} (above the BUY line, {:.0}) but data coverage is only {:.0}% — held at HOLD until more data confirms it.",
                    score, buy_t, result.confidence_score
                )
            } else if score <= sell_t {
                format!(
                    "Scores {:.1} (below the SELL line, {:.0}) but data coverage is only {:.0}% — held at HOLD until more data confirms it.",
                    score, sell_t, result.confidence_score
                )
            } else {
                format!(
                    "In the HOLD band — {:.1} pts from BUY ({:.0}) and {:.1} pts from SELL ({:.0}).",
                    buy_t - score, buy_t, score - sell_t, sell_t
                )
            }
        }
    };

    Explanation { headline, drivers, detractors, flip }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EdgarData, FinnhubData, FredData, Horizon, YahooData};
    use crate::scoring::engine::calculate_analysis;

    fn empty_inputs() -> (YahooData, FredData, EdgarData, FinnhubData) {
        let yahoo = YahooData {
            ticker: "T".into(), price: Some(100.0), market_cap: Some(1e9),
            price_52w_change_pct: None, pe_ratio: None, ps_ratio: None, pb_ratio: None,
            eps: None, eps_growth_yoy: None, revenue_growth_yoy: None,
            fifty_two_week_high: None, fifty_two_week_low: None, short_interest_percent: None,
            dividend_yield: None, sector: Some("Technology".into()), industry: None,
            institutional_ownership_percent: None, gross_margin: None, operating_margin: None,
        };
        let fred = FredData {
            fed_funds_rate: None, cpi_yoy_change: None, cpi_trend: None, fear_and_greed: None,
            yield_curve_spread: None, unemployment_rate: None, vix: None,
        };
        let edgar = EdgarData {
            ticker: "T".into(), fcf_latest: None, fcf_growth_yoy: None, debt_to_equity: None,
            interest_coverage_ratio: None, total_debt: None, cash_and_equivalents: None,
            actual_eps_growth_yoy: None,
        };
        let finnhub = FinnhubData {
            ticker: "T".into(), news_sentiment_score: None, insider_net_shares_3m: None,
            sector_pe_avg: None, sector_ps_avg: None, sector_growth_score: None,
            recent_competitor_ipos: Vec::new(), next_earnings_days: None,
        };
        (yahoo, fred, edgar, finnhub)
    }

    #[test]
    fn test_strong_fundamentals_drive_explanation() {
        let (mut yahoo, fred, mut edgar, finnhub) = empty_inputs();
        // Cheap valuation + strong fundamentals → bullish drivers.
        yahoo.pe_ratio = Some(8.0);
        yahoo.revenue_growth_yoy = Some(30.0);
        edgar.actual_eps_growth_yoy = Some(40.0);
        let result = calculate_analysis("T", Horizon::Long, &yahoo, &fred, &edgar, &finnhub);

        let ex = explain(&result);
        assert!(!ex.drivers.is_empty(), "expected at least one bullish driver");
        assert!(ex.headline.contains(signal_word(result.signal)));
        assert!(!ex.flip.is_empty());
    }

    #[test]
    fn test_flip_mentions_thresholds() {
        let (mut yahoo, fred, edgar, finnhub) = empty_inputs();
        // Full, perfectly-neutral valuation coverage (P/E, P/S, P/B all score 50)
        // with everything else absent → a mid-band HOLD whose flip line names both bands.
        yahoo.pe_ratio = Some(20.0); // == default sector ref → 50
        yahoo.ps_ratio = Some(3.0);  // == default sector ref → 50
        yahoo.pb_ratio = Some(20.0); // half the tech ceiling (40) → 50
        let result = calculate_analysis("T", Horizon::Medium, &yahoo, &fred, &edgar, &finnhub);
        assert_eq!(result.signal, Signal::Hold, "score was {}", result.composite_score);
        let ex = explain(&result);
        assert!(ex.flip.contains("BUY") && ex.flip.contains("SELL"));
    }
}
