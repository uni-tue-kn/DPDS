//! Telemetry glue: the per-direction [`Stats`], the worker's [`Telemetry`] handle,
//! the reply builders behind `/dpds/stats`, `/dpds/epochs`, `/dpds/ports`,
//! `/nic/hw_first_loss` and `/nic/epochs`, their registration on DPDK's
//! telemetry socket, and the main lcore's [`NicPoller`] (port of preof's
//! `stats.rs` plus the NIC pieces of its `main.rs`).
//!
//! Registration is a no-op without the `telemetry` feature. The reply fields are
//! documented on the counter, epoch and first-loss types (`counters.rs`, `first_loss.rs`).

use crate::counters::{
    epochs_reply, DpdsCounters, DpdsEpochLog, EpochDriver, EpochFirstLoss, FrameSizes, NicEpochLog,
    NicEpochTracker,
};
use crate::delay_stats::{DelayStats, HistRange};
use crate::first_loss::{
    detect_hardware_drop, format_first_loss, FirstLossReason, Stage, StageLossDetail,
    StageLossTracker,
};
use crate::reply::{Reply, Value};

/// Everything the worker lcore writes and the telemetry thread reads.
pub struct Stats {
    pub first_loss: StageLossTracker,
    pub counters: DpdsCounters,
    /// Received frames per size bucket, cumulative.
    pub frame_sizes: FrameSizes,
    pub epochs: DpdsEpochLog,
    /// Delay (arrival until enqueued into the TX descriptors) / lateness statistics and the delay histogram.
    pub delay: DelayStats,
}

impl Default for Stats {
    fn default() -> Self {
        Stats::with_range(HistRange::default())
    }
}

fn uint(key: &str, v: u64) -> (String, Value) {
    (key.to_string(), Value::Uint(v))
}

fn string(key: &str, v: impl Into<String>) -> (String, Value) {
    (key.to_string(), Value::Str(v.into()))
}

impl Stats {
    /// Stats whose delay histogram covers `range`.
    pub fn with_range(range: HistRange) -> Stats {
        Stats::with_delay_stats(range, true)
    }

    /// As [`with_range`](Self::with_range); with `delay_stats` off the
    /// direction records no delay statistics, its `/dpds/stats` has no delay
    /// keys and its histogram reply is empty.
    pub fn with_delay_stats(range: HistRange, delay_stats: bool) -> Stats {
        Stats::with_hists(range, HistRange::default(), delay_stats)
    }

    /// As [`with_delay_stats`](Self::with_delay_stats), with the range of the
    /// IAT histogram (recorded exactly when `delay_stats` is on).
    pub fn with_hists(range: HistRange, iat_range: HistRange, delay_stats: bool) -> Stats {
        Stats {
            first_loss: StageLossTracker::new(),
            counters: DpdsCounters::default(),
            frame_sizes: FrameSizes::default(),
            epochs: DpdsEpochLog::default(),
            delay: DelayStats::with_hists(range, iat_range, delay_stats),
        }
    }

    /// Page 0 of the `/dpds/iat_hist` reply: the cumulative histogram of the
    /// gaps between consecutive send times.
    pub fn iat_hist_reply(&self) -> Reply {
        self.delay.iat_hist_page(0).unwrap_or_default()
    }

    /// Page 0 of the `/dpds/delay_hist` reply: the cumulative histogram of the
    /// delay of the packets the NIC accepted (see [`delay_hist_page`](Self::delay_hist_page)).
    pub fn delay_hist_reply(&self) -> Reply {
        self.delay.hist_page(0).unwrap_or_default()
    }

    /// Page `page` of the `/dpds/delay_hist` reply, `None` past the last page.
    pub fn delay_hist_page(&self, page: usize) -> Option<Reply> {
        self.delay.hist_page(page)
    }

