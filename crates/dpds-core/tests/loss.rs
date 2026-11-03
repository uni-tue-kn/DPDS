//! Loss models (`Dropper`) observed through `Buffer::push`: with the limiter
//! off, a huge buffer and zero delay, `push` returns `false` exactly when the
//! dropper dropped the packet. Stochastic tests are seeded (`Config::seed`).
//!
//! Gilbert-Elliot as implemented: per packet, drop iff `u1 > k` (Good) or
//! `u1 > h` (Bad) with `u1` uniform in [0, 1), then transition with `u2`:
//! Good -> Bad iff `u2 < p`, Bad -> Good iff `u2 < r`. The chain always
//! starts in Good. Deterministic edge cases use p, r, k, h in {0, 1}, where the
//! outcome is exact except for `u1 == 0.0` exactly (probability 2^-53 per
//! packet, ignored).
//!
//! Statistics (stationary Good probability `r/(p+r)`):
//! - Random(p), n = 100_000: s.e. = sqrt(p(1-p)/n) <= 0.0016, bound 0.01
//!   (>= 6 s.e.).
//! - Gilbert (k = 1, h = 0, p = 0.1, r = 0.3), n = 200_000: loss rate
//!   `p/(p+r) = 0.25`; the indicator has autocorrelation 0.6 so the s.e. of the
//!   rate is sqrt(0.1875 * 4 / n) = 0.0019, bound 0.012 (6 s.e.); mean loss-run
//!   length `1/r = 3.33` over ~15_000 runs with run variance `(1-r)/r^2 = 7.8`:
//!   s.e. 0.023, bound 0.15.
//! - GE with k = 0.9, h = 0.2: loss = 0.75 * 0.1 + 0.25 * 0.8 = 0.275; the
//!   variance of the rate is at most 0.2 * 4 / n -> s.e. <= 0.002, bound 0.015.

use std::ffi::c_void;

use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

fn config(loss_model: LossModel, capacity: usize, seed: u64) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering: false,
        delayer: DelayerConfig {
            distribution: RVDistribution::DeterministicDistribution,
            mean_ns: 0.0,
            std_ns: 0.0,
            activate_correlation: false,
            half_life_period_ns: 1.0,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1e9,
            limit_bit: 1e9,
        },
        loss_model,
        seed: Some(seed),
    }
}

fn pkt(i: usize) -> *mut c_void {
    i as *mut c_void
}

/// `true` per pushed packet iff it was *accepted* (not dropped).
fn outcomes(cfg: Config, n: usize) -> Vec<bool> {
    let mut b = Buffer::new(cfg);
    (0..n).map(|i| b.push(pkt(i + 1), 100, 0.0)).collect()
}

fn loss_rate(o: &[bool]) -> f64 {
    o.iter().filter(|&&ok| !ok).count() as f64 / o.len() as f64
}

#[test]
fn random_zero_never_drops_and_random_one_always_drops() {
    assert!(outcomes(config(LossModel::Random(0.0), 1000, 1), 1000)
        .iter()
        .all(|&ok| ok));
    assert!(outcomes(config(LossModel::Random(1.0), 1000, 1), 1000)
        .iter()
        .all(|&ok| !ok));
}

#[test]
fn random_loss_rate_matches_p() {
    for (p, seed) in [(0.1, 31), (0.5, 32), (0.9, 33)] {
        let rate = loss_rate(&outcomes(
            config(LossModel::Random(p), 100_000, seed),
            100_000,
        ));
        assert!((rate - p).abs() < 0.01, "p = {p}: observed {rate}");
    }
}

#[test]
fn random_losses_are_independent_of_the_previous_packet() {
    // P(drop | previous dropped) must equal p for a memoryless model.
    let o = outcomes(config(LossModel::Random(0.5), 100_000, 34), 100_000);
    let after_drop: Vec<bool> = o.windows(2).filter(|w| !w[0]).map(|w| !w[1]).collect();
    let frac = after_drop.iter().filter(|&&d| d).count() as f64 / after_drop.len() as f64;
    // ~50_000 conditioned samples: s.e. 0.0022, bound 0.015.
    assert!((frac - 0.5).abs() < 0.015, "{frac}");
}

#[test]
fn gilbert_elliot_with_p_and_r_one_alternates_keep_drop() {
    // Good keeps (k = 1) and always moves to Bad (p = 1); Bad drops (h = 0)
    // and always moves back (r = 1): keep, drop, keep, drop, ...
    // The initial `state_is_good = false` flag must be ignored (starts Good).
    let o = outcomes(
        config(LossModel::GilbertElliot(1.0, 1.0, 1.0, 0.0, false), 1000, 1),
        200,
    );
    for (i, ok) in o.iter().enumerate() {
        assert_eq!(*ok, i % 2 == 0, "packet {i}");
    }
}

