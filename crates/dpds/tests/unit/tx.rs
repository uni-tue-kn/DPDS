//! TX step: per-port accumulation tables in front of `tx_burst`.
//!
//! `unsafe Tx::step(nic, free, t, now_ns, batch)` (valid, owned mbufs), in
//! this order:
//! 1. FLUSH every non-empty per-port table, ascending port id (this is the
//!    "flush at start of step" rule, so packets wait at most one step);
//! 2. for each mbuf in `batch` in order: append it to the table of
//!    `mbuf.port`; as soon as that table holds MAX_BURST_SIZE (16) mbufs,
//!    flush it right away.
//!
//! FLUSH of port p (table of n > 0 mbufs, in arrival order):
//! - `sent = nic.tx_burst(p, &mut table[..n])` (exactly one call, port id p);
//! - mbufs `table[sent..n]` are freed IN ORDER via `free`;
//! - the table is empty afterwards.
//!
//! The clock `now_ns` is read ONLY to timestamp a first-loss record on a
//! partial send (never on the hot path).
//!
//! `Tx::cleanup(nic, free, t, now_ns)`: flush all non-empty tables (same
//! FLUSH). `Tx::pending(p)`: number of mbufs waiting in port p's table.
//!
//! Telemetry (feature only), per FLUSH: `tx_calls += 1`, `tx_pkts += n`,
//! `tx_batch_max.observe(n)`, `tx_sent += sent`; if `sent < n`:
//! `tx_short_calls += 1` and a `DescriptorFull` first loss
//! (`RingFull { from: Dpds, to: Nic }`, relative_ns = now_ns() - start).
//! `tx_drops` is derived at read time (`tx_pkts - tx_sent`), not a counter.

use dpds::tx::Tx;

use crate::support::*;

fn step(
    tx: &mut Tx,
    nic: &mut MockTx,
    free: &mut MockFree,
    t: &mut dpds::stats::Telemetry<'_>,
    batch: &[dpdk::RawMbuf],
) {
    // SAFETY: fake but valid mbufs owned by the test; the mocks never
    // dereference them.
    unsafe { tx.step(nic, free, t, fixed_clock(0.0), batch) }
}

fn cleanup(tx: &mut Tx, nic: &mut MockTx, free: &mut MockFree, t: &mut dpds::stats::Telemetry<'_>) {
    tx.cleanup(nic, free, t, fixed_clock(0.0));
}

#[test]
fn packets_are_held_until_the_next_step() {
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(1);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);

    assert!(nic.calls.is_empty(), "no tx_burst until a flush condition");
    assert_eq!(tx.pending(0), 3);
}

#[test]
fn next_step_flushes_pending_packets_before_processing_new_ones() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(1);

    step(&mut tx, &mut nic, &mut free, &mut t, &p[..3]);
    step(&mut tx, &mut nic, &mut free, &mut t, &p[3..]);

    // The first 3 went out in one burst at the start of the second step;
    // the 2 new ones are now pending.
    assert_eq!(nic.calls, vec![(0u16, p[..3].to_vec())]);
    assert_eq!(tx.pending(0), 2);
}

#[test]
fn empty_step_flushes() {
    let mut m = FakeMbufs::new(2, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(1);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);
    step(&mut tx, &mut nic, &mut free, &mut t, &[]);

    assert_eq!(nic.calls, vec![(0u16, p.clone())]);
    assert_eq!(tx.pending(0), 0);
}

#[test]
fn empty_step_with_nothing_pending_does_nothing() {
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(2);
    step(&mut tx, &mut nic, &mut free, &mut t, &[]);
    assert!(nic.calls.is_empty());
}

#[test]
fn table_is_flushed_immediately_when_it_reaches_max_burst_size() {
    let mut m = FakeMbufs::new(16, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(1);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);

    assert_eq!(nic.calls, vec![(0u16, p.clone())]);
    assert_eq!(tx.pending(0), 0);
}

#[test]
fn a_batch_larger_than_one_burst_splits_at_sixteen() {
    let mut m = FakeMbufs::new(20, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(1);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);

    assert_eq!(nic.calls, vec![(0u16, p[..16].to_vec())]);
    assert_eq!(tx.pending(0), 4);

    cleanup(&mut tx, &mut nic, &mut free, &mut t);
    assert_eq!(nic.calls[1], (0u16, p[16..].to_vec()));
}

