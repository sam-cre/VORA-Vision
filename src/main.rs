#[macro_use]
pub mod logger;
pub mod app;
pub mod models;
pub mod ui;
pub mod data;
pub mod scoring;
pub mod cache;
pub mod sim;

use app::{App, AppState, InputFocus};
use models::{Horizon, DataSource, FetchStatus, AnalysisResult};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::{Duration, Instant};
use crossterm::event::{self, Event, KeyCode};

pub enum FetchEvent {
    Progress(DataSource, FetchStatus),
    /// Which ticker is currently being processed: (ticker, index (1-based), total).
    TickerProgress(String, usize, usize),
    Success(Vec<AnalysisResult>),
    BacktestDone(Box<sim::BacktestResult>),
    /// (calibration, open_full_screen). Auto-runs after analysis pass `false`
    /// (populate inline only); an explicit K press passes `true`.
    CalibrationDone(Box<sim::Calibration>, bool),
    Error(String),
}

struct TerminalGuard;

impl TerminalGuard {
    fn init() -> Result<Self, io::Error> {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::event::EnableMouseCapture
        )?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture
        );
    }
}

fn load_env_file() {
    // Look next to the executable first (installed), then fall back to cwd (dev)
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(".env")));
    let candidates = [exe_dir, Some(std::path::PathBuf::from(".env"))];

    for path in candidates.into_iter().flatten() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    std::env::set_var(key, value);
                }
            }
            log_info!("Loaded environment variables from {}", path.display());
            return;
        }
    }
    log_warn!("No .env file found next to executable or in working directory");
}

