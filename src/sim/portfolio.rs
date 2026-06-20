//! The shared portfolio engine used by both the historical backtest and the
//! forward paper-trading simulator.
//!
//! It tracks cash, positions, a trade log, and an equity curve, and applies
//! realistic transaction costs. It knows nothing about *where* prices or signals
//! come from — callers drive it date by date — so the same engine produces an
//! audited track record whether the dates are in the past or the future.

use std::collections::HashMap;
use chrono::NaiveDate;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeAction {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub date: NaiveDate,
    pub ticker: String,
    pub action: TradeAction,
    pub shares: f64,
    pub price: f64,
    /// The composite score that triggered the trade (for later attribution).
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub shares: f64,
    pub avg_cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquityPoint {
    pub date: NaiveDate,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Portfolio {
    pub cash: f64,
    pub starting_cash: f64,
    pub positions: HashMap<String, Position>,
    pub trades: Vec<Trade>,
    pub equity_curve: Vec<EquityPoint>,
    /// Round-trip transaction cost in basis points (e.g. 10.0 = 0.10% per side).
    pub commission_bps: f64,
}

impl Portfolio {
    pub fn new(starting_cash: f64, commission_bps: f64) -> Self {
        Self {
            cash: starting_cash,
            starting_cash,
            positions: HashMap::new(),
            trades: Vec::new(),
            equity_curve: Vec::new(),
            commission_bps: commission_bps.max(0.0),
        }
    }

    fn comm_rate(&self) -> f64 {
        self.commission_bps / 10_000.0
    }

    /// Spend up to `budget` dollars of cash (inclusive of commission) buying `ticker`.
    /// Returns the number of shares acquired. Commission is taken out of the budget,
    /// so a budget equal to `self.cash` never overdraws.
    pub fn buy(&mut self, date: NaiveDate, ticker: &str, price: f64, budget: f64, score: f64) -> f64 {
        let budget = budget.min(self.cash).max(0.0);
        if price <= 0.0 || budget <= 0.0 {
            return 0.0;
        }
        let invested = budget / (1.0 + self.comm_rate());
        let shares = invested / price;
        self.cash -= budget;

        let pos = self
            .positions
            .entry(ticker.to_string())
            .or_insert(Position { shares: 0.0, avg_cost: 0.0 });
        let total_cost = pos.avg_cost * pos.shares + price * shares;
        pos.shares += shares;
        pos.avg_cost = if pos.shares > 0.0 { total_cost / pos.shares } else { 0.0 };

        self.trades.push(Trade {
            date,
            ticker: ticker.to_string(),
            action: TradeAction::Buy,
            shares,
            price,
            score,
        });
        shares
    }

    /// Liquidate the entire position in `ticker` at `price`. Commission is deducted
    /// from the proceeds. No-op if the position doesn't exist.
    pub fn sell_all(&mut self, date: NaiveDate, ticker: &str, price: f64, score: f64) {
        let pos = match self.positions.remove(ticker) {
            Some(p) => p,
            None => return,
        };
        if pos.shares <= 0.0 || price <= 0.0 {
            return;
        }
        let proceeds = pos.shares * price;
        let commission = proceeds * self.comm_rate();
        self.cash += proceeds - commission;
        self.trades.push(Trade {
            date,
            ticker: ticker.to_string(),
            action: TradeAction::Sell,
            shares: pos.shares,
            price,
            score,
        });
    }

    /// Sell approximately `value` dollars worth of `ticker` at `price` (capped at
    /// the current holding). Used to rebalance toward a target weight. Commission
    /// is deducted from proceeds.
    pub fn sell_value(&mut self, date: NaiveDate, ticker: &str, price: f64, value: f64, score: f64) {
        if price <= 0.0 || value <= 0.0 {
            return;
        }
        let comm = self.comm_rate();
        let (sold_shares, remove) = match self.positions.get_mut(ticker) {
            Some(pos) if pos.shares > 0.0 => {
                let holdings_val = pos.shares * price;
                let sell_val = value.min(holdings_val);
                let s = sell_val / price;
                pos.shares -= s;
                (s, pos.shares <= 1e-9)
            }
            _ => return,
        };
        if remove {
            self.positions.remove(ticker);
        }
        if sold_shares > 0.0 {
            let proceeds = sold_shares * price;
            self.cash += proceeds - proceeds * comm;
            self.trades.push(Trade {
                date,
                ticker: ticker.to_string(),
                action: TradeAction::Sell,
                shares: sold_shares,
                price,
                score,
            });
        }
    }

    /// Rebalance the whole portfolio toward `target_weights` (ticker → fraction of
    /// total value, summing to ≤ 1; the remainder stays in cash). Sells first to
    /// free cash, then buys. `band` is the no-trade threshold (fraction of total)
    /// to avoid churning on tiny drifts. `scores` annotates the trades.
    pub fn rebalance_to(
        &mut self,
        date: NaiveDate,
        prices: &HashMap<String, f64>,
        target_weights: &HashMap<String, f64>,
        scores: &HashMap<String, f64>,
        band: f64,
    ) {
        let total = self.total_value(prices);
        if total <= 0.0 {
            return;
        }

        // 1. Sell positions that are untargeted or overweight (frees cash first).
        let held: Vec<String> = self.positions.keys().cloned().collect();
        for t in held {
            let price = match prices.get(&t) {
                Some(p) => *p,
                None => continue,
            };
            let desired = total * target_weights.get(&t).copied().unwrap_or(0.0);
            let current = self.positions.get(&t).map(|p| p.shares * price).unwrap_or(0.0);
            let diff = desired - current;
            if diff < -total * band {
                let score = scores.get(&t).copied().unwrap_or(0.0);
                self.sell_value(date, &t, price, -diff, score);
            }
        }

        // 2. Buy underweight targets with the freed cash.
        for (t, w) in target_weights {
            let price = match prices.get(t) {
                Some(p) => *p,
                None => continue,
            };
            let desired = total * w;
            let current = self.positions.get(t).map(|p| p.shares * price).unwrap_or(0.0);
            let diff = desired - current;
            if diff > total * band {
                let score = scores.get(t).copied().unwrap_or(0.0);
                self.buy(date, t, price, diff.min(self.cash), score);
            }
        }
    }

    /// Marked-to-market value of all open positions, using `prices` (falling back
    /// to a position's average cost if a price is unavailable for that ticker).
    pub fn holdings_value(&self, prices: &HashMap<String, f64>) -> f64 {
        self.positions
            .iter()
            .map(|(t, p)| {
                let px = prices.get(t).copied().unwrap_or(p.avg_cost);
                p.shares * px
            })
            .sum()
    }

    /// Total portfolio value = cash + marked-to-market holdings.
    pub fn total_value(&self, prices: &HashMap<String, f64>) -> f64 {
        self.cash + self.holdings_value(prices)
    }

    /// Record an equity-curve point at `date` using the supplied prices.
    pub fn mark(&mut self, date: NaiveDate, prices: &HashMap<String, f64>) {
        let value = self.total_value(prices);
        self.equity_curve.push(EquityPoint { date, value });
    }

    /// The equity curve as a bare slice of values (for the metrics module).
    pub fn equity_values(&self) -> Vec<f64> {
        self.equity_curve.iter().map(|p| p.value).collect()
    }

    /// Fraction of closed round trips (a BUY later followed by a SELL of the same
    /// ticker) that were profitable, as a percentage. Returns (hit_rate_pct, closed_trips).
    pub fn hit_rate(&self) -> (f64, usize) {
        // Track the last buy price per ticker; pair it with the next sell.
        let mut last_buy: HashMap<String, f64> = HashMap::new();
        let mut wins = 0usize;
        let mut total = 0usize;
        for t in &self.trades {
            match t.action {
                TradeAction::Buy => {
                    last_buy.insert(t.ticker.clone(), t.price);
                }
                TradeAction::Sell => {
                    if let Some(buy_px) = last_buy.remove(&t.ticker) {
                        total += 1;
                        if t.price > buy_px {
                            wins += 1;
                        }
                    }
                }
            }
        }
        if total == 0 {
            (0.0, 0)
        } else {
            (wins as f64 / total as f64 * 100.0, total)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn prices(ticker: &str, px: f64) -> HashMap<String, f64> {
        let mut m = HashMap::new();
        m.insert(ticker.to_string(), px);
        m
    }

    #[test]
    fn test_buy_and_mark_no_commission() {
        let mut p = Portfolio::new(10_000.0, 0.0);
        let shares = p.buy(d("2020-01-01"), "AAPL", 100.0, p.cash, 80.0);
        assert!((shares - 100.0).abs() < 1e-9);
        assert!(p.cash.abs() < 1e-9);
        // Price rises to 120 → value 12,000 → +20%
        let pr = prices("AAPL", 120.0);
        assert!((p.total_value(&pr) - 12_000.0).abs() < 1e-6);
    }

    #[test]
    fn test_sell_all_realizes_cash() {
        let mut p = Portfolio::new(10_000.0, 0.0);
        p.buy(d("2020-01-01"), "AAPL", 100.0, p.cash, 80.0);
        p.sell_all(d("2021-01-01"), "AAPL", 150.0, 30.0);
        assert!(p.positions.is_empty());
        assert!((p.cash - 15_000.0).abs() < 1e-6);
    }

    #[test]
    fn test_commission_costs_money() {
        // 10 bps each side. Buy then immediately sell at same price loses ~2x commission.
        let mut p = Portfolio::new(10_000.0, 10.0);
        p.buy(d("2020-01-01"), "AAPL", 100.0, p.cash, 80.0);
        p.sell_all(d("2020-01-02"), "AAPL", 100.0, 20.0);
        assert!(p.cash < 10_000.0);
        assert!(p.cash > 9_970.0); // roughly two 0.1% hits
    }

    #[test]
    fn test_buy_never_overdraws() {
        let mut p = Portfolio::new(1_000.0, 25.0);
        p.buy(d("2020-01-01"), "AAPL", 50.0, 5_000.0, 80.0); // budget exceeds cash
        assert!(p.cash >= -1e-9);
    }

    #[test]
    fn test_sell_value_partial() {
        let mut p = Portfolio::new(10_000.0, 0.0);
        p.buy(d("2020-01-01"), "AAPL", 100.0, p.cash, 80.0); // 100 shares
        // Sell $4,000 worth at $100 → 40 shares, 60 remain.
        p.sell_value(d("2020-02-01"), "AAPL", 100.0, 4_000.0, 50.0);
        let pos = p.positions.get("AAPL").unwrap();
        assert!((pos.shares - 60.0).abs() < 1e-6);
        assert!((p.cash - 4_000.0).abs() < 1e-6);
    }

    #[test]
    fn test_sell_value_caps_at_holding() {
        let mut p = Portfolio::new(10_000.0, 0.0);
        p.buy(d("2020-01-01"), "AAPL", 100.0, p.cash, 80.0);
        // Ask to sell more than held → liquidates and removes the position.
        p.sell_value(d("2020-02-01"), "AAPL", 100.0, 99_999.0, 50.0);
        assert!(p.positions.is_empty());
        assert!((p.cash - 10_000.0).abs() < 1e-6);
    }

    #[test]
    fn test_rebalance_to_targets() {
        let mut p = Portfolio::new(10_000.0, 0.0);
        let mut prices = HashMap::new();
        prices.insert("A".to_string(), 100.0);
        prices.insert("B".to_string(), 50.0);
        let mut targets = HashMap::new();
        targets.insert("A".to_string(), 0.5);
        targets.insert("B".to_string(), 0.5);
        let mut scores = HashMap::new();
        scores.insert("A".to_string(), 80.0);
        scores.insert("B".to_string(), 70.0);

        p.rebalance_to(d("2020-01-01"), &prices, &targets, &scores, 0.02);
        assert!((p.positions.get("A").unwrap().shares - 50.0).abs() < 1e-6); // $5k / $100
        assert!((p.positions.get("B").unwrap().shares - 100.0).abs() < 1e-6); // $5k / $50
        assert!(p.cash.abs() < 1e-6);

        // Drop B entirely; pile into A.
        targets.clear();
        targets.insert("A".to_string(), 1.0);
        p.rebalance_to(d("2020-02-01"), &prices, &targets, &scores, 0.02);
        assert!(p.positions.get("B").is_none());
        assert!((p.total_value(&prices) - 10_000.0).abs() < 1e-6);
    }

    #[test]
    fn test_hit_rate() {
        let mut p = Portfolio::new(30_000.0, 0.0);
        // Win
        p.buy(d("2020-01-01"), "AAA", 100.0, 10_000.0, 80.0);
        p.sell_all(d("2020-06-01"), "AAA", 150.0, 30.0);
        // Loss
        p.buy(d("2020-02-01"), "BBB", 100.0, 10_000.0, 80.0);
        p.sell_all(d("2020-07-01"), "BBB", 80.0, 30.0);
        let (rate, trips) = p.hit_rate();
        assert_eq!(trips, 2);
        assert!((rate - 50.0).abs() < 1e-9);
    }
}
