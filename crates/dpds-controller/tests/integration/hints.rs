//! Host hints on fake trees (some suggest root commands, see `host_commands.rs`; none changes the
//! host). `GET /api/host/hints` gives the host-only hints, `POST /api/host/hints` with a start
//! body adds the link-dependent ones for that configuration; the tests use
//! `hints_json(host, setup)` (GET without links, POST with).

use crate::common::*;
use serde_json::Value;

const NEED_PER_MBUF: u64 = 2432; // ~2.4 KB per mbuf
const PAGE: u64 = 2 * 1024 * 1024;

/// One link with the default 100000 packet buffer: pool = capacity + 2*4096 + 768 + 64.
fn pool_bytes_for(links: u64) -> u64 {
    links * (100_000 + 4096 * 2 + 768 + 64) * NEED_PER_MBUF
}

fn one_link_nic0() -> Setup {
    setup(vec![link(NIC0, NIC0)])
}

#[test]
fn every_check_yields_exactly_one_hint_with_the_documented_fields() {
    for host in [FakeHost::bare_metal(), FakeHost::vm(), FakeHost::bare()] {
        let h = hints_json(&host, &one_link_nic0());
        for id in HINT_IDS {
            let x = hint(&h, id);
            for k in ["id", "level", "title", "detail", "suggestion"] {
                assert!(x.get(k).is_some(), "hint {id} lacks `{k}`: {x}");
            }
            assert!(x["title"].as_str().is_some_and(|t| !t.is_empty()), "{x}");
            rank(x["level"].as_str().unwrap());
        }
    }
}

#[test]
fn fully_tuned_bare_metal_host_has_no_warning() {
    let h = hints_json(&FakeHost::bare_metal(), &one_link_nic0());
    for id in [
        "isolation",
        "isolated_cores",
        "hugepages",
        "numa",
        "governor",
        "irq_affinity",
        "iommu",
        "mtu",
        "swappiness",
        "zone_reclaim_mode",
        "rcu_nocb_poll",
    ] {
        assert_level(&h, id, "ok", "ok");
    }
    assert_level(&h, &nic_id("pcie_maxreadreq", NIC0), "ok", "ok");
    assert!(
        h.as_array()
            .unwrap()
            .iter()
            .all(|x| x["level"] != "error" && x["level"] != "warn"),
        "{h}"
    );
}

#[test]
fn there_is_no_mprq_hint_and_no_hint_discusses_devargs() {
    for host in [FakeHost::bare_metal(), FakeHost::vm(), FakeHost::bare()] {
        for state in [setup(vec![]), one_link_nic0()] {
            let h = hints_json(&host, &state);
            assert!(
                h.as_array().unwrap().iter().all(|x| x["id"] != "mprq"),
                "{h}"
            );
            assert_no_devargs_hint(&h);
        }
    }
}

// ---- isolation -----------------------------------------------------------

#[test]
fn isolation_all_three_parameters_ok_only_isolcpus_warn_none_error() {
    let st = setup(vec![]);
    let h = FakeHost::bare_metal();
    assert_level(&hints_json(&h, &st), "isolation", "ok", "ok");

    h.cmdline("ro quiet isolcpus=2-3,6-7");
    assert_level(&hints_json(&h, &st), "isolation", "warn", "warn");

    h.cmdline("ro quiet");
    h.write("sys/devices/system/cpu/isolated", "\n");
    assert_level(&hints_json(&h, &st), "isolation", "error", "error");
}

#[test]
fn isolation_on_a_host_without_any() {
    let h = hints_json(&FakeHost::bare(), &setup(vec![]));
    assert_level(&h, "isolation", "error", "error");
}

// ---- isolated core count -------------------------------------------------

fn vdev_links(n: u32) -> Setup {
    setup(
        (0..n)
            .map(|i| {
                let p = format!("net_null{i}");
                link(&p, &p)
            })
            .collect(),
    )
}