    /// The `/dpds/stats` reply: cumulative since start. Gauges are the max
    /// over closed epochs and the live (open) epoch.
    pub fn stats_reply(&self, uptime_ns: u64) -> Reply {
        let c = &self.counters;
        let tx_pkts = c.tx_pkts.get();
        let tx_sent = c.tx_sent.get();
        let fields = format_first_loss(self.first_loss.current());
        let mut reply = vec![
            uint("uptime_ns", uptime_ns),
            uint("polls", c.polls.get()),
            uint("rx_pkts", c.rx_pkts.get()),
            uint("rx_full_bursts", c.rx_full_bursts.get()),
            uint("dpds_rejects", c.dpds_rejects.get()),
            uint("loss_drops", c.loss_drops.get()),
            uint("limiter_drops", c.limiter_drops.get()),
            uint("buffer_full_drops", c.buffer_full_drops.get()),
            uint("pops", c.pops.get()),
            uint("tx_calls", c.tx_calls.get()),
            uint("tx_pkts", tx_pkts),
            uint("tx_sent", tx_sent),
            uint("tx_drops", tx_pkts.saturating_sub(tx_sent)),
            uint("tx_short_calls", c.tx_short_calls.get()),
            uint(
                "tx_batch_max",
                self.epochs.aggregate_gauge_max(0, c.tx_batch_max.get()),
            ),
            string(
                "tx_pkts_per_call",
                crate::counters::mean_2dp(tx_pkts, c.tx_calls.get()),
            ),
            uint(
                "buffer_len_max",
                self.epochs.aggregate_gauge_max(1, c.buffer_len_max.get()),
            ),
            (
                "frame_sizes".to_string(),
                Value::UintArray(self.frame_sizes.get()),
            ),
            uint("out_of_order", c.out_of_order.get()),
            string("first_loss_reason", fields.reason),
            string("first_loss_detail", fields.detail),
            uint("first_loss_relative_ns", fields.relative_ns),
        ];
        reply.extend(self.delay.stats_fields());
        reply
    }

    /// The `/dpds/epochs` reply.
    pub fn epochs_reply(&self) -> Reply {
        epochs_reply(
            &self.epochs,
            &DpdsCounters::EPOCH_COUNTER_NAMES,
            &DpdsCounters::EPOCH_GAUGE_NAMES,
        )
    }
}

/// The [`Stats`] of every direction, indexed by direction (= RX port).
pub struct DirectionStats {
    pub stats: Vec<Stats>,
}

/// The direction index a `,<param>` query names: a plain decimal number.
/// Also the port index of the `/nic/*` commands.
fn index_param(param: &str) -> Option<usize> {
    if param.is_empty() || !param.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    param.parse().ok()
}

/// `<dir>` or `<dir>,<page>`, plain decimal numbers.
fn dir_and_page(param: &str) -> Option<(usize, usize)> {
    let mut parts = param.split(',');
    let dir = index_param(parts.next()?)?;
    let page = parts.next().map_or(Some(0), index_param)?;
    if parts.next().is_some() {
        return None;
    }
    Some((dir, page))
}

impl DirectionStats {
    pub fn new(num_directions: usize) -> DirectionStats {
        DirectionStats::with_ranges(vec![HistRange::default(); num_directions])
    }

    /// One direction per range, each with its own delay histogram range.
    pub fn with_ranges(ranges: Vec<HistRange>) -> DirectionStats {
        DirectionStats::with_delay_settings(ranges.into_iter().map(|r| (r, true)).collect())
    }

    /// One direction per `(range, delay_stats)`.
    pub fn with_delay_settings(settings: Vec<(HistRange, bool)>) -> DirectionStats {
        DirectionStats::with_hists(
            settings
                .into_iter()
                .map(|(range, on)| (range, HistRange::default(), on))
                .collect(),
        )
    }

    /// One direction per `(delay range, IAT range, delay_stats)`.
    pub fn with_hists(settings: Vec<(HistRange, HistRange, bool)>) -> DirectionStats {
        DirectionStats {
            stats: settings
                .into_iter()
                .map(|(range, iat, on)| Stats::with_hists(range, iat, on))
                .collect(),
        }
    }

    /// `/dpds/stats,<param>`: the stats reply of that direction. `None` for a
    /// param that is not the decimal index of an existing direction.
    pub fn stats_reply(&self, param: &str, uptime_ns: u64) -> Option<Reply> {
        Some(self.stats.get(index_param(param)?)?.stats_reply(uptime_ns))
    }

