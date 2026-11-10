//! `Buffer::pop_due(now_ns) -> Option<*mut c_void>` and
//! `Buffer::peek_nth(n) -> Option<*mut c_void>`.
//!
//! `pop_due` pops the head iff its send time *as `front_time` reports it* is
//! `<= now_ns` (boundary `==` pops). `peek_nth(n)` is the packet that would be
//! popped n-th from now: FIFO mode the n-th queued entry; reordering mode
//! `n == 0` is exact and `n > 0` is a best-effort look-ahead (`Some` must be
//! right, `None` may just mean "not known cheaply").
//!
//! Everything is deterministic: no seeding needed.

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
fn pop_due_on_an_empty_buffer_is_none() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(4, reordering, 0.0));
        assert_eq!(b.pop_due(0.0), None);
        assert_eq!(b.pop_due(f64::INFINITY), None);
    }
}

#[test]
fn pop_due_pops_exactly_at_the_send_time_and_not_before() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(4, reordering, 1000.0));
        assert!(b.push(pkt(1), 100, 0.0)); // front_time 1000
        assert_eq!(b.pop_due(999.999), None, "reordering={reordering}");
        assert_eq!(b.pop_due(-1.0), None);
        assert_eq!(b.len(), 1, "a refused pop_due must not remove anything");
        assert_eq!(
            b.pop_due(1000.0),
            Some(pkt(1)),
            "`<=` boundary, reordering={reordering}"
        );
        assert!(b.is_empty());
        assert_eq!(b.pop_due(1e9), None);
    }
}

#[test]
fn pop_due_compares_against_the_truncated_front_time() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(4, reordering, 100.0));
        assert!(b.push(pkt(1), 100, 5.75)); // 105.75 -> front_time 105
        assert_eq!(b.front_time(), Some(105.0));
        assert_eq!(b.pop_due(104.99), None);
        assert_eq!(b.pop_due(105.0), Some(pkt(1)), "reordering={reordering}");
    }
}

#[test]
fn pop_due_with_infinite_now_drains_in_pop_order() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(8, reordering, 10.0));
        for i in 1..=5 {
            assert!(b.push(pkt(i), 100, i as f64 * 100.0));
        }
        for i in 1..=5 {
            assert_eq!(b.pop_due(f64::INFINITY), Some(pkt(i)));
        }
        assert_eq!(b.pop_due(f64::INFINITY), None);
    }
}

#[test]
fn fifo_head_that_is_not_due_blocks_later_entries_that_are() {
    let mut b = Buffer::new(config(4, false, 100.0));
    assert!(b.push(pkt(1), 100, 1000.0)); // due 1100
    assert!(b.push(pkt(2), 100, 0.0)); // due 100, behind the head
    assert_eq!(b.pop_due(500.0), None);
    assert_eq!(b.pop_due(1100.0), Some(pkt(1)));
    assert_eq!(b.pop_due(1100.0), Some(pkt(2)));
}

#[test]
fn reordering_pop_due_serves_the_earliest_entry_even_if_pushed_later() {
    let mut b = Buffer::new(config(4, true, 100.0));
    assert!(b.push(pkt(1), 100, 1000.0)); // due 1100
    assert!(b.push(pkt(2), 100, 0.0)); // due 100
    assert_eq!(b.pop_due(500.0), Some(pkt(2)));
    assert_eq!(b.pop_due(500.0), None);
    assert_eq!(b.pop_due(1100.0), Some(pkt(1)));
}