#[test]
fn isolated_core_count_versus_links() {
    let host = FakeHost::bare_metal(); // 4 isolated
    assert_level(
        &hints_json(&host, &vdev_links(4)),
        "isolated_cores",
        "ok",
        "ok",
    );
    assert_level(
        &hints_json(&host, &vdev_links(5)),
        "isolated_cores",
        "error",
        "error",
    );
    // no links at all (the host-only hints) or one harmless link: nothing to complain about
    assert_level(
        &hints_json(&host, &filler()),
        "isolated_cores",
        "ok",
        "info",
    );
}

// ---- hugepages -----------------------------------------------------------

fn hugepages_hint(need_factor_per_node: f64, links: u64) -> Value {
    let host = FakeHost::bare_metal();
    let pages = (pool_bytes_for(links) as f64 * need_factor_per_node / PAGE as f64).ceil() as u64;
    host.hugepages(&[0, 1], pages, pages);
    // first link on a real NIC, the others on vdevs: only the pool need matters here
    let links: Vec<_> = (0..links as u32)
        .map(|i| {
            let port = if i == 0 {
                NIC0.to_string()
            } else {
                format!("net_null{i}")
            };
            link(&port, &port)
        })
        .collect();
    hints_json(&host, &setup(links))
}

#[test]
fn hugepages_plenty_ok() {
    assert_level(&hugepages_hint(3.0, 1), "hugepages", "ok", "ok");
}

#[test]
fn hugepages_well_below_need_is_an_error() {
    // global = 2 x per node = 1 x need, per node 0.5 x need: error under either reading
    // of "free" (per node of the NIC, or global with safety margin)
    assert_level(&hugepages_hint(0.25, 1), "hugepages", "error", "error");
}

#[test]
fn hugepage_need_grows_with_the_number_of_links() {
    // 3x one link's need per node is plenty for one link but not for four
    let one = hugepages_hint(3.0, 1);
    assert_level(&one, "hugepages", "ok", "ok");
    let four = hugepages_hint(0.25, 4);
    assert_level(&four, "hugepages", "error", "error");
}

#[test]
fn no_hugepages_configured_with_links_is_an_error() {
    let h = hints_json(&FakeHost::bare(), &one_link_nic0());
    assert_level(&h, "hugepages", "error", "error");
}

#[test]
fn no_hugepages_configured_without_links_still_warns_at_least() {
    let h = hints_json(&FakeHost::bare(), &setup(vec![]));
    assert_level(&h, "hugepages", "warn", "error");
}

// ---- NUMA ----------------------------------------------------------------

#[test]
fn numa_automatic_workers_on_the_nic_node_are_ok() {
    let host = FakeHost::bare_metal();
    let local = setup(vec![link(NIC1, NIC1)]);
    assert_level(&hints_json(&host, &local), "numa", "ok", "ok");
    let pair = setup(vec![link(NIC1, NIC0), link(NIC0, NIC1)]);
    assert_level(&hints_json(&host, &pair), "numa", "ok", "ok");
}

#[test]
fn numa_a_worker_spilled_to_the_other_node_warns() {
    let host = FakeHost::bare_metal(); // node 1 has two isolated cores (6, 7)
    let extra = "0000:b4:00.0";
    host.add_nic(&Nic::mlx5(extra, "enp180s0", Some(1)));
    // three links whose ingress NICs sit on node 1: the third spills to node 0
    let links = [NIC1, INTEL, extra].iter().map(|p| link(p, p)).collect();
    assert_level(&hints_json(&host, &setup(links)), "numa", "warn", "warn");
}

#[test]
fn numa_unknown_on_a_vm_is_not_a_problem() {
    let h = hints_json(
        &FakeHost::vm(),
        &setup(vec![link("0000:00:05.0", "0000:00:05.0")]),
    );
    assert_level(&h, "numa", "ok", "info");
}

// ---- governor ------------------------------------------------------------

