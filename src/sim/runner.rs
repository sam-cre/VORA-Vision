//! Orchestrates a full historical backtest: fetch point-in-time data once, build
//! month-by-month observations (price + as-of score), then replay through the
//! shared portfolio engine and compare against buy-and-hold + SPY.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::{Duration, NaiveDate, Utc};
use serde_json::Value;

use crate::data::edgar::{fetch_companyfacts, fetch_sector};
use crate::models::Horizon;
use crate::scoring::signal::thresholds;
use crate::sim::backtest::{run_single_ticker, BacktestResult, Observation, SubPeriod};
use crate::sim::calibrate::{build_calibration, Calibration};
use crate::sim::data::{
    fetch_price_history, price_on_or_before, score_as_of, trailing_annual_vol, MacroSeries,
};
use crate::sim::{metrics, Portfolio};

/// Default starting capital and round-trip commission for backtests/paper trades.
pub const DEFAULT_START_CASH: f64 = 10_000.0;
pub const DEFAULT_COMMISSION_BPS: f64 = 10.0; // 0.10% per side
/// Default number of names to hold in portfolio mode.
pub const DEFAULT_TOP_N: usize = 5;

const TARGET_VOL: f64 = 0.30; // annualized per-name vol target for the risk guard
const VOL_WINDOW: usize = 6; // months of returns used to estimate volatility
const REBALANCE_BAND: f64 = 0.03; // no-trade band (fraction of total value)
const RANDOM_TRIALS: usize = 300; // monkey-portfolio Monte Carlo trials
const RANDOM_SEED: u64 = 0x9E37_79B9_7F4A_7C15; // fixed seed → reproducible random benchmark

/// Tiny dependency-free xorshift64 PRNG (only used for the random benchmark).
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// Run a single-ticker historical backtest over the trailing `years`.
pub async fn run_backtest(
    ticker: &str,
    horizon: Horizon,
    years: i64,
    starting_cash: f64,
    commission_bps: f64,
) -> Result<BacktestResult> {
    let ticker_u = ticker.trim().to_uppercase();
    let end = Utc::now().date_naive();
    let start = end - Duration::days(365 * years.max(1) + 30);

    // Prices for the subject and the SPY benchmark.
    let price_history = fetch_price_history(&ticker_u, start, end).await?;
    let spy = fetch_price_history("SPY", start, end).await.ok();

    // Historical fundamentals (all filings; filtered to as-of date during scoring).
    let facts = fetch_companyfacts(&ticker_u).await?;
    let us_gaap = facts["facts"]["us-gaap"].clone();
    let dei = facts["facts"]["dei"].clone();
    // Sector (SEC SIC) so backtest scoring uses the same sector-aware normalizers as live.
    let sector = fetch_sector(&ticker_u).await;

    // Macro series, fetched with a ~14-month lookback so CPI YoY is available from
    // the very first observation. Missing FRED key → empty → macro omitted (honest).
    let fred_start = start - Duration::days(420);
    let macro_series = MacroSeries::fetch(fred_start, end).await;

    // Build observations: for each price point, score using only as-of data.
    let mut observations = Vec::with_capacity(price_history.len());
    for (date, price) in &price_history {
        let score = score_as_of(
            &ticker_u, &us_gaap, &dei, sector.as_deref(), &macro_series, &price_history, *date, *price, horizon,
        );
        observations.push(Observation { date: *date, price: *price, score });
    }

    // Risk-free rate for Sharpe = average Fed Funds over the window.
    let rf = macro_series.avg_fed();

    run_single_ticker(
        &ticker_u,
        &observations,
        horizon,
        starting_cash,
        commission_bps,
        spy.as_deref(),
        rf,
    )
    .ok_or_else(|| anyhow!("not enough data to backtest {}", ticker_u))
}

struct Asset {
    ticker: String,
    prices: Vec<(NaiveDate, f64)>,
    us_gaap: Value,
    dei: Value,
    sector: Option<String>,
}

