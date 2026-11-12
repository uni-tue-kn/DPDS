//! The host-uplink guard. A NIC is an **uplink** (the host uses it) when the netdev, or any device
//! above it (sysfs `upper_*` and `master` links, also nested), has
//!
//! 1. a route in `/proc/net/route` or `/proc/net/ipv6_route`, or
//! 2. an IP address from `getifaddrs` (any IPv4, or a non-link-local IPv6; `lo` never counts; the
//!    tests inject the addresses with `IfAddrs::Fixed` through `FakeHost::addr`), or
//! 3. is enslaved to Open vSwitch (a `master` link named `ovs-system`).
//!
//! Effects pinned here:
//!
//! - discovery: `uplink: true`, `dpds_usable: false` and the exact `reason` of the first signal
//!   that applies (route, then address, then OVS), naming the upper device when the signal is on one;
//! - hints: an `error` hint with id `uplink`, title "Host uplink", naming the NIC, when a link of
//!   the body of `POST /api/host/hints` uses such a port (still a 200, not a 409; one hint names
//!   all uplink NICs);
//! - `POST /api/emulation/start` and `/validate` answer 409 `{"message"}` naming the link, the
//!   port and the reason, and spawn nothing;
//! - the binary takes the addresses from the test-only `--ifaddrs-file` /
//!   `DPDS_CONTROLLER_IFADDRS_FILE`. `check` has no links, so it cannot name an uplink.
//!
//! IPv6 routes: link-local-only interfaces are not "routed". The kernel lists `fe80::/64`, the
//! link-local address itself (`/128`) and the multicast route `ff00::/8` on every IPv6 interface,
//! and `lo` has `::1/128`; none of these counts (the tests use that realistic set).

use crate::common::*;
use dpds_controller::discovery::discover;
use dpds_controller::host::{HostRoots, IfAddrs};
use serde_json::Value;

const DEFAULT6: &str = "00000000000000000000000000000000";
const GLOBAL6: &str = "20010db8000000000000000000000000";
const LINK_LOCAL6: &str = "fe800000000000000000000000000000";
const LINK_LOCAL_ADDR6: &str = "fe80000000000000a8bbccfffe000001";
const MULTICAST6: &str = "ff000000000000000000000000000000";
const LOOPBACK6: &str = "00000000000000000000000000000001";

