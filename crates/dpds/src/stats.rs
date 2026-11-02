//! Per-stage counters, their snapshots, and the dashboard / CSV text formats.
//!
//! Each stage's counters are written by exactly one lcore and read by the
//! main lcore, so they are plain `Relaxed` atomics (written with
//! load-then-store, see [`add`], which avoids a locked read-modify-write on
//! the hot path) and each stage struct sits on its own cache lines.

use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use crate::consts::{BURST_HIST_BUCKETS, MAX_BURST_SIZE};

/// Adds `n` to a counter that only the calling lcore ever writes.
#[inline(always)]
pub fn add(counter: &AtomicU64, n: u64) {
    counter.store(counter.load(Relaxed) + n, Relaxed);
}

/// Histogram bucket of a burst of `n > 0` packets: `floor(log2(n))`.
#[inline(always)]
pub fn burst_bucket(n: usize) -> usize {
    n.ilog2() as usize
}

#[derive(Default)]
#[repr(align(64))]
pub struct RxStats {
    pub received: AtomicU64,
    pub enqueued: AtomicU64,
    pub rx_ring_drops: AtomicU64,
    pub burst_hist: [AtomicU64; BURST_HIST_BUCKETS],
}

#[derive(Default)]
#[repr(align(64))]
pub struct WorkerStats {
    pub received: AtomicU64,
    pub sent: AtomicU64,
    pub emulator_drops: AtomicU64,
    pub tx_ring_drops: AtomicU64,
}

#[derive(Default)]
#[repr(align(64))]
pub struct TxStats {
    pub received: AtomicU64,
    pub transmitted: AtomicU64,
    pub tx_drops: AtomicU64,
    pub burst_hist: [AtomicU64; BURST_HIST_BUCKETS],
}

#[derive(Default)]
pub struct PipelineStats {
    pub rx: RxStats,
    pub worker: WorkerStats,
    pub tx: TxStats,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RxSnapshot {
    pub received: u64,
    pub enqueued: u64,
    pub rx_ring_drops: u64,
    pub burst_hist: [u64; BURST_HIST_BUCKETS],
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WorkerSnapshot {
    pub received: u64,
    pub sent: u64,
    pub emulator_drops: u64,
    pub tx_ring_drops: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TxSnapshot {
    pub received: u64,
    pub transmitted: u64,
    pub tx_drops: u64,
    pub burst_hist: [u64; BURST_HIST_BUCKETS],
}

/// Plain-value copy of [`PipelineStats`] at one point in time.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub rx: RxSnapshot,
    pub worker: WorkerSnapshot,
    pub tx: TxSnapshot,
}

fn load_hist(hist: &[AtomicU64; BURST_HIST_BUCKETS]) -> [u64; BURST_HIST_BUCKETS] {
    std::array::from_fn(|i| hist[i].load(Relaxed))
}

impl PipelineStats {
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            rx: RxSnapshot {
                received: self.rx.received.load(Relaxed),
                enqueued: self.rx.enqueued.load(Relaxed),
                rx_ring_drops: self.rx.rx_ring_drops.load(Relaxed),
                burst_hist: load_hist(&self.rx.burst_hist),
            },
            worker: WorkerSnapshot {
                received: self.worker.received.load(Relaxed),
                sent: self.worker.sent.load(Relaxed),
                emulator_drops: self.worker.emulator_drops.load(Relaxed),
                tx_ring_drops: self.worker.tx_ring_drops.load(Relaxed),
            },
            tx: TxSnapshot {
                received: self.tx.received.load(Relaxed),
                transmitted: self.tx.transmitted.load(Relaxed),
                tx_drops: self.tx.tx_drops.load(Relaxed),
                burst_hist: load_hist(&self.tx.burst_hist),
            },
        }
    }
}

/// The NIC hardware counters shown per port.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NicStats {
    pub imissed: u64,
    pub ierrors: u64,
    pub oerrors: u64,
    pub rx_nombuf: u64,
}

/// Fixed dashboard lines: header, RX, RX histogram, Worker, TX, TX histogram.
const DASHBOARD_FIXED_LINES: usize = 6;

/// Clears to end of line, so shrinking numbers leave no artifacts.
const CLEAR_EOL: &str = "\x1b[K";

/// Inclusive burst-size range `(lo, hi)` of histogram bucket `i`.
fn bucket_range(i: usize) -> (usize, usize) {
    let lo = 1usize << i;
    let hi = if i == BURST_HIST_BUCKETS - 1 {
        lo
    } else {
        (lo << 1) - 1
    };
    debug_assert!(hi <= MAX_BURST_SIZE);
    (lo, hi)
}