#[test]
fn governor_performance_ok_powersave_not_ok() {
    let host = FakeHost::bare_metal();
    let st = one_link_nic0();
    assert_level(&hints_json(&host, &st), "governor", "ok", "ok");
    host.governor_all(8, "powersave");
    assert_level(&hints_json(&host, &st), "governor", "info", "warn");
}

#[test]
fn governor_missing_in_a_vm_is_ok_or_info() {
    let h = hints_json(&FakeHost::vm(), &setup(vec![]));
    assert_level(&h, "governor", "ok", "info");
}

// ---- irq affinity (see `host_commands.rs`) ----------

#[test]
fn the_irqbalance_hint_is_gone_a_running_irqbalance_is_no_hint_of_its_own() {
    let host = FakeHost::bare_metal();
    host.processes(&[(812, "irqbalance")]);
    for st in [setup(vec![]), one_link_nic0()] {
        let h = hints_json(&host, &st);
        assert!(!ids(&h).contains(&"irqbalance".to_string()), "{h}");
        let _ = hint(&h, "irq_affinity");
    }
}

// ---- IOMMU ---------------------------------------------------------------

#[test]
fn iommu_matters_only_when_a_vfio_nic_is_used() {
    let host = FakeHost::bare_metal();
    host.rebind(INTEL, "vfio-pci", true);
    let vfio = setup(vec![link(INTEL, INTEL)]);
    assert_level(&hints_json(&host, &vfio), "iommu", "ok", "ok");

    host.iommu(false);
    host.cmdline("ro quiet isolcpus=2-3,6-7 nohz_full=2-3,6-7 rcu_nocbs=2-3,6-7");
    assert_level(&hints_json(&host, &vfio), "iommu", "warn", "error");
    // an mlx5 (bifurcated) link does not need an IOMMU
    assert_level(&hints_json(&host, &one_link_nic0()), "iommu", "ok", "info");
}

// ---- MTU -----------------------------------------------------------------

#[test]
fn mtu_below_1500_on_a_used_nic_warns() {
    let host = FakeHost::bare_metal();
    assert_level(&hints_json(&host, &one_link_nic0()), "mtu", "ok", "ok");
    host.write("sys/class/net/ens1f0/mtu", "1400\n");
    assert_level(&hints_json(&host, &one_link_nic0()), "mtu", "warn", "warn");
    // not used by the link -> no complaint
    assert_level(
        &hints_json(&host, &setup(vec![link(NIC1, NIC1)])),
        "mtu",
        "ok",
        "ok",
    );
}

#[test]
fn mtu_above_1500_is_fine() {
    let host = FakeHost::bare_metal(); // NIC1 has 9000
    assert_level(
        &hints_json(&host, &setup(vec![link(NIC1, NIC1)])),
        "mtu",
        "ok",
        "ok",
    );
}

#[test]
fn mtu_of_an_egress_only_and_of_an_ingress_only_nic_counts() {
    let host = FakeHost::bare_metal();
    host.write("sys/class/net/ens1f0/mtu", "1400\n"); // NIC0
    for l in [link(NIC0, NIC1), link(NIC1, NIC0)] {
        assert_level(&hints_json(&host, &setup(vec![l])), "mtu", "warn", "warn");
    }
}

// ---- PCIe MaxReadReq -----------------------------------------------------

#[test]
fn maxreadreq_decoded_from_the_config_space_of_used_nics() {
    let host = FakeHost::bare_metal();
    let st = one_link_nic0();
    let id = nic_id("pcie_maxreadreq", NIC0);
    assert_level(&hints_json(&host, &st), &id, "ok", "ok"); // 4096

    host.write_bytes(
        &format!("sys/bus/pci/devices/{NIC0}/config"),
        &config_space(512),
    );
    assert_level(&hints_json(&host, &st), &id, "warn", "warn");

    for bytes in [1024, 2048] {
        host.write_bytes(
            &format!("sys/bus/pci/devices/{NIC0}/config"),
            &config_space(bytes),
        );
        assert_level(&hints_json(&host, &st), &id, "info", "info");
    }
}

