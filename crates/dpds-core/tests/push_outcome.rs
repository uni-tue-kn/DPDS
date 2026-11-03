//! `Buffer::push_outcome` and `Buffer::pop_due_with_time` (both additive, new
//! API for the drop-cause counters and the delay statistics of `dpds`).
//!
//! # Test-facing API (what these tests need to exist)
//!
//! ```text
//! dpds_core::buffer::PushOutcome            Debug + Clone + Copy + PartialEq + Eq
//!     Accepted { send_time_ns: u64 }
//!     Lost                                   (the loss model dropped it)
//!     LimiterDrop                            (the limiter rejected it)
//!     BufferFull                             (the buffer holds `buffer_capacity` packets)
//! Buffer::push_outcome(&mut self, ptr: *mut c_void, len: u32, now_ns: f64) -> PushOutcome
//! Buffer::pop_due_with_time(&mut self, now_ns: f64) -> Option<(*mut c_void, u64)>
//! ```
//!
//! `Buffer::push` stays, bit-identical to before: it is
//! `matches!(self.push_outcome(..), PushOutcome::Accepted { .. })`.
//!
//! # Semantics pinned here
//!
//! - Evaluation order is unchanged: the limiter is consulted first (and accounts
//!   the packet, also when the packet is lost or the buffer is full afterwards),
//!   then the loss model, then the capacity. The precedence of the *reported*
//!   cause is loss model (`Lost`), else limiter reject (`LimiterDrop`), else
//!   capacity (`BufferFull`): a packet that both the loss model and the limiter
//!   reject is `Lost`.
//! - `Accepted.send_time_ns` is the whole-ns send time that `front_time` reports
//!   for that entry: FIFO mode `(now + delay + limiter offset) as u64`;
//!   reordering mode `key >> 6`, i.e. INCLUDING the 1/64 ns tie offsets (the
//!   65th packet pushed at one instant is reported 1 ns later than the first).
//! - `pop_due_with_time(now)` pops exactly what `pop_due(now)` pops (same
//!   pointer, same condition `front_time <= now`) and also returns that
//!   whole-ns send time (= the `front_time` before the pop).
//!
//! Hand-derived values: `len = 100` is a 104 byte frame for the limiter (the
//! buffer adds 4), i.e. `(104 + 20) * 8 = 992` bit on the wire; `rate_bps = 1e9`
//! is 1 bit/ns, so Spacer offsets are integers; deterministic delays are exact.

use std::collections::HashMap;
use std::ffi::c_void;

use dpds_core::buffer::{Buffer, PushOutcome};
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

const LEN: u32 = 100; // 992 bit on the wire
const BITS: f64 = 992.0;

#[derive(Clone, Copy)]
struct Cfg {
    capacity: usize,
    reordering: bool,
    delay_ns: f64,
    /// `(mode, rate_bps, limit_bit)`; `None` = limiter off.
    limiter: Option<(LimiterMode, f64, f64)>,
    loss: LossModel,
    seed: Option<u64>,
}

impl Cfg {
    fn new(capacity: usize, reordering: bool) -> Cfg {
        Cfg {
            capacity,
            reordering,
            delay_ns: 0.0,
            limiter: None,
            loss: LossModel::Random(0.0),
            seed: Some(7),
        }
    }
    fn delay(mut self, d: f64) -> Cfg {
        self.delay_ns = d;
        self
    }
    fn spacer(mut self, limit_bit: f64) -> Cfg {
        self.limiter = Some((LimiterMode::Spacer, 1e9, limit_bit));
        self
    }
    fn tbf(mut self, limit_bit: f64) -> Cfg {
        self.limiter = Some((LimiterMode::TokenBucketFilter, 1e9, limit_bit));
        self
    }
    fn loss(mut self, l: LossModel) -> Cfg {
        self.loss = l;
        self
    }
    fn build(self) -> Buffer {
        Buffer::new(self.config())
    }
    fn config(self) -> Config {
        let (mode, rate, limit, activate) = match self.limiter {
            Some((m, r, l)) => (m, r, l, true),
            None => (LimiterMode::Spacer, 1e9, 1e9, false),
        };
        Config {
            buffer_capacity: self.capacity,
            reordering: self.reordering,
            delayer: DelayerConfig {
                distribution: RVDistribution::DeterministicDistribution,
                mean_ns: self.delay_ns,
                std_ns: 0.0,
                activate_correlation: false,
                half_life_period_ns: 1.0,
                initialization_rate_pps: 15e6,
            },
            limiter: LimiterConfig {
                mode,
                activate,
                rate_bps: rate,
                limit_bit: limit,
            },
            loss_model: self.loss,
            seed: self.seed,
        }
    }
}

