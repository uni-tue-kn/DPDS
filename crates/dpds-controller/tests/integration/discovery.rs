//! Interface discovery from a fake sysfs/procfs (B5).

use crate::common::*;
use dpds_controller::discovery::discover;
use serde_json::Value;

fn list(host: &FakeHost) -> Vec<Value> {
    serde_json::to_value(discover(&host.roots()))
        .unwrap()
        .as_array()
        .expect("discovery is a JSON array")
        .clone()
}

fn by_pci<'a>(v: &'a [Value], pci: &str) -> &'a Value {
    v.iter()
        .find(|i| i["pci"] == pci)
        .unwrap_or_else(|| panic!("{pci} not listed in {v:?}"))
}

#[test]
fn bare_metal_lists_only_network_devices_sorted_by_pci_address() {
    let v = list(&FakeHost::bare_metal());
    let pcis: Vec<&str> = v.iter().map(|i| i["pci"].as_str().unwrap()).collect();
    assert_eq!(
        pcis,
        [MGMT, NIC0, NIC1, INTEL],
        "NVMe (class 0x01) must not be listed"
    );
}

#[test]
fn mlx5_nic_fields() {
    let v = list(&FakeHost::bare_metal());
    let n = by_pci(&v, NIC0);
    assert_eq!(n["vendor_id"], "0x15b3");
    assert_eq!(n["device_id"], "0x1017");
    assert!(n["vendor"].as_str().unwrap().contains("Mellanox"), "{n}");
    assert_eq!(n["driver"], "mlx5_core");
    assert_eq!(n["netdev"], "ens1f0");
    assert_eq!(n["mac"], "aa:bb:cc:00:00:01");
    assert_eq!(n["operstate"], "up");
    assert_eq!(n["speed_mbps"], 100000);
    assert_eq!(n["mtu"], 1500);
    assert_eq!(n["numa_node"], 0);
    assert_eq!(n["pcie_link"]["speed"], "16.0 GT/s PCIe");
    assert_eq!(n["pcie_link"]["width"], 16);
    assert_eq!(n["pcie_link"]["max_speed"], "16.0 GT/s PCIe");
    assert_eq!(n["pcie_link"]["max_width"], 16);
    assert_eq!(n["uplink"], false);
    assert!(n.get("kernel_route").is_none(), "{n}");
    assert_eq!(n["dpds_usable"], true);
}

#[test]
fn numa_and_mtu_of_the_second_mlx5_nic() {
    let v = list(&FakeHost::bare_metal());
    let n = by_pci(&v, NIC1);
    assert_eq!(n["numa_node"], 1);
    assert_eq!(n["mtu"], 9000);
}

#[test]
fn vendor_names_and_hex_fallback() {
    let v = list(&FakeHost::bare_metal());
    assert!(by_pci(&v, INTEL)["vendor"]
        .as_str()
        .unwrap()
        .contains("Intel"));
    let unknown = by_pci(&v, MGMT)["vendor"]
        .as_str()
        .unwrap()
        .to_ascii_lowercase();
    assert!(
        unknown.contains("beef"),
        "unknown vendor must fall back to hex: {unknown}"
    );
}

#[test]
fn kernel_driver_nics_are_not_usable_and_say_bind_to_vfio() {
    let v = list(&FakeHost::bare_metal());
    // An uplink NIC explains that instead (binding it to vfio-pci would be wrong advice).
    for (pci, needle) in [(INTEL, "vfio-pci"), (MGMT, "uplink")] {
        let n = by_pci(&v, pci);
        assert_eq!(n["dpds_usable"], false, "{pci}");
        let reason = n["reason"].as_str().unwrap_or_default();
        assert!(reason.contains(needle), "{pci}: reason {reason:?}");
    }
    assert_eq!(by_pci(&v, INTEL)["driver"], "ice");
}

#[test]
fn the_nic_with_the_default_route_is_an_uplink_others_are_not() {
    let v = list(&FakeHost::bare_metal());
    assert_eq!(by_pci(&v, MGMT)["uplink"], true);
    for pci in [NIC0, NIC1, INTEL] {
        assert_eq!(by_pci(&v, pci)["uplink"], false, "{pci}");
    }
}

