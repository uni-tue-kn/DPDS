//! Interface discovery from a fake sysfs/procfs.

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
    assert_eq!(n["port"], NIC0, "a physical port is the PCI address");
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

// ---- virtual network: the interfaces are the kernel's netdevs ----------------------------------

fn by_port<'a>(v: &'a [Value], port: &str) -> &'a Value {
    v.iter()
        .find(|i| i["port"] == port)
        .unwrap_or_else(|| panic!("{port} not listed in {v:?}"))
}

#[test]
fn virtual_discovery_lists_every_netdev_but_lo_by_name() {
    let h = FakeHost::virtual_net(4, &["veth-a", "eth2", "eth1"]);
    h.write("sys/class/net/eth1/speed", "10000\n");
    h.write("sys/class/net/eth1/mtu", "9000\n");
    h.write("sys/class/net/veth-a/operstate", "down\n");
    h.netdev_driver("eth2", "virtio_net");
    // a PCI NIC of the sysfs is listed once, as the netdev it is, without its PCI data
    h.add_nic(&Nic::mlx5(NIC0, "ens1f0", Some(0)));
    let v = list(&h);
    let ports: Vec<&str> = v.iter().map(|i| i["port"].as_str().unwrap()).collect();
    assert_eq!(
        ports,
        ["ens1f0", "eth1", "eth2", "veth-a"],
        "sorted by name, no lo"
    );
    for i in &v {
        for k in [
            "pci",
            "vendor",
            "vendor_id",
            "device_id",
            "pcie_link",
            "numa_node",
        ] {
            assert!(absent(i, k), "{k} of {i}");
        }
    }
    let eth1 = by_port(&v, "eth1");
    assert_eq!(eth1["netdev"], "eth1");
    assert_eq!(eth1["mac"], h.read("sys/class/net/eth1/address").trim());
    assert_eq!(eth1["operstate"], "up");
    assert_eq!(eth1["speed_mbps"], 10000);
    assert_eq!(eth1["mtu"], 9000);
    assert!(absent(eth1, "driver"), "{eth1}");
    assert_eq!(eth1["uplink"], false);
    assert_eq!(eth1["dpds_usable"], true);
    assert!(absent(eth1, "reason"), "{eth1}");
    assert_eq!(by_port(&v, "eth2")["driver"], "virtio_net");
    assert_eq!(by_port(&v, "veth-a")["operstate"], "down");
}

#[test]
fn a_virtual_interface_with_a_route_an_address_or_a_bridge_is_an_uplink() {
    let h = FakeHost::virtual_net(4, &["eth0", "eth1", "eth2", "eth3", "eth4"]);
    h.routes(&[("eth0", "00000000", "0100A8C0", "00000000")]);
    h.addr("eth1", "192.168.1.5");
    h.master_link("eth2", "br0");
    h.addr("br0", "10.1.1.1");
    let v = list(&h);
    for used in ["eth0", "eth1", "eth2", "br0"] {
        let i = by_port(&v, used);
        assert_eq!(i["uplink"], true, "{i}");
        assert_eq!(i["dpds_usable"], false, "{i}");
        assert!(i["reason"].as_str().is_some_and(|r| !r.is_empty()), "{i}");
    }
    assert!(by_port(&v, "eth1")["reason"]
        .as_str()
        .unwrap()
        .contains("192.168.1.5"));
    for free in ["eth3", "eth4"] {
        let i = by_port(&v, free);
        assert_eq!(
            (i["uplink"].clone(), i["dpds_usable"].clone()),
            (false.into(), true.into()),
            "{i}"
        );
    }
}
