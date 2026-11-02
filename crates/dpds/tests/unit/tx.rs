//! TX stage: per-port accumulation tables in front of `tx_burst`.
//!
//! `Tx::step(nic, free, stats, batch)`, in this order:
//! 1. FLUSH every non-empty per-port table, ascending port id (this is the
//!    "flush at start of step" rule, so packets wait at most one step);
//! 2. if `batch` is empty, stop (nothing counted);
//! 3. `stats.received += batch.len()`;
//! 4. for each mbuf in order: append it to the table of `mbuf.port`; as soon
//!    as that table holds MAX_BURST_SIZE (16) mbufs, flush it right away.
//!
//! FLUSH of port p (table of n > 0 mbufs, in arrival order):
//! - `sent = nic.tx_burst(p, &mut table[..n])` (exactly one call, port id p);
//! - `stats.burst_hist[burst_bucket(n)] += 1` (bucket of the ATTEMPTED size);
//! - `stats.transmitted += sent`;
//! - mbufs `table[sent..n]` are freed IN ORDER via `free`, `stats.tx_drops += n - sent`;
//! - the table is empty afterwards.
//!
//! `Tx::cleanup(...)`: flush all non-empty tables (same FLUSH), counts nothing else.
//! `Tx::pending(p)`: number of mbufs waiting in port p's table.

use std::sync::atomic::Ordering::Relaxed;

use dpds::stats::TxStats;
use dpds::tx::Tx;

use crate::support::*;

fn hist(s: &TxStats) -> [u64; 5] {
    std::array::from_fn(|i| s.burst_hist[i].load(Relaxed))
}

#[test]
fn packets_are_held_until_the_next_step() {
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(1);

    tx.step(&mut nic, &mut free, &stats, &p);

    assert!(nic.calls.is_empty(), "no tx_burst until a flush condition");
    assert_eq!(tx.pending(0), 3);
    assert_eq!(stats.received.load(Relaxed), 3);
    assert_eq!(stats.transmitted.load(Relaxed), 0);
}

#[test]
fn next_step_flushes_pending_packets_before_processing_new_ones() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(1);

    tx.step(&mut nic, &mut free, &stats, &p[..3]);
    tx.step(&mut nic, &mut free, &stats, &p[3..]);

    // The first 3 went out in one burst at the start of the second step;
    // the 2 new ones are now pending.
    assert_eq!(nic.calls, vec![(0u16, p[..3].to_vec())]);
    assert_eq!(tx.pending(0), 2);
    assert_eq!(stats.received.load(Relaxed), 5);
    assert_eq!(stats.transmitted.load(Relaxed), 3);
    assert_eq!(hist(&stats), [0, 1, 0, 0, 0]); // burst of 3 -> bucket [2-3]
}

