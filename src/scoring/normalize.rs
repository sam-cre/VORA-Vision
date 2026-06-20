pub fn normalize_pe(pe: f64, sector_pe_avg: Option<f64>) -> f64 {
    if pe <= 0.0 {
        return 10.0;
    }
    let ref_pe = sector_pe_avg.unwrap_or(20.0);
    if ref_pe <= 0.0001 {
        return 50.0;
    }
    if pe <= ref_pe {
        100.0 - (pe / ref_pe) * 50.0
    } else {
        (50.0 - ((pe - ref_pe) / (3.0 * ref_pe)) * 50.0).clamp(0.0, 50.0)
    }
}

pub fn normalize_ps(ps: f64, sector_ps_avg: Option<f64>) -> f64 {
    if ps <= 0.0 {
        return 10.0;
    }
    let ref_ps = sector_ps_avg.unwrap_or(3.0);
    if ref_ps <= 0.0001 {
        return 50.0;
    }
    if ps <= ref_ps {
        100.0 - (ps / ref_ps) * 50.0
    } else {
        (50.0 - ((ps - ref_ps) / (3.0 * ref_ps)) * 50.0).clamp(0.0, 50.0)
    }
}

/// Score Price-to-Book with sector context.
/// sector_pb_ceiling: the P/B at which the score reaches 0 for this sector type.
///   - Asset-heavy sectors (banking, real estate, industrials): ceiling = 5
///   - Mixed sectors (consumer, healthcare): ceiling = 15
///   - Intangible-heavy sectors (tech, software, pharma): ceiling = 40
/// 
/// A P/B of half the ceiling scores ~50 (neutral). A P/B of 0 would score 100.
pub fn normalize_pb(pb: f64, sector: Option<&str>) -> f64 {
    if pb < 0.0 {
        // Negative book value — deeply problematic
        return 5.0;
    }

    let ceiling = match sector {
        Some(s) => {
            let s = s.to_lowercase();
            if s.contains("technology") || s.contains("software") || s.contains("biotech")
                || s.contains("pharmaceutical") || s.contains("communication")
            {
                40.0 // intangible-heavy: high P/B is normal
            } else if s.contains("financial") || s.contains("real estate")
                || s.contains("utilities") || s.contains("basic materials")
            {
                5.0 // asset-heavy: P/B above 5 is expensive
            } else {
                15.0 // everything else: moderate ceiling
            }
        }
        None => 15.0, // no sector info: use moderate default
    };

    (100.0 - ((pb / ceiling) * 100.0)).clamp(0.0, 100.0)
}

pub fn normalize_eps_growth(growth: f64) -> f64 {
    (50.0 + (growth * 1.5)).clamp(0.0, 100.0)
}

pub fn normalize_rev_growth(growth: f64) -> f64 {
    (50.0 + (growth * 2.0)).clamp(0.0, 100.0)
}

/// Score Free Cash Flow using FCF yield (FCF / Market Cap) continuously to avoid cliffs.
pub fn normalize_fcf(fcf_m: f64, market_cap: Option<f64>) -> f64 {
    match market_cap {
        Some(mc) if mc > 0.0 => {
            let fcf_yield = fcf_m / (mc / 1_000_000.0);
            if fcf_yield < 0.0 {
                (30.0 + (fcf_yield / 0.10) * 30.0).clamp(0.0, 30.0)
            } else if fcf_yield < 0.05 {
                30.0 + (fcf_yield / 0.05) * 30.0
            } else {
                (60.0 + ((fcf_yield - 0.05) / 0.10) * 40.0).clamp(60.0, 100.0)
            }
        }
        _ => {
            // Fallback if market cap is missing
            if fcf_m < 0.0 { 20.0 } else { 60.0 }
        }
    }
}

pub fn normalize_fcf_growth(growth: f64) -> f64 {
    (50.0 + (growth * 1.5)).clamp(0.0, 100.0)
}

