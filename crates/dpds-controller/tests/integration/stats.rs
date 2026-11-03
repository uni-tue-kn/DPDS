//! Rate / percentage maths from two telemetry snapshots (B2, B4).
//!
//! Spec v10: a direction is a directed link (ingress -> egress); the stats of link `d` use the
//! ports `dir<d>_rx_port` (the link's ingress) for rx and `dir<d>_tx_port` (its egress) for tx.
//! The fixture snapshot is the pair `a-to-b` / `b-to-a` (two links); `ring_snapshot` below is a
//! three-link ring over three ports and a hairpin.
//!
//! Contract of `compute_stats(prev, cur)` (JSON of `StatsView`):
//! `{"directions":[{dir, link, rx, tx, rx_pps, tx_pps, rx_bps, tx_bps, loss_pct,
//!   limiter_pct, buffer_full_pct, nic_tx_pct, nic_rx_pct, delay_mean_ns, delay_std_ns,
//!   delay_min_ns, delay_max_ns, lateness_mean_ns, lateness_max_ns, buffer_len_max,
//!   first_loss_reason, first_loss_detail, first_loss_relative_ns,
//!   counters:{rx_pkts, dpds_rejects, loss_drops, limiter_drops, buffer_full_drops, pops,
//!             tx_pkts, tx_sent, tx_drops}}],
//!   "ports":[{port, name, ipackets, opackets, ibytes, obytes, imissed, ierrors, oerrors, rx_nombuf}]}`
//! Rates and percentages are computed over the window between the two snapshots
//! (dt = delta of the direction's own `uptime_ns`); they are 0.0 (never null/NaN/inf)
//! without `prev`, with dt == 0, with zero rx delta, or when a counter went backwards
//! (restart). `rx_bps` = delta `ibytes` of the rx port * 8 / dt, `tx_bps` = delta
//! `obytes` of the tx port * 8 / dt. `nic_rx_pct` = (delta imissed + delta rx_nombuf of the
//! rx port) / delta rx_pkts * 100. Delay / lateness / buffer_len_max / first_loss are
//! passed through from `cur` (cumulative), as numbers even when telemetry sent
//! decimal strings.

use crate::common::*;
use dpds_controller::stats::{compute_stats, delay_histogram};
use serde_json::{json, Value};

fn d(uptime_ns: u64, rx: u64, tx: u64, loss: u64, lim: u64, full: u64, txd: u64) -> DirSample {
    DirSample {
        uptime_ns,
        rx_pkts: rx,
        tx_sent: tx,
        loss,
        limiter: lim,
        full,
        tx_drops: txd,
    }
}

fn p(ibytes: u64, obytes: u64, imissed: u64, nombuf: u64) -> PortSample {
    PortSample {
        ibytes,
        obytes,
        imissed,
        rx_nombuf: nombuf,
    }
}

fn prev() -> dpds_controller::stats::Snapshot {
    snapshot(
        [
            d(
                10_000_000_000,
                1_000_000,
                900_000,
                10_000,
                5_000,
                1_000,
                100,
            ),
            d(10_000_000_000, 500, 500, 0, 0, 0, 0),
        ],
        [
            p(1_000_000_000, 500_000_000, 100, 0),
            p(10, 800_000_000, 0, 0),
        ],
    )
}

fn cur() -> dpds_controller::stats::Snapshot {
    // +2 s; dir0: +2,000,000 rx, +1,800,000 sent, +40,000 loss, +20,000 limiter, +10,000 full, +5,000 tx_drops
    snapshot(
        [
            d(
                12_000_000_000,
                3_000_000,
                2_700_000,
                50_000,
                25_000,
                11_000,
                5_100,
            ),
            d(12_000_000_000, 2_500, 2_500, 0, 0, 0, 0),
        ],
        [
            p(
                1_250_000_000 + 1_000_000_000,
                500_000_000 + 225_000_000,
                30_100,
                10_000,
            ),
            p(1000, 1_800_000_000, 0, 0),
        ],
    )
}

fn view(
    prev: Option<&dpds_controller::stats::Snapshot>,
    cur: &dpds_controller::stats::Snapshot,
) -> Value {
    serde_json::to_value(compute_stats(prev, cur)).unwrap()
}