#[test]
fn pop_due_is_equivalent_to_front_time_check_then_pop() {
    // Deterministic pseudo-random operation sequence applied to two identical
    // buffers per mode; one uses pop_due, the other the manual front_time/pop.
    for reordering in [false, true] {
        let mut a = Buffer::new(config(64, reordering, 50.0));
        let mut m = Buffer::new(config(64, reordering, 50.0));
        let mut state = 99u64;
        let mut now = 0.0f64;
        let mut next_id = 1;
        for _ in 0..5000 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let r = (state >> 33) % 8;
            now += (r as f64) * 7.5;
            if r < 4 {
                let ra = a.push(pkt(next_id), 100, now);
                let rm = m.push(pkt(next_id), 100, now);
                assert_eq!(ra, rm);
                next_id += 1;
            } else {
                let got = a.pop_due(now);
                let want = match m.front_time() {
                    Some(t) if t <= now => m.pop(),
                    _ => None,
                };
                assert_eq!(got, want, "reordering={reordering}");
            }
            assert_eq!(a.len(), m.len());
            assert_eq!(a.front_time(), m.front_time());
        }
    }
}

#[test]
fn peek_nth_on_an_empty_buffer_is_none() {
    for reordering in [false, true] {
        let b = Buffer::new(config(4, reordering, 0.0));
        assert_eq!(b.peek_nth(0), None);
        assert_eq!(b.peek_nth(1), None);
        assert_eq!(b.peek_nth(usize::MAX), None);
    }
}

#[test]
fn fifo_peek_nth_returns_the_nth_queued_entry_without_popping() {
    let mut b = Buffer::new(config(8, false, 100.0));
    for i in 1..=5 {
        assert!(b.push(pkt(i), 100, i as f64));
    }
    for n in 0..5 {
        assert_eq!(b.peek_nth(n), Some(pkt(n + 1)), "n = {n}");
    }
    assert_eq!(b.peek_nth(5), None, "one past the end");
    assert_eq!(b.peek_nth(usize::MAX), None);
    assert_eq!(b.len(), 5, "peeking must not remove anything");

    // After a pop the window slides: the old second entry is the head.
    assert_eq!(b.pop(), Some(pkt(1)));
    assert_eq!(b.peek_nth(0), Some(pkt(2)));
    assert_eq!(b.peek_nth(3), Some(pkt(5)));
    assert_eq!(b.peek_nth(4), None);
}

#[test]
fn fifo_peek_nth_predicts_the_pop_order_even_when_send_times_decrease() {
    let mut b = Buffer::new(config(8, false, 0.0));
    for (i, now) in [900.0, 100.0, 500.0].into_iter().enumerate() {
        assert!(b.push(pkt(i + 1), 100, now));
    }
    let predicted: Vec<_> = (0..3).map(|n| b.peek_nth(n)).collect();
    let popped: Vec<_> = (0..3).map(|_| b.pop()).collect();
    assert_eq!(predicted, popped);
}

#[test]
fn fifo_peek_nth_counts_entries_that_are_not_yet_due() {
    // peek_nth ignores send times entirely: it is positional.
    let mut b = Buffer::new(config(8, false, 1_000_000.0));
    assert!(b.push(pkt(1), 100, 0.0));
    assert!(b.push(pkt(2), 100, 0.0));
    assert_eq!(b.pop_due(0.0), None);
    assert_eq!(b.peek_nth(1), Some(pkt(2)));
}

#[test]
fn reordering_peek_nth_is_exact_at_zero_and_sound_beyond() {
    let mut b = Buffer::new(config(8, true, 100.0));
    assert!(b.push(pkt(1), 100, 1000.0)); // due 1100
    assert!(b.push(pkt(2), 100, 0.0)); // due 100 (head)
    assert!(b.push(pkt(3), 100, 500.0)); // due 600
    let order = [pkt(2), pkt(3), pkt(1)];
    while !b.is_empty() {
        let left = b.len();
        assert_eq!(b.peek_nth(0), Some(order[3 - left]), "n == 0 is exact");
        // n > 0 may be None (best effort), but a Some must be right.
        for n in 1..left {
            if let Some(p) = b.peek_nth(n) {
                assert_eq!(p, order[3 - left + n], "n = {n}, len = {left}");
            }
        }
        assert_eq!(b.peek_nth(left), None, "past the end");
        assert_eq!(b.peek_nth(100), None);
        assert_eq!(b.len(), left, "peeking must not remove anything");
        assert_eq!(b.pop(), Some(order[3 - left]));
    }
    assert_eq!(b.peek_nth(0), None);
}

