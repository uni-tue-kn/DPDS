//! `Buffer::try_new(config) -> Result<Buffer, BuildError>` and
//! `Delayer::try_new(..) -> Result<Delayer, BuildError>`; `BuildError` is a
//! `std::error::Error` with a non-empty Display naming the bad parameter;
//! `new` keeps panicking for the same inputs.
//!
//! The error cases are every panic site of `Delayer::new`/`Buffer::new` today,
//! derived from the rand_distr 0.6 / rand 0.10 sources:
//! - Normal: `std` not finite (inf, NaN);
//! - LogNormal (`from_mean_cv(mean, std / mean)`): cv != 0 and mean not > 0, cv
//!   negative or NaN (so `mean = 0, std = 0` is NaN -> error);
//! - Gamma (`alpha = (mean/std)^2`, `beta = std^2/mean`): alpha or beta not > 0
//!   (`std = 0` -> beta 0; `mean = 0` -> alpha 0; `mean < 0` -> beta < 0);
//! - Uniform (`mean -+ sqrt(3) std`): low >= high (`std <= 0`) or NaN;
//! - `Buffer`: `with_capacity(usize::MAX)` panics with "capacity overflow".
//!
//! Not asserted (deliberately left open): negative `std` for Normal (rand_distr
//! accepts it), whether the `Random(p)` probability is range-checked (it does not
//! panic today), and the exact wording/type path of `BuildError`.

use std::ffi::c_void;
use std::panic::catch_unwind;

use dpds_core::buffer::Buffer;
use dpds_core::delayer::{Delayer, RVDistribution};
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{BuildError, Config, DelayerConfig, LimiterConfig};

use RVDistribution::*;

fn config(dist: RVDistribution, mean: f64, std: f64, corr: bool, capacity: usize) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering: false,
        delayer: DelayerConfig {
            distribution: dist,
            mean_ns: mean,
            std_ns: std,
            activate_correlation: corr,
            half_life_period_ns: 1000.0,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1e9,
            limit_bit: 1e9,
        },
        loss_model: LossModel::Random(0.0),
        seed: None,
    }
}

/// (name, distribution, mean, std, error message must mention one of these).
fn invalid() -> Vec<(&'static str, RVDistribution, f64, f64)> {
    vec![
        (
            "normal std = +inf",
            NormalDistribution,
            1000.0,
            f64::INFINITY,
        ),
        ("normal std = NaN", NormalDistribution, 1000.0, f64::NAN),
        ("lognormal mean = 0", LogNormalDistribution, 0.0, 5.0),
        ("lognormal mean < 0", LogNormalDistribution, -100.0, 10.0),
        ("lognormal std < 0", LogNormalDistribution, 100.0, -10.0),
        ("lognormal mean = std = 0", LogNormalDistribution, 0.0, 0.0),
        (
            "lognormal mean = NaN",
            LogNormalDistribution,
            f64::NAN,
            10.0,
        ),
        ("gamma std = 0", GammaDistribution, 100.0, 0.0),
        ("gamma mean = 0", GammaDistribution, 0.0, 10.0),
        ("gamma mean < 0", GammaDistribution, -100.0, 10.0),
        ("gamma mean = std = 0", GammaDistribution, 0.0, 0.0),
        ("uniform std = 0", UniformDistribution, 100.0, 0.0),
        ("uniform std < 0", UniformDistribution, 100.0, -10.0),
        ("uniform std = NaN", UniformDistribution, 100.0, f64::NAN),
    ]
}

fn names_a_distribution_parameter(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("mean") || m.contains("std")
}

fn assert_std_error<E: std::error::Error + 'static>(e: &E) {
    assert!(!e.to_string().is_empty(), "Display must not be empty");
    assert!(!format!("{e:?}").is_empty(), "Debug must not be empty");
}

#[test]
fn buffer_try_new_reports_every_invalid_distribution_parameter_set() {
    for (name, dist, mean, std) in invalid() {
        let err = Buffer::try_new(config(dist, mean, std, false, 8))
            .err()
            .unwrap_or_else(|| panic!("{name}: try_new must fail"));
        assert_std_error(&err);
        assert!(
            names_a_distribution_parameter(&err.to_string()),
            "{name}: message {:?} should name the offending parameter (mean/std)",
            err.to_string()
        );
    }
}