pub fn normalize_debt_to_equity(d_to_e: f64, sector: Option<&str>) -> f64 {
    if d_to_e < 0.0 {
        return 0.0;
    }
    let ceiling = match sector {
        Some(s) => {
            let s = s.to_lowercase();
            if s.contains("financial") || s.contains("banking") || s.contains("real estate") {
                15.0
            } else {
                5.0
            }
        }
        None => 5.0,
    };
    (100.0 - ((d_to_e / ceiling) * 100.0)).clamp(0.0, 100.0)
}

/// Score interest coverage ratio (EBIT / interest expense).
/// Uses a logarithmic curve so differences at the low end (1× vs 3×) matter more
/// than differences at the high end (30× vs 50×), which is financially correct.
/// <0× = 0 (can't cover interest), 1× = ~33, 3× = ~55, 10× = ~80, 30× = ~95, 100× = 100
pub fn normalize_interest_coverage(coverage: f64) -> f64 {
    if coverage < 0.0 {
        return 0.0;
    }
    if coverage < 1.0 {
        // Below 1× is dangerous: linearly scale 0–1× to scores 0–30
        return coverage * 30.0;
    }
    // For 1× and above: use log base 100 to spread the scale.
    // log100(1) = 0, log100(10) = 0.5, log100(100) = 1.0
    let log_score = coverage.log(100.0_f64).clamp(0.0, 1.0);
    (30.0 + (log_score * 70.0)).clamp(0.0, 100.0)
}

/// Score the Federal Funds Rate.
/// Sweet spot: ~1–3% (accommodative without emergency signaling) = ~80–90 score.
/// Near 0%: crisis territory — score decreases toward 60.
/// Above 5%: restrictive — score decreases toward 0 at 10%.
/// Above 8%: historically recessionary, score floors at 0.
pub fn normalize_fed_funds(rate: f64) -> f64 {
    if rate < 0.0 {
        return 50.0; // negative rates are unusual; treat as neutral
    }
    if rate <= 1.0 {
        // Very low: could be crisis response; not purely good
        50.0 + (rate * 15.0) // 0% → 50, 1% → 65
    } else if rate <= 3.0 {
        // Sweet spot: accommodative
        65.0 + ((rate - 1.0) / 2.0 * 25.0) // 1% → 65, 3% → 90
    } else if rate <= 6.0 {
        // Tightening: increasingly restrictive
        90.0 - ((rate - 3.0) / 3.0 * 60.0) // 3% → 90, 6% → 30
    } else {
        // Very tight: recession risk
        (30.0 - ((rate - 6.0) * 8.0)).clamp(0.0, 30.0)
    }
}

pub fn normalize_cpi(cpi_change: f64, trend: Option<&str>) -> f64 {
    let target = 2.0;
    let deviation = (cpi_change - target).abs();
    // Penalize proportionally above 1 point deviation; cap penalty at 50 pts
    let base = (100.0 - (deviation * 20.0).min(100.0)).clamp(0.0, 100.0);
    match trend {
        Some("Falling") if cpi_change > target => (base + 10.0).clamp(0.0, 100.0),
        Some("Rising") if cpi_change < target => (base + 10.0).clamp(0.0, 100.0), // approaching target
        Some("Rising") => (base - 10.0).clamp(0.0, 100.0),
        Some("Falling") => (base - 5.0).clamp(0.0, 100.0),
        _ => base,
    }
}

pub fn normalize_sentiment(sentiment: f64) -> f64 {
    (((sentiment + 1.0) / 2.0) * 100.0).clamp(0.0, 100.0)
}

/// Score net insider buying/selling. When share count is given context by
/// `shares_outstanding`, the signal scales by the *fraction of the float*
/// transacted (size-aware): 500k shares is a loud signal for a micro-cap and
/// noise for a mega-cap. ~0.20% of the float maps to a full ±40-point swing.
/// Falls back to a raw log-share-count magnitude when size is unknown.
pub fn normalize_insider(net_shares: i64, shares_outstanding: Option<f64>) -> f64 {
    if net_shares == 0 {
        return 50.0;
    }
    let sign = if net_shares > 0 { 1.0 } else { -1.0 };
    let magnitude = match shares_outstanding {
        Some(so) if so > 0.0 => {
            let pct = (net_shares.abs() as f64 / so) * 100.0;
            (pct / 0.20).min(1.0) * 40.0
        }
        _ => (net_shares.abs() as f64).log10().clamp(0.0, 6.0) / 6.0 * 40.0,
    };
    (50.0 + sign * magnitude).clamp(10.0, 90.0)
}

