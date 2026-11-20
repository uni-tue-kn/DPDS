//! Reordering mode keeps entry order for equal send times.
//!
//! The heap key (hand-derived below) is
//! `key = floor(t * 64) + tie` (`TIE_FRAC_BITS = 6`, 1/64 ns resolution), with
//! `t = now + delay (+ limiter offset)` before truncation and `tie` the number
//! of accepted pushes since the last push with a different `current_time_ns`
//! (0 for the first packet of an instant). `front_time()` is `(key >> 6) as
//! f64`, `pop`/`pop_due` order by `key`. Saturating arithmetic, no overflow.
//!
//! Constants used in the expectations: `105.75 * 64 = 6768 = 105 * 64 + 48`,
//! `105.0 * 64 = 6720`, `100.0 * 64 = 6400`; all instants/delays are exact
//! binary fractions so `t * 64` is exact.

use std::collections::BTreeSet;
use std::ffi::c_void;

use dpds_core::buffer::{Buffer, PushOutcome};
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

// ---- Edge cases for time values far from "now-ish" ----

#[test]
fn far_future_send_times_wait_while_near_term_packets_leave_in_order() {
    // Delay 0. 1e18 ns saturates the key: it reports u64::MAX >> 6, which is
    // still later than every other entry.
    let saturated = (u64::MAX >> 6) as f64;
    let hour = 3.6e12;
    let mut b = Buffer::new(config(16, true, 0.0));
    for (id, at) in [(1, 10.0), (2, hour), (3, 1e18), (4, 30.0), (5, 5.0)] {
        assert!(b.push(pkt(id), 100, at));
    }
    assert_eq!(b.front_time(), Some(5.0));
    assert_eq!(b.pop_due(1e6), Some(pkt(5)));
    assert_eq!(b.pop_due(1e6), Some(pkt(1)));
    assert_eq!(b.pop_due(1e6), Some(pkt(4)));
    assert_eq!(b.pop_due(1e6), None);
    assert_eq!(b.front_time(), Some(hour));
    assert_eq!(b.pop_due(hour - 1.0), None);
    assert_eq!(b.pop_due(hour), Some(pkt(2)));
    assert_eq!(b.front_time(), Some(saturated));
    assert_eq!(b.pop_due(saturated / 2.0), None);
    assert_eq!(b.pop_due(1e18), Some(pkt(3)));
    assert!(b.is_empty());
}

#[test]
fn a_packet_earlier_than_one_already_popped_is_still_served_first() {
    let mut b = Buffer::new(config(8, true, 0.0));
    assert!(b.push(pkt(1), 100, 1000.0));
    assert_eq!(b.pop_due(1000.0), Some(pkt(1)));
    assert!(b.push(pkt(2), 100, 2000.0));
    assert!(b.push(pkt(3), 100, 10.0)); // earlier than the popped packet 1
    assert_eq!(b.front_time(), Some(10.0));
    assert_eq!(b.pop_due(10.0), Some(pkt(3)));
    assert_eq!(b.pop_due(1999.0), None);
    assert_eq!(b.pop_due(2000.0), Some(pkt(2)));
}

#[test]
fn pop_due_jumping_far_over_empty_periods_releases_one_packet_per_call_in_order() {
    let mut b = Buffer::new(config(16, true, 0.0));
    for (id, at) in [(1, 5.0), (2, 1e6), (3, 1e9), (4, 6e10), (5, 1e12)] {
        assert!(b.push(pkt(id), 100, at));
    }
    assert_eq!(b.pop_due(0.0), None);
    assert_eq!(b.pop_due(10.0), Some(pkt(1)));
    // From 10 ns straight to 1e11 ns: three entries are due, 1e12 is not.
    assert_eq!(b.pop_due(1e11), Some(pkt(2)));
    assert_eq!(b.pop_due(1e11), Some(pkt(3)));
    assert_eq!(b.pop_due(1e11), Some(pkt(4)));
    assert_eq!(b.pop_due(1e11), None);
    // A packet between the jumped-over period and the head, pushed afterwards.
    assert!(b.push(pkt(6), 100, 2e11));
    assert_eq!(b.pop_due(1e13), Some(pkt(6)));
    assert_eq!(b.pop_due(1e13), Some(pkt(5)));
    assert_eq!(b.pop_due(1e13), None);
}