/// Monte Carlo "monkey portfolio": each month hold `top_n` randomly chosen names
/// (equal-weight, fully invested), rebalanced monthly with the same commission and
/// risk-free cash growth as the strategy. Returns `(final_return_pct, equity_curve)`
/// for each trial. Selection is the only difference vs. the VORA portfolio — so the
/// spread of these trials is the fair "is the signal better than chance?" baseline.
fn run_random_benchmark(
    assets: &[Asset],
    grid: &[NaiveDate],
    cash: f64,
    commission_bps: f64,
    top_n: usize,
    rf_period: f64,
) -> Vec<(f64, Vec<f64>)> {
    let mut rng = Rng::new(RANDOM_SEED);
    let mut out = Vec::with_capacity(RANDOM_TRIALS);
    for _ in 0..RANDOM_TRIALS {
        let mut pf = Portfolio::new(cash, commission_bps);
        for (i, date) in grid.iter().enumerate() {
            if i > 0 && rf_period != 0.0 {
                pf.cash *= 1.0 + rf_period;
            }
            // Assets with a valid price as of this date.
            let mut prices_map: HashMap<String, f64> = HashMap::new();
            let mut avail: Vec<usize> = Vec::new();
            for (idx, a) in assets.iter().enumerate() {
                if let Some(p) = price_on_or_before(&a.prices, *date) {
                    if p > 0.0 {
                        prices_map.insert(a.ticker.clone(), p);
                        avail.push(idx);
                    }
                }
            }
            if avail.is_empty() {
                pf.mark(*date, &prices_map);
                continue;
            }
            // Pick up to top_n distinct names at random (partial Fisher-Yates).
            let k = top_n.min(avail.len());
            for s in 0..k {
                let j = s + rng.below(avail.len() - s);
                avail.swap(s, j);
            }
            let w = 1.0 / k as f64;
            let mut targets: HashMap<String, f64> = HashMap::new();
            for &idx in &avail[..k] {
                targets.insert(assets[idx].ticker.clone(), w);
            }
            pf.rebalance_to(*date, &prices_map, &targets, &HashMap::new(), REBALANCE_BAND);
            pf.mark(*date, &prices_map);
        }
        let curve = pf.equity_values();
        out.push((metrics::total_return_pct(&curve), curve));
    }
    out
}