async fn run_fetch_task(
    tickers: Vec<String>,
    horizon: Horizon,
    tx: tokio::sync::mpsc::UnboundedSender<FetchEvent>,
) {
    use data::{fred::fetch_fred, yahoo::fetch_yahoo, edgar::fetch_edgar, finnhub::fetch_finnhub};
    use scoring::engine::calculate_analysis;

    log_info!("Starting fetch task for tickers: {:?}", tickers);

    // Check FRED_API_KEY and FINNHUB_API_KEY
    if std::env::var("FRED_API_KEY").is_err() {
        log_error!("Missing FRED_API_KEY environment variable");
        let _ = tx.send(FetchEvent::Error("Missing environment variable: FRED_API_KEY. Set it and restart.".to_string()));
        return;
    }
    if std::env::var("FINNHUB_API_KEY").is_err() {
        log_error!("Missing FINNHUB_API_KEY environment variable");
        let _ = tx.send(FetchEvent::Error("Missing environment variable: FINNHUB_API_KEY. Set it and restart.".to_string()));
        return;
    }

    // FRED fetch
    let _ = tx.send(FetchEvent::Progress(DataSource::Fred, FetchStatus::Pending));
    let fred_data = if let Some(cached) = cache::load_from_cache::<models::FredData>("fred.json", cache::TTL_FRED_SECS) {
        let _ = tx.send(FetchEvent::Progress(DataSource::Fred, FetchStatus::Success));
        cached
    } else {
        match fetch_fred().await {
            Ok(d) => {
                let _ = tx.send(FetchEvent::Progress(DataSource::Fred, FetchStatus::Success));
                cache::save_to_cache("fred.json", &d);
                d
            }
            Err(e) => {
                log_error!("FRED fetch failed: {}", e);
                let _ = tx.send(FetchEvent::Progress(DataSource::Fred, FetchStatus::Failed(e.to_string())));
                models::FredData { fed_funds_rate: None, cpi_yoy_change: None, cpi_trend: None, fear_and_greed: None, yield_curve_spread: None, unemployment_rate: None, vix: None }
            }
        }
    };

    let mut results = Vec::new();
    let mut overall_success = false;

    let total_tickers = tickers.len();
    for (ticker_idx, ticker) in tickers.into_iter().enumerate() {
        let _ = tx.send(FetchEvent::TickerProgress(ticker.clone(), ticker_idx + 1, total_tickers));
        let yahoo_cache_file = format!("{}_yahoo.json", ticker);
        let edgar_cache_file = format!("{}_edgar.json", ticker);
        let finnhub_cache_file = format!("{}_finnhub.json", ticker);

        // Yahoo fetch
        let _ = tx.send(FetchEvent::Progress(DataSource::Yahoo, FetchStatus::Pending));
        let yahoo_data = if let Some(cached) = cache::load_from_cache::<models::YahooData>(&yahoo_cache_file, cache::TTL_YAHOO_SECS) {
            let _ = tx.send(FetchEvent::Progress(DataSource::Yahoo, FetchStatus::Success));
            cached
        } else {
            match fetch_yahoo(&ticker).await {
                Ok(d) => {
                    let _ = tx.send(FetchEvent::Progress(DataSource::Yahoo, FetchStatus::Success));
                    cache::save_to_cache(&yahoo_cache_file, &d);
                    d
                }
                Err(e) => {
                    log_error!("Yahoo fetch failed for {}: {}", ticker, e);
                    let _ = tx.send(FetchEvent::Progress(DataSource::Yahoo, FetchStatus::Failed(e.to_string())));
                    models::YahooData {
                        ticker: ticker.clone(),
                        price: None,
                        market_cap: None,
                        price_52w_change_pct: None,
                        pe_ratio: None,
                        ps_ratio: None,
                        pb_ratio: None,
                        eps: None,
                        eps_growth_yoy: None,
                        revenue_growth_yoy: None,
                        fifty_two_week_high: None,
                        fifty_two_week_low: None,
                        short_interest_percent: None,
                        dividend_yield: None,
                        sector: None,
                        industry: None,
                        institutional_ownership_percent: None,
                        gross_margin: None,
                        operating_margin: None,
                    }
                }
            }
        };

        let sector = yahoo_data.sector.clone().unwrap_or_default();

        // Edgar and Finnhub fetch
        let _ = tx.send(FetchEvent::Progress(DataSource::Edgar, FetchStatus::Pending));
        let _ = tx.send(FetchEvent::Progress(DataSource::Finnhub, FetchStatus::Pending));

        let cached_edgar = cache::load_from_cache::<models::EdgarData>(&edgar_cache_file, cache::TTL_EDGAR_SECS);
        let cached_finnhub = cache::load_from_cache::<models::FinnhubData>(&finnhub_cache_file, cache::TTL_FINNHUB_SECS);

        let (edgar_res, finnhub_res) = match (cached_edgar, cached_finnhub) {
            (Some(ed), Some(fh)) => {
                let _ = tx.send(FetchEvent::Progress(DataSource::Edgar, FetchStatus::Success));
                let _ = tx.send(FetchEvent::Progress(DataSource::Finnhub, FetchStatus::Success));
                (Ok(ed), Ok(fh))
            }
            (Some(ed), None) => {
                let _ = tx.send(FetchEvent::Progress(DataSource::Edgar, FetchStatus::Success));
                let fh_res = fetch_finnhub(&ticker, &sector).await;
                (Ok(ed), fh_res)
            }
            (None, Some(fh)) => {
                let _ = tx.send(FetchEvent::Progress(DataSource::Finnhub, FetchStatus::Success));
                let ed_res = fetch_edgar(&ticker).await;
                (ed_res, Ok(fh))
            }
            (None, None) => {
                let ed_fut = fetch_edgar(&ticker);
                let fh_fut = fetch_finnhub(&ticker, &sector);
                let (ed_res, fh_res) = tokio::join!(ed_fut, fh_fut);
                (ed_res, fh_res)
            }
        };

        let edgar_data = match edgar_res {
            Ok(d) => {
                let _ = tx.send(FetchEvent::Progress(DataSource::Edgar, FetchStatus::Success));
                cache::save_to_cache(&edgar_cache_file, &d);
                d
            }
            Err(e) => {
                log_error!("EDGAR fetch failed for {}: {}", ticker, e);
                let _ = tx.send(FetchEvent::Progress(DataSource::Edgar, FetchStatus::Failed(e.to_string())));
                models::EdgarData {
                    ticker: ticker.clone(),
                    fcf_latest: None,
                    fcf_growth_yoy: None,
                    debt_to_equity: None,
                    interest_coverage_ratio: None,
                    total_debt: None,
                    cash_and_equivalents: None,
                    actual_eps_growth_yoy: None,
                }
            }
        };

        let finnhub_data = match finnhub_res {
            Ok(d) => {
                let _ = tx.send(FetchEvent::Progress(DataSource::Finnhub, FetchStatus::Success));
                cache::save_to_cache(&finnhub_cache_file, &d);
                d
            }
            Err(e) => {
                log_error!("Finnhub fetch failed for {}: {}", ticker, e);
                let _ = tx.send(FetchEvent::Progress(DataSource::Finnhub, FetchStatus::Failed(e.to_string())));
                models::FinnhubData {
                    ticker: ticker.clone(),
                    news_sentiment_score: None,
                    insider_net_shares_3m: None,
                    sector_pe_avg: None,
                    sector_ps_avg: None,
                    sector_growth_score: None,
                    recent_competitor_ipos: Vec::new(),
                    next_earnings_days: None,
                }
            }
        };

        // Check if all sources failed
        let yahoo_failed = yahoo_data.price.is_none() && yahoo_data.market_cap.is_none();
        let fred_failed = fred_data.fed_funds_rate.is_none();
        let edgar_failed = edgar_data.fcf_latest.is_none() && edgar_data.debt_to_equity.is_none();
        let finnhub_failed = finnhub_data.insider_net_shares_3m.is_none()
            && finnhub_data.sector_growth_score.is_none()
            && finnhub_data.recent_competitor_ipos.is_empty();

        if yahoo_failed && fred_failed && edgar_failed && finnhub_failed {
            log_warn!("All data sources failed for ticker: {}", ticker);
            continue;
        }

        overall_success = true;
        let analysis = calculate_analysis(&ticker, horizon, &yahoo_data, &fred_data, &edgar_data, &finnhub_data);
        results.push(analysis);
    }

    if overall_success {
        log_info!("Successful analysis generated for: {:?}", results.iter().map(|r| &r.ticker).collect::<Vec<_>>());
        let _ = tx.send(FetchEvent::Success(results));
    } else {
        log_error!("All data sources failed for all entered tickers.");
        let _ = tx.send(FetchEvent::Error("All data sources failed. Check API keys and internet connection.".to_string()));
    }
}

