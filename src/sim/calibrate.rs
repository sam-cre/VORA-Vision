//! Score calibration: how have past composite scores actually mapped to forward
//! returns? This converts the backtest's track record into *forward expectations*
//! — e.g. "names scoring 80–90 returned +14% on average over the next 6 months,
//! and were positive 71% of the time."
//!
//! The pooling/bucketing logic here is pure and unit-tested; the data that feeds
//! it (point-in-time scores + realized forward returns) is built in `runner.rs`.

use serde::{Deserialize, Serialize};

/// Summary of forward returns for one score band.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationBucket {
    pub lo: f64,
    pub hi: f64,
    pub n: usize,
    /// Mean forward return (%).
    pub mean: f64,
    /// Median forward return (%).
    pub median: f64,
    /// Sample standard deviation of forward returns (%).
    pub stdev: f64,
    /// Share of observations with a positive forward return (%).
    pub win_rate: f64,
    /// 25th percentile forward return (%) — lower edge of the likely range.
    pub p25: f64,
    /// 75th percentile forward return (%) — upper edge of the likely range.
    pub p75: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Calibration {
    /// Forward window the returns were measured over.
    pub horizon_months: i64,
    pub buckets: Vec<CalibrationBucket>,
    pub n_total: usize,
}

impl Calibration {
    /// The bucket a given composite score falls into (if any with data).
    pub fn bucket_for(&self, score: f64) -> Option<&CalibrationBucket> {
        self.buckets.iter().find(|b| score >= b.lo && score < b.hi && b.n > 0)
    }
}

/// Linear-interpolated percentile of an already-sorted slice. `q` in [0,1].
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return sorted[0];
    }
    let rank = q * (n as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        let frac = rank - lo as f64;
        sorted[lo] * (1.0 - frac) + sorted[hi] * frac
    }
}

fn summarize(lo: f64, hi: f64, rets: &[f64]) -> CalibrationBucket {
    let n = rets.len();
    if n == 0 {
        return CalibrationBucket {
            lo, hi, n: 0, mean: 0.0, median: 0.0, stdev: 0.0, win_rate: 0.0, p25: 0.0, p75: 0.0,
        };
    }
    let mean = rets.iter().sum::<f64>() / n as f64;
    let stdev = if n > 1 {
        (rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0)).sqrt()
    } else {
        0.0
    };
    let mut sorted = rets.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = percentile(&sorted, 0.5);
    let p25 = percentile(&sorted, 0.25);
    let p75 = percentile(&sorted, 0.75);
    let wins = rets.iter().filter(|r| **r > 0.0).count();
    let win_rate = wins as f64 / n as f64 * 100.0;
    CalibrationBucket { lo, hi, n, mean, median, stdev, win_rate, p25, p75 }
}

/// Score bands used for calibration (upper-exclusive; the top band includes 100).
const BANDS: &[(f64, f64)] = &[
    (0.0, 50.0),
    (50.0, 60.0),
    (60.0, 70.0),
    (70.0, 80.0),
    (80.0, 90.0),
    (90.0, 100.01),
];

/// Bucket `(score, forward_return_pct)` pairs into score bands and summarize each.
pub fn build_calibration(pairs: &[(f64, f64)], horizon_months: i64) -> Calibration {
    let buckets = BANDS
        .iter()
        .map(|(lo, hi)| {
            let rets: Vec<f64> = pairs
                .iter()
                .filter(|(s, _)| *s >= *lo && *s < *hi)
                .map(|(_, r)| *r)
                .collect();
            summarize(*lo, hi.min(100.0), &rets)
        })
        .collect();
    Calibration { horizon_months, buckets, n_total: pairs.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bucketing_and_stats() {
        let pairs = vec![
            (85.0, 20.0),
            (82.0, 10.0),
            (55.0, 2.0),
            (55.0, -2.0),
            (95.0, 30.0),
        ];
        let c = build_calibration(&pairs, 6);
        assert_eq!(c.n_total, 5);

        // [80,90): two obs, mean 15
        let b80 = c.bucket_for(83.0).unwrap();
        assert_eq!(b80.n, 2);
        assert!((b80.mean - 15.0).abs() < 1e-9);
        assert!((b80.win_rate - 100.0).abs() < 1e-9);

        // [50,60): two obs, mean 0, 50% win rate
        let b50 = c.bucket_for(55.0).unwrap();
        assert_eq!(b50.n, 2);
        assert!((b50.mean - 0.0).abs() < 1e-9);
        assert!((b50.win_rate - 50.0).abs() < 1e-9);

        // [90,100]: one obs
        let b90 = c.bucket_for(95.0).unwrap();
        assert_eq!(b90.n, 1);
        assert!((b90.mean - 30.0).abs() < 1e-9);
    }

    #[test]
    fn test_median_even_and_odd() {
        let odd = build_calibration(&[(75.0, 1.0), (75.0, 3.0), (75.0, 10.0)], 6);
        assert!((odd.bucket_for(75.0).unwrap().median - 3.0).abs() < 1e-9);
        let even = build_calibration(&[(75.0, 1.0), (75.0, 3.0)], 6);
        assert!((even.bucket_for(75.0).unwrap().median - 2.0).abs() < 1e-9);
    }

    #[test]
    fn test_empty_band_has_no_data() {
        let c = build_calibration(&[(85.0, 5.0)], 6);
        assert!(c.bucket_for(20.0).is_none()); // [0,50) empty
    }

    #[test]
    fn test_percentiles_interpolate() {
        let sorted = [0.0, 10.0, 20.0, 30.0, 40.0];
        assert!((percentile(&sorted, 0.0) - 0.0).abs() < 1e-9);
        assert!((percentile(&sorted, 1.0) - 40.0).abs() < 1e-9);
        assert!((percentile(&sorted, 0.5) - 20.0).abs() < 1e-9);
        // p25 over 5 points: rank = 0.25*4 = 1.0 → exactly 10
        assert!((percentile(&sorted, 0.25) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn test_bucket_carries_p25_p75() {
        // Ten obs in [70,80): 0..90 step 10. p25≈22.5, p75≈67.5.
        let pairs: Vec<(f64, f64)> = (0..10).map(|i| (75.0, i as f64 * 10.0)).collect();
        let c = build_calibration(&pairs, 6);
        let b = c.bucket_for(75.0).unwrap();
        assert!(b.p25 < b.median && b.median < b.p75);
        assert!((b.p25 - 22.5).abs() < 1e-6);
        assert!((b.p75 - 67.5).abs() < 1e-6);
    }
}