    /// `/dpds/epochs,<param>`: the epochs reply of that direction. `None` for
    /// a bad param.
    pub fn epochs_reply(&self, param: &str) -> Option<Reply> {
        Some(self.stats.get(index_param(param)?)?.epochs_reply())
    }

    /// `/dpds/delay_hist,<param>`: one page of the delay histogram of a
    /// direction. `<param>` is `<dir>` (page 0) or `<dir>,<page>`, both plain
    /// decimal numbers. `None` for a bad param, a direction that does not exist
    /// or a page past the last one.
    pub fn delay_hist_reply(&self, param: &str) -> Option<Reply> {
        let (dir, page) = dir_and_page(param)?;
        self.stats.get(dir)?.delay_hist_page(page)
    }

    /// `/dpds/iat_hist,<param>`: like [`delay_hist_reply`](Self::delay_hist_reply),
    /// for the histogram of the gaps between consecutive send times.
    pub fn iat_hist_reply(&self, param: &str) -> Option<Reply> {
        let (dir, page) = dir_and_page(param)?;
        self.stats.get(dir)?.delay.iat_hist_page(page)
    }

    /// Registers `/dpds/stats`, `/dpds/epochs`, `/dpds/delay_hist` and `/dpds/iat_hist` (no-op
    /// without the feature). `start_ns` is the shared `now_ns()` zero point. A bad param
    /// yields an empty dict, like `/nic/epochs`.
    #[cfg(not(feature = "telemetry"))]
    pub fn register_telemetry(
        _stats: &'static DirectionStats,
        _start_ns: f64,
    ) -> Result<(), dpdk::DpdkError> {
        Ok(())
    }

    #[cfg(feature = "telemetry")]
    pub fn register_telemetry(
        stats: &'static DirectionStats,
        start_ns: f64,
    ) -> Result<(), dpdk::DpdkError> {
        dpdk::telemetry::register_command(
            "/dpds/stats",
            "DPDS counters + first loss of one direction, cumulative (query with ,0 / ,1)",
            move |param, d| {
                let uptime = (dpdk::now_ns() - start_ns).max(0.0) as u64;
                if let Some(reply) = stats.stats_reply(param, uptime) {
                    fill(d, reply);
                }
            },
        )?;
        dpdk::telemetry::register_command(
            "/dpds/epochs",
            "DPDS per-epoch counter/gauge history of one direction (query with ,0 / ,1)",
            move |param, d| {
                if let Some(reply) = stats.epochs_reply(param) {
                    fill(d, reply);
                }
            },
        )?;
        dpdk::telemetry::register_command(
            "/dpds/delay_hist",
            "One page of the cumulative delay histogram of one direction (query with ,<dir>,<page>)",
            move |param, d| {
                if let Some(reply) = stats.delay_hist_reply(param) {
                    fill(d, reply);
                }
            },
        )?;
        dpdk::telemetry::register_command(
            "/dpds/iat_hist",
            "One page of the cumulative histogram of the departure gaps (IAT) of one direction (query with ,<dir>,<page>)",
            move |param, d| {
                if let Some(reply) = stats.iat_hist_reply(param) {
                    fill(d, reply);
                }
            },
        )
    }
}

/// Copies a [`Reply`] into the telemetry thread's dict, entry by entry.
#[cfg(feature = "telemetry")]
fn fill(d: &mut dpdk::telemetry::Dict, reply: Reply) {
    use dpdk::telemetry::Container;
    for (key, value) in reply {
        match value {
            Value::Uint(v) => d.add(&key, v),
            Value::Str(v) => d.add_str(&key, &v),
            Value::UintArray(vs) => {
                let mut c = Container::new_uint_array();
                if let Some(e) = vs.iter().find_map(|&v| c.add_uint(v).err()) {
                    // leave the key out rather than send a cut array
                    eprintln!(
                        "telemetry array {key:?} of {} entries dropped: {e}",
                        vs.len()
                    );
                    continue;
                }
                d.add_array_container(&key, c);
            }
            Value::StrArray(vs) => {
                let mut c = Container::new_string_array();
                for v in &vs {
                    c.add_string(v);
                }
                d.add_array_container(&key, c);
            }
        }
    }
}

