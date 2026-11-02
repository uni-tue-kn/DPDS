//! TX stage: per-port accumulation tables in front of `tx_burst`.

use std::sync::atomic::Ordering::Acquire;

use dpdk::mbuf::{port, prefetch_header};
use dpdk::{RawMbuf, Ring};

use crate::consts::MAX_BURST_SIZE;
use crate::io::{dequeue_burst, MbufFree, TxBurst};
use crate::quit::Quit;
use crate::stats::{add, burst_bucket, TxStats};

/// The application supports one or two ports.
const MAX_PORTS: usize = 2;

pub struct Tx {
    tables: [[RawMbuf; MAX_BURST_SIZE]; MAX_PORTS],
    lens: [usize; MAX_PORTS],
    num_ports: u16,
}

impl Tx {
    pub fn new(num_ports: u16) -> Tx {
        assert!(num_ports as usize <= MAX_PORTS);
        Tx {
            tables: [[std::ptr::null_mut(); MAX_BURST_SIZE]; MAX_PORTS],
            lens: [0; MAX_PORTS],
            num_ports,
        }
    }

    /// Number of mbufs waiting in `port`'s table.
    pub fn pending(&self, port: u16) -> usize {
        self.lens[port as usize]
    }

    /// Sends `port`'s table to the NIC; frees and counts what it refuses.
    #[inline(always)]
    fn flush(
        &mut self,
        port: usize,
        nic: &mut impl TxBurst,
        free: &mut impl MbufFree,
        stats: &TxStats,
    ) {
        let n = self.lens[port];
        let table = &mut self.tables[port][..n];
        add(&stats.burst_hist[burst_bucket(n)], 1);
        let sent = nic.tx_burst(port as u16, table) as usize;
        add(&stats.transmitted, sent as u64);
        if sent < n {
            add(&stats.tx_drops, (n - sent) as u64);
            for &m in &table[sent..] {
                free.free(m);
            }
        }
        self.lens[port] = 0;
    }

    fn flush_pending(&mut self, nic: &mut impl TxBurst, free: &mut impl MbufFree, stats: &TxStats) {
        for port in 0..self.num_ports as usize {
            if self.lens[port] > 0 {
                self.flush(port, nic, free, stats);
            }
        }
    }

    /// Flushes everything still waiting from earlier steps (so a packet
    /// waits at most one step), then sorts `batch` into the per-port tables,
    /// sending a table as soon as it holds `MAX_BURST_SIZE` mbufs.
    ///
    /// All mbufs must be valid and owned by the caller.
    #[inline(always)]
    pub fn step(
        &mut self,
        nic: &mut impl TxBurst,
        free: &mut impl MbufFree,
        stats: &TxStats,
        batch: &[RawMbuf],
    ) {
        self.flush_pending(nic, free, stats);
        if batch.is_empty() {
            return;
        }
        add(&stats.received, batch.len() as u64);
        for (i, &m) in batch.iter().enumerate() {
            if let Some(&ahead) = batch.get(i + 3) {
                prefetch_header(ahead);
            }
            // SAFETY: valid mbuf owned by us (see above).
            let p = unsafe { port(m) } as usize;
            self.tables[p][self.lens[p]] = m;
            self.lens[p] += 1;
            if self.lens[p] == MAX_BURST_SIZE {
                self.flush(p, nic, free, stats);
            }
        }
    }

    /// Flushes the remaining tables at shutdown.
    pub fn cleanup(&mut self, nic: &mut impl TxBurst, free: &mut impl MbufFree, stats: &TxStats) {
        self.flush_pending(nic, free, stats);
    }
}

/// Standalone TX lcore: ring in, NIC out. Runs until told to quit and the
/// ring is empty, then flushes.
pub fn run(
    num_ports: u16,
    input: &Ring,
    nic: &mut impl TxBurst,
    free: &mut impl MbufFree,
    stats: &TxStats,
    quit: &Quit,
) {
    let mut tx = Tx::new(num_ports);
    let mut batch = [std::ptr::null_mut(); MAX_BURST_SIZE];
    loop {
        // Read before dequeuing, see `quit`.
        let quit_requested = quit.transmit.load(Acquire);
        let n = dequeue_burst(input, &mut batch) as usize;
        tx.step(nic, free, stats, &batch[..n]);
        if quit_requested && n == 0 {
            break;
        }
    }
    tx.cleanup(nic, free, stats);
}
