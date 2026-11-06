//! Rates, drop shares and histogram edges from raw telemetry replies.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::cdf::DelayModel;
use crate::dto::{ConfigDto, DelayerDto};

/// One poll of the telemetry socket, kept as the raw replies.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Reply of `/dpds/ports`.
    pub ports: Value,
    /// `dirs[d]` = reply of `/dpds/stats,<d>`.
    pub dirs: Vec<Value>,
    /// `eth[p]` = reply of `/ethdev/xstats,<p>` (p = DPDK port id), a flat `{name: value}` object.
    pub eth: Vec<Value>,
    /// `hists[d]` = reply of `/dpds/delay_hist,<d>` (page 0) without its `bins`, only the
    /// metadata (edges, under/overflow); `null` when the histogram is absent.
    pub hists: Vec<Value>,
    /// `hist_bins[d]` = every bin of direction `d`, the bins of all its pages in order
    /// (`Vec<u64>`, 8 bytes a bin). A direction without bins has no histogram.
    pub hist_bins: Vec<Vec<u64>>,
    /// Like `hists`, for `/dpds/iat_hist,<d>` (the departure-gap histogram).
    pub iat_hists: Vec<Value>,
    /// Like `hist_bins`, for the IAT histogram.
    pub iat_hist_bins: Vec<Vec<u64>>,
}

/// The two latest snapshots; rates come from their difference.
#[derive(Debug, Clone)]
pub struct Polled {
    pub prev: Option<Snapshot>,
    pub cur: Snapshot,
    /// Milliseconds since the last successful poll.
    pub age_ms: u64,
    /// The last poll(s) failed; `cur` is the last good data.
    pub stale: bool,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct Counters {
    pub rx_pkts: u64,
    pub dpds_rejects: u64,
    pub loss_drops: u64,
    pub limiter_drops: u64,
    pub buffer_full_drops: u64,
    pub pops: u64,
    pub tx_pkts: u64,
    pub tx_sent: u64,
    pub tx_drops: u64,
    /// Cumulative received frames per size: [0,63], [64], [65,127], [128,255], [256,511],
    /// [512,1023], [1024,1518], [1519,inf) bytes, before any drop; 0s when
    /// `dpds` does not report them.
    pub frame_sizes: [u64; 8],
    /// Cumulative departures out of arrival order (reordering links only; 0 otherwise).
    pub out_of_order: u64,
    /// Cumulative `rx_missed_errors` (= `imissed`) of the ingress port; links that share an
    /// ingress port (a hairpin of one port) both report the port's counter.
    pub nic_rx_missed: u64,
    /// Cumulative `rx_mbuf_allocation_errors` (= `rx_nombuf`) of the ingress port.
    pub nic_rx_nombuf: u64,
    /// Cumulative sum of the ingress port's `rx_prio[0-7]_(buf|cong)_discard_packets`.
    pub nic_rx_port_buffer_discards: u64,
    /// Cumulative `rx_phy_discard_packets` of the ingress port.
    pub nic_rx_phy_discards: u64,
    /// Cumulative `tx_phy_discard_packets` of the egress port: frames the NIC took from
    /// `tx_burst` and dropped before the wire.
    pub nic_tx_phy_discards: u64,
    /// Diagnostics of the ingress port, counted in no loss total: `rx_phy_crc_errors`.
    pub nic_rx_crc_errors: u64,
    /// `rx_phy_symbol_errors` of the ingress port (a diagnostic).
    pub nic_rx_symbol_errors: u64,
    /// `rx_phy_in_range_len_errors` of the ingress port (a diagnostic).
    pub nic_rx_length_errors: u64,
}

/// `^rx_prio[0-7]_(buf|cong)_discard_packets$`
fn is_port_buffer_discard(name: &str) -> bool {
    name.strip_prefix("rx_prio")
        .and_then(|r| r.strip_suffix("_discard_packets"))
        .and_then(|r| r.strip_prefix(|c: char| ('0'..='7').contains(&c)))
        .is_some_and(|r| r == "_buf" || r == "_cong")
}

/// The loss xstats of one port; a name the NIC does not report is 0.
#[derive(Debug, Clone, Copy, Default)]
struct NicXstats {
    missed: u64,
    nombuf: u64,
    port_buffer: u64,
    rx_phy: u64,
    tx_phy: u64,
    crc: u64,
    symbol: u64,
    length: u64,
}

impl NicXstats {
    fn read(e: &Value) -> NicXstats {
        let port_buffer = e.as_object().map_or(0, |o| {
            o.iter()
                .filter(|(k, _)| is_port_buffer_discard(k))
                .filter_map(|(_, v)| v.as_u64())
                .sum()
        });
        NicXstats {
            missed: u(e, "rx_missed_errors"),
            nombuf: u(e, "rx_mbuf_allocation_errors"),
            port_buffer,
            rx_phy: u(e, "rx_phy_discard_packets"),
            tx_phy: u(e, "tx_phy_discard_packets"),
            crc: u(e, "rx_phy_crc_errors"),
            symbol: u(e, "rx_phy_symbol_errors"),
            length: u(e, "rx_phy_in_range_len_errors"),
        }
    }

