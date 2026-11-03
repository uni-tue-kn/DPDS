//! `DelayerConfig::initialization_rate_pps`: the packet rate (packets per
//! second) the correlated delay is initialised with. It is a required config
//! field, there is no built-in rate any more, and it is validated whether or
//! not the correlation is active: finite and at least 1, else a `BuildError`
//! whose message names the field.
//!
//! Derivations (from `delayer.rs`):
//! - `w = 1 / 2^(1/k)`, `k = half_life_ns * rate_pps / 1e9`; `try_new` fails
//!   with the half-life error iff `w >= 1` in f64, which happens once
//!   `ln 2 / k` is below half an ulp of 1 (2^-53), i.e. `k` above about 6.2e15.
//!   With `half_life_ns = 2e16`: `k = 2e7` at 1 pps, `3e14` at 15 Mpps, `2e16`
//!   at 1e9 pps. So that half-life is valid at 1 pps and 15 Mpps and invalid at
//!   1e9 pps: the configured rate, and no constant, decides.
//! - First 1 ms: the rate estimate is refreshed only after more than 1 ms, so
//!   10_000 packets at 15 Mpps spacing (0.667 ms) see only the initialisation
//!   weight. The successive difference `d_i = x_i - x_{i-1}` of the delays has
//!   `std(d) = sigma sqrt(2 (1 - w))` (stationary EMA, see
//!   `delayer_correlation_start.rs`). With `half_life = 20 / 15e-3 = 1333.3 ns`:
//!   w = 0.5946 / 0.9659 / 0.9948 and std(d) = 0.90 / 0.26 / 0.10 sigma at
//!   1e6 / 15e6 / 1e8 pps. The standard error of the RMS over 10_000 samples is
//!   below 1%, the tolerance is 8%, the gap between the candidates at least a
//!   factor 2.5.

use std::ffi::c_void;
use std::panic::catch_unwind;

use dpds_core::buffer::{Buffer, PushOutcome};
use dpds_core::delayer::{Delayer, RVDistribution};
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{BuildError, Config, DelayerConfig, LimiterConfig};

use RVDistribution::*;

const MEAN: f64 = 1e6;
const STD: f64 = 1e5;
const HALF_LIFE_NS: f64 = 20.0 / 15e-3;
const SPACING_NS: f64 = 1e9 / 15e6;

