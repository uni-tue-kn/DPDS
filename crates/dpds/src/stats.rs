//! Telemetry glue: the per-direction [`Stats`], the worker's [`Telemetry`] handle,
//! the reply builders behind `/dpds/stats`, `/dpds/epochs`, `/dpds/ports`,
//! `/nic/hw_first_loss` and `/nic/epochs`, their registration on DPDK's
//! telemetry socket, and the main lcore's [`NicPoller`].
//!
//! Registration is a no-op without the `telemetry` feature. The reply fields are
//! documented on the counter, epoch and first-loss types.

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
    /// Delay / lateness statistics and the delay histogram.
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
    pub fn with_range(range: HistRange) -> Stats {
        Stats::with_delay_stats(range, true)
    }

    /// With `delay_stats` off no delay statistics are recorded, `/dpds/stats`
    /// has no delay keys and the histogram reply is empty.
    pub fn with_delay_stats(range: HistRange, delay_stats: bool) -> Stats {
        Stats::with_hists(range, HistRange::default(), delay_stats)
    }

    /// With the range of the IAT histogram (recorded exactly when `delay_stats`
    /// is on).
    pub fn with_hists(range: HistRange, iat_range: HistRange, delay_stats: bool) -> Stats {
        Stats {
            first_loss: StageLossTracker::new(),
            counters: DpdsCounters::default(),
            frame_sizes: FrameSizes::default(),
            epochs: DpdsEpochLog::default(),
            delay: DelayStats::with_hists(range, iat_range, delay_stats),
        }
    }

    /// Page 0 of the `/dpds/iat_hist` reply.
    pub fn iat_hist_reply(&self) -> Reply {
        self.delay.iat_hist_page(0).unwrap_or_default()
    }

    /// Page 0 of the `/dpds/delay_hist` reply.
    pub fn delay_hist_reply(&self) -> Reply {
        self.delay.hist_page(0).unwrap_or_default()
    }

    pub fn delay_hist_page(&self, page: usize) -> Option<Reply> {
        self.delay.hist_page(page)
    }

    /// The `/dpds/stats` reply: cumulative since start; gauges are the max over
    /// closed epochs and the open one.
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

/// The direction (or, for `/nic/*`, port) index a `,<param>` query names: a
/// plain decimal number.
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

    pub fn with_ranges(ranges: Vec<HistRange>) -> DirectionStats {
        DirectionStats::with_delay_settings(ranges.into_iter().map(|r| (r, true)).collect())
    }

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

    /// `/dpds/stats,<param>`; `None` unless `param` is the decimal index of an
    /// existing direction.
    pub fn stats_reply(&self, param: &str, uptime_ns: u64) -> Option<Reply> {
        Some(self.stats.get(index_param(param)?)?.stats_reply(uptime_ns))
    }

    /// `/dpds/epochs,<param>`; `None` for a bad param.
    pub fn epochs_reply(&self, param: &str) -> Option<Reply> {
        Some(self.stats.get(index_param(param)?)?.epochs_reply())
    }

    /// `/dpds/delay_hist,<param>`: one page of a direction's delay histogram;
    /// `<param>` is `<dir>` (page 0) or `<dir>,<page>`. `None` for a bad param,
    /// a missing direction or a page past the last one.
    pub fn delay_hist_reply(&self, param: &str) -> Option<Reply> {
        let (dir, page) = dir_and_page(param)?;
        self.stats.get(dir)?.delay_hist_page(page)
    }

    /// `/dpds/iat_hist,<param>`: like [`delay_hist_reply`](Self::delay_hist_reply)
    /// for the IAT histogram.
    pub fn iat_hist_reply(&self, param: &str) -> Option<Reply> {
        let (dir, page) = dir_and_page(param)?;
        self.stats.get(dir)?.delay.iat_hist_page(page)
    }

    /// Registers `/dpds/stats`, `/dpds/epochs`, `/dpds/delay_hist` and
    /// `/dpds/iat_hist` (no-op without the feature). `start_ns` is the shared
    /// `now_ns()` zero point. A bad param yields an empty dict.
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
/// worker-owned (non-atomic) epoch state. Zero-cost without the feature.
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
            // The driver runs in uptime (relative to `start_ns`), so
            // `epoch_*_uptime_ns` compare with `uptime_ns` and `first_loss_relative_ns`.
            driver: EpochDriver::new(0.0),
            epoch_first_loss: EpochFirstLoss::new(),
        }
    }

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
    /// `stats.epochs` (resetting the gauges).
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
    /// Descriptor ring sizes applied by the driver.
    pub rx_ring: u16,
    pub tx_ring: u16,
    /// Descriptor ring sizes requested before the driver clamped them.
    pub rx_ring_requested: u16,
    pub tx_ring_requested: u16,
}

/// Static info of one link (= direction) for `/dpds/ports`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectionInfo {
    pub rx_port: u16,
    pub tx_port: u16,
    pub lcore: u32,
    /// Where the config comes from: a config file or the links file.
    pub config: String,
    pub pool_size: u32,
    pub raw_free: bool,
    /// Link name (`default` without `--links`).
    pub link: String,
    /// DPDK device names of the receiving and transmitting port.
    pub rx_name: String,
    pub tx_name: String,
    pub delay_stats: bool,
}

/// The `/dpds/ports` reply: `num_ports`, then per port index `i`
/// `port<i>_name` (string), `port<i>_rx_ring`, `port<i>_tx_ring`,
/// `port<i>_rx_ring_requested`, `port<i>_tx_ring_requested`; then
/// `num_directions`, `num_links` and per direction `d` `dir<d>_rx_port`,
/// `dir<d>_tx_port`, `dir<d>_lcore`, `dir<d>_config` (string),
/// `dir<d>_pool_size`, `dir<d>_raw_free` (0 or 1), `dir<d>_link` (string, the
/// link name; `d` is the link's index), `dir<d>_rx_name` and `dir<d>_tx_name`
/// (strings, DPDK device names), `dir<d>_delay_stats` (0 or 1). That is
/// `3 + 5 * ports + 10 * links` keys, which must stay within
/// [`MAX_TELEMETRY_DICT_ENTRIES`](crate::consts::MAX_TELEMETRY_DICT_ENTRIES).
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

/// Registers `/dpds/ports` (no-op without the feature).
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

/// The main lcore's per-port state; separate from [`Stats`] so the main lcore
/// never writes into state a worker owns.
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
    /// `hw_first_loss_detail`, `hw_first_loss_relative_ns`. `None` unless
    /// `param` is the decimal index of a polled port.
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
    /// `rx_port_discards`; `None` for a bad param.
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
    // `EpochLog::close` takes running totals but the tracker returns deltas:
    // keep a running sum, which the log's diff turns back into the delta.
    cumulative_rx_packets: u64,
    cumulative_imissed: u64,
    cumulative_rx_port_discards: u64,
}

impl NicPoller {
    /// `port` indexes `NicHwStats`; `initial` / `initial_discards` seed the baselines.
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

    /// One poll: (1) a rise in a hardware drop counter is recorded in
    /// `hw.first_loss[port]`; (2) the epoch tracker runs and a closed epoch goes
    /// into `hw.epochs[port]` (`read_port_discards`, a firmware query, is only
    /// called when an epoch closes); (3) `curr` becomes the new baseline. Times
    /// are uptime (`now_ns - start_ns`).
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
