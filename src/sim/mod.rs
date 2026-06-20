//! Simulation engine.
//!
//! `portfolio` is the shared core (cash, positions, trades, equity curve) used by
//! both the historical backtest and the forward paper-trading simulator.
//! `metrics` turns an equity curve into performance statistics.

pub mod backtest;
pub mod calibrate;
pub mod data;
pub mod metrics;
pub mod paper;
pub mod portfolio;
pub mod runner;

pub use backtest::{run_single_ticker, BacktestResult, Observation};
pub use calibrate::{Calibration, CalibrationBucket};
pub use portfolio::{EquityPoint, Portfolio, Position, Trade, TradeAction};
pub use runner::{run_backtest, run_calibration, run_portfolio_backtest};
