//! Reordering combined with an active Spacer limiter (two stages).
//!
//! Push: capacity check, delayer, loss model, then the reorder heap keyed by
//! `now + delay` (1/64 ns tie rule as in plain reordering). The Spacer is not
//! applied at push; `Accepted.send_time_ns` is the heap time (whole ns).
//! When an entry is due at its heap time `t_h`, it moves into a FIFO; the
//! Spacer (no limit, never drops) runs at `t_h` with the packet length + 4 FEC
//! bytes, and the send time is `(t_h + offset) as u64`. Entries leave the heap
//! in heap order, so the Spacer sees non-decreasing times.
//!
//! Numbers used by the hand-derived cases: rate 1e9 bit/s = 1 bit/ns; a
//! 1000-byte packet is `(1000 + 4 + 20) * 8 = 8192` bit = 8192 ns of Spacer
//! time; a 1518-byte one is `(1518 + 4 + 20) * 8 = 12336` bit = 12336 ns.

use std::ffi::c_void;

use dpds_core::buffer::{Buffer, PushOutcome};
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::{Limiter, LimiterMode};
use dpds_core::{Config, DelayerConfig, LimiterConfig};

const LEN: u32 = 1000;
const SLOT_NS: u64 = 8192;

fn config(capacity: usize, distribution: RVDistribution, mean_ns: f64, std_ns: f64) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering: true,
        delayer: DelayerConfig {
            distribution,
            mean_ns,
            std_ns,
            activate_correlation: false,
            half_life_period_ns: 1.0,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: true,
            rate_bps: 1e9,
        },
        loss_model: LossModel::Random(0.0),
        seed: Some(7),
    }
}

fn fixed(capacity: usize, delay_ns: f64) -> Config {
    config(
        capacity,
        RVDistribution::DeterministicDistribution,
        delay_ns,
        0.0,
    )
}

fn pkt(i: usize) -> *mut c_void {
    i as *mut c_void
}

fn accepted(b: &mut Buffer, i: usize, len: u32, now: f64) -> u64 {
    match b.push_outcome(pkt(i), len, now) {
        PushOutcome::Accepted { send_time_ns } => send_time_ns,
        other => panic!("packet {i} at {now}: {other:?}"),
    }
}

/// Delay 100 ns; packet 1 enters at 1000 (heap time 1100), packet 2 at 0
/// (100), packet 3 at 50 (150). Heap order 2, 3, 1; the Spacer then sends
/// 2 at 100, 3 right behind it at 100 + 8192 = 8292 (offset 8142 at 150) and
/// 1 right behind that at 100 + 2 * 8192 = 16484.
fn overtaking() -> Buffer {
    let mut b = Buffer::new(fixed(8, 100.0));
    assert_eq!(accepted(&mut b, 1, LEN, 1000.0), 1100);
    assert_eq!(accepted(&mut b, 2, LEN, 0.0), 100);
    assert_eq!(accepted(&mut b, 3, LEN, 50.0), 150);
    b
}

#[test]
fn a_packet_with_a_shorter_delay_overtakes_and_the_earlier_one_is_spaced_behind_it() {
    let mut b = overtaking();
    assert_eq!(b.len(), 3);
    // Exact front_time, also while the FIFO is empty but the Spacer has a backlog.
    assert_eq!(b.front_time(), Some(100.0));
    assert_eq!(b.pop_due_with_time(99.0), None);
    assert_eq!(b.pop_due_with_time(100.0), Some((pkt(2), 100)));
    assert_eq!(b.front_time(), Some(8292.0));
    assert_eq!(b.pop_due_with_time(8291.0), None);
    assert_eq!(b.pop_due_with_time(8292.0), Some((pkt(3), 8292)));
    assert_eq!(b.front_time(), Some(16484.0));
    assert_eq!(b.pop_due_with_time(16483.0), None);
    assert_eq!(b.pop_due_with_time(16484.0), Some((pkt(1), 16484)));
    assert!(b.is_empty());
    assert_eq!(b.front_time(), None);
}

#[test]
fn polling_far_past_the_due_times_gives_the_same_send_times() {
    let mut b = overtaking();
    assert_eq!(b.pop_due_with_time(1e12), Some((pkt(2), 100)));
    assert_eq!(b.pop_due_with_time(1e12), Some((pkt(3), 8292)));
    assert_eq!(b.pop_due_with_time(1e12), Some((pkt(1), 16484)));
    assert_eq!(b.pop_due_with_time(1e12), None);
}

#[test]
fn pop_and_pop_due_leave_in_the_same_order() {
    let mut b = overtaking();
    assert_eq!(b.pop(), Some(pkt(2)));
    assert_eq!(b.pop(), Some(pkt(3)));
    assert_eq!(b.pop(), Some(pkt(1)));
    assert_eq!(b.pop(), None);

    let mut b = overtaking();
    assert_eq!(b.pop_due(1e12), Some(pkt(2)));
    assert_eq!(b.pop_due(1e12), Some(pkt(3)));
    assert_eq!(b.pop_due(1e12), Some(pkt(1)));
}

#[test]
fn out_of_order_counts_the_overtaken_packet_at_the_final_pop() {
    let mut b = overtaking();
    assert_eq!(b.out_of_order(), 0);
    b.pop_due(100.0); // packet 2 (arrival 1)
    b.pop_due(8292.0); // packet 3 (arrival 2)
    assert_eq!(b.out_of_order(), 0);
    b.pop_due(16484.0); // packet 1 (arrival 0): behind two later arrivals
    assert_eq!(b.out_of_order(), 1);
}

