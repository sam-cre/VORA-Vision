use crate::models::YahooData;
use anyhow::{anyhow, Result};
use std::sync::Arc;
use reqwest::cookie::Jar;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;

pub async fn fetch_yahoo(ticker: &str) -> Result<YahooData> {
    // 1. Fetch current price using the crate
    let provider = yahoo_finance_api::YahooConnector::new()
        .map_err(|e| anyhow!("Failed to create YahooConnector: {:?}", e))?;

    let ticker_upper = ticker.trim().to_uppercase();

    let latest_quotes_res = provider.get_latest_quotes(&ticker_upper, "1d").await;
    let price = match latest_quotes_res {
        Ok(resp) => resp.last_quote().ok().map(|q| q.close),
        Err(_) => None,
    };

    // 2. Fetch fundamentals using the custom cookie/crumb method
    let cookie_jar = Arc::new(Jar::default());
    let user_agent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

    let client = reqwest::Client::builder()
        .cookie_provider(cookie_jar.clone())
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_str(user_agent)?);
    
    let _ = client.get("https://fc.yahoo.com")
        .headers(headers.clone())
        .send()
        .await;

    let crumb_resp = client.get("https://query2.finance.yahoo.com/v1/test/getcrumb")
        .headers(headers.clone())
        .send()
        .await?;
    
    let crumb = crumb_resp.text().await?;
    let crumb = crumb.trim();

    let url = format!(
        "https://query2.finance.yahoo.com/v10/finance/quoteSummary/{}?crumb={}&modules=assetProfile,financialData,defaultKeyStatistics,summaryDetail",
        ticker_upper, crumb
    );

    let summary_resp = client.get(&url)
        .headers(headers)
        .send()
        .await?;

    if !summary_resp.status().is_success() {
        return Err(anyhow!("Failed to fetch Yahoo quoteSummary: status {}", summary_resp.status()));
    }

    let summary_val: Value = summary_resp.json().await?;
    let result = &summary_val["quoteSummary"]["result"][0];

    let get_raw_f64 = |val: &Value| val["raw"].as_f64();
    let get_str = |val: &Value| val.as_str().map(|s| s.to_string());

    let market_cap = get_raw_f64(&result["summaryDetail"]["marketCap"])
        .or_else(|| get_raw_f64(&result["defaultKeyStatistics"]["marketCap"]));

    // 52WeekChange from Yahoo is the stock's price return over 52 weeks as a decimal (e.g. 0.23 = +23%).
    // This is price momentum, NOT market cap change. Renamed accordingly.
    let price_52w_change_pct = get_raw_f64(&result["defaultKeyStatistics"]["52WeekChange"])
        .map(|v| v * 100.0);

    let pe_ratio = get_raw_f64(&result["summaryDetail"]["trailingPE"]);
    let ps_ratio = get_raw_f64(&result["summaryDetail"]["priceToSalesTrailing12Months"]);
    let pb_ratio = get_raw_f64(&result["defaultKeyStatistics"]["priceToBook"]);
    
    let trailing_eps = get_raw_f64(&result["defaultKeyStatistics"]["trailingEps"]);
    // forwardEps is kept for display purposes only. Do NOT use it for growth calculation —
    // it is an analyst estimate, not realized earnings. eps_growth_yoy is now computed
    // from actual EDGAR historical data in fetch_edgar().
    let _forward_eps = get_raw_f64(&result["defaultKeyStatistics"]["forwardEps"]);
    let eps_growth_yoy: Option<f64> = None; // Set by EDGAR fetch; see data/edgar.rs

    let revenue_growth_yoy = get_raw_f64(&result["financialData"]["revenueGrowth"])
        .map(|v| v * 100.0);

    let fifty_two_week_high = get_raw_f64(&result["summaryDetail"]["fiftyTwoWeekHigh"]);
    let fifty_two_week_low = get_raw_f64(&result["summaryDetail"]["fiftyTwoWeekLow"]);

    let short_interest_percent = get_raw_f64(&result["defaultKeyStatistics"]["shortPercentOfFloat"])
        .map(|v| v * 100.0);

    let dividend_yield = get_raw_f64(&result["summaryDetail"]["dividendYield"])
        .map(|v| v * 100.0);

    let sector = get_str(&result["assetProfile"]["sector"]);
    let industry = get_str(&result["assetProfile"]["industry"]);

    let institutional_ownership_percent = get_raw_f64(&result["defaultKeyStatistics"]["heldPercentInstitutions"])
        .map(|v| v * 100.0);

    let gross_margin = get_raw_f64(&result["financialData"]["grossMargins"])
        .map(|v| v * 100.0);

    let operating_margin = get_raw_f64(&result["financialData"]["operatingMargins"])
        .map(|v| v * 100.0);

    Ok(YahooData {
        ticker: ticker_upper,
        price,
        market_cap,
        price_52w_change_pct,
        pe_ratio,
        ps_ratio,
        pb_ratio,
        eps: trailing_eps,
        eps_growth_yoy,
        revenue_growth_yoy,
        fifty_two_week_high,
        fifty_two_week_low,
        short_interest_percent,
        dividend_yield,
        sector,
        industry,
        institutional_ownership_percent,
        gross_margin,
        operating_margin,
    })
}