#[test]
fn gilbert_elliot_bad_state_can_be_absorbing() {
    // p = 1, r = 0, k = 1, h = 0: first packet kept, then Bad forever.
    let o = outcomes(
        config(LossModel::GilbertElliot(1.0, 0.0, 1.0, 0.0, true), 1000, 1),
        100,
    );
    assert!(o[0]);
    assert!(o[1..].iter().all(|&ok| !ok));
}

#[test]
fn gilbert_elliot_good_state_can_be_absorbing_and_lossless_or_lossy() {
    // p = 0: never leaves Good. k = 1: nothing lost, even though the config
    // says state_is_good = false and Bad would drop everything (h = 0, r = 0).
    let o = outcomes(
        config(LossModel::GilbertElliot(0.0, 0.0, 1.0, 0.0, false), 1000, 1),
        100,
    );
    assert!(o.iter().all(|&ok| ok), "must start in Good");

    // k = 0: everything lost while staying in Good.
    let o = outcomes(
        config(LossModel::GilbertElliot(0.0, 1.0, 0.0, 1.0, true), 1000, 1),
        100,
    );
    assert!(o.iter().all(|&ok| !ok));
}

#[test]
fn gilbert_elliot_loss_rate_and_burst_length_match_the_markov_chain() {
    let o = outcomes(
        config(
            LossModel::GilbertElliot(0.1, 0.3, 1.0, 0.0, true),
            200_000,
            41,
        ),
        200_000,
    );
    let rate = loss_rate(&o);
    assert!((rate - 0.25).abs() < 0.012, "loss rate {rate}");

    let losses = o.iter().filter(|&&ok| !ok).count();
    let runs = o.windows(2).filter(|w| w[0] && !w[1]).count() + usize::from(!o[0]);
    let mean_run = losses as f64 / runs as f64;
    assert!(
        (mean_run - 1.0 / 0.3).abs() < 0.15,
        "mean loss burst {mean_run}"
    );
}

#[test]
fn gilbert_elliot_with_lossy_good_and_partially_lossy_bad_state() {
    let o = outcomes(
        config(
            LossModel::GilbertElliot(0.1, 0.3, 0.9, 0.2, true),
            200_000,
            42,
        ),
        200_000,
    );
    let rate = loss_rate(&o);
    assert!((rate - 0.275).abs() < 0.015, "loss rate {rate}");
}

#[test]
fn packets_are_accounted_by_the_limiter_before_the_dropper_drops_them() {
    // GE with k = 0 in Good drops the first packet, then (p = 1) enters Bad
    // where h = 1 never drops. Spacer at 1 bit/ns; a 100-byte packet is
    // (100 + 4 + 20) * 8 = 992 bit. If the limiter saw the dropped first
    // packet, the second leaves 992 ns later; if drops happened before the
    // limiter it would leave at 0.
    let mut cfg = config(LossModel::GilbertElliot(1.0, 0.0, 0.0, 1.0, true), 8, 1);
    cfg.limiter.activate = true;
    let mut b = Buffer::new(cfg);
    assert!(!b.push(pkt(1), 100, 0.0), "first packet dropped");
    assert!(b.push(pkt(2), 100, 0.0), "second packet kept");
    assert_eq!(b.front_time(), Some(992.0));
}

#[test]
fn a_limiter_rejected_packet_still_steps_the_loss_chain() {
    // Alternating GE (keep, drop, keep, ...). Spacer limit 1000 bit holds one
    // 992 bit packet. A is kept (chain -> Bad); B at the same instant is
    // rejected by the limiter, but the dropper has already been consulted
    // (Bad -> drop, chain -> Good). C, 2000 ns later when the limiter has
    // drained, meets the chain in Good and is kept. Had B not stepped the
    // chain, C would meet Bad and be dropped.
    let mut cfg = config(LossModel::GilbertElliot(1.0, 1.0, 1.0, 0.0, true), 8, 1);
    cfg.limiter.activate = true;
    cfg.limiter.limit_bit = 1000.0;
    let mut b = Buffer::new(cfg);
    assert!(b.push(pkt(1), 100, 0.0));
    assert!(!b.push(pkt(2), 100, 0.0));
    assert!(b.push(pkt(3), 100, 2000.0));
    assert_eq!(b.len(), 2);
}

#[test]
fn a_packet_rejected_for_lack_of_capacity_is_still_accounted_by_the_limiter() {
    // Characterisation of the current order: limiter and dropper run before
    // the capacity check. Capacity 1, Spacer 1 bit/ns, 992 bit per packet.
    let mut cfg = config(LossModel::Random(0.0), 1, 1);
    cfg.limiter.activate = true;
    let mut b = Buffer::new(cfg);
    assert!(b.push(pkt(1), 100, 0.0)); // fill 992
    assert!(!b.push(pkt(2), 100, 0.0)); // buffer full, but fill becomes 1984
    assert_eq!(b.pop(), Some(pkt(1)));
    assert!(b.push(pkt(3), 100, 0.0));
    assert_eq!(b.front_time(), Some(1984.0));
}
