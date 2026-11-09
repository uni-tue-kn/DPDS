//! `Buffer::push_outcome` and `Buffer::pop_due_with_time`.
//!
//! - Evaluation order: the capacity check comes first, so a packet that finds
//!   the buffer full is `BufferFull` and touches nothing else; then the delay
//!   sample, the limiter accounting (also when the packet is lost afterwards),
//!   the loss model, the limiter verdict, the enqueue. The reported cause is
//!   `BufferFull`, else `Lost`, else `LimiterDrop`. (`tests/push_order.rs` pins
//!   that a full-buffer push leaves delayer, limiter and dropper state untouched.)
//! - The limiter's limit is derived (`buffer_capacity * 1542 * 8` bit), see
//!   `tests/derived_limit.rs`. A packet of `len = 1518` is exactly
//!   `1542 * 8 = 12336` bit on the wire, `len = 1519` is 8 bit more.
//! - `Accepted.send_time_ns` is the whole-ns send time that `front_time` reports
//!   for that entry: FIFO mode `(now + delay + limiter offset) as u64`;
//!   reordering mode `key >> 6`, i.e. including the 1/64 ns tie offsets (the
//!   65th packet pushed at one instant is reported 1 ns later than the first).
//! - `pop_due_with_time(now)` pops exactly what `pop_due(now)` pops and also
//!   returns that whole-ns send time (the `front_time` before the pop).
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
/// (1518 + 4 + 20) * 8 = 12336 = 1542 * 8 bit on the wire
const LEN_FULL: u32 = 1518;

