//! Exact arithmetic of `Buffer::push`: the +4 FEC byte rule, truncation of the
//! send time, fractional limiter offsets, FIFO send times, capacity limits.
//!
//! Wire size used by the limiters: `(len + 4 + 20) * 8` bit (4 = the FEC bytes
//! `Buffer` adds, 20 = L1 overhead the limiter adds).
//! At 1 Gb/s (1 bit/ns) a Spacer therefore delays a second packet of the same
//! instant by exactly that many ns.

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

/// Limiter active; the limit is derived: `capacity * 1542 * 8` bit.
fn limited_with_capacity(capacity: usize, mode: LimiterMode, rate_bps: f64) -> Config {
    let mut c = config(capacity, false, 0.0);
    c.limiter = LimiterConfig {
        mode,
        activate: true,
        rate_bps,
    };
    c
}

fn limited(mode: LimiterMode, rate_bps: f64) -> Config {
    limited_with_capacity(16, mode, rate_bps)
}

fn pkt(i: usize) -> *mut c_void {
    i as *mut c_void
}

#[test]
fn four_fec_bytes_are_added_to_every_packet_before_the_spacer() {
    for len in [0u32, 1, 60, 100, 1496, 1500, 9000] {
        let mut b = Buffer::new(limited(LimiterMode::Spacer, 1e9));
        assert!(b.push(pkt(1), len, 0.0));
        assert!(b.push(pkt(2), len, 0.0));
        b.pop();
        let expected = ((len + 4 + 20) * 8) as f64;
        assert_eq!(b.front_time(), Some(expected), "len {len}");
    }
}

#[test]
fn spacer_limit_check_includes_the_fec_bytes() {
    // Capacity 1: derived limit 1542 * 8 = 12336 bit. len 1518 is
    // (1518 + 4 + 20) * 8 = 12336 bit = exactly the limit; len 1519 is 8 bit
    // more. (Without the 4 FEC bytes the boundary would be len 1502.)
    let mut at_limit = Buffer::new(limited_with_capacity(1, LimiterMode::Spacer, 1e9));
    assert!(at_limit.push(pkt(1), 1518, 0.0));
    let mut above = Buffer::new(limited_with_capacity(1, LimiterMode::Spacer, 1e9));
    assert!(!above.push(pkt(1), 1519, 0.0));
    assert!(above.is_empty());
}

#[test]
fn tbf_budget_check_includes_the_fec_bytes() {
    // Same boundary for the bucket depth, which starts full.
    let mut b = Buffer::new(limited_with_capacity(
        1,
        LimiterMode::TokenBucketFilter,
        1e9,
    ));
    assert!(b.push(pkt(1), 1518, 0.0));
    let mut b = Buffer::new(limited_with_capacity(
        1,
        LimiterMode::TokenBucketFilter,
        1e9,
    ));
    assert!(!b.push(pkt(1), 1519, 0.0));
}

#[test]
fn tbf_refills_with_time_and_drops_the_excess_instead_of_delaying_it() {
    // Capacity 2: bucket 24672 bit (full), 1 bit/ns. len 1518 = 12336 bit, so
    // two packets empty the bucket. Popping keeps the buffer itself below its
    // capacity: only the limiter rejects.
    let mut b = Buffer::new(limited_with_capacity(
        2,
        LimiterMode::TokenBucketFilter,
        1e9,
    ));
    assert!(b.push(pkt(1), 1518, 0.0)); // 24672 -> 12336
    assert!(b.push(pkt(2), 1518, 0.0)); // 12336 -> 0
    assert_eq!(b.pop(), Some(pkt(1)));
    assert_eq!(b.pop(), Some(pkt(2)));
    assert!(!b.push(pkt(3), 1518, 0.0)); // 0 < 12336: dropped, not delayed
    assert!(b.is_empty());
    assert!(!b.push(pkt(4), 1518, 12_335.0)); // 12175 < 12336
    assert!(b.push(pkt(5), 1518, 24_671.0)); // 12175 + 12336 = 24351 (below the 24672 cap) >= 12336
    assert_eq!(b.len(), 1);
    // TBF never shifts the send time: it carries now + 0.
    assert_eq!(b.front_time(), Some(24_671.0));
}

#[test]
fn send_time_is_truncated_towards_zero_for_fractional_times() {
    // (now, delay, expected whole ns). All operands are exact binary fractions.
    for (now, delay, expected) in [
        (0.0, 0.0, 0.0),
        (0.0, 0.9375, 0.0),
        (10.0, 0.5, 10.0),
        (5.75, 100.0, 105.0),
        (0.25, 0.75, 1.0),
        (1e12 + 0.75, 0.0, 1e12),
        (123_456_789.5, 0.25, 123_456_789.0),
    ] {
        let mut b = Buffer::new(config(4, false, delay));
        assert!(b.push(pkt(1), 100, now));
        assert_eq!(b.front_time(), Some(expected), "now {now} delay {delay}");
    }
}

