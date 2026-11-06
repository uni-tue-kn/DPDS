//! Tests for the basic `Buffer` API: `new`, `push`, `pop`, `front_time`,
//! `is_empty`, `len`, `capacity`.
//!
//! Packet pointers are opaque here (never dereferenced), so small integers
//! cast to pointers serve as fake packets.
//!
//! `Config::seed` is `None` here: entropy seeding, nothing below is stochastic.
//!
//! Delay is `DeterministicDistribution` with a fixed mean and no correlation,
//! so the scheduled send time of a packet pushed at `now` is exactly
//! `(now + mean) as u64`.

use std::ffi::c_void;

use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

fn config(capacity: usize, reordering: bool, delay_ns: f64) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering,
        delayer: DelayerConfig {
            distribution: RVDistribution::DeterministicDistribution,
            mean_ns: delay_ns,
            std_ns: 0.0,
            activate_correlation: false,
            half_life_period_ns: 1.0,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1e9,
        },
        loss_model: LossModel::Random(0.0),
        seed: None,
    }
}

fn pkt(i: usize) -> *mut c_void {
    i as *mut c_void
}

#[test]
fn new_buffer_is_empty_and_reports_its_capacity() {
    let mut b = Buffer::new(config(8, false, 100.0));
    assert!(b.is_empty());
    assert_eq!(b.len(), 0);
    assert_eq!(b.capacity(), 8);
    assert_eq!(b.front_time(), None);
    assert_eq!(b.pop(), None);
}

#[test]
fn pop_and_front_time_on_empty_buffer_return_none_in_both_modes() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(4, reordering, 100.0));
        assert_eq!(b.pop(), None, "reordering={reordering}");
        assert_eq!(b.front_time(), None, "reordering={reordering}");
    }
}

#[test]
fn push_accepts_and_updates_len_and_front_time() {
    let mut b = Buffer::new(config(8, false, 1000.0));
    assert!(b.push(pkt(1), 100, 0.0));
    assert_eq!(b.len(), 1);
    assert!(!b.is_empty());
    assert_eq!(b.front_time(), Some(1000.0));
}

#[test]
fn send_time_is_now_plus_delay_truncated_to_whole_ns() {
    let mut b = Buffer::new(config(8, false, 100.0));
    assert!(b.push(pkt(1), 100, 5.75));
    // 5.75 + 100 = 105.75 -> truncated to 105 ns.
    assert_eq!(b.front_time(), Some(105.0));
}

#[test]
fn fifo_mode_pops_in_push_order() {
    let mut b = Buffer::new(config(16, false, 1000.0));
    for i in 1..=5 {
        assert!(b.push(pkt(i), 100, i as f64));
    }
    for i in 1..=5 {
        assert_eq!(b.pop(), Some(pkt(i)));
    }
    assert_eq!(b.pop(), None);
    assert!(b.is_empty());
}

#[test]
fn fifo_mode_does_not_reorder_even_if_send_times_decrease() {
    let mut b = Buffer::new(config(8, false, 100.0));
    assert!(b.push(pkt(1), 100, 1000.0)); // due 1100
    assert!(b.push(pkt(2), 100, 0.0)); // due 100
    assert_eq!(b.front_time(), Some(1100.0)); // head is still the first pushed
    assert_eq!(b.pop(), Some(pkt(1)));
    assert_eq!(b.front_time(), Some(100.0));
    assert_eq!(b.pop(), Some(pkt(2)));
}

#[test]
fn reordering_mode_pops_earliest_send_time_first() {
    let mut b = Buffer::new(config(8, true, 100.0));
    assert!(b.push(pkt(1), 100, 1000.0)); // due 1100
    assert!(b.push(pkt(2), 100, 0.0)); // due 100
    assert!(b.push(pkt(3), 100, 500.0)); // due 600
    assert_eq!(b.front_time(), Some(100.0));
    assert_eq!(b.pop(), Some(pkt(2)));
    assert_eq!(b.front_time(), Some(600.0));
    assert_eq!(b.pop(), Some(pkt(3)));
    assert_eq!(b.front_time(), Some(1100.0));
    assert_eq!(b.pop(), Some(pkt(1)));
    assert_eq!(b.pop(), None);
}

#[test]
fn reordering_mode_with_monotonic_times_behaves_like_fifo() {
    let mut b = Buffer::new(config(32, true, 50.0));
    for i in 1..=10 {
        assert!(b.push(pkt(i), 64, (i * 10) as f64));
    }
    for i in 1..=10 {
        assert_eq!(b.pop(), Some(pkt(i)));
    }
}

