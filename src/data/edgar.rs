use crate::models::EdgarData;
use anyhow::{anyhow, Result};
use serde_json::Value;

/// Get the most recent annual (10-K) value for a given XBRL fact.
/// Selects the entry with the highest "fy" (fiscal year) value,
/// NOT .last() which relies on insertion order (which is not guaranteed).
fn get_latest_val(fact: &Value) -> Option<f64> {
    let units = fact.get("units")?;
    let usd_arr = units.get("USD")?.as_array()?;
    usd_arr.iter()
        .filter(|item| item["form"] == "10-K" || item["form"] == "10-K/A")
        .filter(|item| item["fy"].as_i64().is_some()) // must have a fiscal year
        .max_by_key(|item| (item["fy"].as_i64().unwrap_or(0), item["filed"].as_str().unwrap_or("")))
        .and_then(|item| item.get("val"))
        .and_then(|v| v.as_f64())
}

fn get_fcf_annuals(us_gaap: &Value) -> Option<(f64, Option<f64>)> {
    let ocf_fact = us_gaap.get("NetCashProvidedByUsedInOperatingActivities")?;
    let ocf_units = ocf_fact.get("units")?;
    let ocf_usd = ocf_units.get("USD")?.as_array()?;
    
    let mut years: Vec<i64> = ocf_usd.iter()
        .filter(|item| item["form"] == "10-K" || item["form"] == "10-K/A")
        .filter_map(|item| item["fy"].as_i64())
        .collect();
    years.sort_unstable();
    years.dedup();
    
    let get_capex_for_year = |yr: i64| -> Option<f64> {
        if let Some(fact) = us_gaap.get("PaymentsToAcquirePropertyPlantAndEquipment") {
            if let Some(units) = fact.get("units") {
                if let Some(usd_arr) = units.get("USD").and_then(|u| u.as_array()) {
                    if let Some(val) = usd_arr.iter()
                        .filter(|item| (item["form"] == "10-K" || item["form"] == "10-K/A") && item["fy"].as_i64() == Some(yr))
                        .max_by_key(|item| item["filed"].as_str().unwrap_or(""))
                        .and_then(|item| item.get("val"))
                        .and_then(|v| v.as_f64())
                    {
                        return Some(val);
                    }
                }
            }
        }
        if let Some(fact) = us_gaap.get("PaymentsToAcquireProductiveAssets") {
            if let Some(units) = fact.get("units") {
                if let Some(usd_arr) = units.get("USD").and_then(|u| u.as_array()) {
                    if let Some(val) = usd_arr.iter()
                        .filter(|item| (item["form"] == "10-K" || item["form"] == "10-K/A") && item["fy"].as_i64() == Some(yr))
                        .max_by_key(|item| item["filed"].as_str().unwrap_or(""))
                        .and_then(|item| item.get("val"))
                        .and_then(|v| v.as_f64())
                    {
                        return Some(val);
                    }
                }
            }
        }
        None
    };

    if years.len() >= 2 {
        let latest_year = years[years.len() - 1];
        let prev_year = years[years.len() - 2];
        
        let get_fcf_for_year = |yr: i64| -> Option<f64> {
            let ocf = ocf_usd.iter()
                .filter(|item| (item["form"] == "10-K" || item["form"] == "10-K/A") && item["fy"].as_i64() == Some(yr))
                .max_by_key(|item| item["filed"].as_str().unwrap_or(""))?
                .get("val")?
                .as_f64()?;
                
            let capex = get_capex_for_year(yr)?;
            Some(ocf - capex)
        };
        
        let latest_fcf = get_fcf_for_year(latest_year)?;
        let prev_fcf = get_fcf_for_year(prev_year)?;
        Some((latest_fcf, Some(prev_fcf)))
    } else if years.len() == 1 {
        let yr = years[0];
        let ocf = ocf_usd.iter()
            .filter(|item| (item["form"] == "10-K" || item["form"] == "10-K/A") && item["fy"].as_i64() == Some(yr))
            .max_by_key(|item| item["filed"].as_str().unwrap_or(""))?
            .get("val")?
            .as_f64()?;
        let capex = get_capex_for_year(yr)?;
        let fcf = ocf - capex;
        Some((fcf, None))
    } else {
        None
    }
}

async fn send_request_with_retry(
    client: &reqwest::Client,
    url: &str,
) -> Result<reqwest::Response> {
    let mut delay = std::time::Duration::from_millis(500);
    let mut attempts = 0;
    let max_attempts = 4;
    loop {
        let resp = client.get(url).send().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        if (status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) && attempts < max_attempts {
            attempts += 1;
            log_warn!("EDGAR request failed with status: {}. Retrying attempt {}/{} after {:?}", status, attempts, max_attempts, delay);
            tokio::time::sleep(delay).await;
            delay *= 2;
        } else {
            return Err(anyhow!("SEC EDGAR request failed with status: {}", status));
        }
    }
}

