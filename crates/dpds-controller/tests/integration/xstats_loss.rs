//! Every lost frame counted, from the NIC's xstats instead of `rte_eth_stats`.
//!
//! The poller asks `/ethdev/xstats,<p>` (a flat `{name: value}` reply) and `compute_stats` reads
//! everything NIC-related from it. The fake replies are the `snapshot()` of `common` (two ports,
//! link 0 = port 0 -> port 1, link 1 = port 1 -> port 0) with [`set_xstats`] on top. Every cause
//! gets a different value on each port, so a link that reads the wrong port shows.
//!
//! Contract the tests pin:
//!
//! - A link's `counters` (JSON of `DirectionStats`) carry, from the INGRESS port's xstats:
//!   `nic_rx_missed` (`rx_missed_errors`), `nic_rx_nombuf` (`rx_mbuf_allocation_errors`),
//!   `nic_rx_port_buffer_discards` (sum of `^rx_prio[0-7]_(buf|cong)_discard_packets$`),
//!   `nic_rx_phy_discards` (`rx_phy_discard_packets`), and the diagnostics `nic_rx_crc_errors`
//!   (`rx_phy_crc_errors`), `nic_rx_symbol_errors` (`rx_phy_symbol_errors`), `nic_rx_length_errors`
//!   (`rx_phy_in_range_len_errors`); from the EGRESS port's xstats `nic_tx_phy_discards`
//!   (`tx_phy_discard_packets`). All u64,
//!   cumulative, 0 for a name the NIC does not report (or a port without a reply).
//! - `HistorySample` / `HistoryTotals` carry the nine causes (not the diagnostics).
//! - `nic_rx_pct` is the share of the window's rx packets lost to the four RX-side NIC causes,
//!   `nic_tx_pct` that of `tx_drops` plus `nic_tx_phy_discards`; diagnostics are in neither.

use crate::common::*;
use crate::fake_telemetry::{short_tempdir, FakeTelemetryServer};
use dpds_controller::stats::{compute_stats, HistorySample, HistoryTotals, Snapshot};
use dpds_controller::telemetry::{poll_once, socket_path, Client};
use serde_json::{json, Value};

fn dir(uptime_ns: u64, rx: u64, tx: u64, tx_drops: u64) -> DirSample {
    DirSample {
        uptime_ns,
        rx_pkts: rx,
        tx_sent: tx,
        loss: 0,
        limiter: 0,
        full: 0,
        tx_drops,
    }
}

fn quiet_ports() -> [PortSample; 2] {
    [0, 1].map(|_| PortSample {
        ibytes: 1000,
        obytes: 1000,
        rx_missed_errors: 0,
        rx_mbuf_allocation_errors: 0,
    })
}

/// Port 0 and port 1 with different values for every loss xstat and diagnostic, plus names that
/// look like loss xstats but are not (they must be ignored by the prio sum).
fn loaded() -> Snapshot {
    let mut s = snapshot(
        [
            dir(1_000_000_000, 100, 100, 0),
            dir(1_000_000_000, 100, 100, 0),
        ],
        quiet_ports(),
    );
    set_xstats(
        &mut s,
        0,
        &[
            ("rx_missed_errors", 1),
            ("rx_mbuf_allocation_errors", 2),
            ("rx_prio0_buf_discard_packets", 10),
            ("rx_prio1_cong_discard_packets", 20),
            ("rx_prio4_buf_discard_packets", 40),
            ("rx_prio7_cong_discard_packets", 70),
            ("rx_phy_discard_packets", 5),
            ("tx_phy_discard_packets", 6),
            ("rx_phy_crc_errors", 7),
            ("rx_phy_symbol_errors", 8),
            ("rx_phy_in_range_len_errors", 9),
            // not matching ^rx_prio[0-7]_(buf|cong)_discard_packets$
            ("rx_prio8_buf_discard_packets", 1000),
            ("rx_prio0_buf_discard_bytes", 2000),
            ("rx_prio10_cong_discard_packets", 3000),
            ("tx_prio0_buf_discard_packets", 4000),
            ("rx_prio0_discard_packets", 5000),
            ("rx_prio0_packets", 6000),
        ],
    );
    set_xstats(
        &mut s,
        1,
        &[
            ("rx_missed_errors", 100),
            ("rx_mbuf_allocation_errors", 200),
            ("rx_prio2_buf_discard_packets", 1000),
            ("rx_prio2_cong_discard_packets", 2000),
            ("rx_prio3_buf_discard_packets", 4000),
            ("rx_prio5_buf_discard_packets", 8000),
            ("rx_phy_discard_packets", 500),
            ("tx_phy_discard_packets", 600),
            ("rx_phy_crc_errors", 700),
            ("rx_phy_symbol_errors", 800),
            ("rx_phy_in_range_len_errors", 900),
        ],
    );
    s
}