fn pkt(i: usize) -> *mut c_void {
    i as *mut c_void
}

fn accepted(t: u64) -> PushOutcome {
    PushOutcome::Accepted { send_time_ns: t }
}

const MODES: [bool; 2] = [false, true];

// ── each variant in its situation ───────────────────────────────────

#[test]
fn accepted_reports_the_whole_ns_send_time_that_front_time_reports() {
    for reordering in MODES {
        let mut b = Cfg::new(8, reordering).delay(100.0).build();
        // 5.75 + 100 = 105.75 -> whole ns 105
        assert_eq!(b.push_outcome(pkt(1), LEN, 5.75), accepted(105));
        assert_eq!(b.front_time(), Some(105.0), "reordering={reordering}");
        // a later instant: 6.0 + 100
        assert_eq!(b.push_outcome(pkt(2), LEN, 6.0), accepted(106));
        assert_eq!(b.len(), 2);
        // large values stay exact (below 2^53)
        let mut b = Cfg::new(8, reordering).delay(100.0).build();
        assert_eq!(
            b.push_outcome(pkt(1), LEN, 1.0e12),
            accepted(1_000_000_000_100)
        );
    }
}

#[test]
fn accepted_with_zero_delay_reports_the_truncated_now() {
    for reordering in MODES {
        let mut b = Cfg::new(8, reordering).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), accepted(0));
        assert_eq!(b.push_outcome(pkt(2), LEN, 42.99), accepted(42));
    }
}

#[test]
fn lost_is_reported_by_the_loss_model_and_nothing_is_queued() {
    for reordering in MODES {
        let mut b = Cfg::new(8, reordering).loss(LossModel::Random(1.0)).build();
        for i in 0..20 {
            assert_eq!(
                b.push_outcome(pkt(i + 1), LEN, 0.0),
                PushOutcome::Lost,
                "reordering={reordering}"
            );
        }
        assert!(b.is_empty());

        let mut b = Cfg::new(8, reordering).loss(LossModel::Random(0.0)).build();
        for i in 0..8 {
            assert!(matches!(
                b.push_outcome(pkt(i + 1), LEN, 0.0),
                PushOutcome::Accepted { .. }
            ));
        }
    }
}

#[test]
fn limiter_drop_of_the_spacer_and_its_recovery_by_draining() {
    for reordering in MODES {
        // limit 1984 = exactly two 992 bit packets
        let mut b = Cfg::new(64, reordering).spacer(2.0 * BITS).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), accepted(0)); // fill 992
        assert_eq!(b.push_outcome(pkt(2), LEN, 0.0), accepted(992)); // fill 1984 == limit: ok
        assert_eq!(
            b.push_outcome(pkt(3), LEN, 0.0),
            PushOutcome::LimiterDrop,
            "1984 + 992 > 1984, reordering={reordering}"
        );
        assert_eq!(
            b.push_outcome(pkt(4), LEN, 0.0),
            PushOutcome::LimiterDrop,
            "a rejected packet is not added to the fill"
        );
        assert_eq!(b.len(), 2, "a limiter drop is not queued");
        // 992 ns later 992 bit have drained: fill 992, one packet fits again,
        // offset = fill / rate = 992 -> leaves at 992 + 992
        assert_eq!(b.push_outcome(pkt(5), LEN, 992.0), accepted(1984));
        assert_eq!(b.len(), 3);
    }
}

#[test]
fn limiter_drop_of_the_token_bucket_filter_and_its_refill() {
    for reordering in MODES {
        // the TBF starts full: 1984 bit = two packets
        let mut b = Cfg::new(64, reordering).tbf(2.0 * BITS).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), accepted(0)); // 1984 >= 992 -> 992
        assert_eq!(b.push_outcome(pkt(2), LEN, 0.0), accepted(0)); // 992 >= 992 -> 0
        assert_eq!(
            b.push_outcome(pkt(3), LEN, 0.0),
            PushOutcome::LimiterDrop,
            "reordering={reordering}"
        );
        assert_eq!(b.len(), 2);
        // +992 ns refills 992 bit: accepted again, TBF adds no offset
        assert_eq!(b.push_outcome(pkt(4), LEN, 992.0), accepted(992));
    }
}