pub fn normalize_short_interest(short_pct: f64) -> f64 {
    (100.0 - (short_pct * 5.0)).clamp(0.0, 100.0)
}

/// Score competitive pressure from recent same-sector IPOs.
/// 0 competitors = no new competitive entrants (good).
/// Each additional competitor costs 15 points; clamped so 5+ competitors floor at 25.
/// Note: the data layer samples up to 5 recent NYSE/NASDAQ IPOs before sector matching,
/// so `count` here is the same-sector subset (0..=5).
pub fn normalize_competitor_ipos(count: usize) -> f64 {
    if count == 0 {
        return 100.0;
    }
    let score = 100.0 - (count as f64 * 15.0);
    score.clamp(25.0, 100.0)
}

/// Score 52-week price momentum. 0% change = 50 (neutral).
/// +25% = 100 (strong bull). -25% = 0 (strong bear).
pub fn normalize_price_momentum(change_pct: f64) -> f64 {
    (50.0 + (change_pct * 2.0)).clamp(0.0, 100.0)
}

/// Score gross margin on a sector-aware scale.
pub fn normalize_gross_margin(gm_pct: f64, sector: Option<&str>) -> f64 {
    if gm_pct < 0.0 {
        return 0.0;
    }
    let ceiling = match sector {
        Some(s) => {
            let s = s.to_lowercase();
            if s.contains("technology") || s.contains("software") || s.contains("biotech")
                || s.contains("pharmaceutical") || s.contains("communication")
            {
                85.0 // Intangible-heavy: high margin expected
            } else if s.contains("financial") || s.contains("real estate")
                || s.contains("utilities") || s.contains("basic materials")
                || s.contains("consumer defensive") || s.contains("retail") || s.contains("food")
            {
                35.0 // Thin margin / asset-heavy: lower margin is normal
            } else {
                60.0 // Mixed / others
            }
        }
        None => 60.0,
    };
    ((gm_pct / ceiling) * 100.0).clamp(0.0, 100.0)
}

/// Score operating margin with sector context.
pub fn normalize_operating_margin(om_pct: f64, sector: Option<&str>) -> f64 {
    let (floor, ceiling) = match sector {
        Some(s) => {
            let s = s.to_lowercase();
            if s.contains("technology") || s.contains("software") || s.contains("biotech")
                || s.contains("pharmaceutical") || s.contains("communication")
            {
                (-10.0, 35.0) // Intangible-heavy
            } else if s.contains("financial") || s.contains("real estate")
                || s.contains("utilities") || s.contains("basic materials")
                || s.contains("consumer defensive") || s.contains("retail") || s.contains("food")
            {
                (-5.0, 12.0) // Asset-heavy / thin-margin retail
            } else {
                (-10.0, 20.0) // Mixed
            }
        }
        None => (-10.0, 20.0),
    };

    if om_pct <= floor {
        0.0
    } else if om_pct >= ceiling {
        100.0
    } else {
        ((om_pct - floor) / (ceiling - floor)) * 100.0
    }
}

/// Score institutional ownership on a continuous scale.
pub fn normalize_institutional_ownership(p: f64) -> f64 {
    (20.0 + (p * 0.9)).clamp(20.0, 92.0)
}

/// Score sector growth based on peer performance.
pub fn normalize_sector_growth(g: f64) -> f64 {
    let score = 50.0 + (g * 1.5);
    score.clamp(0.0, 100.0)
}

