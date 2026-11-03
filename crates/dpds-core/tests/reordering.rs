//! Reordering mode keeps entry order for equal send times.
//!
//! The heap key (hand-derived below) is
//! `key = floor(t * 64) + tie` (`TIE_FRAC_BITS = 6`, 1/64 ns resolution), with
//! `t = now + delay (+ limiter offset)` before truncation and `tie` the number
//! of accepted pushes since the last push with a different `current_time_ns`
//! (0 for the first packet of an instant). `front_time()` is `(key >> 6) as
//! f64`, `pop`/`pop_due` order by `key`. Saturating arithmetic, no overflow.
//! FIFO mode is unchanged (`as u64` truncation, no keys).
//!
//! Constants used in the expectations: `105.75 * 64 = 6768 = 105 * 64 + 48`,
//! `105.0 * 64 = 6720`, `100.0 * 64 = 6400`; all instants/delays are exact
//! binary fractions so `t * 64` is exact.

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
            limit_bit: 1e9,
        },
        loss_model: LossModel::Random(0.0),
        seed: None,
    }
}

fn pkt(i: usize) -> *mut c_void {
    i as *mut c_void
}

fn drain(b: &mut Buffer) -> Vec<usize> {
    std::iter::from_fn(|| b.pop()).map(|p| p as usize).collect()
}

#[test]
fn packets_entering_at_one_instant_leave_in_push_order() {
    let mut b = Buffer::new(config(2000, true, 100.0));
    for i in 1..=1000 {
        assert!(b.push(pkt(i), 100, 0.0));
    }
    assert_eq!(drain(&mut b), (1..=1000).collect::<Vec<_>>());
}

#[test]
fn the_tie_rule_also_holds_for_pop_due() {
    let mut b = Buffer::new(config(2000, true, 100.0));
    for i in 1..=500 {
        assert!(b.push(pkt(i), 100, 0.0));
    }
    let popped: Vec<usize> = std::iter::from_fn(|| b.pop_due(1e9))
        .map(|p| p as usize)
        .collect();
    assert_eq!(popped, (1..=500).collect::<Vec<_>>());
}

#[test]
fn fifo_mode_is_unchanged_for_packets_entering_at_one_instant() {
    let mut b = Buffer::new(config(2000, false, 100.0));
    for i in 1..=1000 {
        assert!(b.push(pkt(i), 100, 0.0));
        // No tie offsets in FIFO mode: every packet reports the same time.
        assert_eq!(b.front_time(), Some(100.0));
    }
    assert_eq!(drain(&mut b), (1..=1000).collect::<Vec<_>>());
}

#[test]
fn first_packet_of_an_instant_reports_exactly_the_truncated_send_time() {
    for (now, delay, expected) in [
        (5.75, 100.0, 105.0),
        (0.999_023_437_5, 0.0, 0.0), // 1023/1024
        (1e12 + 0.75, 0.0, 1e12),
        (0.0, 0.0, 0.0),
        (7.0, 0.0, 7.0),
    ] {
        let mut b = Buffer::new(config(4, true, delay));
        assert!(b.push(pkt(1), 100, now));
        assert_eq!(b.front_time(), Some(expected), "now {now} delay {delay}");
    }
}

#[test]
fn reported_front_time_with_ties_is_key_shifted_right_by_6() {
    // t = 105.75 -> floor(t * 64) = 6768; packet i has key 6768 + i, so
    // front_time = (6768 + i) >> 6 = 105 for i < 16 and 106 from i = 16
    // (6784 = 106 * 64).
    let mut b = Buffer::new(config(64, true, 100.0));
    for i in 0..20 {
        assert!(b.push(pkt(i + 1), 100, 5.75));
    }
    for i in 0..20usize {
        let expected = ((6768 + i) >> 6) as f64;
        assert_eq!(b.front_time(), Some(expected), "packet {i}");
        assert_eq!(b.pop(), Some(pkt(i + 1)));
    }
}

#[test]
fn front_time_carries_into_the_next_ns_after_64_ties() {
    // t = 105.0 -> floor(t * 64) = 6720 = 105 * 64: ties 0..=63 stay in 105,
    // tie 64 gives 6784 >> 6 = 106.
    let mut b = Buffer::new(config(128, true, 100.0));
    for i in 0..70 {
        assert!(b.push(pkt(i + 1), 100, 5.0));
    }
    for i in 0..70usize {
        let expected = if i < 64 { 105.0 } else { 106.0 };
        assert_eq!(b.front_time(), Some(expected), "packet {i}");
        b.pop();
    }
}

