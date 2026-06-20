use crate::models::FredData;
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::Value;
use std::env;

#[derive(Deserialize, Debug)]
struct FredResponse {
    observations: Option<Vec<Observation>>,
}

#[derive(Deserialize, Debug)]
struct Observation {
    #[allow(dead_code)]
    date: String,
    value: String,
}

pub async fn fetch_fred() -> Result<FredData> {
    let api_key = env::var("FRED_API_KEY")
        .map_err(|_| anyhow!("Missing environment variable: FRED_API_KEY. Set it and restart."))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let url = "https://api.stlouisfed.org/fred/series/observations";

    // 1. Fetch Federal Funds Rate
    let fed_funds_res = client
        .get(url)
        .query(&[
            ("series_id", "FEDFUNDS"),
            ("limit", "1"),
            ("sort_order", "desc"),
            ("file_type", "json"),
            ("api_key", &api_key),
        ])
        .send()
        .await?
        .json::<FredResponse>()
        .await?;

    let fed_funds_rate = fed_funds_res
        .observations
        .as_ref()
        .and_then(|obs| obs.first())
        .and_then(|obs| {
            if obs.value == "." {
                None
            } else {
                obs.value.parse::<f64>().ok()
            }
        });

    // 2. Fetch CPI (All Urban Consumers)
    let cpi_res = client
        .get(url)
        .query(&[
            ("series_id", "CPIAUCSL"),
            ("limit", "14"), // 13 for a full YoY window + 1 buffer if the newest month isn't posted yet
            ("sort_order", "desc"),
            ("file_type", "json"),
            ("api_key", &api_key),
        ])
        .send()
        .await?
        .json::<FredResponse>()
        .await?;

    let obs_list = cpi_res.observations.unwrap_or_default();

    let mut cpi_yoy_change = None;
    let mut cpi_trend = None;

    if obs_list.len() >= 13 {
        let most_recent_opt = if obs_list[0].value == "." {
            None
        } else {
            obs_list[0].value.parse::<f64>().ok()
        };
        let twelve_months_ago_opt = if obs_list[12].value == "." {
            None
        } else {
            obs_list[12].value.parse::<f64>().ok()
        };

        if let (Some(most_recent), Some(twelve_months_ago)) = (most_recent_opt, twelve_months_ago_opt) {
            if twelve_months_ago.abs() > 0.0001 {
                cpi_yoy_change = Some(((most_recent - twelve_months_ago) / twelve_months_ago) * 100.0);
            }
        }
    }

    // CPI trend: only classify as Rising/Falling if the month-over-month change
    // exceeds a meaningful threshold (0.1 CPI index points ≈ ~0.05% MoM inflation).
    // This prevents noise (rounding changes of 0.01) from triggering trend adjustments.
    const CPI_TREND_THRESHOLD: f64 = 0.1;

    if obs_list.len() >= 3 {
        let parse_obs = |i: usize| -> Option<f64> {
            if obs_list[i].value == "." { None } else { obs_list[i].value.parse::<f64>().ok() }
        };

        if let (Some(most_recent), Some(one_month_ago), Some(two_months_ago)) =
            (parse_obs(0), parse_obs(1), parse_obs(2))
        {
            let delta1 = most_recent - one_month_ago;   // latest vs previous month
            let delta2 = one_month_ago - two_months_ago; // previous vs two months ago

            if delta1 > CPI_TREND_THRESHOLD && delta2 > CPI_TREND_THRESHOLD {
                cpi_trend = Some("Rising".to_string());
            } else if delta1 < -CPI_TREND_THRESHOLD && delta2 < -CPI_TREND_THRESHOLD {
                cpi_trend = Some("Falling".to_string());
            } else {
                cpi_trend = Some("Stable".to_string());
            }
        }
    }

    // 3. Additional macro regime series (latest observation each).
    //    All free FRED series: yield curve (recession signal), unemployment, VIX.
    let yield_curve_spread = fetch_latest(&client, url, &api_key, "T10Y2Y").await;
    let unemployment_rate = fetch_latest(&client, url, &api_key, "UNRATE").await;
    let vix = fetch_latest(&client, url, &api_key, "VIXCLS").await;

    // 4. Fetch Fear and Greed Index
    //    Try CNN endpoint first, then feargreedchart.com as fallback
    let fear_and_greed = match fetch_cnn_fear_greed(&client).await {
        Some(score) => Some(score),
        None => {
            log_info!("CNN Fear & Greed unavailable, trying feargreedchart.com fallback");
            fetch_feargreedchart_fallback(&client).await
        }
    };

    if fear_and_greed.is_some() {
        log_info!("Fear & Greed Index fetched: {:.1}", fear_and_greed.unwrap());
    } else {
        log_warn!("All Fear & Greed sources failed");
    }

    Ok(FredData {
        fed_funds_rate,
        cpi_yoy_change,
        cpi_trend,
        fear_and_greed,
        yield_curve_spread,
        unemployment_rate,
        vix,
    })
}

/// Fetch the most recent *valid* observation for a FRED series. Daily series
/// (VIX, the yield-curve spread) carry a missing "." on weekends/holidays, so we
/// pull the latest dozen observations and return the newest parseable one rather
/// than giving up when only the very latest is missing.
async fn fetch_latest(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    series_id: &str,
) -> Option<f64> {
    let res = client
        .get(url)
        .query(&[
            ("series_id", series_id),
            ("limit", "12"),
            ("sort_order", "desc"),
            ("file_type", "json"),
            ("api_key", api_key),
        ])
        .send()
        .await
        .ok()?
        .json::<FredResponse>()
        .await
        .ok()?;

    // Observations are newest-first; take the first one that isn't ".".
    res.observations
        .as_ref()?
        .iter()
        .find_map(|obs| if obs.value == "." { None } else { obs.value.parse::<f64>().ok() })
}

/// Fetch CNN Fear & Greed Index from the production dataviz endpoint.
/// Requires a browser-like User-Agent header to avoid 418 errors.
async fn fetch_cnn_fear_greed(client: &reqwest::Client) -> Option<f64> {
    let cnn_url = "https://production.dataviz.cnn.io/index/fearandgreed/graphdata";
    
    let resp = client
        .get(cnn_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        log_warn!("CNN Fear & Greed returned status: {}", resp.status());
        return None;
    }

    let body: Value = resp.json().await.ok()?;
    
    // The CNN response has: { "fear_and_greed": { "score": 45.0, ... }, ... }
    let score = body
        .get("fear_and_greed")
        .and_then(|fg| fg.get("score"))
        .and_then(|s| s.as_f64());

    if score.is_some() {
        log_info!("CNN Fear & Greed score fetched successfully");
    }
    
    score
}

/// Fallback: fetch from feargreedchart.com API
async fn fetch_feargreedchart_fallback(client: &reqwest::Client) -> Option<f64> {
    #[derive(Deserialize, Debug)]
    struct FearGreedScoreObj {
        score: f64,
    }

    #[derive(Deserialize, Debug)]
    struct FearGreedApiResponse {
        score: FearGreedScoreObj,
    }

    let fg_url = "https://feargreedchart.com/api/?action=all";
    let resp = client.get(fg_url).send().await.ok()?;
    
    if !resp.status().is_success() {
        log_warn!("feargreedchart.com returned status: {}", resp.status());
        return None;
    }
    
    let parsed = resp.json::<FearGreedApiResponse>().await.ok()?;
    log_info!("feargreedchart.com fallback score fetched: {:.1}", parsed.score.score);
    Some(parsed.score.score)
}
