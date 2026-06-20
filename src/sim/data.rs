//! Point-in-time data reconstruction for the backtest.
//!
//! Everything here exists to answer one question honestly: *what did we know on
//! date T?* Fundamentals are read from EDGAR filings whose filing date is on or
//! before T (no look-ahead), macro from FRED observations dated on or before T,
//! and prices from historical market data. Signals that cannot be reconstructed
//! historically (news sentiment, insider flow, institutional %, live peer
//! averages) are simply left `None`, which the coverage-aware scoring engine
//! handles gracefully — they are omitted, never faked.

use std::collections::BTreeMap;
use std::env;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, NaiveDate};
use serde_json::Value;

use crate::models::{EdgarData, FinnhubData, FredData, Horizon, YahooData};
use crate::scoring::engine::calculate_analysis;

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// ---------------------------------------------------------------------------
// Date / parsing helpers
// ---------------------------------------------------------------------------

fn parse_d(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn to_unix(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0).map(|ndt| ndt.and_utc().timestamp()).unwrap_or(0)
}

fn is_annual(item: &Value) -> bool {
    item["form"] == "10-K" || item["form"] == "10-K/A"
}

// ---------------------------------------------------------------------------
// As-of XBRL fact extraction (pure)
// ---------------------------------------------------------------------------

/// Latest annual value of `fact` (in unit `unit`) from a 10-K filed on or before `as_of`.
fn val_as_of(fact: &Value, unit: &str, as_of: NaiveDate) -> Option<f64> {
    let arr = fact.get("units")?.get(unit)?.as_array()?;
    arr.iter()
        .filter(|i| is_annual(i))
        .filter(|i| parse_d(i["filed"].as_str().unwrap_or("")).map_or(false, |d| d <= as_of))
        .filter(|i| i["fy"].as_i64().is_some())
        .max_by_key(|i| (i["fy"].as_i64().unwrap_or(0), i["filed"].as_str().unwrap_or("").to_string()))
        .and_then(|i| i["val"].as_f64())
}

/// First tag among `tags` that yields an as-of value.
fn val_any(us_gaap: &Value, tags: &[&str], unit: &str, as_of: NaiveDate) -> Option<f64> {
    tags.iter()
        .filter_map(|t| us_gaap.get(*t))
        .find_map(|f| val_as_of(f, unit, as_of))
}

/// fiscal-year -> value map, keeping the latest filing per fiscal year filed on/before `as_of`.
fn annual_map(fact: &Value, unit: &str, as_of: NaiveDate) -> BTreeMap<i64, (NaiveDate, f64)> {
    let mut map: BTreeMap<i64, (NaiveDate, f64)> = BTreeMap::new();
    if let Some(arr) = fact.get("units").and_then(|u| u.get(unit)).and_then(|a| a.as_array()) {
        for i in arr {
            if !is_annual(i) {
                continue;
            }
            let fy = match i["fy"].as_i64() {
                Some(v) => v,
                None => continue,
            };
            let filed = match parse_d(i["filed"].as_str().unwrap_or("")) {
                Some(d) => d,
                None => continue,
            };
            if filed > as_of {
                continue;
            }
            let val = match i["val"].as_f64() {
                Some(v) => v,
                None => continue,
            };
            map.entry(fy)
                .and_modify(|e| {
                    if filed >= e.0 {
                        *e = (filed, val);
                    }
                })
                .or_insert((filed, val));
        }
    }
    map
}

/// First non-empty annual map among `tags`.
fn first_map(us_gaap: &Value, tags: &[&str], unit: &str, as_of: NaiveDate) -> BTreeMap<i64, (NaiveDate, f64)> {
    for t in tags {
        if let Some(f) = us_gaap.get(*t) {
            let m = annual_map(f, unit, as_of);
            if !m.is_empty() {
                return m;
            }
        }
    }
    BTreeMap::new()
}

/// The two most recent annual values `(prev, latest)` in a fiscal-year map.
fn two_latest(map: &BTreeMap<i64, (NaiveDate, f64)>) -> Option<(f64, f64)> {
    let mut it = map.values().rev();
    let (_, latest) = it.next()?;
    let (_, prev) = it.next()?;
    Some((*prev, *latest))
}