/// The worker's handle on its telemetry: the shared [`Stats`] plus the
/// worker-owned (non-atomic) epoch state. Zero-cost without the feature:
/// the driver and scratch are unit structs and everything below compiles
/// to nothing but the (no-op) counter calls.
pub struct Telemetry<'a> {
    pub stats: &'a Stats,
    start_ns: f64,
    driver: EpochDriver,
    epoch_first_loss: EpochFirstLoss,
}

impl<'a> Telemetry<'a> {
    pub fn new(stats: &'a Stats, start_ns: f64) -> Telemetry<'a> {
        Telemetry {
            stats,
            start_ns,
            // The driver runs in uptime (relative to `start_ns`), so the
            // first epoch starts at 0 and `epoch_*_uptime_ns` compare with
            // `uptime_ns` and `first_loss_relative_ns`.
            driver: EpochDriver::new(0.0),
            epoch_first_loss: EpochFirstLoss::new(),
        }
    }

    /// Records a loss: the whole-process first-loss latch plus this epoch's.
    pub fn record_loss(&mut self, reason: FirstLossReason, detail: StageLossDetail, now_ns: f64) {
        self.epoch_first_loss.record(
            &self.stats.first_loss,
            reason,
            detail,
            now_ns - self.start_ns,
        );
    }

    /// One loop iteration (`arrivals` = packets received in it): advances the
    /// epoch state machine and, on a boundary, closes an epoch into
    /// `stats.epochs` with the cumulative counters, this epoch's gauge
    /// maxima (resetting the gauges) and its first loss.
    pub fn tick(&mut self, now_ns: f64, arrivals: u64) {
        if let Some(boundary) = self.driver.tick(now_ns - self.start_ns, arrivals) {
            let c = &self.stats.counters;
            self.stats.epochs.close(
                c.cumulative_snapshot(),
                [c.tx_batch_max.take(), c.buffer_len_max.take()],
                boundary.start_uptime_ns as u64,
                boundary.end_uptime_ns as u64,
                self.epoch_first_loss.take(),
            );
        }
    }
}

/// Static info of one port for `/dpds/ports`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    pub name: String,
    /// Descriptor ring sizes as applied by the driver.
    pub rx_ring: u16,
    pub tx_ring: u16,
    /// Descriptor ring sizes as requested before the driver clamped them.
    pub rx_ring_requested: u16,
    pub tx_ring_requested: u16,
}

/// Static info of one link (= direction) for `/dpds/ports`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectionInfo {
    pub rx_port: u16,
    pub tx_port: u16,
    /// EAL lcore of the direction's worker.
    pub lcore: u32,
    /// Where the direction's config comes from: the config file, or the links
    /// file.
    pub config: String,
    /// Mbufs in the direction's mempool.
    pub pool_size: u32,
    /// Whether the direction frees mbufs with the raw bulk free.
    pub raw_free: bool,
    /// Name of the link the direction belongs to (`default` without `--links`).
    pub link: String,
    /// DPDK device names of the receiving and the transmitting port.
    pub rx_name: String,
    pub tx_name: String,
    /// Whether the direction records delay statistics.
    pub delay_stats: bool,
}