#[derive(Clone, Copy)]
struct Cfg {
    capacity: usize,
    reordering: bool,
    delay_ns: f64,
    /// `(mode, rate_bps)`; `None` = limiter off. The limit is derived.
    limiter: Option<(LimiterMode, f64)>,
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
    fn spacer(mut self) -> Cfg {
        self.limiter = Some((LimiterMode::Spacer, 1e9));
        self
    }
    fn tbf(mut self) -> Cfg {
        self.limiter = Some((LimiterMode::TokenBucketFilter, 1e9));
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
        let (mode, rate, activate) = match self.limiter {
            Some((m, r)) => (m, r, true),
            None => (LimiterMode::Spacer, 1e9, false),
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
    // FIFO only: the reordering Spacer has no limit and reports the heap time (reordering_spacer.rs).
    // capacity 2: derived limit 2 * 12336 = exactly two len-1518 packets
    let mut b = Cfg::new(2, false).spacer().build();
    assert_eq!(b.push_outcome(pkt(1), LEN_FULL, 0.0), accepted(0)); // fill 12336
    assert_eq!(b.pop_due(0.0), Some(pkt(1))); // the buffer keeps room
    assert_eq!(b.push_outcome(pkt(2), LEN_FULL, 0.0), accepted(12336)); // fill 24672 == limit: ok
    assert_eq!(
        b.push_outcome(pkt(3), LEN_FULL, 0.0),
        PushOutcome::LimiterDrop,
        "24672 + 12336 > 24672"
    );
    assert_eq!(
        b.push_outcome(pkt(4), LEN_FULL, 0.0),
        PushOutcome::LimiterDrop,
        "a rejected packet is not added to the fill"
    );
    assert_eq!(b.len(), 1, "a limiter drop is not queued");
    // 12336 ns later 12336 bit have drained: fill 12336, one packet fits
    // again, offset = fill / rate = 12336 -> leaves at 12336 + 12336
    assert_eq!(b.push_outcome(pkt(5), LEN_FULL, 12_336.0), accepted(24_672));
    assert_eq!(b.len(), 2);
}

#[test]
fn limiter_drop_of_the_token_bucket_filter_and_its_refill() {
    for reordering in MODES {
        // capacity 2: the TBF starts full with the derived 24672 bit = two len-1518 packets
        let mut b = Cfg::new(2, reordering).tbf().build();
        assert_eq!(b.push_outcome(pkt(1), LEN_FULL, 0.0), accepted(0)); // 24672 >= 12336 -> 12336
        assert_eq!(b.pop_due(0.0), Some(pkt(1)));
        assert_eq!(b.push_outcome(pkt(2), LEN_FULL, 0.0), accepted(0)); // 12336 >= 12336 -> 0
        assert_eq!(
            b.push_outcome(pkt(3), LEN_FULL, 0.0),
            PushOutcome::LimiterDrop,
            "reordering={reordering}"
        );
        assert_eq!(b.len(), 1);
        // +12336 ns refills 12336 bit: accepted again, TBF adds no offset
        assert_eq!(b.push_outcome(pkt(4), LEN_FULL, 12_336.0), accepted(12_336));
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

// ── precedence: capacity, then loss, then limiter ───────────────────

/// Longer than the derived limit of a capacity-8 buffer (8 * 12336 = 98688 bit):
/// (12400 + 4 + 20) * 8 = 99392 bit. The Spacer and the TBF both reject it.
const LEN_OVER_LIMIT_CAP8: u32 = 12_400;

#[test]
fn precedence_buffer_full_before_loss_before_limiter() {
    for reordering in MODES {
        for tbf in [false, true] {
            if reordering && !tbf {
                continue; // reordering + Spacer is the two-stage mode (reordering_spacer.rs): no limit, send time known only at pop
            }
            let with_limiter = |c: Cfg| if tbf { c.tbf() } else { c.spacer() };

            // sanity: without loss the oversized packet is a limiter drop
            let mut b = with_limiter(Cfg::new(8, reordering)).build();
            assert_eq!(
                b.push_outcome(pkt(1), LEN_OVER_LIMIT_CAP8, 0.0),
                PushOutcome::LimiterDrop,
                "tbf={tbf} reordering={reordering}"
            );
            // loss + limiter reject, with room -> Lost
            let mut b = with_limiter(Cfg::new(8, reordering))
                .loss(LossModel::Random(1.0))
                .build();
            assert_eq!(
                b.push_outcome(pkt(1), LEN_OVER_LIMIT_CAP8, 0.0),
                PushOutcome::Lost,
                "tbf={tbf} reordering={reordering}"
            );
            // capacity 0 is always full: loss and limiter never get a say
            let mut b = with_limiter(Cfg::new(0, reordering))
                .loss(LossModel::Random(1.0))
                .build();
            assert_eq!(
                b.push_outcome(pkt(1), LEN_OVER_LIMIT_CAP8, 0.0),
                PushOutcome::BufferFull,
                "full + loss + limiter reject: tbf={tbf} reordering={reordering}"
            );
            let mut b = with_limiter(Cfg::new(0, reordering)).build();
            assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::BufferFull);
        }
        // loss + full, limiter off -> BufferFull (not Lost)
        let mut b = Cfg::new(0, reordering).loss(LossModel::Random(1.0)).build();
        assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::BufferFull);

        // a non-empty full buffer: the next packet would be lost AND rejected
        // by the limiter, but it is BufferFull. Alternating GE (p=r=1, k=1,
        // h=0): Good keeps, Bad drops.
        let ge = LossModel::GilbertElliot(1.0, 1.0, 1.0, 0.0, true);
        let limited = if reordering {
            Cfg::new(1, reordering).tbf() // reordering + Spacer is the two-stage mode (reordering_spacer.rs): no limit, send time known only at pop
        } else {
            Cfg::new(1, reordering).spacer()
        };
        let mut b = limited.loss(ge).build();
        assert!(matches!(
            b.push_outcome(pkt(1), LEN, 0.0),
            PushOutcome::Accepted { .. }
        ));
        assert_eq!(
            b.push_outcome(pkt(2), 1519, 0.0),
            PushOutcome::BufferFull,
            "reordering={reordering}"
        );
    }
}

#[test]
fn a_lost_packet_still_consumes_limiter_budget() {
    // GilbertElliot(p = 1, r = 0, k = 0, h = 1): the chain starts Good and
    // drops (u > k = 0), moves to Bad for good (p = 1, r = 0), where nothing is
    // dropped (u > h = 1 is impossible): exactly the first packet is lost.
    let ge = LossModel::GilbertElliot(1.0, 0.0, 0.0, 1.0, true);
    // FIFO only: the reordering Spacer has no limit and reports the heap time (reordering_spacer.rs).
    let mut b = Cfg::new(8, false).spacer().loss(ge).build();
    assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::Lost);
    // the lost packet was accounted by the limiter (fill 992): the next one
    // waits for it, offset 992 (not 0)
    assert_eq!(b.push_outcome(pkt(2), LEN, 0.0), accepted(992), "fifo");
    assert_eq!(b.push_outcome(pkt(3), LEN, 0.0), accepted(1984));
    assert_eq!(b.len(), 2);
}

#[test]
fn a_buffer_full_packet_does_not_consume_limiter_budget() {
    // FIFO only: the reordering Spacer has no limit and reports the heap time (reordering_spacer.rs).
    let mut b = Cfg::new(1, false).spacer().build();
    assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), accepted(0)); // fill 992
    assert_eq!(b.push_outcome(pkt(2), LEN, 0.0), PushOutcome::BufferFull); // fill stays 992
    assert_eq!(b.pop_due(0.0), Some(pkt(1)));
    // the full-buffer packet was not accounted: offset 992, not 1984
    assert_eq!(b.push_outcome(pkt(3), LEN, 0.0), accepted(992), "fifo");
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
                .map(|_| 1400 + rng.below(119) as u32) // large frames: lost packets push the Spacer's fill to the derived limit
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
            if reordering && matches!(mode, LimiterMode::Spacer) {
                continue; // reordering + Spacer is the two-stage mode (reordering_spacer.rs): no limit, send time known only at pop
            }
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
                    // Large frames (up to 1518 B = one 12336 bit slot of the derived
                    // limit): the Spacer's fill also counts lost packets, so the
                    // limit binds before the buffer (capacity 16) has filled, and
                    // the buffer fills at other moments.
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
            if reordering && matches!(mode, LimiterMode::Spacer) {
                continue; // reordering + Spacer is the two-stage mode (reordering_spacer.rs): no limit, send time known only at pop
            }
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
    // FIFO only: the reordering Spacer has no limit and reports the heap time (reordering_spacer.rs).
    let mut b = Cfg::new(8, false).spacer().build();
    for i in 0..3 {
        assert!(b.push(pkt(i + 1), LEN, 0.0)); // send times 0, 992, 1984
    }
    assert_eq!(b.pop_due_with_time(0.0), Some((pkt(1), 0)));
    assert_eq!(b.pop_due_with_time(991.0), None);
    assert_eq!(b.pop_due_with_time(5_000.0), Some((pkt(2), 992)));
    assert_eq!(b.pop_due_with_time(5_000.0), Some((pkt(3), 1984)));
}

#[test]
fn pop_due_with_time_pops_the_same_pointers_in_the_same_order_as_pop_due() {
    for mode in [LimiterMode::Spacer, LimiterMode::TokenBucketFilter] {
        for reordering in MODES {
            if reordering && matches!(mode, LimiterMode::Spacer) {
                continue; // reordering + Spacer is the two-stage mode (reordering_spacer.rs): no limit, send time known only at pop
            }
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
