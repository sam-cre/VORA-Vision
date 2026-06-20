//! Performance metrics for an equity curve.
//!
//! All functions here are pure and unit-tested. They operate on a plain slice of
//! portfolio values (the equity curve) so they work identically for a historical
//! backtest and a forward paper-trading run.

/// Total return over the curve: `last / first - 1`, expressed as a percentage.
pub fn total_return_pct(curve: &[f64]) -> f64 {
    if curve.len() < 2 || curve[0].abs() < f64::EPSILON {
        return 0.0;
    }
    (curve[curve.len() - 1] / curve[0] - 1.0) * 100.0
}

/// Compound annual growth rate (%), given the number of years the curve spans.
pub fn cagr_pct(curve: &[f64], years: f64) -> f64 {
    if curve.len() < 2 || curve[0].abs() < f64::EPSILON || years <= 0.0 {
        return 0.0;
    }
    let ratio = curve[curve.len() - 1] / curve[0];
    if ratio <= 0.0 {
        return -100.0;
    }
    (ratio.powf(1.0 / years) - 1.0) * 100.0
}

/// Maximum peak-to-trough drawdown (%), returned as a non-positive number.
/// e.g. a fall from 120 to 60 yields -50.0.
pub fn max_drawdown_pct(curve: &[f64]) -> f64 {
    let mut peak = f64::MIN;
    let mut max_dd = 0.0;
    for &v in curve {
        if v > peak {
            peak = v;
        }
        if peak > 0.0 {
            let dd = (v - peak) / peak;
            if dd < max_dd {
                max_dd = dd;
            }
        }
    }
    max_dd * 100.0
}

/// Convert an equity curve into period-over-period simple returns.
pub fn periodic_returns(curve: &[f64]) -> Vec<f64> {
    curve
        .windows(2)
        .filter(|w| w[0].abs() > f64::EPSILON)
        .map(|w| w[1] / w[0] - 1.0)
        .collect()
}

/// Annualized Sharpe ratio from periodic returns.
/// `periods_per_year` e.g. 12 for monthly rebalancing; `rf_annual_pct` is the
/// annual risk-free rate (use the average Fed Funds rate over the window).
pub fn sharpe(periodic_returns: &[f64], periods_per_year: f64, rf_annual_pct: f64) -> f64 {
    if periodic_returns.len() < 2 || periods_per_year <= 0.0 {
        return 0.0;
    }
    let rf_period = rf_annual_pct / 100.0 / periods_per_year;
    let excess: Vec<f64> = periodic_returns.iter().map(|r| r - rf_period).collect();
    let mean = excess.iter().sum::<f64>() / excess.len() as f64;
    let var = excess.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (excess.len() as f64 - 1.0);
    let sd = var.sqrt();
    if sd.abs() < 1e-12 {
        return 0.0;
    }
    (mean / sd) * periods_per_year.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_total_return() {
        assert!((total_return_pct(&[100.0, 200.0]) - 100.0).abs() < 1e-9);
        assert!((total_return_pct(&[100.0, 50.0]) + 50.0).abs() < 1e-9);
        assert_eq!(total_return_pct(&[100.0]), 0.0); // too short
    }

    #[test]
    fn test_cagr() {
        // Doubling in 1 year = 100% CAGR
        assert!((cagr_pct(&[100.0, 200.0], 1.0) - 100.0).abs() < 1e-6);
        // Doubling over 2 years ≈ 41.42% CAGR
        assert!((cagr_pct(&[100.0, 200.0], 2.0) - 41.4213562).abs() < 1e-4);
    }

    #[test]
    fn test_max_drawdown() {
        // peak 120 → trough 60 = -50%
        assert!((max_drawdown_pct(&[100.0, 120.0, 60.0, 90.0]) + 50.0).abs() < 1e-9);
        // monotonically rising = no drawdown
        assert_eq!(max_drawdown_pct(&[100.0, 110.0, 120.0]), 0.0);
    }

    #[test]
    fn test_periodic_returns() {
        let r = periodic_returns(&[100.0, 110.0, 99.0]);
        assert!((r[0] - 0.10).abs() < 1e-9);
        assert!((r[1] + 0.10).abs() < 1e-9);
    }

    #[test]
    fn test_sharpe_constant_returns_is_zero() {
        // No volatility → undefined/zero Sharpe (guarded)
        assert_eq!(sharpe(&[0.01, 0.01, 0.01], 12.0, 0.0), 0.0);
    }

    #[test]
    fn test_sharpe_positive() {
        let returns = [0.02, 0.01, 0.03, -0.01, 0.02];
        let s = sharpe(&returns, 12.0, 0.0);
        assert!(s > 0.0);
    }
}
