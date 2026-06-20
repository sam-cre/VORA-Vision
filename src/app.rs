use crate::models::{Horizon, AnalysisResult, DataSource, FetchStatus};
use crate::sim::BacktestResult;
use crate::sim::Calibration;
use crate::sim::paper::PaperPortfolio;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Input,
    Loading,
    Results,
    Compare,
    Backtest,
    Paper,
    Calibrate,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFocus {
    TextInput,
    HorizonSelector,
}

pub struct App {
    pub state: AppState,
    pub input_buffer: String,
    pub horizon: Horizon,
    pub focus: InputFocus,
    pub active_tab: usize,
    pub fetch_progress: Vec<(DataSource, FetchStatus)>,
    pub results: Vec<AnalysisResult>,
    pub compare_cursor: usize,
    pub active_result_idx: usize,
    pub tick_count: u64,
    pub loading_label: String,
    /// Result of the most recent historical backtest, shown on the Backtest screen.
    pub backtest: Option<BacktestResult>,
    /// Trailing-years window for backtests (default 5).
    pub backtest_years: i64,
    /// Persistent forward paper-trading portfolio.
    pub paper: PaperPortfolio,
    /// Transient status line (e.g. confirmation after a paper trade).
    pub status_message: Option<String>,
    /// Most recent score calibration (forward returns by score band).
    pub calibration: Option<Calibration>,
    /// Composite score of the focused ticker, to highlight its band on the calibration screen.
    pub calib_current_score: Option<f64>,
}

impl App {
    pub fn new() -> Self {
        Self {
            state: AppState::Input,
            input_buffer: String::new(),
            horizon: Horizon::Medium, // Default horizon
            focus: InputFocus::TextInput,
            active_tab: 0,
            fetch_progress: vec![
                (DataSource::Yahoo, FetchStatus::Pending),
                (DataSource::Fred, FetchStatus::Pending),
                (DataSource::Edgar, FetchStatus::Pending),
                (DataSource::Finnhub, FetchStatus::Pending),
            ],
            results: Vec::new(),
            compare_cursor: 0,
            active_result_idx: 0,
            tick_count: 0,
            loading_label: String::new(),
            backtest: None,
            backtest_years: 5,
            paper: PaperPortfolio::load(),
            status_message: None,
            calibration: None,
            calib_current_score: None,
        }
    }
}
