//! Lock-free per-worker counters/gauges, per-epoch bookkeeping and the NIC
//! epoch tracker (port of preof's `counters.rs`, DPDS counters only).
//!
//! Without the `telemetry` feature every type here except [`EpochBoundary`],
//! [`EpochRecord`] and [`NicEpochRecord`] is a zero-sized unit struct whose
//! methods are no-ops.

#[cfg(feature = "telemetry")]
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::first_loss::{
    format_first_loss, FirstLossFields, FirstLossReason, StageLossDetail, StageLossEvent,
    StageLossTracker,
};
use crate::reply::{Reply, Value};

/// One accumulate-forever counter. Single writer for its whole lifetime: an
/// update is a relaxed load followed by a relaxed store (a non-atomic
/// read-modify-write, race-free because there is only one writer), never an
/// atomic RMW. The telemetry thread only reads.
#[cfg(feature = "telemetry")]
#[derive(Default)]
pub struct Counter(AtomicU64);
#[cfg(not(feature = "telemetry"))]
#[derive(Default)]
pub struct Counter;

impl Counter {
    #[inline(always)]
    pub fn inc(&self) {
        self.add(1);
    }

    /// `add(0)` is a true no-op (no write at all): the fast path calls it
    /// unconditionally with burst-sized counts that are often zero.
    #[inline(always)]
    pub fn add(&self, n: u64) {
        #[cfg(feature = "telemetry")]
        if n > 0 {
            let cur = self.0.load(Ordering::Relaxed);
            self.0.store(cur + n, Ordering::Relaxed);
        }
        #[cfg(not(feature = "telemetry"))]
        let _ = n;
    }

    /// Always 0 without the feature.
    #[inline(always)]
    pub fn get(&self) -> u64 {
        #[cfg(feature = "telemetry")]
        {
            self.0.load(Ordering::Relaxed)
        }
        #[cfg(not(feature = "telemetry"))]
        {
            0
        }
    }
}

/// A running high-water mark, same single-writer discipline as [`Counter`].
#[cfg(feature = "telemetry")]
#[derive(Default)]
pub struct MaxGauge(AtomicU64);
#[cfg(not(feature = "telemetry"))]
#[derive(Default)]
pub struct MaxGauge;

impl MaxGauge {
    /// Raises the mark if `v` is larger; never decreases it.
    #[inline(always)]
    pub fn observe(&self, v: u64) {
        #[cfg(feature = "telemetry")]
        {
            let cur = self.0.load(Ordering::Relaxed);
            if v > cur {
                self.0.store(v, Ordering::Relaxed);
            }
        }
        #[cfg(not(feature = "telemetry"))]
        let _ = v;
    }

    /// Lazy [`observe`](Self::observe): with the feature, evaluates `f` once
    /// and observes it; without it `f` is never called, so the sampling work
    /// disappears at the source level instead of relying on the optimizer.
    #[inline(always)]
    pub fn observe_with(&self, f: impl FnOnce() -> u64) {
        #[cfg(feature = "telemetry")]
        self.observe(f());
        #[cfg(not(feature = "telemetry"))]
        let _ = f;
    }

    /// Current mark; 0 without the feature or before anything was observed.
    #[inline(always)]
    pub fn get(&self) -> u64 {
        #[cfg(feature = "telemetry")]
        {
            self.0.load(Ordering::Relaxed)
        }
        #[cfg(not(feature = "telemetry"))]
        {
            0
        }
    }

    /// Returns the current mark and resets it to 0: called once per epoch
    /// boundary so gauges are per-epoch maxima.
    #[inline(always)]
    pub fn take(&self) -> u64 {
        #[cfg(feature = "telemetry")]
        {
            let cur = self.0.load(Ordering::Relaxed);
            self.0.store(0, Ordering::Relaxed);
            cur
        }
        #[cfg(not(feature = "telemetry"))]
        {
            0
        }
    }
}

