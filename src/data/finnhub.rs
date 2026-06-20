use crate::models::FinnhubData;
use crate::data::yahoo::fetch_yahoo;
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::env;
use std::time::Duration;

#[derive(Deserialize, Debug)]
struct InsiderTransaction {
    #[allow(dead_code)]
    share: Option<f64>,
    change: Option<f64>,
    #[serde(rename = "transactionCode")]
    transaction_code: Option<String>,
    // Finnhub returns "date" as "YYYY-MM-DD" string
    date: Option<String>,
}

#[derive(Deserialize, Debug)]
struct InsiderTransactionsResponse {
    data: Option<Vec<InsiderTransaction>>,
}



#[derive(Deserialize, Debug)]
struct IpoCalendarResponse {
    #[serde(rename = "ipoCalendar")]
    ipo_calendar: Option<Vec<IpoItem>>,
}

#[derive(Deserialize, Debug)]
struct IpoItem {
    symbol: String,
    exchange: Option<String>,
}

#[derive(Deserialize, Debug)]
struct EarningsCalendarResponse {
    #[serde(rename = "earningsCalendar")]
    earnings_calendar: Option<Vec<EarningsItem>>,
}

#[derive(Deserialize, Debug)]
struct EarningsItem {
    date: Option<String>,
}

#[derive(Deserialize, Debug)]
struct CompanyNewsItem {
    headline: Option<String>,
    /// Unix publish time (seconds) — used for recency weighting.
    datetime: Option<i64>,
}

