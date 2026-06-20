use std::fmt;
use chrono::{DateTime, Utc};
use ratatui::style::Color;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Horizon {
    Short,
    Medium,
    Long,
}

impl fmt::Display for Horizon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Horizon::Short => write!(f, "Short-Term"),
            Horizon::Medium => write!(f, "Medium-Term"),
            Horizon::Long => write!(f, "Long-Term"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal {
    Buy,
    Hold,
    Sell,
}

impl Signal {
    pub fn color(&self) -> Color {
        match self {
            Signal::Buy => Color::Green,
            Signal::Hold => Color::Yellow,
            Signal::Sell => Color::Red,
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Signal::Buy => write!(f, "BUY"),
            Signal::Hold => write!(f, "HOLD"),
            Signal::Sell => write!(f, "SELL"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataSource {
    Yahoo,
    Fred,
    Edgar,
    Finnhub,
}

impl fmt::Display for DataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataSource::Yahoo => write!(f, "Yahoo Finance"),
            DataSource::Fred => write!(f, "FRED"),
            DataSource::Edgar => write!(f, "SEC EDGAR"),
            DataSource::Finnhub => write!(f, "Finnhub"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FetchStatus {
    Pending,
    Success,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YahooData {
    pub ticker: String,
    pub price: Option<f64>,
    pub market_cap: Option<f64>,
    pub price_52w_change_pct: Option<f64>,  // 52-week price return (%), NOT market cap change
    pub pe_ratio: Option<f64>,
    pub ps_ratio: Option<f64>,
    pub pb_ratio: Option<f64>,
    pub eps: Option<f64>,
    pub eps_growth_yoy: Option<f64>,
    pub revenue_growth_yoy: Option<f64>,
    pub fifty_two_week_high: Option<f64>,
    pub fifty_two_week_low: Option<f64>,
    pub short_interest_percent: Option<f64>,
    pub dividend_yield: Option<f64>,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub institutional_ownership_percent: Option<f64>,
    pub gross_margin: Option<f64>,          // NEW: gross margin as a % (0-100)
    pub operating_margin: Option<f64>,      // NEW: operating margin as a % (may be negative)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FredData {
    pub fed_funds_rate: Option<f64>,
    pub cpi_yoy_change: Option<f64>,
    pub cpi_trend: Option<String>,
    pub fear_and_greed: Option<f64>,
    /// 10Y-2Y Treasury yield spread (%). Negative = inverted curve (recession warning).
    pub yield_curve_spread: Option<f64>,
    /// U-3 unemployment rate (%).
    pub unemployment_rate: Option<f64>,
    /// CBOE volatility index (VIX) — market-wide fear/volatility regime.
    pub vix: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgarData {
    pub ticker: String,
    // NOTE: insider_net_shares_3m and institutional_ownership_percent are removed
    // from EdgarData because they are sourced from Finnhub and Yahoo, respectively.
    pub fcf_latest: Option<f64>,
    pub fcf_growth_yoy: Option<f64>,
    pub debt_to_equity: Option<f64>,
    pub interest_coverage_ratio: Option<f64>,
    pub total_debt: Option<f64>,
    pub cash_and_equivalents: Option<f64>,
    pub actual_eps_growth_yoy: Option<f64>,  // NEW: from EDGAR 10-K filings, not analyst estimates
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinnhubData {
    pub ticker: String,
    pub news_sentiment_score: Option<f64>,
    pub insider_net_shares_3m: Option<i64>,
    pub sector_pe_avg: Option<f64>,
    pub sector_ps_avg: Option<f64>,
    pub sector_growth_score: Option<f64>,
    pub recent_competitor_ipos: Vec<String>,
    /// Days until the next scheduled earnings report (live-only; None in backtest).
    pub next_earnings_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryScore {
    pub name: String,
    pub raw_score: f64,
    pub weight: f64,
    pub weighted_score: f64,
    pub missing_data: bool,
    pub notes: Vec<String>,
    pub coverage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub ticker: String,
    pub horizon: Horizon,
    pub composite_score: f64,
    pub signal: Signal,
    pub valuation: CategoryScore,
    pub fundamentals: CategoryScore,
    pub macro_env: CategoryScore,
    pub sentiment: CategoryScore,
    pub risk: CategoryScore,
    pub yahoo: YahooData,
    pub fred: FredData,
    pub edgar: EdgarData,
    pub finnhub: FinnhubData,
    pub generated_at: DateTime<Utc>,
    pub confidence_score: f64,
}