fn counters(s: &Snapshot, d: usize) -> Value {
    serde_json::to_value(compute_stats(None, s)).unwrap()["directions"][d]["counters"].clone()
}

fn num(c: &Value, k: &str) -> u64 {
    c[k].as_u64()
        .unwrap_or_else(|| panic!("counters.{k} is not a u64 in {c}"))
}

/// The nine NIC-side causes and three diagnostics of a link, in one tuple-free list.
const NIC_KEYS: [&str; 8] = [
    "nic_rx_missed",
    "nic_rx_nombuf",
    "nic_rx_port_buffer_discards",
    "nic_rx_phy_discards",
    "nic_tx_phy_discards",
    "nic_rx_crc_errors",
    "nic_rx_symbol_errors",
    "nic_rx_length_errors",
];

fn nic(c: &Value) -> [u64; 8] {
    NIC_KEYS.map(|k| num(c, k))
}

// ---- the poller asks for xstats ----------------------------------------------------------

#[test]
fn the_poller_asks_for_xstats_and_not_for_stats() {
    let dir = short_tempdir();
    let path = socket_path(dir.path());
    let server = FakeTelemetryServer::serving(&path, 4242, loaded());
    let (mut client, _greeting) = Client::connect(&path).expect("connect to the fake socket");
    let snap = poll_once(&mut client).expect("a poll");
    let queries = server.queries();
    for q in ["/ethdev/xstats,0", "/ethdev/xstats,1"] {
        assert!(queries.iter().any(|x| x == q), "{q} is asked: {queries:?}");
    }
    assert!(
        !queries.iter().any(|q| q.starts_with("/ethdev/stats")),
        "the basic stats are not asked any more: {queries:?}"
    );
    assert_eq!(snap.eth.len(), 2, "one reply per port");
    assert_eq!(snap.eth[0]["rx_phy_discard_packets"], 5, "{}", snap.eth[0]);
    assert_eq!(
        snap.eth[1]["tx_phy_discard_packets"], 600,
        "{}",
        snap.eth[1]
    );
}

// ---- rates come from the good_* xstats ----------------------------------------------------

#[test]
fn rates_come_from_rx_good_bytes_and_tx_good_bytes() {
    let mk = |t_s: u64| {
        let mut s = snapshot(
            [
                dir(t_s * 1_000_000_000, 1000 * t_s, 1000 * t_s, 0),
                dir(t_s * 1_000_000_000, 1000 * t_s, 1000 * t_s, 0),
            ],
            [
                PortSample {
                    ibytes: 1_000_000 * t_s,
                    obytes: 3_000_000 * t_s,
                    rx_missed_errors: 0,
                    rx_mbuf_allocation_errors: 0,
                },
                PortSample {
                    ibytes: 5_000_000 * t_s,
                    obytes: 7_000_000 * t_s,
                    rx_missed_errors: 0,
                    rx_mbuf_allocation_errors: 0,
                },
            ],
        );
        // names that are not the good_* ones must not feed the rates
        for p in 0..2 {
            set_xstats(
                &mut s,
                p,
                &[
                    ("rx_bytes", 999_999_999_999 * t_s),
                    ("tx_bytes", 999_999_999_999 * t_s),
                    ("rx_wqe_errors", 5),
                ],
            );
        }
        s
    };
    let v = serde_json::to_value(compute_stats(Some(&mk(1)), &mk(2))).unwrap();
    // link 0: rx port 0 (1 MB/s), tx port 1 (7 MB/s); link 1: rx port 1 (5 MB/s), tx port 0 (3 MB/s)
    assert_close(f(&v["directions"][0], "rx_bps"), 1_000_000.0 * 8.0);
    assert_close(f(&v["directions"][0], "tx_bps"), 7_000_000.0 * 8.0);
    assert_close(f(&v["directions"][1], "rx_bps"), 5_000_000.0 * 8.0);
    assert_close(f(&v["directions"][1], "tx_bps"), 3_000_000.0 * 8.0);
}