fn list(host: &FakeHost) -> Vec<Value> {
    serde_json::to_value(discover(&host.roots()))
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn nic(v: &[Value], pci: &str) -> Value {
    v.iter().find(|i| i["pci"] == pci).unwrap().clone()
}

/// A bare-metal host whose IPv4 table has no route at all (so only what a test adds counts).
fn host() -> FakeHost {
    let h = FakeHost::bare_metal();
    h.routes(&[]);
    h
}

const TAIL: &str = "DPDK would take it away from the host";

fn route_reason(netdev: &str) -> String {
    format!("{netdev} carries a kernel route (host uplink); {TAIL}")
}

fn route_above(holder: &str, netdev: &str) -> String {
    format!(
        "{holder} above {netdev} carries a kernel route (host uplink); DPDK would take {netdev} away from the host"
    )
}

fn addr_reason(netdev: &str, addr: &str) -> String {
    format!("{netdev} has the IP address {addr} (host uplink); {TAIL}")
}

fn addr_above(holder: &str, netdev: &str, addr: &str) -> String {
    format!(
        "{holder} above {netdev} has the IP address {addr} (host uplink); DPDK would take {netdev} away from the host"
    )
}

fn ovs_reason(netdev: &str) -> String {
    format!("{netdev} is an Open vSwitch port (host uplink); {TAIL}")
}

/// An uplink: `uplink: true`, not usable, and exactly this reason.
fn assert_uplink(n: &Value, reason: &str) {
    assert_eq!(n["uplink"], true, "{n}");
    assert_eq!(n["dpds_usable"], false, "{n}");
    assert_eq!(n["reason"], reason, "{n}");
}

fn assert_free(n: &Value) {
    assert_eq!(n["uplink"], false, "{n}");
    assert_eq!(n["dpds_usable"], true, "{n}");
}

// ---- discovery -------------------------------------------------------------

#[test]
fn an_ipv4_route_on_the_nic_itself_guards_it_even_with_an_mlx5_driver() {
    let h = host();
    h.routes(&[("ens1f0", "00000000", "0100A8C0", "00000000")]);
    let v = list(&h);
    assert_uplink(&nic(&v, NIC0), &route_reason("ens1f0"));
    assert_free(&nic(&v, NIC1));
}

#[test]
fn an_ipv6_only_default_route_guards_the_nic() {
    let h = host();
    h.routes6(&[("ens1f0", DEFAULT6, 0)]);
    assert_uplink(&nic(&list(&h), NIC0), &route_reason("ens1f0"));
}

#[test]
fn an_ipv6_prefix_route_on_the_nic_guards_it() {
    let h = host();
    h.routes6(&[("ens2f0", GLOBAL6, 64)]);
    let v = list(&h);
    assert_uplink(&nic(&v, NIC1), &route_reason("ens2f0"));
    assert_free(&nic(&v, NIC0));
}

#[test]
fn a_link_local_only_ipv6_interface_is_not_routed() {
    let h = host();
    h.routes6(&[("ens1f0", LINK_LOCAL6, 64)]);
    assert_free(&nic(&list(&h), NIC0));
}

#[test]
fn the_realistic_link_local_and_loopback_set_is_not_routed() {
    let h = host();
    h.routes6(&[
        ("lo", LOOPBACK6, 128),
        ("ens1f0", LINK_LOCAL6, 64),
        ("ens1f0", LINK_LOCAL_ADDR6, 128),
        ("ens1f0", MULTICAST6, 8),
        ("ens2f0", LINK_LOCAL6, 64),
        ("ens2f0", MULTICAST6, 8),
    ]);
    let v = list(&h);
    assert_free(&nic(&v, NIC0));
    assert_free(&nic(&v, NIC1));
}

#[test]
fn a_global_route_next_to_link_local_ones_still_counts() {
    let h = host();
    h.routes6(&[
        ("ens1f0", LINK_LOCAL6, 64),
        ("ens1f0", MULTICAST6, 8),
        ("ens1f0", GLOBAL6, 64),
    ]);
    assert_uplink(&nic(&list(&h), NIC0), &route_reason("ens1f0"));
}

#[test]
fn a_route_on_another_device_does_not_guard_the_nic() {
    let h = host();
    h.virtual_netdev("bond7"); // exists, carries a route, but is not above any NIC
    h.routes(&[
        ("eth9", "00000000", "0100A8C0", "00000000"),
        ("bond7", "00000000", "0100A8C0", "00000000"),
    ]);
    h.routes6(&[("eth9", DEFAULT6, 0)]);
    let v = list(&h);
    for pci in [NIC0, NIC1] {
        assert_free(&nic(&v, pci));
    }
}

#[test]
fn a_route_on_a_bond_above_the_nic_guards_the_nic_via_upper_and_names_the_bond() {
    let h = host();
    h.upper_link("ens1f0", "bond0");
    h.routes(&[("bond0", "00000000", "0100A8C0", "00000000")]);
    let v = list(&h);
    assert_uplink(&nic(&v, NIC0), &route_above("bond0", "ens1f0"));
    assert_free(&nic(&v, NIC1));
}

#[test]
fn a_route_on_a_bond_above_the_nic_guards_it_via_the_master_link_alone() {
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.routes6(&[("bond0", DEFAULT6, 0)]);
    assert_uplink(&nic(&list(&h), NIC0), &route_above("bond0", "ens1f0"));
}

#[test]
fn a_route_on_a_bridge_master_guards_the_nic() {
    let h = host();
    h.master_link("ens2f0", "br0");
    h.routes(&[("br0", "0000A8C0", "00000000", "00FFFFFF")]);
    let v = list(&h);
    assert_uplink(&nic(&v, NIC1), &route_above("br0", "ens2f0"));
    assert_free(&nic(&v, NIC0));
}

#[test]
fn a_route_on_a_vlan_above_the_nic_guards_it() {
    let h = host();
    h.upper_link("ens1f0", "ens1f0.100");
    h.routes6(&[("ens1f0.100", GLOBAL6, 64)]);
    assert_uplink(&nic(&list(&h), NIC0), &route_above("ens1f0.100", "ens1f0"));
}

#[test]
fn a_route_on_a_vlan_above_a_bond_above_the_nic_guards_it() {
    // ens1f0 -> bond0 -> bond0.200 (the usual management setup)
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.upper_link("bond0", "bond0.200");
    h.routes(&[("bond0.200", "00000000", "0100A8C0", "00000000")]);
    assert_uplink(&nic(&list(&h), NIC0), &route_above("bond0.200", "ens1f0"));
}

#[test]
fn an_upper_device_without_a_route_does_not_guard_the_nic() {
    let h = host();
    h.upper_link("ens1f0", "bond0");
    h.routes(&[("eno1", "00000000", "0100A8C0", "00000000")]);
    assert_free(&nic(&list(&h), NIC0));
}

// ---- discovery: the address signal -----------------------------------------

#[test]
fn an_ipv4_address_on_the_nic_guards_it_even_without_any_route() {
    let h = host();
    h.addr("ens1f0", "192.168.1.5");
    let v = list(&h);
    assert_uplink(&nic(&v, NIC0), &addr_reason("ens1f0", "192.168.1.5"));
    assert_free(&nic(&v, NIC1));
}

#[test]
fn a_global_ipv6_address_on_the_nic_guards_it() {
    let h = host();
    h.addr("ens2f0", "2001:db8::5");
    let v = list(&h);
    assert_uplink(&nic(&v, NIC1), &addr_reason("ens2f0", "2001:db8::5"));
    assert_free(&nic(&v, NIC0));
}

#[test]
fn an_ipv6_link_local_address_alone_is_not_an_uplink() {
    // fe80::/10 reaches febf:ffff:...; every IPv6 interface has such an address
    let h = host();
    h.addr("ens1f0", "fe80::a8bb:ccff:fe00:1");
    h.addr("ens2f0", "febf::1");
    let v = list(&h);
    assert_free(&nic(&v, NIC0));
    assert_free(&nic(&v, NIC1));
}

#[test]
fn an_ipv6_address_just_outside_fe80_10_counts() {
    // fec0::/10 (site-local) is not link-local
    let h = host();
    h.addr("ens1f0", "fec0::1");
    assert_uplink(&nic(&list(&h), NIC0), &addr_reason("ens1f0", "fec0::1"));
}

#[test]
fn an_ipv4_link_local_address_is_an_uplink() {
    // 169.254/16 is what a failed DHCP leaves behind, and the host still uses the port
    let h = host();
    h.addr("ens1f0", "169.254.12.7");
    assert_uplink(
        &nic(&list(&h), NIC0),
        &addr_reason("ens1f0", "169.254.12.7"),
    );
}

#[test]
fn the_reason_names_the_first_usable_address_in_getifaddrs_order() {
    let h = host();
    h.addr("ens1f0", "fe80::1"); // link-local: skipped, not named
    h.addr("ens1f0", "10.0.0.1");
    h.addr("ens1f0", "10.0.0.2");
    h.addr("ens1f0", "2001:db8::1");
    assert_uplink(&nic(&list(&h), NIC0), &addr_reason("ens1f0", "10.0.0.1"));
    let h = host();
    h.addr("ens1f0", "fe80::1");
    h.addr("ens1f0", "2001:db8::7");
    assert_uplink(&nic(&list(&h), NIC0), &addr_reason("ens1f0", "2001:db8::7"));
}

#[test]
fn a_static_address_on_a_down_interface_still_guards_it() {
    let h = host();
    h.write("sys/class/net/ens1f0/operstate", "down\n");
    h.addr("ens1f0", "192.168.1.5");
    let n = nic(&list(&h), NIC0);
    assert_eq!(n["operstate"], "down");
    assert_uplink(&n, &addr_reason("ens1f0", "192.168.1.5"));
}

#[test]
fn an_address_on_another_device_does_not_guard_the_nic() {
    let h = host();
    h.virtual_netdev("bond7"); // exists, has an address, is not above any NIC
    h.addr("eth9", "192.168.1.5");
    h.addr("bond7", "10.1.1.1");
    h.addr("ens1f0x", "10.1.1.2"); // a name that merely starts like a NIC
    h.addr("lo", "127.0.0.1");
    h.addr("lo", "::1");
    let v = list(&h);
    assert_free(&nic(&v, NIC0));
    assert_free(&nic(&v, NIC1));
}

#[test]
fn a_loopback_device_above_the_nic_never_counts() {
    let h = host();
    h.master_link("ens1f0", "lo");
    h.addr("lo", "127.0.0.1");
    h.addr("lo", "::1");
    assert_free(&nic(&list(&h), NIC0));
}

#[test]
fn an_address_on_a_bond_above_the_nic_names_the_bond() {
    let h = host();
    h.upper_link("ens1f0", "bond0");
    h.addr("bond0", "192.168.1.5");
    let v = list(&h);
    assert_uplink(
        &nic(&v, NIC0),
        &addr_above("bond0", "ens1f0", "192.168.1.5"),
    );
    assert_free(&nic(&v, NIC1));
}

#[test]
fn an_address_on_a_master_bond_or_bridge_names_the_master() {
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.master_link("ens2f0", "br0");
    h.addr("bond0", "2001:db8::9");
    h.addr("br0", "10.9.9.9");
    let v = list(&h);
    assert_uplink(
        &nic(&v, NIC0),
        &addr_above("bond0", "ens1f0", "2001:db8::9"),
    );
    assert_uplink(&nic(&v, NIC1), &addr_above("br0", "ens2f0", "10.9.9.9"));
}

#[test]
fn an_address_on_a_vlan_above_a_bond_above_the_nic_names_the_vlan() {
    // ens1f0 -> bond0 -> bond0.200
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.upper_link("bond0", "bond0.200");
    h.addr("bond0.200", "192.168.200.4");
    assert_uplink(
        &nic(&list(&h), NIC0),
        &addr_above("bond0.200", "ens1f0", "192.168.200.4"),
    );
}

#[test]
fn a_link_local_only_bond_above_the_nic_is_not_an_uplink() {
    let h = host();
    h.upper_link("ens1f0", "bond0");
    h.addr("bond0", "fe80::1");
    assert_free(&nic(&list(&h), NIC0));
}

#[test]
fn a_kernel_driver_nic_with_an_address_explains_the_uplink_not_the_driver() {
    let h = host();
    h.addr("enp179s0", "192.168.7.7");
    let n = nic(&list(&h), INTEL);
    assert_eq!(n["driver"], "ice");
    assert_uplink(&n, &addr_reason("enp179s0", "192.168.7.7"));
}

// ---- discovery: the Open vSwitch signal ------------------------------------

#[test]
fn a_port_enslaved_to_open_vswitch_is_an_uplink_without_route_or_address() {
    let h = host();
    h.master_link("ens1f0", "ovs-system"); // the bridge IP lives on an OVS internal port
    let v = list(&h);
    assert_uplink(&nic(&v, NIC0), &ovs_reason("ens1f0"));
    assert_free(&nic(&v, NIC1));
}

#[test]
fn another_master_without_route_or_address_is_not_an_uplink() {
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.master_link("ens2f0", "ovs-system-not");
    let v = list(&h);
    assert_free(&nic(&v, NIC0));
    assert_free(&nic(&v, NIC1));
}

#[test]
fn a_bond_enslaved_to_open_vswitch_makes_its_slave_an_uplink_too() {
    // the same "itself or any device above it" walk; the wording of the reason is not pinned
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.master_link("bond0", "ovs-system");
    let n = nic(&list(&h), NIC0);
    assert_eq!(n["uplink"], true, "{n}");
    assert_eq!(n["dpds_usable"], false, "{n}");
    assert!(
        n["reason"].as_str().unwrap().contains("Open vSwitch"),
        "{n}"
    );
}

// ---- discovery: precedence, no signal, field shape -------------------------

#[test]
fn route_beats_address_beats_open_vswitch() {
    let h = host();
    h.routes(&[("ens1f0", "00000000", "0100A8C0", "00000000")]);
    h.addr("ens1f0", "192.168.1.5");
    h.master_link("ens1f0", "ovs-system");
    h.addr("ens2f0", "192.168.1.6");
    h.master_link("ens2f0", "ovs-system");
    let v = list(&h);
    assert_uplink(&nic(&v, NIC0), &route_reason("ens1f0"));
    assert_uplink(&nic(&v, NIC1), &addr_reason("ens2f0", "192.168.1.6"));
}

#[test]
fn route_beats_address_even_when_the_route_is_on_a_device_above() {
    let h = host();
    h.upper_link("ens1f0", "bond0");
    h.addr("ens1f0", "192.168.1.5");
    h.routes6(&[("bond0", DEFAULT6, 0)]);
    assert_uplink(&nic(&list(&h), NIC0), &route_above("bond0", "ens1f0"));
}

#[test]
fn a_host_without_any_signal_keeps_every_driver_case_as_before() {
    let h = host();
    let v = list(&h);
    assert_free(&nic(&v, NIC0));
    assert_free(&nic(&v, NIC1));
    let tg3 = nic(&v, MGMT);
    assert_eq!(tg3["uplink"], false, "{tg3}");
    assert_eq!(tg3["dpds_usable"], false, "{tg3}");
    assert!(
        tg3["reason"].as_str().unwrap().contains("vfio-pci"),
        "{tg3}"
    );
    h.rebind(INTEL, "vfio-pci", true);
    let n = nic(&list(&h), INTEL);
    assert_eq!(n["uplink"], false, "{n}");
    assert_eq!(n["dpds_usable"], true, "{n}");
}

#[test]
fn every_interface_has_a_boolean_uplink_and_no_kernel_route_field() {
    let h = FakeHost::bare_metal();
    h.addr("ens1f0", "192.168.1.5");
    for n in list(&h) {
        assert!(n["uplink"].is_boolean(), "{n}");
        assert!(n.get("kernel_route").is_none(), "{n}");
    }
}

#[test]
fn the_system_address_source_is_the_default_of_the_real_host() {
    assert!(matches!(HostRoots::system().ifaddrs, IfAddrs::System));
}

#[test]
fn the_system_source_never_counts_the_loopback_device() {
    // real getifaddrs: `lo` always has 127.0.0.1 and usually ::1
    let h = host();
    h.master_link("ens1f0", "lo");
    let roots = HostRoots {
        ifaddrs: IfAddrs::System,
        ..h.roots()
    };
    let v = serde_json::to_value(discover(&roots)).unwrap();
    let n = nic(v.as_array().unwrap(), NIC0);
    assert_free(&n);
}

/// A non-loopback interface of the machine running the test that has an address, if any.
fn real_interface_with_an_address() -> Option<String> {
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `ifap` with a list that freeifaddrs releases below; every
    // pointer read from the list is checked for null and the list is not used after the free.
    unsafe {
        if libc::getifaddrs(&mut ifap) != 0 {
            return None;
        }
        let mut found = None;
        let mut cur = ifap;
        while !cur.is_null() && found.is_none() {
            let a = &*cur;
            cur = a.ifa_next;
            if a.ifa_addr.is_null() || a.ifa_flags & (libc::IFF_LOOPBACK as u32) != 0 {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(a.ifa_name)
                .to_string_lossy()
                .into_owned();
            if i32::from((*a.ifa_addr).sa_family) == libc::AF_INET {
                found = Some(name);
            }
        }
        libc::freeifaddrs(ifap);
        found
    }
}

#[test]
fn the_system_source_reads_the_real_getifaddrs() {
    // Needs an IPv4 address on a non-loopback interface of the test machine (CI runners and
    // hosts have one; a container with only `lo` has none, and the test says so and stops).
    let Some(name) = real_interface_with_an_address() else {
        eprintln!("no non-loopback IPv4 interface on this machine: real getifaddrs not exercised");
        return;
    };
    let h = FakeHost::empty();
    h.add_nic(&Nic::mlx5(NIC0, &name, Some(0)));
    let roots = HostRoots {
        ifaddrs: IfAddrs::System,
        ..h.roots()
    };
    let v = serde_json::to_value(discover(&roots)).unwrap();
    let n = nic(v.as_array().unwrap(), NIC0);
    assert_eq!(n["uplink"], true, "{n}");
    assert!(n["reason"].as_str().unwrap().contains("IP address"), "{n}");
}

// ---- hints -----------------------------------------------------------------

const UPLINK_FIX: &str = "Use NICs the host does not use (no IP address, no route, not in Open vSwitch); binding the host's uplink to DPDK cuts the host off the network. Start is refused.";

fn state_using(ingress: &str, egress: &str) -> Setup {
    setup(vec![link(ingress, egress)])
}

fn uplink_hints(host: &FakeHost, ingress: &str, egress: &str) -> Vec<Value> {
    hints_json(host, &state_using(ingress, egress))
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["id"] == "uplink")
        .cloned()
        .collect()
}

fn assert_one_error_naming(found: &[Value], pcis: &[&str]) {
    assert_eq!(found.len(), 1, "{found:?}");
    let h = &found[0];
    assert_eq!(h["level"], "error", "{h}");
    assert_eq!(h["title"], "Host uplink", "{h}");
    assert_eq!(h["suggestion"], UPLINK_FIX, "{h}");
    let text = format!("{} {}", h["title"], h["detail"]);
    for pci in pcis {
        assert!(text.contains(pci), "{pci} is named: {text}");
    }
}

#[test]
fn a_link_on_the_routed_nic_gives_one_error_hint_naming_it() {
    let h = FakeHost::bare_metal();
    assert_one_error_naming(&uplink_hints(&h, MGMT, NIC1), &[MGMT]);
}

#[test]
fn the_uplink_is_guarded_as_egress_and_as_a_hairpin_too() {
    let h = FakeHost::bare_metal();
    for (i, e) in [(NIC1, MGMT), (MGMT, MGMT)] {
        assert_one_error_naming(&uplink_hints(&h, i, e), &[MGMT]);
    }
}

#[test]
fn an_address_only_uplink_gives_the_hint_too() {
    let h = host();
    h.addr("ens1f0", "192.168.1.5");
    assert_one_error_naming(&uplink_hints(&h, NIC0, NIC1), &[NIC0]);
    assert_one_error_naming(&uplink_hints(&h, NIC1, NIC0), &[NIC0]);
}

#[test]
fn an_open_vswitch_port_gives_the_hint_too() {
    let h = host();
    h.master_link("ens2f0", "ovs-system");
    assert_one_error_naming(&uplink_hints(&h, NIC0, NIC1), &[NIC1]);
}

#[test]
fn the_hint_covers_bond_and_ipv6_routes_too() {
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.routes6(&[("bond0", DEFAULT6, 0)]);
    assert_one_error_naming(&uplink_hints(&h, NIC0, NIC1), &[NIC0]);
}

#[test]
fn several_uplink_nics_still_give_one_hint_naming_all_of_them() {
    let h = host();
    h.addr("ens1f0", "192.168.1.5");
    h.master_link("ens2f0", "ovs-system");
    assert_one_error_naming(&uplink_hints(&h, NIC0, NIC1), &[NIC0, NIC1]);
}

#[test]
fn no_error_hint_when_no_link_uses_an_uplink() {
    let h = FakeHost::bare_metal(); // MGMT is routed, but unused
    h.addr("eno1", "192.168.0.10");
    assert!(uplink_hints(&h, NIC0, NIC1).is_empty());
    assert!(uplink_hints(&h, "net_null0", "net_null0").is_empty());
    // link-local IPv6 only: not an uplink, no hint
    let h = host();
    h.addr("ens1f0", "fe80::1");
    assert!(uplink_hints(&h, NIC0, NIC1).is_empty());
    let all = hints_json(&h, &Default::default());
    assert!(all.as_array().unwrap().iter().all(|x| x["id"] != "uplink"));
}

#[test]
fn the_old_hint_id_is_gone() {
    let h = FakeHost::bare_metal();
    let all = hints_json(&h, &state_using(MGMT, NIC1));
    assert!(
        all.as_array()
            .unwrap()
            .iter()
            .all(|x| x["id"] != "management_nic"),
        "{all}"
    );
}

// ---- start, validate, the interfaces endpoint ------------------------------

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

/// A host where NIC0 (`ens1f0`) has an address, so it is an uplink without a route.
fn address_uplink_app() -> TestApp {
    let h = host();
    h.addr("ens1f0", "192.168.1.5");
    TestApp::new(h)
}

/// A host where NIC0 (`ens1f0`) is an Open vSwitch port.
fn ovs_uplink_app() -> TestApp {
    let h = host();
    h.master_link("ens1f0", "ovs-system");
    TestApp::new(h)
}

/// The 409 of start for a link `ingress` -> `egress`: names the link by its ports, the port and
/// the reason, and nothing was spawned.
async fn assert_start_refused(a: &TestApp, ingress: &str, egress: &str, port: &str, reason: &str) {
    let r = a.start_links(vec![link(ingress, egress)]).await;
    assert_eq!(r.status, 409, "{ingress} -> {egress}: {}", r.text());
    let m = r.message();
    assert!(
        m.contains(&format!("link {ingress} -> {egress}")),
        "the link is named by its ports: {m}"
    );
    assert!(m.contains(port), "the port is named: {m}");
    assert!(m.contains(reason), "the reason is given: {m}");
    assert!(a.supervisor.plans().is_empty(), "nothing is spawned");
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

#[tokio::test]
async fn start_with_a_routed_ingress_is_a_409_naming_link_port_and_reason() {
    let a = app();
    assert_start_refused(&a, MGMT, NIC1, MGMT, &route_reason("eno1")).await;
}

#[tokio::test]
async fn start_with_a_routed_egress_is_a_409_naming_link_port_and_reason() {
    let a = app();
    assert_start_refused(&a, NIC1, MGMT, MGMT, &route_reason("eno1")).await;
}

#[tokio::test]
async fn start_with_an_address_uplink_is_a_409_as_ingress_egress_and_hairpin() {
    let a = address_uplink_app();
    let reason = addr_reason("ens1f0", "192.168.1.5");
    assert_start_refused(&a, NIC0, NIC1, NIC0, &reason).await;
    assert_start_refused(&a, NIC1, NIC0, NIC0, &reason).await;
    assert_start_refused(&a, NIC0, NIC0, NIC0, &reason).await;
}

#[tokio::test]
async fn start_with_an_open_vswitch_port_is_a_409_as_ingress_and_egress() {
    let a = ovs_uplink_app();
    let reason = ovs_reason("ens1f0");
    assert_start_refused(&a, NIC0, NIC1, NIC0, &reason).await;
    assert_start_refused(&a, NIC1, NIC0, NIC0, &reason).await;
}

#[tokio::test]
async fn start_names_the_device_above_when_the_address_is_on_a_bond() {
    let h = host();
    h.upper_link("ens1f0", "bond0");
    h.addr("bond0", "192.168.1.5");
    let a = TestApp::new(h);
    assert_start_refused(
        &a,
        NIC0,
        NIC1,
        NIC0,
        &addr_above("bond0", "ens1f0", "192.168.1.5"),
    )
    .await;
}

#[tokio::test]
async fn start_refuses_an_mlx5_port_whose_bond_carries_a_route() {
    let host = FakeHost::bare_metal();
    host.master_link("ens1f0", "bond0");
    host.routes6(&[("bond0", DEFAULT6, 0)]);
    let a = TestApp::new(host);
    assert_start_refused(&a, NIC0, NIC1, NIC0, &route_above("bond0", "ens1f0")).await;
}

#[tokio::test]
async fn the_second_link_of_a_body_is_checked_too() {
    let a = address_uplink_app();
    let r = a
        .start_links(vec![link(NIC1, NIC1), link("net_null0", NIC0)])
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(
        r.message().contains(&format!("link net_null0 -> {NIC0}")),
        "{}",
        r.message()
    );
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn start_still_works_on_free_nics_and_vdevs_next_to_link_local_addresses() {
    let h = host();
    h.addr("ens1f0", "fe80::1");
    h.addr("ens2f0", "fe80::2");
    h.addr("lo", "127.0.0.1");
    let a = TestApp::new(h);
    assert_eq!(a.start_links(vec![link(NIC0, NIC1)]).await.status, 202);
    let a = app();
    assert_eq!(
        a.start_links(vec![link("net_null0", "net_null0")])
            .await
            .status,
        202
    );
}

#[tokio::test]
async fn validate_refuses_every_kind_of_uplink_like_start_and_spawns_nothing() {
    let a = app();
    for (i, e) in [(MGMT, NIC1), (NIC1, MGMT), (MGMT, MGMT)] {
        let r = a.validate(&state_using(i, e)).await;
        assert_eq!(r.status, 409, "{i} -> {e}: {}", r.text());
        assert!(r.message().contains(MGMT), "{}", r.text());
        assert!(r.message().contains("route"), "{}", r.text());
    }
    // and an unrouted pair validates
    assert_eq!(a.validate(&state_using(NIC0, NIC1)).await.status, 200);
    for a in [address_uplink_app(), ovs_uplink_app()] {
        for (i, e) in [(NIC0, NIC1), (NIC1, NIC0), (NIC0, NIC0)] {
            let r = a.validate(&setup(vec![link(i, e)])).await;
            assert_eq!(r.status, 409, "{i} -> {e}: {}", r.text());
            let m = r.message();
            assert!(m.contains(NIC0) && m.contains("host uplink"), "{m}");
            assert!(
                m.contains(&format!("link {i} -> {e}")),
                "the link is named by its ports: {m}"
            );
        }
        assert_eq!(a.validate(&state_using(NIC1, NIC1)).await.status, 200);
        assert!(a.supervisor.plans().is_empty());
    }
}

#[tokio::test]
async fn post_hints_with_an_uplink_port_is_a_200_with_the_error_hint() {
    let a = address_uplink_app();
    for (i, e) in [(NIC0, NIC1), (NIC1, NIC0)] {
        let r = a.post("/api/host/hints", state_using(i, e).body()).await;
        assert_eq!(r.status, 200, "{i} -> {e}: {}", r.text());
        let found: Vec<Value> = r
            .json()
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["id"] == "uplink")
            .cloned()
            .collect();
        assert_one_error_naming(&found, &[NIC0]);
    }
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn the_interfaces_endpoint_has_uplink_and_no_kernel_route() {
    let a = address_uplink_app();
    let v = a.get("/api/interfaces").await.json();
    let list = v.as_array().unwrap();
    for n in list {
        assert!(n["uplink"].is_boolean(), "{n}");
        assert!(n.get("kernel_route").is_none(), "{n}");
    }
    let n = list.iter().find(|i| i["pci"] == NIC0).unwrap();
    assert_uplink(n, &addr_reason("ens1f0", "192.168.1.5"));
    let free = list.iter().find(|i| i["pci"] == NIC1).unwrap();
    assert_free(free);
}

#[tokio::test]
async fn the_interfaces_endpoint_marks_the_routed_management_nic() {
    let a = app();
    let v = a.get("/api/interfaces").await.json();
    let m = v
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pci"] == MGMT)
        .unwrap();
    assert_eq!(m["uplink"], true);
    assert_eq!(m["dpds_usable"], false);
    assert!(m["reason"].as_str().is_some_and(|r| !r.is_empty()));
}

#[tokio::test]
async fn the_openapi_document_has_uplink_and_no_kernel_route() {
    let a = app();
    let text = a.get("/api/openapi.json").await.text();
    let doc: Value = serde_json::from_str(&text).unwrap();
    let props = &doc["components"]["schemas"]["Interface"]["properties"];
    assert_eq!(props["uplink"]["type"], "boolean", "{props}");
    assert!(props.get("kernel_route").is_none(), "{props}");
    assert!(!text.contains("kernel_route"));
    assert!(!text.contains("management interface"));
}

#[test]
fn the_controller_sources_have_no_kernel_route_or_management_nic_left() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    for e in std::fs::read_dir(&src).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            let text = std::fs::read_to_string(&p).unwrap();
            for needle in ["kernel_route", "management_nic", "management interface"] {
                if text.contains(needle) {
                    hits.push(format!("{}: {needle}", p.display()));
                }
            }
        }
    }
    assert!(hits.is_empty(), "leftovers of the old names: {hits:#?}");
}