#[test]
fn push_into_full_buffer_returns_false_and_keeps_contents() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(2, reordering, 100.0));
        assert!(b.push(pkt(1), 100, 0.0));
        assert!(b.push(pkt(2), 100, 1.0));
        assert!(!b.push(pkt(3), 100, 2.0), "reordering={reordering}");
        assert_eq!(b.len(), 2);
        assert_eq!(b.pop(), Some(pkt(1)));
        assert_eq!(b.pop(), Some(pkt(2)));
        assert_eq!(b.pop(), None, "rejected packet must not appear");
    }
}

#[test]
fn space_is_reusable_after_pop() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(1, reordering, 100.0));
        assert!(b.push(pkt(1), 100, 0.0));
        assert!(!b.push(pkt(2), 100, 1.0));
        assert_eq!(b.pop(), Some(pkt(1)));
        assert!(b.push(pkt(3), 100, 2.0), "reordering={reordering}");
        assert_eq!(b.pop(), Some(pkt(3)));
    }
}

#[test]
fn zero_capacity_buffer_rejects_every_push() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(0, reordering, 100.0));
        assert_eq!(b.capacity(), 0);
        assert!(!b.push(pkt(1), 100, 0.0));
        assert!(b.is_empty());
    }
}

#[test]
fn full_loss_drops_every_packet() {
    let mut cfg = config(8, false, 100.0);
    cfg.loss_model = LossModel::Random(1.0);
    let mut b = Buffer::new(cfg);
    for i in 1..=20 {
        assert!(!b.push(pkt(i), 100, i as f64));
    }
    assert!(b.is_empty());
}

#[test]
fn zero_loss_never_drops() {
    let mut b = Buffer::new(config(1000, false, 100.0));
    for i in 1..=1000 {
        assert!(b.push(pkt(i), 100, i as f64));
    }
    assert_eq!(b.len(), 1000);
}

#[test]
fn token_bucket_filter_drops_packet_that_exceeds_budget() {
    // Capacity 1 gives the bucket the derived depth 1 * 1542 * 8 = 12336 bit.
    // A 1519-byte packet is (1519 + 4 + 20) * 8 = 12344 bit: larger than the
    // whole bucket, so it is dropped although the bucket starts full.
    let mut cfg = config(1, false, 0.0);
    cfg.limiter = LimiterConfig {
        mode: LimiterMode::TokenBucketFilter,
        activate: true,
        rate_bps: 1e9,
    };
    let mut b = Buffer::new(cfg);
    assert!(!b.push(pkt(1), 1519, 0.0));
    assert!(b.is_empty());
}

#[test]
fn spacer_drops_packet_that_would_overflow_its_limit() {
    // Capacity 2 gives the Spacer the derived limit 2 * 1542 * 8 = 24672 bit.
    // One 1519-byte packet occupies (1519 + 4 FCS + 20 L1 overhead) * 8 =
    // 12344 bit, so the second one (24688 bit) does not fit although the
    // buffer has room for it.
    let mut cfg = config(2, false, 0.0);
    cfg.limiter = LimiterConfig {
        mode: LimiterMode::Spacer,
        activate: true,
        rate_bps: 1e9,
    };
    let mut b = Buffer::new(cfg);
    assert!(b.push(pkt(1), 1519, 0.0));
    assert!(
        !b.push(pkt(2), 1519, 0.0),
        "12344 + 12344 bit > 24672 bit limit"
    );
    assert_eq!(b.len(), 1);
}

#[test]
fn spacer_spaces_back_to_back_packets_by_their_serialization_time() {
    // rate = 1 bit/ns; a 100-byte packet is 992 bit on the wire (see above),
    // so the second packet is scheduled 992 ns after the first.
    let mut cfg = config(8, false, 0.0);
    cfg.limiter = LimiterConfig {
        mode: LimiterMode::Spacer,
        activate: true,
        rate_bps: 1e9,
    };
    let mut b = Buffer::new(cfg);
    assert!(b.push(pkt(1), 100, 0.0));
    assert!(b.push(pkt(2), 100, 0.0));
    assert_eq!(b.front_time(), Some(0.0));
    assert_eq!(b.pop(), Some(pkt(1)));
    assert_eq!(b.front_time(), Some(992.0));
}

#[test]
fn deactivated_limiter_adds_no_offset() {
    let mut b = Buffer::new(config(8, false, 0.0));
    assert!(b.push(pkt(1), 1500, 0.0));
    assert!(b.push(pkt(2), 1500, 0.0));
    assert_eq!(b.front_time(), Some(0.0));
    b.pop();
    assert_eq!(b.front_time(), Some(0.0));
}