/// `total / count` with 2 decimals, `"0.00"` for `count == 0`. DPDK
/// telemetry dicts have no float setter, so averages cross the wire as
/// strings.
pub fn mean_2dp(total: u64, count: u64) -> String {
    if count == 0 {
        "0.00".to_string()
    } else {
        format!("{:.2}", total as f64 / count as f64)
    }
}

/// The data-plane worker's counters (one instance, one writer).
/// `#[repr(align(64))]` keeps it off the cache lines of its neighbours.
#[repr(align(64))]
#[derive(Default)]
pub struct DpdsCounters {
    pub polls: Counter,
    pub rx_pkts: Counter,
    /// `rx_burst` returned a full `MAX_BURST_SIZE` batch: intake-limited.
    pub rx_full_bursts: Counter,
    pub emulator_rejects: Counter,
    pub pops: Counter,
    pub tx_calls: Counter,
    pub tx_pkts: Counter,
    pub tx_sent: Counter,
    pub tx_short_calls: Counter,
    pub tx_batch_max: MaxGauge,
    pub buffer_len_max: MaxGauge,
}

impl DpdsCounters {
    /// Positional with [`cumulative_snapshot`](Self::cumulative_snapshot).
    pub const EPOCH_COUNTER_NAMES: [&'static str; 9] = [
        "polls",
        "rx_pkts",
        "rx_full_bursts",
        "emulator_rejects",
        "pops",
        "tx_calls",
        "tx_pkts",
        "tx_sent",
        "tx_short_calls",
    ];
    /// Positional with the `epoch_gauges` array fed to `EpochLog::close`.
    pub const EPOCH_GAUGE_NAMES: [&'static str; 2] = ["tx_batch_max", "buffer_len_max"];

    /// Current cumulative values in `EPOCH_COUNTER_NAMES` order.
    pub fn cumulative_snapshot(&self) -> [u64; 9] {
        [
            self.polls.get(),
            self.rx_pkts.get(),
            self.rx_full_bursts.get(),
            self.emulator_rejects.get(),
            self.pops.get(),
            self.tx_calls.get(),
            self.tx_pkts.get(),
            self.tx_sent.get(),
            self.tx_short_calls.get(),
        ]
    }
}

pub type DpdsEpochLog = EpochLog<9, 2>;

// ── Epochs ───────────────────────────────────────────────────────────────
//
// The cumulative counters blend a whole run together. Epochs segment them
// into traffic bursts, detected by the worker itself from its own arrival
// pattern (no cross-thread signal): an epoch closes after more than
// `EPOCH_IDLE_GAP_NS` without arrivals, provided it saw any.

/// Closed epochs kept per log; further closes are dropped and counted (the
/// oldest epochs are kept, never overwritten).
pub const EPOCH_CAPACITY: usize = 32;

/// Idle gap (ns) that closes an epoch: 500 ms, strictly greater than.
pub const EPOCH_IDLE_GAP_NS: f64 = 500_000_000.0;

/// `saw_arrival_this_epoch && now - last_arrival > EPOCH_IDLE_GAP_NS` with
/// the feature, always `false` without. An epoch that saw no arrival is never
/// closed, however long the gap, so startup idle cannot emit empty epochs.
#[cfg(feature = "telemetry")]
pub fn should_close_epoch(now_ns: f64, last_arrival_ns: f64, saw_arrival_this_epoch: bool) -> bool {
    saw_arrival_this_epoch && (now_ns - last_arrival_ns) > EPOCH_IDLE_GAP_NS
}
#[cfg(not(feature = "telemetry"))]
pub fn should_close_epoch(
    _now_ns: f64,
    _last_arrival_ns: f64,
    _saw_arrival_this_epoch: bool,
) -> bool {
    false
}

/// A closed epoch's uptime window. Not gated: call sites read its fields in
/// both builds (with the feature off `tick` just never returns one).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EpochBoundary {
    pub start_uptime_ns: f64,
    pub end_uptime_ns: f64,
}