#[test]
fn a_buffer_filled_to_capacity_and_drained_can_be_reused() {
    const CAP: usize = 64;
    let mut b = Buffer::new(config(CAP, true, 0.0));
    // Round 1: descending times; round 2: a shuffle that reaches both before
    // round 1's earliest time and far past its latest.
    let rounds = [
        (0..CAP)
            .map(|i| 5000.0 - 10.0 * i as f64)
            .collect::<Vec<_>>(),
        (0..CAP)
            .map(|i| match i % 3 {
                0 => (i * 7 % CAP) as f64,
                1 => 1e12 + (i * 13 % CAP) as f64,
                _ => 3000.0 + (i * 5 % CAP) as f64,
            })
            .collect::<Vec<_>>(),
    ];
    for times in rounds {
        for (i, &t) in times.iter().enumerate() {
            assert!(b.push(pkt(i + 1), 100, t));
        }
        assert_eq!(b.len(), CAP);
        assert!(!b.push(pkt(999), 100, 0.0), "full buffer must reject");
        let order = drain(&mut b);
        assert!(b.is_empty());
        assert_eq!(b.front_time(), None);
        let popped: Vec<f64> = order.iter().map(|&id| times[id - 1]).collect();
        assert!(popped.windows(2).all(|w| w[0] <= w[1]), "{popped:?}");
        assert_eq!(order.len(), CAP);
    }
}

// ---- Randomized differential tests against a reference model ----

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A multiple of 1/64 below `n` ns, so sums with it stay exact in f64.
    fn ns(&mut self, n: u64) -> f64 {
        self.below(n * 64) as f64 / 64.0
    }
}

/// Push instants and pop clocks: mostly tiny steps, sometimes microsecond and
/// minute jumps, back in time, or anywhere in the first ~5 hours.
struct Script {
    rng: Rng,
    now: f64,
}

impl Script {
    fn next_instant(&mut self) -> f64 {
        let r = &mut self.rng;
        self.now = match r.below(100) {
            0..=49 => self.now + r.below(4) as f64 / 64.0,
            50..=74 => self.now + r.ns(10_000),
            75..=84 => self.now + r.ns(60_000_000_000),
            85..=92 => (self.now - r.ns(self.now as u64 + 1)).max(0.0),
            _ => r.ns(1 << 44),
        };
        self.now
    }

    /// The `now` of a drain: exactly the head's time, the last push instant,
    /// a bit past it, or far past it.
    fn next_clock(&mut self, head: Option<f64>) -> f64 {
        match self.rng.below(5) {
            0 | 1 => head.unwrap_or(self.now),
            2 => self.now,
            3 => self.now + 1e4,
            _ => self.now + 6e10,
        }
    }
}

struct Entry {
    /// Order key of the reference: the exact key, or the whole-ns time.
    key: u64,
    /// The send time `push_outcome` reported.
    reported: u64,
    /// Arrival sequence number (accepted pushes only).
    seq: u64,
}

/// Sorted reference model of the reordering buffer.
#[derive(Default)]
struct Model {
    entries: Vec<Entry>,
    order: BTreeSet<(u64, usize)>,
    popped: Vec<bool>,
    popped_seq_end: u64,
    out_of_order: u64,
}

impl Model {
    fn accept(&mut self, key: u64, reported: u64) {
        let id = self.entries.len();
        let seq = id as u64;
        self.entries.push(Entry { key, reported, seq });
        self.order.insert((key, id));
        self.popped.push(false);
    }

    fn head(&self) -> Option<&Entry> {
        self.order.first().map(|&(_, id)| &self.entries[id])
    }

    /// Checks a popped packet is one with the smallest key and removes it.
    fn pop(&mut self, p: *mut c_void, time: Option<u64>) {
        let id = p as usize - 1;
        let e = &self.entries[id];
        assert_eq!(Some(e.key), self.head().map(|h| h.key), "wrong packet {id}");
        assert!(self.order.remove(&(e.key, id)));
        assert_eq!(time.unwrap_or(e.reported), e.reported);
        self.popped[id] = true;
        if e.seq < self.popped_seq_end {
            self.out_of_order += 1;
        } else {
            self.popped_seq_end = e.seq + 1;
        }
    }
}

