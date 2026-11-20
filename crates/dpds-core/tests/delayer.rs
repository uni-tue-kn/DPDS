//! `Delayer` characterisation tests that need no randomness: exact values for
//! the deterministic cases, the negative-sample clamp, and "invalid parameters
//! panic in `Delayer::new`" (the behaviour `try_new` must keep for `new`).
//!
//! Statistical behaviour (mean/std per distribution, correlation) lives in
//! `distributions.rs`, where it can be seeded through `Config::seed`.
//!
//! Expected panics were derived from rand_distr 0.6 / rand 0.10:
//! - `Normal::new` fails iff std is not finite;
//! - `LogNormal::from_mean_cv(mean, std/mean)` fails iff (cv != 0 and mean is
//!   not > 0) or cv is NaN/negative or sigma is not finite;
//! - `Gamma::new(alpha = (mean/std)^2, beta = std^2/mean)` fails iff alpha or
//!   beta is not > 0 (std = 0 gives beta = 0);
//! - `Uniform::new(mean - sqrt(3) std, mean + sqrt(3) std)` fails iff low >= high.

use std::panic::catch_unwind;

use dpds_core::delayer::{Delayer, RVDistribution};

const HALF_LIFE: f64 = 50_000_000.0;
const RATE_PPS: f64 = 15_000_000.0;

fn delayer(dist: RVDistribution, mean: f64, std: f64, corr: bool) -> Delayer {
    Delayer::new(dist, mean, std, corr, HALF_LIFE, RATE_PPS)
}

#[test]
fn deterministic_returns_exactly_the_mean_whatever_std_and_time() {
    let mut d = delayer(
        RVDistribution::DeterministicDistribution,
        1234.5,
        999.0,
        false,
    );
    for t in [0.0, 1.0, 1e6, 1e12] {
        assert_eq!(d.delayer_sample_ns(t), 1234.5);
    }
}

#[test]
fn deterministic_zero_mean_is_zero_delay() {
    let mut d = delayer(RVDistribution::DeterministicDistribution, 0.0, 0.0, false);
    assert_eq!(d.delayer_sample_ns(5.0), 0.0);
}

#[test]
fn negative_samples_are_clamped_to_zero() {
    let mut d = delayer(RVDistribution::DeterministicDistribution, -5.0, 0.0, false);
    assert_eq!(d.delayer_sample_ns(0.0), 0.0);

    // Normal(10, 1000): P(sample < 0) is about 0.5, so in 1000 samples the
    // clamp must have produced exact zeros (and never a negative value).
    let mut n = delayer(RVDistribution::NormalDistribution, 10.0, 1000.0, false);
    let samples: Vec<f64> = (0..1000).map(|i| n.delayer_sample_ns(i as f64)).collect();
    assert!(samples.iter().all(|&s| s >= 0.0));
    assert!(samples.contains(&0.0), "heavy left tail must hit the clamp");
    assert!(samples.iter().any(|&s| s > 0.0));
}

#[test]
fn normal_with_zero_std_is_exactly_the_mean() {
    // mean + 0 * z == mean for any finite z.
    let mut d = delayer(RVDistribution::NormalDistribution, 1234.5, 0.0, false);
    for i in 0..100 {
        assert_eq!(d.delayer_sample_ns(i as f64), 1234.5);
    }
}

#[test]
fn lognormal_with_zero_cv_is_the_mean_up_to_rounding() {
    // cv == 0 -> mu = ln(mean), sigma = 0 -> exp(ln(mean)).
    let mut d = delayer(RVDistribution::LogNormalDistribution, 5000.0, 0.0, false);
    for i in 0..100 {
        let s = d.delayer_sample_ns(i as f64);
        assert!((s - 5000.0).abs() < 5000.0 * 1e-12, "{s}");
    }
}