// ---- every loss xstat lands in the right link's counter ----------------------------------

#[test]
fn each_loss_xstat_lands_in_its_counter_of_the_right_link() {
    let s = loaded();
    // link 0: ingress port 0, egress port 1; counters in the order of NIC_KEYS
    //   missed, nombuf, port buffer (10+20+40+70), rx phy, tx phy (egress port 1), crc, symbol, len
    assert_eq!(
        nic(&counters(&s, 0)),
        [1, 2, 140, 5, 600, 7, 8, 9],
        "{}",
        counters(&s, 0)
    );
    // link 1: ingress port 1, egress port 0
    assert_eq!(
        nic(&counters(&s, 1)),
        [100, 200, 15_000, 500, 6, 700, 800, 900],
        "{}",
        counters(&s, 1)
    );
}

#[test]
fn the_port_buffer_discards_sum_buf_and_cong_of_priorities_zero_to_seven_only() {
    let mut s = snapshot(
        [dir(1_000_000_000, 1, 1, 0), dir(1_000_000_000, 1, 1, 0)],
        quiet_ports(),
    );
    let mut want = 0;
    let mut pairs = Vec::new();
    let names: Vec<String> = (0..8)
        .flat_map(|p| {
            ["buf", "cong"]
                .iter()
                .map(move |k| format!("rx_prio{p}_{k}_discard_packets"))
        })
        .collect();
    for (i, n) in names.iter().enumerate() {
        let v = 1u64 << i; // distinct bits: a missed or doubled name shows in the sum
        want += v;
        pairs.push((n.as_str(), v));
    }
    pairs.push(("rx_prio8_cong_discard_packets", 1 << 20));
    pairs.push(("rx_prio_buf_discard_packets", 1 << 21));
    set_xstats(&mut s, 0, &pairs);
    assert_eq!(want, (1 << 16) - 1);
    assert_eq!(num(&counters(&s, 0), "nic_rx_port_buffer_discards"), want);
    assert_eq!(num(&counters(&s, 1), "nic_rx_port_buffer_discards"), 0);
}

#[test]
fn a_hairpin_link_reads_both_sides_from_its_single_port() {
    let mut s = loaded();
    s.ports["dir0_rx_port"] = json!(0);
    s.ports["dir0_tx_port"] = json!(0);
    // rx causes and tx phy discards all of port 0 (link 1 is unchanged)
    assert_eq!(
        nic(&counters(&s, 0)),
        [1, 2, 140, 5, 6, 7, 8, 9],
        "{}",
        counters(&s, 0)
    );
    assert_eq!(
        nic(&counters(&s, 1)),
        [100, 200, 15_000, 500, 6, 700, 800, 900],
        "link 1 egresses on port 0: {}",
        counters(&s, 1)
    );
}

// ---- missing names are 0 ------------------------------------------------------------------

