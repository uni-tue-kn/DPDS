//! Rates, drop shares and histogram edges from raw telemetry replies (B2, B4).

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

/// One poll of the telemetry socket, kept as the raw replies.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Reply of `/dpds/ports`.
    pub ports: Value,
    /// `dirs[d]` = reply of `/dpds/stats,<d>`.
    pub dirs: Vec<Value>,
    /// `eth[p]` = reply of `/ethdev/stats,<p>` (p = DPDK port id).
    pub eth: Vec<Value>,
    /// `hists[d]` = reply of `/dpds/delay_hist,<d>`.
    pub hists: Vec<Value>,
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
    /// TX ring drops (`tx_drops`) of the NIC.
    pub nic_tx_pct: f64,
    /// `imissed` + `rx_nombuf` of the receiving NIC.
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

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PortStats {
    pub port: u32,
    pub name: Option<String>,
    pub ipackets: u64,
    pub opackets: u64,
    pub ibytes: u64,
    pub obytes: u64,
    pub imissed: u64,
    pub ierrors: u64,
    pub oerrors: u64,
    pub rx_nombuf: u64,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct StatsView {
    pub directions: Vec<DirectionStats>,
    pub ports: Vec<PortStats>,
    /// Milliseconds since the last successful poll; absent without any data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
    /// No data, or the last poll(s) failed.
    pub stale: bool,
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
    let n_ports = uint(&cur.ports, "num_ports").unwrap_or(0) as usize;

    let directions = (0..n_dirs.min(cur.dirs.len()))
        .map(|d| {
            let c = &cur.dirs[d];
            let rx_port = uint(&cur.ports, &format!("dir{d}_rx_port")).unwrap_or(0) as usize;
            let tx_port = uint(&cur.ports, &format!("dir{d}_tx_port")).unwrap_or(0) as usize;
            let delay_stats = delay_stats_on(&cur.ports, d);
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
            s.nic_tx_pct = pct(d_or_0("tx_drops"), rx);
            let port = |snap: &Snapshot, i: usize| snap.eth.get(i).cloned().unwrap_or(Value::Null);
            let (rc, rp) = (port(cur, rx_port), port(p, rx_port));
            let (tc, tp) = (port(cur, tx_port), port(p, tx_port));
            let nic = |a: &Value, b: &Value, k: &str| delta(a, b, k).unwrap_or(0);
            s.rx_bps = nic(&rc, &rp, "ibytes") as f64 * 8.0 / secs;
            s.tx_bps = nic(&tc, &tp, "obytes") as f64 * 8.0 / secs;
            s.nic_rx_pct = pct(nic(&rc, &rp, "imissed") + nic(&rc, &rp, "rx_nombuf"), rx);
            s
        })
        .collect();

    let ports = (0..n_ports.min(cur.eth.len()))
        .map(|p| {
            let e = &cur.eth[p];
            PortStats {
                port: p as u32,
                name: text(&cur.ports, &format!("port{p}_name")),
                ipackets: u(e, "ipackets"),
                opackets: u(e, "opackets"),
                ibytes: u(e, "ibytes"),
                obytes: u(e, "obytes"),
                imissed: u(e, "imissed"),
                ierrors: u(e, "ierrors"),
                oerrors: u(e, "oerrors"),
                rx_nombuf: u(e, "rx_nombuf"),
            }
        })
        .collect();

    StatsView {
        directions,
        ports,
        age_ms: None,
        stale: false,
    }
}

/// The cumulative delay histogram of a direction with its bin edges; `None`
/// without data or for a direction with delay statistics off.
pub fn delay_histogram(cur: &Snapshot, dir: usize) -> Option<HistogramView> {
    if !delay_stats_on(&cur.ports, dir) {
        return None;
    }
    let h = cur.hists.get(dir)?;
    let bins: Vec<u64> = h
        .get("bins")?
        .as_array()?
        .iter()
        .map(|b| b.as_u64().unwrap_or(0))
        .collect();
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
    })
}