#[test]
fn maxreadreq_only_looks_at_used_nics() {
    // INTEL has 512 in the fixture but the link uses NIC0 (4096)
    let h = hints_json(&FakeHost::bare_metal(), &one_link_nic0());
    assert_level(&h, &nic_id("pcie_maxreadreq", NIC0), "ok", "ok");
    assert!(!ids(&h).contains(&nic_id("pcie_maxreadreq", INTEL)), "{h}");
    let h = hints_json(&FakeHost::bare_metal(), &setup(vec![link(INTEL, INTEL)]));
    assert_level(&h, &nic_id("pcie_maxreadreq", INTEL), "warn", "warn");
}

#[test]
fn maxreadreq_looks_at_both_the_ingress_and_the_egress_nic() {
    // INTEL has 512 in the fixture; NIC0 has 4096
    for l in [link(INTEL, NIC0), link(NIC0, INTEL)] {
        let h = hints_json(&FakeHost::bare_metal(), &setup(vec![l]));
        assert_level(&h, &nic_id("pcie_maxreadreq", INTEL), "warn", "warn");
        assert_level(&h, &nic_id("pcie_maxreadreq", NIC0), "ok", "ok");
    }
}

#[test]
fn a_ring_through_three_nics_needs_three_isolated_cores() {
    let host = FakeHost::bare_metal(); // 4 isolated
    let ring = setup(vec![link(NIC0, NIC1), link(NIC1, INTEL), link(INTEL, NIC0)]);
    assert_level(&hints_json(&host, &ring), "isolated_cores", "ok", "ok");
    // with only 2 isolated cores the ring (three links) does not fit, a pair (two links) does
    host.cpus(8, Some("2-3"), &[(0, "0-3"), (1, "4-7")]);
    host.cmdline("ro isolcpus=2-3 nohz_full=2-3 rcu_nocbs=2-3");
    assert_level(
        &hints_json(&host, &ring),
        "isolated_cores",
        "error",
        "error",
    );
    let pair = setup(vec![link(NIC0, NIC1), link(NIC1, NIC0)]);
    assert_level(&hints_json(&host, &pair), "isolated_cores", "ok", "ok");
}

#[test]
fn unreadable_config_space_is_info_not_a_failure() {
    let host = FakeHost::bare_metal();
    host.remove(&format!("sys/bus/pci/devices/{NIC0}/config"));
    assert_level(
        &hints_json(&host, &one_link_nic0()),
        &nic_id("pcie_maxreadreq", NIC0),
        "info",
        "info",
    );
    // non-root reads see only the first 64 bytes
    host.write_bytes(
        &format!("sys/bus/pci/devices/{NIC0}/config"),
        &config_space(4096)[..64],
    );
    assert_level(
        &hints_json(&host, &one_link_nic0()),
        &nic_id("pcie_maxreadreq", NIC0),
        "info",
        "info",
    );
}

#[test]
fn vm_has_no_readable_pcie_state() {
    let h = hints_json(
        &FakeHost::vm(),
        &setup(vec![link("0000:00:05.0", "0000:00:05.0")]),
    );
    assert_level(
        &h,
        &nic_id("pcie_maxreadreq", "0000:00:05.0"),
        "info",
        "info",
    );
}

// ---- bare host -----------------------------------------------------------

#[test]
fn host_without_isolation_and_hugepages_reports_errors() {
    let h = hints_json(&FakeHost::bare(), &one_link_nic0());
    assert_level(&h, "isolation", "error", "error");
    assert_level(&h, "isolated_cores", "error", "error");
    assert_level(&h, "hugepages", "error", "error");
    assert_level(&h, "governor", "info", "warn");
}

// ---- GET is host-only, POST takes the configuration -----------------------------