/// The `/dpds/ports` reply: `num_ports`, then per port index `i`
/// `port<i>_name` (string), `port<i>_rx_ring`, `port<i>_tx_ring`,
/// `port<i>_rx_ring_requested`, `port<i>_tx_ring_requested`; then
/// `num_directions`, `num_links` and per direction `d` `dir<d>_rx_port`,
/// `dir<d>_tx_port`, `dir<d>_lcore`, `dir<d>_config` (string),
/// `dir<d>_pool_size`, `dir<d>_raw_free` (0 or 1), `dir<d>_link` (string, the
/// link name; `d` is the link's index), `dir<d>_rx_name` and `dir<d>_tx_name` (strings, DPDK device
/// names), `dir<d>_delay_stats` (0 or 1: the direction's delay statistics are
/// off or on). That is `3 + 5 * ports + 10 * links` keys in all, which
/// must stay within DPDK's `RTE_TEL_MAX_DICT_ENTRIES`
/// ([`MAX_TELEMETRY_DICT_ENTRIES`](crate::consts::MAX_TELEMETRY_DICT_ENTRIES)).
pub fn ports_reply(ports: &[PortInfo], directions: &[DirectionInfo], num_links: usize) -> Reply {
    let mut reply = vec![uint("num_ports", ports.len() as u64)];
    for (i, p) in ports.iter().enumerate() {
        reply.push(string(&format!("port{i}_name"), p.name.as_str()));
        reply.push(uint(&format!("port{i}_rx_ring"), p.rx_ring as u64));
        reply.push(uint(&format!("port{i}_tx_ring"), p.tx_ring as u64));
        reply.push(uint(
            &format!("port{i}_rx_ring_requested"),
            p.rx_ring_requested as u64,
        ));
        reply.push(uint(
            &format!("port{i}_tx_ring_requested"),
            p.tx_ring_requested as u64,
        ));
    }
    reply.push(uint("num_directions", directions.len() as u64));
    reply.push(uint("num_links", num_links as u64));
    for (d, dir) in directions.iter().enumerate() {
        reply.push(uint(&format!("dir{d}_rx_port"), dir.rx_port as u64));
        reply.push(uint(&format!("dir{d}_tx_port"), dir.tx_port as u64));
        reply.push(uint(&format!("dir{d}_lcore"), dir.lcore as u64));
        reply.push(string(&format!("dir{d}_config"), dir.config.as_str()));
        reply.push(uint(&format!("dir{d}_pool_size"), dir.pool_size as u64));
        reply.push(uint(&format!("dir{d}_raw_free"), dir.raw_free as u64));
        reply.push(string(&format!("dir{d}_link"), dir.link.as_str()));
        reply.push(string(&format!("dir{d}_rx_name"), dir.rx_name.as_str()));
        reply.push(string(&format!("dir{d}_tx_name"), dir.tx_name.as_str()));
        reply.push(uint(&format!("dir{d}_delay_stats"), dir.delay_stats as u64));
    }
    reply
}

/// Registers `/dpds/ports` (no-op without the feature). The values are
/// static, so the feature gate only keeps a plain build off the socket.
#[cfg(not(feature = "telemetry"))]
pub fn register_port_telemetry(
    _ports: Vec<PortInfo>,
    _directions: Vec<DirectionInfo>,
    _num_links: usize,
) -> Result<(), dpdk::DpdkError> {
    Ok(())
}

#[cfg(feature = "telemetry")]
pub fn register_port_telemetry(
    ports: Vec<PortInfo>,
    directions: Vec<DirectionInfo>,
    num_links: usize,
) -> Result<(), dpdk::DpdkError> {
    dpdk::telemetry::register_command(
        "/dpds/ports",
        "DPDS ports (names, applied/requested rings) and per-direction layout",
        move |_param, d| fill(d, ports_reply(&ports, &directions, num_links)),
    )
}

/// The main lcore's own per-port state. Separate from [`Stats`] so the main
/// lcore never writes into state the worker owns.
pub struct NicHwStats {
    pub first_loss: Vec<StageLossTracker>,
    pub epochs: Vec<NicEpochLog>,
}

impl NicHwStats {
    pub fn new(num_ports: usize) -> NicHwStats {
        NicHwStats {
            first_loss: (0..num_ports).map(|_| StageLossTracker::new()).collect(),
            epochs: (0..num_ports).map(|_| NicEpochLog::new()).collect(),
        }
    }

    /// `/nic/hw_first_loss,<param>`: `hw_first_loss_reason`,
    /// `hw_first_loss_detail`, `hw_first_loss_relative_ns`. `None` for a
    /// param that is not the decimal index of a polled port.
    pub fn hw_first_loss_reply(&self, param: &str) -> Option<Reply> {
        let tracker = self.first_loss.get(index_param(param)?)?;
        let fields = format_first_loss(tracker.current());
        Some(vec![
            string("hw_first_loss_reason", fields.reason),
            string("hw_first_loss_detail", fields.detail),
            uint("hw_first_loss_relative_ns", fields.relative_ns),
        ])
    }

