//! RX stage.
//!
//! `Rx::step(q, stats, batch)`:
//! - calls `q.rx_burst(current_port, &mut batch[..])` exactly once, with the
//!   full `MAX_BURST_SIZE`-long slice, on the CURRENT port (queue 0 is implied);
//! - then advances round-robin to the next port (wrapping to 0), EVEN if
//!   nothing was received;
//! - if n > 0: `stats.received += n` and `stats.burst_hist[floor(log2 n)] += 1`;
//!   if n == 0 nothing is counted;
//! - returns n; the first n entries of `batch` are the received mbufs.
//! - `Rx::next_port()` is the port the next `step` will poll (starts at 0).
//!
//! `rx::enqueue_burst(ring, free, stats, batch)` (standalone RX only):
//! - `sent = ring.enqueue_burst(batch)`; `stats.enqueued += sent`;
//! - mbufs `batch[sent..]` are freed IN ORDER and counted in
//!   `stats.rx_ring_drops`.

use std::sync::atomic::Ordering::Relaxed;

use dpds::consts::MAX_BURST_SIZE;
use dpds::rx::{enqueue_burst, Rx};
use dpds::stats::RxStats;

use crate::support::*;

fn batch() -> [dpdk::RawMbuf; MAX_BURST_SIZE] {
    [std::ptr::null_mut(); MAX_BURST_SIZE]
}

fn hist(s: &RxStats) -> [u64; 5] {
    std::array::from_fn(|i| s.burst_hist[i].load(Relaxed))
}

#[test]
fn step_polls_with_the_full_burst_size_and_returns_what_was_received() {
    let mut mbufs = FakeMbufs::new(5, 0, 64);
    let ptrs = mbufs.ptrs();
    let mut q = MockRx::with_script(vec![ptrs.clone()]);
    let stats = RxStats::default();
    let mut rx = Rx::new(1);
    let mut b = batch();

    let n = rx.step(&mut q, &stats, &mut b);

    assert_eq!(n, 5);
    assert_eq!(&b[..5], &ptrs[..]);
    assert_eq!(q.calls, vec![(0u16, MAX_BURST_SIZE)]);
    assert_eq!(stats.received.load(Relaxed), 5);
    assert_eq!(hist(&stats), [0, 0, 1, 0, 0]); // 5 -> bucket [4-7]
}

