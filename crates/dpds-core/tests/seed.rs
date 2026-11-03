//! `Config.seed: Option<u64>`. `Some(s)` seeds the delayer and dropper RNGs
//! deterministically (and differently from each other); `None` keeps the
//! entropy seeding. Same seed + same input sequence => identical push results
//! and send times.
//!
//! The runs below push 2000 packets with a Normal delay (mean 1e6, std 1e5 ns,
//! so ~1e5 distinct ns values) and 30% loss; two independent streams of that
//! length being equal by chance is impossible for practical purposes
//! (probability < 2^-1000), so "different seeds differ" and "unseeded runs
//! differ" cannot flake.

use std::ffi::c_void;

use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

fn config(seed: Option<u64>, reordering: bool, loss: LossModel, correlation: bool) -> Config {
    Config {
        buffer_capacity: 4,
        reordering,
        delayer: DelayerConfig {
            distribution: RVDistribution::NormalDistribution,
            mean_ns: 1e6,
            std_ns: 1e5,
            activate_correlation: correlation,
            half_life_period_ns: 4000.0,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1e9,
            limit_bit: 1e9,
        },
        loss_model: loss,
        seed,
    }
}

/// One entry per push: `Some(send_time)` if accepted, `None` if dropped.
fn run_with(mut b: Buffer, n: usize) -> Vec<Option<f64>> {
    (0..n)
        .map(|i| {
            if b.push(std::ptr::dangling_mut::<c_void>(), 100, i as f64 * 1000.0) {
                let t = b.front_time();
                b.pop();
                t
            } else {
                None
            }
        })
        .collect()
}

fn run(cfg: Config, n: usize) -> Vec<Option<f64>> {
    run_with(Buffer::new(cfg), n)
}

const N: usize = 2000;

#[test]
fn same_seed_gives_identical_results_for_every_loss_model_and_mode() {
    for reordering in [false, true] {
        for correlation in [false, true] {
            for loss in [
                LossModel::Random(0.3),
                LossModel::GilbertElliot(0.1, 0.3, 0.95, 0.4, true),
            ] {
                let a = run(config(Some(42), reordering, loss, correlation), N);
                let b = run(config(Some(42), reordering, loss, correlation), N);
                assert_eq!(
                    a, b,
                    "reordering={reordering} correlation={correlation} {loss:?}"
                );
                // Guard against a vacuous comparison.
                assert!(a.iter().any(|x| x.is_some()) && a.iter().any(|x| x.is_none()));
            }
        }
    }
}

#[test]
fn seed_zero_is_a_valid_deterministic_seed() {
    let a = run(config(Some(0), false, LossModel::Random(0.3), false), N);
    let b = run(config(Some(0), false, LossModel::Random(0.3), false), N);
    assert_eq!(a, b);
}

#[test]
fn try_new_with_the_same_seed_is_reproducible_too() {
    let cfg = config(Some(7), false, LossModel::Random(0.3), false);
    let run_try = |cfg: Config| run_with(Buffer::try_new(cfg).expect("valid config"), N);
    assert_eq!(run_try(cfg), run_try(cfg));
    assert_eq!(
        run_try(cfg),
        run(cfg, N),
        "try_new and new seed identically"
    );
}

#[test]
fn different_seeds_give_different_delays_and_different_losses() {
    let a = run(config(Some(1), false, LossModel::Random(0.3), false), N);
    let b = run(config(Some(2), false, LossModel::Random(0.3), false), N);
    assert_ne!(a, b);
    // Both the delay stream and the loss stream must differ, not just one.
    let loss_a: Vec<bool> = a.iter().map(|x| x.is_none()).collect();
    let loss_b: Vec<bool> = b.iter().map(|x| x.is_none()).collect();
    assert_ne!(loss_a, loss_b, "loss decisions must depend on the seed");
    let both: Vec<(f64, f64)> = a
        .iter()
        .zip(&b)
        .filter_map(|(x, y)| Some(((*x)?, (*y)?)))
        .collect();
    assert!(
        both.iter().any(|(x, y)| x != y),
        "delays must depend on the seed"
    );
}

#[test]
fn seeds_differing_in_one_bit_differ() {
    let a = run(config(Some(1000), false, LossModel::Random(0.3), false), N);
    let b = run(config(Some(1001), false, LossModel::Random(0.3), false), N);
    assert_ne!(a, b);
}

#[test]
fn without_a_seed_runs_use_fresh_entropy() {
    let a = run(config(None, false, LossModel::Random(0.3), false), N);
    let b = run(config(None, false, LossModel::Random(0.3), false), N);
    assert_ne!(a, b);
}

#[test]
fn a_seeded_buffer_is_not_influenced_by_other_buffers_existing_or_being_used() {
    let reference = run(config(Some(5), false, LossModel::Random(0.3), false), N);

    let mut other = Buffer::new(config(Some(6), false, LossModel::Random(0.3), false));
    let mut unseeded = Buffer::new(config(None, false, LossModel::Random(0.3), false));
    let mut mine = Buffer::new(config(Some(5), false, LossModel::Random(0.3), false));
    let mut got = Vec::new();
    for i in 0..N {
        let now = i as f64 * 1000.0;
        // Interleave pushes on unrelated buffers.
        other.push(std::ptr::dangling_mut::<c_void>(), 100, now);
        other.pop();
        unseeded.push(std::ptr::dangling_mut::<c_void>(), 100, now);
        unseeded.pop();
        got.push(
            mine.push(std::ptr::dangling_mut::<c_void>(), 100, now)
                .then(|| {
                    let t = mine.front_time();
                    mine.pop();
                    t
                })
                .flatten(),
        );
    }
    assert_eq!(got, reference);
}

#[test]
fn the_delayer_and_dropper_rngs_are_seeded_differently_from_each_other() {
    // Uniform delay (mean 1e6, std 1e5 -> [826_795, 1_173_205)) and Random(0.5).
    // Both consume exactly one uniform draw per packet. If both RNGs started
    // from the same state, packet i would be dropped iff its delay draw u_i is
    // below 0.5, i.e. every ACCEPTED packet would have a delay >= 1e6 (the
    // midpoint). With independent streams about half of the accepted delays lie
    // below the midpoint: ~2000 accepted packets, s.e. 0.011, bound 0.3..0.7.
    let mut cfg = config(Some(99), false, LossModel::Random(0.5), false);
    cfg.delayer.distribution = RVDistribution::UniformDistribution;
    let accepted: Vec<f64> = run(cfg, 4000)
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.map(|t| t - i as f64 * 1000.0))
        .collect();
    assert!(accepted.len() > 1500, "{} accepted", accepted.len());
    let below = accepted.iter().filter(|&&d| d < 1e6).count() as f64 / accepted.len() as f64;
    assert!(
        (0.3..0.7).contains(&below),
        "fraction of accepted delays below the midpoint: {below}"
    );
}
