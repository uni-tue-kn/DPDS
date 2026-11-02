//! Worker stage (delay buffer in front of the mbuf stream).
//!
//! `Worker::step(stats, free, now_ns, in_batch, out_batch) -> usize`:
//! 1. `stats.received += in_batch.len()`.
//! 2. `t = now_ns()` once for the whole batch. For each mbuf in order:
//!    with 2 ports `mbuf.port ^= 1`, with 1 port the port is left untouched;
//!    then `buffer.push(mbuf, mbuf.pkt_len, t)`; if push returns false:
//!    `stats.emulator_drops += 1` and the mbuf is freed via `free`.
//! 3. `k = min(MAX_BURST_SIZE, buffer.len())`. If k == 0 return 0.
//!    Otherwise take a FRESH `t2 = now_ns()` and pop up to k packets while
//!    `buffer.front_time() <= t2` (equal counts as due); they go to
//!    `out_batch[..count]` in pop order; return count.
//! 4. `step` never counts `sent`/`tx_ring_drops` (the caller does); it never
//!    calls `now_ns` more than twice.
//!
//! `Worker::has_backlog()` is `!buffer.is_empty()`.
//!
//! `worker::enqueue_burst(ring, free, stats, batch)` (standalone worker only):
//! `sent = ring.enqueue_burst(batch)`; `stats.sent += sent`;
//! `batch[sent..]` is freed in order and counted in `stats.tx_ring_drops`.
//!
//! Tests use `DeterministicDistribution` delay without correlation, so a packet
//! ingested at time t is due at exactly `(t + delay) as u64`.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::Ordering::Relaxed;

use dpds::consts::MAX_BURST_SIZE;
use dpds::stats::WorkerStats;
use dpds::worker::{enqueue_burst, Worker};
use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

use crate::support::*;

fn config(capacity: usize, delay_ns: f64, loss: f64) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering: false,
        delayer: DelayerConfig {
            distribution: RVDistribution::DeterministicDistribution,
            mean_ns: delay_ns,
            std_ns: 0.0,
            activate_correlation: false,
            half_life_period_ns: 1.0,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1e9,
            limit_bit: 1e9,
        },
        loss_model: LossModel::Random(loss),
    }
}

fn worker(cfg: Config, ports: u16) -> Worker {
    Worker::new(Buffer::new(cfg), ports)
}

fn out() -> [dpdk::RawMbuf; MAX_BURST_SIZE] {
    [std::ptr::null_mut(); MAX_BURST_SIZE]
}

/// Clock returning `times[0]`, `times[1]`, ... and then the last value
/// forever; the shared counter records how often it was called.
fn clock(times: &[f64]) -> (impl FnMut() -> f64, Rc<Cell<usize>>) {
    let calls = Rc::new(Cell::new(0usize));
    let c = calls.clone();
    let times = times.to_vec();
    (
        move || {
            let i = c.get();
            c.set(i + 1);
            times[i.min(times.len() - 1)]
        },
        calls,
    )
}

#[test]
fn zero_delay_passes_the_whole_batch_through_in_order() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 0.0, 0.0), 1);
    let mut o = out();
    let (now, _) = clock(&[1000.0]);

    let n = w.step(&stats, &mut free, now, &p, &mut o);

    assert_eq!(n, 5);
    assert_eq!(&o[..5], &p[..]);
    assert_eq!(stats.received.load(Relaxed), 5);
    assert_eq!(stats.emulator_drops.load(Relaxed), 0);
    assert_eq!(stats.sent.load(Relaxed), 0, "caller counts sent");
    assert_eq!(stats.tx_ring_drops.load(Relaxed), 0);
    assert!(free.freed.is_empty());
    assert!(!w.has_backlog());
}

#[test]
fn two_ports_flip_the_port_field_one_port_keeps_it() {
    // two ports: 0 <-> 1
    let mut m = FakeMbufs::new(4, 0, 64);
    for (i, port) in [0u16, 1, 0, 1].into_iter().enumerate() {
        m.set_port(i, port);
    }
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 0.0, 0.0), 2);
    let mut o = out();
    let (now, _) = clock(&[1000.0]);
    assert_eq!(w.step(&stats, &mut free, now, &p, &mut o), 4);
    assert_eq!((0..4).map(|i| m.port(i)).collect::<Vec<_>>(), [1, 0, 1, 0]);

    // one port: untouched
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let mut w = worker(config(100, 0.0, 0.0), 1);
    let (now, _) = clock(&[1000.0]);
    assert_eq!(w.step(&stats, &mut free, now, &p, &mut o), 3);
    assert_eq!((0..3).map(|i| m.port(i)).collect::<Vec<_>>(), [0, 0, 0]);
}