const HOST_ONLY_IDS: [&str; 8] = [
    "isolation",
    "governor",
    "irq_affinity",
    "hugepages",
    "swappiness",
    "zone_reclaim_mode",
    "rcu_nocb_poll",
    "boot_hugepages",
];

fn ids(h: &Value) -> Vec<String> {
    h.as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn get_hints_are_the_host_only_hints_and_need_no_configuration() {
    for host in [FakeHost::bare_metal(), FakeHost::vm(), FakeHost::bare()] {
        let a = TestApp::new(host);
        let r = a.get("/api/host/hints").await;
        assert_eq!(r.status, 200, "{}", r.text());
        let h = r.json();
        for id in HOST_ONLY_IDS {
            let x = hint(&h, id);
            for k in ["id", "level", "title", "detail", "suggestion"] {
                assert!(x.get(k).is_some(), "hint {id} lacks `{k}`: {x}");
            }
        }
        // nothing that needs links: no hint about an uplink or a drain timeout
        for id in ["uplink", "drain_timeout"] {
            assert!(!ids(&h).contains(&id.to_string()), "{id} in GET: {h}");
        }
        assert_no_devargs_hint(&h);
    }
}

#[tokio::test]
async fn get_hints_do_not_change_when_an_emulation_runs() {
    // the controller keeps no configuration: GET answers the same before and during a run
    let a = TestApp::new(FakeHost::bare_metal());
    let before = a.get("/api/host/hints").await.json();
    a.start_ok(vec![link(NIC1, NIC1)]).await;
    assert_eq!(a.get("/api/host/hints").await.json(), before);
}

#[tokio::test]
async fn post_hints_add_the_link_dependent_ones_for_the_body() {
    let a = TestApp::new(FakeHost::bare_metal());
    let host_only = a.get("/api/host/hints").await.json();
    let body = setup(vec![link(INTEL, INTEL)]).body();
    let r = a.post("/api/host/hints", body).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let h = r.json();
    for id in HINT_IDS {
        let _ = hint(&h, id);
    }
    // the host-only ids are all there, once, and the NIC of the body is what the link-dependent
    // ones look at: INTEL has MaxReadReq 512
    for id in ids(&host_only) {
        let _ = hint(&h, &id);
    }
    assert_level(&h, &nic_id("pcie_maxreadreq", INTEL), "warn", "warn");
    // the same host, another body, another answer: the body is the only input
    let body = setup(vec![link(NIC0, NIC0)]).body();
    let h2 = a.post("/api/host/hints", body).await.json();
    assert_level(&h2, &nic_id("pcie_maxreadreq", NIC0), "ok", "ok");
    assert!(
        !ids(&h2).contains(&nic_id("pcie_maxreadreq", INTEL)),
        "{h2}"
    );
}

#[tokio::test]
async fn post_hints_with_no_links_equal_the_get_hints() {
    let a = TestApp::new(FakeHost::bare_metal());
    let get = a.get("/api/host/hints").await.json();
    let r = a.post("/api/host/hints", Setup::default().body()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), get);
}

#[tokio::test]
async fn post_hints_refuse_a_malformed_body_and_a_wrong_content_type() {
    let a = TestApp::new(FakeHost::bare_metal());
    let mut unknown = setup(vec![link(NIC0, NIC0)]).body();
    unknown["presets"] = serde_json::json!([]);
    for body in [
        serde_json::json!({}),
        serde_json::json!({"links": 5}),
        unknown,
    ] {
        let r = a.post("/api/host/hints", body.clone()).await;
        assert_eq!(r.status, 400, "{body}: {}", r.text());
        r.message();
    }
    let r = a
        .request_raw(
            "POST",
            "/api/host/hints",
            Some("text/plain"),
            setup(vec![link(NIC0, NIC0)])
                .body()
                .to_string()
                .into_bytes(),
        )
        .await;
    assert_eq!(r.status, 415, "{}", r.text());
    r.message();
}

