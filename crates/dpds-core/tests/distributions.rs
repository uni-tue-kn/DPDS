//! Statistical tests of the delay distributions and the correlation (UTEMA),
//! driven through `Buffer` with `Config::seed = Some(..)` so every run is
//! reproducible. The delay of a packet pushed at an integer `now` is read back
//! as `front_time() - now` (floor of the sampled delay, so at most 1 ns low).
//!
//! Setup shared by the plain-distribution tests: mean 1e6 ns, std 1e5 ns,
//! n = 100_000 samples, all at `now = 0`.
//!
//! Tolerances (all >= 6 standard errors, so a correct implementation cannot
//! realistically fail for any seed):
//! - sample mean: s.e. = std / sqrt(n) = 316 ns, bound 2000 ns (6.3 s.e.; the
//!   floor bias is 0.5 ns);
//! - sample std: s.e. ~= std / sqrt(2 n) = 224 ns (kurtosis of these shapes is
//!   within 3.2), bound 2000 ns (8.9 s.e.).
//!
//! Correlation: for an EMA `x_i = w x_{i-1} + (1 - w) r_i` with `r ~ (mean,
//! sigma_r)`, `sigma_r^2 = (1 + w)/(1 - w) std^2`, the stationary variance is
//! `std^2` and the lag-1 autocorrelation is `w`, with `w = 2^(-1/k)`,
//! `k = half_life * rate_ppns` and `rate` the packet rate measured over the
//! last > 1 ms (`HalfLifePeriodCounter`).

use std::ffi::c_void;

use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

const MEAN: f64 = 1e6;
const STD: f64 = 1e5;
const N: usize = 100_000;

fn config(
    dist: RVDistribution,
    mean: f64,
    std: f64,
    corr: bool,
    half_life: f64,
    seed: u64,
) -> Config {
    Config {
        buffer_capacity: 4,
        reordering: false,
        delayer: DelayerConfig {
            distribution: dist,
            mean_ns: mean,
            std_ns: std,
            activate_correlation: corr,
            half_life_period_ns: half_life,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1e9,
            limit_bit: 1e9,
        },
        loss_model: LossModel::Random(0.0),
        seed: Some(seed),
    }
}

/// Delay of each of `n` packets pushed at `now = i * spacing_ns`.
fn delays(cfg: Config, n: usize, spacing_ns: f64) -> Vec<f64> {
    let mut b = Buffer::new(cfg);
    (0..n)
        .map(|i| {
            let now = i as f64 * spacing_ns;
            assert!(b.push(std::ptr::dangling_mut::<c_void>(), 100, now));
            let d = b.front_time().unwrap() - now;
            b.pop();
            d
        })
        .collect()
}

fn mean(x: &[f64]) -> f64 {
    x.iter().sum::<f64>() / x.len() as f64
}

fn std(x: &[f64]) -> f64 {
    let m = mean(x);
    (x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / x.len() as f64).sqrt()
}

fn lag1_autocorrelation(x: &[f64]) -> f64 {
    let m = mean(x);
    let var: f64 = x.iter().map(|v| (v - m) * (v - m)).sum();
    let cov: f64 = x.windows(2).map(|w| (w[0] - m) * (w[1] - m)).sum();
    cov / var
}

fn check_moments(dist: RVDistribution, seed: u64) -> Vec<f64> {
    let d = delays(config(dist, MEAN, STD, false, 1.0, seed), N, 0.0);
    assert!((mean(&d) - MEAN).abs() < 2000.0, "mean {}", mean(&d));
    assert!((std(&d) - STD).abs() < 2000.0, "std {}", std(&d));
    // No correlation configured: successive samples are independent
    // (lag-1 autocorrelation s.e. = 1/sqrt(n) = 0.0032, bound 0.02).
    assert!(lag1_autocorrelation(&d).abs() < 0.02);
    d
}

#[test]
fn deterministic_delay_is_exactly_the_mean_with_any_std() {
    let d = delays(
        config(
            RVDistribution::DeterministicDistribution,
            MEAN,
            STD,
            false,
            1.0,
            1,
        ),
        1000,
        0.0,
    );
    assert!(d.iter().all(|&v| v == MEAN));
}

#[test]
fn normal_delay_has_the_configured_mean_and_std() {
    let d = check_moments(RVDistribution::NormalDistribution, 11);
    // Symmetric: about half of the samples are below the mean (s.e. 0.0016).
    let below = d.iter().filter(|&&v| v < MEAN).count() as f64 / N as f64;
    assert!((below - 0.5).abs() < 0.01, "{below}");
}

