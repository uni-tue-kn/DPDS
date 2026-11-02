//! TX step: per-port accumulation tables in front of `tx_burst`.

use dpdk::mbuf::{port, prefetch_header};
use dpdk::RawMbuf;

use crate::consts::MAX_BURST_SIZE;
use crate::first_loss::{FirstLossReason, Stage, StageLossDetail};
use crate::io::{MbufFree, TxBurst};
use crate::stats::Telemetry;

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

    /// Sends `port`'s table to the NIC (one `tx_burst`); frees what it
    /// refuses and records the first loss, reading the clock only then.
    ///
    /// # Safety
    ///
    /// The mbufs in `port`'s table are valid and owned by us (see `step`).
    #[inline(always)]
    unsafe fn flush(
        &mut self,
        port: usize,
        nic: &mut impl TxBurst,
        free: &mut impl MbufFree,
        t: &mut Telemetry<'_>,
        now_ns: &mut impl FnMut() -> f64,
    ) {
        let n = self.lens[port];
        let table = &mut self.tables[port][..n];
        let sent = nic.tx_burst(port as u16, table) as usize;
        let c = &t.stats.counters;
        c.tx_calls.inc();
        c.tx_pkts.add(n as u64);
        c.tx_batch_max.observe(n as u64);
        c.tx_sent.add(sent as u64);
        if sent < n {
            c.tx_short_calls.inc();
            t.record_loss(
                FirstLossReason::DescriptorFull,
                StageLossDetail::RingFull {
                    from: Stage::Dpds,
                    to: Stage::Nic,
                },
                now_ns(),
            );
            for &m in &table[sent..] {
                // SAFETY: the NIC did not take this mbuf, we still own it.
                unsafe { free.free(m) };
            }
        }
        self.lens[port] = 0;
    }

    /// # Safety
    ///
    /// The mbufs in the tables are valid and owned by us (see `step`).
    unsafe fn flush_pending(
        &mut self,
        nic: &mut impl TxBurst,
        free: &mut impl MbufFree,
        t: &mut Telemetry<'_>,
        now_ns: &mut impl FnMut() -> f64,
    ) {
        for port in 0..self.num_ports as usize {
            if self.lens[port] > 0 {
                // SAFETY: as above.
                unsafe { self.flush(port, nic, free, t, now_ns) };
            }
        }
    }

    /// Flushes every non-empty table (ascending port id), so a packet waits
    /// at most one step, then sorts `batch` into the per-port tables,
    /// flushing a table as soon as it holds `MAX_BURST_SIZE`. A flush is one
    /// `tx_burst` call: counts `tx_calls`, `tx_pkts`, `tx_batch_max`,
    /// `tx_sent`; on a partial send counts `tx_short_calls`, records a
    /// `DescriptorFull` first loss (clock read lazily, only then) and frees
    /// the unsent tail in order.
    ///
    /// # Safety
    ///
    /// Every mbuf in `batch` must be valid and owned by the caller;
    /// ownership passes to the `Tx` (sent, freed, or pending in a table).
    #[inline(always)]
    pub unsafe fn step(
        &mut self,
        nic: &mut impl TxBurst,
        free: &mut impl MbufFree,
        t: &mut Telemetry<'_>,
        mut now_ns: impl FnMut() -> f64,
        batch: &[RawMbuf],
    ) {
        // SAFETY: tables only ever hold mbufs handed to `step`, which the
        // caller guarantees valid and owned.
        unsafe { self.flush_pending(nic, free, t, &mut now_ns) };
        for (i, &m) in batch.iter().enumerate() {
            if let Some(&ahead) = batch.get(i + 3) {
                prefetch_header(ahead);
            }
            // SAFETY: valid mbuf owned by us (see above).
            let p = unsafe { port(m) } as usize;
            self.tables[p][self.lens[p]] = m;
            self.lens[p] += 1;
            if self.lens[p] == MAX_BURST_SIZE {
                // SAFETY: as above.
                unsafe { self.flush(p, nic, free, t, &mut now_ns) };
            }
        }
    }

    /// Flushes the remaining tables at shutdown (same flush as `step`).
    pub fn cleanup(
        &mut self,
        nic: &mut impl TxBurst,
        free: &mut impl MbufFree,
        t: &mut Telemetry<'_>,
        mut now_ns: impl FnMut() -> f64,
    ) {
        // SAFETY: the tables hold mbufs `step` took ownership of.
        unsafe { self.flush_pending(nic, free, t, &mut now_ns) };
    }
}