#[test]
fn names_a_nic_does_not_report_count_as_zero() {
    // a NIC with the generic xstats only (no mlx5 names): every NIC cause is 0
    let s = snapshot(
        [dir(1_000_000_000, 1, 1, 0), dir(1_000_000_000, 1, 1, 0)],
        quiet_ports(),
    );
    for d in 0..2 {
        assert_eq!(nic(&counters(&s, d)), [0; 8], "{}", counters(&s, d));
    }
    // an empty reply, and a port without any reply
    let mut s = loaded();
    s.eth[0] = json!({});
    s.eth.truncate(1);
    assert_eq!(
        nic(&counters(&s, 0)),
        [0; 8],
        "empty reply and no egress port"
    );
    assert_eq!(
        nic(&counters(&s, 1)),
        [0; 8],
        "no ingress port, and the egress port 0 has an empty reply"
    );
}

#[test]
fn one_present_name_does_not_need_the_others() {
    let mut s = snapshot(
        [dir(1_000_000_000, 1, 1, 0), dir(1_000_000_000, 1, 1, 0)],
        quiet_ports(),
    );
    set_xstats(&mut s, 0, &[("rx_phy_discard_packets", 42)]);
    set_xstats(&mut s, 1, &[("tx_phy_discard_packets", 7)]);
    assert_eq!(nic(&counters(&s, 0)), [0, 0, 0, 42, 7, 0, 0, 0]);
    assert_eq!(nic(&counters(&s, 1)), [0, 0, 0, 0, 0, 0, 0, 0]);
}

// ---- names ---------------------------------------------------------------------------------

#[test]
fn the_counters_have_the_new_names_and_not_the_old_one() {
    let c = counters(&loaded(), 0);
    assert!(
        c.get("nic_rx_imissed").is_none(),
        "renamed to nic_rx_missed: {c}"
    );
    assert!(
        c.get("nic_tx_drops").is_none(),
        "tx_drops keeps its name here: {c}"
    );
    assert_eq!(num(&c, "tx_drops"), 0);
}

// ---- diagnostics pass through but are not counted -----------------------------------------

#[test]
fn diagnostics_are_passed_through_in_the_counters_only() {
    let s = loaded();
    let c = counters(&s, 0);
    assert_eq!(
        [
            num(&c, "nic_rx_crc_errors"),
            num(&c, "nic_rx_symbol_errors"),
            num(&c, "nic_rx_length_errors")
        ],
        [7, 8, 9]
    );
    // history samples and totals carry the causes, never the diagnostics
    let view = &compute_stats(None, &s).directions[0];
    let sample = serde_json::to_value(HistorySample::new(5000, view)).unwrap();
    let totals = serde_json::to_value(HistoryTotals::from(&view.counters)).unwrap();
    for k in [
        "nic_rx_crc_errors",
        "nic_rx_symbol_errors",
        "nic_rx_length_errors",
    ] {
        assert!(sample.get(k).is_none(), "{k} not in the sample: {sample}");
        assert!(totals.get(k).is_none(), "{k} not in the totals: {totals}");
    }
}

#[test]
fn diagnostics_move_no_loss_total_and_no_share() {
    let mk = |t_s: u64, diag: u64| {
        let mut s = snapshot(
            [
                dir(t_s * 1_000_000_000, 1_000_000 * t_s, 1_000_000 * t_s, 0),
                dir(t_s * 1_000_000_000, 1_000_000 * t_s, 1_000_000 * t_s, 0),
            ],
            quiet_ports(),
        );
        for p in 0..2 {
            set_xstats(
                &mut s,
                p,
                &[
                    ("rx_phy_crc_errors", diag),
                    ("rx_phy_symbol_errors", diag),
                    ("rx_phy_in_range_len_errors", diag),
                ],
            );
        }
        s
    };
    let view = serde_json::to_value(compute_stats(Some(&mk(1, 0)), &mk(2, 50_000))).unwrap();
    for d in view["directions"].as_array().unwrap() {
        let c = &d["counters"];
        let sum: u64 = [
            "loss_drops",
            "limiter_drops",
            "buffer_full_drops",
            "tx_drops",
            "nic_rx_missed",
            "nic_rx_nombuf",
            "nic_rx_port_buffer_discards",
            "nic_rx_phy_discards",
            "nic_tx_phy_discards",
        ]
        .iter()
        .map(|k| num(c, k))
        .sum();
        assert_eq!(sum, 0, "diagnostics are not losses: {c}");
        assert_close(f(d, "nic_rx_pct"), 0.0);
        assert_close(f(d, "nic_tx_pct"), 0.0);
        assert_eq!(num(c, "nic_rx_crc_errors"), 50_000);
    }
}