#[test]
fn rates_and_drop_shares_from_two_snapshots() {
    let v = view(Some(&prev()), &cur());
    let d0 = &v["directions"][0];
    assert_eq!(d0["dir"], 0);
    assert_eq!(d0["link"], "a-to-b");
    assert_eq!(d0["rx"], NIC0);
    assert_eq!(d0["tx"], NIC1);
    assert_close(f(d0, "rx_pps"), 1_000_000.0);
    assert_close(f(d0, "tx_pps"), 900_000.0);
    // rx port 0 ibytes +1.25e9 B over 2 s = 5 Gb/s ; tx port 1 obytes +1.0e9 B -> 4 Gb/s
    assert_close(f(d0, "rx_bps"), 1.25e9 * 8.0 / 2.0);
    assert_close(f(d0, "tx_bps"), 1.0e9 * 8.0 / 2.0);
    assert_close(f(d0, "loss_pct"), 2.0);
    assert_close(f(d0, "limiter_pct"), 1.0);
    assert_close(f(d0, "buffer_full_pct"), 0.5);
    assert_close(f(d0, "nic_tx_pct"), 0.25);
    assert_close(f(d0, "nic_rx_pct"), 2.0); // (30,000 imissed + 10,000 nombuf) / 2e6
}

#[test]
fn direction_one_uses_port_one_for_rx_and_port_zero_for_tx() {
    let v = view(Some(&prev()), &cur());
    let d1 = &v["directions"][1];
    assert_eq!(d1["dir"], 1);
    assert_eq!(d1["link"], "b-to-a");
    assert_eq!(d1["rx"], NIC1);
    assert_eq!(d1["tx"], NIC0);
    assert_close(f(d1, "rx_pps"), 1000.0);
    // rx port 1 ibytes 10 -> 1000, tx port 0 obytes 500e6 -> 725e6 (+225e6)
    assert_close(f(d1, "rx_bps"), (1000.0 - 10.0) * 8.0 / 2.0);
    assert_close(f(d1, "tx_bps"), 225.0e6 * 8.0 / 2.0);
}

#[test]
fn cumulative_counters_and_passthrough_fields() {
    let v = view(Some(&prev()), &cur());
    let d0 = &v["directions"][0];
    let c = &d0["counters"];
    assert_eq!(c["rx_pkts"], 3_000_000);
    assert_eq!(c["tx_sent"], 2_700_000);
    assert_eq!(c["loss_drops"], 50_000);
    assert_eq!(c["limiter_drops"], 25_000);
    assert_eq!(c["buffer_full_drops"], 11_000);
    assert_eq!(c["dpds_rejects"], 86_000);
    assert_eq!(c["tx_drops"], 5_100);
    // telemetry sends the float-ish values as decimal strings (and ints): output is numeric
    assert_close(f(d0, "delay_mean_ns"), 10_000_000.25);
    assert_close(f(d0, "delay_std_ns"), 1_000_000.5);
    assert_close(f(d0, "delay_min_ns"), 5_000_000.0);
    assert_close(f(d0, "delay_max_ns"), 15_000_000.0);
    assert_close(f(d0, "lateness_mean_ns"), 250.75);
    assert_close(f(d0, "lateness_max_ns"), 9000.0);
    assert_eq!(d0["buffer_len_max"], 777);
    assert_eq!(d0["first_loss_reason"], "Clean");
    assert_eq!(d0["first_loss_detail"], "");
    assert_eq!(d0["first_loss_relative_ns"], 0);
}

#[test]
fn numeric_delay_fields_are_accepted_too() {
    let mut c = cur();
    c.dirs[0]["delay_mean_ns"] = json!(123456);
    c.dirs[0]["delay_std_ns"] = json!(789);
    let v = view(Some(&prev()), &c);
    assert_close(f(&v["directions"][0], "delay_mean_ns"), 123456.0);
    assert_close(f(&v["directions"][0], "delay_std_ns"), 789.0);
}

