//! Forward paper-trading portfolio.
//!
//! Unlike the backtest, this can't cheat: it follows the *live* signals going
//! forward and persists to disk, building an audited track record over time.
//! Each time the user acts on a signal, we record the trade, mark the portfolio
//! to market with the latest known prices, and save.

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::models::Signal;
use crate::sim::portfolio::Portfolio;
use crate::sim::runner::{DEFAULT_COMMISSION_BPS, DEFAULT_START_CASH};

const PAPER_FILE: &str = ".cache/portfolio.json";

/// Maximum fraction of total portfolio value to put into any single new position.
const MAX_POSITION_FRACTION: f64 = 0.20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperPortfolio {
    pub portfolio: Portfolio,
    /// Last price we observed for each ticker (used to mark holdings to market
    /// even when we only just fetched one ticker this session).
    pub last_prices: HashMap<String, f64>,
    pub created: NaiveDate,
    pub last_updated: NaiveDate,
}

impl PaperPortfolio {
    pub fn new() -> Self {
        let today = Utc::now().date_naive();
        Self {
            portfolio: Portfolio::new(DEFAULT_START_CASH, DEFAULT_COMMISSION_BPS),
            last_prices: HashMap::new(),
            created: today,
            last_updated: today,
        }
    }

    /// Load the saved paper portfolio, or start a fresh one.
    pub fn load() -> Self {
        File::open(PAPER_FILE)
            .ok()
            .and_then(|f| serde_json::from_reader(f).ok())
            .unwrap_or_else(Self::new)
    }

    /// Persist atomically (temp file + rename), mirroring the cache module.
    pub fn save(&self) {
        if let Some(parent) = Path::new(PAPER_FILE).parent() {
            if fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let tmp = format!("{}.tmp", PAPER_FILE);
        if let Ok(file) = File::create(&tmp) {
            if serde_json::to_writer_pretty(file, self).is_ok() {
                let _ = fs::rename(&tmp, PAPER_FILE);
            } else {
                let _ = fs::remove_file(&tmp);
            }
        }
    }

    /// Total marked-to-market value using the last known prices.
    pub fn total_value(&self) -> f64 {
        self.portfolio.total_value(&self.last_prices)
    }

    /// Return since inception (%).
    pub fn total_return_pct(&self) -> f64 {
        if self.portfolio.starting_cash.abs() < f64::EPSILON {
            return 0.0;
        }
        (self.total_value() / self.portfolio.starting_cash - 1.0) * 100.0
    }

    /// Act on a live signal for `ticker` at `price`.
    /// BUY → open/keep a position up to `MAX_POSITION_FRACTION` of total value;
    /// SELL → liquidate; HOLD → just refresh the mark. Records an equity point.
    pub fn apply_signal(&mut self, ticker: &str, signal: Signal, price: f64, score: f64) {
        if price <= 0.0 {
            return;
        }
        let today = Utc::now().date_naive();
        self.last_prices.insert(ticker.to_string(), price);

        match signal {
            Signal::Buy => {
                let already_holds = self
                    .portfolio
                    .positions
                    .get(ticker)
                    .map(|p| p.shares > 0.0)
                    .unwrap_or(false);
                if !already_holds {
                    let total = self.total_value();
                    let budget = (total * MAX_POSITION_FRACTION).min(self.portfolio.cash);
                    if budget > 0.0 {
                        self.portfolio.buy(today, ticker, price, budget, score);
                    }
                }
            }
            Signal::Sell => self.portfolio.sell_all(today, ticker, price, score),
            Signal::Hold => {}
        }

        self.portfolio.mark(today, &self.last_prices);
        self.last_updated = today;
        self.save();
    }
}

impl Default for PaperPortfolio {
    fn default() -> Self {
        Self::new()
    }
}
