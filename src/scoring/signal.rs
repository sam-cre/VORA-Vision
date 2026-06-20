use crate::models::{Signal, Horizon};

/// Minimum overall data coverage (%) required to emit a directional BUY/SELL.
/// Below this the engine holds the call to HOLD — a tool that screams BUY on two
/// data points is worse than one that admits it doesn't know yet.
pub const MIN_COVERAGE_PCT: f64 = 35.0;

/// Compute signal with horizon-adjusted thresholds.
/// Short-Term: tighter bands because risk/momentum are weighted heavily (more volatile signals)
/// Medium-Term: standard bands
/// Long-Term: slightly wider buy band because fundamentals compound over time
/// (buy_threshold, sell_threshold) for a horizon. Exposed so the backtest can
/// build a conviction ramp between the two bands.
pub fn thresholds(horizon: Horizon) -> (f64, f64) {
    match horizon {
        Horizon::Short => (68.0, 38.0),  // Tighter: short-term signals need higher conviction
        Horizon::Medium => (65.0, 40.0), // Standard bands
        Horizon::Long => (62.0, 42.0),   // Wider buy band: fundamentals need time to play out
    }
}

pub fn get_signal(composite_score: f64, horizon: Horizon) -> Signal {
    let (buy_threshold, sell_threshold) = thresholds(horizon);

    if composite_score >= buy_threshold {
        Signal::Buy
    } else if composite_score <= sell_threshold {
        Signal::Sell
    } else {
        Signal::Hold
    }
}