async fn run_backtest_task(
    ticker: String,
    horizon: Horizon,
    years: i64,
    tx: tokio::sync::mpsc::UnboundedSender<FetchEvent>,
) {
    use sim::runner::{run_backtest, DEFAULT_COMMISSION_BPS, DEFAULT_START_CASH};
    log_info!("Starting backtest for {} ({}y)", ticker, years);
    match run_backtest(&ticker, horizon, years, DEFAULT_START_CASH, DEFAULT_COMMISSION_BPS).await {
        Ok(res) => {
            log_info!("Backtest complete for {}", ticker);
            let _ = tx.send(FetchEvent::BacktestDone(Box::new(res)));
        }
        Err(e) => {
            log_error!("Backtest failed for {}: {}", ticker, e);
            let _ = tx.send(FetchEvent::Error(format!("Backtest failed: {}", e)));
        }
    }
}

/// 20-stock diversified universe used by the Quick Backtest (X key).
/// Covers 8 sectors so results aren't skewed by a single theme.
const QUICK_UNIVERSE: &[&str] = &[
    // Technology
    "AAPL", "MSFT", "GOOGL", "NVDA", "META",
    // Financials
    "JPM", "V", "BAC",
    // Healthcare
    "JNJ", "UNH",
    // Consumer
    "AMZN", "WMT", "HD",
    // Energy
    "XOM", "CVX",
    // Industrials
    "CAT", "BA",
    // Telecom
    "T", "VZ",
    // Staples
    "PG",
];

async fn run_portfolio_backtest_task(
    tickers: Vec<String>,
    horizon: Horizon,
    years: i64,
    tx: tokio::sync::mpsc::UnboundedSender<FetchEvent>,
) {
    use sim::runner::{run_portfolio_backtest, DEFAULT_COMMISSION_BPS, DEFAULT_START_CASH, DEFAULT_TOP_N};
    log_info!("Starting portfolio backtest for {:?} ({}y)", tickers, years);
    let top_n = DEFAULT_TOP_N.min(tickers.len());
    match run_portfolio_backtest(&tickers, horizon, years, DEFAULT_START_CASH, DEFAULT_COMMISSION_BPS, top_n).await {
        Ok(res) => {
            log_info!("Portfolio backtest complete");
            let _ = tx.send(FetchEvent::BacktestDone(Box::new(res)));
        }
        Err(e) => {
            log_error!("Portfolio backtest failed: {}", e);
            let _ = tx.send(FetchEvent::Error(format!("Portfolio backtest failed: {}", e)));
        }
    }
}

