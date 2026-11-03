//! Read-only host hints (B6) on fake trees.

use crate::common::*;
use dpds_controller::model::StoredState;
use serde_json::Value;

const NEED_PER_MBUF: u64 = 1920; // README: ~1.9 KB per mbuf
const PAGE: u64 = 2 * 1024 * 1024;

/// One link with the default 100000 packet buffer: pool = capacity + 2*4096 + 768 + 64.
fn pool_bytes_for(links: u64) -> u64 {
    links * (100_000 + 4096 * 2 + 768 + 64) * NEED_PER_MBUF
}

fn one_link_nic0() -> StoredState {
    state_with(vec![link(1, "l", NIC0, NIC0, None)])
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
        "irqbalance",
        "iommu",
        "mtu",
        "pcie_maxreadreq",
    ] {
        assert_level(&h, id, "ok", "ok");
    }
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
        for state in [state_with(vec![]), one_link_nic0()] {
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
    let st = state_with(vec![]);
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
    let h = hints_json(&FakeHost::bare(), &state_with(vec![]));
    assert_level(&h, "isolation", "error", "error");
}

// ---- isolated core count -------------------------------------------------

fn vdev_links(n: u32) -> StoredState {
    state_with(
        (0..n)
            .map(|i| {
                let p = format!("net_null{i}");
                link(i + 1, &format!("l{i}"), &p, &p, None)
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
    assert_level(
        &hints_json(&host, &state_with(vec![])),
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
            link(i + 1, &format!("l{i}"), &port, &port, None)
        })
        .collect();
    hints_json(&host, &state_with(links))
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
    let h = hints_json(&FakeHost::bare(), &state_with(vec![]));
    assert_level(&h, "hugepages", "warn", "error");
}

// ---- NUMA ----------------------------------------------------------------

#[test]
fn numa_workers_on_the_nic_node_ok_elsewhere_warn() {
    let host = FakeHost::bare_metal();
    let local = state_with(vec![link(1, "l", NIC1, NIC1, Some(6))]);
    assert_level(&hints_json(&host, &local), "numa", "ok", "ok");
    let remote = state_with(vec![link(1, "l", NIC1, NIC1, Some(2))]);
    assert_level(&hints_json(&host, &remote), "numa", "warn", "warn");
}

#[test]
fn numa_a_worker_off_the_ingress_nic_node_warns_in_a_two_nic_link() {
    let host = FakeHost::bare_metal();
    // ingress NIC1 is on node 1 (cores 6-7): a worker on node 0 is remote
    let remote = state_with(vec![link(1, "a-to-b", NIC1, NIC0, Some(2))]);
    assert_level(&hints_json(&host, &remote), "numa", "warn", "warn");
    let local = state_with(vec![link(1, "a-to-b", NIC1, NIC0, Some(6))]);
    assert_level(&hints_json(&host, &local), "numa", "ok", "ok");
}

#[test]
fn numa_unknown_on_a_vm_is_not_a_problem() {
    let h = hints_json(
        &FakeHost::vm(),
        &state_with(vec![link(1, "l", "0000:00:05.0", "0000:00:05.0", None)]),
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
    let h = hints_json(&FakeHost::vm(), &state_with(vec![]));
    assert_level(&h, "governor", "ok", "info");
}

// ---- irqbalance ----------------------------------------------------------

#[test]
fn irqbalance_running_is_flagged() {
    let host = FakeHost::bare_metal();
    assert_level(
        &hints_json(&host, &state_with(vec![])),
        "irqbalance",
        "ok",
        "ok",
    );
    host.processes(&[(812, "irqbalance")]);
    assert_level(
        &hints_json(&host, &state_with(vec![])),
        "irqbalance",
        "info",
        "warn",
    );
}

// ---- IOMMU ---------------------------------------------------------------

#[test]
fn iommu_matters_only_when_a_vfio_nic_is_used() {
    let host = FakeHost::bare_metal();
    host.rebind(INTEL, "vfio-pci", true);
    let vfio = state_with(vec![link(1, "l", INTEL, INTEL, None)]);
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
        &hints_json(&host, &state_with(vec![link(1, "l", NIC1, NIC1, None)])),
        "mtu",
        "ok",
        "ok",
    );
}

#[test]
fn mtu_above_1500_is_fine() {
    let host = FakeHost::bare_metal(); // NIC1 has 9000
    assert_level(
        &hints_json(&host, &state_with(vec![link(1, "l", NIC1, NIC1, None)])),
        "mtu",
        "ok",
        "ok",
    );
}

#[test]
fn mtu_of_an_egress_only_and_of_an_ingress_only_nic_counts() {
    let host = FakeHost::bare_metal();
    host.write("sys/class/net/ens1f0/mtu", "1400\n"); // NIC0
    for l in [
        link(1, "ingress-only", NIC0, NIC1, None),
        link(1, "egress-only", NIC1, NIC0, None),
    ] {
        assert_level(
            &hints_json(&host, &state_with(vec![l])),
            "mtu",
            "warn",
            "warn",
        );
    }
}

// ---- PCIe MaxReadReq -----------------------------------------------------

#[test]
fn maxreadreq_decoded_from_the_config_space_of_used_nics() {
    let host = FakeHost::bare_metal();
    let st = one_link_nic0();
    assert_level(&hints_json(&host, &st), "pcie_maxreadreq", "ok", "ok"); // 4096

    host.write_bytes(
        &format!("sys/bus/pci/devices/{NIC0}/config"),
        &config_space(512),
    );
    assert_level(&hints_json(&host, &st), "pcie_maxreadreq", "warn", "warn");

    for bytes in [1024, 2048] {
        host.write_bytes(
            &format!("sys/bus/pci/devices/{NIC0}/config"),
            &config_space(bytes),
        );
        assert_level(&hints_json(&host, &st), "pcie_maxreadreq", "info", "info");
    }
}

#[test]
fn maxreadreq_only_looks_at_used_nics() {
    // INTEL has 512 in the fixture but the link uses NIC0 (4096)
    let h = hints_json(&FakeHost::bare_metal(), &one_link_nic0());
    assert_level(&h, "pcie_maxreadreq", "ok", "ok");
    let h = hints_json(
        &FakeHost::bare_metal(),
        &state_with(vec![link(1, "l", INTEL, INTEL, None)]),
    );
    assert_level(&h, "pcie_maxreadreq", "warn", "warn");
}

#[test]
fn maxreadreq_looks_at_both_the_ingress_and_the_egress_nic() {
    // INTEL has 512 in the fixture; NIC0 has 4096
    for l in [
        link(1, "in", INTEL, NIC0, None),
        link(1, "out", NIC0, INTEL, None),
    ] {
        let h = hints_json(&FakeHost::bare_metal(), &state_with(vec![l]));
        assert_level(&h, "pcie_maxreadreq", "warn", "warn");
    }
}

#[test]
fn a_ring_through_three_nics_needs_three_isolated_cores() {
    let host = FakeHost::bare_metal(); // 4 isolated
    let ring = state_with(vec![
        link(1, "a-to-b", NIC0, NIC1, None),
        link(2, "b-to-c", NIC1, INTEL, None),
        link(3, "c-to-a", INTEL, NIC0, None),
    ]);
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
    let pair = state_with(vec![
        link(1, "a-to-b", NIC0, NIC1, None),
        link(2, "b-to-a", NIC1, NIC0, None),
    ]);
    assert_level(&hints_json(&host, &pair), "isolated_cores", "ok", "ok");
}

#[test]
fn unreadable_config_space_is_info_not_a_failure() {
    let host = FakeHost::bare_metal();
    host.remove(&format!("sys/bus/pci/devices/{NIC0}/config"));
    assert_level(
        &hints_json(&host, &one_link_nic0()),
        "pcie_maxreadreq",
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
        "pcie_maxreadreq",
        "info",
        "info",
    );
}

#[test]
fn vm_has_no_readable_pcie_state() {
    let h = hints_json(
        &FakeHost::vm(),
        &state_with(vec![link(1, "l", "0000:00:05.0", "0000:00:05.0", None)]),
    );
    assert_level(&h, "pcie_maxreadreq", "info", "info");
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