/// Soundness, no side effects and (via `peeking == false`) the baseline for the
/// no-side-effects comparison. Scrambling delays make the pop order differ from
/// the push order. Returns the pop sequence.
fn drive_and_check_peeks(spacer: bool, peeking: bool) -> Vec<usize> {
    let mut cfg = config(64, true, 20_000.0);
    cfg.delayer.distribution = RVDistribution::NormalDistribution;
    cfg.delayer.std_ns = 8_000.0;
    cfg.limiter.activate = spacer;
    cfg.seed = Some(7);
    let mut b = Buffer::new(cfg);
    let mut state = 12345u64;
    let mut rnd = |m: u64| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) % m
    };
    let (mut now, mut next_id, mut popped) = (0.0f64, 1usize, Vec::new());
    for _ in 0..300 {
        now += rnd(3000) as f64;
        for _ in 0..=rnd(20) {
            if b.push(pkt(next_id), 1000, now) {
                next_id += 1;
            }
        }
        // Predictions made during one drain; no push happens in between.
        let mut predicted: Vec<Option<usize>> = Vec::new();
        for _ in 0..=rnd(b.len() as u64) {
            if b.is_empty() {
                break;
            }
            if peeking {
                let (len, front) = (b.len(), b.front_time());
                let step = popped.len();
                predicted.resize(step + 8, None);
                assert!(b.peek_nth(0).is_some(), "n == 0 is exact, spacer={spacer}");
                for n in 0..8 {
                    match b.peek_nth(n) {
                        Some(p) => {
                            assert!(n < len, "Some past the end");
                            let slot = &mut predicted[step + n];
                            assert!(slot.is_none_or(|q| q == p as usize), "inconsistent");
                            *slot = Some(p as usize);
                        }
                        None => assert!(n > 0 && n < len || n >= len),
                    }
                }
                assert_eq!(
                    (b.len(), b.front_time()),
                    (len, front),
                    "peek changed state"
                );
            }
            let got = b.pop().expect("not empty") as usize;
            if let Some(Some(want)) = predicted.get(popped.len()) {
                assert_eq!(got, *want, "spacer={spacer}, pop {}", popped.len());
            }
            popped.push(got);
        }
    }
    popped
}

#[test]
fn reordering_peek_nth_some_matches_later_pops_and_changes_nothing() {
    for spacer in [false, true] {
        let with = drive_and_check_peeks(spacer, true);
        assert!(with.len() > 500, "the scenario must pop plenty");
        assert_eq!(
            with,
            drive_and_check_peeks(spacer, false),
            "spacer={spacer}"
        );
    }
}

#[test]
fn reordering_peek_nth_looks_ahead_within_a_tight_burst() {
    // 64 packets at one instant: send times within 1 ns of each other.
    let mut b = Buffer::new(config(64, true, 1000.0));
    for i in 1..=64 {
        assert!(b.push(pkt(i), 100, 0.0));
    }
    let (mut with_next, mut with_successor) = (0, 0);
    while b.len() > 1 {
        with_successor += 1;
        let next = b.peek_nth(1);
        let head = b.pop();
        if next.is_some() {
            with_next += 1;
            assert_eq!(next, b.peek_nth(0), "look-ahead must be exact");
        }
        assert!(head.is_some());
    }
    assert!(
        with_next * 4 > with_successor * 3,
        "peek_nth(1) known for only {with_next} of {with_successor} pops"
    );
}

#[test]
fn peek_nth_zero_agrees_with_what_pop_returns_in_both_modes() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(8, reordering, 10.0));
        for i in 1..=6 {
            assert!(b.push(pkt(i), 100, ((i * 37) % 11) as f64 * 10.0));
        }
        while !b.is_empty() {
            let peeked = b.peek_nth(0);
            assert_eq!(peeked, b.pop(), "reordering={reordering}");
        }
        assert_eq!(b.peek_nth(0), None);
    }
}