#[test]
fn per_port_nic_stats_are_listed() {
    let v = view(Some(&prev()), &cur());
    let ports = v["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 2);
    assert_eq!(ports[0]["port"], 0);
    assert_eq!(ports[0]["name"], NIC0);
    assert_eq!(ports[0]["ibytes"], 2_250_000_000u64);
    assert_eq!(ports[0]["imissed"], 30_100);
    assert_eq!(ports[0]["rx_nombuf"], 10_000);
    assert_eq!(ports[1]["name"], NIC1);
    for k in ["ipackets", "opackets", "obytes", "ierrors", "oerrors"] {
        assert!(ports[1].get(k).is_some(), "port stats lack {k}");
    }
}

#[test]
fn without_a_previous_snapshot_rates_are_zero_but_counters_present() {
    let v = view(None, &cur());
    let d0 = &v["directions"][0];
    for k in [
        "rx_pps",
        "tx_pps",
        "rx_bps",
        "tx_bps",
        "loss_pct",
        "limiter_pct",
        "buffer_full_pct",
        "nic_tx_pct",
        "nic_rx_pct",
    ] {
        assert_eq!(f(d0, k), 0.0, "{k}");
    }
    assert_eq!(d0["counters"]["rx_pkts"], 3_000_000);
}

#[test]
fn zero_time_delta_or_zero_rx_delta_gives_zero_not_nan() {
    let c = cur();
    let v = view(Some(&c), &c); // identical snapshots: dt == 0, delta rx == 0
    for dir in v["directions"].as_array().unwrap() {
        for k in [
            "rx_pps",
            "tx_pps",
            "rx_bps",
            "tx_bps",
            "loss_pct",
            "limiter_pct",
            "buffer_full_pct",
            "nic_tx_pct",
            "nic_rx_pct",
        ] {
            assert_eq!(f(dir, k), 0.0, "{k} in {dir}");
        }
    }
    // time moved on but no packets: shares stay 0, no division by zero
    let mut later = c.clone();
    for dd in &mut later.dirs {
        dd["uptime_ns"] = json!(dd["uptime_ns"].as_u64().unwrap() + 1_000_000_000);
    }
    let v = view(Some(&c), &later);
    let d0 = &v["directions"][0];
    assert_eq!(f(d0, "rx_pps"), 0.0);
    assert_eq!(f(d0, "loss_pct"), 0.0);
    assert_eq!(f(d0, "nic_rx_pct"), 0.0);
}

#[test]
fn counters_going_backwards_after_a_restart_are_clamped_to_zero() {
    let v = view(Some(&cur()), &prev()); // "restart": everything smaller, uptime smaller
    for dir in v["directions"].as_array().unwrap() {
        for k in [
            "rx_pps",
            "tx_pps",
            "rx_bps",
            "tx_bps",
            "loss_pct",
            "nic_tx_pct",
            "nic_rx_pct",
        ] {
            let x = f(dir, k);
            assert!(x == 0.0, "{k} must be 0.0 after a counter reset, got {x}");
        }
    }
}

#[test]
fn delay_histogram_has_bins_and_edges() {
    let h = serde_json::to_value(delay_histogram(&cur(), 0).unwrap()).unwrap();
    assert_eq!(h["dir"], 0);
    assert_close(f(&h, "lo_ns"), 1000.0);
    assert_close(f(&h, "bin_ns"), 10.0);
    assert_eq!(h["bins"].as_array().unwrap().len(), 256);
    assert_eq!(h["bins"][100], 5000);
    assert_eq!(h["underflow"], 3);
    assert_eq!(h["overflow"], 4);
    let edges = h["edges_ns"].as_array().unwrap();
    assert_eq!(edges.len(), 257);
    assert_close(edges[0].as_f64().unwrap(), 1000.0);
    assert_close(edges[256].as_f64().unwrap(), 1000.0 + 256.0 * 10.0);
    let h1 = serde_json::to_value(delay_histogram(&cur(), 1).unwrap()).unwrap();
    assert_close(f(&h1, "bin_ns"), 20.0);
    // direction 1 has 100 bins (num_bins), not 256: arrays and edges follow
    assert_eq!(h1["bins"].as_array().unwrap().len(), 100);
    assert_eq!(h1["bins"][39], 5000);
    let e1 = h1["edges_ns"].as_array().unwrap();
    assert_eq!(e1.len(), 101);
    assert_close(e1[100].as_f64().unwrap(), 2000.0 + 100.0 * 20.0);
}

#[test]
fn delay_histogram_for_unknown_direction_is_none() {
    assert!(delay_histogram(&cur(), 2).is_none());
}

// ---- rings and hairpins (spec v10) -------------------------------------------------------------

/// Ports 0, 1, 2 (a, b, c), links `a-to-b` (0 -> 1), `b-to-c` (1 -> 2), `c-to-a` (2 -> 0) and
/// the hairpin `loop` on port 3 (3 -> 3). Port counters are chosen so that every mapping error
/// changes a number: ibytes = 1000 * (port + 1) per second, obytes = 100_000 * (port + 1).
fn ring_snapshot(t_s: u64) -> dpds_controller::stats::Snapshot {
    let names = ["0000:17:00.0", "0000:65:00.0", "0000:b3:00.0", "net_null0"];
    let links = [
        ("a-to-b", 0usize, 1usize),
        ("b-to-c", 1, 2),
        ("c-to-a", 2, 0),
        ("loop", 3, 3),
    ];
    let mut ports = json!({"num_ports": 4, "num_links": 4, "num_directions": 4});
    for (p, n) in names.iter().enumerate() {
        ports[format!("port{p}_name")] = json!(n);
    }
    for (d, (link, rx, tx)) in links.iter().enumerate() {
        ports[format!("dir{d}_link")] = json!(link);
        ports[format!("dir{d}_rx_port")] = json!(rx);
        ports[format!("dir{d}_tx_port")] = json!(tx);
        ports[format!("dir{d}_rx_name")] = json!(names[*rx]);
        ports[format!("dir{d}_tx_name")] = json!(names[*tx]);
        ports[format!("dir{d}_lcore")] = json!(2 + d);
    }
    let dir = |d: usize| {
        let rx = 1_000 * (d as u64 + 1) * t_s;
        json!({
            "uptime_ns": t_s * 1_000_000_000, "polls": 1, "rx_pkts": rx,
            "dpds_rejects": 0, "loss_drops": 0, "limiter_drops": 0, "buffer_full_drops": 0,
            "pops": rx, "tx_pkts": rx, "tx_sent": rx, "tx_drops": 0, "buffer_len_max": 1,
            "delay_mean_ns": "1.00", "delay_std_ns": "0.00", "delay_min_ns": 1, "delay_max_ns": 1,
            "lateness_mean_ns": "0.00", "lateness_max_ns": 0,
            "first_loss_reason": "Clean", "first_loss_detail": "", "first_loss_relative_ns": 0
        })
    };
    let eth = |p: u64| {
        json!({"ipackets": 0, "opackets": 0, "ibytes": 1_000 * (p + 1) * t_s,
               "obytes": 100_000 * (p + 1) * t_s, "imissed": 0, "ierrors": 0, "oerrors": 0,
               "rx_nombuf": 0})
    };
    dpds_controller::stats::Snapshot {
        ports,
        dirs: (0..4).map(dir).collect(),
        eth: (0..4).map(eth).collect(),
        hists: (0..4).map(|_| hist_json(0.0, 1.0, 16)).collect(),
    }
}

#[test]
fn ring_links_map_rx_to_the_ingress_port_and_tx_to_the_egress_port() {
    let v = view(Some(&ring_snapshot(1)), &ring_snapshot(2));
    let dirs = v["directions"].as_array().unwrap();
    assert_eq!(dirs.len(), 4);
    let want = [
        ("a-to-b", "0000:17:00.0", "0000:65:00.0", 0usize, 1usize),
        ("b-to-c", "0000:65:00.0", "0000:b3:00.0", 1, 2),
        ("c-to-a", "0000:b3:00.0", "0000:17:00.0", 2, 0),
        ("loop", "net_null0", "net_null0", 3, 3),
    ];
    for (d, (link, rx, tx, rx_port, tx_port)) in want.iter().enumerate() {
        let x = &dirs[d];
        assert_eq!(x["dir"], d, "{x}");
        assert_eq!(x["link"], *link);
        assert_eq!(x["rx"], *rx);
        assert_eq!(x["tx"], *tx);
        // +1 s: rx_bps = delta ibytes of the INGRESS port * 8, tx_bps = delta obytes of the EGRESS port * 8
        assert_close(f(x, "rx_bps"), 1_000.0 * (*rx_port as f64 + 1.0) * 8.0);
        assert_close(f(x, "tx_bps"), 100_000.0 * (*tx_port as f64 + 1.0) * 8.0);
        assert_close(f(x, "rx_pps"), 1_000.0 * (d as f64 + 1.0));
    }
}

#[test]
fn ring_ports_are_listed_once_each() {
    let v = view(Some(&ring_snapshot(1)), &ring_snapshot(2));
    let ports = v["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 4);
    assert_eq!(ports[3]["name"], "net_null0");
    assert_eq!(ports[2]["name"], "0000:b3:00.0");
}

#[test]
fn ring_histograms_are_per_link_index() {
    for d in 0..4 {
        let h = serde_json::to_value(delay_histogram(&ring_snapshot(2), d).unwrap()).unwrap();
        assert_eq!(h["dir"], d);
    }
    assert!(delay_histogram(&ring_snapshot(2), 4).is_none());
}