#[test]
fn packets_are_accumulated_per_port_by_mbuf_port_field() {
    let mut m = FakeMbufs::new(6, 0, 64);
    // ports: 0 1 1 0 1 0
    for (i, port) in [0u16, 1, 1, 0, 1, 0].into_iter().enumerate() {
        m.set_port(i, port);
    }
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(2);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);
    assert!(nic.calls.is_empty());
    assert_eq!(tx.pending(0), 3);
    assert_eq!(tx.pending(1), 3);

    // Next (empty) step flushes ascending port id, preserving arrival order per port.
    step(&mut tx, &mut nic, &mut free, &mut t, &[]);
    assert_eq!(
        nic.calls,
        vec![
            (0u16, vec![p[0], p[3], p[5]]),
            (1u16, vec![p[1], p[2], p[4]]),
        ]
    );
}

#[test]
fn one_port_filling_up_does_not_flush_the_other() {
    let mut m = FakeMbufs::new(17, 0, 64);
    m.set_port(16, 1); // 16 on port 0, one on port 1
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(2);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);

    assert_eq!(nic.calls.len(), 1);
    assert_eq!(nic.calls[0].0, 0);
    assert_eq!(nic.calls[0].1.len(), 16);
    assert_eq!(tx.pending(0), 0);
    assert_eq!(tx.pending(1), 1);
}

#[test]
fn partial_tx_burst_frees_the_unsent_tail_in_order() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx {
        accept_limit: 3,
        ..Default::default()
    };
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(1);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);
    cleanup(&mut tx, &mut nic, &mut free, &mut t);

    assert_eq!(
        nic.calls,
        vec![(0u16, p.clone())],
        "all 5 offered in one call"
    );
    assert_eq!(free.freed, vec![p[3], p[4]], "unsent mbufs freed in order");
    assert_eq!(tx.pending(0), 0);
}

#[test]
fn tx_burst_accepting_nothing_frees_the_whole_burst() {
    let mut m = FakeMbufs::new(4, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx {
        accept_limit: 0,
        ..Default::default()
    };
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(1);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);
    cleanup(&mut tx, &mut nic, &mut free, &mut t);

    assert_eq!(free.freed, p);
}

#[test]
fn full_burst_with_partial_accept_frees_from_the_accepted_count() {
    let mut m = FakeMbufs::new(16, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx {
        accept_limit: 10,
        ..Default::default()
    };
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(1);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);

    assert_eq!(free.freed, p[10..].to_vec());
}

#[test]
fn successful_transmission_frees_nothing_and_never_reads_the_clock() {
    let mut m = FakeMbufs::new(4, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(1);
    let reads = std::cell::Cell::new(0u32);
    let clock = || {
        reads.set(reads.get() + 1);
        0.0
    };

    // SAFETY: fake but valid mbufs owned by the test.
    unsafe { tx.step(&mut nic, &mut free, &mut t, clock, &p) };
    tx.cleanup(&mut nic, &mut free, &mut t, clock);

    assert!(free.freed.is_empty(), "the NIC owns transmitted mbufs");
    assert_eq!(reads.get(), 0, "the clock is only read on a partial send");
}

#[test]
fn cleanup_flushes_every_port_and_is_idempotent() {
    let mut m = FakeMbufs::new(3, 0, 64);
    m.set_port(1, 1);
    m.set_port(2, 1);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(2);

    step(&mut tx, &mut nic, &mut free, &mut t, &p);
    cleanup(&mut tx, &mut nic, &mut free, &mut t);

    assert_eq!(
        nic.calls,
        vec![(0u16, vec![p[0]]), (1u16, vec![p[1], p[2]])]
    );
    assert_eq!(tx.pending(0), 0);
    assert_eq!(tx.pending(1), 0);

    cleanup(&mut tx, &mut nic, &mut free, &mut t);
    assert_eq!(nic.calls.len(), 2, "second cleanup has nothing to flush");
}

#[test]
fn cleanup_on_a_fresh_tx_does_nothing() {
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(2);
    cleanup(&mut tx, &mut nic, &mut free, &mut t);
    assert!(nic.calls.is_empty());
}

#[test]
fn nothing_is_lost_or_duplicated_over_many_steps() {
    // 100 packets alternating ports in steps of 7; every mbuf must be
    // offered to the NIC exactly once.
    let n = 100;
    let mut m = FakeMbufs::new(n, 0, 64);
    for i in 0..n {
        m.set_port(i, (i % 2) as u16);
    }
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);
    let mut tx = Tx::new(2);

    for chunk in p.chunks(7) {
        step(&mut tx, &mut nic, &mut free, &mut t, chunk);
    }
    cleanup(&mut tx, &mut nic, &mut free, &mut t);

    let mut offered: Vec<_> = nic.calls.iter().flat_map(|c| c.1.clone()).collect();
    offered.sort();
    let mut expected = p.clone();
    expected.sort();
    assert_eq!(offered, expected);
    // each call carries only mbufs of the port it was sent on
    for (port, bufs) in &nic.calls {
        for b in bufs {
            assert_eq!(port_of(*b), *port);
        }
    }
}