pub async fn fetch_finnhub(ticker: &str, sector: &str) -> Result<FinnhubData> {
    let api_key = env::var("FINNHUB_API_KEY")
        .map_err(|_| anyhow!("Missing environment variable: FINNHUB_API_KEY. Set it and restart."))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let ticker_upper = ticker.trim().to_uppercase();

    let today = chrono::Utc::now().date_naive();

    // 1. News Sentiment: Finnhub's quantitative sentiment endpoint is premium-only,
    // but its company-news headline feed is free. Score the last 14 days of headlines
    // with a finance lexicon (see scoring::news). Live signal — the backtest leaves
    // this None and the coverage-aware engine falls back to the Fear & Greed proxy.
    let news_from = today - chrono::Duration::days(14);
    let news_from_s = news_from.format("%Y-%m-%d").to_string();
    let news_to_s = today.format("%Y-%m-%d").to_string();
    let mut news_sentiment_score: Option<f64> = None;
    match client
        .get("https://finnhub.io/api/v1/company-news")
        .query(&[
            ("symbol", ticker_upper.as_str()),
            ("from", news_from_s.as_str()),
            ("to", news_to_s.as_str()),
            ("token", api_key.as_str()),
        ])
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(items) = resp.json::<Vec<CompanyNewsItem>>().await {
                let now = chrono::Utc::now().timestamp();
                let headlines: Vec<crate::scoring::news::Headline> = items
                    .into_iter()
                    .filter_map(|i| {
                        let text = i.headline?;
                        let age_days = i
                            .datetime
                            .map(|dt| ((now - dt).max(0) as f64) / 86_400.0)
                            .unwrap_or(0.0);
                        Some(crate::scoring::news::Headline { text, age_days })
                    })
                    .take(80)
                    .collect();
                news_sentiment_score = crate::scoring::news::score_headlines(&headlines);
                if let Some(s) = news_sentiment_score {
                    log_info!(
                        "Finnhub: news sentiment for {} = {:.2} ({} headlines)",
                        ticker_upper, s, headlines.len()
                    );
                }
            }
        }
        Ok(resp) => { log_warn!("Finnhub company-news returned status: {}", resp.status()); }
        Err(e) => { log_warn!("Finnhub company-news request failed: {}", e); }
    }

    // 2. Fetch Insider Transactions (free tier endpoint)
    let insider_url = "https://finnhub.io/api/v1/stock/insider-transactions";
    let mut insider_net_shares = None;

    match client
        .get(insider_url)
        .query(&[
            ("symbol", ticker_upper.as_str()),
            ("token", &api_key),
        ])
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<InsiderTransactionsResponse>().await {
                    Ok(insider_resp) => {
                        if let Some(transactions) = insider_resp.data {
                            // Calculate net insider shares over the last 90 days only
                            let ninety_days_ago = today - chrono::Duration::days(90);
                            let mut net_shares: i64 = 0;
                            let mut has_transactions = false;
                            
                            for txn in &transactions {
                                // Skip transactions outside the 90-day window
                                let in_window = txn.date.as_deref()
                                    .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                                    .map(|d| d >= ninety_days_ago)
                                    .unwrap_or(false); // If no date, exclude it

                                if !in_window {
                                    continue;
                                }

                                // transactionCode: P = Purchase, S = Sale
                                let change = txn.change.unwrap_or(0.0) as i64;
                                let code = txn.transaction_code.as_deref().unwrap_or("");
                                
                                match code {
                                    "P" => {
                                        // Open-market Purchase — genuine vote of confidence
                                        net_shares += change.abs();
                                        has_transactions = true;
                                    }
                                    "S" => {
                                        // Open-market Sale — genuine exit signal
                                        net_shares -= change.abs();
                                        has_transactions = true;
                                    }
                                    // "A" (Award/Grant), "M" (Exercise), "G" (Gift), "X" (Exercise+Sale),
                                    // and "F" (tax withholding) are mechanical/compensation events, not sentiment signals.
                                    _ => {}
                                }
                            }
                            
                            if has_transactions {
                                insider_net_shares = Some(net_shares);
                                log_info!("Finnhub: Insider net shares for {}: {}", ticker_upper, net_shares);
                            } else {
                                log_info!("Finnhub: No relevant insider transactions for {}", ticker_upper);
                            }
                        }
                    }
                    Err(e) => {
                        log_warn!("Finnhub insider-transactions JSON parse failed: {}", e);
                    }
                }
            } else {
                log_warn!("Finnhub insider-transactions returned status: {}", resp.status());
            }
        }
        Err(e) => {
            log_warn!("Finnhub insider-transactions request failed: {}", e);
        }
    }

    tokio::time::sleep(Duration::from_millis(250)).await;

    // 3. Fetch Peers (for sector comparison)
    let peers_url = "https://finnhub.io/api/v1/stock/peers";
    let mut peers: Vec<String> = Vec::new();
    if let Ok(resp) = client
        .get(peers_url)
        .query(&[
            ("symbol", ticker_upper.as_str()),
            ("token", &api_key),
        ])
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(p) = resp.json::<Vec<String>>().await {
                peers = p;
            }
        }
    }

    let filtered_peers: Vec<String> = peers
        .into_iter()
        .filter(|p| p.to_uppercase() != ticker_upper)
        .take(5)
        .collect();

    let mut peer_pes = Vec::new();
    let mut peer_pss = Vec::new();
    let mut peer_growths = Vec::new();

    let mut peer_handles = Vec::new();
    for peer in &filtered_peers {
        let peer = peer.clone();
        peer_handles.push(tokio::spawn(async move {
            fetch_yahoo(&peer).await
        }));
    }

    for h in peer_handles {
        if let Ok(Ok(yahoo_data)) = h.await {
            if let Some(pe) = yahoo_data.pe_ratio {
                peer_pes.push(pe);
            }
            if let Some(ps) = yahoo_data.ps_ratio {
                peer_pss.push(ps);
            }
            if let Some(rev_g) = yahoo_data.revenue_growth_yoy {
                peer_growths.push(rev_g);
            }
        }
    }

    let sector_pe_avg = if !peer_pes.is_empty() {
        Some(peer_pes.iter().sum::<f64>() / peer_pes.len() as f64)
    } else {
        None
    };

    let sector_ps_avg = if !peer_pss.is_empty() {
        Some(peer_pss.iter().sum::<f64>() / peer_pss.len() as f64)
    } else {
        None
    };

    let avg_peer_growth = if !peer_growths.is_empty() {
        Some(peer_growths.iter().sum::<f64>() / peer_growths.len() as f64)
    } else {
        None
    };

    // NOTE: avg_peer_growth is derived from Yahoo's revenue_growth_yoy for each peer.
    // This is a realized growth metric and serves as an excellent proxy for sector growth momentum.
    let sector_growth_score = avg_peer_growth.map(crate::scoring::normalize::normalize_sector_growth);

    // 4. Fetch IPO Calendar (Handle restrictions gracefully)
    let six_months_ago = today - chrono::Duration::days(180);

    let ipo_url = "https://finnhub.io/api/v1/calendar/ipo";
    let mut ipo_list = Vec::new();
    if let Ok(resp) = client
        .get(ipo_url)
        .query(&[
            ("from", &six_months_ago.format("%Y-%m-%d").to_string()),
            ("to", &today.format("%Y-%m-%d").to_string()),
            ("token", &api_key),
        ])
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(ipo_res) = resp.json::<IpoCalendarResponse>().await {
                ipo_list = ipo_res.ipo_calendar.unwrap_or_default();
            }
        }
    }

    let filtered_ipos: Vec<IpoItem> = ipo_list
        .into_iter()
        .filter(|ipo| {
            if let Some(ref exch) = ipo.exchange {
                let exch_upper = exch.to_uppercase();
                exch_upper.contains("NYSE") || exch_upper.contains("NASDAQ")
            } else {
                false
            }
        })
        .collect();

    let mut recent_competitor_ipos = Vec::new();
    let sector_lower = sector.to_lowercase();

    let mut ipo_handles = Vec::new();
    for ipo in filtered_ipos.iter().take(5) {
        let symbol = ipo.symbol.clone();
        ipo_handles.push(tokio::spawn(async move {
            let res = fetch_yahoo(&symbol).await;
            (symbol, res)
        }));
    }

    for h in ipo_handles {
        if let Ok((symbol, Ok(yahoo_data))) = h.await {
            if let Some(ref ipo_sector) = yahoo_data.sector {
                let ipo_sec_lower = ipo_sector.to_lowercase();
                // Require exact sector match (case-insensitive).
                // Do NOT use substring match: "Technology" would match "Biotechnology".
                if ipo_sec_lower == sector_lower {
                    recent_competitor_ipos.push(symbol);
                }
            }
        }
    }

    // 5. Earnings calendar — days until the next scheduled report (free endpoint).
    //    A near-term earnings date is a volatility catalyst (feeds the Risk category).
    let earnings_to = today + chrono::Duration::days(90);
    let mut next_earnings_days: Option<i64> = None;
    if let Ok(resp) = client
        .get("https://finnhub.io/api/v1/calendar/earnings")
        .query(&[
            ("from", today.format("%Y-%m-%d").to_string().as_str()),
            ("to", earnings_to.format("%Y-%m-%d").to_string().as_str()),
            ("symbol", ticker_upper.as_str()),
            ("token", api_key.as_str()),
        ])
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(cal) = resp.json::<EarningsCalendarResponse>().await {
                next_earnings_days = cal
                    .earnings_calendar
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|e| e.date.as_deref())
                    .filter_map(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                    .map(|d| (d - today).num_days())
                    .filter(|days| *days >= 0)
                    .min();
                if let Some(d) = next_earnings_days {
                    log_info!("Finnhub: next earnings for {} in {} day(s)", ticker_upper, d);
                }
            }
        } else {
            log_warn!("Finnhub earnings-calendar returned status: {}", resp.status());
        }
    }

    Ok(FinnhubData {
        ticker: ticker_upper,
        news_sentiment_score,
        insider_net_shares_3m: insider_net_shares,
        sector_pe_avg,
        sector_ps_avg,
        sector_growth_score,
        recent_competitor_ipos,
        next_earnings_days,
    })
}