/// Run a multi-ticker **portfolio** backtest: each month, score the whole
/// universe, hold the top-`top_n` names (conviction-weighted), trim each holding
/// by a volatility guard, and rebalance. Benchmarks are an equal-weight buy &
/// hold of the same universe (reported as "Buy & Hold") and SPY. Returns a
/// `BacktestResult` so it reuses the backtest screen.
pub async fn run_portfolio_backtest(
    tickers: &[String],
    horizon: Horizon,
    years: i64,
    starting_cash: f64,
    commission_bps: f64,
    top_n: usize,
) -> Result<BacktestResult> {
    let end = Utc::now().date_naive();
    let start = end - Duration::days(365 * years.max(1) + 30);

    // SPY provides both the benchmark and the monthly rebalance grid.
    let spy = fetch_price_history("SPY", start, end).await?;
    let grid: Vec<NaiveDate> = spy.iter().map(|(d, _)| *d).collect();
    if grid.len() < 2 {
        return Err(anyhow!("insufficient benchmark history"));
    }

    let fred_start = start - Duration::days(420);
    let macro_series = MacroSeries::fetch(fred_start, end).await;

    // Fetch per-ticker price history + fundamentals (skip any that fail).
    let mut assets: Vec<Asset> = Vec::new();
    for t in tickers {
        let tu = t.trim().to_uppercase();
        let prices = match fetch_price_history(&tu, start, end).await {
            Ok(p) => p,
            Err(_) => continue,
        };
        let facts = match fetch_companyfacts(&tu).await {
            Ok(f) => f,
            Err(_) => continue,
        };
        let sector = fetch_sector(&tu).await;
        assets.push(Asset {
            ticker: tu,
            prices,
            us_gaap: facts["facts"]["us-gaap"].clone(),
            dei: facts["facts"]["dei"].clone(),
            sector,
        });
    }
    if assets.is_empty() {
        return Err(anyhow!("no tickers had usable data"));
    }

    let top_n = top_n.clamp(1, assets.len());
    let (_buy_t, sell_t) = thresholds(horizon);
    let rf = macro_series.avg_fed();
    let rf_period = rf / 100.0 / 12.0;

    let mut pf = Portfolio::new(starting_cash, commission_bps);
    let mut score_sum = 0.0;
    let mut score_count = 0usize;
    let mut deployment_sum = 0.0f64; // track actual fraction invested each period

    for (i, date) in grid.iter().enumerate() {
        if i > 0 && rf_period != 0.0 {
            pf.cash *= 1.0 + rf_period;
        }

        // Score every asset as of this date (point-in-time).
        let mut prices_map: HashMap<String, f64> = HashMap::new();
        let mut cand: Vec<(String, f64, f64)> = Vec::new(); // (ticker, score, vol_scale)
        for a in &assets {
            let price = match price_on_or_before(&a.prices, *date) {
                Some(p) if p > 0.0 => p,
                _ => continue,
            };
            prices_map.insert(a.ticker.clone(), price);
            let score =
                score_as_of(&a.ticker, &a.us_gaap, &a.dei, a.sector.as_deref(), &macro_series, &a.prices, *date, price, horizon);
            score_sum += score;
            score_count += 1;
            let vol_scale = trailing_annual_vol(&a.prices, *date, VOL_WINDOW)
                .map(|v| (TARGET_VOL / v).min(1.0))
                .unwrap_or(1.0);
            cand.push((a.ticker.clone(), score, vol_scale));
        }

        // Select the top-N eligible names (score above the SELL band), ranked by score.
        let mut eligible: Vec<(String, f64, f64)> =
            cand.into_iter().filter(|(_, s, _)| *s >= sell_t).collect();
        eligible.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        eligible.truncate(top_n);

        // Weight each selected name by conviction (score above neutral 50) and tilt
        // toward lower-volatility names (inverse-vol scaling), then RENORMALIZE so the
        // book stays fully invested whenever names qualify. The eligibility filter
        // (score >= sell band) is what moves the strategy to cash — not the vol guard.
        // Separating those two jobs removes the unintended cash drag that previously
        // left ~30–45% idle while still reporting "100% time in market".
        let raw: Vec<(String, f64, f64)> = eligible
            .iter()
            .map(|(t, s, vol_scale)| (t.clone(), *s, (s - 50.0).max(0.0) * vol_scale))
            .collect();
        let raw_sum: f64 = raw.iter().map(|(_, _, w)| *w).sum();
        let mut targets: HashMap<String, f64> = HashMap::new();
        let mut score_map: HashMap<String, f64> = HashMap::new();
        if raw_sum > 0.0 {
            for (t, s, w) in &raw {
                targets.insert(t.clone(), w / raw_sum); // inverse-vol tilt, sums to 1.0
                score_map.insert(t.clone(), *s);
            }
        }

        pf.rebalance_to(*date, &prices_map, &targets, &score_map, REBALANCE_BAND);

        // Track actual deployment fraction (holdings / total value) not just binary invested.
        let total = pf.total_value(&prices_map);
        if total > 0.0 {
            deployment_sum += pf.holdings_value(&prices_map) / total;
        }

        pf.mark(*date, &prices_map);
    }

    // Equal-weight buy & hold of the universe (the "Buy & Hold" benchmark).
    let per = starting_cash / assets.len() as f64;
    let bh_shares: Vec<f64> = assets
        .iter()
        .map(|a| {
            price_on_or_before(&a.prices, grid[0])
                .filter(|p| *p > 0.0)
                .map(|p| per / p)
                .unwrap_or(0.0)
        })
        .collect();
    let buy_hold_curve: Vec<(NaiveDate, f64)> = grid
        .iter()
        .map(|d| {
            let v: f64 = assets
                .iter()
                .zip(&bh_shares)
                .map(|(a, sh)| sh * price_on_or_before(&a.prices, *d).unwrap_or(0.0))
                .sum();
            (*d, v)
        })
        .collect();

    let spy_shares = starting_cash / spy[0].1;
    let benchmark_curve: Vec<(NaiveDate, f64)> = spy.iter().map(|(d, p)| (*d, spy_shares * p)).collect();

    let curve = pf.equity_values();
    let years_f = ((grid[grid.len() - 1] - grid[0]).num_days() as f64 / 365.25).max(1e-9);
    let (hit_rate_pct, round_trips) = pf.hit_rate();
    let returns = metrics::periodic_returns(&curve);
    let bh_vals: Vec<f64> = buy_hold_curve.iter().map(|(_, v)| *v).collect();
    let spy_vals: Vec<f64> = benchmark_curve.iter().map(|(_, v)| *v).collect();

    let vora_total_return = metrics::total_return_pct(&curve);

    // Random "monkey portfolio" benchmark: how does VORA's selection rank vs. chance?
    let mut random_curve = None;
    let mut random_return_pct = None;
    let mut beats_random_pct = None;
    let mut random_trials = 0usize;
    let mut trials = run_random_benchmark(&assets, &grid, starting_cash, commission_bps, top_n, rf_period);
    if !trials.is_empty() {
        trials.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        random_trials = trials.len();
        let mid = trials.len() / 2;
        random_return_pct = Some(trials[mid].0);
        let beats = trials.iter().filter(|(r, _)| *r < vora_total_return).count();
        beats_random_pct = Some(beats as f64 / trials.len() as f64 * 100.0);
        random_curve = Some(
            grid.iter().zip(trials[mid].1.iter()).map(|(d, v)| (*d, *v)).collect::<Vec<_>>(),
        );
    }

    // Walk-forward robustness: VORA vs SPY in 4 consecutive windows.
    let subperiods = build_subperiods(&grid, &curve, &spy_vals, 4);

    let names: Vec<&str> = assets.iter().map(|a| a.ticker.as_str()).collect();
    let label = format!("PORTFOLIO [{}] top {}", names.join(" "), top_n);

    Ok(BacktestResult {
        ticker: label,
        horizon,
        total_return_pct: vora_total_return,
        cagr_pct: metrics::cagr_pct(&curve, years_f),
        max_drawdown_pct: metrics::max_drawdown_pct(&curve),
        sharpe: metrics::sharpe(&returns, 12.0, rf),
        hit_rate_pct,
        round_trips,
        trades: pf.trades.len(),
        time_in_market_pct: if grid.is_empty() { 0.0 } else { deployment_sum / grid.len() as f64 * 100.0 },
        avg_score: if score_count > 0 { score_sum / score_count as f64 } else { 0.0 },
        buy_hold_return_pct: metrics::total_return_pct(&bh_vals),
        benchmark_return_pct: Some(metrics::total_return_pct(&spy_vals)),
        random_curve,
        random_return_pct,
        beats_random_pct,
        random_trials,
        subperiods,
        strategy: pf,
        buy_hold_curve,
        benchmark_curve: Some(benchmark_curve),
    })
}

