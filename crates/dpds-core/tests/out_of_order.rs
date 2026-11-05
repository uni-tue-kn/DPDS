//! Spec 51 T1: the buffer counts out-of-order departures.
//!
//! P4TG's notion, relative to arrival order: every accepted push gets the next arrival sequence
//! number (0, 1, 2, ...); a popped packet is out of order when its sequence number is lower than
//! the highest sequence number popped before it. FIFO never reorders: always 0.
//!
//! ASSUMPTION (the spec names only the counter, "cumulative `out_of_order` counter per link"): the
//! accessor is `Buffer::out_of_order(&self) -> u64`, cumulative, counted by `pop`, `pop_due` and
//! `pop_due_with_time` alike.
//!
//! The sequences are built with a constant delay of 100 ns and pushes at different instants (the
//! buffer does not require monotonic time): the packet pushed at `now` leaves at `now + 100`.

use std::ffi::c_void;

use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

fn config(capacity: usize, reordering: bool) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering,
        delayer: DelayerConfig {
            distribution: RVDistribution::DeterministicDistribution,
            mean_ns: 100.0,
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

/// Pushes packet `i + 1` at `times[i]` and returns the buffer.
fn filled(reordering: bool, times: &[f64]) -> Buffer {
    let mut b = Buffer::new(config(times.len() + 1, reordering));
    for (i, &t) in times.iter().enumerate() {
        assert!(b.push(pkt(i + 1), 100, t));
    }
    b
}

fn drain(b: &mut Buffer) -> Vec<usize> {
    std::iter::from_fn(|| b.pop()).map(|p| p as usize).collect()
}

#[test]
fn a_fresh_buffer_has_counted_nothing() {
    assert_eq!(Buffer::new(config(4, true)).out_of_order(), 0);
    assert_eq!(Buffer::new(config(4, false)).out_of_order(), 0);
}

#[test]
fn fifo_never_reorders_even_with_send_times_that_are_not_sorted() {
    // FIFO ignores the send times: packets leave in push order
    let mut b = filled(false, &[100.0, 0.0, 50.0, 300.0, 10.0]);
    assert_eq!(drain(&mut b), vec![1, 2, 3, 4, 5]);
    assert_eq!(b.out_of_order(), 0);
}

#[test]
fn reordering_mode_without_actual_reordering_counts_zero() {
    let mut b = filled(true, &[0.0, 10.0, 20.0, 30.0, 40.0]);
    assert_eq!(drain(&mut b), vec![1, 2, 3, 4, 5]);
    assert_eq!(b.out_of_order(), 0);
}

#[test]
fn packets_entering_at_one_instant_are_not_out_of_order() {
    let mut b = filled(true, &[7.0; 50]);
    assert_eq!(drain(&mut b).len(), 50);
    assert_eq!(b.out_of_order(), 0);
}

#[test]
fn a_late_overtaker_makes_every_packet_it_passed_out_of_order_relative_to_the_highest() {
    // arrival 0..=3 (packets 1..=4) at t = 0, 10, 20, 5: they leave in the order of 0, 3, 1, 2
    // (sequence numbers), i.e. packets 1, 4, 2, 3.
    // Highest popped before: -, 0, 3, 3 -> packet 2 (seq 1 < 3) and packet 3 (seq 2 < 3) are
    // out of order: 2. (Comparing with the previous popped one only would give 1.)
    let mut b = filled(true, &[0.0, 10.0, 20.0, 5.0]);
    assert_eq!(drain(&mut b), vec![1, 4, 2, 3]);
    assert_eq!(b.out_of_order(), 2);
}

#[test]
fn one_delayed_packet_is_one_out_of_order_departure() {
    // packet 1 is pushed first but leaves last: seq 0 < the highest popped (3)
    let mut b = filled(true, &[100.0, 0.0, 1.0, 2.0]);
    assert_eq!(drain(&mut b), vec![2, 3, 4, 1]);
    assert_eq!(b.out_of_order(), 1);
}

#[test]
fn a_fully_reversed_sequence_counts_all_but_the_first_departure() {
    // pushed at t = 40, 30, 20, 10, 0: leaves 5, 4, 3, 2, 1 (seq 4 first, then every other one
    // is lower than 4)
    let mut b = filled(true, &[40.0, 30.0, 20.0, 10.0, 0.0]);
    assert_eq!(drain(&mut b), vec![5, 4, 3, 2, 1]);
    assert_eq!(b.out_of_order(), 4);
}

#[test]
fn pop_due_and_pop_due_with_time_count_like_pop() {
    let times = [0.0, 10.0, 20.0, 5.0];
    let mut a = filled(true, &times);
    while a.pop_due(1e9).is_some() {}
    assert_eq!(a.out_of_order(), 2, "pop_due");
    let mut b = filled(true, &times);
    while b.pop_due_with_time(1e9).is_some() {}
    assert_eq!(b.out_of_order(), 2, "pop_due_with_time");
}

#[test]
fn only_popped_packets_are_counted_and_the_count_is_cumulative() {
    let mut b = filled(true, &[0.0, 10.0, 20.0, 5.0]);
    // due by t = 110: send times 100, 110, 120, 105 -> only packet 1 (seq 0) and 4 (seq 3)
    assert!(b.pop_due(100.0).is_some()); // seq 0
    assert_eq!(b.out_of_order(), 0);
    assert!(b.pop_due(105.0).is_some()); // seq 3
    assert_eq!(
        b.out_of_order(),
        0,
        "nothing popped yet that is below the highest"
    );
    assert!(b.pop_due(110.0).is_some()); // seq 1 < 3
    assert_eq!(b.out_of_order(), 1);
    // the packet that is not due and the one still queued are not counted
    assert!(b.pop_due(110.0).is_none());
    assert_eq!(b.out_of_order(), 1);
    assert!(b.pop_due(1e9).is_some()); // seq 2 < 3
    assert_eq!(b.out_of_order(), 2);
}

#[test]
fn the_highest_sequence_number_is_kept_across_gaps_in_the_run() {
    // packets 1 and 2 leave in order; then 3 arrives with an earlier send time than 2's and is
    // pushed after both were popped: it is a later arrival, so not out of order.
    let mut b = Buffer::new(config(8, true));
    assert!(b.push(pkt(1), 100, 0.0));
    assert!(b.push(pkt(2), 100, 10.0));
    assert_eq!(drain(&mut b), vec![1, 2]);
    assert!(b.push(pkt(3), 100, 0.0));
    assert_eq!(drain(&mut b), vec![3]);
    assert_eq!(b.out_of_order(), 0);
}

#[test]
fn rejected_pushes_do_not_disturb_the_count() {
    // capacity 3: the 4th push is refused (buffer full) and gets no place in the arrival order
    let mut b = Buffer::new(config(3, true));
    assert!(b.push(pkt(1), 100, 0.0));
    assert!(b.push(pkt(2), 100, 10.0));
    assert!(b.push(pkt(3), 100, 20.0));
    assert!(!b.push(pkt(4), 100, 5.0));
    assert_eq!(drain(&mut b), vec![1, 2, 3]);
    assert_eq!(b.out_of_order(), 0);
}