async fn run_calibration_task(
    tickers: Vec<String>,
    horizon: Horizon,
    years: i64,
    open_screen: bool,
    tx: tokio::sync::mpsc::UnboundedSender<FetchEvent>,
) {
    use sim::runner::run_calibration;
    log_info!("Starting calibration for {:?} ({}y, open={})", tickers, years, open_screen);
    match run_calibration(&tickers, horizon, years).await {
        Ok(cal) => {
            log_info!("Calibration complete ({} obs)", cal.n_total);
            let _ = tx.send(FetchEvent::CalibrationDone(Box::new(cal), open_screen));
        }
        Err(e) => {
            log_error!("Calibration failed: {}", e);
            // Auto (background) calibration fails silently so it never hijacks the
            // results screen; only an explicit K press surfaces the error.
            if open_screen {
                let _ = tx.send(FetchEvent::Error(format!("Calibration failed: {}", e)));
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    load_env_file();
    log_info!("Starting VORA-Vision terminal application");

    let _guard = TerminalGuard::init()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    // Create channel for background fetch communications
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<FetchEvent>();

    loop {
        terminal.draw(|f| {
            match &app.state {
                AppState::Input => ui::input::render(f, &app),
                AppState::Loading => ui::loading::render(f, &app),
                AppState::Results => ui::results::render(f, &app),
                AppState::Compare => ui::compare::render(f, &app),
                AppState::Backtest => ui::backtest::render(f, &app),
                AppState::Paper => ui::paper::render(f, &app),
                AppState::Calibrate => ui::calibrate::render(f, &app),
                AppState::Error(err_msg) => ui::error::render(f, &app, err_msg),
            }
        })?;


        // Check for progress or results from background thread
        while let Ok(event) = rx.try_recv() {
            match event {
                FetchEvent::Progress(source, status) => {
                    for (src, stat) in &mut app.fetch_progress {
                        if *src == source {
                            *stat = status.clone();
                        }
                    }
                }
                FetchEvent::TickerProgress(ticker, idx, total) => {
                    app.loading_label = if total > 1 {
                        format!("Analyzing {} ({}/{})", ticker, idx, total)
                    } else {
                        format!("Analyzing {}", ticker)
                    };
                }
                FetchEvent::Success(results) => {
                    app.results = results;
                    app.active_result_idx = 0;
                    // Recompute calibration for the new analysis; kick it off in the
                    // background so the expected-return band fills in without a keypress.
                    app.calibration = None;
                    let cal_tickers: Vec<String> = app.results.iter().map(|r| r.ticker.clone()).collect();
                    let cal_horizon = app.results.first().map(|r| r.horizon).unwrap_or(Horizon::Medium);
                    if !cal_tickers.is_empty() {
                        let years = app.backtest_years;
                        let txc = tx.clone();
                        tokio::spawn(async move {
                            run_calibration_task(cal_tickers, cal_horizon, years, false, txc).await;
                        });
                    }
                    if app.results.len() >= 2 {
                        app.state = AppState::Compare;
                    } else {
                        app.state = AppState::Results;
                    }
                }
                FetchEvent::BacktestDone(res) => {
                    app.backtest = Some(*res);
                    app.loading_label.clear();
                    app.state = AppState::Backtest;
                }
                FetchEvent::CalibrationDone(cal, open_screen) => {
                    app.calibration = Some(*cal);
                    app.loading_label.clear();
                    if open_screen {
                        app.state = AppState::Calibrate;
                    }
                }
                FetchEvent::Error(err_msg) => {
                    app.state = AppState::Error(err_msg);
                }
            }
        }

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    match &app.state {
                        AppState::Input => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') => {
                                break;
                            }
                            KeyCode::Tab => {
                                app.focus = match app.focus {
                                    InputFocus::TextInput => InputFocus::HorizonSelector,
                                    InputFocus::HorizonSelector => InputFocus::TextInput,
                                };
                            }
                            KeyCode::Backspace => {
                                if app.focus == InputFocus::TextInput {
                                    app.input_buffer.pop();
                                }
                            }
                            KeyCode::Left => {
                                if app.focus == InputFocus::HorizonSelector {
                                    app.horizon = match app.horizon {
                                        Horizon::Short => Horizon::Long,
                                        Horizon::Medium => Horizon::Short,
                                        Horizon::Long => Horizon::Medium,
                                    };
                                }
                            }
                            KeyCode::Right => {
                                if app.focus == InputFocus::HorizonSelector {
                                    app.horizon = match app.horizon {
                                        Horizon::Short => Horizon::Medium,
                                        Horizon::Medium => Horizon::Long,
                                        Horizon::Long => Horizon::Short,
                                    };
                                }
                            }
                            KeyCode::Enter => {
                                let tickers: Vec<String> = app.input_buffer
                                    .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                                    .map(|s| s.trim().to_uppercase())
                                    // Allow class shares (BRK.B), digits, and hyphens; require at
                                    // least one letter and a sane length.
                                    .filter(|s| {
                                        !s.is_empty()
                                            && s.len() <= 8
                                            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                                            && s.chars().any(|c| c.is_ascii_alphabetic())
                                    })
                                    .collect();

                                if !tickers.is_empty() {
                                    app.state = AppState::Loading;
                                    app.fetch_progress = vec![
                                        (DataSource::Yahoo, FetchStatus::Pending),
                                        (DataSource::Fred, FetchStatus::Pending),
                                        (DataSource::Edgar, FetchStatus::Pending),
                                        (DataSource::Finnhub, FetchStatus::Pending),
                                    ];
                                    app.results.clear();
                                    app.compare_cursor = 0;
                                    app.active_tab = 0;
                                    app.active_result_idx = 0;
                                    app.loading_label.clear();

                                    let tx = tx.clone();
                                    let horizon = app.horizon;
                                    tokio::spawn(async move {
                                        run_fetch_task(tickers, horizon, tx).await;
                                    });
                                }
                            }
                            KeyCode::Char('x') | KeyCode::Char('X') => {
                                // Quick Backtest: run portfolio backtest on the built-in
                                // diversified universe without needing to type any tickers.
                                let tickers: Vec<String> = QUICK_UNIVERSE.iter().map(|s| s.to_string()).collect();
                                let horizon = app.horizon;
                                let years = app.backtest_years;
                                app.loading_label = format!(
                                    "Quick Backtest: {} stocks vs S&P 500 ({}y) — this may take ~60s",
                                    tickers.len(), years
                                );
                                app.state = AppState::Loading;
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    run_portfolio_backtest_task(tickers, horizon, years, tx).await;
                                });
                            }
                            KeyCode::Char(c) => {
                                if app.focus == InputFocus::TextInput {
                                    app.input_buffer.push(c);
                                }
                            }
                            _ => {}
                        },
                        AppState::Loading => {
                            if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('Q') {
                                break;
                            }
                        }
                        AppState::Results => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') => {
                                break;
                            }
                            KeyCode::Left => {
                                app.active_tab = if app.active_tab == 0 { 7 } else { app.active_tab - 1 };
                            }
                            KeyCode::Right => {
                                app.active_tab = (app.active_tab + 1) % 8;
                            }
                            KeyCode::Char('c') | KeyCode::Char('C') => {
                                if app.results.len() >= 2 {
                                    app.state = AppState::Compare;
                                }
                            }
                            KeyCode::Char('b') | KeyCode::Char('B') => {
                                let info = app.results.get(app.active_result_idx)
                                    .map(|r| (r.ticker.clone(), r.horizon));
                                if let Some((ticker, horizon)) = info {
                                    let years = app.backtest_years;
                                    app.loading_label = format!("Backtesting {} ({}y) — this can take ~20s", ticker, years);
                                    app.state = AppState::Loading;
                                    let tx = tx.clone();
                                    tokio::spawn(async move {
                                        run_backtest_task(ticker, horizon, years, tx).await;
                                    });
                                }
                            }
                            KeyCode::Char('p') | KeyCode::Char('P') => {
                                let info = app.results.get(app.active_result_idx)
                                    .and_then(|r| r.yahoo.price.map(|px| (r.ticker.clone(), r.signal, r.composite_score, px)));
                                if let Some((ticker, signal, score, price)) = info {
                                    app.paper.apply_signal(&ticker, signal, price, score);
                                    app.status_message = Some(format!("Paper portfolio updated: {} {}", ticker, signal));
                                    app.state = AppState::Paper;
                                } else {
                                    app.status_message = Some("No live price available to paper-trade.".to_string());
                                }
                            }
                            KeyCode::Char('v') | KeyCode::Char('V') => {
                                app.status_message = None;
                                app.state = AppState::Paper;
                            }
                            KeyCode::Char('k') | KeyCode::Char('K') => {
                                let tickers: Vec<String> = app.results.iter().map(|r| r.ticker.clone()).collect();
                                let horizon = app.results.first().map(|r| r.horizon).unwrap_or(Horizon::Medium);
                                app.calib_current_score = app.results.get(app.active_result_idx).map(|r| r.composite_score);
                                if !tickers.is_empty() {
                                    let years = app.backtest_years;
                                    app.loading_label = format!("Calibrating {} name(s) ({}y) — this can take a while", tickers.len(), years);
                                    app.state = AppState::Loading;
                                    let tx = tx.clone();
                                    tokio::spawn(async move {
                                        run_calibration_task(tickers, horizon, years, true, tx).await;
                                    });
                                }
                            }
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                app.state = AppState::Input;
                                app.input_buffer.clear();
                                app.results.clear();
                            }
                            _ => {}
                        },
                        AppState::Backtest => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') => break,
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                app.state = AppState::Input;
                                app.input_buffer.clear();
                                app.results.clear();
                                app.backtest = None;
                            }
                            KeyCode::Esc | KeyCode::Enter => {
                                app.state = if app.results.is_empty() { AppState::Input } else { AppState::Results };
                            }
                            _ => {}
                        },
                        AppState::Paper => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') => break,
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                app.state = AppState::Input;
                                app.input_buffer.clear();
                                app.results.clear();
                            }
                            KeyCode::Esc | KeyCode::Enter => {
                                app.state = if app.results.is_empty() { AppState::Input } else { AppState::Results };
                            }
                            _ => {}
                        },
                        AppState::Calibrate => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') => break,
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                app.state = AppState::Input;
                                app.input_buffer.clear();
                                app.results.clear();
                                app.calibration = None;
                            }
                            KeyCode::Esc | KeyCode::Enter => {
                                app.state = if app.results.is_empty() { AppState::Input } else { AppState::Results };
                            }
                            _ => {}
                        },
                        AppState::Compare => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') => {
                                break;
                            }
                            KeyCode::Left => {
                                if !app.results.is_empty() {
                                    app.compare_cursor = if app.compare_cursor == 0 {
                                        app.results.len() - 1
                                    } else {
                                        app.compare_cursor - 1
                                    };
                                }
                            }
                            KeyCode::Right => {
                                if !app.results.is_empty() {
                                    app.compare_cursor = (app.compare_cursor + 1) % app.results.len();
                                }
                            }
                            KeyCode::Enter => {
                                if !app.results.is_empty() {
                                    app.active_result_idx = app.compare_cursor;
                                    app.state = AppState::Results;
                                    app.active_tab = 0;
                                }
                            }
                            KeyCode::Char('b') | KeyCode::Char('B') => {
                                let tickers: Vec<String> = app.results.iter().map(|r| r.ticker.clone()).collect();
                                let horizon = app.results.first().map(|r| r.horizon).unwrap_or(Horizon::Medium);
                                if tickers.len() >= 2 {
                                    let years = app.backtest_years;
                                    app.loading_label = format!("Backtesting portfolio of {} names ({}y) — this can take a while", tickers.len(), years);
                                    app.state = AppState::Loading;
                                    let tx = tx.clone();
                                    tokio::spawn(async move {
                                        run_portfolio_backtest_task(tickers, horizon, years, tx).await;
                                    });
                                }
                            }
                            KeyCode::Char('k') | KeyCode::Char('K') => {
                                let tickers: Vec<String> = app.results.iter().map(|r| r.ticker.clone()).collect();
                                let horizon = app.results.first().map(|r| r.horizon).unwrap_or(Horizon::Medium);
                                app.calib_current_score = app.results.get(app.compare_cursor).map(|r| r.composite_score);
                                if !tickers.is_empty() {
                                    let years = app.backtest_years;
                                    app.loading_label = format!("Calibrating {} name(s) ({}y) — this can take a while", tickers.len(), years);
                                    app.state = AppState::Loading;
                                    let tx = tx.clone();
                                    tokio::spawn(async move {
                                        run_calibration_task(tickers, horizon, years, true, tx).await;
                                    });
                                }
                            }
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                app.state = AppState::Input;
                                app.input_buffer.clear();
                                app.results.clear();
                            }
                            _ => {}
                        },
                        AppState::Error(_) => {
                            app.state = AppState::Input;
                        }
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.tick_count += 1;
            last_tick = Instant::now();
        }
    }

    log_info!("VORA-Vision terminal application exiting");
    Ok(())
}
