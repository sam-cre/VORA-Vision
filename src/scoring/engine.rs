use crate::models::{CategoryScore, Horizon, AnalysisResult, YahooData, FredData, EdgarData, FinnhubData};
use crate::scoring::normalize::*;
use crate::scoring::weights::get_weights;
use crate::scoring::signal::get_signal;

/// Coverage-adjusted composite score (0-100) from the five category scores.
///
/// Two stages:
///   1. A coverage-weighted mean of the raw category scores (weight = wᵢ·coverageᵢ),
///      so categories backed by little data count less.
///   2. **Shrinkage toward neutral (50)** by the fraction of total weight actually
///      covered (`k = Σ wᵢ·coverageᵢ`, in 0..1). A fully-covered stock is unchanged;
///      a thin-data stock is pulled toward HOLD — so a single present signal can no
///      longer mint a confident BUY/SELL out of almost no data.
///
/// Exposed so the explanation/sensitivity logic recomputes hypotheticals with
/// exactly the same math the live score uses.
pub fn composite_from_categories(categories: &[&CategoryScore]) -> f64 {
    let mut covered_weight = 0.0; // Σ wᵢ·coverageᵢ  (== k, the shrink factor)
    let mut weighted_sum = 0.0;   // Σ rawᵢ·wᵢ·coverageᵢ
    for cat in categories {
        let cw = cat.weight * cat.coverage;
        covered_weight += cw;
        weighted_sum += cat.raw_score * cw;
    }
    if covered_weight <= 1e-9 {
        return 50.0; // no data anywhere → fully neutral
    }
    let mean = weighted_sum / covered_weight;
    let k = covered_weight.clamp(0.0, 1.0);
    (50.0 + (mean - 50.0) * k).clamp(0.0, 100.0)
}

#[cfg(test)]
mod composite_tests {
    use super::*;

    fn cat(raw: f64, weight: f64, coverage: f64) -> CategoryScore {
        CategoryScore {
            name: "X".into(),
            raw_score: raw,
            weight,
            weighted_score: raw * weight,
            missing_data: false,
            notes: vec![],
            coverage,
        }
    }

    #[test]
    fn test_shrinkage_pulls_thin_data_toward_neutral() {
        // One fully-covered category at 90, the rest absent (coverage 0).
        let c = [
            cat(90.0, 0.25, 1.0),
            cat(50.0, 0.25, 0.0),
            cat(50.0, 0.20, 0.0),
            cat(50.0, 0.15, 0.0),
            cat(50.0, 0.15, 0.0),
        ];
        let refs: Vec<&CategoryScore> = c.iter().collect();
        // mean of covered = 90; k = Σw·cov = 0.25 → 50 + (90-50)*0.25 = 60. Not 90.
        assert!((composite_from_categories(&refs) - 60.0).abs() < 1e-6);
    }

    #[test]
    fn test_full_coverage_is_not_shrunk() {
        let c = [
            cat(90.0, 0.25, 1.0),
            cat(90.0, 0.25, 1.0),
            cat(90.0, 0.20, 1.0),
            cat(90.0, 0.15, 1.0),
            cat(90.0, 0.15, 1.0),
        ];
        let refs: Vec<&CategoryScore> = c.iter().collect();
        assert!((composite_from_categories(&refs) - 90.0).abs() < 1e-6);
    }

    #[test]
    fn test_no_data_is_neutral() {
        let c = [cat(90.0, 0.25, 0.0), cat(10.0, 0.25, 0.0)];
        let refs: Vec<&CategoryScore> = c.iter().collect();
        assert!((composite_from_categories(&refs) - 50.0).abs() < 1e-6);
    }
}