#[test]
fn correlation_with_a_deterministic_distribution_stays_at_the_mean() {
    // x_i = w * x_{i-1} + (1 - w) * mean with x_0 = mean is the mean for any w.
    let mut d = delayer(RVDistribution::DeterministicDistribution, 777.0, 0.0, true);
    for i in 0..1000 {
        let s = d.delayer_sample_ns(i as f64 * 1000.0);
        assert!((s - 777.0).abs() < 1e-6, "{s}");
    }
    // Also across the 1 ms rate-update boundary (> 1_000_000 ns of traffic).
    for i in 0..30_000 {
        let s = d.delayer_sample_ns(1e6 + i as f64 * 1000.0);
        assert!((s - 777.0).abs() < 1e-6, "{s}");
    }
}

/// Parameter sets rand_distr rejects, i.e. where `Delayer::new` must panic.
fn invalid_cases() -> Vec<(&'static str, RVDistribution, f64, f64)> {
    use RVDistribution::*;
    vec![
        (
            "normal std = +inf",
            NormalDistribution,
            1000.0,
            f64::INFINITY,
        ),
        ("normal std = NaN", NormalDistribution, 1000.0, f64::NAN),
        (
            "lognormal mean = 0, std > 0",
            LogNormalDistribution,
            0.0,
            5.0,
        ),
        ("lognormal mean < 0", LogNormalDistribution, -100.0, 10.0),
        ("lognormal std < 0", LogNormalDistribution, 100.0, -10.0),
        (
            "lognormal mean = 0 and std = 0 (cv NaN)",
            LogNormalDistribution,
            0.0,
            0.0,
        ),
        (
            "lognormal mean = NaN",
            LogNormalDistribution,
            f64::NAN,
            10.0,
        ),
        ("gamma std = 0 (scale 0)", GammaDistribution, 100.0, 0.0),
        ("gamma mean = 0 (shape 0)", GammaDistribution, 0.0, 10.0),
        (
            "gamma mean < 0 (scale < 0)",
            GammaDistribution,
            -100.0,
            10.0,
        ),
        ("gamma mean = std = 0 (NaN)", GammaDistribution, 0.0, 0.0),
        (
            "uniform std = 0 (empty range)",
            UniformDistribution,
            100.0,
            0.0,
        ),
        (
            "uniform std < 0 (low > high)",
            UniformDistribution,
            100.0,
            -10.0,
        ),
        ("uniform std = NaN", UniformDistribution, 100.0, f64::NAN),
    ]
}

#[test]
fn new_panics_for_every_invalid_distribution_parameter_set() {
    for (name, dist, mean, std) in invalid_cases() {
        for corr in [false, true] {
            // With correlation the constructor first computes
            // sqrt((1 + 0) / (1 - 0) * std^2) = |std|, so a merely negative std
            // becomes valid there; only the sign-independent cases still panic.
            if corr && std < 0.0 {
                continue;
            }
            let result = catch_unwind(|| {
                let _ = Delayer::new(dist, mean, std, corr, HALF_LIFE, RATE_PPS);
            });
            assert!(result.is_err(), "{name} (correlation {corr}) must panic");
        }
    }
}

#[test]
fn new_accepts_the_boundary_cases_next_to_the_invalid_ones() {
    use RVDistribution::*;
    for (name, dist, mean, std) in [
        (
            "deterministic ignores any std",
            DeterministicDistribution,
            100.0,
            f64::NAN,
        ),
        ("normal std = 0", NormalDistribution, 100.0, 0.0),
        ("lognormal cv = 0", LogNormalDistribution, 100.0, 0.0),
        ("gamma typical", GammaDistribution, 100.0, 10.0),
        ("uniform typical", UniformDistribution, 100.0, 10.0),
        ("uniform tiny std", UniformDistribution, 100.0, 1e-3),
    ] {
        let result = catch_unwind(|| {
            let mut d = Delayer::new(dist, mean, std, false, HALF_LIFE, RATE_PPS);
            d.delayer_sample_ns(0.0)
        });
        assert!(result.is_ok(), "{name} must not panic");
    }
}
