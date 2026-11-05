//! Evaluation order of `Buffer::push_outcome` (spec20): the buffer-room check
//! comes first. A packet that finds the buffer full is `BufferFull` and takes
//! no part in the emulation: no delay sample, no limiter accounting, no loss
//! draw. Otherwise the order is delay, limiter accounting, loss draw (`Lost`),
//! limiter verdict (`LimiterDrop`), enqueue.
//!
//! # How "untouched" is proven
//!
//! The delayer RNG and correlation state, the limiter's fill state and the
//! dropper's RNG and Markov state are private, so they are observed through
//! their effect on later pushes. Two identically configured, identically seeded
//! buffers get exactly the same sequence of pushes and pops, except that the
//! second one also receives a few pushes while it is full. Afterwards both
//! receive the same tail of pushes, and the complete outcome sequence of the tail
//! (including every `Accepted.send_time_ns`) must be identical. If the full-buffer
//! pushes had consumed a delay sample or a loss draw, advanced the correlation
//! estimator, or accounted bits in the limiter, the tail would differ.
//!
//! A control keeps this from being vacuous: when the same extra pushes find
//! room in the buffer (and are popped again), the tail does change, so the tail
//! is sensitive to each of these states.
//!
//! Each of the three states is isolated in one scenario (only the delayer is
//! stochastic, only the limiter is active, only the dropper is stochastic), and
//! one scenario switches everything on. All run in FIFO and reordering mode, except those with the
//! correlated delayer or the Spacer, which run in FIFO mode only (spec 45: reordering excludes them).

use std::ffi::c_void;

use dpds_core::buffer::{Buffer, PushOutcome};
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

const CAPACITY: usize = 4;
const EXTRA: usize = 7;
const TAIL: usize = 300;

#[derive(Clone, Copy)]
enum Delay {
    Fixed,
    /// Uniform jitter: an RNG draw per sample.
    Jitter,
    /// Normal jitter, correlated: RNG draw, EMA state and rate estimator.
    Correlated,
}

#[derive(Clone, Copy)]
struct Scenario {
    reordering: bool,
    delay: Delay,
    limiter: Option<LimiterMode>,
    loss: LossModel,
    /// ns between the extra pushes (they start at the end of the fill phase).
    extra_gap_ns: f64,
    /// ns between the tail pushes.
    tail_step_ns: f64,
    /// ns between the last extra push and the start of the tail.
    tail_pad_ns: f64,
}

impl Scenario {
    fn base(reordering: bool) -> Scenario {
        Scenario {
            reordering,
            delay: Delay::Fixed,
            limiter: None,
            loss: LossModel::Random(0.0),
            extra_gap_ns: 100.0,
            tail_step_ns: 100.0,
            tail_pad_ns: 100.0,
        }
    }