#[test]
fn pop_due_uses_the_reported_whole_ns_time_of_tied_packets() {
    // Same setup as above (t = 105.75): packets 0..16 report 105, 16..20 report 106.
    let mut b = Buffer::new(config(64, true, 100.0));
    for i in 0..20 {
        assert!(b.push(pkt(i + 1), 100, 5.75));
    }
    for i in 0..16 {
        assert_eq!(b.pop_due(105.0), Some(pkt(i + 1)), "packet {i}");
    }
    assert_eq!(b.pop_due(105.0), None);
    assert_eq!(b.front_time(), Some(106.0));
    assert_eq!(b.pop_due(106.0), Some(pkt(17)));
}

#[test]
fn sub_nanosecond_differences_order_packets_from_different_instants() {
    // A: t = 100.5 (key 6432), B: t = 100.25 (key 6416). Both report 100 ns.
    let mut b = Buffer::new(config(4, true, 100.0));
    assert!(b.push(pkt(1), 100, 0.5)); // A
    assert!(b.push(pkt(2), 100, 0.25)); // B
    assert_eq!(b.front_time(), Some(100.0));
    assert_eq!(
        drain(&mut b),
        vec![2, 1],
        "B is 1/4 ns earlier and must leave first"
    );
}

#[test]
fn the_tie_counter_restarts_when_the_instant_changes() {
    // delay 0, keys: instant 10.0 has three packets 640, 641, 642.
    // P enters at 10 + 3/64 (floor(t*64) = 643, tie must restart at 0 -> 643),
    // Q at 10 + 4/64 (644). With a counter that was NOT reset, P would get
    // 643 + 3 = 646 and overtake Q (644).
    let mut b = Buffer::new(config(16, true, 0.0));
    for i in 1..=3 {
        assert!(b.push(pkt(i), 100, 10.0));
    }
    assert!(b.push(pkt(4), 100, 10.046875)); // P
    assert!(b.push(pkt(5), 100, 10.0625)); // Q
    assert_eq!(drain(&mut b), vec![1, 2, 3, 4, 5]);
}

#[test]
fn ties_do_not_accumulate_across_different_instants() {
    // Every packet enters at its own instant, so every tie is 0 and the keys are
    // just floor(t * 64): T (640) must leave first, then 642, 643, 644. A tie that
    // counted all earlier pushes (a global sequence number) would give
    // 642, 644, 646 and 640 + 3 = 643 and so let T overtake S2 and S3 but not S1.
    let mut b = Buffer::new(config(16, true, 0.0));
    assert!(b.push(pkt(1), 100, 10.03125)); // S1: 640 + 2
    assert!(b.push(pkt(2), 100, 10.046875)); // S2: 640 + 3
    assert!(b.push(pkt(3), 100, 10.0625)); // S3: 640 + 4
    assert!(b.push(pkt(4), 100, 10.0)); // T: 640
    assert_eq!(drain(&mut b), vec![4, 1, 2, 3]);
}

#[test]
fn rejected_pushes_for_lack_of_capacity_do_not_advance_the_tie_counter() {
    // Capacity 1, t = 100.0 (key base 6400). P0 is accepted (tie 0); 100 pushes
    // at the same instant are rejected; after P0 leaves, P1 must get tie 1
    // (key 6401, front_time 100). If the rejected pushes had counted, its tie
    // would be 101 (key 6501, front_time 101).
    let mut b = Buffer::new(config(1, true, 0.0));
    assert!(b.push(pkt(1), 100, 100.0));
    for i in 0..100 {
        assert!(!b.push(pkt(100 + i), 100, 100.0));
    }
    assert_eq!(b.pop(), Some(pkt(1)));
    assert!(b.push(pkt(2), 100, 100.0));
    assert_eq!(b.front_time(), Some(100.0));
}