    /// The RX-side NIC losses (not the diagnostics).
    fn rx_losses(&self) -> u64 {
        self.missed + self.nombuf + self.port_buffer + self.rx_phy
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DirectionStats {
    pub dir: u32,
    pub link: Option<String>,
    pub rx: Option<String>,
    pub tx: Option<String>,
    /// Rates over the window between the last two polls; 0 without a window.
    pub rx_pps: f64,
    pub tx_pps: f64,
    pub rx_bps: f64,
    pub tx_bps: f64,
    /// Drop shares in percent of the packets received in the window.
    pub loss_pct: f64,
    pub limiter_pct: f64,
    pub buffer_full_pct: f64,
    /// TX ring drops (`tx_drops`) plus `tx_phy_discard_packets` of the egress NIC.
    pub nic_tx_pct: f64,
    /// `rx_missed_errors`, `rx_mbuf_allocation_errors`, the port buffer discards and
    /// `rx_phy_discard_packets` of the ingress NIC.
    pub nic_rx_pct: f64,
    /// The direction records delay statistics (`dir<d>_delay_stats` of `/dpds/ports`;
    /// on when the key is absent). When off, the `delay_*` and `lateness_*` values are null.
    pub delay_stats: bool,
    /// Delay statistics, cumulative since start (null without the data).
    pub delay_mean_ns: Option<f64>,
    pub delay_std_ns: Option<f64>,
    pub delay_min_ns: Option<f64>,
    pub delay_max_ns: Option<f64>,
    pub lateness_mean_ns: Option<f64>,
    pub lateness_max_ns: Option<f64>,
    pub buffer_len_max: Option<u64>,
    pub first_loss_reason: Option<String>,
    pub first_loss_detail: Option<String>,
    pub first_loss_relative_ns: Option<u64>,
    pub counters: Counters,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct StatsView {
    pub directions: Vec<DirectionStats>,
    /// Milliseconds since the last successful poll; absent without any data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
    /// No data, or the last poll(s) failed.
    pub stale: bool,
    /// Milliseconds since the current (or last) run started, from a monotonic clock; frozen
    /// at the stop. Absent before any run. Shares its clock with `t_ms` of the history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

/// One point of a link's chart history: the rates `/api/stats` reports for the poll window ending
/// at `t_ms`, in the units of the charts, and the link's cumulative drop counters at that poll.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct HistorySample {
    /// Milliseconds since the run started.
    pub t_ms: u64,
    pub rx_gbps: f64,
    pub tx_gbps: f64,
    pub rx_mpps: f64,
    pub tx_mpps: f64,
    pub loss_drops: u64,
    pub limiter_drops: u64,
    pub buffer_full_drops: u64,
    /// `tx_drops` of the NIC.
    pub nic_tx_drops: u64,
    pub nic_rx_missed: u64,
    pub nic_rx_nombuf: u64,
    pub nic_rx_port_buffer_discards: u64,
    pub nic_rx_phy_discards: u64,
    pub nic_tx_phy_discards: u64,
}

impl HistorySample {
    pub fn new(t_ms: u64, d: &DirectionStats) -> HistorySample {
        let c = &d.counters;
        HistorySample {
            t_ms,
            rx_gbps: d.rx_bps / 1e9,
            tx_gbps: d.tx_bps / 1e9,
            rx_mpps: d.rx_pps / 1e6,
            tx_mpps: d.tx_pps / 1e6,
            loss_drops: c.loss_drops,
            limiter_drops: c.limiter_drops,
            buffer_full_drops: c.buffer_full_drops,
            nic_tx_drops: c.tx_drops,
            nic_rx_missed: c.nic_rx_missed,
            nic_rx_nombuf: c.nic_rx_nombuf,
            nic_rx_port_buffer_discards: c.nic_rx_port_buffer_discards,
            nic_rx_phy_discards: c.nic_rx_phy_discards,
            nic_tx_phy_discards: c.nic_tx_phy_discards,
        }
    }
}

/// A link's cumulative counters since the `dpds` process started, as of the last poll of the run.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct HistoryTotals {
    pub rx_pkts: u64,
    /// `counters.tx_sent`: the frames forwarded to the egress port.
    pub tx_sent: u64,
    /// `counters.frame_sizes`.
    pub frame_sizes: [u64; 8],
    /// `counters.out_of_order`.
    pub out_of_order: u64,
    pub loss_drops: u64,
    pub limiter_drops: u64,
    pub buffer_full_drops: u64,
    /// `counters.tx_drops` of `/api/stats`.
    pub nic_tx_drops: u64,
    pub nic_rx_missed: u64,
    pub nic_rx_nombuf: u64,
    pub nic_rx_port_buffer_discards: u64,
    pub nic_rx_phy_discards: u64,
    pub nic_tx_phy_discards: u64,
}

impl From<&Counters> for HistoryTotals {
    fn from(c: &Counters) -> HistoryTotals {
        HistoryTotals {
            rx_pkts: c.rx_pkts,
            tx_sent: c.tx_sent,
            frame_sizes: c.frame_sizes,
            out_of_order: c.out_of_order,
            loss_drops: c.loss_drops,
            limiter_drops: c.limiter_drops,
            buffer_full_drops: c.buffer_full_drops,
            nic_tx_drops: c.tx_drops,
            nic_rx_missed: c.nic_rx_missed,
            nic_rx_nombuf: c.nic_rx_nombuf,
            nic_rx_port_buffer_discards: c.nic_rx_port_buffer_discards,
            nic_rx_phy_discards: c.nic_rx_phy_discards,
            nic_tx_phy_discards: c.nic_tx_phy_discards,
        }
    }
}

/// A link of the run a history belongs to, to title its charts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct HistoryLink {
    pub name: String,
    pub ingress: String,
    pub egress: String,
}

/// Answer of `GET /api/stats/history`.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct HistoryView {
    /// Identity of the run: one more with every new run the controller sees; null before any run.
    pub run_id: Option<u64>,
    /// The links of that run, in the order of the start body (the keys of `samples`); empty
    /// before any run.
    pub links: Vec<HistoryLink>,
    /// Unix seconds, as `/api/emulation` reports it; null while nothing has run.
    pub started_at: Option<u64>,
    /// Per link index (in the order of the start body), the samples with `t_ms` above the
    /// request's `since_ms`, oldest first.
    pub samples: BTreeMap<String, Vec<HistorySample>>,
    /// Per link index, the cumulative counters of the newest successful poll of the run; kept
    /// after the stop, `{}` before any run.
    pub totals: BTreeMap<String, HistoryTotals>,
}

/// Where a clamped percentile lies: below or above the histogram range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Clamped {
    Underflow,
    Overflow,
}