/// The per-tick epoch-boundary state machine of one worker: call
/// [`tick`](Self::tick) once per loop iteration with that iteration's
/// arrival count; on `Some(boundary)` snapshot the counters into the
/// [`EpochLog`]. A zero-sized no-op without the feature.
#[cfg(feature = "telemetry")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EpochDriver {
    pub last_arrival_ns: f64,
    pub saw_arrival_this_epoch: bool,
    pub epoch_start_uptime_ns: f64,
}
#[cfg(not(feature = "telemetry"))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EpochDriver;

#[cfg(feature = "telemetry")]
impl EpochDriver {
    /// No arrivals seen yet; the first epoch starts at `start_uptime_ns`.
    pub fn new(start_uptime_ns: f64) -> Self {
        EpochDriver {
            last_arrival_ns: start_uptime_ns,
            saw_arrival_this_epoch: false,
            epoch_start_uptime_ns: start_uptime_ns,
        }
    }

    /// An arrival tick (`arrivals > 0`) records the arrival and never closes.
    /// An idle tick closes the epoch iff [`should_close_epoch`]; the next
    /// epoch then starts exactly where this one ended and is "empty" again,
    /// so a run of idle ticks cannot emit a run of empty epochs.
    pub fn tick(&mut self, now_ns: f64, arrivals: u64) -> Option<EpochBoundary> {
        if arrivals > 0 {
            self.last_arrival_ns = now_ns;
            self.saw_arrival_this_epoch = true;
            return None;
        }

        if should_close_epoch(now_ns, self.last_arrival_ns, self.saw_arrival_this_epoch) {
            let boundary = EpochBoundary {
                start_uptime_ns: self.epoch_start_uptime_ns,
                end_uptime_ns: now_ns,
            };
            self.saw_arrival_this_epoch = false;
            self.epoch_start_uptime_ns = now_ns;
            Some(boundary)
        } else {
            None
        }
    }
}

#[cfg(not(feature = "telemetry"))]
impl EpochDriver {
    #[inline(always)]
    pub fn new(_start_uptime_ns: f64) -> Self {
        EpochDriver
    }

    #[inline(always)]
    pub fn tick(&mut self, _now_ns: f64, _arrivals: u64) -> Option<EpochBoundary> {
        None
    }
}

/// Per-epoch first-loss scratch, worker-owned (no atomics): the epoch
/// counterpart of the whole-process [`StageLossTracker`], which only ever
/// reports the very first loss of the run.
#[cfg(feature = "telemetry")]
#[derive(Default)]
pub struct EpochFirstLoss {
    /// This epoch's first event, reset by [`take`](Self::take).
    current: Option<StageLossEvent>,
}
#[cfg(not(feature = "telemetry"))]
#[derive(Default)]
pub struct EpochFirstLoss;

#[cfg(feature = "telemetry")]
impl EpochFirstLoss {
    /// Same as `default()`, but also usable on the unit-struct twin without
    /// tripping clippy's `default_constructed_unit_structs`.
    pub fn new() -> Self {
        EpochFirstLoss { current: None }
    }

    /// Always forwards to `tracker.try_record(..)` (the whole-process latch
    /// is unchanged) and additionally latches the event for this epoch if
    /// none is latched yet.
    pub fn record(
        &mut self,
        tracker: &StageLossTracker,
        reason: FirstLossReason,
        detail: StageLossDetail,
        relative_ns: f64,
    ) {
        let _ = tracker.try_record(reason, detail, relative_ns);
        if self.current.is_none() {
            self.current = Some(StageLossEvent {
                reason,
                detail,
                relative_ns,
            });
        }
    }

    /// Returns this epoch's event (`None` = Clean) and resets the scratch.
    pub fn take(&mut self) -> Option<StageLossEvent> {
        self.current.take()
    }
}