#[test]
fn lognormal_delay_has_the_configured_mean_and_std_and_is_right_skewed() {
    let d = check_moments(RVDistribution::LogNormalDistribution, 12);
    assert!(d.iter().all(|&v| v > 0.0));
    // Median of a log-normal is below its mean: median = mean / sqrt(1 + cv^2)
    // = 1e6 / sqrt(1.01) = 995_037; the fraction below the mean is
    // Phi(sigma/2) with sigma = sqrt(ln(1.01)) = 0.0998 -> 0.5199.
    let below = d.iter().filter(|&&v| v < MEAN).count() as f64 / N as f64;
    assert!((below - 0.5199).abs() < 0.01, "{below}");
}

#[test]
fn gamma_delay_has_the_configured_mean_and_std() {
    // shape = (mean/std)^2 = 100, scale = std^2/mean = 1e4.
    let d = check_moments(RVDistribution::GammaDistribution, 13);
    assert!(d.iter().all(|&v| v > 0.0));
}

#[test]
fn uniform_delay_has_the_configured_mean_and_std_and_stays_within_its_bounds() {
    let d = check_moments(RVDistribution::UniformDistribution, 14);
    // [mean - sqrt(3) std, mean + sqrt(3) std) = [826_794.9, 1_173_205.1);
    // front_time floors, so the lower bound is 826_794.
    let sqrt3 = 3f64.sqrt();
    let (lo, hi) = (MEAN - sqrt3 * STD, MEAN + sqrt3 * STD);
    assert!(
        d.iter().all(|&v| v >= lo.floor() && v < hi),
        "out of [{lo}, {hi})"
    );
    // The range is actually used (not collapsed to one end).
    let min = d.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = d.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(min < lo + 0.01 * (hi - lo) && max > hi - 0.01 * (hi - lo));
    // Uniform: a quarter of the samples in each quarter of the range
    // (s.e. 0.0014 per quarter, bound 0.01).
    for q in 0..4 {
        let a = lo + (hi - lo) * q as f64 / 4.0;
        let b = lo + (hi - lo) * (q + 1) as f64 / 4.0;
        let frac = d.iter().filter(|&&v| v >= a && v < b).count() as f64 / N as f64;
        assert!((frac - 0.25).abs() < 0.01, "quarter {q}: {frac}");
    }
}

/// Runs a correlated Normal delayer with packets every `spacing_ns` and
/// returns the delays after the first rate update (at packet 1_001 for a
/// 1 us spacing: the first packet whose age exceeds the 1 ms update period).
fn correlated_delays(spacing_ns: f64, half_life_ns: f64, seed: u64) -> Vec<f64> {
    let packets_per_update = (1_000_000.0 / spacing_ns) as usize + 1;
    let warmup = packets_per_update + 1000;
    let n = warmup + 200_000;
    let all = delays(
        config(
            RVDistribution::NormalDistribution,
            MEAN,
            STD,
            true,
            half_life_ns,
            seed,
        ),
        n,
        spacing_ns,
    );
    all[warmup..].to_vec()
}

fn check_correlated(spacing_ns: f64, half_life_ns: f64, seed: u64) {
    // k = half_life * (1 / spacing); w = 2^(-1/k)
    let k = half_life_ns / spacing_ns;
    let w = 2f64.powf(-1.0 / k);
    let d = correlated_delays(spacing_ns, half_life_ns, seed);

    // n = 200_000 post-warmup samples. For an AR(1) process the s.e. are
    // sqrt((1 - w^2)/n) for the autocorrelation (<= 0.0013 here, bound 0.02),
    // about 0.5% for the std (bound 5%) and std * sqrt((1+w)/((1-w) n)) for
    // the mean (<= 0.011 std, bound 0.1 std).
    let rho = lag1_autocorrelation(&d);
    assert!(
        (rho - w).abs() < 0.02,
        "lag-1 autocorrelation {rho}, expected w = {w}"
    );
    assert!(
        (std(&d) - STD).abs() < 0.05 * STD,
        "stationary std {} should stay the configured {STD}",
        std(&d)
    );
    assert!((mean(&d) - MEAN).abs() < 0.1 * STD, "mean {}", mean(&d));
}

#[test]
fn correlated_delay_has_lag1_autocorrelation_w_and_keeps_the_configured_std() {
    // 1 packet/us, half-life 4 us: k = 4, w = 2^(-1/4) = 0.8409.
    check_correlated(1000.0, 4000.0, 21);
}

#[test]
fn correlation_weight_follows_the_measured_packet_rate() {
    // Twice the rate (0.5 us spacing) with the same half-life: k = 8, w = 0.9170.
    check_correlated(500.0, 4000.0, 22);
}

#[test]
fn correlation_off_gives_uncorrelated_samples_even_with_a_half_life_set() {
    let d = delays(
        config(
            RVDistribution::NormalDistribution,
            MEAN,
            STD,
            false,
            4000.0,
            23,
        ),
        50_000,
        1000.0,
    );
    assert!(lag1_autocorrelation(&d).abs() < 0.03); // s.e. 0.0045
}