/// Random pushes (bursts at one instant), `pop_due` drains and plain pops,
/// checked against `Model` after every operation. With `exact_delay` (the
/// config's deterministic delay) the model computes the keys itself, so the
/// order within an instant and within a nanosecond is pinned too; otherwise it
/// only knows the reported whole-ns times (equal times may leave either way).
fn differential(cfg: Config, seed: u64, ops: usize, exact_delay: Option<f64>) {
    let capacity = cfg.buffer_capacity;
    let mut b = Buffer::new(cfg);
    let mut s = Script {
        rng: Rng(seed),
        now: 0.0,
    };
    let mut m = Model::default();
    let (mut tie_instant, mut tie) = (f64::NAN, 0u64);
    for _ in 0..ops {
        match s.rng.below(100) {
            0..=54 => {
                let now = s.next_instant();
                for _ in 0..1 + s.rng.below(5) {
                    let full = b.len() == capacity;
                    let id = m.entries.len();
                    match b.push_outcome(pkt(id + 1), 100, now) {
                        PushOutcome::Accepted { send_time_ns } => {
                            assert!(!full);
                            let key = exact_delay.map_or(send_time_ns, |d| {
                                if now != tie_instant {
                                    (tie_instant, tie) = (now, 0);
                                }
                                tie += 1;
                                (((now + d) * 64.0) as u64) + tie - 1
                            });
                            assert_eq!(exact_delay.map_or(key, |_| key >> 6), send_time_ns);
                            m.accept(key, send_time_ns);
                        }
                        PushOutcome::BufferFull => assert!(full),
                        other => panic!("unexpected {other:?}"),
                    }
                }
            }
            55..=89 => {
                let clock = s.next_clock(b.front_time());
                loop {
                    let due = m.head().is_some_and(|e| e.reported as f64 <= clock);
                    match b.pop_due_with_time(clock) {
                        Some((p, t)) => {
                            assert!(due);
                            m.pop(p, Some(t));
                        }
                        None => {
                            assert!(!due);
                            break;
                        }
                    }
                }
            }
            _ => match b.pop() {
                Some(p) => m.pop(p, None),
                None => assert!(m.order.is_empty()),
            },
        }
        assert_eq!(b.len(), m.order.len());
        assert_eq!(b.front_time(), m.head().map(|e| e.reported as f64));
        let head_key = b.peek_nth(0).map(|p| m.entries[p as usize - 1].key);
        assert_eq!(head_key, m.head().map(|e| e.key));
    }
    while let Some(p) = b.pop() {
        m.pop(p, None);
    }
    assert!(m.popped.iter().all(|&p| p), "a packet was never popped");
    assert!(b.is_empty());
    assert_eq!(b.out_of_order(), m.out_of_order);
}

fn seeded(cfg: Config, distribution: RVDistribution, mean_ns: f64, std_ns: f64) -> Config {
    Config {
        seed: Some(11),
        delayer: DelayerConfig {
            distribution,
            mean_ns,
            std_ns,
            ..cfg.delayer
        },
        ..cfg
    }
}

#[test]
fn random_operations_match_the_exact_key_model() {
    let cfg = Config {
        seed: Some(3),
        ..config(300, true, 100.0)
    };
    differential(cfg, 1, 30_000, Some(100.0));
}

#[test]
fn random_operations_with_scrambling_normal_delays_match_the_reported_times() {
    let cfg = seeded(
        config(300, true, 0.0),
        RVDistribution::NormalDistribution,
        1000.0,
        300.0,
    );
    differential(cfg, 2, 30_000, None);
}

#[test]
fn random_operations_with_delays_from_ns_to_seconds_match_the_reported_times() {
    // Log-normal with a coefficient of variation of 100: the delays span many
    // orders of magnitude, on top of push instants that jump by minutes.
    let cfg = seeded(
        config(300, true, 0.0),
        RVDistribution::LogNormalDistribution,
        1e6,
        1e8,
    );
    differential(cfg, 3, 30_000, None);
}

#[test]
fn random_operations_with_a_spacer_release_every_packet_once_in_time_order() {
    let mut cfg = seeded(
        config(300, true, 0.0),
        RVDistribution::NormalDistribution,
        5000.0,
        1000.0,
    );
    cfg.limiter.activate = true;
    let mut b = Buffer::new(cfg);
    let mut s = Script {
        rng: Rng(4),
        now: 0.0,
    };
    let (mut pushed, mut popped, mut last) = (0usize, 0usize, 0.0f64);
    let mut seen = Vec::new();
    for _ in 0..30_000 {
        match s.rng.below(100) {
            0..=54 => {
                let now = s.next_instant();
                for _ in 0..1 + s.rng.below(5) {
                    let full = b.len() == 300;
                    let accepted = b.push(pkt(pushed + 1), 1000, now);
                    assert_eq!(accepted, !full);
                    if accepted {
                        pushed += 1;
                        seen.push(false);
                    }
                }
            }
            55..=89 => {
                let clock = s.next_clock(b.front_time());
                while let Some((p, t)) = b.pop_due_with_time(clock) {
                    assert!(t as f64 <= clock && t as f64 >= last);
                    last = t as f64;
                    assert!(!std::mem::replace(&mut seen[p as usize - 1], true));
                    popped += 1;
                }
                assert!(
                    b.front_time().is_none_or(|f| f > clock),
                    "something due is left"
                );
            }
            _ => {
                let front = b.front_time();
                if let Some(p) = b.pop() {
                    let f = front.expect("non-empty buffer has a front time");
                    assert!(f >= last);
                    last = f;
                    assert!(!std::mem::replace(&mut seen[p as usize - 1], true));
                    popped += 1;
                }
            }
        }
        assert_eq!(b.len(), pushed - popped);
    }
    while let Some(p) = b.pop() {
        assert!(!std::mem::replace(&mut seen[p as usize - 1], true));
        popped += 1;
    }
    assert!(popped == pushed && seen.iter().all(|&s| s));
}