fn yoy_pct(pair: Option<(f64, f64)>) -> Option<f64> {
    let (prev, latest) = pair?;
    if prev.abs() < 0.0001 {
        return None;
    }
    Some((latest - prev) / prev.abs() * 100.0)
}

const REV_TAGS: &[&str] = &[
    "Revenues",
    "RevenueFromContractWithCustomerExcludingAssessedTax",
    "SalesRevenueNet",
];

fn eps_map(us_gaap: &Value, as_of: NaiveDate) -> BTreeMap<i64, (NaiveDate, f64)> {
    for tag in ["EarningsPerShareBasic", "EarningsPerShareDiluted"] {
        if let Some(f) = us_gaap.get(tag) {
            let mut m = annual_map(f, "USD/shares", as_of);
            if m.is_empty() {
                m = annual_map(f, "USD", as_of);
            }
            if !m.is_empty() {
                return m;
            }
        }
    }
    BTreeMap::new()
}

/// Free cash flow per fiscal year (OCF − CapEx), in absolute dollars, as of `as_of`.
fn fcf_map(us_gaap: &Value, as_of: NaiveDate) -> BTreeMap<i64, f64> {
    let ocf = us_gaap
        .get("NetCashProvidedByUsedInOperatingActivities")
        .map(|f| annual_map(f, "USD", as_of))
        .unwrap_or_default();
    let capex = first_map(
        us_gaap,
        &["PaymentsToAcquirePropertyPlantAndEquipment", "PaymentsToAcquireProductiveAssets"],
        "USD",
        as_of,
    );
    let mut out = BTreeMap::new();
    for (fy, (_, o)) in &ocf {
        if let Some((_, c)) = capex.get(fy) {
            out.insert(*fy, o - c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// As-of price helpers
// ---------------------------------------------------------------------------

/// The most recent price on or before `target` (falls back to the earliest point).
pub fn price_on_or_before(history: &[(NaiveDate, f64)], target: NaiveDate) -> Option<f64> {
    history
        .iter()
        .filter(|(d, _)| *d <= target)
        .last()
        .map(|(_, p)| *p)
        .or_else(|| history.first().map(|(_, p)| *p))
}

/// Annualized volatility of monthly returns over the trailing `window` months
/// ending at `as_of`. Used by the portfolio risk guard to trim turbulent names.
pub fn trailing_annual_vol(history: &[(NaiveDate, f64)], as_of: NaiveDate, window: usize) -> Option<f64> {
    if window < 2 {
        return None;
    }
    let prices: Vec<f64> = history.iter().filter(|(d, _)| *d <= as_of).map(|(_, p)| *p).collect();
    if prices.len() < window + 1 {
        return None;
    }
    let slice = &prices[prices.len() - (window + 1)..];
    let rets: Vec<f64> = slice.windows(2).filter(|w| w[0] > 0.0).map(|w| w[1] / w[0] - 1.0).collect();
    if rets.len() < 2 {
        return None;
    }
    let mean = rets.iter().sum::<f64>() / rets.len() as f64;
    let var = rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (rets.len() - 1) as f64;
    Some(var.sqrt() * 12.0_f64.sqrt())
}

/// Latest FRED observation value on or before `target` (series sorted ascending).
fn series_as_of(series: &[(NaiveDate, f64)], target: NaiveDate) -> Option<f64> {
    series.iter().filter(|(d, _)| *d <= target).last().map(|(_, v)| *v)
}

/// All point-in-time macro series the backtest needs, fetched once over the window.
/// Each is best-effort: a missing FRED key or unavailable series leaves it empty,
/// and the coverage-aware scoring engine simply omits the corresponding signal.
#[derive(Default)]
pub struct MacroSeries {
    pub fed: Vec<(NaiveDate, f64)>,
    pub cpi: Vec<(NaiveDate, f64)>,
    pub yield_curve: Vec<(NaiveDate, f64)>,
    pub unemployment: Vec<(NaiveDate, f64)>,
    pub vix: Vec<(NaiveDate, f64)>,
}

impl MacroSeries {
    /// Fetch every macro series over `[start, end]` (ascending).
    pub async fn fetch(start: NaiveDate, end: NaiveDate) -> Self {
        Self {
            fed: fetch_fred_series("FEDFUNDS", start, end).await.unwrap_or_default(),
            cpi: fetch_fred_series("CPIAUCSL", start, end).await.unwrap_or_default(),
            yield_curve: fetch_fred_series("T10Y2Y", start, end).await.unwrap_or_default(),
            unemployment: fetch_fred_series("UNRATE", start, end).await.unwrap_or_default(),
            vix: fetch_fred_series("VIXCLS", start, end).await.unwrap_or_default(),
        }
    }

    /// Average Fed Funds rate over the window — used as the Sharpe risk-free rate.
    pub fn avg_fed(&self) -> f64 {
        if self.fed.is_empty() {
            0.0
        } else {
            self.fed.iter().map(|(_, v)| *v).sum::<f64>() / self.fed.len() as f64
        }
    }
}

// ---------------------------------------------------------------------------
// As-of scoring: reconstruct the composite score as it would have been on `as_of`
// ---------------------------------------------------------------------------

/// Build the composite score for `ticker` as of `as_of`, using only point-in-time data.
#[allow(clippy::too_many_arguments)]
pub fn score_as_of(
    ticker: &str,
    us_gaap: &Value,
    dei: &Value,
    sector: Option<&str>,
    macro_series: &MacroSeries,
    price_history: &[(NaiveDate, f64)],
    as_of: NaiveDate,
    price_at: f64,
    horizon: Horizon,
) -> f64 {
    // --- EDGAR fundamentals (as-of) ---
    let eps_m = eps_map(us_gaap, as_of);
    let eps_latest = eps_m.values().last().map(|(_, v)| *v);
    let eps_growth = yoy_pct(two_latest(&eps_m));

    let rev_m = first_map(us_gaap, REV_TAGS, "USD", as_of);
    let rev_latest = rev_m.values().last().map(|(_, v)| *v);
    let rev_growth = yoy_pct(two_latest(&rev_m));

    let fcf_m = fcf_map(us_gaap, as_of);
    let fcf_latest = fcf_m.values().last().copied();
    let fcf_growth = yoy_pct(two_latest(
        &fcf_m.iter().map(|(k, v)| (*k, (as_of, *v))).collect(),
    ));

    let gross_profit = val_any(us_gaap, &["GrossProfit"], "USD", as_of);
    let operating_income = val_any(us_gaap, &["OperatingIncomeLoss"], "USD", as_of);
    let interest_expense = val_any(us_gaap, &["InterestExpense"], "USD", as_of);
    let equity = val_any(us_gaap, &["StockholdersEquity"], "USD", as_of);

    let gross_margin = match (gross_profit, rev_latest) {
        (Some(g), Some(r)) if r.abs() > 1.0 => Some(g / r * 100.0),
        _ => None,
    };
    let operating_margin = match (operating_income, rev_latest) {
        (Some(o), Some(r)) if r.abs() > 1.0 => Some(o / r * 100.0),
        _ => None,
    };

    // Debt (mirrors the live edgar.rs tag logic, as-of)
    let lt_debt = val_any(us_gaap, &["LongTermDebt"], "USD", as_of)
        .or_else(|| val_any(us_gaap, &["LongTermDebtNoncurrent"], "USD", as_of))
        .unwrap_or(0.0);
    let stb = val_any(us_gaap, &["ShortTermBorrowings"], "USD", as_of);
    let cltd = val_any(us_gaap, &["LongTermDebtCurrent"], "USD", as_of);
    let current_debt = match (stb, cltd) {
        (None, None) => val_any(us_gaap, &["DebtCurrent"], "USD", as_of).unwrap_or(0.0),
        (a, b) => a.unwrap_or(0.0) + b.unwrap_or(0.0),
    };
    let combined_debt = lt_debt + current_debt;
    let debt_to_equity = match equity {
        Some(e) if e.abs() > 0.0001 && combined_debt > 0.0 => Some(combined_debt / e),
        _ => None,
    };

    let interest_coverage_ratio = match (operating_income, interest_expense) {
        (Some(oi), Some(ie)) if ie.abs() > 0.0001 => Some(oi / ie),
        _ => None,
    };

    // Shares outstanding → market cap → P/E, P/S, P/B
    let shares = dei
        .get("EntityCommonStockSharesOutstanding")
        .and_then(|f| val_as_of(f, "shares", as_of));
    let market_cap = shares.map(|s| s * price_at);
    let pe_ratio = eps_latest.filter(|e| *e > 0.0).map(|e| price_at / e);
    let ps_ratio = match (market_cap, rev_latest) {
        (Some(mc), Some(r)) if r.abs() > 1.0 => Some(mc / r),
        _ => None,
    };
    let pb_ratio = match (market_cap, equity) {
        (Some(mc), Some(e)) if e.abs() > 1.0 => Some(mc / e),
        _ => None,
    };

    // 52-week price momentum from history
    let prior_price = price_on_or_before(price_history, as_of - Duration::days(365));
    let price_52w_change_pct = prior_price
        .filter(|p| *p > 0.0)
        .map(|p| (price_at / p - 1.0) * 100.0);

    // --- Macro (as-of) ---
    let fed_funds_rate = series_as_of(&macro_series.fed, as_of);
    let cpi_now = series_as_of(&macro_series.cpi, as_of);
    let cpi_year_ago = series_as_of(&macro_series.cpi, as_of - Duration::days(365));
    let cpi_yoy_change = match (cpi_now, cpi_year_ago) {
        (Some(n), Some(a)) if a.abs() > 0.0001 => Some((n - a) / a * 100.0),
        _ => None,
    };
    let yield_curve_spread = series_as_of(&macro_series.yield_curve, as_of);
    let unemployment_rate = series_as_of(&macro_series.unemployment, as_of);
    let vix = series_as_of(&macro_series.vix, as_of);

    let yahoo = YahooData {
        ticker: ticker.to_string(),
        price: Some(price_at),
        market_cap,
        price_52w_change_pct,
        pe_ratio,
        ps_ratio,
        pb_ratio,
        eps: eps_latest,
        eps_growth_yoy: None, // EDGAR provides realized growth below
        revenue_growth_yoy: rev_growth,
        fifty_two_week_high: None,
        fifty_two_week_low: None,
        short_interest_percent: None,
        dividend_yield: None,
        // Sector classification (from SEC SIC) so sector-aware normalizers match the
        // live analyzer. Stable metadata — applied across the whole window.
        sector: sector.map(|s| s.to_string()),
        industry: None,
        institutional_ownership_percent: None,
        gross_margin,
        operating_margin,
    };

    let fred = FredData {
        fed_funds_rate,
        cpi_yoy_change,
        cpi_trend: None,
        fear_and_greed: None,
        yield_curve_spread,
        unemployment_rate,
        vix,
    };

    let edgar = EdgarData {
        ticker: ticker.to_string(),
        fcf_latest: fcf_latest.map(|v| v / 1_000_000.0),
        fcf_growth_yoy: fcf_growth,
        debt_to_equity,
        interest_coverage_ratio,
        total_debt: None,
        cash_and_equivalents: None,
        actual_eps_growth_yoy: eps_growth,
    };

    let finnhub = FinnhubData {
        ticker: ticker.to_string(),
        news_sentiment_score: None,
        insider_net_shares_3m: None,
        sector_pe_avg: None,
        sector_ps_avg: None,
        sector_growth_score: None,
        recent_competitor_ipos: Vec::new(),
        next_earnings_days: None,
    };

    calculate_analysis(ticker, horizon, &yahoo, &fred, &edgar, &finnhub).composite_score
}

// ---------------------------------------------------------------------------
// Async fetchers
// ---------------------------------------------------------------------------

/// Fetch monthly adjusted-close price history from Yahoo's chart endpoint.
pub async fn fetch_price_history(
    ticker: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<(NaiveDate, f64)>> {
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{}?period1={}&period2={}&interval=1mo",
        ticker.trim().to_uppercase(),
        to_unix(start),
        to_unix(end)
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(BROWSER_UA)
        .build()?;

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("Yahoo chart for {} returned status {}", ticker, resp.status()));
    }
    let v: Value = resp.json().await?;
    let result = &v["chart"]["result"][0];

    let ts = result["timestamp"]
        .as_array()
        .ok_or_else(|| anyhow!("no timestamps in chart for {}", ticker))?;
    let closes = result["indicators"]["adjclose"][0]["adjclose"]
        .as_array()
        .or_else(|| result["indicators"]["quote"][0]["close"].as_array())
        .ok_or_else(|| anyhow!("no closes in chart for {}", ticker))?;

    let mut out = Vec::new();
    for (t, c) in ts.iter().zip(closes.iter()) {
        if let (Some(secs), Some(px)) = (t.as_i64(), c.as_f64()) {
            if let Some(dt) = DateTime::from_timestamp(secs, 0) {
                out.push((dt.date_naive(), px));
            }
        }
    }
    if out.len() < 2 {
        return Err(anyhow!("insufficient price history for {}", ticker));
    }
    Ok(out)
}

/// Fetch a full FRED observation series in `[start, end]`, ascending.
pub async fn fetch_fred_series(
    series_id: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<(NaiveDate, f64)>> {
    let api_key = env::var("FRED_API_KEY")
        .map_err(|_| anyhow!("Missing FRED_API_KEY (needed for backtest macro signals)"))?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let s = start.format("%Y-%m-%d").to_string();
    let e = end.format("%Y-%m-%d").to_string();
    let resp = client
        .get("https://api.stlouisfed.org/fred/series/observations")
        .query(&[
            ("series_id", series_id),
            ("observation_start", s.as_str()),
            ("observation_end", e.as_str()),
            ("file_type", "json"),
            ("sort_order", "asc"),
            ("api_key", api_key.as_str()),
        ])
        .send()
        .await?;

    let v: Value = resp.json().await?;
    let mut out = Vec::new();
    if let Some(obs) = v["observations"].as_array() {
        for o in obs {
            let date = o["date"].as_str().and_then(parse_d);
            let val = o["value"].as_str().filter(|s| *s != ".").and_then(|s| s.parse::<f64>().ok());
            if let (Some(d), Some(x)) = (date, val) {
                out.push((d, x));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_val_as_of_respects_filing_date() {
        // Two fiscal years; only FY2020 was filed before our as-of date.
        let fact = json!({
            "units": { "USD": [
                { "form": "10-K", "fy": 2020, "filed": "2021-02-01", "val": 100.0 },
                { "form": "10-K", "fy": 2021, "filed": "2022-02-01", "val": 200.0 }
            ]}
        });
        // As of mid-2021, only FY2020 is knowable.
        assert_eq!(val_as_of(&fact, "USD", parse_d("2021-06-01").unwrap()), Some(100.0));
        // As of mid-2022, FY2021 is now knowable.
        assert_eq!(val_as_of(&fact, "USD", parse_d("2022-06-01").unwrap()), Some(200.0));
    }

    #[test]
    fn test_val_as_of_prefers_latest_amendment() {
        let fact = json!({
            "units": { "USD": [
                { "form": "10-K", "fy": 2020, "filed": "2021-02-01", "val": 100.0 },
                { "form": "10-K/A", "fy": 2020, "filed": "2021-05-01", "val": 110.0 }
            ]}
        });
        assert_eq!(val_as_of(&fact, "USD", parse_d("2021-06-01").unwrap()), Some(110.0));
    }

    #[test]
    fn test_two_latest_growth() {
        let fact = json!({
            "units": { "USD": [
                { "form": "10-K", "fy": 2019, "filed": "2020-02-01", "val": 100.0 },
                { "form": "10-K", "fy": 2020, "filed": "2021-02-01", "val": 120.0 }
            ]}
        });
        let m = annual_map(&fact, "USD", parse_d("2021-06-01").unwrap());
        assert_eq!(yoy_pct(two_latest(&m)), Some(20.0));
    }

    #[test]
    fn test_series_as_of() {
        let series = vec![
            (parse_d("2020-01-01").unwrap(), 1.0),
            (parse_d("2020-06-01").unwrap(), 2.0),
            (parse_d("2021-01-01").unwrap(), 3.0),
        ];
        assert_eq!(series_as_of(&series, parse_d("2020-08-01").unwrap()), Some(2.0));
        assert_eq!(series_as_of(&series, parse_d("2019-01-01").unwrap()), None);
    }

    #[test]
    fn test_price_on_or_before_falls_back_to_first() {
        let h = vec![
            (parse_d("2020-02-01").unwrap(), 10.0),
            (parse_d("2020-03-01").unwrap(), 11.0),
        ];
        // Before any data → earliest point.
        assert_eq!(price_on_or_before(&h, parse_d("2020-01-01").unwrap()), Some(10.0));
        assert_eq!(price_on_or_before(&h, parse_d("2020-02-15").unwrap()), Some(10.0));
    }
}
