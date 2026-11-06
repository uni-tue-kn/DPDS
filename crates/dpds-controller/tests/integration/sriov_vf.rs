//! SR-IOV virtual functions (mlx5 VFs passed into a VM have no sysfs `physfn` link).
//!
//! A port is a VF when its discovered interface has vendor 0x15b3 and one of the device IDs
//! `0x1014 0x1016 0x1018 0x101a 0xa2d3 0x101c 0x101e`, or when its sysfs `physfn` link exists.
//! The VF test is pinned through its two visible effects:
//!
//! - host hints: for a VF NIC of the posted links `firmware:`, `flow_control:`, `port_buffer:` and
//!   `pcie_maxreadreq:<pci>` are INFO without `commands`, their detail says it is an SR-IOV virtual
//!   function whose setting belongs to the physical function on the hypervisor, and no ethtool,
//!   dcb, mstconfig or mlxconfig runs for it. The host-only firmware query skips VFs (`spec55.rs`).
//! - `GET /api/stats`: a VF ingress port makes `nic_rx_port_buffer_discards`, `nic_rx_phy_discards`,
//!   `nic_rx_crc_errors`, `nic_rx_symbol_errors`, `nic_rx_length_errors` JSON null, a VF egress
//!   port `nic_tx_phy_discards`; `nic_rx_missed` and `nic_rx_nombuf` stay numbers; the shares use
//!   the counters that are available. The history is in `chart_history.rs`.
//!
//! `NIC0` is the port under test; the fake host's other mlx5 NIC `NIC1` (device 0x1017) is a PF.

use crate::common::*;
use serde_json::{json, Value};

const MLX: &str = "0x15b3";
const VF_IDS: [&str; 7] = [
    "0x1014", "0x1016", "0x1018", "0x101a", "0xa2d3", "0x101c", "0x101e",
];
const TOOLS: [&str; 4] = ["ethtool", "dcb", "mstconfig", "mlxconfig"];
const MLX_KINDS: [&str; 4] = ["flow_control", "port_buffer", "pcie_maxreadreq", "firmware"];
const OTHER_KINDS: [&str; 2] = ["flow_control", "pcie_maxreadreq"];

fn set_device(host: &FakeHost, vendor: &str, device: &str) {
    host.add_nic(&Nic {
        vendor: vendor.into(),
        device: device.into(),
        // lspci of a guest VF: MaxReadReq 128 (the PF's value applies, the VF's field is reserved)
        max_read_req: Some(128),
        ..Nic::mlx5(NIC0, "ens1f0", Some(0))
    });
}

struct Probe {
    hints: Value,
    tool_calls: Vec<Vec<String>>,
}

/// The hints of a link on `NIC0` with the given PCI vendor and device ID (and a `physfn` link),
/// with every tool installed so that a call that should not happen would show. The outputs are
/// those of a real guest VF: `ethtool -a` says autoneg, rx and tx off (meaningless on a VF, and
/// the hint would say OK), `dcb buffer show` fails with "Operation not supported".
fn probe(vendor: &str, device: &str, physfn: bool) -> Probe {
    let host = FakeHost::bare_metal();
    set_device(&host, vendor, device);
    if physfn {
        host.make_vf(NIC0, "0000:ff:00.0");
    }
    let exec = host.exec();
    let off = EthtoolState {
        autoneg: false,
        rx: false,
        tx: false,
    };
    fake_ethtool(&exec, "ens1f0", off);
    exec.on(
        &["dcb", "-j", "buffer", "show", "dev", "ens1f0"],
        Out::Fail(2, "Attribute read: Operation not supported".into()),
    );
    for tool in ["mstconfig", "mlxconfig"] {
        fake_mst(&exec, tool, NIC0, MstState::untuned());
    }
    let hints = hints_json(&host, &setup(vec![link(NIC0, NIC0)]));
    let tool_calls = exec
        .calls()
        .into_iter()
        .filter(|c| TOOLS.contains(&c[0].as_str()))
        .collect();
    Probe { hints, tool_calls }
}

fn assert_vf(what: &str, p: &Probe, kinds: &[&str]) {
    for kind in kinds {
        let id = nic_id(kind, NIC0);
        assert_level(&p.hints, &id, "info", "info");
        assert_no_commands(&p.hints, &id);
        for words in ["SR-IOV virtual function", "physical function", "hypervisor"] {
            assert!(
                detail_has(&p.hints, &id, words),
                "{what}: {id} detail lacks {words:?}: {}",
                hint(&p.hints, &id)
            );
        }
    }
    assert!(
        p.tool_calls.is_empty(),
        "{what}: no tool runs for a VF: {:?}",
        p.tool_calls
    );
}

