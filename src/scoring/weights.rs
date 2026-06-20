use crate::models::Horizon;

pub struct Weights {
    pub valuation: f64,
    pub fundamentals: f64,
    pub macro_env: f64,
    pub sentiment: f64,
    pub risk: f64,
}

pub fn get_weights(horizon: Horizon) -> Weights {
    let w = match horizon {
        Horizon::Short => Weights {
            valuation: 0.10,
            fundamentals: 0.10,
            macro_env: 0.25,
            sentiment: 0.20,
            risk: 0.35,
        },
        Horizon::Medium => Weights {
            valuation: 0.25,
            fundamentals: 0.25,
            macro_env: 0.20,
            sentiment: 0.15,
            risk: 0.15,
        },
        Horizon::Long => Weights {
            valuation: 0.20,
            fundamentals: 0.45,
            macro_env: 0.15,
            sentiment: 0.10,
            risk: 0.10,
        },
    };

    // Sanity check: all weights must sum to exactly 1.0 (within floating-point tolerance).
    // This fires in `cargo test` and `cargo run` debug builds, but NOT in `cargo build --release`.
    let total = w.valuation + w.fundamentals + w.macro_env + w.sentiment + w.risk;
    assert!(
        (total - 1.0).abs() < 1e-9,
        "Weights for {:?} horizon sum to {:.10} instead of 1.0. Fix the values in weights.rs.",
        horizon,
        total
    );


    w
}