/// Percentiles of the returned counts in ns (linear interpolation inside the bin).
/// A percentile in the underflow or overflow is the respective range edge and its
/// `*_clamped` field says which; all are null without samples.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct Percentiles {
    pub p50: Option<f64>,
    pub p50_clamped: Option<Clamped>,
    pub p90: Option<f64>,
    pub p90_clamped: Option<Clamped>,
    pub p99: Option<f64>,
    pub p99_clamped: Option<Clamped>,
    pub p999: Option<f64>,
    pub p999_clamped: Option<Clamped>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedModel {
    Exact,
    /// Correlated delays: the stationary distribution is approximated as Normal(mean, std).
    NormalApproximation,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HistogramView {
    pub dir: u32,
    pub lo_ns: f64,
    pub bin_ns: f64,
    pub bins: Vec<u64>,
    pub underflow: u64,
    pub overflow: u64,
    /// `bins.len() + 1` bin edges.
    pub edges_ns: Vec<f64>,
    /// Milliseconds since the last successful poll, like `/api/stats`.
    pub age_ms: u64,
    /// The last poll(s) failed.
    pub stale: bool,
    /// The requested window in seconds; 0 = since start.
    pub window_s: u32,
    /// Time the counts cover in milliseconds (the run's age for "since start").
    pub span_ms: u64,
    /// Samples including underflow and overflow.
    pub count: u64,
    pub percentiles: Percentiles,
    /// `delay_mean_ns` / `delay_std_ns` of the link in the latest stats poll; null when unknown.
    pub mean_ns: Option<f64>,
    pub std_ns: Option<f64>,
    /// Expected count per bin for `count` samples under the configured delay distribution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_underflow: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_overflow: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_model: Option<ExpectedModel>,
    /// `excludes limiter queueing delay` when the direction's limiter is active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_note: Option<String>,
}

impl HistogramView {
    /// `self - base`, the samples recorded between the two snapshots; `false`
    /// (and `self` unchanged) if the geometry differs or a counter went backwards.
    pub fn subtract(&mut self, base: &HistogramView) -> bool {
        let same = self.bins.len() == base.bins.len()
            && self.lo_ns == base.lo_ns
            && self.bin_ns == base.bin_ns;
        let ok = same
            && self.underflow >= base.underflow
            && self.overflow >= base.overflow
            && self.bins.iter().zip(&base.bins).all(|(c, b)| c >= b);
        if ok {
            for (c, b) in self.bins.iter_mut().zip(&base.bins) {
                *c -= b;
            }
            self.underflow -= base.underflow;
            self.overflow -= base.overflow;
        }
        ok
    }

    /// Fills `count` and `percentiles` from the counts.
    pub fn summarise(&mut self) {
        self.count = self.underflow + self.overflow + self.bins.iter().sum::<u64>();
        let (p50, p50_clamped) = self.percentile(0.5);
        let (p90, p90_clamped) = self.percentile(0.9);
        let (p99, p99_clamped) = self.percentile(0.99);
        let (p999, p999_clamped) = self.percentile(0.999);
        self.percentiles = Percentiles {
            p50,
            p50_clamped,
            p90,
            p90_clamped,
            p99,
            p99_clamped,
            p999,
            p999_clamped,
        };
    }

    /// Rank `q * count` over underflow, bin 0 .. n-1, overflow: the percentile lies
    /// in the first bin whose cumulative count reaches the rank.
    fn percentile(&self, q: f64) -> (Option<f64>, Option<Clamped>) {
        if self.count == 0 {
            return (None, None);
        }
        let rank = q * self.count as f64;
        if rank <= self.underflow as f64 {
            return (Some(self.lo_ns), Some(Clamped::Underflow));
        }
        let mut before = self.underflow as f64;
        for (i, &c) in self.bins.iter().enumerate() {
            let c = c as f64;
            if c > 0.0 && rank <= before + c {
                let value = self.lo_ns + (i as f64 + (rank - before) / c) * self.bin_ns;
                return (Some(value), None);
            }
            before += c;
        }
        let upper = self.lo_ns + self.bins.len() as f64 * self.bin_ns;
        (Some(upper), Some(Clamped::Overflow))
    }

    /// Fills the `expected*` fields from the direction's delay distribution.
    pub fn add_expected(&mut self, config: &ConfigDto) {
        let delayer = config.delayer.clone().unwrap_or_else(DelayerDto::none);
        let (model, exact) = DelayModel::from_delayer(&delayer);
        let (bins, under, over) =
            model.expected_counts(self.lo_ns, self.bin_ns, self.bins.len(), self.count);
        self.expected = Some(bins);
        self.expected_underflow = Some(under);
        self.expected_overflow = Some(over);
        self.expected_model = Some(if exact {
            ExpectedModel::Exact
        } else {
            ExpectedModel::NormalApproximation
        });
        self.expected_note = config
            .limiter
            .is_some()
            .then(|| "excludes limiter queueing delay".to_string());
    }
}

/// Telemetry sends counters as integers and averages as decimal strings.
fn num(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn uint(v: &Value, key: &str) -> Option<u64> {
    v.get(key)?.as_u64()
}

fn u(v: &Value, key: &str) -> u64 {
    uint(v, key).unwrap_or(0)
}

/// The `frame_sizes` array of a stats reply; 0s for a missing or short one.
fn frame_sizes(v: &Value) -> [u64; 8] {
    let mut out = [0; 8];
    if let Some(a) = v.get("frame_sizes").and_then(Value::as_array) {
        for (o, x) in out.iter_mut().zip(a) {
            *o = x.as_u64().unwrap_or(0);
        }
    }
    out
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_string)
}

/// `cur - prev`, `None` if the counter went backwards (restart).
fn delta(cur: &Value, prev: &Value, key: &str) -> Option<u64> {
    uint(cur, key)?.checked_sub(uint(prev, key)?)
}

fn pct(part: u64, rx: u64) -> f64 {
    if rx == 0 {
        0.0
    } else {
        part as f64 / rx as f64 * 100.0
    }
}

/// Whether direction `d` records delay statistics; on unless `/dpds/ports` says 0.
pub fn delay_stats_on(ports: &Value, d: usize) -> bool {
    uint(ports, &format!("dir{d}_delay_stats")) != Some(0)
}

/// Rates and shares are 0.0 without `prev`, without time between the snapshots
/// or when a counter went backwards.
pub fn compute_stats(prev: Option<&Snapshot>, cur: &Snapshot) -> StatsView {
    let n_dirs = uint(&cur.ports, "num_directions").unwrap_or(0) as usize;

    let directions = (0..n_dirs.min(cur.dirs.len()))
        .map(|d| {
            let c = &cur.dirs[d];
            let rx_port = uint(&cur.ports, &format!("dir{d}_rx_port")).unwrap_or(0) as usize;
            let tx_port = uint(&cur.ports, &format!("dir{d}_tx_port")).unwrap_or(0) as usize;
            let delay_stats = delay_stats_on(&cur.ports, d);
            let nic_of = |snap: &Snapshot, i: usize| snap.eth.get(i).map(NicXstats::read);
            let rx_nic = nic_of(cur, rx_port).unwrap_or_default();
            let tx_nic = nic_of(cur, tx_port).unwrap_or_default();
            let delay = |key: &str| if delay_stats { num(c, key) } else { None };
            let mut s = DirectionStats {
                dir: d as u32,
                link: text(&cur.ports, &format!("dir{d}_link")),
                rx: text(&cur.ports, &format!("dir{d}_rx_name")),
                tx: text(&cur.ports, &format!("dir{d}_tx_name")),
                rx_pps: 0.0,
                tx_pps: 0.0,
                rx_bps: 0.0,
                tx_bps: 0.0,
                loss_pct: 0.0,
                limiter_pct: 0.0,
                buffer_full_pct: 0.0,
                nic_tx_pct: 0.0,
                nic_rx_pct: 0.0,
                delay_stats,
                delay_mean_ns: delay("delay_mean_ns"),
                delay_std_ns: delay("delay_std_ns"),
                delay_min_ns: delay("delay_min_ns"),
                delay_max_ns: delay("delay_max_ns"),
                lateness_mean_ns: delay("lateness_mean_ns"),
                lateness_max_ns: delay("lateness_max_ns"),
                buffer_len_max: uint(c, "buffer_len_max"),
                first_loss_reason: text(c, "first_loss_reason"),
                first_loss_detail: text(c, "first_loss_detail"),
                first_loss_relative_ns: uint(c, "first_loss_relative_ns"),
                counters: Counters {
                    rx_pkts: u(c, "rx_pkts"),
                    dpds_rejects: u(c, "dpds_rejects"),
                    loss_drops: u(c, "loss_drops"),
                    limiter_drops: u(c, "limiter_drops"),
                    buffer_full_drops: u(c, "buffer_full_drops"),
                    pops: u(c, "pops"),
                    tx_pkts: u(c, "tx_pkts"),
                    tx_sent: u(c, "tx_sent"),
                    tx_drops: u(c, "tx_drops"),
                    frame_sizes: frame_sizes(c),
                    out_of_order: u(c, "out_of_order"),
                    nic_rx_missed: rx_nic.missed,
                    nic_rx_nombuf: rx_nic.nombuf,
                    nic_rx_port_buffer_discards: rx_nic.port_buffer,
                    nic_rx_phy_discards: rx_nic.rx_phy,
                    nic_tx_phy_discards: tx_nic.tx_phy,
                    nic_rx_crc_errors: rx_nic.crc,
                    nic_rx_symbol_errors: rx_nic.symbol,
                    nic_rx_length_errors: rx_nic.length,
                },
            };
            let Some(p) = prev else { return s };
            let Some(pd) = p.dirs.get(d) else { return s };
            let dt_ns = match delta(c, pd, "uptime_ns") {
                Some(dt) if dt > 0 => dt as f64,
                _ => return s,
            };
            let secs = dt_ns / 1e9;
            let Some(rx) = delta(c, pd, "rx_pkts") else {
                return s;
            };
            let d_or_0 = |key: &str| delta(c, pd, key).unwrap_or(0);
            s.rx_pps = rx as f64 / secs;
            s.tx_pps = d_or_0("tx_sent") as f64 / secs;
            s.loss_pct = pct(d_or_0("loss_drops"), rx);
            s.limiter_pct = pct(d_or_0("limiter_drops"), rx);
            s.buffer_full_pct = pct(d_or_0("buffer_full_drops"), rx);
            let port = |snap: &Snapshot, i: usize| snap.eth.get(i).cloned().unwrap_or(Value::Null);
            let (rc, rp) = (port(cur, rx_port), port(p, rx_port));
            let (tc, tp) = (port(cur, tx_port), port(p, tx_port));
            let nic = |a: &Value, b: &Value, k: &str| delta(a, b, k).unwrap_or(0);
            s.rx_bps = nic(&rc, &rp, "rx_good_bytes") as f64 * 8.0 / secs;
            s.tx_bps = nic(&tc, &tp, "tx_good_bytes") as f64 * 8.0 / secs;
            // a loss counter that went backwards (restart) adds 0
            let grown = |c: u64, p: u64| c.saturating_sub(p);
            let (rc, rp) = (NicXstats::read(&rc), NicXstats::read(&rp));
            let (tc, tp) = (NicXstats::read(&tc), NicXstats::read(&tp));
            s.nic_tx_pct = pct(d_or_0("tx_drops") + grown(tc.tx_phy, tp.tx_phy), rx);
            s.nic_rx_pct = pct(grown(rc.rx_losses(), rp.rx_losses()), rx);
            s
        })
        .collect();

    StatsView {
        directions,
        age_ms: None,
        stale: false,
        elapsed_ms: None,
    }
}

/// The cumulative delay histogram of a direction with its bin edges; `None`
/// without data or for a direction with delay statistics off.
pub fn delay_histogram(cur: &Snapshot, dir: usize) -> Option<HistogramView> {
    let mut h = histogram_view(cur, dir, &cur.hists, &cur.hist_bins)?;
    h.mean_ns = cur.dirs.get(dir).and_then(|c| num(c, "delay_mean_ns"));
    h.std_ns = cur.dirs.get(dir).and_then(|c| num(c, "delay_std_ns"));
    Some(h)
}

/// The cumulative IAT (departure gap) histogram of a direction, like [`delay_histogram`]
/// without the mean and standard deviation.
pub fn iat_histogram(cur: &Snapshot, dir: usize) -> Option<HistogramView> {
    histogram_view(cur, dir, &cur.iat_hists, &cur.iat_hist_bins)
}

fn histogram_view(
    cur: &Snapshot,
    dir: usize,
    hists: &[Value],
    hist_bins: &[Vec<u64>],
) -> Option<HistogramView> {
    if !delay_stats_on(&cur.ports, dir) {
        return None;
    }
    let h = hists.get(dir)?;
    let bins = hist_bins.get(dir).filter(|b| !b.is_empty())?.clone();
    let lo_ns = num(h, "lo_ns")?;
    let bin_ns = num(h, "bin_ns")?;
    let edges_ns = (0..=bins.len())
        .map(|i| lo_ns + i as f64 * bin_ns)
        .collect();
    Some(HistogramView {
        dir: dir as u32,
        lo_ns,
        bin_ns,
        underflow: u(h, "underflow"),
        overflow: u(h, "overflow"),
        bins,
        edges_ns,
        age_ms: 0,
        stale: false,
        window_s: 0,
        span_ms: 0,
        count: 0,
        percentiles: Percentiles::default(),
        mean_ns: None,
        std_ns: None,
        expected: None,
        expected_underflow: None,
        expected_overflow: None,
        expected_model: None,
        expected_note: None,
    })
}

/// Milliseconds the direction has been running (`uptime_ns` of its stats), if reported.
pub fn uptime_ms(cur: &Snapshot, dir: usize) -> Option<u64> {
    Some(uint(cur.dirs.get(dir)?, "uptime_ns")? / 1_000_000)
}