#[test]
fn any_route_of_the_netdev_counts_not_only_the_default_route() {
    let h = FakeHost::bare_metal();
    h.routes(&[("ens1f0", "0000A8C0", "00000000", "00FFFFFF")]);
    let v = list(&h);
    assert_eq!(by_pci(&v, NIC0)["uplink"], true);
    assert_eq!(by_pci(&v, MGMT)["uplink"], false);
}

#[test]
fn an_address_without_any_route_makes_an_uplink() {
    let h = FakeHost::bare_metal();
    h.addr("ens1f0", "192.168.1.5");
    let v = list(&h);
    let n = by_pci(&v, NIC0);
    assert_eq!(n["uplink"], true, "{n}");
    assert_eq!(n["dpds_usable"], false, "{n}");
    assert!(n["reason"].as_str().unwrap().contains("192.168.1.5"), "{n}");
    assert_eq!(by_pci(&v, NIC1)["uplink"], false);
}

#[test]
fn a_link_local_ipv6_address_alone_does_not_make_an_uplink() {
    let h = FakeHost::bare_metal();
    h.addr("ens1f0", "fe80::1");
    assert_eq!(by_pci(&list(&h), NIC0)["uplink"], false);
}

#[test]
fn an_open_vswitch_port_makes_an_uplink() {
    let h = FakeHost::bare_metal();
    h.master_link("ens2f0", "ovs-system");
    let v = list(&h);
    let n = by_pci(&v, NIC1);
    assert_eq!(n["uplink"], true, "{n}");
    assert!(
        n["reason"].as_str().unwrap().contains("Open vSwitch"),
        "{n}"
    );
}

#[test]
fn vfio_and_uio_bound_nics_are_usable_and_have_no_netdev() {
    for driver in ["vfio-pci", "uio_pci_generic"] {
        let h = FakeHost::bare_metal();
        h.rebind(INTEL, driver, true);
        let v = list(&h);
        let n = by_pci(&v, INTEL);
        assert_eq!(n["driver"], driver);
        assert_eq!(n["dpds_usable"], true, "{driver}");
        assert!(absent(n, "netdev"), "{n}");
        assert!(absent(n, "mac"), "{n}");
    }
}

#[test]
fn unbound_device_has_no_driver_and_is_not_usable() {
    let h = FakeHost::bare_metal();
    h.remove(&format!("sys/bus/pci/devices/{INTEL}/driver"));
    let v = list(&h);
    let n = by_pci(&v, INTEL);
    assert!(absent(n, "driver"));
    assert_eq!(n["dpds_usable"], false);
}

#[test]
fn vm_host_reports_missing_data_as_absent() {
    let v = list(&FakeHost::vm());
    assert_eq!(v.len(), 2);
    for n in &v {
        assert!(absent(n, "numa_node"), "{n}");
        let pcie = &n["pcie_link"];
        assert!(
            pcie.is_null()
                || ["speed", "width", "max_speed", "max_width"]
                    .iter()
                    .all(|k| absent(pcie, k)),
            "{n}"
        );
    }
    let virtio = by_pci(&v, "0000:00:03.0");
    assert_eq!(virtio["driver"], "virtio-pci");
    assert!(absent(virtio, "speed_mbps"));
    assert_eq!(virtio["uplink"], true);
    assert_eq!(virtio["dpds_usable"], false);
    let vf = by_pci(&v, "0000:00:05.0");
    assert_eq!(vf["dpds_usable"], true);
    assert_eq!(vf["speed_mbps"], 100000);
}

#[test]
fn numa_minus_one_and_speed_minus_one_mean_unknown() {
    let h = FakeHost::bare_metal();
    h.write(&format!("sys/bus/pci/devices/{NIC0}/numa_node"), "-1\n");
    h.write("sys/class/net/ens1f0/speed", "-1\n");
    let v = list(&h);
    let n = by_pci(&v, NIC0);
    assert!(absent(n, "numa_node"), "{n}");
    assert!(absent(n, "speed_mbps"), "{n}");
}

#[test]
fn empty_sysfs_yields_an_empty_list() {
    assert!(list(&FakeHost::empty()).is_empty());
}

#[test]
fn down_interface_keeps_its_operstate() {
    let h = FakeHost::bare_metal();
    h.write("sys/class/net/ens1f0/operstate", "down\n");
    let v = list(&h);
    assert_eq!(by_pci(&v, NIC0)["operstate"], "down");
}
