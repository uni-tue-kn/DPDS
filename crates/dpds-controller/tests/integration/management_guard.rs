//! C1: the management-NIC guard. A NIC whose netdev, or a bond/VLAN/bridge device
//! above it (sysfs `upper_*` and `master` links, also nested), carries an IPv4 route
//! (`/proc/net/route`) or an IPv6 route (`/proc/net/ipv6_route`) is not usable by DPDK:
//!
//! - discovery: `kernel_route: true`, `dpds_usable: false`, `reason` explaining it
//!   (naming the upper device when the route is on one);
//! - hints: an `error` hint with id `management_nic` naming the NIC, when a stored link
//!   uses such a port (`check` then exits 1);
//! - `POST /api/emulation/start` answers 409 `{"message"}` naming the port and spawns
//!   nothing. Creating or updating a link with such a port is still allowed.
//!
//! IPv6: link-local-only interfaces are not "routed". The kernel lists `fe80::/64`, the
//! link-local address itself (`/128`) and the multicast route `ff00::/8` on every IPv6
//! interface, and `lo` has `::1/128`; none of these counts (the tests below use the
//! realistic set). ASSUMPTION of the tester: `ff00::/8` is part of "link-local-only".

use std::process::Command;

use crate::common::*;
use dpds_controller::discovery::discover;
use dpds_controller::store;
use serde_json::{json, Value};

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

fn assert_guarded(n: &Value, needle: &str) {
    assert_eq!(n["kernel_route"], true, "{n}");
    assert_eq!(n["dpds_usable"], false, "{n}");
    let reason = n["reason"].as_str().unwrap_or_default().to_lowercase();
    assert!(
        reason.contains("route") || reason.contains("management"),
        "reason must explain the guard: {n}"
    );
    assert!(reason.contains(needle), "reason must name {needle}: {n}");
}

fn assert_free(n: &Value) {
    assert_eq!(n["kernel_route"], false, "{n}");
    assert_eq!(n["dpds_usable"], true, "{n}");
}

// ---- discovery -------------------------------------------------------------

#[test]
fn an_ipv4_route_on_the_nic_itself_guards_it_even_with_an_mlx5_driver() {
    let h = host();
    h.routes(&[("ens1f0", "00000000", "0100A8C0", "00000000")]);
    let v = list(&h);
    assert_guarded(&nic(&v, NIC0), "route");
    assert_free(&nic(&v, NIC1));
}

#[test]
fn an_ipv6_only_default_route_guards_the_nic() {
    let h = host();
    h.routes6(&[("ens1f0", DEFAULT6, 0)]);
    assert_guarded(&nic(&list(&h), NIC0), "route");
}

#[test]
fn an_ipv6_prefix_route_on_the_nic_guards_it() {
    let h = host();
    h.routes6(&[("ens2f0", GLOBAL6, 64)]);
    let v = list(&h);
    assert_guarded(&nic(&v, NIC1), "route");
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
    assert_guarded(&nic(&list(&h), NIC0), "route");
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
    assert_guarded(&nic(&v, NIC0), "bond0");
    assert_free(&nic(&v, NIC1));
}

#[test]
fn a_route_on_a_bond_above_the_nic_guards_it_via_the_master_link_alone() {
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.routes6(&[("bond0", DEFAULT6, 0)]);
    assert_guarded(&nic(&list(&h), NIC0), "bond0");
}

#[test]
fn a_route_on_a_bridge_master_guards_the_nic() {
    let h = host();
    h.master_link("ens2f0", "br0");
    h.routes(&[("br0", "0000A8C0", "00000000", "00FFFFFF")]);
    let v = list(&h);
    assert_guarded(&nic(&v, NIC1), "br0");
    assert_free(&nic(&v, NIC0));
}

#[test]
fn a_route_on_a_vlan_above_the_nic_guards_it() {
    let h = host();
    h.upper_link("ens1f0", "ens1f0.100");
    h.routes6(&[("ens1f0.100", GLOBAL6, 64)]);
    assert_guarded(&nic(&list(&h), NIC0), "ens1f0.100");
}

#[test]
fn a_route_on_a_vlan_above_a_bond_above_the_nic_guards_it() {
    // ens1f0 -> bond0 -> bond0.200 (the usual management setup)
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.upper_link("bond0", "bond0.200");
    h.routes(&[("bond0.200", "00000000", "0100A8C0", "00000000")]);
    assert_guarded(&nic(&list(&h), NIC0), "bond0");
}