#[test]
fn empty_step_flushes_but_does_not_count_received() {
    let mut m = FakeMbufs::new(2, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(1);

    tx.step(&mut nic, &mut free, &stats, &p);
    tx.step(&mut nic, &mut free, &stats, &[]);

    assert_eq!(nic.calls, vec![(0u16, p.clone())]);
    assert_eq!(tx.pending(0), 0);
    assert_eq!(stats.received.load(Relaxed), 2);
    assert_eq!(stats.transmitted.load(Relaxed), 2);
}

#[test]
fn empty_step_with_nothing_pending_does_nothing() {
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(2);
    tx.step(&mut nic, &mut free, &stats, &[]);
    assert!(nic.calls.is_empty());
    assert_eq!(hist(&stats), [0; 5]);
    assert_eq!(stats.received.load(Relaxed), 0);
}

#[test]
fn table_is_flushed_immediately_when_it_reaches_max_burst_size() {
    let mut m = FakeMbufs::new(16, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(1);

    tx.step(&mut nic, &mut free, &stats, &p);

    assert_eq!(nic.calls, vec![(0u16, p.clone())]);
    assert_eq!(tx.pending(0), 0);
    assert_eq!(stats.transmitted.load(Relaxed), 16);
    assert_eq!(hist(&stats), [0, 0, 0, 0, 1]); // [16]
}

#[test]
fn a_batch_larger_than_one_burst_splits_at_sixteen() {
    let mut m = FakeMbufs::new(20, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(1);

    tx.step(&mut nic, &mut free, &stats, &p);

    assert_eq!(nic.calls, vec![(0u16, p[..16].to_vec())]);
    assert_eq!(tx.pending(0), 4);

    tx.cleanup(&mut nic, &mut free, &stats);
    assert_eq!(nic.calls[1], (0u16, p[16..].to_vec()));
    assert_eq!(stats.transmitted.load(Relaxed), 20);
    assert_eq!(hist(&stats), [0, 0, 1, 0, 1]); // bursts of 4 and 16
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
    let stats = TxStats::default();
    let mut tx = Tx::new(2);

    tx.step(&mut nic, &mut free, &stats, &p);
    assert!(nic.calls.is_empty());
    assert_eq!(tx.pending(0), 3);
    assert_eq!(tx.pending(1), 3);
    assert_eq!(stats.received.load(Relaxed), 6);

    // Next (empty) step flushes ascending port id, preserving arrival order per port.
    tx.step(&mut nic, &mut free, &stats, &[]);
    assert_eq!(
        nic.calls,
        vec![
            (0u16, vec![p[0], p[3], p[5]]),
            (1u16, vec![p[1], p[2], p[4]]),
        ]
    );
    assert_eq!(stats.transmitted.load(Relaxed), 6);
    assert_eq!(hist(&stats), [0, 2, 0, 0, 0]); // two bursts of 3
}

#[test]
fn one_port_filling_up_does_not_flush_the_other() {
    let mut m = FakeMbufs::new(17, 0, 64);
    m.set_port(16, 1); // 16 on port 0, one on port 1
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(2);

    tx.step(&mut nic, &mut free, &stats, &p);

    assert_eq!(nic.calls.len(), 1);
    assert_eq!(nic.calls[0].0, 0);
    assert_eq!(nic.calls[0].1.len(), 16);
    assert_eq!(tx.pending(0), 0);
    assert_eq!(tx.pending(1), 1);
}

#[test]
fn partial_tx_burst_frees_the_unsent_tail_and_counts_drops() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx {
        accept_limit: 3,
        ..Default::default()
    };
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(1);

    tx.step(&mut nic, &mut free, &stats, &p);
    tx.cleanup(&mut nic, &mut free, &stats);

    assert_eq!(
        nic.calls,
        vec![(0u16, p.clone())],
        "all 5 offered in one call"
    );
    assert_eq!(stats.transmitted.load(Relaxed), 3);
    assert_eq!(stats.tx_drops.load(Relaxed), 2);
    assert_eq!(free.freed, vec![p[3], p[4]], "unsent mbufs freed in order");
    assert_eq!(hist(&stats), [0, 0, 1, 0, 0], "bucket of attempted size 5");
    assert_eq!(tx.pending(0), 0);
}

#[test]
fn tx_burst_accepting_nothing_drops_the_whole_burst() {
    let mut m = FakeMbufs::new(4, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx {
        accept_limit: 0,
        ..Default::default()
    };
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(1);

    tx.step(&mut nic, &mut free, &stats, &p);
    tx.cleanup(&mut nic, &mut free, &stats);

    assert_eq!(stats.transmitted.load(Relaxed), 0);
    assert_eq!(stats.tx_drops.load(Relaxed), 4);
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
    let stats = TxStats::default();
    let mut tx = Tx::new(1);

    tx.step(&mut nic, &mut free, &stats, &p);

    assert_eq!(stats.transmitted.load(Relaxed), 10);
    assert_eq!(stats.tx_drops.load(Relaxed), 6);
    assert_eq!(free.freed, p[10..].to_vec());
}

#[test]
fn successful_transmission_frees_nothing() {
    let mut m = FakeMbufs::new(4, 0, 64);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(1);

    tx.step(&mut nic, &mut free, &stats, &p);
    tx.cleanup(&mut nic, &mut free, &stats);

    assert!(free.freed.is_empty(), "the NIC owns transmitted mbufs");
    assert_eq!(stats.tx_drops.load(Relaxed), 0);
}

#[test]
fn cleanup_flushes_every_port_and_is_idempotent() {
    let mut m = FakeMbufs::new(3, 0, 64);
    m.set_port(1, 1);
    m.set_port(2, 1);
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(2);

    tx.step(&mut nic, &mut free, &stats, &p);
    tx.cleanup(&mut nic, &mut free, &stats);

    assert_eq!(
        nic.calls,
        vec![(0u16, vec![p[0]]), (1u16, vec![p[1], p[2]])]
    );
    assert_eq!(stats.transmitted.load(Relaxed), 3);
    assert_eq!(hist(&stats), [1, 1, 0, 0, 0]);
    assert_eq!(tx.pending(0), 0);
    assert_eq!(tx.pending(1), 0);

    tx.cleanup(&mut nic, &mut free, &stats);
    assert_eq!(nic.calls.len(), 2, "second cleanup has nothing to flush");
    assert_eq!(stats.transmitted.load(Relaxed), 3);
}

#[test]
fn cleanup_on_a_fresh_tx_does_nothing() {
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(2);
    tx.cleanup(&mut nic, &mut free, &stats);
    assert!(nic.calls.is_empty());
    assert_eq!(hist(&stats), [0; 5]);
}

#[test]
fn nothing_is_lost_or_duplicated_over_many_steps() {
    // 100 packets alternating ports in steps of 7; every mbuf must be
    // offered to the NIC exactly once and transmitted + dropped == received.
    let n = 100;
    let mut m = FakeMbufs::new(n, 0, 64);
    for i in 0..n {
        m.set_port(i, (i % 2) as u16);
    }
    let p = m.ptrs();
    let mut nic = MockTx::default();
    let mut free = MockFree::default();
    let stats = TxStats::default();
    let mut tx = Tx::new(2);

    for chunk in p.chunks(7) {
        tx.step(&mut nic, &mut free, &stats, chunk);
    }
    tx.cleanup(&mut nic, &mut free, &stats);

    let mut offered: Vec<_> = nic.calls.iter().flat_map(|c| c.1.clone()).collect();
    offered.sort();
    let mut expected = p.clone();
    expected.sort();
    assert_eq!(offered, expected);
    assert_eq!(stats.received.load(Relaxed), 100);
    assert_eq!(
        stats.transmitted.load(Relaxed) + stats.tx_drops.load(Relaxed),
        100
    );
    // each call carries only mbufs of the port it was sent on
    for (port, bufs) in &nic.calls {
        for b in bufs {
            assert_eq!(port_of(*b), *port);
        }
    }
    // histogram counts one entry per tx_burst call
    assert_eq!(hist(&stats).iter().sum::<u64>(), nic.calls.len() as u64);
}
