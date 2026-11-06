//! Start-up of the correlated delay: the innovation std uses the weight of
//! the 15 Mpps initialisation rate from the very first packet, the packet rate
//! is re-estimated every 1 ms, and the EMA starts at `x_0 = mean`, not at a
//! random value.
//!
//! Model: `x_i = w x_{i-1} + (1 - w) r_i`, `r_i ~ N(mean, sigma_r^2)`,
//! `sigma_r^2 = (1 + w)/(1 - w) sigma^2`, `w = 2^(-1/k)`,
//! `k = half_life * rate_ppns`.
//!
//! Observable: the successive difference `d_i = x_i - x_{i-1} = (1 - w)(r_i -
//! x_{i-1})`. `r_i` is independent of `x_{i-1}`, so
//! `Var(d) = (1 - w)^2 (sigma_r^2 + Var(x_{i-1}))`.
//! - Fixed order: `Var(x) = sigma^2` (stationary) and `Var(d) = 2 (1 - w)
//!   sigma^2`, i.e. `std(d) = sigma sqrt(2 (1 - w))`.
//! - Wrong start (innovation std computed with weight 0, so `sigma_r = sigma`):
//!   `Var(x) = sigma^2 (1 - w)/(1 + w)` and `Var(d) = 2 (1 - w)^2/(1 + w)
//!   sigma^2`; for the weights used here that is 5 to 7 times smaller than the
//!   fixed value, far outside every tolerance below.
//!
//! (The start `x_0 = mean` makes `Var(x_i) = sigma^2 (1 - w^(2i))`, a
//! transient of about `1/(1 - w^2)` = 15 packets at w = 0.966, which changes
//! the RMS over >= 800 packets by well under 1%.)
//!
//! `d` is Gaussian with lag-1 autocorrelation `-(1 - w)/2` (>= -0.2 here) and
//! smaller beyond, so the RMS over n samples has a standard error of about
//! `std(d)/sqrt(2 n)` (+-20% for the correlation): 0.8% for n = 10_000, 2.5%
//! for n = 850, 1.2% for n = 3600. The tolerances are 5% (first-packet test) and
//! 12% (re-estimation tests), at least 4 standard errors; the gap between the
//! candidate values compared in the re-estimation tests (0.26, 0.49, 0.90
//! times sigma) is far larger.

use dpds_core::delayer::{Delayer, RVDistribution};

const MEAN: f64 = 1e6;
const STD: f64 = 1e5;
/// 15 Mpps, the rate the delayer is initialised with.
const INIT_RATE_PPS: f64 = 15e6;
const INIT_SPACING_NS: f64 = 1e9 / INIT_RATE_PPS; // 66.67 ns
/// k = 20 at the initialisation rate: w = 2^(-1/20) = 0.966.
const HALF_LIFE_NS: f64 = 20.0 / (INIT_RATE_PPS / 1e9);

fn weight(rate_pps: f64) -> f64 {
    2f64.powf(-1.0 / (HALF_LIFE_NS * rate_pps / 1e9))
}

/// Expected `std(x_i - x_{i-1})` of a correctly started correlated delayer at
/// the given measured packet rate.
fn expected_diff_std(rate_pps: f64) -> f64 {
    STD * (2.0 * (1.0 - weight(rate_pps))).sqrt()
}

fn delayer(seed: u64) -> Delayer {
    Delayer::try_new_seeded(
        RVDistribution::NormalDistribution,
        MEAN,
        STD,
        true,
        HALF_LIFE_NS,
        INIT_RATE_PPS,
        seed,
    )
    .unwrap()
}

/// Delays of packets arriving at the given times.
fn delays_at(d: &mut Delayer, times: impl Iterator<Item = f64>) -> Vec<f64> {
    times.map(|t| d.delayer_sample_ns(t)).collect()
}

/// Root mean square of the successive differences `x_i - x_{i-1}` over
/// `x[from..to]`.
fn diff_rms(x: &[f64], from: usize, to: usize) -> f64 {
    let sq: f64 = (from..to).map(|i| (x[i] - x[i - 1]).powi(2)).sum();
    (sq / (to - from) as f64).sqrt()
}

fn assert_close(actual: f64, expected: f64, rel_tol: f64, what: &str) {
    assert!(
        (actual - expected).abs() < rel_tol * expected,
        "{what}: got {actual}, expected {expected} +- {}%",
        rel_tol * 100.0
    );
}