#[test]
fn delayer_try_new_reports_every_invalid_distribution_parameter_set() {
    for (name, dist, mean, std) in invalid() {
        let err = Delayer::try_new(dist, mean, std, false, 1000.0, 15e6)
            .err()
            .unwrap_or_else(|| panic!("{name}: try_new must fail"));
        assert_std_error(&err);
        assert!(
            names_a_distribution_parameter(&err.to_string()),
            "{name}: {err}"
        );
    }
}

#[test]
fn invalid_parameters_are_also_errors_with_correlation_active() {
    for (name, dist, mean, std) in invalid() {
        // With correlation the constructor first uses |std|, so a merely
        // negative std is valid there (see delayer.rs); skip those.
        if std < 0.0 {
            continue;
        }
        assert!(
            Buffer::try_new(config(dist, mean, std, true, 8)).is_err(),
            "{name} (correlation)"
        );
        assert!(
            Delayer::try_new(dist, mean, std, true, 1000.0, 15e6).is_err(),
            "{name} (correlation)"
        );
    }
}

#[test]
fn buffer_try_new_rejects_a_capacity_that_overflows_the_allocation() {
    // `VecDeque::with_capacity(usize::MAX)` panics ("capacity overflow") today,
    // and so does BinaryHeap: both modes must return an error instead.
    for reordering in [false, true] {
        let mut cfg = config(DeterministicDistribution, 0.0, 0.0, false, usize::MAX);
        cfg.reordering = reordering;
        let err = Buffer::try_new(cfg)
            .err()
            .unwrap_or_else(|| panic!("reordering={reordering}: must fail"));
        assert_std_error(&err);
        assert!(
            err.to_string().to_lowercase().contains("capacity"),
            "message {:?} should name buffer_capacity",
            err.to_string()
        );
    }
}

#[test]
fn try_new_accepts_valid_and_boundary_configurations() {
    for (name, dist, mean, std) in [
        ("deterministic", DeterministicDistribution, 100.0, 0.0),
        (
            "deterministic ignores NaN std",
            DeterministicDistribution,
            100.0,
            f64::NAN,
        ),
        ("normal", NormalDistribution, 100.0, 10.0),
        ("normal std 0", NormalDistribution, 100.0, 0.0),
        ("lognormal", LogNormalDistribution, 100.0, 10.0),
        ("lognormal cv 0", LogNormalDistribution, 100.0, 0.0),
        ("gamma", GammaDistribution, 100.0, 10.0),
        ("uniform", UniformDistribution, 100.0, 10.0),
    ] {
        assert!(
            Buffer::try_new(config(dist, mean, std, false, 8)).is_ok(),
            "{name}"
        );
        assert!(
            Delayer::try_new(dist, mean, std, false, 1000.0, 15e6).is_ok(),
            "{name}"
        );
    }
    // Capacity 0 and a large-but-allocatable capacity are fine.
    assert!(Buffer::try_new(config(DeterministicDistribution, 0.0, 0.0, false, 0)).is_ok());
    assert!(Buffer::try_new(config(
        DeterministicDistribution,
        0.0,
        0.0,
        false,
        1_000_000
    ))
    .is_ok());
}

#[test]
fn a_buffer_from_try_new_behaves_like_one_from_new() {
    let cfg = config(DeterministicDistribution, 1000.0, 0.0, false, 4);
    let mut a = Buffer::try_new(cfg).expect("valid");
    let mut b = Buffer::new(cfg);
    for i in 1..=6 {
        let p = i as *mut c_void;
        assert_eq!(a.push(p, 100, i as f64), b.push(p, 100, i as f64));
        assert_eq!(a.front_time(), b.front_time());
    }
    assert_eq!(a.capacity(), b.capacity());
    assert_eq!(a.len(), b.len());
    while let Some(p) = a.pop() {
        assert_eq!(Some(p), b.pop());
    }
    assert!(b.is_empty());
}