fn assert_not_vf(what: &str, p: &Probe, kinds: &[&str]) {
    for kind in kinds {
        let id = nic_id(kind, NIC0);
        assert!(
            !detail_has(&p.hints, &id, "SR-IOV"),
            "{what}: {id} is not a VF: {}",
            hint(&p.hints, &id)
        );
    }
    assert!(
        p.tool_calls.iter().any(|c| c[0] == "ethtool"),
        "{what}: the pause settings are read: {:?}",
        p.tool_calls
    );
}

#[test]
fn every_listed_mellanox_device_id_is_a_vf() {
    for id in VF_IDS {
        assert_vf(id, &probe(MLX, id, false), &MLX_KINDS);
    }
}

#[test]
fn the_physical_function_and_a_foreign_vendors_id_are_not_vfs() {
    assert_not_vf("PF 0x101d", &probe(MLX, "0x101d", false), &MLX_KINDS);
    assert_not_vf("CX5 PF 0x1017", &probe(MLX, "0x1017", false), &MLX_KINDS);
    // a listed ID of another vendor is not an mlx5 VF
    assert_not_vf(
        "Intel 0x101e",
        &probe("0x8086", "0x101e", false),
        &OTHER_KINDS,
    );
}

#[test]
fn a_physfn_link_makes_any_device_a_vf() {
    assert_vf(
        "Intel with physfn",
        &probe("0x8086", "0x1592", true),
        &OTHER_KINDS,
    );
    assert_vf("PF id with physfn", &probe(MLX, "0x101d", true), &MLX_KINDS);
}

// ---- /api/stats ------------------------------------------------------------------------------

/// Counters that the VF test nulls on the ingress port, and on the egress port.
const RX_PHY: [&str; 5] = [
    "nic_rx_port_buffer_discards",
    "nic_rx_phy_discards",
    "nic_rx_crc_errors",
    "nic_rx_symbol_errors",
    "nic_rx_length_errors",
];
const TX_PHY: &str = "nic_tx_phy_discards";

/// Every xstat of port 0 and 1 has a value (a VF's `*_phy_*` names exist but never count).
fn loaded(t: u64) -> dpds_controller::stats::Snapshot {
    let dir = |rx: u64, drops: u64| DirSample {
        uptime_ns: 10_000_000_000 + 2_000_000_000 * t,
        rx_pkts: rx * t,
        tx_sent: rx * t,
        loss: 0,
        limiter: 0,
        full: 0,
        tx_drops: drops * t,
    };
    let quiet = || PortSample {
        ibytes: 1000,
        obytes: 1000,
        rx_missed_errors: 0,
        rx_mbuf_allocation_errors: 0,
    };
    let mut s = snapshot([dir(1_000_000, 250), dir(1_000_000, 0)], [quiet(), quiet()]);
    set_xstats(
        &mut s,
        0,
        &[
            ("rx_missed_errors", 1000 * t),
            ("rx_mbuf_allocation_errors", 500 * t),
            ("rx_prio0_buf_discard_packets", 2000 * t),
            ("rx_phy_discard_packets", 750 * t),
            ("tx_phy_discard_packets", 300 * t),
            ("rx_phy_crc_errors", 7 * t),
            ("rx_phy_symbol_errors", 8 * t),
            ("rx_phy_in_range_len_errors", 9 * t),
        ],
    );
    set_xstats(
        &mut s,
        1,
        &[
            ("rx_missed_errors", 100 * t),
            ("rx_mbuf_allocation_errors", 200 * t),
            ("rx_prio1_cong_discard_packets", 1000 * t),
            ("rx_phy_discard_packets", 400 * t),
            ("tx_phy_discard_packets", 1500 * t),
            ("rx_phy_crc_errors", 17 * t),
            ("rx_phy_symbol_errors", 18 * t),
            ("rx_phy_in_range_len_errors", 19 * t),
        ],
    );
    s
}

/// A running emulation of NIC0 -> NIC1 (link 0) and NIC1 -> NIC0 (link 1) whose NIC0 has the
/// given device ID, polled twice (window of 2 s) when `window` is set.
async fn stats_of(device: &str, ports: [&str; 2], window: bool) -> Value {
    let host = FakeHost::bare_metal();
    set_device(&host, MLX, device);
    let app = TestApp::new(host);
    app.start_ok(vec![link(ports[0], ports[1]), link(ports[1], ports[0])])
        .await;
    let mut cur = loaded(1);
    for key in ["port0_name", "dir0_rx_name", "dir1_tx_name"] {
        cur.ports[key] = json!(ports[0]);
    }
    for key in ["port1_name", "dir0_tx_name", "dir1_rx_name"] {
        cur.ports[key] = json!(ports[1]);
    }
    let prev = window.then(|| loaded(0));
    app.telemetry.set(prev, cur);
    let r = app.get("/api/stats").await;
    assert_eq!(r.status, 200);
    r.json()
}

fn counters(v: &Value, d: usize) -> &Value {
    &v["directions"][d]["counters"]
}

fn assert_null(c: &Value, keys: &[&str]) {
    for k in keys {
        assert!(c[*k].is_null(), "{k} is JSON null, got {}: {c}", c[*k]);
    }
}