/// SEC asks callers to identify themselves. Honor SEC_EDGAR_USER_AGENT if set
/// (documented in the README), otherwise fall back to a sane default.
fn edgar_client() -> Result<reqwest::Client> {
    let user_agent = std::env::var("SEC_EDGAR_USER_AGENT")
        .unwrap_or_else(|_| "vora-vision support@voravision.com".to_string());
    Ok(reqwest::Client::builder().user_agent(user_agent).build()?)
}

/// Resolve a ticker to its zero-padded SEC CIK via the public ticker map.
async fn resolve_cik(client: &reqwest::Client, ticker_upper: &str) -> Result<u64> {
    let tickers_url = "https://www.sec.gov/files/company_tickers.json";
    let resp = send_request_with_retry(client, tickers_url).await?;
    let tickers_val: Value = resp.json().await?;
    if let Some(obj) = tickers_val.as_object() {
        for (_key, val) in obj {
            if let Some(t) = val["ticker"].as_str() {
                if t.to_uppercase() == *ticker_upper {
                    if let Some(c) = val["cik_str"].as_u64() {
                        return Ok(c);
                    }
                }
            }
        }
    }
    Err(anyhow!("Ticker {} not found in SEC database", ticker_upper))
}

/// Fetch the raw SEC `companyfacts` JSON for a ticker (all historical XBRL facts).
/// Shared by the live analyzer and the backtest's point-in-time reconstruction.
pub async fn fetch_companyfacts(ticker: &str) -> Result<Value> {
    let client = edgar_client()?;
    let ticker_upper = ticker.trim().to_uppercase();
    let cik_num = resolve_cik(&client, &ticker_upper).await?;
    let facts_url = format!("https://data.sec.gov/api/xbrl/companyfacts/CIK{:010}.json", cik_num);
    let facts_resp = send_request_with_retry(&client, &facts_url).await?;
    let facts_val: Value = facts_resp.json().await?;
    Ok(facts_val)
}

/// Best-effort sector for `ticker`, derived from the company's SEC SIC code.
/// Used by the backtest so its point-in-time scoring uses the same sector-aware
/// normalizers (P/B, margins, debt/equity ceilings) as the live analyzer — without
/// it, backtest and live scores diverge for the same company. Returns `None` on any
/// failure or an unmapped SIC (the engine then falls back to generic defaults).
pub async fn fetch_sector(ticker: &str) -> Option<String> {
    let client = edgar_client().ok()?;
    let ticker_upper = ticker.trim().to_uppercase();
    let cik = resolve_cik(&client, &ticker_upper).await.ok()?;
    let url = format!("https://data.sec.gov/submissions/CIK{:010}.json", cik);
    let resp = send_request_with_retry(&client, &url).await.ok()?;
    let v: Value = resp.json().await.ok()?;
    sic_to_sector(v["sic"].as_str()?).map(|s| s.to_string())
}

/// Map a SEC SIC code to a coarse sector keyword that the normalizers recognize.
/// Only high-confidence buckets are mapped; everything else returns `None` and the
/// engine uses sector-agnostic defaults.
fn sic_to_sector(sic: &str) -> Option<&'static str> {
    let code: u32 = sic.trim().parse().ok()?;
    let sector = match code {
        1000..=1499 => "Basic Materials",
        2000..=2079 => "Consumer Defensive Food",
        2080..=2199 => "Consumer Defensive",
        2830..=2836 => "Pharmaceutical",
        2840..=2844 => "Consumer Defensive",
        2900..=2999 => "Basic Materials",
        3570..=3579 => "Technology",
        3661..=3679 => "Technology",
        3826 | 3829 | 3841..=3851 => "Healthcare",
        4810..=4899 => "Communication",
        4900..=4949 => "Utilities",
        5200..=5999 => "Retail",
        6000..=6499 => "Financial Services",
        6500..=6599 => "Real Estate",
        6798 => "Real Estate",
        6700..=6797 | 6799 => "Financial Services",
        7370..=7379 => "Technology",
        8000..=8099 => "Healthcare",
        _ => return None,
    };
    Some(sector)
}