fn config(corr: bool, half_life_ns: f64, rate_pps: f64, capacity: usize, seed: u64) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering: false,
        delayer: DelayerConfig {
            distribution: NormalDistribution,
            mean_ns: MEAN,
            std_ns: STD,
            activate_correlation: corr,
            half_life_period_ns: half_life_ns,
            initialization_rate_pps: rate_pps,
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

fn names_the_rate(err: &BuildError) -> bool {
    err.to_string().contains("initialization_rate_pps")
}

const BAD_RATES: [(&str, f64); 8] = [
    ("zero", 0.0),
    ("0.5", 0.5),
    ("just below 1", 0.999_999_999),
    ("negative", -15e6),
    ("tiny negative", -f64::MIN_POSITIVE),
    ("NaN", f64::NAN),
    ("+inf", f64::INFINITY),
    ("-inf", f64::NEG_INFINITY),
];

#[test]
fn buffer_try_new_rejects_a_bad_rate_naming_the_field() {
    for corr in [false, true] {
        for (name, rate) in BAD_RATES {
            let err = Buffer::try_new(config(corr, HALF_LIFE_NS, rate, 8, 1))
                .err()
                .unwrap_or_else(|| panic!("{name} rate (correlation {corr}): must fail"));
            assert!(
                names_the_rate(&err),
                "{name} rate (correlation {corr}): message {:?} must name initialization_rate_pps",
                err.to_string()
            );
        }
    }
}

#[test]
fn buffer_try_new_rejects_a_bad_rate_without_seed_too() {
    let mut cfg = config(false, HALF_LIFE_NS, 0.0, 8, 1);
    cfg.seed = None;
    let err = Buffer::try_new(cfg).err().expect("rate 0 must fail");
    assert!(names_the_rate(&err), "{err}");
}

#[test]
fn a_bad_rate_is_rejected_for_every_distribution() {
    for dist in [
        DeterministicDistribution,
        NormalDistribution,
        LogNormalDistribution,
        GammaDistribution,
        UniformDistribution,
    ] {
        let mut cfg = config(false, HALF_LIFE_NS, f64::NAN, 8, 1);
        cfg.delayer.distribution = dist;
        let err = Buffer::try_new(cfg)
            .err()
            .unwrap_or_else(|| panic!("{dist:?}: NaN rate must fail"));
        assert!(names_the_rate(&err), "{dist:?}: {err}");
    }
}

#[test]
fn delayer_try_new_rejects_a_bad_rate_naming_the_field() {
    for corr in [false, true] {
        for (name, rate) in BAD_RATES {
            let err = Delayer::try_new(NormalDistribution, MEAN, STD, corr, HALF_LIFE_NS, rate)
                .err()
                .unwrap_or_else(|| panic!("{name} rate (correlation {corr}): must fail"));
            assert!(names_the_rate(&err), "{name} (correlation {corr}): {err}");
            let err =
                Delayer::try_new_seeded(NormalDistribution, MEAN, STD, corr, HALF_LIFE_NS, rate, 3)
                    .err()
                    .unwrap_or_else(|| {
                        panic!("{name} rate (correlation {corr}), seeded: must fail")
                    });
            assert!(
                names_the_rate(&err),
                "{name} (correlation {corr}), seeded: {err}"
            );
        }
    }
}

#[test]
fn new_panics_on_a_bad_rate_at_construction_naming_the_field() {
    for (name, rate) in BAD_RATES {
        let payload = catch_unwind(|| {
            let _ = Buffer::new(config(true, HALF_LIFE_NS, rate, 8, 1));
        })
        .err()
        .unwrap_or_else(|| panic!("Buffer::new({name} rate) must panic"));
        let msg = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(
            msg.contains("initialization_rate_pps"),
            "Buffer::new({name}): panic message {msg:?} must name the field"
        );
        assert!(
            catch_unwind(|| {
                let _ = Delayer::new(NormalDistribution, MEAN, STD, false, HALF_LIFE_NS, rate);
            })
            .is_err(),
            "Delayer::new({name} rate) must panic"
        );
    }
}

#[test]
fn rates_of_at_least_one_are_valid_with_and_without_correlation() {
    // Boundary: exactly 1 pps is valid (finite, >= 1); the very large finite
    // rate 1e12 pps is a valid rate too (1e12 pps is not a realistic rate, but
    // the rule is "finite and >= 1").
    for corr in [false, true] {
        for rate in [1.0, 1.5, 1000.0, 15e6, 1e9, 1e12] {
            let cfg = config(corr, HALF_LIFE_NS, rate, 8, 1);
            Buffer::try_new(cfg)
                .unwrap_or_else(|e| panic!("rate {rate} (correlation {corr}) must be valid: {e}"));
            Delayer::try_new(NormalDistribution, MEAN, STD, corr, HALF_LIFE_NS, rate)
                .unwrap_or_else(|e| panic!("Delayer, rate {rate} (correlation {corr}): {e}"));
        }
    }
}

#[test]
fn the_rate_is_validated_also_with_correlation_off_and_a_zero_half_life() {
    // With correlation off the rate is never used for a weight, still it must
    // be valid: one rule for everybody.
    for corr in [false, true] {
        assert!(
            Buffer::try_new(config(corr, 0.0, 0.0, 8, 1)).is_err(),
            "{corr}"
        );
        assert!(
            Buffer::try_new(config(corr, 0.0, 1.0, 8, 1)).is_ok(),
            "{corr}"
        );
    }
}

// ---- no built-in 15 Mpps: the configured rate decides ----------------------------

#[test]
fn the_configured_rate_decides_whether_a_long_half_life_is_valid() {
    let long_half_life = 2e16;
    // k = 2e7 (1 pps), 3e14 (15 Mpps): weight < 1, valid.
    for rate in [1.0, 15e6] {
        Buffer::try_new(config(true, long_half_life, rate, 8, 1))
            .unwrap_or_else(|e| panic!("rate {rate}: {e}"));
    }
    // k = 2e16 (1e9 pps): the weight rounds to 1, a half-life error. A buffer
    // that used a built-in 15 Mpps instead of the configured rate would accept it.
    let err = Buffer::try_new(config(true, long_half_life, 1e9, 8, 1))
        .err()
        .expect("1e9 pps with a 2e16 ns half-life must fail");
    assert!(
        matches!(err, BuildError::HalfLifePeriod { .. }),
        "expected the half-life error, got {err:?}"
    );
    assert!(err.to_string().contains("half_life_period_ns"), "{err}");
    // Without correlation the weight is never computed, so the same numbers are fine.
    Buffer::try_new(config(false, long_half_life, 1e9, 8, 1)).expect("correlation off");
    // The Delayer agrees (it takes the rate as a parameter).
    assert!(Delayer::try_new(NormalDistribution, MEAN, STD, true, long_half_life, 1e9).is_err());
    assert!(Delayer::try_new(NormalDistribution, MEAN, STD, true, long_half_life, 15e6).is_ok());
}

/// Delays (ns) of `n` packets pushed at 15 Mpps spacing into a buffer built
/// from `config`, read as `send_time - now` (floor of the sampled delay, at
/// most 1 ns low).
fn delays(config: Config, n: usize) -> Vec<f64> {
    let mut buf = Buffer::try_new(config).expect("valid config");
    (0..n)
        .map(|i| {
            let now = i as f64 * SPACING_NS;
            match buf.push_outcome(std::ptr::dangling_mut::<c_void>(), 1500, now) {
                PushOutcome::Accepted { send_time_ns } => send_time_ns as f64 - now,
                other => panic!("packet {i}: {other:?}"),
            }
        })
        .collect()
}

fn diff_rms(x: &[f64]) -> f64 {
    let sq: f64 = x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
    (sq / (x.len() - 1) as f64).sqrt()
}

fn expected_diff_std(rate_pps: f64) -> f64 {
    let k = HALF_LIFE_NS * rate_pps / 1e9;
    let w = 2f64.powf(-1.0 / k);
    STD * (2.0 * (1.0 - w)).sqrt()
}

#[test]
fn two_configs_differing_only_in_the_rate_start_with_different_delay_spread() {
    let n = 10_000;
    for seed in [61, 62] {
        let mut measured = Vec::new();
        for rate in [1e6, 15e6, 1e8] {
            let x = delays(config(true, HALF_LIFE_NS, rate, n, seed), n);
            let got = diff_rms(&x);
            let expected = expected_diff_std(rate);
            assert!(
                (got - expected).abs() < 0.08 * expected,
                "seed {seed}, init rate {rate}: std of successive differences {got}, expected {expected} +- 8%"
            );
            measured.push(got);
        }
        // Strictly decreasing with the rate and well separated: the rate in
        // the config is the one in effect (a fixed 15 Mpps would give the same
        // value three times).
        assert!(
            measured[0] > 2.0 * measured[1] && measured[1] > 2.0 * measured[2],
            "seed {seed}: {measured:?}"
        );
    }
}

#[test]
fn the_rate_is_unused_without_correlation() {
    // Same seed and same samples whatever the (valid) rate: the uncorrelated
    // delay does not depend on it.
    let n = 200;
    let a = delays(config(false, HALF_LIFE_NS, 1.0, n, 9), n);
    let b = delays(config(false, HALF_LIFE_NS, 1e9, n, 9), n);
    assert_eq!(a, b);
}