// ---- the binary: --ifaddrs-file / DPDS_CONTROLLER_IFADDRS_FILE ----------------------------

fn interfaces_of(c: &ControllerProcess) -> Vec<Value> {
    let (st, v) = http(&c.addr, "GET", "/api/interfaces", None).unwrap();
    assert_eq!(st, 200, "{v}");
    v.as_array().unwrap().clone()
}

fn fake_roots_args(h: &FakeHost) -> Vec<String> {
    vec![
        "serve".into(),
        "--bind".into(),
        "{addr}".into(),
        "--sysfs-root".into(),
        h.sysfs().display().to_string(),
        "--procfs-root".into(),
        h.procfs().display().to_string(),
    ]
}

fn start_binary(args: &[String], envs: &[(&str, &str)]) -> ControllerProcess {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    ControllerProcess::start(&args, envs)
}

#[test]
fn the_binary_marks_the_netdevs_of_the_ifaddrs_file_as_uplinks() {
    let h = host();
    h.addr("ens1f0", "192.168.1.5");
    h.addr("ens1f0", "2001:db8::5");
    h.addr("ens2f0", "fe80::2");
    let mut args = fake_roots_args(&h);
    args.push("--ifaddrs-file".into());
    args.push(h.ifaddrs_file().display().to_string());
    let c = start_binary(&args, &[]);
    let v = interfaces_of(&c);
    assert_uplink(
        v.iter().find(|i| i["pci"] == NIC0).unwrap(),
        &addr_reason("ens1f0", "192.168.1.5"),
    );
    assert_free(v.iter().find(|i| i["pci"] == NIC1).unwrap());
    // and the guard works through the API of that process
    let body = state_using(NIC0, NIC1).body();
    let (st, r) = http(&c.addr, "POST", "/api/emulation/validate", Some(&body)).unwrap();
    assert_eq!(st, 409, "{r}");
    assert!(
        r["message"].as_str().unwrap().contains("192.168.1.5"),
        "{r}"
    );
}