fn assert_numbers(c: &Value, keys: &[&str]) {
    for k in keys {
        assert!(c[*k].is_u64(), "{k} is a number, got {}: {c}", c[*k]);
    }
}

#[tokio::test]
async fn a_vf_ingress_or_egress_port_nulls_its_physical_port_counters() {
    let v = stats_of("0x101e", [NIC0, NIC1], false).await;
    // link 0: ingress NIC0 (VF), egress NIC1 (PF)
    let c = counters(&v, 0);
    assert_null(c, &RX_PHY);
    assert_eq!(c[TX_PHY], 1500, "egress port 1 is a PF: {c}");
    assert_eq!(
        (c["nic_rx_missed"].as_u64(), c["nic_rx_nombuf"].as_u64()),
        (Some(1000), Some(500))
    );
    // link 1: ingress NIC1 (PF), egress NIC0 (VF)
    let c = counters(&v, 1);
    assert_numbers(c, &RX_PHY);
    assert_eq!(c["nic_rx_port_buffer_discards"], 1000);
    assert_eq!(c["nic_rx_crc_errors"], 17);
    assert_null(c, &[TX_PHY]);
    assert_eq!(
        (c["nic_rx_missed"].as_u64(), c["nic_rx_nombuf"].as_u64()),
        (Some(100), Some(200))
    );
}

#[tokio::test]
async fn the_shares_use_only_the_counters_that_are_available() {
    let v = stats_of("0x101e", [NIC0, NIC1], true).await;
    let (d0, d1) = (&v["directions"][0], &v["directions"][1]);
    // link 0, VF ingress: (1000 missed + 500 nombuf) / 1e6; the VF's prio and rx phy values do not
    assert_close(f(d0, "nic_rx_pct"), 0.15);
    // link 0, PF egress: (250 tx_drops + 1500 tx phy) / 1e6
    assert_close(f(d0, "nic_tx_pct"), 0.175);
    // link 1, PF ingress: 100 + 200 + 1000 + 400 over 1e6
    assert_close(f(d1, "nic_rx_pct"), 0.17);
    // link 1, VF egress: no tx_drops, and the VF's tx phy value does not count
    assert_close(f(d1, "nic_tx_pct"), 0.0);
}

#[tokio::test]
async fn a_pf_and_a_vdev_keep_their_numbers() {
    // NIC0 is a PF
    let v = stats_of("0x101d", [NIC0, NIC1], true).await;
    for d in 0..2 {
        assert_numbers(counters(&v, d), &[&RX_PHY[..], &[TX_PHY]].concat());
    }
    // (1000 + 500 + 2000 + 750) / 1e6
    assert_close(f(&v["directions"][0], "nic_rx_pct"), 0.425);
    // port 0 is a vdev (the host's NIC0 is a VF, but the run does not use it)
    let v = stats_of("0x101e", ["net_null0", NIC1], true).await;
    for d in 0..2 {
        assert_numbers(counters(&v, d), &[&RX_PHY[..], &[TX_PHY]].concat());
    }
    // (1000 + 500 + 2000 + 750) / 1e6
    assert_close(f(&v["directions"][0], "nic_rx_pct"), 0.425);
}

#[tokio::test]
async fn openapi_marks_the_vf_counters_nullable_and_says_why() {
    let app = TestApp::new(FakeHost::bare_metal());
    let doc = app.get("/api/openapi.json").await.json();
    let schemas = &doc["components"]["schemas"];
    let nullable = |p: &Value| {
        p["nullable"] == true
            || p["type"]
                .as_array()
                .is_some_and(|t| t.iter().any(|x| x == "null"))
            || p["oneOf"]
                .as_array()
                .or_else(|| p["anyOf"].as_array())
                .is_some_and(|a| a.iter().any(|x| x["type"] == "null"))
    };
    let fields: Vec<&str> = RX_PHY.iter().copied().chain([TX_PHY]).collect();
    for (schema, keys) in [
        ("Counters", &fields[..]),
        (
            "HistorySample",
            &["nic_rx_port_buffer_discards", "nic_rx_phy_discards", TX_PHY][..],
        ),
        (
            "HistoryTotals",
            &["nic_rx_port_buffer_discards", "nic_rx_phy_discards", TX_PHY][..],
        ),
    ] {
        for k in keys {
            let p = &schemas[schema]["properties"][*k];
            assert!(nullable(p), "{schema}.{k} is nullable: {p}");
            let text = p["description"].as_str().unwrap_or_default();
            assert!(
                text.contains("null when the port is an SR-IOV virtual function")
                    && text.contains("physical function on the hypervisor"),
                "{schema}.{k} documents the null: {p}"
            );
        }
    }
    // the valid ones stay plain numbers
    for k in ["nic_rx_missed", "nic_rx_nombuf"] {
        assert!(!nullable(&schemas["Counters"]["properties"][k]), "{k}");
    }
}