/// Split the run into `k` consecutive windows and report VORA vs SPY return in
/// each. Curves are aligned to `grid` (same length). This is the walk-forward
/// robustness view: consistent edge across windows vs. one lucky stretch.
fn build_subperiods(grid: &[NaiveDate], curve: &[f64], spy: &[f64], k: usize) -> Vec<SubPeriod> {
    let n = grid.len();
    if n < 2 || curve.len() != n || spy.len() != n {
        return Vec::new();
    }
    let k = k.clamp(1, n - 1);
    let seg_ret = |v: &[f64], a: usize, b: usize| {
        if v[a].abs() > f64::EPSILON { (v[b] / v[a] - 1.0) * 100.0 } else { 0.0 }
    };
    let mut out = Vec::with_capacity(k);
    for i in 0..k {
        let a = i * (n - 1) / k;
        let b = (i + 1) * (n - 1) / k;
        if b <= a {
            continue;
        }
        out.push(SubPeriod {
            label: format!("{} → {}", grid[a].format("%Y-%m"), grid[b].format("%Y-%m")),
            vora_pct: seg_ret(curve, a, b),
            spy_pct: seg_ret(spy, a, b),
        });
    }
    out
}

/// Forward-return window (months) used for calibration, by horizon.
fn horizon_months(h: Horizon) -> i64 {
    match h {
        Horizon::Short => 3,
        Horizon::Medium => 6,
        Horizon::Long => 12,
    }
}