#[test]
fn a_delayer_from_try_new_samples_like_one_from_new() {
    let mut d = Delayer::try_new(DeterministicDistribution, 321.0, 0.0, false, 1000.0, 15e6)
        .expect("valid");
    assert_eq!(d.delayer_sample_ns(0.0), 321.0);
}

#[test]
fn new_still_panics_where_try_new_errors() {
    for (name, dist, mean, std) in invalid() {
        assert!(
            catch_unwind(|| {
                let _ = Buffer::new(config(dist, mean, std, false, 8));
            })
            .is_err(),
            "Buffer::new({name}) must panic"
        );
        assert!(
            catch_unwind(|| {
                let _ = Delayer::new(dist, mean, std, false, 1000.0, 15e6);
            })
            .is_err(),
            "Delayer::new({name}) must panic"
        );
    }
    assert!(
        catch_unwind(|| {
            let _ = Buffer::new(config(
                DeterministicDistribution,
                0.0,
                0.0,
                false,
                usize::MAX,
            ));
        })
        .is_err(),
        "Buffer::new(capacity usize::MAX) must panic"
    );
}

// ---- half_life_period_ns must be a finite, non-negative number --------
//
// A negative, NaN or infinite half-life used to pass `try_new` and panic in
// `delayer_sample_ns` once traffic flowed. It is now rejected up front, always
// (also with `activate_correlation: false`), and `0` stays valid.

const BAD_HALF_LIVES: [(&str, f64); 5] = [
    ("negative", -1000.0),
    ("tiny negative", -f64::MIN_POSITIVE),
    ("NaN", f64::NAN),
    ("+inf", f64::INFINITY),
    ("-inf", f64::NEG_INFINITY),
];

fn config_half_life(corr: bool, half_life: f64) -> Config {
    let mut cfg = config(NormalDistribution, 1000.0, 10.0, corr, 8);
    cfg.delayer.half_life_period_ns = half_life;
    cfg
}

#[test]
fn buffer_try_new_rejects_a_bad_half_life_naming_it() {
    for corr in [false, true] {
        for (name, h) in BAD_HALF_LIVES {
            let err = Buffer::try_new(config_half_life(corr, h))
                .err()
                .unwrap_or_else(|| panic!("{name} half_life (correlation {corr}): must fail"));
            assert_std_error(&err);
            assert!(
                err.to_string().contains("half_life_period_ns"),
                "{name} (correlation {corr}): message {:?} must name half_life_period_ns",
                err.to_string()
            );
        }
    }
}

#[test]
fn delayer_try_new_rejects_a_bad_half_life_naming_it() {
    for corr in [false, true] {
        for (name, h) in BAD_HALF_LIVES {
            let err = Delayer::try_new(NormalDistribution, 1000.0, 10.0, corr, h, 15e6)
                .err()
                .unwrap_or_else(|| panic!("{name} half_life (correlation {corr}): must fail"));
            assert_std_error(&err);
            assert!(
                err.to_string().contains("half_life_period_ns"),
                "{name} (correlation {corr}): {err}"
            );
        }
    }
}

#[test]
fn a_bad_half_life_is_rejected_for_every_distribution() {
    for dist in [
        DeterministicDistribution,
        NormalDistribution,
        LogNormalDistribution,
        GammaDistribution,
        UniformDistribution,
    ] {
        let mut cfg = config_half_life(false, -1.0);
        cfg.delayer.distribution = dist;
        assert!(Buffer::try_new(cfg).is_err(), "{dist:?}");
        assert!(
            Delayer::try_new(dist, 1000.0, 10.0, false, f64::NAN, 15e6).is_err(),
            "{dist:?}"
        );
    }
}