#[test]
fn buffer_full_at_capacity_and_free_again_after_a_pop() {
    for reordering in MODES {
        let mut b = Cfg::new(2, reordering).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), accepted(0));
        assert_eq!(b.push_outcome(pkt(2), LEN, 0.0), accepted(0));
        assert_eq!(
            b.push_outcome(pkt(3), LEN, 0.0),
            PushOutcome::BufferFull,
            "reordering={reordering}"
        );
        assert_eq!(b.len(), 2, "a full-buffer drop is not queued");
        assert_eq!(b.pop_due(0.0), Some(pkt(1)));
        assert_eq!(b.push_outcome(pkt(4), LEN, 1.0), accepted(1));
    }
}

#[test]
fn a_zero_capacity_buffer_is_always_full() {
    for reordering in MODES {
        let mut b = Cfg::new(0, reordering).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::BufferFull);
        assert!(b.is_empty());
    }
}

// ── precedence: loss, then limiter, then capacity ───────────────────

#[test]
fn precedence_loss_before_limiter_before_capacity() {
    for reordering in MODES {
        // limit 0 rejects everything (0 + 992 > 0), capacity 0 is always full.
        let all_limiter = |loss| Cfg::new(0, reordering).spacer(0.0).loss(loss);

        // loss + limiter + full -> Lost
        let mut b = all_limiter(LossModel::Random(1.0)).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::Lost);
        // limiter + full (no loss) -> LimiterDrop
        let mut b = all_limiter(LossModel::Random(0.0)).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::LimiterDrop);
        // loss + limiter with room -> Lost
        let mut b = Cfg::new(8, reordering)
            .spacer(0.0)
            .loss(LossModel::Random(1.0))
            .build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::Lost);
        // loss + full, limiter off -> Lost
        let mut b = Cfg::new(0, reordering).loss(LossModel::Random(1.0)).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::Lost);
        // limiter fine + full -> BufferFull (see buffer_full_at_capacity...)
        let mut b = Cfg::new(0, reordering).spacer(1e9).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::BufferFull);
    }
}

#[test]
fn a_lost_packet_still_consumes_limiter_budget() {
    // GilbertElliot(p = 1, r = 0, k = 0, h = 1): the chain starts Good and
    // drops (u > k = 0), moves to Bad for good (p = 1, r = 0), where nothing is
    // dropped (u > h = 1 is impossible): exactly the first packet is lost.
    let ge = LossModel::GilbertElliot(1.0, 0.0, 0.0, 1.0, true);
    for reordering in MODES {
        let mut b = Cfg::new(8, reordering).spacer(1e6).loss(ge).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::Lost);
        // the lost packet was accounted by the limiter (fill 992): the next one
        // waits for it, offset 992 (not 0)
        assert_eq!(
            b.push_outcome(pkt(2), LEN, 0.0),
            accepted(992),
            "reordering={reordering}"
        );
        assert_eq!(b.push_outcome(pkt(3), LEN, 0.0), accepted(1984));
        assert_eq!(b.len(), 2);
    }
}

#[test]
fn a_buffer_full_packet_still_consumes_limiter_budget() {
    for reordering in MODES {
        let mut b = Cfg::new(1, reordering).spacer(1e6).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), accepted(0)); // fill 992
        assert_eq!(b.push_outcome(pkt(2), LEN, 0.0), PushOutcome::BufferFull); // fill 1984
        assert_eq!(b.pop_due(0.0), Some(pkt(1)));
        // fill is 1984 (the full-buffer packet counted): offset 1984
        assert_eq!(
            b.push_outcome(pkt(3), LEN, 0.0),
            accepted(1984),
            "reordering={reordering}"
        );
    }
}

// ── send_time_ns == the whole-ns front_time of the entry ────────────

#[test]
fn reordering_ties_shift_the_reported_send_time_like_front_time_does() {
    // 70 packets at the integral instant 10: keys 640 + tie, whole ns
    // (640 + tie) >> 6: ties 0..=63 -> 10, ties 64..=69 -> 11.
    let mut b = Cfg::new(100, true).build();
    for i in 0..70usize {
        let expect = if i < 64 { 10 } else { 11 };
        assert_eq!(
            b.push_outcome(pkt(i + 1), LEN, 10.0),
            accepted(expect),
            "packet {i}"
        );
    }
    // and front_time agrees entry by entry while draining (ties keep entry order)
    for i in 0..70usize {
        let expect = if i < 64 { 10 } else { 11 };
        assert_eq!(b.front_time(), Some(expect as f64), "packet {i}");
        assert_eq!(b.pop_due(11.0), Some(pkt(i + 1)));
    }
}