#[test]
fn an_upper_device_without_a_route_does_not_guard_the_nic() {
    let h = host();
    h.upper_link("ens1f0", "bond0");
    h.routes(&[("eno1", "00000000", "0100A8C0", "00000000")]);
    assert_free(&nic(&list(&h), NIC0));
}

// ---- hints, check, start ---------------------------------------------------

fn state_using(ports: &[&str]) -> dpds_controller::model::StoredState {
    state_with(vec![link(1, "l", ports, None)])
}

fn mgmt_hints(host: &FakeHost, ports: &[&str]) -> Vec<Value> {
    hints_json(host, &state_using(ports))
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["id"] == "management_nic")
        .cloned()
        .collect()
}

#[test]
fn a_stored_link_on_the_management_nic_gives_one_error_hint_naming_it() {
    let h = FakeHost::bare_metal();
    let found = mgmt_hints(&h, &[MGMT, NIC1]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["level"], "error");
    let text = format!("{} {}", found[0]["title"], found[0]["detail"]);
    assert!(text.contains(MGMT), "the NIC is named: {text}");
}

#[test]
fn the_hint_covers_bond_and_ipv6_routes_too() {
    let h = host();
    h.master_link("ens1f0", "bond0");
    h.routes6(&[("bond0", DEFAULT6, 0)]);
    let found = mgmt_hints(&h, &[NIC0, NIC1]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["level"], "error");
    assert!(found[0]["detail"].as_str().unwrap().contains(NIC0));
}

#[test]
fn no_error_hint_when_no_stored_link_uses_a_routed_nic() {
    let h = FakeHost::bare_metal(); // MGMT is routed, but unused
    assert!(mgmt_hints(&h, &[NIC0, NIC1])
        .iter()
        .all(|x| x["level"] != "error"));
    assert!(mgmt_hints(&h, &["net_null0"])
        .iter()
        .all(|x| x["level"] != "error"));
    let all = hints_json(&h, &Default::default());
    assert!(all
        .as_array()
        .unwrap()
        .iter()
        .all(|x| x["id"] != "management_nic" || x["level"] != "error"));
}

#[test]
fn check_exits_one_and_names_the_management_nic() {
    let h = FakeHost::bare_metal();
    let state = tempfile::tempdir().unwrap();
    store::save(state.path(), &state_using(&[MGMT, NIC1])).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_dpds-controller"))
        .arg("check")
        .arg("--sysfs-root")
        .arg(h.sysfs())
        .arg("--procfs-root")
        .arg(h.procfs())
        .arg("--state-dir")
        .arg(state.path())
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains(MGMT), "{out}");
}

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

#[tokio::test]
async fn start_with_a_link_on_the_management_nic_is_a_409_and_spawns_nothing() {
    let a = app();
    assert_eq!(a.create_link("m", &[MGMT, NIC1]).await.status, 201);
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().contains(MGMT), "the port is named");
    assert!(a.supervisor.plans().is_empty());
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

#[tokio::test]
async fn start_refuses_an_mlx5_port_whose_bond_carries_a_route() {
    let host = FakeHost::bare_metal();
    host.master_link("ens1f0", "bond0");
    host.routes6(&[("bond0", DEFAULT6, 0)]);
    let a = TestApp::new(host);
    assert_eq!(a.create_link("b", &[NIC0, NIC1]).await.status, 201);
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().contains(NIC0));
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn start_still_works_on_unrouted_nics_and_vdevs() {
    let a = app();
    a.create_link("ok", &[NIC0, NIC1]).await;
    assert_eq!(a.post("/api/emulation/start", json!({})).await.status, 202);
    let a = app();
    a.create_link("v", &["net_null0"]).await;
    assert_eq!(a.post("/api/emulation/start", json!({})).await.status, 202);
}

#[tokio::test]
async fn creating_and_updating_a_link_with_a_management_port_is_allowed() {
    let a = app();
    let r = a.create_link("m", &[MGMT, NIC1]).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = r.json()["id"].as_u64().unwrap();
    let r = a
        .put(&format!("/api/links/{id}"), link_body("m2", &[NIC1, MGMT]))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
}

#[tokio::test]
async fn the_interfaces_endpoint_carries_the_guard_fields() {
    let a = app();
    let v = a.get("/api/interfaces").await.json();
    let m = v
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pci"] == MGMT)
        .unwrap();
    assert_eq!(m["kernel_route"], true);
    assert_eq!(m["dpds_usable"], false);
    assert!(m["reason"].as_str().is_some_and(|r| !r.is_empty()));
}
