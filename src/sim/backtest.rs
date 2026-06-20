//! Historical backtest replay.
//!
//! This is the "strategy brain": given a series of observations — each a price on
//! a date plus the composite score the algorithm produced *using only data
//! knowable on that date* — it drives the shared [`Portfolio`] and compares the
//! result against buying and holding the same ticker and an optional benchmark
//! (e.g. SPY).
//!
//! It is deliberately data-source-agnostic and pure: the caller is responsible for
//! building point-in-time observations (no look-ahead bias). That separation is
//! what keeps the backtest honest *and* unit-testable.

use std::collections::HashMap;
use chrono::NaiveDate;

use crate::models::Horizon;
use crate::scoring::signal::thresholds;
use crate::sim::{metrics, Portfolio};

/// One point-in-time observation: the price on `date` and the composite score the
/// algorithm produced from data available as of `date`.
#[derive(Debug, Clone)]
pub struct Observation {
    pub date: NaiveDate,
    pub price: f64,
    pub score: f64,
}

/// A dated value series (used for the buy-and-hold and benchmark equity curves).
pub type Series = Vec<(NaiveDate, f64)>;

/// One consecutive out-of-sample window in a walk-forward robustness check.
/// Because the strategy has no parameters fit to the data, every window is
/// out-of-sample by construction — these answer "is the edge consistent across
/// time, or did one lucky stretch carry the whole run?"
#[derive(Debug, Clone)]
pub struct SubPeriod {
    pub label: String,
    pub vora_pct: f64,
    pub spy_pct: f64,
}

#[derive(Debug, Clone)]
pub struct BacktestResult {
    pub ticker: String,
    pub horizon: Horizon,
    /// The strategy portfolio, including its equity curve and trade log.
    pub strategy: Portfolio,
    /// Buy-and-hold the ticker: invest everything at the first price, hold to the end.
    pub buy_hold_curve: Series,
    /// Optional benchmark (e.g. SPY) bought and held over the same window.
    pub benchmark_curve: Option<Series>,

    pub total_return_pct: f64,
    pub cagr_pct: f64,
    pub max_drawdown_pct: f64,
    pub sharpe: f64,
    pub hit_rate_pct: f64,
    pub round_trips: usize,
    pub trades: usize,
    /// Fraction of periods with a non-zero position (diagnoses cash drag / flat curve).
    pub time_in_market_pct: f64,
    /// Average composite score across the window.
    pub avg_score: f64,

    pub buy_hold_return_pct: f64,
    pub benchmark_return_pct: Option<f64>,

    /// Median equity curve across random "monkey portfolio" trials (portfolio mode only).
    pub random_curve: Option<Series>,
    /// Median final return (%) across the random trials.
    pub random_return_pct: Option<f64>,
    /// Percentage of random portfolios the strategy outperformed (skill check).
    pub beats_random_pct: Option<f64>,
    /// Number of random trials run (0 = none).
    pub random_trials: usize,

    /// Walk-forward robustness: VORA vs SPY return in each consecutive sub-window.
    /// Empty for single-ticker backtests.
    pub subperiods: Vec<SubPeriod>,
}

fn year_fraction(start: NaiveDate, end: NaiveDate) -> f64 {
    ((end - start).num_days() as f64 / 365.25).max(1e-9)
}

/// Infer how many observation periods fall in a year (≈12 for monthly), used to
/// annualize the Sharpe ratio.
fn periods_per_year(obs: &[Observation]) -> f64 {
    if obs.len() < 2 {
        return 12.0;
    }
    let total_days = (obs[obs.len() - 1].date - obs[0].date).num_days() as f64;
    if total_days <= 0.0 {
        return 12.0;
    }
    let avg_days = total_days / (obs.len() - 1) as f64;
    (365.25 / avg_days).clamp(1.0, 365.0)
}

fn series_return_pct(series: &Series) -> f64 {
    let vals: Vec<f64> = series.iter().map(|(_, v)| *v).collect();
    metrics::total_return_pct(&vals)
}

/// Minimum rebalance trade size, as a fraction of total value, to avoid churning
/// on tiny target-weight changes (and racking up commissions).
const REBALANCE_BAND: f64 = 0.05;