/// Score the 10Y-2Y Treasury yield-curve spread (%).
/// An inverted curve (negative spread) is one of the most reliable recession
/// warnings, so it scores low. A normal upward slope scores high.
///   -1.0% (deeply inverted) → 10, 0% (flat) → 40, +1.0% → 70, +2.0%+ → 100
pub fn normalize_yield_curve(spread: f64) -> f64 {
    (40.0 + spread * 30.0).clamp(0.0, 100.0)
}

/// Score the CBOE VIX (market-wide volatility/fear). Lower = calmer = better.
///   12 → 100, 20 → 76, 30 → 46, 40 → 16. Capped at 100 below 12.
pub fn normalize_vix(vix: f64) -> f64 {
    (100.0 - (vix - 12.0) * 3.0).clamp(0.0, 100.0)
}

/// Score the U-3 unemployment rate (%). A tight labor market (low rate) is a
/// macro tailwind; a rising rate signals a weakening economy.
///   3.5% → 100, 4.5% → 88, 6% → 70, 8% → 46, 10% → 22
pub fn normalize_unemployment(rate: f64) -> f64 {
    (100.0 - (rate - 3.5).max(0.0) * 12.0).clamp(0.0, 100.0)
}

/// Score proximity to the next scheduled earnings report (a near-term volatility
/// event). Pure near-term penalty: imminent earnings lower the risk score; once
/// more than a week out it's neutral, so longer horizons are unaffected.
///   ≤2 days → 30, 3-7 days → 42, otherwise → 50 (neutral)
pub fn normalize_earnings_proximity(days_until: i64) -> f64 {
    if days_until < 0 {
        return 50.0; // unknown / already reported
    }
    if days_until <= 2 {
        30.0
    } else if days_until <= 7 {
        42.0
    } else {
        50.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pe_normalization() {
        // Under ref PE: linear decay
        assert!((normalize_pe(10.0, Some(20.0)) - 75.0).abs() < 1e-5);
        // At ref PE: 50.0
        assert!((normalize_pe(20.0, Some(20.0)) - 50.0).abs() < 1e-5);
        // Above ref PE: softer decay (e.g. at 2x ref PE, score should be > 0.0)
        let score_2x = normalize_pe(40.0, Some(20.0));
        assert!(score_2x > 0.0);
        assert!((score_2x - 33.33333).abs() < 1e-2);
        // Below zero: returns 10.0
        assert_eq!(normalize_pe(-5.0, Some(20.0)), 10.0);
    }

    #[test]
    fn test_ps_normalization() {
        assert!((normalize_ps(1.5, Some(3.0)) - 75.0).abs() < 1e-5);
        assert!((normalize_ps(3.0, Some(3.0)) - 50.0).abs() < 1e-5);
        let score_2x = normalize_ps(6.0, Some(3.0));
        assert!(score_2x > 0.0);
        assert!((score_2x - 33.33333).abs() < 1e-2);
    }

    #[test]
    fn test_fcf_normalization() {
        // Market cap = 1,000.0M (1,000,000,000)
        let mc = Some(1_000_000_000.0);
        // FCF = 50.0M -> 5% yield -> 60 score
        assert!((normalize_fcf(50.0, mc) - 60.0).abs() < 1e-5);
        // FCF = 0.0M -> 0% yield -> 30 score
        assert!((normalize_fcf(0.0, mc) - 30.0).abs() < 1e-5);
        // FCF = 150.0M -> 15% yield -> 100 score
        assert!((normalize_fcf(150.0, mc) - 100.0).abs() < 1e-5);
        // FCF = -50.0M -> -5% yield -> 15 score
        assert!((normalize_fcf(-50.0, mc) - 15.0).abs() < 1e-5);
    }

    #[test]
    fn test_debt_to_equity_normalization() {
        // Non-financial sector (ceiling = 5.0)
        assert!((normalize_debt_to_equity(2.5, Some("Technology")) - 50.0).abs() < 1e-5);
        // Financial sector (ceiling = 15.0)
        assert!((normalize_debt_to_equity(7.5, Some("Financial Services")) - 50.0).abs() < 1e-5);
    }

    #[test]
    fn test_gross_margin_normalization() {
        // Tech (ceiling = 85.0)
        assert!((normalize_gross_margin(42.5, Some("Software")) - 50.0).abs() < 1e-5);
        // Retail/Defensive (ceiling = 35.0)
        assert!((normalize_gross_margin(17.5, Some("Consumer Defensive")) - 50.0).abs() < 1e-5);
    }

    #[test]
    fn test_operating_margin_normalization() {
        // Tech: floor -10%, ceiling 35% -> midpoint 12.5% should be 50.0
        assert!((normalize_operating_margin(12.5, Some("Software")) - 50.0).abs() < 1e-5);
        // Retail: floor -5%, ceiling 12% -> midpoint 3.5% should be 50.0
        assert!((normalize_operating_margin(3.5, Some("Retail")) - 50.0).abs() < 1e-5);
    }

    #[test]
    fn test_institutional_ownership_normalization() {
        assert!((normalize_institutional_ownership(0.0) - 20.0).abs() < 1e-5);
        assert!((normalize_institutional_ownership(50.0) - 65.0).abs() < 1e-5);
        assert!((normalize_institutional_ownership(80.0) - 92.0).abs() < 1e-5);
        assert!((normalize_institutional_ownership(100.0) - 92.0).abs() < 1e-5);
    }

    #[test]
    fn test_insider_normalization() {
        // Size-aware: 0.20% of a 1M-share float bought → full +40 swing → 90.
        assert!((normalize_insider(2_000, Some(1_000_000.0)) - 90.0).abs() < 1e-6);
        // Same magnitude sold → 10.
        assert!((normalize_insider(-2_000, Some(1_000_000.0)) - 10.0).abs() < 1e-6);
        // Tiny relative to a mega-cap float → near neutral.
        assert!(normalize_insider(2_000, Some(1_000_000_000.0)) < 55.0);
        // Zero → neutral; no size context → log-scale fallback (positive > 50).
        assert_eq!(normalize_insider(0, Some(1_000_000.0)), 50.0);
        assert!(normalize_insider(1_000, None) > 50.0);
    }

    #[test]
    fn test_sector_growth_normalization() {
        assert!((normalize_sector_growth(0.0) - 50.0).abs() < 1e-5);
        assert!((normalize_sector_growth(10.0) - 65.0).abs() < 1e-5);
        assert!((normalize_sector_growth(-10.0) - 35.0).abs() < 1e-5);
    }

    #[test]
    fn test_yield_curve_normalization() {
        assert!((normalize_yield_curve(0.0) - 40.0).abs() < 1e-5);   // flat
        assert!((normalize_yield_curve(1.0) - 70.0).abs() < 1e-5);   // normal slope
        assert!((normalize_yield_curve(-1.0) - 10.0).abs() < 1e-5);  // inverted
        assert!((normalize_yield_curve(3.0) - 100.0).abs() < 1e-5);  // steep, clamped
    }

    #[test]
    fn test_vix_normalization() {
        assert!((normalize_vix(12.0) - 100.0).abs() < 1e-5);
        assert!((normalize_vix(20.0) - 76.0).abs() < 1e-5);
        assert!((normalize_vix(30.0) - 46.0).abs() < 1e-5);
        assert!((normalize_vix(8.0) - 100.0).abs() < 1e-5); // calm, clamped
    }

    #[test]
    fn test_unemployment_normalization() {
        assert!((normalize_unemployment(3.5) - 100.0).abs() < 1e-5);
        assert!((normalize_unemployment(6.0) - 70.0).abs() < 1e-5);
        assert!((normalize_unemployment(2.0) - 100.0).abs() < 1e-5); // very tight, clamped
    }

    #[test]
    fn test_earnings_proximity_normalization() {
        assert_eq!(normalize_earnings_proximity(1), 30.0);   // imminent
        assert_eq!(normalize_earnings_proximity(5), 42.0);   // this week
        assert_eq!(normalize_earnings_proximity(30), 50.0);  // far out — neutral
        assert_eq!(normalize_earnings_proximity(-1), 50.0);  // unknown — neutral
    }
}