    fn config(self) -> Config {
        let (distribution, mean_ns, std_ns, activate_correlation) = match self.delay {
            Delay::Fixed => (RVDistribution::DeterministicDistribution, 500.0, 0.0, false),
            Delay::Jitter => (RVDistribution::UniformDistribution, 3_000.0, 800.0, false),
            Delay::Correlated => (RVDistribution::NormalDistribution, 5_000.0, 1_500.0, true),
        };
        let (mode, activate) = match self.limiter {
            Some(m) => (m, true),
            None => (LimiterMode::Spacer, false),
        };
        Config {
            buffer_capacity: CAPACITY,
            reordering: self.reordering,
            delayer: DelayerConfig {
                distribution,
                mean_ns,
                std_ns,
                activate_correlation,
                half_life_period_ns: 4_000.0,
                initialization_rate_pps: 1e6,
            },
            limiter: LimiterConfig {
                mode,
                activate,
                rate_bps: 1e9,
            },
            loss_model: self.loss,
            seed: Some(1234),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Extra {
    /// Nothing between the fill and the tail.
    None,
    /// `EXTRA` pushes into the full buffer.
    WhileFull,
    /// Control: the same pushes, but after the buffer was drained, so they find
    /// room; each is popped again right away.
    WithRoom,
}

fn pkt(i: usize) -> *mut c_void {
    i as *mut c_void
}

fn drain(b: &mut Buffer) {
    while b.pop().is_some() {}
}

/// Fill phase (small packets), the extra pushes, then the tail (large packets).
/// Returns the outcomes of the fill phase and of the tail.
fn trace(s: Scenario, extra: Extra) -> Vec<PushOutcome> {
    let mut b = Buffer::new(s.config());
    let mut out = Vec::new();
    let mut next = 1usize;
    let mut now = 0.0f64;

    // Fill until the buffer holds `CAPACITY` packets (lost ones are retried).
    while b.len() < CAPACITY {
        let o = b.push_outcome(pkt(next), 100, now);
        next += 1;
        now += 50.0;
        out.push(o);
        assert!(next < 10_000, "the buffer never filled: {out:?}");
    }
    assert_eq!(b.len(), CAPACITY);
    let fill_end = now;

    let push_extra = |b: &mut Buffer, next: &mut usize, expect_full: bool| {
        for j in 0..EXTRA {
            let t = fill_end + s.extra_gap_ns * (j as f64 + 1.0);
            let o = b.push_outcome(pkt(*next), 1518, t);
            *next += 1;
            if expect_full {
                assert_eq!(
                    o,
                    PushOutcome::BufferFull,
                    "extra push {j} into a full buffer"
                );
                assert_eq!(b.len(), CAPACITY, "a full-buffer push queues nothing");
            } else {
                b.pop();
            }
        }
    };

    match extra {
        Extra::None => drain(&mut b),
        Extra::WhileFull => {
            push_extra(&mut b, &mut next, true);
            drain(&mut b);
        }
        Extra::WithRoom => {
            drain(&mut b);
            push_extra(&mut b, &mut next, false);
        }
    }
    assert!(b.is_empty() || extra == Extra::WithRoom);
    drain(&mut b);

    let tail_start = fill_end + s.extra_gap_ns * (EXTRA as f64) + s.tail_pad_ns;
    for k in 0..TAIL {
        let t = tail_start + s.tail_step_ns * k as f64;
        out.push(b.push_outcome(pkt(next), 1518, t));
        next += 1;
        drain(&mut b);
    }
    out
}

fn count(o: &[PushOutcome], f: impl Fn(&PushOutcome) -> bool) -> usize {
    o.iter().filter(|x| f(x)).count()
}

/// The tail must be identical with and without the full-buffer pushes, and
/// (control) must differ when the same pushes find room.
fn check(name: &str, s: Scenario) {
    let without = trace(s, Extra::None);
    let with_full = trace(s, Extra::WhileFull);
    assert_eq!(
        with_full, without,
        "{name} (reordering={}): pushes into a full buffer changed later outcomes",
        s.reordering
    );
    let with_room = trace(s, Extra::WithRoom);
    assert_ne!(
        with_room, without,
        "{name} (reordering={}): control: pushes that find room must change the tail, \
         otherwise this scenario cannot detect a touched state",
        s.reordering
    );
    // the traces are not trivial
    assert!(
        count(&without, |o| matches!(o, PushOutcome::Accepted { .. })) >= 5,
        "{name}: too few accepted packets"
    );
}

const MODES: [bool; 2] = [false, true];

#[test]
fn a_full_buffer_push_does_not_sample_the_delayer() {
    for reordering in MODES {
        let s = Scenario {
            delay: Delay::Jitter,
            ..Scenario::base(reordering)
        };
        check("delayer rng", s);
    }
}

#[test]
fn a_full_buffer_push_does_not_advance_the_correlated_delayer() {
    // The extra pushes are 1.5 ms apart: every one would refresh the rate
    // estimate (every > 1 ms) and move the correlation weight and the EMA.
    // FIFO only: reordering cannot be combined with correlation (spec 45).
    let s = Scenario {
        delay: Delay::Correlated,
        extra_gap_ns: 1_500_000.0,
        tail_pad_ns: 1_500_000.0,
        tail_step_ns: 10_000.0,
        ..Scenario::base(false)
    };
    check("correlated delayer", s);
}

#[test]
fn a_full_buffer_push_does_not_account_bits_in_the_spacer() {
    // The extra pushes (12336 bit each) sit at the same instants as the tail;
    // accounting them would push the tail's spacing offsets up and its limit
    // drops earlier.
    // FIFO only: reordering works only with the token bucket (spec 45).
    let s = Scenario {
        limiter: Some(LimiterMode::Spacer),
        ..Scenario::base(false)
    };
    check("spacer", s);
    let t = trace(s, Extra::None);
    assert!(
        count(&t, |o| *o == PushOutcome::LimiterDrop) > 10,
        "spacer limit reached"
    );
}

#[test]
fn a_full_buffer_push_does_not_consume_tokens_of_the_token_bucket() {
    for reordering in MODES {
        let s = Scenario {
            limiter: Some(LimiterMode::TokenBucketFilter),
            ..Scenario::base(reordering)
        };
        check("token bucket", s);
        let t = trace(s, Extra::None);
        assert!(
            count(&t, |o| *o == PushOutcome::LimiterDrop) > 10,
            "bucket runs dry"
        );
    }
}

#[test]
fn a_full_buffer_push_does_not_draw_from_the_random_loss_model() {
    for reordering in MODES {
        let s = Scenario {
            loss: LossModel::Random(0.5),
            ..Scenario::base(reordering)
        };
        check("random loss", s);
        let t = trace(s, Extra::None);
        assert!(count(&t, |o| *o == PushOutcome::Lost) > 50, "losses happen");
    }
}

#[test]
fn a_full_buffer_push_does_not_step_the_gilbert_elliot_chain() {
    for reordering in MODES {
        let s = Scenario {
            loss: LossModel::GilbertElliot(0.3, 0.3, 0.9, 0.2, true),
            ..Scenario::base(reordering)
        };
        check("gilbert-elliot", s);
    }
}

#[test]
fn a_full_buffer_push_leaves_every_stage_untouched_when_all_are_active() {
    // FIFO only: reordering cannot be combined with correlation (spec 45).
    for mode in [LimiterMode::Spacer, LimiterMode::TokenBucketFilter] {
        let s = Scenario {
            delay: Delay::Correlated,
            limiter: Some(mode),
            loss: LossModel::GilbertElliot(0.2, 0.4, 0.95, 0.3, true),
            ..Scenario::base(false)
        };
        check(&format!("everything, {mode:?}"), s);
    }
}

// ── the same, stated directly on one push ───────────────────────────

#[test]
fn a_buffer_full_push_reports_buffer_full_whatever_the_stages_would_say() {
    // An oversized packet (60000 B, far above the derived limit) into a full
    // buffer: the limiter would reject it, but it is never consulted.
    for reordering in MODES {
        for mode in [LimiterMode::Spacer, LimiterMode::TokenBucketFilter] {
            if reordering && matches!(mode, LimiterMode::Spacer) {
                continue; // reordering works only with the token bucket (spec 45)
            }
            let mut cfg = Scenario {
                limiter: Some(mode),
                ..Scenario::base(reordering)
            }
            .config();
            cfg.buffer_capacity = 1;
            let mut b = Buffer::new(cfg);
            assert!(matches!(
                b.push_outcome(pkt(1), 100, 0.0),
                PushOutcome::Accepted { .. }
            ));
            assert_eq!(
                b.push_outcome(pkt(2), 60_000, 0.0),
                PushOutcome::BufferFull,
                "{mode:?} reordering={reordering}"
            );
            assert_eq!(b.len(), 1);
            assert_eq!(b.pop(), Some(pkt(1)));
        }
    }
}

#[test]
fn push_and_push_outcome_agree_that_a_full_buffer_rejects() {
    for reordering in MODES {
        let mut cfg = Scenario::base(reordering).config();
        cfg.buffer_capacity = 1;
        let mut b = Buffer::new(cfg);
        assert!(b.push(pkt(1), 100, 0.0));
        assert!(!b.push(pkt(2), 100, 0.0));
        assert_eq!(b.len(), 1);
    }
}