// ---- history samples and totals ---------------------------------------------------------------

#[test]
fn history_samples_and_totals_carry_the_nine_causes() {
    let s = loaded();
    let view = &compute_stats(None, &s).directions[0];
    let sample = serde_json::to_value(HistorySample::new(5000, view)).unwrap();
    let totals = serde_json::to_value(HistoryTotals::from(&view.counters)).unwrap();
    // link 0: ingress port 0, egress port 1
    let causes = json!({
        "loss_drops": 0, "limiter_drops": 0, "buffer_full_drops": 0, "nic_tx_drops": 0,
        "nic_rx_missed": 1, "nic_rx_nombuf": 2, "nic_rx_port_buffer_discards": 140,
        "nic_rx_phy_discards": 5, "nic_tx_phy_discards": 600,
    });
    for (what, v) in [("sample", &sample), ("totals", &totals)] {
        for (k, want) in causes.as_object().unwrap() {
            assert_eq!(&v[k], want, "{what}.{k}: {v}");
        }
        assert!(v.get("nic_rx_imissed").is_none(), "{what}: renamed: {v}");
    }
    assert_eq!(sample["t_ms"], 5000);
    assert_eq!(totals["rx_pkts"], 100);
}

#[test]
fn history_nic_tx_drops_is_still_the_dpds_tx_drops() {
    let s = snapshot(
        [
            dir(1_000_000_000, 100, 90, 10),
            dir(1_000_000_000, 100, 100, 0),
        ],
        quiet_ports(),
    );
    let view = &compute_stats(None, &s).directions[0];
    let sample = serde_json::to_value(HistorySample::new(1, view)).unwrap();
    let totals = serde_json::to_value(HistoryTotals::from(&view.counters)).unwrap();
    assert_eq!(sample["nic_tx_drops"], 10);
    assert_eq!(totals["nic_tx_drops"], 10);
    assert_eq!(sample["nic_tx_phy_discards"], 0, "a different counter");
}

// ---- the shares --------------------------------------------------------------------------------

#[test]
fn the_nic_shares_include_the_new_causes() {
    // window of 2 s, link 0 receives 1,000,000 packets; port 0 (ingress) loses 1,000 missed, 500
    // nombuf, 2,000 + 250 prio (buf and cong), 750 rx phy; port 1 (egress) 1,500 tx phy;
    // dpds' own tx_drops +250
    let mk = |t_s: u64| {
        let mut s = snapshot(
            [
                dir(
                    10_000_000_000 + 2_000_000_000 * t_s,
                    1_000_000 * t_s,
                    1_000_000 * t_s,
                    250 * t_s,
                ),
                dir(10_000_000_000 + 2_000_000_000 * t_s, 5 * t_s, 5 * t_s, 0),
            ],
            quiet_ports(),
        );
        set_xstats(
            &mut s,
            0,
            &[
                ("rx_missed_errors", 1_000 * t_s),
                ("rx_mbuf_allocation_errors", 500 * t_s),
                ("rx_prio0_buf_discard_packets", 2_000 * t_s),
                ("rx_prio6_cong_discard_packets", 250 * t_s),
                ("rx_phy_discard_packets", 750 * t_s),
                ("rx_phy_crc_errors", 99_999 * t_s),
            ],
        );
        set_xstats(&mut s, 1, &[("tx_phy_discard_packets", 1_500 * t_s)]);
        s
    };
    let v = compute_stats(Some(&mk(0)), &mk(1));
    let d0 = &v.directions[0];
    // (1,000 + 500 + 2,250 + 750) / 1e6 = 0.45 %
    assert_close(d0.nic_rx_pct, 0.45);
    // (250 tx_drops + 1,500 tx phy) / 1e6 = 0.175 %
    assert_close(d0.nic_tx_pct, 0.175);
}