#[test]
fn packets_dropped_by_the_loss_model_do_not_advance_the_tie_counter() {
    // GE(p = 1, r = 1, k = 0, h = 1): Good drops and moves to Bad, Bad keeps and
    // moves back: drop, keep, drop, keep, ... 130 pushes at t = 100.0 (base
    // 6400) accept 65 packets with ties 0..=64 if drops do not count, so
    // front_time is 100 for the first 64 and 101 for the 65th. If drops
    // counted, the 33rd accepted packet (tie 65) would already report 101.
    let mut cfg = config(256, true, 0.0);
    cfg.loss_model = LossModel::GilbertElliot(1.0, 1.0, 0.0, 1.0, true);
    let mut b = Buffer::new(cfg);
    let mut accepted = 0;
    for i in 0..130 {
        if b.push(pkt(i + 1), 100, 100.0) {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 65);
    for j in 0..65 {
        let expected = if j < 64 { 100.0 } else { 101.0 };
        assert_eq!(b.front_time(), Some(expected), "accepted packet {j}");
        b.pop();
    }
}

#[test]
fn key_arithmetic_saturates_instead_of_overflowing() {
    // t = 1e19 ns: t * 64 > u64::MAX. Must not panic (debug overflow checks are
    // on in tests) and every packet must still come out exactly once; the
    // reported time is u64::MAX >> 6 = 2^58 - 1 (as f64: 2^58).
    let mut b = Buffer::new(config(16, true, 0.0));
    for i in 1..=3 {
        assert!(b.push(pkt(i), 100, 1e19));
    }
    assert_eq!(b.front_time(), Some((u64::MAX >> 6) as f64));
    let mut out = drain(&mut b);
    out.sort_unstable();
    assert_eq!(out, vec![1, 2, 3]);
}

#[test]
fn tie_offsets_saturate_at_the_top_of_the_key_range() {
    // t = 2^58 - 32 is the largest f64 below 2^58, t * 64 = 2^64 - 2048 exactly,
    // so ties 0..=2047 give distinct keys up to u64::MAX; tie >= 2048 would
    // overflow and must saturate (to u64::MAX).
    let t = (1u64 << 58) as f64 - 32.0;
    let mut b = Buffer::new(config(4096, true, 0.0));
    for i in 1..=2100 {
        assert!(b.push(pkt(i), 100, t));
    }
    let out = drain(&mut b);
    assert_eq!(out.len(), 2100);
    assert_eq!(
        &out[..2047],
        &(1..=2047).collect::<Vec<_>>()[..],
        "distinct keys keep push order"
    );
    let mut tail = out[2047..].to_vec();
    tail.sort_unstable();
    assert_eq!(tail, (2048..=2100).collect::<Vec<_>>());
}

#[test]
fn random_arrivals_come_out_sorted_by_key_and_in_push_order_within_an_instant() {
    // Deterministic LCG; `now` is non-decreasing in steps of k/64 ns, k in 0..=3,
    // so many packets share an instant and many keys are close. delay = 100.
    // Expected key (test-side implementation of the rule):
    //   floor((now + 100) * 64) + tie, tie restarting when `now` changes.
    let mut b = Buffer::new(config(4096, true, 100.0));
    let mut state = 7u64;
    let mut now = 0.0f64;
    let mut prev_now = f64::NAN;
    let mut tie = 0u64;
    let mut keys = Vec::new(); // key by packet id - 1
    let mut instants = Vec::new();
    for i in 0..3000 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        now += ((state >> 40) % 4) as f64 / 64.0;
        if now != prev_now {
            tie = 0;
            prev_now = now;
        }
        keys.push((((now + 100.0) * 64.0).floor() as u64) + tie);
        instants.push(now);
        tie += 1;
        assert!(b.push(pkt(i + 1), 100, now));
    }
    let out = drain(&mut b);
    assert_eq!(out.len(), 3000);
    let popped_keys: Vec<u64> = out.iter().map(|&id| keys[id - 1]).collect();
    assert!(
        popped_keys.windows(2).all(|w| w[0] <= w[1]),
        "pop order must be non-decreasing in key"
    );
    // Within one instant keys strictly increase, so push order must be kept.
    for w in out.windows(2) {
        if instants[w[0] - 1] == instants[w[1] - 1] {
            assert!(
                w[0] < w[1],
                "same-instant packets {} and {} reordered",
                w[0],
                w[1]
            );
        }
    }
}