#[test]
fn m1_innovation_std_matches_the_configured_std_from_the_first_packet() {
    // 10_000 packets at exactly 15 Mpps span 0.667 ms: no rate update yet
    // (the period is 1 ms), so only the initialisation weight is in effect.
    for seed in [31, 32] {
        let mut d = delayer(seed);
        let x = delays_at(&mut d, (0..10_000).map(|i| i as f64 * INIT_SPACING_NS));
        let expected = expected_diff_std(INIT_RATE_PPS);
        assert_close(
            diff_rms(&x, 1, x.len()),
            expected,
            0.05,
            &format!("seed {seed}: std of successive differences, all packets"),
        );
        // Also in the very first packets: 400 differences (s.e. 3.5%, bound 15%).
        assert_close(
            diff_rms(&x, 1, 401),
            expected,
            0.15,
            &format!("seed {seed}: std of successive differences, first 400 packets"),
        );
    }
}

#[test]
fn start_value_is_the_mean_and_is_not_randomised() {
    // Reference: an uncorrelated Normal with the same seed and the *corrected*
    // innovation std draws exactly the r_i the correlated delayer uses
    // (Normal = mean + std * z with the same z stream), so
    // x_1 = w mean + (1 - w) r_1 and x_i = w x_{i-1} + (1 - w) r_i must hold.
    let w = weight(INIT_RATE_PPS);
    let sigma_r = STD * ((1.0 + w) / (1.0 - w)).sqrt();
    for seed in [41, 42, 43] {
        let mut corr = delayer(seed);
        let mut reference = Delayer::try_new_seeded(
            RVDistribution::NormalDistribution,
            MEAN,
            sigma_r,
            false,
            HALF_LIFE_NS,
            INIT_RATE_PPS,
            seed,
        )
        .unwrap();
        let mut x_prev = MEAN; // x_0 = mean
        for i in 0..5 {
            let t = i as f64 * INIT_SPACING_NS;
            let r = reference.delayer_sample_ns(t);
            let expected = w * x_prev + (1.0 - w) * r;
            let got = corr.delayer_sample_ns(t);
            assert!(
                (got - expected).abs() < 1e-3,
                "seed {seed}, packet {}: got {got}, expected {expected} (x_0 = mean)",
                i + 1
            );
            x_prev = got;
        }
    }
}

#[test]
fn m2_rate_is_re_estimated_after_1_ms_not_10_ms() {
    // 1 Mpps (1000 ns spacing) instead of the initialised 15 Mpps. The first
    // packet is at t = 0, so the first update happens at packet 1001 (t = 1.001
    // ms, the first one strictly more than 1 ms after the start), measuring
    // exactly 1 Mpps.
    let spacing = 1000.0;
    let mut d = delayer(51);
    let x = delays_at(&mut d, (0..2000).map(|i| i as f64 * spacing));

    // Before 1 ms the initialisation weight still applies (a shorter period
    // would mix in the 1 Mpps value). Skip the first 100 as transient.
    assert_close(
        diff_rms(&x, 100, 950),
        expected_diff_std(15e6),
        0.12,
        "std of differences before the first update",
    );
    // After the update (skip 100 packets of transient) and well before 10 ms.
    assert_close(
        diff_rms(&x, 1100, 2000),
        expected_diff_std(1e6),
        0.12,
        "std of differences after the 1 ms update (at 1.1 ms to 2 ms)",
    );
}

#[test]
fn m2_every_update_uses_the_1_ms_period_not_only_the_first() {
    // 1 Mpps for the first 3 ms, then 4 Mpps (250 ns spacing) until 5 ms.
    // Updates fall at t = 1.001 ms, 2.002 ms, 3.00225 ms (measures a mix that
    // is still 1.007 Mpps) and 4.0025 ms (measures 4 Mpps).
    let mut times: Vec<f64> = (0..3000).map(|i| i as f64 * 1000.0).collect();
    times.extend((0..8000).map(|j| 3e6 + j as f64 * 250.0));
    let mut d = delayer(52);
    let x = delays_at(&mut d, times.iter().copied());
    let idx = |t_ns: f64| times.partition_point(|&t| t < t_ns);

    // 3.1 ms to 4.0 ms: weight of ~1 Mpps.
    assert_close(
        diff_rms(&x, idx(3.1e6), idx(4.0e6)),
        expected_diff_std(1.00675e6),
        0.12,
        "std of differences at 3.1 ms to 4.0 ms",
    );
    // 4.1 ms to 5.0 ms: weight of 4 Mpps, reached by the fourth update.
    assert_close(
        diff_rms(&x, idx(4.1e6), idx(5.0e6)),
        expected_diff_std(4e6),
        0.12,
        "std of differences at 4.1 ms to 5.0 ms (needs the 4 Mpps estimate)",
    );
}