pub async fn fetch_edgar(ticker: &str) -> Result<EdgarData> {
    let ticker_upper = ticker.trim().to_uppercase();
    let facts_val = fetch_companyfacts(&ticker_upper).await?;
    let us_gaap = &facts_val["facts"]["us-gaap"];

    // 0. Actual EPS Growth YoY (from realized 10-K filings, not analyst estimates)
    let actual_eps_growth_yoy: Option<f64> = (|| {
        // Prefer Basic EPS; fall back to Diluted for filers that only report Diluted.
        let eps_fact = us_gaap.get("EarningsPerShareBasic")
            .or_else(|| us_gaap.get("EarningsPerShareDiluted"))?;
        let eps_units = eps_fact.get("units")?;
        // EPS is reported per share in USD
        let eps_usd = eps_units.get("USD/shares")
            .or_else(|| eps_units.get("USD"))?
            .as_array()?;

        // Collect all 10-K annual EPS values, keyed by fiscal year
        let mut annual_eps: Vec<(i64, f64, String)> = eps_usd.iter()
            .filter(|item| item["form"] == "10-K" || item["form"] == "10-K/A")
            .filter_map(|item| {
                let fy = item["fy"].as_i64()?;
                let val = item["val"].as_f64()?;
                let filed = item["filed"].as_str().unwrap_or("").to_string();
                Some((fy, val, filed))
            })
            .collect();

        // Sort by fiscal year and filing date ascending
        annual_eps.sort_by(|a, b| {
            match a.0.cmp(&b.0) {
                std::cmp::Ordering::Equal => a.2.cmp(&b.2),
                other => other,
            }
        });

        // Deduplicate: keep the last entry (the latest amendment) for each fiscal year
        annual_eps.dedup_by(|a, b| {
            if a.0 == b.0 {
                *b = a.clone();
                true
            } else {
                false
            }
        });

        if annual_eps.len() < 2 {
            return None;
        }

        let (_, prev_eps, _) = &annual_eps[annual_eps.len() - 2];
        let (_, latest_eps, _) = &annual_eps[annual_eps.len() - 1];

        if prev_eps.abs() < 0.0001 {
            // Avoid division by near-zero (e.g. company was breakeven)
            return None;
        }

        Some(((latest_eps - prev_eps) / prev_eps.abs()) * 100.0)
    })();

    // 1. FCF & FCF Growth
    let mut fcf_latest = None;
    let mut fcf_growth_yoy = None;
    if let Some((latest_fcf, prev_fcf_opt)) = get_fcf_annuals(us_gaap) {
        fcf_latest = Some(latest_fcf / 1_000_000.0);
        if let Some(prev_fcf) = prev_fcf_opt {
            if prev_fcf.abs() > 0.0001 {
                fcf_growth_yoy = Some(((latest_fcf - prev_fcf) / prev_fcf.abs()) * 100.0);
            }
        }
    }

    // 2. Debt to Equity & Total Debt
    // Total debt = LongTermDebt + ShortTermDebt (current maturities + revolving credit)
    // Long-term portion: prefer the all-in LongTermDebt tag; fall back to the
    // noncurrent-only tag for filers that don't report LongTermDebt.
    let long_term_debt = us_gaap.get("LongTermDebt").and_then(get_latest_val)
        .or_else(|| us_gaap.get("LongTermDebtNoncurrent").and_then(get_latest_val))
        .unwrap_or(0.0);

    // Current/short-term portion: sum the explicit current tags, or fall back to
    // the generic DebtCurrent tag when neither explicit tag is present.
    let short_term_borrowings = us_gaap.get("ShortTermBorrowings").and_then(get_latest_val);
    let current_long_term = us_gaap.get("LongTermDebtCurrent").and_then(get_latest_val);
    let current_debt = match (short_term_borrowings, current_long_term) {
        (None, None) => us_gaap.get("DebtCurrent").and_then(get_latest_val).unwrap_or(0.0),
        (a, b) => a.unwrap_or(0.0) + b.unwrap_or(0.0),
    };

    let combined_debt = long_term_debt + current_debt;
    let equity = us_gaap.get("StockholdersEquity").and_then(get_latest_val);

    // Only store total_debt if we have any debt data at all
    let total_debt = if combined_debt > 0.0 {
        Some(combined_debt / 1_000_000.0)
    } else {
        None
    };

    let debt_to_equity = match equity {
        Some(eq) if eq.abs() > 0.0001 && combined_debt > 0.0 => Some(combined_debt / eq),
        _ => None,
    };

    // 3. Cash and equivalents
    let cash_and_equivalents = us_gaap.get("CashAndCashEquivalentsAtCarryingValue")
        .and_then(get_latest_val)
        .map(|c| c / 1_000_000.0);

    // 4. Interest coverage
    let operating_income = us_gaap.get("OperatingIncomeLoss").and_then(get_latest_val);
    let interest_expense = us_gaap.get("InterestExpense").and_then(get_latest_val);
    
    let interest_coverage_ratio = match (operating_income, interest_expense) {
        (Some(oi), Some(ie)) if ie.abs() > 0.0001 => Some(oi / ie),
        _ => None,
    };

    Ok(EdgarData {
        ticker: ticker_upper,
        fcf_latest,
        fcf_growth_yoy,
        debt_to_equity,
        interest_coverage_ratio,
        total_debt,
        cash_and_equivalents,
        actual_eps_growth_yoy,
    })
}