/// One histogram line from the delta of two histograms, e.g.
/// `  RX burst/s: [1]:5 [2-3]:10 ... | avg: 4.8`. The average (`delta_pkts`
/// over the number of bursts) is only printed when there were bursts.
pub fn format_burst_hist(
    label: &str,
    cur: &[u64; BURST_HIST_BUCKETS],
    prev: &[u64; BURST_HIST_BUCKETS],
    delta_pkts: u64,
) -> String {
    let mut out = format!("  {label} burst/s:");
    let mut total_bursts = 0u64;
    for i in 0..BURST_HIST_BUCKETS {
        let (lo, hi) = bucket_range(i);
        let delta = cur[i].saturating_sub(prev[i]);
        total_bursts += delta;
        if lo == hi {
            write!(out, " [{lo}]:{delta}").unwrap();
        } else {
            write!(out, " [{lo}-{hi}]:{delta}").unwrap();
        }
    }
    if total_bursts > 0 {
        write!(
            out,
            " | avg: {:.1}",
            delta_pkts as f64 / total_bursts as f64
        )
        .unwrap();
    }
    writeln!(out, "{CLEAR_EOL}").unwrap();
    out
}

/// The in-place dashboard. Per-second values are `cur - prev`; with
/// `overwrite` the cursor is first moved up over the previous print.
pub fn format_dashboard(
    cur: &StatsSnapshot,
    prev: &StatsSnapshot,
    nics: &[(u16, NicStats)],
    overwrite: bool,
) -> String {
    let rx_pps = cur.rx.received.saturating_sub(prev.rx.received);
    let tx_pps = cur.tx.transmitted.saturating_sub(prev.tx.transmitted);

    let mut out = String::new();
    if overwrite {
        write!(out, "\x1b[{}A", DASHBOARD_FIXED_LINES + nics.len()).unwrap();
    }
    writeln!(out, "--- Emulator Stats (1s interval) ---{CLEAR_EOL}").unwrap();
    writeln!(
        out,
        "RX:     {rx_pps} pkt/s | Total: {} | Ring drops: {}{CLEAR_EOL}",
        cur.rx.received, cur.rx.rx_ring_drops
    )
    .unwrap();
    out += &format_burst_hist("RX", &cur.rx.burst_hist, &prev.rx.burst_hist, rx_pps);
    writeln!(
        out,
        "Worker: recv {} | sent {} | emulator drops: {} | TX ring drops: {}{CLEAR_EOL}",
        cur.worker.received, cur.worker.sent, cur.worker.emulator_drops, cur.worker.tx_ring_drops
    )
    .unwrap();
    writeln!(
        out,
        "TX:     {tx_pps} pkt/s | Total: {} | TX drops: {}{CLEAR_EOL}",
        cur.tx.transmitted, cur.tx.tx_drops
    )
    .unwrap();
    out += &format_burst_hist("TX", &cur.tx.burst_hist, &prev.tx.burst_hist, tx_pps);
    for (id, n) in nics {
        writeln!(
            out,
            "NIC {id}:  imissed={} ierr={} oerr={} rx_nombuf={}{CLEAR_EOL}",
            n.imissed, n.ierrors, n.oerrors, n.rx_nombuf
        )
        .unwrap();
    }
    out
}

/// Column-name suffixes of the histogram buckets: `1`, `2_3`, `4_7`, `8_15`, `16`.
fn bucket_columns(prefix: &str) -> String {
    let mut out = String::new();
    for i in 0..BURST_HIST_BUCKETS {
        let (lo, hi) = bucket_range(i);
        if lo == hi {
            write!(out, ",{prefix}_burst_{lo}").unwrap();
        } else {
            write!(out, ",{prefix}_burst_{lo}_{hi}").unwrap();
        }
    }
    out
}

/// The stats CSV: a header line and one values line.
pub fn format_csv(stats: &StatsSnapshot, nics: &[(u16, NicStats)]) -> String {
    let mut out = String::from("rx_received,rx_ring_drops");
    out += &bucket_columns("rx");
    out += ",worker_received,worker_sent,worker_emulator_drops,worker_tx_ring_drops";
    out += ",tx_transmitted,tx_drops";
    out += &bucket_columns("tx");
    for (id, _) in nics {
        write!(
            out,
            ",nic_{id}_imissed,nic_{id}_ierrors,nic_{id}_oerrors,nic_{id}_rx_nombuf"
        )
        .unwrap();
    }
    out.push('\n');

    write!(out, "{},{}", stats.rx.received, stats.rx.rx_ring_drops).unwrap();
    for h in stats.rx.burst_hist {
        write!(out, ",{h}").unwrap();
    }
    write!(
        out,
        ",{},{},{},{}",
        stats.worker.received,
        stats.worker.sent,
        stats.worker.emulator_drops,
        stats.worker.tx_ring_drops
    )
    .unwrap();
    write!(out, ",{},{}", stats.tx.transmitted, stats.tx.tx_drops).unwrap();
    for h in stats.tx.burst_hist {
        write!(out, ",{h}").unwrap();
    }
    for (_, n) in nics {
        write!(
            out,
            ",{},{},{},{}",
            n.imissed, n.ierrors, n.oerrors, n.rx_nombuf
        )
        .unwrap();
    }
    out.push('\n');
    out
}