#[test]
fn dropped_packets_are_freed_in_order_and_counted() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 0.0, 1.0), 1); // 100 % loss
    let mut o = out();
    let (now, _) = clock(&[1000.0]);

    let n = w.step(&stats, &mut free, now, &p, &mut o);

    assert_eq!(n, 0);
    assert_eq!(free.freed, p);
    assert_eq!(stats.emulator_drops.load(Relaxed), 5);
    assert_eq!(
        stats.received.load(Relaxed),
        5,
        "received counts dropped packets too"
    );
    assert!(!w.has_backlog());
}

#[test]
fn buffer_full_drops_only_the_overflow() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    // capacity 2, delay long enough that nothing is due in this step
    let mut w = worker(config(2, 1_000_000.0, 0.0), 1);
    let mut o = out();
    let (now, _) = clock(&[0.0]);

    let n = w.step(&stats, &mut free, now, &p, &mut o);

    assert_eq!(n, 0);
    assert_eq!(free.freed, p[2..].to_vec(), "packets 2..5 overflowed");
    assert_eq!(stats.emulator_drops.load(Relaxed), 3);
    assert!(w.has_backlog());
}

#[test]
fn pkt_len_is_passed_to_the_buffer() {
    // Spacer limit 1500 bit: one 100-byte packet is 992 bit on the wire, two do not fit.
    let mut cfg = config(100, 0.0, 0.0);
    cfg.limiter = LimiterConfig {
        mode: LimiterMode::Spacer,
        activate: true,
        rate_bps: 1e9,
        limit_bit: 1500.0,
    };
    let mut m = FakeMbufs::new(2, 0, 100);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(cfg, 1);
    let mut o = out();
    let (now, _) = clock(&[0.0]);

    w.step(&stats, &mut free, now, &p, &mut o);

    assert_eq!(stats.emulator_drops.load(Relaxed), 1);
    assert_eq!(free.freed, vec![p[1]]);
}

#[test]
fn not_yet_due_packets_stay_in_the_buffer() {
    let mut m = FakeMbufs::new(2, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 500.0, 0.0), 1);
    let mut o = out();
    let (now, _) = clock(&[1000.0]); // due at 1500

    assert_eq!(w.step(&stats, &mut free, now, &p, &mut o), 0);
    assert!(w.has_backlog());
    assert!(free.freed.is_empty());
}

#[test]
fn only_due_packets_are_drained_and_boundary_counts_as_due() {
    let mut m = FakeMbufs::new(4, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 500.0, 0.0), 1);
    let mut o = out();

    // p0,p1 ingested at 1000 -> due 1500; nothing due at 1000.
    let (now, _) = clock(&[1000.0]);
    assert_eq!(w.step(&stats, &mut free, now, &p[..2], &mut o), 0);

    // p2,p3 ingested at 1200 -> due 1700; drain time 1600: only p0,p1.
    let (now, _) = clock(&[1200.0, 1600.0]);
    assert_eq!(w.step(&stats, &mut free, now, &p[2..], &mut o), 2);
    assert_eq!(&o[..2], &p[..2]);
    assert!(w.has_backlog());

    // Drain time 1699: still not due (due at 1700).
    let (now, _) = clock(&[1699.0]);
    assert_eq!(w.step(&stats, &mut free, now, &[], &mut o), 0);

    // Drain time exactly 1700: due.
    let (now, _) = clock(&[1700.0]);
    assert_eq!(w.step(&stats, &mut free, now, &[], &mut o), 2);
    assert_eq!(&o[..2], &p[2..]);
    assert!(!w.has_backlog());
    assert_eq!(stats.received.load(Relaxed), 4);
}