#[test]
fn single_port_keeps_polling_port_zero() {
    let mut q = MockRx::default();
    let stats = RxStats::default();
    let mut rx = Rx::new(1);
    let mut b = batch();
    for _ in 0..4 {
        rx.step(&mut q, &stats, &mut b);
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
    let stats = RxStats::default();
    let mut rx = Rx::new(2);
    let mut b = batch();
    assert_eq!(rx.next_port(), 0);
    for expected_next in [1u16, 0, 1, 0, 1] {
        rx.step(&mut q, &stats, &mut b);
        assert_eq!(rx.next_port(), expected_next);
    }
    assert_eq!(
        q.calls.iter().map(|c| c.0).collect::<Vec<_>>(),
        [0, 1, 0, 1, 0]
    );
}

#[test]
fn empty_burst_counts_nothing() {
    let mut q = MockRx::default();
    let stats = RxStats::default();
    let mut rx = Rx::new(2);
    let mut b = batch();
    assert_eq!(rx.step(&mut q, &stats, &mut b), 0);
    assert_eq!(stats.received.load(Relaxed), 0);
    assert_eq!(hist(&stats), [0; 5]);
}

#[test]
fn histogram_bucket_is_floor_log2_of_burst_size() {
    // (burst size, expected bucket)
    let cases = [
        (1usize, 0usize),
        (2, 1),
        (3, 1),
        (4, 2),
        (7, 2),
        (8, 3),
        (15, 3),
        (16, 4),
    ];
    for (n, bucket) in cases {
        let mut mbufs = FakeMbufs::new(n, 0, 64);
        let mut q = MockRx::with_script(vec![mbufs.ptrs()]);
        let stats = RxStats::default();
        let mut rx = Rx::new(1);
        let mut b = batch();
        assert_eq!(rx.step(&mut q, &stats, &mut b), n);
        let mut expected = [0u64; 5];
        expected[bucket] = 1;
        assert_eq!(hist(&stats), expected, "burst of {n}");
        assert_eq!(stats.received.load(Relaxed), n as u64);
    }
}

#[test]
fn counters_accumulate_across_steps_and_ports() {
    let mut mbufs = FakeMbufs::new(1 + 16 + 3, 0, 64);
    let p = mbufs.ptrs();
    let mut q = MockRx::with_script(vec![
        p[0..1].to_vec(),   // port 0: 1
        p[1..17].to_vec(),  // port 1: 16
        vec![],             // port 0: 0
        p[17..20].to_vec(), // port 1: 3
    ]);
    let stats = RxStats::default();
    let mut rx = Rx::new(2);
    let mut b = batch();
    for _ in 0..4 {
        rx.step(&mut q, &stats, &mut b);
    }
    assert_eq!(stats.received.load(Relaxed), 20);
    assert_eq!(hist(&stats), [1, 1, 0, 0, 1]);
}

// ---------- enqueue_burst ----------

#[test]
fn enqueue_all_when_ring_has_space() {
    let mut mbufs = FakeMbufs::new(4, 0, 64);
    let p = mbufs.ptrs();
    let mut ring = MockRing::with_space(100);
    let mut free = MockFree::default();
    let stats = RxStats::default();

    enqueue_burst(&mut ring, &mut free, &stats, &p);

    assert_eq!(ring.enqueued, p);
    assert!(free.freed.is_empty());
    assert_eq!(stats.enqueued.load(Relaxed), 4);
    assert_eq!(stats.rx_ring_drops.load(Relaxed), 0);
}

#[test]
fn enqueue_on_full_ring_frees_the_rest_in_order_and_counts_drops() {
    let mut mbufs = FakeMbufs::new(6, 0, 64);
    let p = mbufs.ptrs();
    let mut ring = MockRing::with_space(2);
    let mut free = MockFree::default();
    let stats = RxStats::default();

    enqueue_burst(&mut ring, &mut free, &stats, &p);

    assert_eq!(ring.enqueued, p[..2].to_vec());
    assert_eq!(free.freed, p[2..].to_vec());
    assert_eq!(stats.enqueued.load(Relaxed), 2);
    assert_eq!(stats.rx_ring_drops.load(Relaxed), 4);
}

#[test]
fn enqueue_on_completely_full_ring_drops_everything() {
    let mut mbufs = FakeMbufs::new(3, 0, 64);
    let p = mbufs.ptrs();
    let mut ring = MockRing::with_space(0);
    let mut free = MockFree::default();
    let stats = RxStats::default();

    enqueue_burst(&mut ring, &mut free, &stats, &p);

    assert!(ring.enqueued.is_empty());
    assert_eq!(free.freed, p);
    assert_eq!(stats.enqueued.load(Relaxed), 0);
    assert_eq!(stats.rx_ring_drops.load(Relaxed), 3);
}

#[test]
fn enqueue_counters_accumulate_over_calls() {
    let mut mbufs = FakeMbufs::new(6, 0, 64);
    let p = mbufs.ptrs();
    let mut ring = MockRing::with_space(5);
    let mut free = MockFree::default();
    let stats = RxStats::default();

    enqueue_burst(&mut ring, &mut free, &stats, &p[..3]);
    enqueue_burst(&mut ring, &mut free, &stats, &p[3..]);

    assert_eq!(stats.enqueued.load(Relaxed), 5);
    assert_eq!(stats.rx_ring_drops.load(Relaxed), 1);
    assert_eq!(free.freed, vec![p[5]]);
}