#[cfg(not(feature = "telemetry"))]
impl EpochFirstLoss {
    #[inline(always)]
    pub fn new() -> Self {
        EpochFirstLoss
    }

    #[inline(always)]
    pub fn record(
        &mut self,
        tracker: &StageLossTracker,
        reason: FirstLossReason,
        detail: StageLossDetail,
        relative_ns: f64,
    ) {
        let _ = tracker.try_record(reason, detail, relative_ns);
    }

    #[inline(always)]
    pub fn take(&mut self) -> Option<StageLossEvent> {
        None
    }
}

/// One closed epoch: counter *deltas* (same order as the cumulative values
/// fed to [`EpochLog::close`]), per-epoch gauge maxima and the uptime window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochRecord<const NC: usize, const NG: usize> {
    pub start_uptime_ns: u64,
    pub end_uptime_ns: u64,
    pub counters: [u64; NC],
    pub gauges: [u64; NG],
}

/// Append-only log of closed epochs, `EPOCH_CAPACITY` deep, oldest kept.
/// Single writer / concurrent lock-free readers: [`close`](Self::close)
/// fills slot `n` completely, then publishes it with a `Release` store to
/// `epochs_closed`; readers do one `Acquire` load of that count and only
/// read slots below it. `#[repr(align(64))]` with the feature; a zero-sized
/// no-op without it.
#[cfg(feature = "telemetry")]
#[repr(align(64))]
pub struct EpochLog<const NC: usize, const NG: usize> {
    slots: std::cell::UnsafeCell<[EpochRecord<NC, NG>; EPOCH_CAPACITY]>,
    // Published under the same `Release` store as `slots`.
    first_loss: std::cell::UnsafeCell<[Option<StageLossEvent>; EPOCH_CAPACITY]>,
    epochs_closed: AtomicU32,
    epochs_dropped: AtomicU32,
    // Previous call's cumulative counters, diffed into the next delta.
    prev_cumulative: std::cell::UnsafeCell<[u64; NC]>,
    // Gauge peaks of epochs dropped past capacity: counters of a dropped
    // epoch are discarded, but a gauge peak still counts toward the
    // whole-process maximum. Read by concurrent readers without a publish
    // barrier, hence real atomics.
    overflow_gauge_max: [AtomicU64; NG],
}

// SAFETY: `slots`, `first_loss` and `prev_cumulative` are only written by
// `close`, which the design requires to be called from exactly one thread
// (the owning worker). Readers only read a slot after an `Acquire` load of
// `epochs_closed` that happens-after the writer's matching `Release` store.
#[cfg(feature = "telemetry")]
unsafe impl<const NC: usize, const NG: usize> Sync for EpochLog<NC, NG> {}