#[test]
fn a_zero_half_life_stays_valid() {
    for corr in [false, true] {
        assert!(
            Buffer::try_new(config_half_life(corr, 0.0)).is_ok(),
            "Buffer, correlation {corr}"
        );
        assert!(
            Delayer::try_new(NormalDistribution, 1000.0, 10.0, corr, 0.0, 15e6).is_ok(),
            "Delayer, correlation {corr}"
        );
    }
    // Ordinary positive values, including a very large finite one (1e15 ns:
    // k = 1.5e13, w = 2^(-1/k) = 1 - 4.6e-14 < 1, so the innovation std is
    // finite). With correlation off any finite half-life is valid, f64::MAX
    // included, because the weight is never computed.
    for h in [1.0, 1000.0, 1e15] {
        assert!(Buffer::try_new(config_half_life(false, h)).is_ok(), "{h}");
        assert!(Buffer::try_new(config_half_life(true, h)).is_ok(), "{h}");
    }
    assert!(Buffer::try_new(config_half_life(false, f64::MAX)).is_ok());
    assert!(Delayer::try_new(NormalDistribution, 1000.0, 10.0, false, f64::MAX, 15e6).is_ok());
}

// With correlation on, a half-life so large that the weight from the
// initialisation rate rounds to 1.0 (2^(-1/k) == 1.0 in f64, i.e. k = half_life
// * rate_ppns above roughly 6e15: f64::MAX gives k = inf) makes the innovation
// std (1 + w)/(1 - w) infinite. That is a half-life configuration error, not a
// distribution error.
#[test]
fn a_half_life_whose_weight_rounds_to_one_is_a_half_life_error_with_correlation() {
    let named = |err: &BuildError, what: &str| {
        assert!(
            matches!(err, BuildError::HalfLifePeriod { .. }),
            "{what}: expected BuildError::HalfLifePeriod, got {err:?}"
        );
        assert!(
            err.to_string().contains("half_life_period_ns"),
            "{what}: message {:?} must name half_life_period_ns",
            err.to_string()
        );
    };
    for dist in [
        DeterministicDistribution,
        NormalDistribution,
        LogNormalDistribution,
        GammaDistribution,
        UniformDistribution,
    ] {
        let err = Delayer::try_new(dist, 1000.0, 10.0, true, f64::MAX, 15e6)
            .err()
            .unwrap_or_else(|| panic!("Delayer {dist:?}, f64::MAX half-life must fail"));
        named(&err, &format!("Delayer {dist:?}"));
    }
    let mut cfg = config_half_life(true, f64::MAX);
    for seed in [None, Some(1)] {
        cfg.seed = seed;
        let err = Buffer::try_new(cfg)
            .err()
            .expect("Buffer with f64::MAX half-life and correlation must fail");
        named(&err, &format!("Buffer, seed {seed:?}"));
    }
    // Same through the seeded constructor.
    let err = Delayer::try_new_seeded(NormalDistribution, 1000.0, 10.0, true, f64::MAX, 15e6, 7)
        .err()
        .expect("seeded Delayer must fail");
    named(&err, "Delayer::try_new_seeded");
    // `new` panics instead of returning an error.
    assert!(catch_unwind(|| {
        let _ = Delayer::new(NormalDistribution, 1000.0, 10.0, true, f64::MAX, 15e6);
    })
    .is_err());
}

#[test]
fn new_panics_on_a_bad_half_life_at_construction_not_later() {
    for (name, h) in BAD_HALF_LIVES {
        assert!(
            catch_unwind(|| {
                let _ = Buffer::new(config_half_life(false, h));
            })
            .is_err(),
            "Buffer::new({name} half_life) must panic"
        );
        assert!(
            catch_unwind(|| {
                let _ = Delayer::new(NormalDistribution, 1000.0, 10.0, false, h, 15e6);
            })
            .is_err(),
            "Delayer::new({name} half_life) must panic"
        );
    }
}

#[test]
fn try_new_seeded_rejects_a_bad_half_life_and_accepts_zero() {
    for corr in [false, true] {
        for (name, h) in BAD_HALF_LIVES {
            let err = Delayer::try_new_seeded(NormalDistribution, 1000.0, 10.0, corr, h, 15e6, 7)
                .err()
                .unwrap_or_else(|| panic!("{name} (correlation {corr}): must fail"));
            assert!(err.to_string().contains("half_life_period_ns"), "{err}");
        }
        assert!(
            Delayer::try_new_seeded(NormalDistribution, 1000.0, 10.0, corr, 0.0, 15e6, 7).is_ok()
        );
    }
}