/// Deterministic xorshift64*, so the tests do not depend on a `rand` version.
struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A busy config: normal-ish jitter through a Uniform delay, an active limiter
/// that overflows, a small buffer and random loss, so all four outcomes occur.
fn busy(mode: LimiterMode, reordering: bool, seed: u64) -> Config {
    Config {
        buffer_capacity: 16,
        reordering,
        delayer: DelayerConfig {
            distribution: RVDistribution::UniformDistribution,
            mean_ns: 3_000.0,
            std_ns: 800.0,
            activate_correlation: false,
            half_life_period_ns: 1.0,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode,
            activate: true,
            rate_bps: 1e10,
            limit_bit: 100_000.0,
        },
        loss_model: LossModel::Random(0.1),
        seed: Some(seed),
    }
}

/// One scripted step: advance time, push 0..=4 packets, then pop what is due.
fn script(seed: u64, steps: usize) -> Vec<(f64, Vec<u32>, f64)> {
    let mut rng = Xs(seed);
    let mut now = 0.0f64;
    (0..steps)
        .map(|_| {
            now += rng.below(300) as f64 + 0.25 * rng.below(4) as f64;
            let lens = (0..rng.below(5))
                .map(|_| 60 + rng.below(1400) as u32)
                .collect();
            // sometimes pop at a later time than the push time, sometimes never
            let pop_at = now + rng.below(6_000) as f64;
            (now, lens, pop_at)
        })
        .collect()
}

#[test]
fn push_is_bit_identical_to_push_outcome_accepted() {
    for mode in [LimiterMode::Spacer, LimiterMode::TokenBucketFilter] {
        for reordering in MODES {
            for seed in [1u64, 2, 3] {
                let cfg = busy(mode, reordering, seed);
                let (mut a, mut b) = (Buffer::new(cfg), Buffer::new(cfg));
                let mut next_ptr = 1usize;
                let mut seen = [0usize; 4]; // accepted, lost, limiter, full
                for (step, (now, lens, pop_at)) in script(seed, 3_000).into_iter().enumerate() {
                    for len in lens {
                        let p = pkt(next_ptr);
                        next_ptr += 1;
                        let via_push = a.push(p, len, now);
                        let outcome = b.push_outcome(p, len, now);
                        assert_eq!(
                            via_push,
                            matches!(outcome, PushOutcome::Accepted { .. }),
                            "step {step} mode {mode:?} reordering {reordering} seed {seed}"
                        );
                        seen[match outcome {
                            PushOutcome::Accepted { .. } => 0,
                            PushOutcome::Lost => 1,
                            PushOutcome::LimiterDrop => 2,
                            PushOutcome::BufferFull => 3,
                        }] += 1;
                        assert_eq!(a.len(), b.len());
                        assert_eq!(a.front_time(), b.front_time());
                    }
                    // identical pops, whatever the order
                    while a.front_time().is_some_and(|t| t <= pop_at) {
                        assert_eq!(a.pop_due(pop_at), b.pop_due(pop_at), "step {step}");
                    }
                    assert_eq!(b.pop_due(pop_at), None);
                    assert_eq!(a.len(), b.len());
                }
                let [acc, lost, lim, full] = seen;
                assert!(
                    acc > 0 && lost > 0 && lim > 0,
                    "the sequence must exercise accepted/lost/limiter outcomes: {seen:?} ({mode:?})"
                );
                if matches!(mode, LimiterMode::Spacer) {
                    // The Spacer accounts packets the buffer then refuses, so its
                    // fill passes the limit eventually and the buffer fills first.
                    assert!(full > 0, "the sequence must also hit BufferFull: {seen:?}");
                }
            }
        }
    }
}