#[cfg(feature = "telemetry")]
impl<const NC: usize, const NG: usize> EpochLog<NC, NG> {
    pub fn new() -> Self {
        let empty = EpochRecord {
            start_uptime_ns: 0,
            end_uptime_ns: 0,
            counters: [0u64; NC],
            gauges: [0u64; NG],
        };
        EpochLog {
            slots: std::cell::UnsafeCell::new([empty; EPOCH_CAPACITY]),
            first_loss: std::cell::UnsafeCell::new([None; EPOCH_CAPACITY]),
            epochs_closed: AtomicU32::new(0),
            epochs_dropped: AtomicU32::new(0),
            prev_cumulative: std::cell::UnsafeCell::new([0u64; NC]),
            overflow_gauge_max: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    pub fn epochs_closed(&self) -> u32 {
        self.epochs_closed.load(Ordering::Acquire)
    }

    /// Closes past `EPOCH_CAPACITY` that were dropped.
    pub fn epochs_dropped(&self) -> u32 {
        self.epochs_dropped.load(Ordering::Relaxed)
    }

    /// Closes the open epoch. `cumulative_counters` are running totals,
    /// diffed (saturating) against the previous call's, so the first epoch
    /// is a delta from zero. `epoch_gauges` are this epoch's own maxima (the
    /// caller resets its gauges each epoch). Once `EPOCH_CAPACITY` epochs are
    /// stored, further calls only count `epochs_dropped` and fold the gauge
    /// peaks into the overflow maximum.
    pub fn close(
        &self,
        cumulative_counters: [u64; NC],
        epoch_gauges: [u64; NG],
        start_uptime_ns: u64,
        end_uptime_ns: u64,
        first_loss: Option<StageLossEvent>,
    ) {
        let closed = self.epochs_closed.load(Ordering::Relaxed) as usize;
        if closed >= EPOCH_CAPACITY {
            let dropped = self.epochs_dropped.load(Ordering::Relaxed);
            self.epochs_dropped.store(dropped + 1, Ordering::Relaxed);
            for (peak, g) in self.overflow_gauge_max.iter().zip(epoch_gauges) {
                if g > peak.load(Ordering::Relaxed) {
                    peak.store(g, Ordering::Relaxed);
                }
            }
            return;
        }

        // SAFETY: single writer, see the `Sync` impl above.
        let prev = unsafe { &mut *self.prev_cumulative.get() };
        let mut deltas = [0u64; NC];
        for i in 0..NC {
            deltas[i] = cumulative_counters[i].saturating_sub(prev[i]);
        }
        *prev = cumulative_counters;

        let record = EpochRecord {
            start_uptime_ns,
            end_uptime_ns,
            counters: deltas,
            gauges: epoch_gauges,
        };
        // SAFETY: single writer; readers only look below `epochs_closed`.
        unsafe {
            (*self.slots.get())[closed] = record;
            (*self.first_loss.get())[closed] = first_loss;
        }

        self.epochs_closed
            .store((closed + 1) as u32, Ordering::Release);
    }

    /// The closed epoch at `index` (0 = oldest), `None` if not published.
    pub fn record(&self, index: usize) -> Option<EpochRecord<NC, NG>> {
        let closed = self.epochs_closed.load(Ordering::Acquire) as usize;
        if index < closed {
            // SAFETY: `index < closed` was observed via an `Acquire` load,
            // which happens-after the writer's `Release` store of that slot.
            Some(unsafe { (*self.slots.get())[index] })
        } else {
            None
        }
    }

    /// Outer `None`: `index` not published; `Some(None)`: published, Clean.
    pub fn first_loss(&self, index: usize) -> Option<Option<StageLossEvent>> {
        let closed = self.epochs_closed.load(Ordering::Acquire) as usize;
        if index < closed {
            // SAFETY: as in `record`.
            Some(unsafe { (*self.first_loss.get())[index] })
        } else {
            None
        }
    }

    /// Max of gauge `gauge_index` over all closed epochs, the dropped
    /// epochs' peaks and `live_value` (the not-yet-closed epoch's reading):
    /// keeps whole-process gauges such as `buffer_len_max` meaningful even
    /// though the gauges themselves reset every epoch.
    pub fn aggregate_gauge_max(&self, gauge_index: usize, live_value: u64) -> u64 {
        let closed = self.epochs_closed.load(Ordering::Acquire) as usize;
        let mut max = live_value;
        for i in 0..closed {
            // SAFETY: as in `record`.
            let v = unsafe { (*self.slots.get())[i].gauges[gauge_index] };
            max = max.max(v);
        }
        max.max(self.overflow_gauge_max[gauge_index].load(Ordering::Relaxed))
    }
}

#[cfg(not(feature = "telemetry"))]
pub struct EpochLog<const NC: usize, const NG: usize>;

#[cfg(not(feature = "telemetry"))]
impl<const NC: usize, const NG: usize> EpochLog<NC, NG> {
    pub fn new() -> Self {
        EpochLog
    }

    pub fn epochs_closed(&self) -> u32 {
        0
    }

    pub fn epochs_dropped(&self) -> u32 {
        0
    }

    pub fn close(
        &self,
        _cumulative_counters: [u64; NC],
        _epoch_gauges: [u64; NG],
        _start_uptime_ns: u64,
        _end_uptime_ns: u64,
        _first_loss: Option<StageLossEvent>,
    ) {
    }

    pub fn record(&self, _index: usize) -> Option<EpochRecord<NC, NG>> {
        None
    }

    pub fn first_loss(&self, _index: usize) -> Option<Option<StageLossEvent>> {
        None
    }

    pub fn aggregate_gauge_max(&self, _gauge_index: usize, live_value: u64) -> u64 {
        live_value
    }
}

impl<const NC: usize, const NG: usize> Default for EpochLog<NC, NG> {
    fn default() -> Self {
        Self::new()
    }
}

/// The `/*/epochs` wire reply of `log`: `epochs_closed`, `epochs_dropped`,
/// `epoch_start_uptime_ns`, `epoch_end_uptime_ns`, `first_loss_reason`,
/// `first_loss_detail`, `first_loss_relative_ns`, then one uint array per
/// counter name and per gauge name, in that order. Columnar because DPDK's
/// `rte_tel_data_add_array_container` only takes arrays of uint64/int/string,
/// never of objects.
pub fn epochs_reply<const NC: usize, const NG: usize>(
    log: &EpochLog<NC, NG>,
    counter_names: &[&str; NC],
    gauge_names: &[&str; NG],
) -> Reply {
    let closed = log.epochs_closed() as usize;
    let records: Vec<EpochRecord<NC, NG>> = (0..closed).filter_map(|i| log.record(i)).collect();
    let fields: Vec<FirstLossFields> = (0..closed)
        .map(|i| format_first_loss(log.first_loss(i).flatten()))
        .collect();

    let mut reply: Reply = vec![
        ("epochs_closed".into(), Value::Uint(closed as u64)),
        (
            "epochs_dropped".into(),
            Value::Uint(log.epochs_dropped() as u64),
        ),
        (
            "epoch_start_uptime_ns".into(),
            Value::UintArray(records.iter().map(|r| r.start_uptime_ns).collect()),
        ),
        (
            "epoch_end_uptime_ns".into(),
            Value::UintArray(records.iter().map(|r| r.end_uptime_ns).collect()),
        ),
        (
            "first_loss_reason".into(),
            Value::StrArray(fields.iter().map(|f| f.reason.clone()).collect()),
        ),
        (
            "first_loss_detail".into(),
            Value::StrArray(fields.iter().map(|f| f.detail.clone()).collect()),
        ),
        (
            "first_loss_relative_ns".into(),
            Value::UintArray(fields.iter().map(|f| f.relative_ns).collect()),
        ),
    ];
    for (i, name) in counter_names.iter().enumerate() {
        reply.push((
            (*name).into(),
            Value::UintArray(records.iter().map(|r| r.counters[i]).collect()),
        ));
    }
    for (i, name) in gauge_names.iter().enumerate() {
        reply.push((
            (*name).into(),
            Value::UintArray(records.iter().map(|r| r.gauges[i]).collect()),
        ));
    }
    reply
}

// ── NIC epochs (main lcore) ──────────────────────────────────────────────
//
// Same per-trial segmentation for the NIC hardware counters, driven by the
// main lcore's 250 ms poll of `rx_packets` and fed into a per-port
// `NicEpochLog`, so a worker epoch's window can be matched to the NIC
// epoch overlapping it.

/// Not gated, like [`EpochBoundary`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NicEpochRecord {
    pub start_uptime_ns: f64,
    pub end_uptime_ns: f64,
    pub rx_packets: u64,
    pub imissed: u64,
    pub rx_port_discards: u64,
}

/// One port's epoch log: counters only, a NIC port has no gauges.
pub type NicEpochLog = EpochLog<3, 0>;

impl NicEpochLog {
    /// Column names, in the order [`NicEpochTracker`] feeds
    /// [`NicEpochRecord`]'s fields into the log.
    pub const EPOCH_COUNTER_NAMES: [&'static str; 3] =
        ["rx_packets", "imissed", "rx_port_discards"];
    pub const EPOCH_GAUGE_NAMES: [&'static str; 0] = [];
}

/// Drives one port's [`EpochDriver`] from the main lcore's periodic poll,
/// turning the cumulative `rx_packets` arrival stream into epoch boundaries.
/// Every recorded field is a delta since the previous close (`saturating_sub`
/// so a counter reset reports 0). Keeps no history: callers feed the returned
/// record into the port's [`NicEpochLog`]. A zero-sized no-op without the
/// feature.
#[cfg(feature = "telemetry")]
pub struct NicEpochTracker {
    driver: EpochDriver,
    /// Last cumulative `rx_packets`, to derive this poll's arrivals.
    last_rx_packets_cum: u64,
    /// Cumulative readings at the previous close (or construction).
    baseline_rx_packets: u64,
    baseline_imissed: u64,
    baseline_rx_port_discards: u64,
}

#[cfg(feature = "telemetry")]
impl NicEpochTracker {
    /// Seeds the first epoch's baselines with the current cumulative values.
    pub fn new(
        stage_start_ns: f64,
        rx_packets_cum: u64,
        imissed_cum: u64,
        rx_port_discards_cum: u64,
    ) -> Self {
        NicEpochTracker {
            driver: EpochDriver::new(stage_start_ns),
            last_rx_packets_cum: rx_packets_cum,
            baseline_rx_packets: rx_packets_cum,
            baseline_imissed: imissed_cum,
            baseline_rx_port_discards: rx_port_discards_cum,
        }
    }

    /// One main-lcore poll. `read_port_discards` (the discard-xstats sum) is
    /// called ONLY on the poll that closes an epoch: frequent firmware
    /// xstats queries are suspected of amplifying NIC stalls. `None` from it
    /// (a failed read) reports a 0 delta without advancing the discard
    /// baseline, so the next successful read catches up.
    pub fn poll(
        &mut self,
        now_ns: f64,
        rx_packets_cum: u64,
        imissed_cum: u64,
        read_port_discards: impl FnOnce() -> Option<u64>,
    ) -> Option<NicEpochRecord> {
        let arrivals = rx_packets_cum.saturating_sub(self.last_rx_packets_cum);
        self.last_rx_packets_cum = rx_packets_cum;

        let boundary = self.driver.tick(now_ns, arrivals)?;

        let rx_packets = rx_packets_cum.saturating_sub(self.baseline_rx_packets);
        let imissed = imissed_cum.saturating_sub(self.baseline_imissed);
        let rx_port_discards = match read_port_discards() {
            Some(cum) => {
                let delta = cum.saturating_sub(self.baseline_rx_port_discards);
                self.baseline_rx_port_discards = cum;
                delta
            }
            None => 0,
        };

        self.baseline_rx_packets = rx_packets_cum;
        self.baseline_imissed = imissed_cum;

        Some(NicEpochRecord {
            start_uptime_ns: boundary.start_uptime_ns,
            end_uptime_ns: boundary.end_uptime_ns,
            rx_packets,
            imissed,
            rx_port_discards,
        })
    }
}

#[cfg(not(feature = "telemetry"))]
pub struct NicEpochTracker;

#[cfg(not(feature = "telemetry"))]
impl NicEpochTracker {
    #[inline(always)]
    pub fn new(
        _stage_start_ns: f64,
        _rx_packets_cum: u64,
        _imissed_cum: u64,
        _rx_port_discards_cum: u64,
    ) -> Self {
        NicEpochTracker
    }

    #[inline(always)]
    pub fn poll(
        &mut self,
        _now_ns: f64,
        _rx_packets_cum: u64,
        _imissed_cum: u64,
        _read_port_discards: impl FnOnce() -> Option<u64>,
    ) -> Option<NicEpochRecord> {
        None
    }
}