/// Build a score calibration across `tickers`: pool every (point-in-time score,
/// realized forward return) pair over the trailing `years` and bucket by score.
/// The forward window matches the horizon (3 / 6 / 12 months).
pub async fn run_calibration(
    tickers: &[String],
    horizon: Horizon,
    years: i64,
) -> Result<Calibration> {
    let end = Utc::now().date_naive();
    let start = end - Duration::days(365 * years.max(1) + 30);
    let hm = horizon_months(horizon);
    let h_idx = hm as usize;

    let fred_start = start - Duration::days(420);
    let macro_series = MacroSeries::fetch(fred_start, end).await;

    let mut pairs: Vec<(f64, f64)> = Vec::new();
    for t in tickers {
        let tu = t.trim().to_uppercase();
        let prices = match fetch_price_history(&tu, start, end).await {
            Ok(p) => p,
            Err(_) => continue,
        };
        let facts = match fetch_companyfacts(&tu).await {
            Ok(f) => f,
            Err(_) => continue,
        };
        let us_gaap = facts["facts"]["us-gaap"].clone();
        let dei = facts["facts"]["dei"].clone();
        let sector = fetch_sector(&tu).await;

        if prices.len() <= h_idx {
            continue;
        }
        // Step by the forward window so observations don't overlap — overlapping
        // monthly windows are heavily autocorrelated and inflate the apparent sample
        // size. Non-overlapping windows give a smaller but ~independent n.
        for i in (0..(prices.len() - h_idx)).step_by(h_idx.max(1)) {
            let (date, px) = prices[i];
            let fpx = prices[i + h_idx].1;
            if px <= 0.0 || fpx <= 0.0 {
                continue;
            }
            let score = score_as_of(&tu, &us_gaap, &dei, sector.as_deref(), &macro_series, &prices, date, px, horizon);
            let fwd = (fpx / px - 1.0) * 100.0;
            pairs.push((score, fwd));
        }
    }

    if pairs.is_empty() {
        return Err(anyhow!("no data to calibrate"));
    }
    Ok(build_calibration(&pairs, hm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn test_build_subperiods_splits_and_measures() {
        let grid: Vec<NaiveDate> = (0..5)
            .map(|m| d(&format!("2020-{:02}-01", m + 1)))
            .collect();
        // VORA doubles overall (100→200); SPY flat.
        let curve = vec![100.0, 120.0, 140.0, 170.0, 200.0];
        let spy = vec![100.0, 100.0, 100.0, 100.0, 100.0];
        let sp = build_subperiods(&grid, &curve, &spy, 2);
        assert_eq!(sp.len(), 2);
        // Every window: VORA up, SPY flat → VORA beats in both.
        assert!(sp.iter().all(|s| s.vora_pct > s.spy_pct));
        assert!(sp.iter().all(|s| (s.spy_pct).abs() < 1e-9));
    }

    #[test]
    fn test_build_subperiods_guards_mismatched_input() {
        let grid = vec![d("2020-01-01"), d("2020-02-01")];
        assert!(build_subperiods(&grid, &[100.0], &[100.0, 100.0], 2).is_empty());
    }

    #[test]
    fn test_random_benchmark_runs_and_is_reasonable() {
        let grid = vec![d("2020-01-01"), d("2020-02-01"), d("2020-03-01")];
        let mk = |t: &str, p: &[f64]| Asset {
            ticker: t.to_string(),
            prices: grid.iter().cloned().zip(p.iter().cloned()).collect(),
            us_gaap: json!({}),
            dei: json!({}),
            sector: None,
        };
        // Both names compound ~+10%/month.
        let assets = vec![mk("A", &[100.0, 110.0, 121.0]), mk("B", &[50.0, 55.0, 60.5])];

        let trials = run_random_benchmark(&assets, &grid, 10_000.0, 0.0, 1, 0.0);
        assert_eq!(trials.len(), RANDOM_TRIALS);
        for (ret, curve) in &trials {
            assert_eq!(curve.len(), grid.len());
            // Any single pick (and switching between them at 0 commission) → ~+21%.
            assert!(*ret > 15.0 && *ret < 25.0, "unexpected random return {}", ret);
        }
    }
}