#[test]
fn accepted_send_time_is_what_pop_due_with_time_and_front_time_report() {
    for mode in [LimiterMode::Spacer, LimiterMode::TokenBucketFilter] {
        for reordering in MODES {
            let cfg = busy(mode, reordering, 99);
            let mut buf = Buffer::new(cfg);
            let mut expected: HashMap<usize, u64> = HashMap::new();
            let mut next_ptr = 1usize;
            let mut popped = 0usize;
            for (now, lens, pop_at) in script(99, 2_000) {
                for len in lens {
                    let p = pkt(next_ptr);
                    next_ptr += 1;
                    if let PushOutcome::Accepted { send_time_ns } = buf.push_outcome(p, len, now) {
                        expected.insert(p as usize, send_time_ns);
                    }
                }
                let mut last_time = 0u64; // per drain: later pushes may jitter earlier
                while let Some(front) = buf.front_time() {
                    let before = buf.len();
                    let Some((p, t)) = buf.pop_due_with_time(pop_at) else {
                        assert!(front > pop_at, "not due, so nothing may be popped");
                        assert_eq!(buf.len(), before, "a refused pop changes nothing");
                        break;
                    };
                    assert_eq!(t as f64, front, "the time is the front_time before the pop");
                    assert_eq!(
                        Some(&t),
                        expected.get(&(p as usize)),
                        "pop time == Accepted.send_time_ns of that packet"
                    );
                    assert!(t as f64 <= pop_at);
                    if reordering {
                        assert!(t >= last_time, "heap pops in non-decreasing send time");
                    }
                    last_time = t;
                    popped += 1;
                }
            }
            assert!(popped > 100, "the sequence popped something: {popped}");
        }
    }
}

// ── pop_due_with_time ───────────────────────────────────────────────

#[test]
fn pop_due_with_time_on_an_empty_buffer_is_none() {
    for reordering in MODES {
        let mut b = Cfg::new(4, reordering).build();
        assert_eq!(b.pop_due_with_time(0.0), None);
        assert_eq!(b.pop_due_with_time(f64::INFINITY), None);
    }
}

#[test]
fn pop_due_with_time_pops_exactly_at_the_send_time_and_returns_it() {
    for reordering in MODES {
        let mut b = Cfg::new(4, reordering).delay(100.0).build();
        assert!(b.push(pkt(1), LEN, 5.75)); // front_time 105
        assert_eq!(b.pop_due_with_time(104.99), None, "reordering={reordering}");
        assert_eq!(b.len(), 1, "a refused pop removes nothing");
        assert_eq!(b.pop_due_with_time(105.0), Some((pkt(1), 105)), "`<=` pops");
        assert!(b.is_empty());
        assert_eq!(b.pop_due_with_time(1e9), None);
    }
}

#[test]
fn pop_due_with_time_returns_the_limiter_shifted_time() {
    for reordering in MODES {
        let mut b = Cfg::new(8, reordering).spacer(1e6).build();
        for i in 0..3 {
            assert!(b.push(pkt(i + 1), LEN, 0.0)); // send times 0, 992, 1984
        }
        assert_eq!(b.pop_due_with_time(0.0), Some((pkt(1), 0)));
        assert_eq!(b.pop_due_with_time(991.0), None);
        assert_eq!(b.pop_due_with_time(5_000.0), Some((pkt(2), 992)));
        assert_eq!(b.pop_due_with_time(5_000.0), Some((pkt(3), 1984)));
    }
}

#[test]
fn pop_due_with_time_pops_the_same_pointers_in_the_same_order_as_pop_due() {
    for mode in [LimiterMode::Spacer, LimiterMode::TokenBucketFilter] {
        for reordering in MODES {
            let cfg = busy(mode, reordering, 5);
            let (mut a, mut b) = (Buffer::new(cfg), Buffer::new(cfg));
            let mut next_ptr = 1usize;
            let mut popped = 0usize;
            for (now, lens, pop_at) in script(5, 3_000) {
                for len in lens {
                    let p = pkt(next_ptr);
                    next_ptr += 1;
                    assert_eq!(a.push(p, len, now), b.push(p, len, now));
                }
                loop {
                    let front = b.front_time();
                    let x = a.pop_due(pop_at);
                    let y = b.pop_due_with_time(pop_at);
                    assert_eq!(x, y.map(|(p, _)| p), "{mode:?} reordering={reordering}");
                    match (y, front) {
                        (Some((_, t)), Some(f)) => assert_eq!(t as f64, f),
                        (None, _) => break,
                        (Some(_), None) => panic!("popped from an empty buffer"),
                    }
                    popped += 1;
                }
                assert_eq!(a.len(), b.len());
            }
            assert!(popped > 100, "{popped}");
        }
    }
}