#[test]
fn drain_uses_a_fresh_time_taken_after_ingest() {
    let mut m = FakeMbufs::new(1, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 100.0, 0.0), 1);
    let mut o = out();
    // Ingest at t=1000 (due 1100). Drain at the fresh time 1200 releases it;
    // a stale 1000 would not, and ingesting at 1200 would make it due at 1300.
    let (now, calls) = clock(&[1000.0, 1200.0]);

    let n = w.step(&stats, &mut free, now, &p, &mut o);

    assert_eq!(n, 1);
    assert_eq!(o[0], p[0]);
    assert_eq!(calls.get(), 2);
}

#[test]
fn the_clock_is_read_once_per_batch_not_once_per_packet() {
    let mut m = FakeMbufs::new(16, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 0.0, 0.0), 1);
    let mut o = out();
    let (now, calls) = clock(&[1000.0]);

    assert_eq!(w.step(&stats, &mut free, now, &p, &mut o), 16);
    assert!(calls.get() <= 2, "now_ns called {} times", calls.get());
    assert!(calls.get() >= 1);
}

#[test]
fn at_most_max_burst_size_packets_leave_per_step() {
    let mut m = FakeMbufs::new(40, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 1000.0, 0.0), 1);
    let mut o = out();

    // Ingest 16 + 16 + 8 at t=0 (due 1000); nothing leaves yet.
    for chunk in p.chunks(16) {
        let (now, _) = clock(&[0.0]);
        assert_eq!(w.step(&stats, &mut free, now, chunk, &mut o), 0);
    }

    // Everything is due now; steps drain 16, 16, 8 in FIFO order.
    let mut drained = Vec::new();
    let mut sizes = Vec::new();
    while w.has_backlog() {
        let (now, _) = clock(&[2000.0]);
        let n = w.step(&stats, &mut free, now, &[], &mut o);
        sizes.push(n);
        drained.extend_from_slice(&o[..n]);
        assert!(sizes.len() < 10, "no progress");
    }
    assert_eq!(sizes, [16, 16, 8]);
    assert_eq!(drained, p);
}

#[test]
fn drain_count_is_limited_by_buffer_len_when_smaller_than_a_burst() {
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 0.0, 0.0), 1);
    let mut o = out();
    let (now, _) = clock(&[5.0]);
    assert_eq!(w.step(&stats, &mut free, now, &p, &mut o), 3);
}

#[test]
fn empty_step_on_empty_buffer_returns_zero() {
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 0.0, 0.0), 2);
    let mut o = out();
    let (now, calls) = clock(&[1.0]);
    assert!(!w.has_backlog());
    assert_eq!(w.step(&stats, &mut free, now, &[], &mut o), 0);
    assert!(calls.get() <= 2);
    assert_eq!(stats.received.load(Relaxed), 0);
    assert!(free.freed.is_empty());
}

#[test]
fn received_accumulates_over_steps() {
    let mut m = FakeMbufs::new(6, 0, 64);
    let p = m.ptrs();
    let stats = WorkerStats::default();
    let mut free = MockFree::default();
    let mut w = worker(config(100, 0.0, 0.0), 1);
    let mut o = out();
    for chunk in p.chunks(2) {
        let (now, _) = clock(&[1.0]);
        w.step(&stats, &mut free, now, chunk, &mut o);
    }
    assert_eq!(stats.received.load(Relaxed), 6);
}

// ---------- enqueue_burst ----------

#[test]
fn enqueue_all_counts_sent() {
    let mut m = FakeMbufs::new(4, 0, 64);
    let p = m.ptrs();
    let mut ring = MockRing::with_space(10);
    let mut free = MockFree::default();
    let stats = WorkerStats::default();

    enqueue_burst(&mut ring, &mut free, &stats, &p);

    assert_eq!(ring.enqueued, p);
    assert_eq!(stats.sent.load(Relaxed), 4);
    assert_eq!(stats.tx_ring_drops.load(Relaxed), 0);
    assert!(free.freed.is_empty());
}

#[test]
fn enqueue_overflow_frees_the_rest_in_order_and_counts_tx_ring_drops() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let mut ring = MockRing::with_space(3);
    let mut free = MockFree::default();
    let stats = WorkerStats::default();

    enqueue_burst(&mut ring, &mut free, &stats, &p);

    assert_eq!(ring.enqueued, p[..3].to_vec());
    assert_eq!(free.freed, p[3..].to_vec());
    assert_eq!(stats.sent.load(Relaxed), 3);
    assert_eq!(stats.tx_ring_drops.load(Relaxed), 2);
}