/// Run a single-ticker backtest with **conviction-scaled** position sizing.
///
/// At each observation the as-of score maps to a target equity weight: fully
/// invested at/above the BUY threshold, fully in cash at/below the SELL threshold,
/// and a linear ramp in between. The portfolio rebalances toward that target
/// (trading only when the gap exceeds `REBALANCE_BAND`). Idle cash compounds at
/// the risk-free rate `rf_annual_pct` (a money-market proxy), so sitting out is
/// not penalized to zero. No look-ahead: every input is point-in-time.
///
/// `benchmark` is an optional `(date, price)` series for a benchmark like SPY.
pub fn run_single_ticker(
    ticker: &str,
    observations: &[Observation],
    horizon: Horizon,
    starting_cash: f64,
    commission_bps: f64,
    benchmark: Option<&[(NaiveDate, f64)]>,
    rf_annual_pct: f64,
) -> Option<BacktestResult> {
    if observations.len() < 2 || starting_cash <= 0.0 {
        return None;
    }

    let mut pf = Portfolio::new(starting_cash, commission_bps);
    let (buy_t, sell_t) = thresholds(horizon);
    let ppy = periods_per_year(observations);
    let rf_period = rf_annual_pct / 100.0 / ppy;

    let mut deployment_sum = 0.0f64;
    let mut counted_periods = 0usize;
    let mut score_sum = 0.0;

    for (i, obs) in observations.iter().enumerate() {
        if obs.price <= 0.0 {
            continue;
        }
        // Idle cash earns the risk-free rate between observations.
        if i > 0 && rf_period != 0.0 {
            pf.cash *= 1.0 + rf_period;
        }

        score_sum += obs.score;
        counted_periods += 1;

        // Map score → target equity weight (conviction ramp between the bands).
        let target_w = if obs.score >= buy_t {
            1.0
        } else if obs.score <= sell_t {
            0.0
        } else {
            (obs.score - sell_t) / (buy_t - sell_t)
        };

        let mut prices = HashMap::new();
        prices.insert(ticker.to_string(), obs.price);

        let invested = pf.holdings_value(&prices);
        let total = pf.cash + invested;
        let desired = total * target_w;
        let diff = desired - invested;
        if diff > total * REBALANCE_BAND {
            pf.buy(obs.date, ticker, obs.price, diff.min(pf.cash), obs.score);
        } else if diff < -total * REBALANCE_BAND {
            pf.sell_value(obs.date, ticker, obs.price, -diff, obs.score);
        }

        // Track actual deployment fraction rather than binary in/out.
        let total_after = pf.total_value(&prices);
        if total_after > 0.0 {
            deployment_sum += pf.holdings_value(&prices) / total_after;
        }
        pf.mark(obs.date, &prices);
    }

    // Buy & hold the ticker.
    let first = &observations[0];
    let bh_shares = starting_cash / first.price;
    let buy_hold_curve: Series = observations.iter().map(|o| (o.date, bh_shares * o.price)).collect();

    // Buy & hold the benchmark over the same window.
    let benchmark_curve = benchmark.and_then(|series| {
        let f = series.first()?;
        if f.1 <= 0.0 {
            return None;
        }
        let shares = starting_cash / f.1;
        Some(series.iter().map(|(dt, px)| (*dt, shares * px)).collect::<Series>())
    });

    let curve = pf.equity_values();
    let years = year_fraction(first.date, observations[observations.len() - 1].date);
    let (hit_rate_pct, round_trips) = pf.hit_rate();
    let returns = metrics::periodic_returns(&curve);
    let time_in_market_pct = if counted_periods > 0 {
        deployment_sum / counted_periods as f64 * 100.0
    } else {
        0.0
    };
    let avg_score = if counted_periods > 0 {
        score_sum / counted_periods as f64
    } else {
        0.0
    };

    Some(BacktestResult {
        ticker: ticker.to_string(),
        horizon,
        total_return_pct: metrics::total_return_pct(&curve),
        cagr_pct: metrics::cagr_pct(&curve, years),
        max_drawdown_pct: metrics::max_drawdown_pct(&curve),
        sharpe: metrics::sharpe(&returns, ppy, rf_annual_pct),
        hit_rate_pct,
        round_trips,
        trades: pf.trades.len(),
        time_in_market_pct,
        avg_score,
        buy_hold_return_pct: series_return_pct(&buy_hold_curve),
        benchmark_return_pct: benchmark_curve.as_ref().map(|c| series_return_pct(c)),
        random_curve: None,
        random_return_pct: None,
        beats_random_pct: None,
        random_trials: 0,
        subperiods: Vec::new(),
        strategy: pf,
        buy_hold_curve,
        benchmark_curve,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn obs(date: &str, price: f64, score: f64) -> Observation {
        Observation { date: d(date), price, score }
    }

    #[test]
    fn test_steady_uptrend_matches_buy_hold() {
        // Always-BUY signal in a rising market: the strategy is fully invested the
        // whole time, so with no commission it must equal buy & hold.
        let observations = vec![
            obs("2020-01-01", 100.0, 85.0),
            obs("2020-02-01", 110.0, 85.0),
            obs("2020-03-01", 121.0, 85.0),
            obs("2020-04-01", 133.0, 85.0),
        ];
        let r = run_single_ticker("AAA", &observations, Horizon::Medium, 10_000.0, 0.0, None, 0.0).unwrap();
        assert!((r.total_return_pct - r.buy_hold_return_pct).abs() < 1e-6);
    }

    #[test]
    fn test_dodging_a_crash_beats_buy_hold() {
        // Algorithm flips to SELL (score 30) before a crash, then re-enters cheaper.
        let observations = vec![
            obs("2020-01-01", 100.0, 85.0), // BUY @100
            obs("2020-02-01", 110.0, 85.0), // hold (already invested)
            obs("2020-03-01", 120.0, 30.0), // SELL @120 → cash
            obs("2020-04-01", 60.0, 30.0),  // stay in cash through the crash
            obs("2020-05-01", 90.0, 85.0),  // BUY back @60? no — re-enters at this step @90
        ];
        let r = run_single_ticker("AAA", &observations, Horizon::Medium, 10_000.0, 0.0, None, 0.0).unwrap();
        // Strategy avoided the drawdown; buy & hold ends at 90 (down from 100).
        assert!(r.total_return_pct > r.buy_hold_return_pct);
        assert!(r.buy_hold_return_pct < 0.0);
        assert!(r.total_return_pct > 0.0);
    }

    #[test]
    fn test_benchmark_curve_is_built() {
        let observations = vec![
            obs("2020-01-01", 100.0, 85.0),
            obs("2020-02-01", 110.0, 85.0),
        ];
        let spy = vec![(d("2020-01-01"), 300.0), (d("2020-02-01"), 330.0)];
        let r = run_single_ticker("AAA", &observations, Horizon::Medium, 10_000.0, 0.0, Some(&spy), 0.0).unwrap();
        assert!(r.benchmark_curve.is_some());
        // SPY rose 10% → benchmark return ≈ 10%
        assert!((r.benchmark_return_pct.unwrap() - 10.0).abs() < 1e-6);
    }
}