#[cfg(feature = "telemetry")]
mod counters {
    use dpds::first_loss::{FirstLossReason, Stage, StageLossDetail};

    use super::*;

    #[test]
    fn a_flush_counts_one_call_its_packets_and_the_batch_max() {
        let mut m = FakeMbufs::new(3, 0, 64);
        let p = m.ptrs();
        let mut nic = MockTx::default();
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);
        let mut tx = Tx::new(1);

        step(&mut tx, &mut nic, &mut free, &mut t, &p);
        let c = &stats.counters;
        assert_eq!(c.tx_calls.get(), 0, "held packets are not a call yet");
        assert_eq!(c.tx_pkts.get(), 0);

        step(&mut tx, &mut nic, &mut free, &mut t, &[]);
        assert_eq!(c.tx_calls.get(), 1);
        assert_eq!(c.tx_pkts.get(), 3);
        assert_eq!(c.tx_sent.get(), 3);
        assert_eq!(c.tx_short_calls.get(), 0);
        assert_eq!(c.tx_batch_max.get(), 3);
        assert!(stats.first_loss.current().is_none());
    }

    #[test]
    fn tx_calls_counts_only_calls_with_at_least_one_packet() {
        let mut nic = MockTx::default();
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);
        let mut tx = Tx::new(2);
        for _ in 0..5 {
            step(&mut tx, &mut nic, &mut free, &mut t, &[]);
        }
        cleanup(&mut tx, &mut nic, &mut free, &mut t);
        assert_eq!(stats.counters.tx_calls.get(), 0);
        assert_eq!(stats.counters.tx_pkts.get(), 0);
        assert_eq!(stats.counters.tx_batch_max.get(), 0);
        assert!(nic.calls.is_empty());
    }

    #[test]
    fn tx_calls_equals_the_number_of_tx_burst_calls_and_tx_pkts_their_total_size() {
        let n = 100;
        let mut m = FakeMbufs::new(n, 0, 64);
        for i in 0..n {
            m.set_port(i, (i % 2) as u16);
        }
        let p = m.ptrs();
        let mut nic = MockTx::default();
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);
        let mut tx = Tx::new(2);
        for chunk in p.chunks(7) {
            step(&mut tx, &mut nic, &mut free, &mut t, chunk);
        }
        cleanup(&mut tx, &mut nic, &mut free, &mut t);

        let c = &stats.counters;
        assert_eq!(c.tx_calls.get(), nic.calls.len() as u64);
        assert_eq!(c.tx_pkts.get(), 100);
        assert_eq!(c.tx_sent.get(), 100);
        let biggest = nic.calls.iter().map(|c| c.1.len()).max().unwrap() as u64;
        assert_eq!(c.tx_batch_max.get(), biggest);
    }

    #[test]
    fn a_full_table_flush_counts_a_16_packet_call_and_batch_max_16() {
        let mut m = FakeMbufs::new(20, 0, 64);
        let p = m.ptrs();
        let mut nic = MockTx::default();
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);
        let mut tx = Tx::new(1);
        step(&mut tx, &mut nic, &mut free, &mut t, &p);
        let c = &stats.counters;
        assert_eq!(c.tx_calls.get(), 1);
        assert_eq!(c.tx_pkts.get(), 16);
        assert_eq!(c.tx_batch_max.get(), 16);
        cleanup(&mut tx, &mut nic, &mut free, &mut t);
        assert_eq!(c.tx_calls.get(), 2);
        assert_eq!(c.tx_pkts.get(), 20);
        assert_eq!(
            c.tx_batch_max.get(),
            16,
            "a high-water mark, not the last call"
        );
    }

    #[test]
    fn a_partial_send_counts_short_call_derives_drops_and_latches_descriptor_full() {
        let mut m = FakeMbufs::new(5, 0, 64);
        let p = m.ptrs();
        let mut nic = MockTx {
            accept_limit: 3,
            ..Default::default()
        };
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(1_000.0);
        let mut tx = Tx::new(1);

        // SAFETY: fake but valid mbufs owned by the test.
        unsafe { tx.step(&mut nic, &mut free, &mut t, fixed_clock(9_000.0), &p) };
        tx.cleanup(&mut nic, &mut free, &mut t, fixed_clock(9_000.0));

        let c = &stats.counters;
        assert_eq!(c.tx_calls.get(), 1);
        assert_eq!(c.tx_pkts.get(), 5);
        assert_eq!(c.tx_sent.get(), 3);
        assert_eq!(c.tx_short_calls.get(), 1);
        assert_eq!(c.tx_batch_max.get(), 5);
        // tx_drops (= tx_pkts - tx_sent) is derived in the reply, see replies.rs

        let ev = stats.first_loss.current().expect("a loss was recorded");
        assert_eq!(ev.reason, FirstLossReason::DescriptorFull);
        assert_eq!(
            ev.detail,
            StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic
            }
        );
        assert_eq!(ev.relative_ns, 8_000.0, "now (9000) - start (1000)");
    }

    #[test]
    fn a_complete_send_is_not_a_short_call() {
        let mut m = FakeMbufs::new(16, 0, 64);
        let p = m.ptrs();
        let mut nic = MockTx {
            accept_limit: 16,
            ..Default::default()
        };
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);
        let mut tx = Tx::new(1);
        step(&mut tx, &mut nic, &mut free, &mut t, &p);
        assert_eq!(stats.counters.tx_short_calls.get(), 0);
    }

    #[test]
    fn every_partial_call_counts_but_only_the_first_loss_is_latched() {
        let mut m = FakeMbufs::new(4, 0, 64);
        let p = m.ptrs();
        let mut nic = MockTx {
            accept_limit: 0,
            ..Default::default()
        };
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);
        let mut tx = Tx::new(1);

        // SAFETY: fake but valid mbufs owned by the test.
        unsafe {
            tx.step(&mut nic, &mut free, &mut t, fixed_clock(100.0), &p[..2]);
            tx.step(&mut nic, &mut free, &mut t, fixed_clock(200.0), &p[2..]);
        }
        tx.cleanup(&mut nic, &mut free, &mut t, fixed_clock(300.0));

        assert_eq!(stats.counters.tx_short_calls.get(), 2);
        assert_eq!(stats.counters.tx_sent.get(), 0);
        assert_eq!(stats.counters.tx_pkts.get(), 4);
        // the first partial flush is the one at the start of the 2nd step (clock 200)
        assert_eq!(stats.first_loss.current().unwrap().relative_ns, 200.0);
    }

    #[test]
    fn tx_does_not_touch_rx_worker_or_poll_counters() {
        let mut m = FakeMbufs::new(4, 0, 64);
        let p = m.ptrs();
        let mut nic = MockTx::default();
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);
        let mut tx = Tx::new(1);
        step(&mut tx, &mut nic, &mut free, &mut t, &p);
        cleanup(&mut tx, &mut nic, &mut free, &mut t);
        let c = &stats.counters;
        assert_eq!(c.polls.get(), 0);
        assert_eq!(c.rx_pkts.get(), 0);
        assert_eq!(c.rx_full_bursts.get(), 0);
        assert_eq!(c.emulator_rejects.get(), 0);
        assert_eq!(c.pops.get(), 0);
        assert_eq!(c.buffer_len_max.get(), 0);
    }
}