#[test]
fn peek_nth_answers_from_the_fifo() {
    let mut b = overtaking();
    // At 150 the heap entries of packets 2 (100) and 3 (150) are due: 2 leaves,
    // 3 waits in the FIFO behind the Spacer backlog; 1 (1100) is still in the heap.
    assert_eq!(b.pop_due(150.0), Some(pkt(2)));
    assert_eq!(b.peek_nth(0), Some(pkt(3)));
    assert_eq!(b.peek_nth(1), None);
    assert_eq!(b.len(), 2);
}

#[test]
fn a_lost_packet_takes_no_spacer_budget() {
    // GilbertElliot(1, 0, 0, 1, true): exactly the first packet is lost.
    let mut c = fixed(8, 100.0);
    c.loss_model = LossModel::GilbertElliot(1.0, 0.0, 0.0, 1.0, true);
    let mut b = Buffer::new(c);
    assert_eq!(b.push_outcome(pkt(1), LEN, 0.0), PushOutcome::Lost);
    assert_eq!(accepted(&mut b, 2, LEN, 0.0), 100);
    assert_eq!(accepted(&mut b, 3, LEN, 0.0), 100);
    assert_eq!(b.pop_due_with_time(1e12), Some((pkt(2), 100)));
    assert_eq!(b.pop_due_with_time(1e12), Some((pkt(3), 100 + SLOT_NS)));
}

#[test]
fn the_spacer_has_no_limit_and_never_drops_an_accepted_packet() {
    // Capacity 2: the derived limit would be two 1518-byte slots, and in FIFO
    // mode the third packet pushed at one instant is a LimiterDrop even though
    // each is popped at once. Here all are accepted and spaced one slot apart.
    let mut b = Buffer::new(fixed(2, 0.0));
    for k in 0..20u64 {
        assert_eq!(accepted(&mut b, k as usize + 1, 1518, 0.0), 0);
        assert_eq!(
            b.pop_due_with_time(1e12),
            Some((pkt(k as usize + 1), k * 12_336)),
            "packet {k}"
        );
    }
}

#[test]
fn the_capacity_is_shared_by_the_heap_and_the_fifo() {
    let mut b = Buffer::new(fixed(3, 100.0));
    for i in 1..=3 {
        accepted(&mut b, i, LEN, 0.0);
    }
    assert_eq!(b.push_outcome(pkt(4), LEN, 0.0), PushOutcome::BufferFull);
    // The pop releases all three due entries into the FIFO; one leaves.
    assert_eq!(b.pop_due(1e12), Some(pkt(1)));
    assert_eq!(b.len(), 2);
    accepted(&mut b, 4, LEN, 200.0);
    assert_eq!(b.len(), 3);
    assert_eq!(b.push_outcome(pkt(5), LEN, 200.0), PushOutcome::BufferFull);
    // Nothing accepted is lost: the three left (2, 3, 4) all leave, in non-decreasing send times.
    let times: Vec<u64> = std::iter::from_fn(|| b.pop_due_with_time(1e12))
        .map(|(_, t)| t)
        .collect();
    assert_eq!(times.len(), 3);
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "{times:?}");
}

#[derive(Clone, Copy, Debug)]
enum Poll {
    /// Before each step, pop everything due, polling exactly at `front_time`.
    AtFrontTime,
    /// Before each step, pop everything due at the step's time.
    EveryStep,
    /// Pop nothing until all packets are pushed, then poll far in the future.
    Late,
}

const STEPS: usize = 300;
const BURST: usize = 3;
const STEP_NS: f64 = 2000.0;

/// Runs the push script (bursts of `BURST` packets every `STEP_NS`, normal
/// delay, 10% loss) and returns the accepted heap times and the send times in
/// departure order.
fn run(poll: Poll) -> (Vec<u64>, Vec<u64>) {
    let c = config(
        STEPS * BURST,
        RVDistribution::NormalDistribution,
        5000.0,
        1000.0,
    );
    let mut b = Buffer::new(Config {
        loss_model: LossModel::Random(0.1),
        ..c
    });
    let (mut heap_times, mut departures) = (Vec::new(), Vec::new());
    let mut next = 1;
    for step in 0..STEPS {
        let now = step as f64 * STEP_NS;
        match poll {
            Poll::AtFrontTime => {
                while let Some(f) = b.front_time().filter(|&f| f <= now) {
                    let (_, t) = b.pop_due_with_time(f).expect("due at front_time");
                    assert_eq!(t as f64, f);
                    departures.push(t);
                }
            }
            Poll::EveryStep => {
                while let Some((_, t)) = b.pop_due_with_time(now) {
                    departures.push(t);
                }
            }
            Poll::Late => {}
        }
        for _ in 0..BURST {
            if let PushOutcome::Accepted { send_time_ns } = b.push_outcome(pkt(next), LEN, now) {
                heap_times.push(send_time_ns);
            }
            next += 1;
        }
    }
    while let Some((_, t)) = b.pop_due_with_time(1e18) {
        departures.push(t);
    }
    assert!(b.is_empty());
    (heap_times, departures)
}

#[test]
fn send_times_equal_the_standalone_spacer_fed_in_heap_order_whatever_the_polling() {
    for poll in [Poll::AtFrontTime, Poll::EveryStep, Poll::Late] {
        let (mut heap_times, departures) = run(poll);
        assert!(heap_times.len() > STEPS, "{poll:?}: loss model too strong");
        assert_eq!(departures.len(), heap_times.len(), "{poll:?}");

        heap_times.sort_unstable();
        let mut spacer = Limiter::new(LimiterMode::Spacer, true, 1e9, f64::MAX);
        let expected: Vec<u64> = heap_times
            .iter()
            .map(|&t| {
                let t = t as f64;
                let offset = spacer
                    .limiter_calculate_offset_ns(t, LEN + 4)
                    .expect("no limit");
                (t + offset) as u64
            })
            .collect();
        assert_eq!(departures, expected, "{poll:?}");
    }
}