#[test]
fn the_binary_reads_the_ifaddrs_file_from_the_environment_too() {
    let h = host();
    h.addr("ens2f0", "10.20.30.40");
    let file = h.ifaddrs_file();
    let c = start_binary(
        &fake_roots_args(&h),
        &[("DPDS_CONTROLLER_IFADDRS_FILE", file.to_str().unwrap())],
    );
    let v = interfaces_of(&c);
    assert_uplink(
        v.iter().find(|i| i["pci"] == NIC1).unwrap(),
        &addr_reason("ens2f0", "10.20.30.40"),
    );
    assert_free(v.iter().find(|i| i["pci"] == NIC0).unwrap());
}

#[test]
fn the_binary_works_without_an_ifaddrs_file() {
    // absent: the real getifaddrs; the fake netdevs are not among the machine's interfaces
    let h = host();
    let c = start_binary(&fake_roots_args(&h), &[]);
    let v = interfaces_of(&c);
    assert_free(v.iter().find(|i| i["pci"] == NIC0).unwrap());
    assert_free(v.iter().find(|i| i["pci"] == NIC1).unwrap());
}

#[test]
fn an_empty_ifaddrs_file_means_no_addresses() {
    let h = host();
    let mut args = fake_roots_args(&h);
    args.push("--ifaddrs-file".into());
    args.push(h.ifaddrs_file().display().to_string()); // `[]`
    let c = start_binary(&args, &[]);
    for n in interfaces_of(&c) {
        if n["pci"] == NIC0 || n["pci"] == NIC1 {
            assert_free(&n);
        }
    }
}

#[test]
fn the_help_documents_the_ifaddrs_file_as_test_only_like_the_roots() {
    for cmd in ["serve", "check"] {
        let o = controller_cmd().args([cmd, "--help"]).output().unwrap();
        let out = String::from_utf8_lossy(&o.stdout).to_lowercase();
        assert!(out.contains("--ifaddrs-file"), "{cmd}: {out}");
        assert!(out.contains("dpds_controller_ifaddrs_file"), "{cmd}: {out}");
        let line = out
            .lines()
            .skip_while(|l| !l.contains("--ifaddrs-file"))
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            line.contains("test"),
            "{cmd}: documented as test-only: {line}"
        );
    }
}