    /// `/nic/epochs,<param>`: `epochs_reply` with `rx_packets`, `imissed`,
    /// `rx_port_discards`. `None` for a bad param.
    pub fn epochs_reply(&self, param: &str) -> Option<Reply> {
        let log = self.epochs.get(index_param(param)?)?;
        Some(epochs_reply(
            log,
            &NicEpochLog::EPOCH_COUNTER_NAMES,
            &NicEpochLog::EPOCH_GAUGE_NAMES,
        ))
    }

    /// Registers `/nic/hw_first_loss` and `/nic/epochs` (no-op without the
    /// feature). A bad param yields an empty dict.
    #[cfg(not(feature = "telemetry"))]
    pub fn register_telemetry(_stats: &'static NicHwStats) -> Result<(), dpdk::DpdkError> {
        Ok(())
    }

    #[cfg(feature = "telemetry")]
    pub fn register_telemetry(stats: &'static NicHwStats) -> Result<(), dpdk::DpdkError> {
        dpdk::telemetry::register_command(
            "/nic/hw_first_loss",
            "Polled NIC-hardware first-loss result by port (query with ,0 / ,1)",
            move |param, d| {
                if let Some(reply) = stats.hw_first_loss_reply(param) {
                    fill(d, reply);
                }
            },
        )?;
        dpdk::telemetry::register_command(
            "/nic/epochs",
            "Per-epoch NIC rx_packets/imissed/rx_port_discards by port (query with ,0 / ,1)",
            move |param, d| {
                if let Some(reply) = stats.epochs_reply(param) {
                    fill(d, reply);
                }
            },
        )
    }
}

/// The main lcore's poll state of one port.
pub struct NicPoller {
    port: usize,
    start_ns: f64,
    prev: dpdk::port::PortStats,
    tracker: NicEpochTracker,
    // `EpochLog::close` wants running totals and diffs them itself, but the
    // tracker hands back each closed epoch's delta: keep a running sum of the
    // deltas, which the log's own diff turns back into the same delta.
    cumulative_rx_packets: u64,
    cumulative_imissed: u64,
    cumulative_rx_port_discards: u64,
}

impl NicPoller {
    /// `port` indexes `NicHwStats`; `initial` / `initial_discards` seed the
    /// baselines.
    pub fn new(
        port: usize,
        start_ns: f64,
        initial: &dpdk::port::PortStats,
        initial_discards: u64,
    ) -> NicPoller {
        NicPoller {
            port,
            start_ns,
            prev: *initial,
            tracker: NicEpochTracker::new(0.0, initial.ipackets, initial.imissed, initial_discards),
            cumulative_rx_packets: 0,
            cumulative_imissed: 0,
            cumulative_rx_port_discards: 0,
        }
    }

    /// One 250 ms poll. (1) A rise in a hardware drop counter is recorded in
    /// `hw.first_loss[port]` (first one wins); (2) the epoch tracker runs and
    /// a closed epoch goes into `hw.epochs[port]` (`read_port_discards`, a
    /// firmware query, is only called when an epoch closes); (3) `curr`
    /// becomes the new baseline. All times are uptime (`now_ns - start_ns`).
    pub fn poll(
        &mut self,
        now_ns: f64,
        curr: &dpdk::port::PortStats,
        hw: &NicHwStats,
        read_port_discards: impl FnOnce() -> Option<u64>,
    ) {
        let uptime = now_ns - self.start_ns;
        if let Some((reason, count)) = detect_hardware_drop(&self.prev, curr) {
            hw.first_loss[self.port].try_record(
                reason,
                StageLossDetail::HardwareDrop {
                    from: Stage::Dpds,
                    count,
                },
                uptime,
            );
        }
        if let Some(record) =
            self.tracker
                .poll(uptime, curr.ipackets, curr.imissed, read_port_discards)
        {
            self.cumulative_rx_packets += record.rx_packets;
            self.cumulative_imissed += record.imissed;
            self.cumulative_rx_port_discards += record.rx_port_discards;
            hw.epochs[self.port].close(
                [
                    self.cumulative_rx_packets,
                    self.cumulative_imissed,
                    self.cumulative_rx_port_discards,
                ],
                [],
                record.start_uptime_ns as u64,
                record.end_uptime_ns as u64,
                None,
            );
        }
        self.prev = *curr;
    }
}
