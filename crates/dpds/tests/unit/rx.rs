//! RX step.
//!
//! `Rx::step(q, t, batch)`:
//! - calls `q.rx_burst(current_port, &mut batch[..])` exactly once, with the
//!   full `MAX_BURST_SIZE`-long slice, on the CURRENT port (queue 0 is implied);
//! - then advances round-robin to the next port (wrapping to 0), EVEN if
//!   nothing was received;
//! - returns n; the first n entries of `batch` are the received mbufs;
//! - telemetry (feature only): `rx_pkts += n`; `rx_full_bursts += 1` iff
//!   n == MAX_BURST_SIZE. `polls` is NOT counted here (the loop does it).
//! - `Rx::next_port()` is the port the next `step` will poll (starts at 0).

use dpds::consts::MAX_BURST_SIZE;
use dpds::rx::Rx;

use crate::support::*;

fn batch() -> [dpdk::RawMbuf; MAX_BURST_SIZE] {
    [std::ptr::null_mut(); MAX_BURST_SIZE]
}

#[test]
fn step_polls_with_the_full_burst_size_and_returns_what_was_received() {
    let mut mbufs = FakeMbufs::new(5, 0, 64);
    let ptrs = mbufs.ptrs();
    let mut q = MockRx::with_script(vec![ptrs.clone()]);
    let (_stats, mut t) = telemetry(0.0);
    let mut rx = Rx::new(1);
    let mut b = batch();

    let n = rx.step(&mut q, &mut t, &mut b);

    assert_eq!(n, 5);
    assert_eq!(&b[..5], &ptrs[..]);
    assert_eq!(q.calls, vec![(0u16, MAX_BURST_SIZE)]);
}

#[test]
fn single_port_keeps_polling_port_zero() {
    let mut q = MockRx::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut rx = Rx::new(1);
    let mut b = batch();
    for _ in 0..4 {
        rx.step(&mut q, &mut t, &mut b);
    }
    assert_eq!(
        q.calls.iter().map(|c| c.0).collect::<Vec<_>>(),
        [0, 0, 0, 0]
    );
    assert_eq!(rx.next_port(), 0);
}

#[test]
fn two_ports_alternate_round_robin_even_when_empty() {
    let mut q = MockRx::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut rx = Rx::new(2);
    let mut b = batch();
    assert_eq!(rx.next_port(), 0);
    for expected_next in [1u16, 0, 1, 0, 1] {
        rx.step(&mut q, &mut t, &mut b);
        assert_eq!(rx.next_port(), expected_next);
    }
    assert_eq!(
        q.calls.iter().map(|c| c.0).collect::<Vec<_>>(),
        [0, 1, 0, 1, 0]
    );
}

#[test]
fn empty_burst_returns_zero() {
    let mut q = MockRx::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut rx = Rx::new(2);
    let mut b = batch();
    assert_eq!(rx.step(&mut q, &mut t, &mut b), 0);
}

#[test]
fn a_full_burst_is_returned_whole() {
    let mut mbufs = FakeMbufs::new(MAX_BURST_SIZE, 0, 64);
    let ptrs = mbufs.ptrs();
    let mut q = MockRx::with_script(vec![ptrs.clone()]);
    let (_stats, mut t) = telemetry(0.0);
    let mut rx = Rx::new(1);
    let mut b = batch();
    assert_eq!(rx.step(&mut q, &mut t, &mut b), MAX_BURST_SIZE);
    assert_eq!(&b[..], &ptrs[..]);
}

#[cfg(feature = "telemetry")]
mod counters {
    use super::*;

    #[test]
    fn rx_pkts_accumulates_what_was_received_and_polls_is_left_to_the_loop() {
        let mut m = FakeMbufs::new(9, 0, 64);
        let p = m.ptrs();
        let mut q = MockRx::with_script(vec![p[..5].to_vec(), vec![], p[5..].to_vec()]);
        let (stats, mut t) = telemetry(0.0);
        let mut rx = Rx::new(1);
        let mut b = batch();
        for _ in 0..3 {
            rx.step(&mut q, &mut t, &mut b);
        }
        assert_eq!(stats.counters.rx_pkts.get(), 9);
        assert_eq!(stats.counters.polls.get(), 0, "the loop counts polls");
        assert_eq!(stats.counters.rx_full_bursts.get(), 0);
    }

    #[test]
    fn rx_full_bursts_counts_exactly_the_full_max_burst_size_bursts() {
        let mut m = FakeMbufs::new(2 * MAX_BURST_SIZE + 15, 0, 64);
        let p = m.ptrs();
        let mut q = MockRx::with_script(vec![
            p[..MAX_BURST_SIZE].to_vec(),                    // full
            p[MAX_BURST_SIZE..MAX_BURST_SIZE + 15].to_vec(), // 15: not full
            p[MAX_BURST_SIZE + 15..].to_vec(),               // full
        ]);
        let (stats, mut t) = telemetry(0.0);
        let mut rx = Rx::new(1);
        let mut b = batch();
        for _ in 0..3 {
            rx.step(&mut q, &mut t, &mut b);
        }
        assert_eq!(stats.counters.rx_full_bursts.get(), 2);
        assert_eq!(
            stats.counters.rx_pkts.get(),
            (2 * MAX_BURST_SIZE + 15) as u64
        );
    }

    #[test]
    fn an_empty_burst_counts_nothing() {
        let mut q = MockRx::default();
        let (stats, mut t) = telemetry(0.0);
        let mut rx = Rx::new(2);
        let mut b = batch();
        rx.step(&mut q, &mut t, &mut b);
        assert_eq!(stats.counters.rx_pkts.get(), 0);
        assert_eq!(stats.counters.rx_full_bursts.get(), 0);
    }
}