pub fn calculate_analysis(
    ticker: &str,
    horizon: Horizon,
    yahoo: &YahooData,
    fred: &FredData,
    edgar: &EdgarData,
    finnhub: &FinnhubData,
) -> AnalysisResult {
    let w = get_weights(horizon);

    let is_financial = yahoo.sector.as_deref().map(|s| {
        let s = s.to_lowercase();
        s.contains("financial") || s.contains("banking") || s.contains("real estate")
    }).unwrap_or(false);

    // 1. Valuation Category
    let mut val_missing = false;
    let mut val_scores = Vec::new();
    let mut val_notes = Vec::new();

    // P/E
    match yahoo.pe_ratio {
        Some(pe) => {
            let s = normalize_pe(pe, finnhub.sector_pe_avg);
            val_scores.push(s);
            if let Some(avg) = finnhub.sector_pe_avg {
                if pe < avg {
                    val_notes.push("P/E below sector average — relatively undervalued".to_string());
                } else if pe > avg * 1.5 {
                    val_notes.push("P/E significantly above sector — premium valuation".to_string());
                }
            }
        }
        None => {
            val_missing = true;
        }
    }

    // P/S
    match yahoo.ps_ratio {
        Some(ps) => {
            let s = normalize_ps(ps, finnhub.sector_ps_avg);
            val_scores.push(s);
            if let Some(avg) = finnhub.sector_ps_avg {
                if ps < avg {
                    val_notes.push("P/S below sector average".to_string());
                }
            }
        }
        None => {
            val_missing = true;
        }
    }

    // P/B — sector-aware ceiling applied
    match yahoo.pb_ratio {
        Some(pb) => {
            let s = normalize_pb(pb, yahoo.sector.as_deref());
            val_scores.push(s);
        }
        None => {
            val_missing = true;
        }
    }

    let val_raw = if val_scores.is_empty() {
        50.0
    } else {
        val_scores.iter().sum::<f64>() / val_scores.len() as f64
    };
    let val_weighted = val_raw * w.valuation;

    let mut val_present = 0;
    if yahoo.pe_ratio.is_some() { val_present += 1; }
    if yahoo.ps_ratio.is_some() { val_present += 1; }
    if yahoo.pb_ratio.is_some() { val_present += 1; }
    let val_coverage = val_present as f64 / 3.0;

    let valuation = CategoryScore {
        name: "Valuation".to_string(),
        raw_score: val_raw,
        weight: w.valuation,
        weighted_score: val_weighted,
        missing_data: val_missing,
        notes: val_notes,
        coverage: val_coverage,
    };

    // 2. Fundamentals Category
    let mut fund_missing = false;
    let mut fund_scores = Vec::new();
    let mut fund_notes = Vec::new();

    // Actual EPS Growth YoY — sourced from EDGAR 10-K filings (realized, not forecast)
    // Falls back to Yahoo forward estimate only if EDGAR data is unavailable.
    let eps_growth_source = edgar.actual_eps_growth_yoy.or(yahoo.eps_growth_yoy);
    match eps_growth_source {
        Some(g) => {
            fund_scores.push(normalize_eps_growth(g));
            if g > 25.0 {
                fund_notes.push(format!("Strong EPS growth {:.1}% YoY", g));
            } else if g < -20.0 {
                fund_notes.push(format!("EPS declined {:.1}% YoY — earnings pressure", g));
            }
        }
        None => {
            fund_missing = true;
        }
    }

    // Revenue growth YoY
    match yahoo.revenue_growth_yoy {
        Some(g) => {
            fund_scores.push(normalize_rev_growth(g));
        }
        None => {
            fund_missing = true;
        }
    }

    // FCF (FCF yield score continuously normalized)
    match edgar.fcf_latest {
        Some(fcf) => {
            fund_scores.push(normalize_fcf(fcf, yahoo.market_cap));
            if fcf < 0.0 {
                fund_notes.push("Negative free cash flow — monitor cash runway".to_string());
            }
        }
        None => {
            fund_missing = true;
        }
    }

    // FCF Growth YoY (separate score — counted exactly once)
    match edgar.fcf_growth_yoy {
        Some(g) => {
            fund_scores.push(normalize_fcf_growth(g));
            if g > 20.0 {
                fund_notes.push(format!("FCF growing {:.1}% YoY — strong cash generation trend", g));
            } else if g < -20.0 {
                fund_notes.push(format!("FCF shrinking {:.1}% YoY — monitor cash burn", g));
            }
        }
        None => {
            fund_missing = true;
        }
    }

    // Gross Margin — measures pricing power and cost efficiency (Fundamentals-appropriate)
    // Skip gross margin completely for financial sectors (Issue 5 minor notes)
    if !is_financial {
        match yahoo.gross_margin {
            Some(gm) => {
                fund_scores.push(normalize_gross_margin(gm, yahoo.sector.as_deref()));
                if gm > 60.0 {
                    fund_notes.push(format!("Strong gross margin ({:.1}%) — high pricing power", gm));
                } else if gm < 10.0 {
                    fund_notes.push(format!("Thin gross margin ({:.1}%) — cost pressure risk", gm));
                }
            }
            None => {
                fund_missing = true;
            }
        }
    }

    // Operating Margin — measures operational efficiency
    match yahoo.operating_margin {
        Some(om) => {
            let score = normalize_operating_margin(om, yahoo.sector.as_deref());
            fund_scores.push(score);
            if om < 0.0 {
                fund_notes.push(format!("Negative operating margin ({:.1}%) — unprofitable operations", om));
            }
        }
        None => {
            fund_missing = true;
        }
    }

    let fund_raw = if fund_scores.is_empty() {
        50.0
    } else {
        fund_scores.iter().sum::<f64>() / fund_scores.len() as f64
    };
    let fund_weighted = fund_raw * w.fundamentals;

    let fund_total_metrics = if is_financial { 5 } else { 6 };
    let mut fund_present = 0;
    if eps_growth_source.is_some() { fund_present += 1; }
    if yahoo.revenue_growth_yoy.is_some() { fund_present += 1; }
    if edgar.fcf_latest.is_some() { fund_present += 1; }
    if edgar.fcf_growth_yoy.is_some() { fund_present += 1; }
    if !is_financial && yahoo.gross_margin.is_some() { fund_present += 1; }
    if yahoo.operating_margin.is_some() { fund_present += 1; }
    let fund_coverage = fund_present as f64 / fund_total_metrics as f64;

    let fundamentals = CategoryScore {
        name: "Fundamentals".to_string(),
        raw_score: fund_raw,
        weight: w.fundamentals,
        weighted_score: fund_weighted,
        missing_data: fund_missing,
        notes: fund_notes,
        coverage: fund_coverage,
    };

    // 3. Macro Category
    let mut macro_missing = false;
    let mut macro_scores = Vec::new();
    let mut macro_notes = Vec::new();

    // Fed Funds Rate
    match fred.fed_funds_rate {
        Some(r) => {
            macro_scores.push(normalize_fed_funds(r));
        }
        None => {
            macro_missing = true;
        }
    }

    // CPI
    match fred.cpi_yoy_change {
        Some(cpi) => {
            macro_scores.push(normalize_cpi(cpi, fred.cpi_trend.as_deref()));
            if fred.cpi_trend.as_deref() == Some("Rising") {
                macro_notes.push("Rising CPI may pressure Fed to hold rates higher".to_string());
            }
        }
        None => {
            macro_missing = true;
        }
    }

    // Sector growth score
    match finnhub.sector_growth_score {
        Some(s) => {
            macro_scores.push(s);
        }
        None => {
            macro_missing = true;
        }
    }

    // Yield curve (10Y-2Y spread) — recession signal
    match fred.yield_curve_spread {
        Some(spread) => {
            macro_scores.push(normalize_yield_curve(spread));
            if spread < 0.0 {
                macro_notes.push(format!("Inverted yield curve ({:.2}%) — historical recession warning", spread));
            }
        }
        None => {
            macro_missing = true;
        }
    }

    // Unemployment rate — labor market strength
    match fred.unemployment_rate {
        Some(rate) => {
            macro_scores.push(normalize_unemployment(rate));
            if rate > 6.0 {
                macro_notes.push(format!("Elevated unemployment ({:.1}%) — weakening economy", rate));
            }
        }
        None => {
            macro_missing = true;
        }
    }

    // VIX — market-wide volatility/fear regime
    match fred.vix {
        Some(vix) => {
            macro_scores.push(normalize_vix(vix));
            if vix > 30.0 {
                macro_notes.push(format!("High VIX ({:.0}) — elevated market fear", vix));
            }
        }
        None => {
            macro_missing = true;
        }
    }

    let macro_raw = if macro_scores.is_empty() {
        50.0
    } else {
        macro_scores.iter().sum::<f64>() / macro_scores.len() as f64
    };
    let macro_weighted = macro_raw * w.macro_env;

    let mut macro_present = 0;
    if fred.fed_funds_rate.is_some() { macro_present += 1; }
    if fred.cpi_yoy_change.is_some() { macro_present += 1; }
    if finnhub.sector_growth_score.is_some() { macro_present += 1; }
    if fred.yield_curve_spread.is_some() { macro_present += 1; }
    if fred.unemployment_rate.is_some() { macro_present += 1; }
    if fred.vix.is_some() { macro_present += 1; }
    let macro_coverage = macro_present as f64 / 6.0;

    let macro_env = CategoryScore {
        name: "Macro".to_string(),
        raw_score: macro_raw,
        weight: w.macro_env,
        weighted_score: macro_weighted,
        missing_data: macro_missing,
        notes: macro_notes,
        coverage: macro_coverage,
    };

    // 4. Sentiment Category
    let mut sent_missing = false;
    let mut sent_scores = Vec::new();
    let mut sent_notes = Vec::new();

    // News Sentiment
    match finnhub.news_sentiment_score {
        Some(s) => {
            sent_scores.push(normalize_sentiment(s));
            if s >= 0.4 {
                sent_notes.push(format!("Positive recent news flow (sentiment {:+.2})", s));
            } else if s <= -0.4 {
                sent_notes.push(format!("Negative recent news flow (sentiment {:+.2})", s));
            }
        }
        None => {
            if let Some(fg) = fred.fear_and_greed {
                sent_scores.push(fg);
                sent_notes.push(format!("Using CNN Fear & Greed Index ({:.0}) as sentiment proxy", fg));
            } else {
                sent_missing = true;
            }
        }
    }

    // Insider shares (from Finnhub insider-transactions API). Size the signal by
    // the company's share base so 500k shares means something different for a
    // micro-cap than for a mega-cap. shares_outstanding ≈ market cap / price.
    let shares_outstanding = match (yahoo.market_cap, yahoo.price) {
        (Some(mc), Some(p)) if p > 0.0 => Some(mc / p),
        _ => None,
    };
    match finnhub.insider_net_shares_3m {
        Some(shares) => {
            sent_scores.push(normalize_insider(shares, shares_outstanding));
            if shares > 0 {
                sent_notes.push("Insiders net buying over the last 90 days".to_string());
            } else if shares < 0 {
                sent_notes.push("Insiders net selling over the last 90 days".to_string());
            }
        }
        None => {
            sent_missing = true;
        }
    }

    // Institutional Ownership — continuous linear scale
    match yahoo.institutional_ownership_percent {
        Some(p) => {
            let s = normalize_institutional_ownership(p);
            sent_scores.push(s);
            if p > 80.0 {
                sent_notes.push(format!("High institutional ownership ({:.1}%) — strong smart money presence", p));
            } else if p < 20.0 {
                sent_notes.push(format!("Low institutional ownership ({:.1}%) — limited institutional coverage", p));
            }
        }
        None => {
            sent_missing = true;
        }
    }

    // 52-Week Price Momentum (moved from Valuation)
    match yahoo.price_52w_change_pct {
        Some(price_change) => {
            let s = normalize_price_momentum(price_change);
            sent_scores.push(s);
            if price_change > 50.0 {
                sent_notes.push(format!("Strong 52-week price momentum (+{:.1}%)", price_change));
            } else if price_change < -30.0 {
                sent_notes.push(format!("Significant 52-week price decline ({:.1}%)", price_change));
            }
        }
        None => {
            sent_missing = true;
        }
    }

    let sent_raw = if sent_scores.is_empty() {
        50.0
    } else {
        sent_scores.iter().sum::<f64>() / sent_scores.len() as f64
    };
    let sent_weighted = sent_raw * w.sentiment;

    let mut sent_present = 0;
    if finnhub.news_sentiment_score.is_some() || fred.fear_and_greed.is_some() { sent_present += 1; }
    if finnhub.insider_net_shares_3m.is_some() { sent_present += 1; }
    if yahoo.institutional_ownership_percent.is_some() { sent_present += 1; }
    if yahoo.price_52w_change_pct.is_some() { sent_present += 1; }
    let sent_coverage = sent_present as f64 / 4.0;

    let sentiment = CategoryScore {
        name: "Sentiment".to_string(),
        raw_score: sent_raw,
        weight: w.sentiment,
        weighted_score: sent_weighted,
        missing_data: sent_missing,
        notes: sent_notes,
        coverage: sent_coverage,
    };

    // 5. Risk Category
    let mut risk_missing = false;
    let mut risk_scores = Vec::new();
    let mut risk_notes = Vec::new();

    // Short interest
    match yahoo.short_interest_percent {
        Some(short_pct) => {
            risk_scores.push(normalize_short_interest(short_pct));
            if short_pct > 15.0 {
                risk_notes.push("High short interest — bearish market bet".to_string());
            }
        }
        None => {
            risk_missing = true;
        }
    }

    // Competitor IPOs — only a signal when competitors were actually found. "0 found"
    // is weak evidence (the feed samples a handful of recent IPOs), NOT a confirmed
    // absence of competition, so it must not hand every stock a free max-bullish score
    // or inflate data coverage. Treated as a live-only bonus signal (not in coverage).
    let ipo_count = finnhub.recent_competitor_ipos.len();
    if ipo_count > 0 {
        risk_scores.push(normalize_competitor_ipos(ipo_count));
        risk_notes.push(format!(
            "New competitors entered market recently: {}",
            finnhub.recent_competitor_ipos.join(", ")
        ));
    }

    // Debt to Equity (shared)
    match edgar.debt_to_equity {
        Some(d_to_e) => {
            risk_scores.push(normalize_debt_to_equity(d_to_e, yahoo.sector.as_deref()));
            if d_to_e > 3.0 {
                risk_notes.push("High leverage — review debt maturity schedule".to_string());
            }
        }
        None => {
            risk_missing = true;
        }
    }

    // Interest coverage (shared)
    match edgar.interest_coverage_ratio {
        Some(coverage) => {
            risk_scores.push(normalize_interest_coverage(coverage));
        }
        None => {
            risk_missing = true;
        }
    }

    // Earnings proximity — a near-term volatility catalyst (live-only bonus signal).
    // Contributes to the risk score when present but is NOT counted in coverage, so
    // backtests (where it's always None) are unaffected and longer horizons only feel
    // it when a report is imminent.
    if let Some(days) = finnhub.next_earnings_days {
        risk_scores.push(normalize_earnings_proximity(days));
        if days <= 7 {
            risk_notes.push(format!("Earnings report in {} day(s) — elevated near-term volatility", days));
        }
    }

    let risk_raw = if risk_scores.is_empty() {
        50.0
    } else {
        risk_scores.iter().sum::<f64>() / risk_scores.len() as f64
    };
    let risk_weighted = risk_raw * w.risk;

    // Coverage counts the three reconstructable core risk metrics. Competitor IPOs and
    // earnings proximity are live-only bonus signals (excluded), so a stock isn't
    // credited with data it doesn't really have.
    let mut risk_present = 0;
    if yahoo.short_interest_percent.is_some() { risk_present += 1; }
    if edgar.debt_to_equity.is_some() { risk_present += 1; }
    if edgar.interest_coverage_ratio.is_some() { risk_present += 1; }
    let risk_coverage = risk_present as f64 / 3.0;

    let risk = CategoryScore {
        name: "Risk".to_string(),
        raw_score: risk_raw,
        weight: w.risk,
        weighted_score: risk_weighted,
        missing_data: risk_missing,
        notes: risk_notes,
        coverage: risk_coverage,
    };

    // 6. Confidence Tracking / Data Coverage
    let confidence_score = (val_present + fund_present + macro_present + sent_present + risk_present) as f64
        / (3 + fund_total_metrics + 6 + 4 + 3) as f64 * 100.0;

    // 7. Composite Score Renormalization (shared helper — see composite_from_categories)
    let categories = [&valuation, &fundamentals, &macro_env, &sentiment, &risk];
    let composite = composite_from_categories(&categories);
    let composite_rounded = (composite * 10.0).round() / 10.0;

    // Data-coverage floor: don't emit a directional BUY/SELL on thin data. Below the
    // minimum coverage the call is held to HOLD regardless of score (the shrinkage
    // above already pulls it toward neutral; this is the explicit hard guard).
    let signal = if confidence_score < crate::scoring::signal::MIN_COVERAGE_PCT {
        crate::models::Signal::Hold
    } else {
        get_signal(composite_rounded, horizon)
    };

    AnalysisResult {
        ticker: ticker.to_uppercase(),
        horizon,
        composite_score: composite_rounded,
        signal,
        valuation,
        fundamentals,
        macro_env,
        sentiment,
        risk,
        yahoo: yahoo.clone(),
        fred: fred.clone(),
        edgar: edgar.clone(),
        finnhub: finnhub.clone(),
        generated_at: chrono::Utc::now(),
        confidence_score,
    }
}