#[tokio::test]
async fn post_hints_start_nothing_and_store_nothing() {
    let a = TestApp::new(FakeHost::bare_metal());
    let before = a.get("/api/emulation").await.json();
    let r = a
        .post("/api/host/hints", setup(vec![link(NIC0, NIC1)]).body())
        .await;
    assert_eq!(r.status, 200);
    assert!(a.supervisor.plans().is_empty());
    assert_eq!(a.get("/api/emulation").await.json(), before);
}

#[tokio::test]
async fn post_hints_know_the_uplink_and_the_drain_timeout_of_the_body() {
    let a = TestApp::new(FakeHost::bare_metal());
    let h = a
        .post("/api/host/hints", setup(vec![link(MGMT, NIC1)]).body())
        .await
        .json();
    let m: Vec<&Value> = h
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["id"] == "uplink")
        .collect();
    assert_eq!(m.len(), 1, "{h}");
    assert_eq!(m[0]["level"], "error");
    assert_eq!(m[0]["title"], "Host uplink");
    // an address alone is an uplink too, and the host-only hints never carry it
    let h = FakeHost::bare_metal();
    h.addr("ens1f0", "192.168.1.5");
    let a2 = TestApp::new(h);
    let h2 = a2
        .post("/api/host/hints", setup(vec![link(NIC0, NIC1)]).body())
        .await
        .json();
    assert_eq!(
        h2.as_array()
            .unwrap()
            .iter()
            .filter(|x| x["id"] == "uplink")
            .count(),
        1,
        "{h2}"
    );
    let mut slow = link("net_null0", "net_null0");
    slow["config"]["buffer_capacity"] = serde_json::json!(1000);
    slow["config"]["limiter"] = serde_json::json!({"mode": "Spacer", "rate_bps": 1.0e3});
    let h = a
        .post("/api/host/hints", setup(vec![slow]).body())
        .await
        .json();
    assert!(
        h.as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == "drain_timeout"),
        "{h}"
    );
}

// ---- virtual network ------------------------------------------------------

fn virtual_ids(h: &Value) -> Vec<String> {
    let mut ids = hint_ids(h);
    ids.sort();
    ids
}

#[test]
fn virtual_mode_has_the_virtual_network_and_cpus_hints_and_nothing_about_nics_or_hugepages() {
    let host = FakeHost::virtual_net(4, &["eth1", "eth2"]);
    for st in [setup(vec![]), setup(vec![link("eth1", "eth2")])] {
        let h = hints_json(&host, &st);
        assert_eq!(virtual_ids(&h), ["cpus", "virtual-network"], "{h}");
        assert_eq!(hint(&h, "virtual-network")["level"], "info");
        assert_no_commands(&h, "virtual-network");
        assert_level(&h, "cpus", "ok", "ok");
    }
    // no tool was asked anything (no firmware, no ethtool)
    assert!(host.exec().calls().is_empty(), "{:?}", host.exec().calls());
}

#[test]
fn the_cpus_hint_is_an_error_when_the_links_need_more_cores_than_are_online() {
    // a link needs a core of its own besides the main core
    let ports = ["eth1", "eth2", "eth3", "eth4"];
    let links = |n: usize| setup(ports[..n].iter().map(|p| link(p, p)).collect());
    let host = FakeHost::virtual_net(3, &ports);
    assert_level(&hints_json(&host, &links(2)), "cpus", "ok", "ok");
    assert_level(&hints_json(&host, &links(3)), "cpus", "error", "error");
    // alone, the main core is enough
    let host = FakeHost::virtual_net(1, &ports);
    assert_level(&hints_json(&host, &setup(vec![])), "cpus", "ok", "ok");
}

#[test]
fn physical_mode_has_neither_the_virtual_network_hint_nor_the_cpus_hint() {
    let h = hints_json(&FakeHost::bare_metal(), &one_link_nic0());
    let ids = hint_ids(&h);
    assert!(
        !ids.iter().any(|i| i == "virtual-network" || i == "cpus"),
        "{ids:?}"
    );
}