#[test]
fn fifo_negative_times_saturate_to_zero() {
    // `as u64` of a negative f64 is 0.
    let mut b = Buffer::new(config(4, false, 2.0));
    assert!(b.push(pkt(1), 100, -5.0));
    assert_eq!(b.front_time(), Some(0.0));
}

#[test]
fn fractional_spacer_offsets_are_truncated_after_adding_to_the_send_time() {
    // 100 Gb/s = 100 bit/ns; len 1500 -> (1500 + 4 + 20) * 8 = 12192 bit
    // -> 121.92 ns per packet: send times 0, 121.92, 243.84, 365.76, 487.68.
    let mut b = Buffer::new(limited(LimiterMode::Spacer, 100e9));
    for i in 0..5 {
        assert!(b.push(pkt(i + 1), 1500, 0.0));
    }
    for expected in [0.0, 121.0, 243.0, 365.0, 487.0] {
        assert_eq!(b.front_time(), Some(expected));
        b.pop();
    }
}

#[test]
fn fifo_send_times_are_bit_identical_to_the_truncated_f64_time() {
    // Deterministic delay 1000.25; `now` sweeps pseudo-random quarter-ns values
    // with a deterministic LCG, monotone so that the buffer is also time-sorted.
    let delay = 1000.25;
    let mut b = Buffer::new(config(1024, false, delay));
    let mut now = 0.0f64;
    let mut state = 12345u64;
    let mut expected = Vec::new();
    for i in 0..1000 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        now += ((state >> 40) % 4000) as f64 * 0.25; // 0 .. 999.75 ns in 0.25 steps
        assert!(b.push(pkt(i + 1), 100, now));
        expected.push(((now + delay) as u64) as f64);
    }
    for (i, e) in expected.into_iter().enumerate() {
        assert_eq!(b.front_time().unwrap().to_bits(), e.to_bits(), "packet {i}");
        assert_eq!(b.pop(), Some(pkt(i + 1)));
    }
}

#[test]
fn capacity_is_exact_in_both_modes_and_len_never_exceeds_it() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(1000, reordering, 100.0));
        for i in 0..1000 {
            assert!(
                b.push(pkt(i + 1), 100, i as f64),
                "reordering={reordering} i={i}"
            );
        }
        assert_eq!(b.len(), 1000);
        assert_eq!(b.capacity(), 1000);
        for i in 0..50 {
            assert!(
                !b.push(pkt(5000 + i), 100, 2000.0),
                "reordering={reordering}"
            );
        }
        assert_eq!(b.len(), 1000);
        // Exactly one slot is freed by one pop.
        assert!(b.pop().is_some());
        assert!(b.push(pkt(9999), 100, 2000.0));
        assert!(!b.push(pkt(10000), 100, 2000.0));
    }
}

#[test]
fn a_packet_rejected_by_the_limiter_does_not_occupy_capacity() {
    // Capacity 2: bucket 24672 bit = two 1518-byte packets (12336 bit each).
    let mut b = Buffer::new(limited_with_capacity(
        2,
        LimiterMode::TokenBucketFilter,
        1e9,
    ));
    assert!(b.push(pkt(1), 1518, 0.0));
    assert!(b.push(pkt(2), 1518, 0.0));
    assert_eq!(b.pop(), Some(pkt(1))); // one slot free again, bucket empty
    for i in 0..10 {
        assert!(!b.push(pkt(10 + i), 1518, 0.0), "rejected by the limiter");
    }
    assert_eq!(b.len(), 1);
    assert_eq!(b.pop(), Some(pkt(2)));
    assert_eq!(b.pop(), None);
}

#[test]
fn fifo_order_is_arrival_order_regardless_of_send_times() {
    let mut b = Buffer::new(config(8, false, 0.0));
    // Send times 500, 100, 900, 100: FIFO ignores them.
    for (i, now) in [500.0, 100.0, 900.0, 100.0].into_iter().enumerate() {
        assert!(b.push(pkt(i + 1), 100, now));
    }
    let times: Vec<f64> = (0..4)
        .map(|_| {
            let t = b.front_time().unwrap();
            b.pop();
            t
        })
        .collect();
    assert_eq!(times, vec![500.0, 100.0, 900.0, 100.0]);
}
