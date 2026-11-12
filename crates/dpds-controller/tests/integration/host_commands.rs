//! DPDS never changes the host: a hint that can be fixed suggests the exact root commands
//! (`commands`) and an administrator runs them.
//!
//! - `commands` is omitted (never null, never `[]`) when a hint suggests nothing. Each element is
//!   one trimmed shell line computed from what the controller read (netdev names, PCI addresses,
//!   IRQ numbers, cpulists, page counts); paths are the real `/proc/...` and `/sys/...` ones, never
//!   the root a test points the controller at. The commands per check are asserted below.
//! - A missing tool gives `["apt install <package>"]` at level info and nothing else. No tuning
//!   command for a NIC that is an uplink or not `dpds_usable` (the detail says why), for a virtual
//!   function (MaxReadReq), or for a netdev whose name is not `^[A-Za-z0-9_.@-]+$` or starts with
//!   `-` (no `ethtool`/`dcb` call either; the detail escapes control characters as `\x1b`). The
//!   netdev and PCI address in a command come from sysfs, never from the request body.
//! - Hint computation only runs read-only tools (`ethtool --json -a`, `dcb -j buffer show`,
//!   `mstconfig|mlxconfig ... q`) through the `Exec` seam, each with a 30 s timeout and a shared
//!   budget of 60 s per request. `Exec::now()` is the clock for that budget; `FakeExec` simulates it
//!   (it moves only for calls registered with `FakeExec::takes`). Tools run only while the
//!   emulation is Stopped or Failed, and without taking the lifecycle lock; otherwise the per-NIC
//!   hints that need one say "not checked while an emulation is running".
//! - `SystemExec::run` runs the child in its own process group, kills the group on timeout, joins
//!   its reader threads only up to the deadline, retries reads that fail with `Interrupted`, and
//!   fails with `InvalidData` above 4 MiB of output (exactly 4 MiB is fine).
//! - The firmware query of a NIC (function 0) is cached per router, in memory, keyed by the
//!   function-0 address; only an answered and parsed query is stored. A cached hint runs no tool,
//!   costs no budget and ends its detail with "(firmware values read <N> s ago; Recheck to read
//!   them again)". `?recheck=true` drops the entries of the posted links first; any other value
//!   of `recheck` is a 400 naming it. `dpds-controller check` has no cache and no `--recheck`.
//! - `POST /api/host/actions/<anything>` is a 404 and OpenAPI has no such path.
//! - No source file executes a mutating argv, writes below `/proc` or `/sys`, or writes any file
//!   except the supervisor's links file and the telemetry socket.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::common::*;
use dpds_controller::host::{parse_cpulist, Exec, SystemExec};
use dpds_controller::supervisor::State;
use serde_json::{json, Value};

const NEED_PER_MBUF: u64 = 2432;
const PAGE: u64 = 2 * 1024 * 1024;
const T30: Duration = Duration::from_secs(30);

fn link_on(pci: &str) -> Setup {
    setup(vec![link(pci, pci)])
}

fn text(hints: &Value, id: &str) -> String {
    let h = hint(hints, id);
    format!(
        "{} {} {}",
        h["title"].as_str().unwrap_or_default(),
        h["detail"].as_str().unwrap_or_default(),
        h["suggestion"].as_str().unwrap_or_default()
    )
    .to_lowercase()
}

fn h_level<'a>(hints: &'a Value, id: &str) -> &'a str {
    hint(hints, id)["level"].as_str().expect("a hint level")
}

fn has_hint(hints: &Value, id: &str) -> bool {
    hint_ids(hints).iter().any(|i| i == id)
}

fn eth(autoneg: bool, rx: bool, tx: bool) -> EthtoolState {
    EthtoolState { autoneg, rx, tx }
}

fn st(s: &str) -> String {
    s.to_string()
}

/// `echo <cpulist> > /proc/irq/<n>/smp_affinity_list` as `(n, cpus)`.
fn irq_command(cmd: &str) -> (u32, Vec<u32>) {
    let rest = cmd
        .strip_prefix("echo ")
        .unwrap_or_else(|| panic!("not an echo command: {cmd:?}"));
    let (list, path) = rest
        .split_once(" > ")
        .unwrap_or_else(|| panic!("no redirection: {cmd:?}"));
    assert!(
        !list.is_empty()
            && list
                .chars()
                .all(|c| c.is_ascii_digit() || c == ',' || c == '-'),
        "not a cpulist: {cmd:?}"
    );
    let n = path
        .strip_prefix("/proc/irq/")
        .and_then(|p| p.strip_suffix("/smp_affinity_list"))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("not an IRQ affinity file: {cmd:?}"));
    (n, parse_cpulist(list))
}

/// `echo <n> > <path>` as `(n, path)`.
fn echo_command(cmd: &str) -> (u64, String) {
    let (n, path) = cmd
        .strip_prefix("echo ")
        .and_then(|r| r.split_once(" > "))
        .unwrap_or_else(|| panic!("not an `echo <n> > <path>` command: {cmd:?}"));
    (n.parse().unwrap_or_else(|_| panic!("{cmd:?}")), path.into())
}

fn set_mrr(host: &FakeHost, pci: &str, bytes: u32) {
    host.write_bytes(
        &format!("sys/bus/pci/devices/{pci}/config"),
        &config_space(bytes),
    );
}

/// The `reason` `GET /api/interfaces` gives for `pci`.
async fn reason_of(a: &TestApp, pci: &str) -> String {
    let v = a.get("/api/interfaces").await.json();
    v.as_array()
        .unwrap()
        .iter()
        .find(|i| i["pci"] == pci)
        .and_then(|i| i["reason"].as_str())
        .unwrap_or_else(|| panic!("no reason for {pci} in {v}"))
        .to_string()
}

// ---- vm.swappiness and vm.zone_reclaim_mode ------------------------------------------------

#[test]
fn swappiness_not_zero_names_the_command_with_current_and_target() {
    let host = FakeHost::bare_metal();
    host.sysctl("swappiness", Some("60"));
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "swappiness", "info", "info");
    assert_commands(&h, "swappiness", &["sysctl -w vm.swappiness=0"]);
    assert_no_action_members(&h);
    assert!(detail_has(&h, "swappiness", "60"), "{h}");
    assert!(
        text(&h, "swappiness").contains("0"),
        "the target 0 is named: {h}"
    );
    assert_eq!(
        host.read("proc/sys/vm/swappiness"),
        "60",
        "the hint changes nothing"
    );
}

#[test]
fn swappiness_zero_is_ok_and_unreadable_suggests_nothing() {
    let host = FakeHost::bare_metal();
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "swappiness", "ok", "ok");
    assert_no_commands(&h, "swappiness");
    host.sysctl("swappiness", None);
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "swappiness", "ok", "info");
    assert_no_commands(&h, "swappiness");
}

#[test]
fn zone_reclaim_mode_absent_is_ok_nonzero_suggests_the_sysctl() {
    let host = FakeHost::bare_metal();
    host.sysctl("zone_reclaim_mode", None); // no NUMA
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "zone_reclaim_mode", "ok", "ok");
    assert_no_commands(&h, "zone_reclaim_mode");
    host.sysctl("zone_reclaim_mode", Some("1"));
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "zone_reclaim_mode", "info", "info");
    assert_commands(
        &h,
        "zone_reclaim_mode",
        &["sysctl -w vm.zone_reclaim_mode=0"],
    );
    assert!(detail_has(&h, "zone_reclaim_mode", "1"), "{h}");
    assert_eq!(host.read("proc/sys/vm/zone_reclaim_mode"), "1");
    host.sysctl("zone_reclaim_mode", Some("0"));
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "zone_reclaim_mode", "ok", "ok");
    assert_no_commands(&h, "zone_reclaim_mode");
}

// ---- NIC interrupts off the isolated cores --------------------------------------------------

#[test]
fn nic_irqs_on_housekeeping_cores_are_ok() {
    let h = hints_json(&FakeHost::bare_metal(), &link_on(NIC0));
    assert_level(&h, "irq_affinity", "ok", "ok");
    assert_no_commands(&h, "irq_affinity");
    assert!(!has_hint(&h, "irqbalance"), "the irqbalance hint is gone");
}

#[test]
fn no_link_means_no_nic_irqs_to_check() {
    let host = FakeHost::bare_metal();
    host.irq_affinity(101, "3");
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "irq_affinity", "ok", "ok");
    assert_no_commands(&h, "irq_affinity");
}

#[test]
fn a_nic_irq_on_an_isolated_core_warns_and_names_the_commands() {
    let host = FakeHost::bare_metal();
    host.irq_affinity(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "warn", "warn");
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    // NIC0 sits on node 0 (cores 0-3, isolated 2-3): the housekeeping cores of that node
    assert_eq!(irq_command(&cmds[0]), (101, vec![0, 1]));
    assert!(detail_has(&h, "irq_affinity", "101"), "names the irq: {h}");
    assert!(!detail_has(&h, "irq_affinity", "irq 100"), "{h}");
}

#[test]
fn the_irq_hint_works_from_the_smp_affinity_when_there_is_no_effective_list() {
    let host = FakeHost::bare_metal();
    host.remove("proc/irq/101/effective_affinity_list");
    host.irq_affinity(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "warn", "warn");
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (101, vec![0, 1]));
}

#[test]
fn an_irq_without_a_smp_affinity_list_is_skipped() {
    // the effective list alone on an isolated core does not count
    let host = FakeHost::bare_metal();
    host.remove("proc/irq/101/smp_affinity_list");
    host.irq_effective(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "ok", "ok");
    assert_no_commands(&h, "irq_affinity");
    // the other IRQs of the NIC are still judged
    host.irq_affinity(100, "2");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "warn", "warn");
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]).0, 100);
}

#[test]
fn a_move_still_pending_is_ok_and_named_in_the_detail() {
    // `echo 0,1 > smp_affinity_list` was run, the IRQ has not fired since: effective is still core 3
    let host = FakeHost::bare_metal();
    host.irq_affinity(101, "0,1");
    host.irq_effective(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "ok", "ok");
    assert_no_commands(&h, "irq_affinity");
    assert!(detail_has(&h, "irq_affinity", "101"), "names the irq: {h}");
    assert!(
        detail_has(&h, "irq_affinity", "next"),
        "says the kernel moves it when it next fires: {h}"
    );
}

#[test]
fn the_requested_affinity_on_an_isolated_core_warns_even_when_the_effective_one_is_housekeeping() {
    let host = FakeHost::bare_metal();
    host.irq_affinity(101, "3");
    host.irq_effective(101, "1");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "warn", "warn");
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (101, vec![0, 1]));
}

#[test]
fn a_pending_irq_next_to_a_warning_one_gets_no_command_but_is_named() {
    let host = FakeHost::bare_metal();
    host.irq_affinity(100, "2"); // still requested on an isolated core
    host.irq_affinity(101, "0,1"); // moved already, effective not yet
    host.irq_effective(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "warn", "warn");
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (100, vec![0, 1]));
    assert!(detail_has(&h, "irq_affinity", "100"), "{h}");
    assert!(
        detail_has(&h, "irq_affinity", "101"),
        "the pending irq is named too: {h}"
    );
    assert!(detail_has(&h, "irq_affinity", "next"), "{h}");
}

#[test]
fn only_the_irqs_of_the_used_nics_count() {
    let host = FakeHost::bare_metal();
    host.irq_affinity(110, "2"); // NIC1, on an isolated core
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "ok", "ok");
    let h = hints_json(&host, &link_on(NIC1));
    assert_level(&h, "irq_affinity", "warn", "warn");
    // the egress NIC counts as well
    let h = hints_json(&host, &setup(vec![link(NIC0, NIC1)]));
    assert_level(&h, "irq_affinity", "warn", "warn");
}

#[test]
fn without_isolated_cores_there_is_nothing_to_move_away_from() {
    let host = FakeHost::bare_metal();
    host.cpus(8, Some(""), &[(0, "0-3"), (1, "4-7")]);
    host.irq_affinity(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "ok", "info");
    assert_no_commands(&h, "irq_affinity");
}

#[test]
fn nohz_full_cores_count_as_isolated_too() {
    let host = FakeHost::bare_metal();
    host.cpus(8, Some(""), &[(0, "0-3"), (1, "4-7")]);
    host.write("sys/devices/system/cpu/nohz_full", "2-3\n");
    host.irq_affinity(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "warn", "warn");
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    // node 0 is cores 0-3, of which 2-3 are nohz_full
    assert_eq!(irq_command(&cmds[0]), (101, vec![0, 1]));
}

#[test]
fn a_running_irqbalance_is_named_in_the_detail() {
    let host = FakeHost::bare_metal();
    host.irq_affinity(101, "3");
    host.processes(&[(812, "irqbalance")]);
    let h = hints_json(&host, &link_on(NIC0));
    assert!(detail_has(&h, "irq_affinity", "irqbalance"), "{h}");
}

#[test]
fn the_irq_commands_move_only_the_irqs_on_isolated_cores_to_the_housekeeping_cores_of_the_node() {
    let host = FakeHost::bare_metal(); // isolated 2-3,6-7 of 8 CPUs
    host.irq_affinity(100, "2");
    host.irq_affinity(101, "3");
    host.irq_affinity(110, "2"); // NIC1: not used by the link
    let h = hints_json(&host, &link_on(NIC0));
    let cmds = commands_of(&h, "irq_affinity");
    let parsed: Vec<(u32, Vec<u32>)> = cmds.iter().map(|c| irq_command(c)).collect();
    assert_eq!(
        parsed,
        [(100, vec![0, 1]), (101, vec![0, 1])],
        "the NIC's IRQs on isolated cores, ascending, no others, to node 0's housekeeping cores: {cmds:?}"
    );
    for c in &cmds {
        assert!(
            c.ends_with(" > /proc/irq/100/smp_affinity_list")
                || c.ends_with(" > /proc/irq/101/smp_affinity_list"),
            "the real /proc path, not the fake root: {c}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(host.tmp.path().join("proc/irq/100/smp_affinity_list")).unwrap(),
        "2\n",
        "nothing is written"
    );
}

#[test]
fn the_target_is_the_non_isolated_online_cores_of_the_nics_node() {
    // node 1 is cores 4-7; core 4 is isolated and core 7 is offline
    let host = FakeHost::bare_metal();
    host.cpus(7, Some("2-3,4"), &[(0, "0-3"), (1, "4-7")]);
    host.irq_affinity(110, "4");
    let h = hints_json(&host, &link_on(NIC1));
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (110, vec![5, 6]));
}

#[test]
fn each_nic_gets_the_housekeeping_cores_of_its_own_node() {
    let host = FakeHost::bare_metal(); // node 0: 0-3, node 1: 4-7, isolated 2-3,6-7
    host.irq_affinity(101, "3"); // NIC0, node 0
    host.irq_affinity(110, "6"); // NIC1, node 1
    host.irq_affinity(111, "7");
    let h = hints_json(&host, &setup(vec![link(NIC0, NIC1)]));
    let parsed: Vec<(u32, Vec<u32>)> = commands_of(&h, "irq_affinity")
        .iter()
        .map(|c| irq_command(c))
        .collect();
    assert_eq!(
        parsed,
        [(101, vec![0, 1]), (110, vec![4, 5]), (111, vec![4, 5])]
    );
}

#[test]
fn a_node_without_housekeeping_cores_falls_back_to_all_non_isolated_online_cores() {
    // all of node 1 (4-7) is isolated
    let host = FakeHost::bare_metal();
    host.cpus(8, Some("2-3,4-7"), &[(0, "0-3"), (1, "4-7")]);
    host.irq_affinity(110, "5");
    host.irq_affinity(111, "0"); // already on a housekeeping core
    let h = hints_json(&host, &link_on(NIC1));
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (110, vec![0, 1]));
}

#[test]
fn without_numa_information_the_target_is_all_non_isolated_online_cores() {
    // the VM has no NUMA node for its NIC and no `node` directory; isolated 2-3 of 4 cores
    let host = FakeHost::vm();
    host.nic_irqs("0000:00:05.0", &[(30, "2"), (31, "0")]);
    let h = hints_json(&host, &link_on("0000:00:05.0"));
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (30, vec![0, 1]));
    // a NIC whose numa_node is -1 (unknown) is no different
    let host = FakeHost::bare_metal();
    host.write("sys/bus/pci/devices/0000:65:00.0/numa_node", "-1\n");
    host.irq_affinity(110, "6");
    let h = hints_json(&host, &link_on(NIC1));
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (110, vec![0, 1, 4, 5]));
}

#[test]
fn nics_that_are_uplinks_or_not_usable_are_skipped_and_named() {
    // NIC1 has an address (uplink), INTEL is bound to `ice` (not usable); both have an interrupt
    // on an isolated core. NIC0, which is usable, has one too
    let host = FakeHost::bare_metal();
    host.addr("ens2f0", "192.168.7.7");
    host.irq_affinity(101, "3");
    host.irq_affinity(110, "6");
    host.irq_affinity(120, "2");
    let h = hints_json(&host, &setup(vec![link(NIC0, NIC1), link(INTEL, NIC0)]));
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(
        cmds.iter().map(|c| irq_command(c)).collect::<Vec<_>>(),
        [(101, vec![0, 1])],
        "only the interrupt of the usable NIC is moved: {cmds:?}"
    );
    for (netdev, pci) in [("ens2f0", NIC1), ("enp179s0", INTEL)] {
        assert!(
            detail_has(&h, "irq_affinity", netdev) || detail_has(&h, "irq_affinity", pci),
            "the detail names the skipped NIC {netdev}: {h}"
        );
    }
    for irq in ["110", "120"] {
        assert!(
            !commands_of(&h, "irq_affinity")
                .iter()
                .any(|c| c.contains(&format!("/proc/irq/{irq}/"))),
            "{irq}"
        );
    }
}

#[test]
fn when_every_used_nic_is_skipped_there_is_no_irq_command_and_the_detail_names_them() {
    let host = FakeHost::bare_metal();
    host.addr("ens2f0", "192.168.7.7");
    host.irq_affinity(110, "6");
    host.irq_affinity(120, "2");
    for (pci, name) in [(NIC1, "ens2f0"), (INTEL, "enp179s0")] {
        let h = hints_json(&host, &link_on(pci));
        assert_no_commands(&h, "irq_affinity");
        assert!(
            rank(h_level(&h, "irq_affinity")) <= rank("info"),
            "nothing to do for a NIC DPDS cannot use: {h}"
        );
        assert!(
            detail_has(&h, "irq_affinity", name) || detail_has(&h, "irq_affinity", pci),
            "{name}: {h}"
        );
    }
}

// ---- Ethernet flow control -------------------------------------------------------------------

#[test]
fn flow_control_without_ethtool_names_the_package_and_the_install_command() {
    let host = FakeHost::bare_metal();
    let h = hints_json(&host, &link_on(NIC0));
    let id = nic_id("flow_control", NIC0);
    assert_level(&h, &id, "info", "info");
    assert_commands(&h, &id, &["apt install ethtool"]);
    assert!(text(&h, &id).contains("ethtool"), "{h}");
}

#[test]
fn flow_control_on_names_the_command_and_off_is_ok() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    let state = fake_ethtool(&exec, "ens1f0", eth(false, true, true));
    let id = nic_id("flow_control", NIC0);
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &id, "info", "info");
    assert_commands(&h, &id, &["ethtool -A ens1f0 rx off tx off"]);
    let t = text(&h, &id);
    assert!(
        t.contains("rx") && t.contains("tx") && t.contains("on"),
        "{t}"
    );
    // one direction on is still on
    *state.lock().unwrap() = eth(false, true, false);
    assert_commands(
        &hints_json(&host, &link_on(NIC0)),
        &id,
        &["ethtool -A ens1f0 rx off tx off"],
    );
    *state.lock().unwrap() = eth(false, false, false);
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &id, "ok", "ok");
    assert_no_commands(&h, &id);
    // negotiated pause frames count as off when rx and tx are off
    *state.lock().unwrap() = eth(true, false, false);
    assert_level(&hints_json(&host, &link_on(NIC0)), &id, "ok", "ok");
    exec.assert_read_only();
}

#[test]
fn flow_control_with_autonegotiation_on_turns_it_off_in_the_same_command() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_ethtool(&exec, "ens1f0", eth(true, true, true));
    let id = nic_id("flow_control", NIC0);
    let h = hints_json(&host, &link_on(NIC0));
    let cmds = commands_of(&h, &id);
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    let words: Vec<&str> = cmds[0].split(' ').collect();
    assert_eq!(&words[..3], ["ethtool", "-A", "ens1f0"], "{cmds:?}");
    let mut pairs: Vec<(&str, &str)> = words[3..].chunks(2).map(|p| (p[0], p[1])).collect();
    pairs.sort();
    assert_eq!(
        pairs,
        [("autoneg", "off"), ("rx", "off"), ("tx", "off")],
        "{cmds:?}"
    );
}

#[test]
fn flow_control_falls_back_to_the_text_output_of_an_old_ethtool() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    exec.on(
        &["ethtool", "--json", "-a", "ens1f0"],
        Out::Fail(1, "ethtool: unrecognized option '--json'".into()),
    );
    let pause = |rx: &str, tx: &str| {
        format!(
            "Pause parameters for ens1f0:\nAutonegotiate:\toff\nRX:\t\t{rx}\nTX:\t\t{tx}\n\
             RX negotiated:\toff\nTX negotiated:\toff\n"
        )
    };
    exec.on(&["ethtool", "-a", "ens1f0"], Out::Ok(pause("on", "on")));
    let id = nic_id("flow_control", NIC0);
    assert_commands(
        &hints_json(&host, &link_on(NIC0)),
        &id,
        &["ethtool -A ens1f0 rx off tx off"],
    );
    exec.on(&["ethtool", "-a", "ens1f0"], Out::Ok(pause("off", "off")));
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &id, "ok", "ok");
    assert_no_commands(&h, &id);
}

#[test]
fn flow_control_that_cannot_be_read_suggests_nothing() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    let why = "Cannot get device pause settings: Operation not supported";
    exec.on(
        &["ethtool", "--json", "-a", "ens1f0"],
        Out::Fail(1, why.into()),
    );
    exec.on(&["ethtool", "-a", "ens1f0"], Out::Fail(76, why.into()));
    let h = hints_json(&host, &link_on(NIC0));
    let id = nic_id("flow_control", NIC0);
    assert_level(&h, &id, "ok", "info");
    assert_no_commands(&h, &id);
}

// ---- PCIe MaxReadReq -------------------------------------------------------------------------

#[test]
fn maxreadreq_is_one_hint_per_used_nic_with_current_target_and_the_setpci_command() {
    let host = FakeHost::bare_metal();
    set_mrr(&host, NIC0, 512);
    let before = fs::read(
        host.tmp
            .path()
            .join(format!("sys/bus/pci/devices/{NIC0}/config")),
    )
    .unwrap();
    let h = hints_json(&host, &setup(vec![link(NIC0, NIC1)]));
    let (a, b) = (
        nic_id("pcie_maxreadreq", NIC0),
        nic_id("pcie_maxreadreq", NIC1),
    );
    assert_level(&h, &a, "warn", "warn");
    assert_commands(&h, &a, &["setpci -s 0000:17:00.0 CAP_EXP+8.w=5000:7000"]);
    assert!(
        detail_has(&h, &a, "512") && detail_has(&h, &a, "4096"),
        "{h}"
    );
    assert_level(&h, &b, "ok", "ok");
    assert_no_commands(&h, &b);
    assert!(!has_hint(&h, "pcie_maxreadreq"), "no aggregated hint: {h}");
    // 1024 and 2048 stay acceptable (info) and are still improved by the command
    for bytes in [1024, 2048] {
        set_mrr(&host, NIC0, bytes);
        let h = hints_json(&host, &link_on(NIC0));
        assert_level(&h, &a, "info", "info");
        assert_commands(&h, &a, &["setpci -s 0000:17:00.0 CAP_EXP+8.w=5000:7000"]);
    }
    set_mrr(&host, NIC0, 512);
    assert_eq!(
        fs::read(
            host.tmp
                .path()
                .join(format!("sys/bus/pci/devices/{NIC0}/config"))
        )
        .unwrap(),
        before,
        "the config space is not touched"
    );
}

#[test]
fn maxreadreq_unreadable_config_space_suggests_nothing() {
    let host = FakeHost::bare_metal();
    let a = nic_id("pcie_maxreadreq", NIC0);
    host.remove(&format!("sys/bus/pci/devices/{NIC0}/config"));
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &a, "info", "info");
    assert_no_commands(&h, &a);
    // a non-root read sees only the first 64 bytes
    host.write_bytes(
        &format!("sys/bus/pci/devices/{NIC0}/config"),
        &config_space(512)[..64],
    );
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &a, "info", "info");
    assert_no_commands(&h, &a);
}

#[test]
fn maxreadreq_in_a_vm_says_to_verify_on_the_hypervisor_host_and_still_names_the_command() {
    let host = FakeHost::bare_metal();
    set_mrr(&host, NIC0, 512);
    let a = nic_id("pcie_maxreadreq", NIC0);
    assert!(
        !detail_has(&hints_json(&host, &link_on(NIC0)), &a, "hypervisor"),
        "bare metal needs no such note"
    );
    host.write("sys/hypervisor/type", "xen\n");
    let h = hints_json(&host, &link_on(NIC0));
    assert!(detail_has(&h, &a, "hypervisor"), "{h}");
    assert_commands(&h, &a, &["setpci -s 0000:17:00.0 CAP_EXP+8.w=5000:7000"]);
    // DMI says VM as well
    let host = FakeHost::bare_metal();
    set_mrr(&host, NIC0, 512);
    host.write("sys/class/dmi/id/sys_vendor", "QEMU\n");
    assert!(detail_has(
        &hints_json(&host, &link_on(NIC0)),
        &a,
        "hypervisor"
    ));
}

#[test]
fn maxreadreq_of_a_virtual_function_has_no_command() {
    let host = FakeHost::bare_metal();
    let vf = "0000:17:00.1";
    host.add_nic(&Nic::mlx5(vf, "ens1f0v0", Some(0)));
    set_mrr(&host, vf, 512);
    host.make_vf(vf, NIC0);
    let a = nic_id("pcie_maxreadreq", vf);
    let h = hints_json(&host, &link_on(vf));
    assert!(has_hint(&h, &a), "the hint exists: {:?}", hint_ids(&h));
    assert_no_commands(&h, &a);
    // the physical function next to it still gets its command
    set_mrr(&host, NIC0, 512);
    let h = hints_json(&host, &link_on(NIC0));
    assert_commands(
        &h,
        &nic_id("pcie_maxreadreq", NIC0),
        &["setpci -s 0000:17:00.0 CAP_EXP+8.w=5000:7000"],
    );
}

// ---- NIC port buffer (dcb) -------------------------------------------------------------------

fn port_buffer_id() -> String {
    nic_id("port_buffer", NIC0)
}

#[test]
fn port_buffer_without_dcb_names_iproute2_and_the_install_command() {
    let h = hints_json(&FakeHost::bare_metal(), &link_on(NIC0));
    assert_level(&h, &port_buffer_id(), "info", "info");
    assert_commands(&h, &port_buffer_id(), &["apt install iproute2"]);
    assert!(text(&h, &port_buffer_id()).contains("iproute2"), "{h}");
}

#[test]
fn port_buffer_split_over_buffers_names_the_dcb_command_when_flow_control_is_off() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_ethtool(&exec, "ens1f0", eth(false, false, false));
    let state = fake_dcb(&exec, "ens1f0", dcb_untuned());
    let id = port_buffer_id();
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &id, "info", "info");
    assert_commands(
        &h,
        &id,
        &["dcb buffer set dev ens1f0 buffer-size all:0 0:262016"],
    );
    assert!(
        detail_has(&h, &id, "157696") && detail_has(&h, &id, "262016"),
        "{h}"
    );
    assert!(detail_has(&h, &id, "ConnectX-6 Dx"), "NVIDIA's advice: {h}");
    // the buffer that priority 0 maps to receives the whole total, whichever it is
    *state.lock().unwrap() = DcbState {
        prio: [2; 8],
        sizes: [100_000, 100_000, 62_016, 0, 0, 0, 0, 0],
        total: 262_016,
    };
    assert_commands(
        &hints_json(&host, &link_on(NIC0)),
        &id,
        &["dcb buffer set dev ens1f0 buffer-size all:0 2:262016"],
    );
    // tuned: priority 0's buffer holds the whole total
    *state.lock().unwrap() = DcbState {
        prio: [3; 8],
        sizes: [0, 0, 0, 262_016, 0, 0, 0, 0],
        total: 262_016,
    };
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &id, "ok", "ok");
    assert_no_commands(&h, &id);
    // the other buffers are empty but priority 0's buffer holds less than the whole total
    *state.lock().unwrap() = DcbState {
        prio: [0; 8],
        sizes: [200_000, 0, 0, 0, 0, 0, 0, 0],
        total: 262_016,
    };
    assert_commands(
        &hints_json(&host, &link_on(NIC0)),
        &id,
        &["dcb buffer set dev ens1f0 buffer-size all:0 0:262016"],
    );
    exec.assert_read_only();
}

#[test]
fn port_buffer_with_priorities_split_over_buffers_suggests_nothing_and_explains() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_ethtool(&exec, "ens1f0", eth(false, false, false));
    fake_dcb(
        &exec,
        "ens1f0",
        DcbState {
            prio: [0, 0, 0, 0, 1, 1, 1, 1],
            ..dcb_untuned()
        },
    );
    let h = hints_json(&host, &link_on(NIC0));
    let id = port_buffer_id();
    assert_level(&h, &id, "info", "info");
    assert_no_commands(&h, &id);
    assert!(detail_has(&h, &id, "priorit"), "{h}");
}

#[test]
fn port_buffer_waits_for_flow_control_to_be_off() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    let eth_state = fake_ethtool(&exec, "ens1f0", eth(false, true, true));
    fake_dcb(&exec, "ens1f0", dcb_untuned());
    let id = port_buffer_id();
    let h = hints_json(&host, &link_on(NIC0));
    assert_no_commands(&h, &id);
    assert!(detail_has(&h, &id, "flow control"), "{h}");
    // the administrator runs the flow-control command, then the port buffer follows
    eth_state.lock().unwrap().rx = false;
    assert_no_commands(&hints_json(&host, &link_on(NIC0)), &id); // tx is still on
    eth_state.lock().unwrap().tx = false;
    assert_commands(
        &hints_json(&host, &link_on(NIC0)),
        &id,
        &["dcb buffer set dev ens1f0 buffer-size all:0 0:262016"],
    );
}

#[test]
fn port_buffer_and_firmware_hints_exist_for_mellanox_mlx5_nics_only() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    for dev in ["ens1f0", "enp179s0"] {
        fake_ethtool(&exec, dev, eth(false, false, false));
        fake_dcb(&exec, dev, dcb_untuned());
    }
    let h = hints_json(&host, &link_on(INTEL));
    assert!(has_hint(&h, &nic_id("flow_control", INTEL)));
    assert!(has_hint(&h, &nic_id("pcie_maxreadreq", INTEL)));
    assert!(!has_hint(&h, &nic_id("port_buffer", INTEL)), "{h}");
    assert!(!has_hint(&h, &nic_id("firmware", INTEL)), "{h}");
    let h = hints_json(&host, &link_on(NIC0));
    assert!(has_hint(&h, &nic_id("port_buffer", NIC0)));
    assert!(has_hint(&h, &nic_id("firmware", NIC0)));
}

// ---- CPU governor ----------------------------------------------------------------------------

fn gov_cmd(cpu: u32) -> String {
    format!("echo performance > /sys/devices/system/cpu/cpu{cpu}/cpufreq/scaling_governor")
}

fn gov_path(cpu: u32) -> String {
    format!("sys/devices/system/cpu/cpu{cpu}/cpufreq/scaling_governor")
}

#[test]
fn governor_not_performance_names_the_command_where_performance_is_available() {
    let host = FakeHost::bare_metal(); // isolated 2-3,6-7
    host.governor_all(8, "powersave");
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "governor", "warn", "warn");
    assert_commands(
        &h,
        "governor",
        &[&gov_cmd(2), &gov_cmd(3), &gov_cmd(6), &gov_cmd(7)],
    );
    for c in [0, 1, 4, 5] {
        assert_eq!(host.read(&gov_path(c)), "powersave", "nothing is written");
    }
    // performance is not offered by any core: nothing to suggest
    host.governors_available(8, "powersave");
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "governor", "warn", "warn");
    assert_no_commands(&h, "governor");
    // no cpufreq at all (a VM): info and no command
    let vm = hints_json(&FakeHost::vm(), &Setup::default());
    assert_level(&vm, "governor", "info", "info");
    assert_no_commands(&vm, "governor");
    // already performance: ok
    let ok = hints_json(&FakeHost::bare_metal(), &Setup::default());
    assert_level(&ok, "governor", "ok", "ok");
    assert_no_commands(&ok, "governor");
}

#[test]
fn the_governor_commands_cover_the_isolated_cores_that_offer_performance() {
    let host = FakeHost::bare_metal(); // isolated 2-3,6-7
    host.governor_all(8, "powersave");
    host.write(
        "sys/devices/system/cpu/cpu3/cpufreq/scaling_available_governors",
        "powersave\n",
    );
    // the worker runs on the isolated core 6; core 3 offers no performance governor
    let h = hints_json(&host, &setup(vec![link(NIC1, NIC1)]));
    assert_commands(&h, "governor", &[&gov_cmd(2), &gov_cmd(6), &gov_cmd(7)]);
    assert_eq!(host.read(&gov_path(4)), "powersave");
}

// ---- hugepages -------------------------------------------------------------------------------

fn need_bytes() -> u64 {
    (100_000 + 4096 * 2 + 768 + 64) * NEED_PER_MBUF // the default link of `link()`
}

fn node0_nr() -> &'static str {
    "/sys/devices/system/node/node0/hugepages/hugepages-2048kB/nr_hugepages"
}

const GLOBAL_NR: &str = "/sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages";

/// NUMA node 0 only holds hugepages (`total`/`free` pages of 2 MiB); the NIC `NIC0` sits there.
fn node0_host(total: u64, free: u64) -> FakeHost {
    let host = FakeHost::bare_metal();
    host.remove("sys/devices/system/node/node1/hugepages");
    host.hugepages(&[0], total, free);
    host
}

#[test]
fn hugepages_below_the_need_name_the_command_for_the_node_of_the_nic() {
    let host = node0_host(40, 40);
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "hugepages", "error", "error");
    let cmds = commands_of(&h, "hugepages");
    assert_eq!(
        cmds.len(),
        1,
        "one command, for the node of the NIC: {cmds:?}"
    );
    let (n, path) = echo_command(&cmds[0]);
    assert_eq!(path, node0_nr());
    let missing = (need_bytes() - 40 * PAGE).div_ceil(PAGE);
    let most = (need_bytes() * 3 / 2 - 40 * PAGE).div_ceil(PAGE);
    assert!(
        (40 + missing..=40 + most).contains(&n),
        "the new total grows by the missing pages ({missing}), got {n}"
    );
    assert!(
        text(&h, "hugepages").contains("default_hugepagesz=1g"),
        "the boot-time reservation is named as the reliable way: {h}"
    );
    assert!(
        !detail_has(&h, "hugepages", "memavailable"),
        "no cap was needed: {h}"
    );
    // the need depends on the links: the host-only hint cannot suggest anything
    let h = hints_json(&FakeHost::bare(), &Setup::default());
    assert_level(&h, "hugepages", "warn", "error");
    assert_no_commands(&h, "hugepages");
}

#[test]
fn hugepages_that_cover_the_need_suggest_nothing_even_with_little_headroom() {
    let pages = (need_bytes() as f64 * 1.2 / PAGE as f64).ceil() as u64;
    let h = hints_json(&node0_host(pages, pages), &link_on(NIC0));
    assert_level(&h, "hugepages", "warn", "warn");
    assert_no_commands(&h, "hugepages");
}

#[test]
fn hugepages_without_node_information_use_the_global_file() {
    let host = FakeHost::vm();
    host.hugepages(&[], 40, 40);
    let h = hints_json(&host, &link_on("0000:00:05.0"));
    let cmds = commands_of(&h, "hugepages");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    let (n, path) = echo_command(&cmds[0]);
    assert_eq!(path, GLOBAL_NR);
    assert!(n >= 100, "{cmds:?}");
}

#[test]
fn hugepages_use_the_default_hugepage_size() {
    let host = FakeHost::vm();
    host.remove("sys/kernel/mm/hugepages/hugepages-2048kB");
    let dir = "sys/kernel/mm/hugepages/hugepages-1048576kB";
    host.write(&format!("{dir}/nr_hugepages"), "0\n");
    host.write(&format!("{dir}/free_hugepages"), "0\n");
    host.write(
        "proc/meminfo",
        "MemTotal: 16000000 kB\nMemAvailable: 12000000 kB\nHugePages_Total: 0\n\
         HugePages_Free: 0\nHugepagesize: 1048576 kB\n",
    );
    let h = hints_json(&host, &link_on("0000:00:05.0"));
    assert_commands(
        &h,
        "hugepages",
        &["echo 1 > /sys/kernel/mm/hugepages/hugepages-1048576kB/nr_hugepages"],
    );
}

#[test]
fn hugepages_from_zero_get_a_command_too() {
    let host = FakeHost::bare(); // no hugepages configured, NIC0 on node 0
    let h = hints_json(&host, &link_on(NIC0));
    let cmds = commands_of(&h, "hugepages");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    let (n, path) = echo_command(&cmds[0]);
    assert_eq!(path, node0_nr());
    assert!(n >= 100, "{cmds:?}");
}

#[test]
fn hugepages_commands_are_capped_at_half_of_memavailable() {
    // 400 MiB available: half is 100 pages of 2 MiB, which just covers the need (100 pages); a
    // target above the need is cut to the cap
    let host = FakeHost::bare();
    host.mem_available_kb(Some(409_600));
    let h = hints_json(&host, &link_on(NIC0));
    let cmds = commands_of(&h, "hugepages");
    assert_eq!(cmds, [format!("echo 100 > {}", node0_nr())], "{h}");
}

#[test]
fn a_cap_below_the_need_is_named_in_the_detail() {
    // 200 MiB available: half is 50 pages, the need is 100
    let host = FakeHost::bare();
    host.mem_available_kb(Some(204_800));
    let h = hints_json(&host, &link_on(NIC0));
    assert!(detail_has(&h, "hugepages", "memavailable"), "{h}");
    for c in commands_of(&h, "hugepages") {
        let (n, _) = echo_command(&c);
        assert!(n <= 50, "at most half of MemAvailable (50 pages): {c}");
    }
}

#[test]
fn no_room_for_even_one_page_means_no_command_and_the_detail_says_why() {
    // 2 MiB available: half is less than one page
    let host = FakeHost::bare();
    host.mem_available_kb(Some(2048));
    let h = hints_json(&host, &link_on(NIC0));
    assert_no_commands(&h, "hugepages");
    assert!(detail_has(&h, "hugepages", "memavailable"), "{h}");
}

#[test]
fn the_cap_counts_the_pages_that_would_be_added_and_never_lowers_the_total() {
    // 40 pages exist; 120 MiB available: at most 30 more pages
    let host = node0_host(40, 40);
    host.mem_available_kb(Some(122_880));
    let h = hints_json(&host, &link_on(NIC0));
    assert!(detail_has(&h, "hugepages", "memavailable"), "{h}");
    for c in commands_of(&h, "hugepages") {
        let (n, path) = echo_command(&c);
        assert_eq!(path, node0_nr());
        assert!((41..=70).contains(&n), "40 now, 30 more at most: {c}");
    }
}

#[test]
fn without_a_memavailable_line_there_is_no_hugepage_command() {
    let host = FakeHost::bare();
    host.mem_available_kb(None);
    let h = hints_json(&host, &link_on(NIC0));
    assert_no_commands(&h, "hugepages");
}

// ---- NIC firmware ----------------------------------------------------------------------------

const BDF0: &str = "0000:17:00.0";

fn fw_id() -> String {
    nic_id("firmware", NIC0)
}

fn fw_set(tool: &str, bdf: &str) -> [String; 2] {
    [
        format!("{tool} -d {bdf} -y set ADVANCED_PCI_SETTINGS=1"),
        format!("{tool} -d {bdf} -y set MAX_ACC_OUT_READ=256 CQE_COMPRESSION=1"),
    ]
}

#[test]
fn firmware_without_mstconfig_names_mstflint_and_the_install_command() {
    let h = hints_json(&FakeHost::bare_metal(), &link_on(NIC0));
    assert_level(&h, &fw_id(), "info", "info");
    assert_commands(&h, &fw_id(), &["apt install mstflint"]);
    assert!(text(&h, &fw_id()).contains("mstflint"), "{h}");
}

#[test]
fn firmware_untuned_names_the_two_set_commands_and_the_previous_next_boot_values() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    let h = hints_json(&host, &link_on(NIC0));
    let id = fw_id();
    assert_level(&h, &id, "info", "info");
    let [first, second] = fw_set("mstconfig", BDF0);
    assert_commands(&h, &id, &[&first, &second]);
    let detail = hint(&h, &id)["detail"].as_str().unwrap().to_lowercase();
    for word in ["nvram", "persist", "reset"] {
        assert!(detail.contains(word), "{word} in the detail: {detail}");
    }
    assert!(
        detail.contains("whole card") || detail.contains("entire card"),
        "the change covers the whole card: {detail}"
    );
    let t = text(&h, &id);
    assert!(
        t.contains("max_acc_out_read") && t.contains("cqe_compression"),
        "{t}"
    );
    assert!(
        !detail.contains("hypervisor"),
        "bare metal needs no hypervisor note: {detail}"
    );
    exec.assert_read_only();
}

#[test]
fn firmware_in_a_vm_says_to_reset_it_from_the_hypervisor_host() {
    let host = FakeHost::bare_metal();
    host.write("sys/hypervisor/type", "xen\n");
    let exec = host.exec();
    fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    let h = hints_json(&host, &link_on(NIC0));
    assert!(detail_has(&h, &fw_id(), "hypervisor"), "{h}");
    assert_eq!(commands_of(&h, &fw_id()).len(), 2);
}

#[test]
fn firmware_with_the_target_values_next_boot_is_ok_in_either_value_notation() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(&exec, "mstconfig", BDF0, MstState::tuned()); // "True(1)", 256, "AGGRESSIVE(1)"
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &fw_id(), "ok", "ok");
    assert_no_commands(&h, &fw_id());
}

#[test]
fn firmware_that_hides_max_acc_out_read_while_advanced_settings_are_off_is_untuned() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(
        &exec,
        "mstconfig",
        BDF0,
        MstState {
            hide_max_while_advanced_off: true,
            ..MstState::untuned()
        },
    );
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &fw_id(), "info", "info");
    let [first, second] = fw_set("mstconfig", BDF0);
    assert_commands(&h, &fw_id(), &[&first, &second]);
}

#[test]
fn firmware_pending_a_reset_is_info_without_commands() {
    // the next-boot values equal the targets, the running firmware has not loaded them yet
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(
        &exec,
        "mstconfig",
        BDF0,
        MstState {
            max_cur: 0,
            ..MstState::tuned()
        },
    );
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &fw_id(), "info", "info");
    assert_no_commands(&h, &fw_id());
    assert!(detail_has(&h, &fw_id(), "reset"), "{h}");
    let detail = fw_hint_detail(&h);
    assert!(
        detail.contains("MAX_ACC_OUT_READ=0 (next boot 256)"),
        "{detail}"
    );
    assert!(!detail.contains("clamped"), "{detail}");
}

fn fw_hint_detail(h: &Value) -> String {
    hint(h, &fw_id())["detail"].as_str().unwrap().to_string()
}

#[test]
fn firmware_that_clamped_max_acc_out_read_to_a_nonzero_value_is_ok() {
    // ConnectX-5: next boot 256, running 58 (the device maximum), the expected tuned state
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(
        &exec,
        "mstconfig",
        BDF0,
        MstState {
            max_cur: 58,
            ..MstState::tuned()
        },
    );
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &fw_id(), "ok", "ok");
    assert_no_commands(&h, &fw_id());
    let detail = fw_hint_detail(&h);
    for word in ["tuned", "58", "maximum"] {
        assert!(detail.contains(word), "{word} in the detail: {detail}");
    }
}

#[test]
fn firmware_with_cqe_compression_not_yet_running_is_info_naming_only_that_parameter() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(
        &exec,
        "mstconfig",
        BDF0,
        MstState {
            cqe_cur: 0,
            ..MstState::tuned()
        },
    );
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &fw_id(), "info", "info");
    assert_no_commands(&h, &fw_id());
    let detail = fw_hint_detail(&h);
    assert!(
        detail.contains("CQE_COMPRESSION=0 (next boot 1)"),
        "{detail}"
    );
    assert!(
        !detail.contains("MAX_ACC_OUT_READ=256 (next boot"),
        "{detail}"
    );
    assert!(
        !detail.contains("ADVANCED_PCI_SETTINGS=1 (next boot"),
        "{detail}"
    );
    assert!(!detail.contains("clamped"), "{detail}");
}

#[test]
fn firmware_with_advanced_settings_not_yet_running_is_info_even_if_max_acc_out_read_runs() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(
        &exec,
        "mstconfig",
        BDF0,
        MstState {
            adv_cur: 0,
            max_cur: 58,
            ..MstState::tuned()
        },
    );
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &fw_id(), "info", "info");
    assert_no_commands(&h, &fw_id());
    let detail = fw_hint_detail(&h);
    assert!(
        detail.contains("ADVANCED_PCI_SETTINGS=0 (next boot 1)"),
        "{detail}"
    );
    assert!(
        !detail.contains("MAX_ACC_OUT_READ=58 (next boot"),
        "{detail}"
    );
}

#[test]
fn firmware_uses_mlxconfig_when_mstconfig_is_missing_and_prefers_mstconfig() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(&exec, "mlxconfig", BDF0, MstState::untuned());
    let h = hints_json(&host, &link_on(NIC0));
    let [first, second] = fw_set("mlxconfig", BDF0);
    assert_commands(&h, &fw_id(), &[&first, &second]);
    assert!(
        exec.calls()
            .iter()
            .any(|c| c[0] == "mlxconfig" && c.contains(&"q".to_string())),
        "{:?}",
        exec.calls()
    );
    // both installed: MFT is only the fallback
    fake_mst(&exec, "mstconfig", BDF0, MstState::tuned());
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, &fw_id(), "ok", "ok");
}

#[test]
fn firmware_queries_and_sets_function_zero_of_the_card_and_leaves_no_temporary_file() {
    let host = FakeHost::bare_metal();
    host.add_nic(&Nic::mlx5("0000:17:00.1", "ens1f1", Some(0)));
    let exec = host.exec();
    fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    let id = nic_id("firmware", "0000:17:00.1");
    let h = hints_json(&host, &link_on("0000:17:00.1"));
    let [first, second] = fw_set("mstconfig", BDF0);
    assert_commands(&h, &id, &[&first, &second]);
    let queries: Vec<Vec<String>> = exec.calls_with("q");
    assert!(!queries.is_empty());
    let mut files: Vec<PathBuf> = vec![];
    for q in &queries {
        assert_eq!(
            q[..5],
            ["mstconfig", "-d", BDF0, "-e", "-j"],
            "the query of the card's function 0: {q:?}"
        );
        assert_eq!(q.len(), 7, "{q:?}");
        assert_eq!(q[6], "q");
        files.push(PathBuf::from(&q[5]));
    }
    for f in files {
        assert!(f.is_absolute(), "{f:?}");
        assert!(!f.exists(), "the temporary output {f:?} is removed");
    }
}

// ---- kernel command line (hints only) --------------------------------------------------------

#[test]
fn rcu_nocb_poll_is_a_bare_word_of_the_command_line() {
    let host = FakeHost::bare_metal(); // has rcu_nocb_poll
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "rcu_nocb_poll", "ok", "ok");
    assert_no_commands(&h, "rcu_nocb_poll");
    // `rcu_nocbs=` is a different parameter
    host.cmdline("ro quiet isolcpus=2-3 nohz_full=2-3 rcu_nocbs=2-3");
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "rcu_nocb_poll", "info", "info");
    assert_no_commands(&h, "rcu_nocb_poll");
    assert!(text(&h, "rcu_nocb_poll").contains("rcu_nocb_poll"), "{h}");
}

#[test]
fn boot_time_1g_hugepages_need_the_size_the_default_size_and_a_count() {
    let host = FakeHost::bare_metal();
    let ok = "ro quiet default_hugepagesz=1G hugepagesz=1G hugepages=16";
    for (cmdline, level) in [
        (ok, "ok"),
        ("ro quiet", "info"),
        ("ro quiet hugepages=1024", "info"),
        ("ro quiet hugepagesz=1G hugepages=16", "info"),
        ("ro quiet default_hugepagesz=1G hugepagesz=1G", "info"),
        ("ro quiet default_hugepagesz=1G hugepages=16", "info"),
    ] {
        host.cmdline(cmdline);
        let h = hints_json(&host, &Setup::default());
        assert_level(&h, "boot_hugepages", level, level);
        assert_no_commands(&h, "boot_hugepages");
    }
    host.cmdline("ro quiet");
    let h = hints_json(&host, &Setup::default());
    assert!(
        text(&h, "boot_hugepages").contains("default_hugepagesz=1g"),
        "the detail names the parameters to add: {h}"
    );
}

#[test]
fn the_isolation_hints_name_the_parameters_and_suggest_no_commands() {
    let host = FakeHost::bare_metal();
    host.cmdline("ro quiet");
    let h = hints_json(&host, &Setup::default());
    for id in ["isolation", "isolated_cores"] {
        assert_no_commands(&h, id);
    }
    assert!(text(&h, "isolation").contains("isolcpus"), "{h}");
    host.cmdline("ro quiet isolcpus=2-3,6-7 nohz_full=2-3,6-7 rcu_nocbs=2-3,6-7");
    assert_level(
        &hints_json(&host, &Setup::default()),
        "isolation",
        "ok",
        "ok",
    );
}

// ---- no commands for what the host uses, VFs or odd names ------------------------------------

#[tokio::test]
async fn a_nic_that_is_not_usable_gets_no_command_and_the_detail_says_why() {
    // INTEL is bound to `ice`: not dpds_usable
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_ethtool(&exec, "enp179s0", eth(false, true, true));
    let a = TestApp::new(host);
    let reason = reason_of(&a, INTEL).await;
    let h = a
        .post("/api/host/hints", link_on(INTEL).body())
        .await
        .json();
    for kind in ["flow_control", "pcie_maxreadreq"] {
        let id = nic_id(kind, INTEL);
        assert_no_commands(&h, &id);
        assert!(
            detail_has(&h, &id, &reason),
            "{id} must give the reason {reason:?}: {h}"
        );
    }
    assert_level(&h, &nic_id("pcie_maxreadreq", INTEL), "warn", "warn");
}

#[tokio::test]
async fn an_uplink_nic_gets_no_command_and_the_detail_says_why() {
    // NIC1 is an mlx5 NIC but the host gave it an address: an uplink. Flow control is on, the
    // MaxReadReq is 512 and the firmware untuned, so each of the three would have commands
    let host = FakeHost::bare_metal();
    host.addr("ens2f0", "192.168.7.7");
    set_mrr(&host, NIC1, 512);
    let exec = host.exec();
    fake_ethtool(&exec, "ens2f0", eth(false, true, true));
    fake_mst(&exec, "mstconfig", NIC1, MstState::untuned());
    fake_ethtool(&exec, "eno1", eth(false, true, true));
    let a = TestApp::new(host);
    let reason = reason_of(&a, NIC1).await;
    assert!(reason.contains("192.168.7.7"), "{reason}");
    let h = a.post("/api/host/hints", link_on(NIC1).body()).await.json();
    assert_level(&h, "uplink", "error", "error");
    for kind in ["flow_control", "pcie_maxreadreq", "firmware"] {
        let id = nic_id(kind, NIC1);
        assert_no_commands(&h, &id);
        assert!(
            detail_has(&h, &id, &reason),
            "{id} must give the reason {reason:?}: {h}"
        );
    }
    // the management NIC (a route on eno1, tg3)
    let reason = reason_of(&a, MGMT).await;
    let h = a.post("/api/host/hints", link_on(MGMT).body()).await.json();
    for kind in ["flow_control", "pcie_maxreadreq"] {
        let id = nic_id(kind, MGMT);
        assert_no_commands(&h, &id);
        assert!(
            detail_has(&h, &id, &reason),
            "{id} must give the reason {reason:?}: {h}"
        );
    }
}

#[tokio::test]
async fn the_port_buffer_of_an_uplink_nic_gets_no_command_either() {
    // flow control is off, so only the uplink keeps the dcb command back
    let host = FakeHost::bare_metal();
    host.addr("ens2f0", "192.168.7.7");
    let exec = host.exec();
    fake_ethtool(&exec, "ens2f0", eth(false, false, false));
    fake_dcb(&exec, "ens2f0", dcb_untuned());
    let a = TestApp::new(host);
    let reason = reason_of(&a, NIC1).await;
    let h = a.post("/api/host/hints", link_on(NIC1).body()).await.json();
    let id = nic_id("port_buffer", NIC1);
    assert!(has_hint(&h, &id), "{:?}", hint_ids(&h));
    assert_no_commands(&h, &id);
    assert!(
        detail_has(&h, &id, &reason),
        "{id} must give the reason {reason:?}: {h}"
    );
    // the same NIC without the address gets its command
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_ethtool(&exec, "ens2f0", eth(false, false, false));
    fake_dcb(&exec, "ens2f0", dcb_untuned());
    assert_commands(
        &hints_json(&host, &link_on(NIC1)),
        &id,
        &["dcb buffer set dev ens2f0 buffer-size all:0 0:262016"],
    );
}

#[test]
fn a_netdev_starting_with_a_dash_gets_no_command_and_no_tool_call() {
    let host = FakeHost::bare_metal();
    let pci = "0000:af:00.0";
    host.add_nic(&Nic::mlx5(pci, "-evil", Some(1)));
    let exec = host.exec();
    fake_ethtool(&exec, "-evil", eth(false, false, false));
    fake_dcb(&exec, "-evil", dcb_untuned());
    fake_mst(&exec, "mstconfig", pci, MstState::untuned());
    set_mrr(&host, pci, 512);
    let h = hints_json(&host, &link_on(pci));
    for id in hint_ids(&h) {
        for c in commands_of(&h, &id) {
            assert!(!c.contains("-evil"), "{id}: {c}");
        }
    }
    assert_no_commands(&h, &nic_id("flow_control", pci));
    if has_hint(&h, &nic_id("port_buffer", pci)) {
        assert_no_commands(&h, &nic_id("port_buffer", pci));
    }
    assert!(
        exec.calls().iter().all(|c| !c.iter().any(|a| a == "-evil")),
        "an option-like netdev is never passed to a tool: {:?}",
        exec.calls()
    );
}

#[test]
fn netdev_and_pci_values_come_from_sysfs_not_from_the_body() {
    let host = FakeHost::bare_metal();
    let pci = "0000:af:00.0";
    host.add_nic(&Nic::mlx5(pci, "ens3f0", Some(1)));
    set_mrr(&host, pci, 512);
    let exec = host.exec();
    fake_ethtool(&exec, "ens3f0", eth(false, true, true));
    fake_mst(&exec, "mstconfig", pci, MstState::untuned());
    // the body spells the address in upper case
    let h = hints_json(&host, &link_on("0000:AF:00.0"));
    assert_commands(
        &h,
        &nic_id("flow_control", pci),
        &["ethtool -A ens3f0 rx off tx off"],
    );
    assert_commands(
        &h,
        &nic_id("pcie_maxreadreq", pci),
        &["setpci -s 0000:af:00.0 CAP_EXP+8.w=5000:7000"],
    );
    let [first, second] = fw_set("mstconfig", pci);
    assert_commands(&h, &nic_id("firmware", pci), &[&first, &second]);
    for id in hint_ids(&h) {
        assert!(!id.contains("AF"), "ids are lower case: {id}");
    }
}

#[test]
fn a_port_that_is_no_nic_of_the_host_has_no_per_nic_hint_and_runs_no_tool() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_ethtool(&exec, "ens1f0", eth(false, true, true));
    let h = hints_json(&host, &link_on("0000:ee:00.0"));
    assert!(
        hint_ids(&h).iter().all(|i| !i.contains(':')),
        "{:?}",
        hint_ids(&h)
    );
    assert!(exec.calls().is_empty(), "{:?}", exec.calls());
}

// ---- the request body is validated like the start body ---------------------------------------

#[tokio::test]
async fn a_hints_body_with_an_invalid_field_is_a_400_naming_it_like_start() {
    let a = TestApp::new(FakeHost::bare_metal());
    let mut bad_cfg = link("net_null0", "net_null1");
    bad_cfg["config"]["delayer"]["std_ns"] = json!(-1.0);
    let mut no_rate = link("net_null0", "net_null1");
    no_rate["config"]["delayer"]
        .as_object_mut()
        .unwrap()
        .remove("initialization_rate_pps");
    let cases: Vec<(&str, Setup, &str)> = vec![
        ("bad port", setup(vec![link("eth0", "net_null0")]), "port"),
        (
            "a port that is shell syntax",
            setup(vec![link("0000:17:00.0;reboot", "net_null0")]),
            "port",
        ),
        ("bad config", setup(vec![bad_cfg]), "std_ns"),
        ("no rate", setup(vec![no_rate]), "initialization_rate_pps"),
    ];
    for (what, s, named) in cases {
        let r = a.post("/api/host/hints", s.body()).await;
        let start = a.start(&s).await;
        assert_eq!(start.status, 400, "start {what}: {}", start.text());
        assert_eq!(r.status, 400, "hints {what}: {}", r.text());
        assert!(r.message().contains(named), "hints {what}: {}", r.text());
    }
    assert!(a.supervisor.plans().is_empty());
    assert!(a.host.exec().calls().is_empty(), "no tool ran");
}

#[tokio::test]
async fn a_valid_body_stays_200_also_where_start_would_answer_409_and_an_empty_one_is_the_get() {
    let a = TestApp::new(FakeHost::bare_metal());
    // an uplink port is a hint, not an error
    let r = a.post("/api/host/hints", link_on(MGMT).body()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_level(&r.json(), "uplink", "error", "error");
    let r = a.post("/api/host/hints", Setup::default().body()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), a.get("/api/host/hints").await.json());
}

// ---- read-only: the timeout, the emulation state, the answers as a whole ---------------------

fn all_tools(host: &FakeHost) {
    let exec = host.exec();
    for dev in ["ens1f0", "ens2f0", "enp179s0"] {
        fake_ethtool(&exec, dev, eth(false, true, true));
        fake_dcb(&exec, dev, dcb_untuned());
    }
    fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    fake_mst(&exec, "mstconfig", NIC1, MstState::untuned());
}

#[test]
fn every_tool_call_passes_a_30_second_timeout_and_is_a_read_only_query() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let h = hints_json(&host, &setup(vec![link(NIC0, NIC1)]));
    let calls = host.exec().calls();
    assert!(
        calls.len() >= 6,
        "ethtool, dcb and mstconfig per NIC: {calls:?}"
    );
    for tool in ["ethtool", "dcb", "mstconfig"] {
        assert!(calls.iter().any(|c| c[0] == tool), "{tool} in {calls:?}");
    }
    let timeouts = host.exec().timeouts();
    assert_eq!(timeouts.len(), calls.len());
    assert!(timeouts.iter().all(|t| *t == T30), "{timeouts:?}");
    host.exec().assert_read_only();
    // the answer still carries the commands
    assert!(!commands_of(&h, &nic_id("flow_control", NIC0)).is_empty());
}

#[test]
fn a_tool_that_times_out_is_an_info_hint_without_commands() {
    for (tool, kind) in [
        ("ethtool", "flow_control"),
        ("dcb", "port_buffer"),
        ("mstconfig", "firmware"),
    ] {
        let host = FakeHost::bare_metal();
        all_tools(&host);
        // flow control is off, so the port buffer is looked at as well
        fake_ethtool(&host.exec(), "ens1f0", eth(false, false, false));
        host.exec().on(&[tool], Out::Timeout);
        let h = hints_json(&host, &link_on(NIC0));
        let id = nic_id(kind, NIC0);
        assert_level(&h, &id, "info", "info");
        assert_no_commands(&h, &id);
        let t = text(&h, &id);
        assert!(
            t.contains("timed out") || t.contains("timeout"),
            "{tool}: {t}"
        );
        assert!(
            !t.contains("apt install"),
            "a tool that timed out is installed: {t}"
        );
    }
}

#[test]
fn a_tool_that_fails_is_an_info_hint_without_commands() {
    for (tool, kind) in [
        ("ethtool", "flow_control"),
        ("dcb", "port_buffer"),
        ("mstconfig", "firmware"),
    ] {
        let host = FakeHost::bare_metal();
        all_tools(&host);
        fake_ethtool(&host.exec(), "ens1f0", eth(false, false, false));
        host.exec()
            .on(&[tool], Out::Fail(1, "boom: no such device".into()));
        let h = hints_json(&host, &link_on(NIC0));
        let id = nic_id(kind, NIC0);
        assert_no_commands(&h, &id);
        assert!(!text(&h, &id).contains("apt install"), "{tool}");
    }
}

#[tokio::test]
async fn no_tool_runs_while_an_emulation_is_starting_running_or_stopping() {
    for state in [State::Starting, State::Running, State::Stopping] {
        let host = FakeHost::bare_metal();
        all_tools(&host);
        set_mrr(&host, NIC0, 512);
        host.sysctl("swappiness", Some("60"));
        let exec = host.exec();
        let a = TestApp::new(host);
        a.supervisor.force_status(status(state));
        let r = a
            .post("/api/host/hints", setup(vec![link(NIC0, NIC1)]).body())
            .await;
        assert_eq!(r.status, 200, "{state:?}: {}", r.text());
        let h = r.json();
        assert!(exec.calls().is_empty(), "{state:?}: {:?}", exec.calls());
        for pci in [NIC0, NIC1] {
            for kind in ["flow_control", "port_buffer", "firmware"] {
                let id = nic_id(kind, pci);
                assert_level(&h, &id, "info", "info");
                assert_no_commands(&h, &id);
                assert!(
                    text(&h, &id).contains("not checked while an emulation is running"),
                    "{state:?} {id}: {h}"
                );
            }
        }
        // what needs no tool is computed as always
        assert_commands(
            &h,
            &nic_id("pcie_maxreadreq", NIC0),
            &["setpci -s 0000:17:00.0 CAP_EXP+8.w=5000:7000"],
        );
        assert_commands(&h, "swappiness", &["sysctl -w vm.swappiness=0"]);
    }
}

#[tokio::test]
async fn while_an_emulation_runs_a_missing_tool_is_not_reported_as_missing() {
    // finding out that a tool is missing means running it
    let host = FakeHost::bare_metal(); // no tool installed
    let exec = host.exec();
    let a = TestApp::new(host);
    a.supervisor.force_status(status(State::Running));
    let h = a.post("/api/host/hints", link_on(NIC0).body()).await.json();
    assert!(exec.calls().is_empty());
    for kind in ["flow_control", "port_buffer", "firmware"] {
        let id = nic_id(kind, NIC0);
        assert_no_commands(&h, &id);
        assert!(
            text(&h, &id).contains("not checked while an emulation is running"),
            "{id}: {h}"
        );
    }
}

#[tokio::test]
async fn the_tools_run_again_once_the_emulation_has_stopped() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let exec = host.exec();
    let a = TestApp::new(host);
    let body = link_on(NIC0).body();
    a.start_ok(vec![link(NIC1, NIC1)]).await; // a real start through the route
    let h = a.post("/api/host/hints", body.clone()).await.json();
    assert!(exec.calls().is_empty(), "{:?}", exec.calls());
    assert_no_commands(&h, &nic_id("flow_control", NIC0));
    for state in [State::Stopped, State::Failed] {
        a.supervisor.force_status(status(state));
        let h = a.post("/api/host/hints", body.clone()).await.json();
        assert_commands(
            &h,
            &nic_id("flow_control", NIC0),
            &["ethtool -A ens1f0 rx off tx off"],
        );
    }
    exec.assert_read_only();
}

#[test]
fn get_has_no_per_nic_hint_and_a_vdev_link_adds_none() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    for h in [
        hints_json(&host, &Setup::default()),
        hints_json(&host, &filler()),
    ] {
        assert!(
            hint_ids(&h).iter().all(|i| !i.contains(':')),
            "per-NIC hint without a NIC: {h}"
        );
    }
    assert!(host.exec().calls().is_empty(), "no tool runs without a NIC");
}

#[test]
fn per_nic_hints_exist_for_the_nics_of_the_links_only_and_ids_and_titles_are_unique() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let h = hints_json(&host, &setup(vec![link(NIC0, NIC1)]));
    let ids = hint_ids(&h);
    for kind in ["flow_control", "pcie_maxreadreq", "port_buffer", "firmware"] {
        for pci in [NIC0, NIC1] {
            assert!(ids.contains(&nic_id(kind, pci)), "{kind}:{pci} in {ids:?}");
        }
        assert!(
            !ids.contains(&nic_id(kind, INTEL)),
            "{kind} for an unused NIC"
        );
        assert!(
            !ids.contains(&nic_id(kind, MGMT)),
            "{kind} for an unused NIC"
        );
    }
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "ids are unique: {ids:?}");
    let mut titles: Vec<String> = h
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["title"].as_str().unwrap().to_string())
        .collect();
    let n = titles.len();
    titles.sort();
    titles.dedup();
    assert_eq!(titles.len(), n, "titles are unique: {titles:?}");
    for id in HINT_IDS {
        let _ = hint(&h, id);
    }
}

#[test]
fn the_hints_that_have_commands_are_exactly_the_ones_that_can_be_fixed() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    host.sysctl("swappiness", Some("60"));
    host.sysctl("zone_reclaim_mode", Some("1"));
    host.governor_all(8, "powersave");
    host.irq_affinity(101, "3");
    set_mrr(&host, NIC0, 512);
    let h = hints_json(&host, &setup(vec![link(NIC0, NIC1)]));
    assert_no_action_members(&h);
    let mut with: Vec<String> = hint_ids(&h)
        .into_iter()
        .filter(|id| !commands_of(&h, id).is_empty())
        .collect();
    with.sort();
    let mut want: Vec<String> = [
        "swappiness",
        "zone_reclaim_mode",
        "irq_affinity",
        "governor",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for pci in [NIC0, NIC1] {
        want.push(nic_id("flow_control", pci));
        want.push(nic_id("firmware", pci));
    }
    want.push(nic_id("pcie_maxreadreq", NIC0)); // NIC1 has 4096 already
    want.sort();
    assert_eq!(
        with, want,
        "the port buffers wait for flow control; everything else is tuned or hint-only"
    );
    // every command is one of the forms of the contract
    for id in &with {
        for c in commands_of(&h, id) {
            assert!(
                [
                    "sysctl -w ",
                    "echo ",
                    "ethtool -A ",
                    "setpci -s ",
                    "dcb buffer set ",
                    "mstconfig ",
                    "mlxconfig ",
                    "apt install "
                ]
                .iter()
                .any(|p| c.starts_with(p)),
                "{id}: {c}"
            );
            assert!(
                !c.contains(host.tmp.path().to_str().unwrap()),
                "a command names the fake root: {c}"
            );
        }
    }
    host.exec().assert_read_only();
}

// ---- no such route -----------------------------------------------------------------------

#[tokio::test]
async fn posting_to_host_actions_is_a_404_whatever_the_id_and_the_body() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    host.sysctl("swappiness", Some("60"));
    let exec = host.exec();
    let a = TestApp::new(host);
    for id in [
        "swappiness",
        "zone_reclaim_mode",
        "irq_affinity",
        "governor",
        "hugepages",
        "flow_control:0000:17:00.0",
        "pcie_maxreadreq:0000:17:00.0",
        "port_buffer:0000:17:00.0",
        "firmware:0000:17:00.0",
        "bogus",
        "",
    ] {
        for body in [link_on(NIC0).body(), Setup::default().body(), json!({})] {
            let r = a
                .post(&format!("/api/host/actions/{id}"), body.clone())
                .await;
            assert_eq!(r.status, 404, "{id:?} {body}: {}", r.text());
            r.message();
        }
    }
    let r = a
        .request_raw(
            "POST",
            "/api/host/actions/swappiness",
            Some("text/plain"),
            b"x".to_vec(),
        )
        .await;
    assert_eq!(r.status, 404, "{}", r.text());
    for m in ["GET", "PUT", "DELETE", "PATCH"] {
        let r = a.request(m, "/api/host/actions/swappiness", None).await;
        assert_eq!(r.status, 404, "{m}: {}", r.text());
    }
    assert_eq!(a.get("/api/host/actions").await.status, 404);
    assert!(exec.calls().is_empty(), "{:?}", exec.calls());
    assert_eq!(a.host.read("proc/sys/vm/swappiness"), "60");
}

#[tokio::test]
async fn openapi_has_no_action_route_and_documents_commands() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    let paths = doc["paths"].as_object().unwrap();
    assert!(
        paths.keys().all(|p| !p.starts_with("/api/host/actions")),
        "{:?}",
        paths.keys().collect::<Vec<_>>()
    );
    assert!(paths.contains_key("/api/host/hints"));
    let schemas = &doc["components"]["schemas"];
    assert!(schemas.get("HintAction").is_none(), "{schemas}");
    let text = doc.to_string();
    assert!(
        !text.contains("host/actions"),
        "a leftover of the action route"
    );
    assert!(!text.contains("HintAction"));
    let hint = &schemas["Hint"];
    assert!(
        hint["properties"].get("action").is_none(),
        "no action member: {hint}"
    );
    let commands = &hint["properties"]["commands"];
    assert!(commands.is_object(), "Hint.commands in {hint}");
    let ty = commands["type"].to_string();
    let schema = commands.to_string();
    assert!(
        ty.contains("array") && schema.contains("string"),
        "commands is an array of strings: {commands}"
    );
    assert!(
        !hint["required"]
            .as_array()
            .is_some_and(|r| r.iter().any(|x| x == "commands")),
        "commands is optional"
    );
}

// ---- the same through the real binary --------------------------------------------------------

fn serve_over(host: &FakeHost, envs: &[(&str, &str)]) -> ControllerProcess {
    let sys = host.sysfs();
    let procfs = host.procfs();
    ControllerProcess::start(
        &[
            "serve",
            "--bind",
            "{addr}",
            "--sysfs-root",
            sys.to_str().unwrap(),
            "--procfs-root",
            procfs.to_str().unwrap(),
        ],
        envs,
    )
}

#[test]
fn the_real_binary_has_no_action_route_and_suggests_the_sysctl() {
    let host = FakeHost::bare_metal();
    host.sysctl("swappiness", Some("60"));
    let c = serve_over(&host, &[]);
    let body = Setup::default().body();
    for id in ["swappiness", "nope"] {
        let (st, _) = http(
            &c.addr,
            "POST",
            &format!("/api/host/actions/{id}"),
            Some(&body),
        )
        .unwrap();
        assert_eq!(st, 404, "{id}");
    }
    assert_eq!(host.read("proc/sys/vm/swappiness"), "60", "nothing changed");
    let (st, v) = http(&c.addr, "GET", "/api/host/hints", None).unwrap();
    assert_eq!(st, 200, "{v}");
    assert_commands(&v, "swappiness", &["sysctl -w vm.swappiness=0"]);
    assert_no_action_members(&v);
}

#[test]
fn the_real_binary_reports_a_missing_tool_by_its_package_and_the_install_command() {
    let host = FakeHost::bare_metal();
    // no ethtool, dcb or mstconfig can run: an empty PATH, so this holds on every machine
    let c = serve_over(&host, &[("PATH", "")]);
    let body = link_on(NIC0).body();
    let (st, h) = http(&c.addr, "POST", "/api/host/hints", Some(&body)).unwrap();
    assert_eq!(st, 200, "{h}");
    for (kind, package) in [
        ("flow_control", "ethtool"),
        ("port_buffer", "iproute2"),
        ("firmware", "mstflint"),
    ] {
        let id = nic_id(kind, NIC0);
        assert_commands(&h, &id, &[&format!("apt install {package}")]);
        assert!(text(&h, &id).contains(package), "{id}: {h}");
    }
}

// ---- SystemExec: the real runner and its timeout ---------------------------------------------

fn sh(script: &str) -> Vec<String> {
    vec![st("sh"), st("-c"), st(script)]
}

#[test]
fn system_exec_returns_the_exit_code_and_both_streams() {
    let out = SystemExec
        .run(&[st("echo"), st("hi")], T30)
        .expect("echo runs");
    assert_eq!((out.code, out.stdout.as_str()), (0, "hi\n"));
    let out = SystemExec
        .run(&sh("echo oops >&2; echo fine; exit 3"), T30)
        .unwrap();
    assert_eq!(out.code, 3);
    assert_eq!(out.stdout, "fine\n");
    assert_eq!(out.stderr, "oops\n");
}

#[test]
fn system_exec_reports_a_missing_tool_as_not_found() {
    let e = SystemExec
        .run(&[st("dpds-no-such-tool-4711")], T30)
        .expect_err("no such tool");
    assert_eq!(e.kind(), ErrorKind::NotFound);
}

#[test]
fn system_exec_reads_output_larger_than_a_pipe_while_it_waits() {
    let t0 = Instant::now();
    let out = SystemExec
        .run(
            &sh("head -c 300000 /dev/zero | tr '\\0' x"),
            Duration::from_secs(20),
        )
        .expect("a large answer is no timeout");
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout.len(), 300_000);
    assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
}

#[test]
fn system_exec_kills_a_tool_that_outlasts_its_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = format!("echo $$ > {}; exec sleep 60", pid_file.display());
    let t0 = Instant::now();
    let e = SystemExec
        .run(&sh(&script), Duration::from_millis(400))
        .expect_err("sleep 60 cannot finish in 400 ms");
    assert_eq!(e.kind(), ErrorKind::TimedOut, "{e}");
    assert!(
        t0.elapsed() < Duration::from_secs(10),
        "returned after {:?}",
        t0.elapsed()
    );
    let pid: u32 = fs::read_to_string(&pid_file)
        .expect("the tool started")
        .trim()
        .parse()
        .unwrap();
    // killed and reaped: no process and no zombie is left behind
    let gone = (0..50).any(|_| {
        let gone = !PathBuf::from(format!("/proc/{pid}")).exists();
        if !gone {
            std::thread::sleep(Duration::from_millis(100));
        }
        gone
    });
    assert!(gone, "process {pid} still exists after the timeout");
}

// ---- netdev names that are unsafe in a shell command -------------------------

/// Names that do not match `^[A-Za-z0-9_.@-]+$` or start with `-`. The kernel refuses some of
/// them (whitespace, `:`), but sysfs of a hostile or odd host may still list them.
const UNSAFE_NETDEVS: [&str; 18] = [
    ";reboot;",
    "$(x)",
    "a\"b",
    "a*b",
    "a\u{1b}[31mb",
    "-evil",
    "a'b",
    "a&b",
    "a>b",
    "a|b",
    "a`b`",
    "a\\b",
    "\u{e4}0",
    "a%b",
    "a=b",
    "a:b",
    "a b",
    "a#b",
];

/// The spellings of `name` that a detail may use: control characters as `\xNN`; other characters
/// as they are or escaped like Rust's `Debug` does.
fn shown(name: &str) -> Vec<String> {
    let with_control_escapes: String = name
        .chars()
        .map(|c| {
            if c.is_control() {
                format!("\\x{:02x}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect();
    if name.chars().any(char::is_control) {
        return vec![with_control_escapes];
    }
    vec![
        with_control_escapes,
        name.chars().flat_map(char::escape_debug).collect(),
    ]
}

#[test]
fn an_unsafe_netdev_name_gets_no_tool_call_and_no_netdev_command() {
    for name in UNSAFE_NETDEVS {
        let pci = "0000:af:00.0";
        let host = FakeHost::bare_metal();
        host.add_nic(&Nic::mlx5(pci, name, Some(1)));
        let exec = host.exec();
        fake_ethtool(&exec, name, eth(false, true, true));
        fake_dcb(&exec, name, dcb_untuned());
        fake_mst(&exec, "mstconfig", pci, MstState::untuned());
        set_mrr(&host, pci, 512);
        let h = hints_json(&host, &link_on(pci));
        let calls = exec.calls();
        assert!(
            calls.iter().all(|c| !c.iter().any(|a| a.contains(name))),
            "{name:?}: the name reached a tool: {calls:?}"
        );
        assert!(
            calls.iter().all(|c| c[0] != "ethtool" && c[0] != "dcb"),
            "{name:?}: no netdev-based tool runs for the NIC: {calls:?}"
        );
        for id in hint_ids(&h) {
            for c in commands_of(&h, &id) {
                assert!(!c.contains(name), "{name:?}: {id}: {c}");
            }
        }
        for kind in ["flow_control", "port_buffer"] {
            let id = nic_id(kind, pci);
            assert_no_commands(&h, &id);
            assert!(
                detail_has(&h, &id, "is not safe to put into a shell command"),
                "{name:?}: {id} says why: {}",
                hint(&h, &id)
            );
            assert!(
                shown(name).iter().any(|s| detail_has(&h, &id, s)),
                "{name:?}: {id} names the interface as {:?}: {}",
                shown(name),
                hint(&h, &id)
            );
            assert!(
                !hint(&h, &id)["detail"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .any(char::is_control),
                "{name:?}: {id}: no raw control character in the detail"
            );
        }
        // what needs the PCI address only stays
        assert_commands(
            &h,
            &nic_id("pcie_maxreadreq", pci),
            &["setpci -s 0000:af:00.0 CAP_EXP+8.w=5000:7000"],
        );
    }
}

#[test]
fn safe_netdev_names_still_get_their_commands() {
    for name in [
        "eth0.100",
        "en-p1",
        "en_p1",
        "EN0",
        "a@b",
        "9a",
        "ens1f0np0",
    ] {
        let pci = "0000:af:00.0";
        let host = FakeHost::bare_metal();
        host.add_nic(&Nic::mlx5(pci, name, Some(1)));
        let exec = host.exec();
        fake_ethtool(&exec, name, eth(false, true, true));
        let h = hints_json(&host, &link_on(pci));
        assert_commands(
            &h,
            &nic_id("flow_control", pci),
            &[&format!("ethtool -A {name} rx off tx off")],
        );
        assert!(
            exec.calls().iter().any(|c| c.iter().any(|a| a == name)),
            "{name}: the tool is asked about it"
        );
    }
}

#[test]
fn check_prints_control_characters_escaped() {
    // a governor name with an escape sequence ends up in the `CPU governor` hint
    let host = FakeHost::bare_metal();
    host.governor_all(8, "powersave");
    host.write(
        "sys/devices/system/cpu/cpu2/cpufreq/scaling_governor",
        "pow\u{1b}[31mer\n",
    );
    let o = controller_cmd()
        .arg("check")
        .arg("--sysfs-root")
        .arg(host.sysfs())
        .arg("--procfs-root")
        .arg(host.procfs())
        .output()
        .expect("cannot run dpds-controller");
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        out.contains("CPU governor") && out.contains("cpu2"),
        "the governor hint names cpu2: {out:?}"
    );
    for c in out.chars().filter(|c| *c != '\n') {
        assert!(!c.is_control(), "raw control character {c:?} in {out:?}");
    }
    assert!(
        out.contains("\\x1b"),
        "the escape is spelled \\x1b: {out:?}"
    );
}

// ---- the real runner -------------------------------------------------------------

/// Whether a process exists and is not a zombie (a process whose parent is gone is only reaped
/// by init, which a container may not do).
fn alive(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => false,
        Ok(stat) => stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| !rest.trim_start().starts_with('Z')),
    }
}

fn gone_within(pid: u32, secs: u64) -> bool {
    (0..secs * 10).any(|_| {
        let gone = !alive(pid);
        if !gone {
            std::thread::sleep(Duration::from_millis(100));
        }
        gone
    })
}

fn pid_in(file: &std::path::Path) -> u32 {
    fs::read_to_string(file)
        .expect("the script wrote its pid")
        .trim()
        .parse()
        .unwrap()
}

// ---- a signal does not cut a tool's output short ----------------------------

extern "C" fn ignore_signal(_: libc::c_int) {}

/// Makes SIGUSR1 interrupt a blocking `read` (a handler without `SA_RESTART`), the way any
/// handler of the host program would.
fn make_sigusr1_interrupt_reads() {
    // SAFETY: a zeroed `sigaction` is a valid starting value; the handler does nothing, and
    // nothing else in this process sends SIGUSR1.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = ignore_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);
        assert_eq!(
            libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut()),
            0
        );
    }
}

/// Sends SIGUSR1 to every thread of this process that is blocked in `read` on the pipe with
/// this link target (`pipe:[<inode>]`); returns how many it hit. Only the readers of that one pipe
/// are signalled, so tests running at the same time are not disturbed.
fn interrupt_readers_of(pipe: &str) -> usize {
    let mut hit = 0;
    for task in fs::read_dir("/proc/self/task").unwrap().flatten() {
        let tid = task.file_name().to_string_lossy().into_owned();
        let Ok(syscall) = fs::read_to_string(format!("/proc/self/task/{tid}/syscall")) else {
            continue;
        };
        let mut words = syscall.split_whitespace();
        if words.next() != Some(&libc::SYS_read.to_string()) {
            continue;
        }
        let Some(fd) = words
            .next()
            .and_then(|w| w.strip_prefix("0x"))
            .and_then(|w| u64::from_str_radix(w, 16).ok())
        else {
            continue;
        };
        let target = fs::read_link(format!("/proc/self/fd/{fd}"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if target != pipe {
            continue;
        }
        let tid: libc::c_long = tid.parse().unwrap();
        // SAFETY: `tgkill` only sends a signal to a thread of this process.
        unsafe {
            libc::syscall(
                libc::SYS_tgkill,
                libc::c_long::from(std::process::id()),
                tid,
                libc::c_long::from(libc::SIGUSR1),
            );
        }
        hit += 1;
    }
    hit
}

#[test]
fn system_exec_reads_all_of_the_output_when_signals_interrupt_its_readers() {
    make_sigusr1_interrupt_reads();
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    // nothing is written for a second, so both readers sit in `read` while the signals come
    let script = format!(
        "echo $$ > {}; sleep 1; echo out; echo err >&2",
        pid_file.display()
    );
    let done = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicUsize::new(0));
    let signaller = std::thread::spawn({
        let (done, hits, pid_file) = (done.clone(), hits.clone(), pid_file.clone());
        move || {
            let t0 = Instant::now();
            while !pid_file.exists() || fs::read_to_string(&pid_file).unwrap_or_default().is_empty()
            {
                assert!(
                    t0.elapsed() < Duration::from_secs(10),
                    "the script never started"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            let pid = pid_in(&pid_file);
            let pipes: Vec<String> = [1, 2]
                .iter()
                .map(|fd| {
                    fs::read_link(format!("/proc/{pid}/fd/{fd}"))
                        .map(|p| p.to_string_lossy().into_owned())
                        .expect("the pipes of the tool")
                })
                .collect();
            while !done.load(Ordering::SeqCst) {
                for pipe in &pipes {
                    hits.fetch_add(interrupt_readers_of(pipe), Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    });
    let out = SystemExec.run(&sh(&script), Duration::from_secs(20));
    done.store(true, Ordering::SeqCst);
    signaller.join().unwrap();
    let out = out.expect("the tool ran");
    assert_eq!(out.code, 0, "{out:?}");
    assert_eq!(out.stdout, "out\n", "stdout is read to its end: {out:?}");
    assert_eq!(out.stderr, "err\n", "stderr is read to its end: {out:?}");
    assert!(
        hits.load(Ordering::SeqCst) >= 4,
        "the test interrupted the readers {} times (it needs to see them blocked in read, \
         through /proc/self/task/<tid>/syscall)",
        hits.load(Ordering::SeqCst)
    );
}

#[test]
fn system_exec_gives_up_on_a_background_process_that_keeps_the_pipes_open() {
    // the shell exits at once; its `sleep` keeps stdout and stderr open for 6 s
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = format!("sleep 6 & echo $! > {}", pid_file.display());
    let t0 = Instant::now();
    let e = SystemExec
        .run(&sh(&script), Duration::from_millis(500))
        .expect_err("the pipes stay open past the deadline");
    assert_eq!(e.kind(), ErrorKind::TimedOut, "{e}");
    assert!(
        t0.elapsed() < Duration::from_secs(3),
        "the call ended with the deadline, not with the grandchild: {:?}",
        t0.elapsed()
    );
    assert!(
        gone_within(pid_in(&pid_file), 5),
        "the background process is killed with the group"
    );
}

#[test]
fn system_exec_kills_the_whole_process_group_on_a_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
    let t0 = Instant::now();
    let e = SystemExec
        .run(&sh(&script), Duration::from_millis(500))
        .expect_err("the shell waits for a 30 s sleep");
    assert_eq!(e.kind(), ErrorKind::TimedOut, "{e}");
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    assert!(
        gone_within(pid_in(&pid_file), 5),
        "the child's child is killed too, not only the child"
    );
}

#[test]
fn system_exec_runs_the_child_in_its_own_process_group() {
    let out = SystemExec
        .run(&sh("echo $$ $(cut -d' ' -f5 /proc/$$/stat)"), T30)
        .expect("the script runs");
    let nums: Vec<i64> = out
        .stdout
        .split_whitespace()
        .map(|w| w.parse().expect("a number"))
        .collect();
    assert_eq!(nums.len(), 2, "pid and process group: {:?}", out.stdout);
    // SAFETY: `getpgrp` has no preconditions and cannot fail.
    let ours = i64::from(unsafe { libc::getpgrp() });
    assert_eq!(nums[1], nums[0], "the child leads its own group");
    assert_ne!(nums[1], ours, "and not the group of the controller");
}

#[test]
fn system_exec_accepts_exactly_4_mib_per_stream_and_fails_above_it() {
    const CAP: usize = 4 * 1024 * 1024;
    let out = SystemExec
        .run(
            &sh(&format!("head -c {CAP} /dev/zero | tr '\\0' x")),
            Duration::from_secs(20),
        )
        .expect("4 MiB is no excess");
    assert_eq!(out.stdout.len(), CAP);
    let out = SystemExec
        .run(
            &sh(&format!("head -c {CAP} /dev/zero | tr '\\0' x >&2")),
            Duration::from_secs(20),
        )
        .expect("4 MiB on stderr is no excess");
    assert_eq!(out.stderr.len(), CAP);
    for to in ["", ">&2"] {
        let e = SystemExec
            .run(
                &sh(&format!("head -c {} /dev/zero | tr '\\0' x {to}", CAP + 1)),
                Duration::from_secs(20),
            )
            .expect_err("4 MiB + 1 byte is above the cap");
        assert_eq!(e.kind(), ErrorKind::InvalidData, "{to:?}: {e}");
    }
}

#[test]
fn system_exec_does_not_stall_on_a_flood_and_fails_with_invalid_data() {
    let t0 = Instant::now();
    let e = SystemExec
        .run(
            &sh("head -c 100000000 /dev/zero | tr '\\0' x"),
            Duration::from_secs(25),
        )
        .expect_err("100 MB is above the cap");
    assert_eq!(e.kind(), ErrorKind::InvalidData, "{e}");
    assert!(
        t0.elapsed() < Duration::from_secs(20),
        "the excess is read and dropped, the tool is not left blocked: {:?}",
        t0.elapsed()
    );
}

// ---- the 409 of the hints post and the address the lookups use -----------------

#[tokio::test]
async fn openapi_documents_the_400_and_409_of_the_hints_post() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    let post = &doc["paths"]["/api/host/hints"]["post"];
    let responses = post["responses"]
        .as_object()
        .unwrap_or_else(|| panic!("no responses for POST /api/host/hints: {post}"));
    for code in ["200", "400", "409"] {
        assert!(responses.contains_key(code), "{code} in {responses:?}");
    }
    assert!(
        responses["409"].to_string().contains("ErrorBody"),
        "the 409 has the error body like start's: {}",
        responses["409"]
    );
}

#[tokio::test]
async fn the_hints_post_answers_409_for_a_reused_port_like_start() {
    let a = TestApp::new(FakeHost::bare_metal());
    all_tools(&a.host);
    let cases = [
        (
            "a reused ingress",
            setup(vec![link(NIC0, NIC1), link(NIC0, INTEL)]),
            NIC0,
        ),
        (
            "a reused egress",
            setup(vec![link(NIC0, NIC1), link(INTEL, NIC1)]),
            NIC1,
        ),
    ];
    for (what, s, named) in cases {
        let hints = a.post("/api/host/hints", s.body()).await;
        let start = a.start(&s).await;
        assert_eq!(start.status, 409, "start {what}: {}", start.text());
        assert_eq!(hints.status, 409, "hints {what}: {}", hints.text());
        assert!(
            hints.message().contains(named),
            "hints {what} names {named}: {}",
            hints.text()
        );
    }
    assert!(a.host.exec().calls().is_empty(), "no tool ran");
}

#[test]
fn an_upper_case_pci_address_in_the_body_gets_the_per_node_hugepage_path() {
    // `bare`: node 0 only, no hugepages; the NIC with hex letters sits on node 0
    let host = FakeHost::bare();
    host.add_nic(&Nic::mlx5("0000:af:00.0", "ens3f0", Some(0)));
    for pci in ["0000:af:00.0", "0000:AF:00.0"] {
        let h = hints_json(&host, &link_on(pci));
        let cmds = commands_of(&h, "hugepages");
        assert_eq!(cmds.len(), 1, "{pci}: {cmds:?}");
        assert_eq!(echo_command(&cmds[0]).1, node0_nr(), "{pci}");
    }
}

#[test]
fn an_upper_case_pci_address_in_the_body_gives_the_same_hints_as_the_lower_case_one() {
    let host = FakeHost::bare_metal();
    let pci = "0000:af:00.0";
    host.add_nic(&Nic::mlx5(pci, "ens3f0", Some(1)));
    set_mrr(&host, pci, 512);
    host.nic_irqs(pci, &[(140, "6")]);
    let exec = host.exec();
    fake_ethtool(&exec, "ens3f0", eth(false, true, true));
    fake_dcb(&exec, "ens3f0", dcb_untuned());
    fake_mst(&exec, "mstconfig", pci, MstState::untuned());
    let lower = hints_json(&host, &link_on(pci));
    let upper = hints_json(&host, &link_on("0000:AF:00.0"));
    assert_eq!(upper, lower);
    assert!(
        !commands_of(&lower, "irq_affinity").is_empty(),
        "the interrupt hint has content to compare: {lower}"
    );
}

// ---- blocked NICs keep the install command ------------------------------------

#[tokio::test]
async fn a_blocked_nic_keeps_the_install_command_of_a_missing_tool() {
    // no tool is installed. NIC1 is an uplink, INTEL is bound to `ice`
    let host = FakeHost::bare_metal();
    host.addr("ens2f0", "192.168.7.7");
    let a = TestApp::new(host);
    let h = a.post("/api/host/hints", link_on(NIC1).body()).await.json();
    for (kind, package) in [
        ("flow_control", "ethtool"),
        ("port_buffer", "iproute2"),
        ("firmware", "mstflint"),
    ] {
        let id = nic_id(kind, NIC1);
        assert_commands(&h, &id, &[&format!("apt install {package}")]);
        assert!(
            !detail_has(&h, &id, "no command is suggested"),
            "nothing was removed, so there is no note: {}",
            hint(&h, &id)
        );
    }
    let h = a
        .post("/api/host/hints", link_on(INTEL).body())
        .await
        .json();
    let id = nic_id("flow_control", INTEL);
    assert_commands(&h, &id, &["apt install ethtool"]);
    assert!(!detail_has(&h, &id, "no command is suggested"), "{h}");
}

#[tokio::test]
async fn the_no_command_note_appears_only_where_tuning_commands_were_removed() {
    // nothing to tune: flow control off, MaxReadReq 4096, firmware tuned
    let host = FakeHost::bare_metal();
    host.addr("ens2f0", "192.168.7.7");
    let exec = host.exec();
    fake_ethtool(&exec, "ens2f0", eth(false, false, false));
    fake_mst(&exec, "mstconfig", NIC1, MstState::tuned());
    let a = TestApp::new(host);
    let h = a.post("/api/host/hints", link_on(NIC1).body()).await.json();
    for kind in ["flow_control", "pcie_maxreadreq", "firmware"] {
        let id = nic_id(kind, NIC1);
        assert_no_commands(&h, &id);
        assert!(
            !detail_has(&h, &id, "no command is suggested"),
            "{id} had no command to remove: {}",
            hint(&h, &id)
        );
    }
    // everything needs tuning: the commands are removed and the note gives the reason
    let host = FakeHost::bare_metal();
    host.addr("ens2f0", "192.168.7.7");
    set_mrr(&host, NIC1, 512);
    let exec = host.exec();
    fake_ethtool(&exec, "ens2f0", eth(false, true, true));
    fake_mst(&exec, "mstconfig", NIC1, MstState::untuned());
    let a = TestApp::new(host);
    let why = reason_of(&a, NIC1).await;
    let h = a.post("/api/host/hints", link_on(NIC1).body()).await.json();
    for kind in ["flow_control", "pcie_maxreadreq", "firmware"] {
        let id = nic_id(kind, NIC1);
        assert_no_commands(&h, &id);
        assert!(
            detail_has(&h, &id, "no command is suggested for this NIC"),
            "{id}: {}",
            hint(&h, &id)
        );
        assert!(detail_has(&h, &id, &why), "{id} gives the reason {why:?}");
    }
}

#[test]
fn the_dcb_install_hint_says_which_iproute2_provides_dcb() {
    let h = hints_json(&FakeHost::bare_metal(), &link_on(NIC0));
    let id = port_buffer_id();
    assert_commands(&h, &id, &["apt install iproute2"]);
    assert!(
        text(&h, &id).contains("iproute2 (5.11 or later provides dcb)"),
        "{}",
        hint(&h, &id)
    );
}

// ---- the time budget and the lifecycle lock ------------------------------------

const USED_UP: &str = "not checked (time budget of 60 s used up)";

/// The hints that need a tool, for the two NICs of the link below.
fn tool_ids() -> Vec<String> {
    [NIC0, NIC1]
        .iter()
        .flat_map(|p| ["flow_control", "port_buffer", "firmware"].map(|k| nic_id(k, p)))
        .collect()
}

fn two_nic_link() -> Setup {
    setup(vec![link(NIC0, NIC1)])
}

fn assert_used_up(h: &Value, id: &str) {
    assert_level(h, id, "info", "info");
    assert_no_commands(h, id);
    let t = text(h, id);
    assert!(t.contains(USED_UP), "{id}: {}", hint(h, id));
    assert!(
        !t.contains("apt install"),
        "{id}: a tool that was not run is not missing"
    );
    assert!(
        !t.contains("emulation"),
        "{id}: the budget is no emulation state: {}",
        hint(h, id)
    );
}

#[test]
fn hung_tools_use_up_a_budget_of_60_seconds_and_the_rest_is_not_checked() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let exec = host.exec();
    for tool in ["ethtool", "dcb", "mstconfig"] {
        exec.takes(&[tool], Duration::from_secs(3600)); // never answers: stopped at its timeout
    }
    let h = hints_json(&host, &two_nic_link());
    let timeouts = exec.timeouts();
    assert!(
        timeouts.iter().all(|t| *t > Duration::ZERO && *t <= T30),
        "no call without time, none above 30 s: {timeouts:?}"
    );
    assert_eq!(
        timeouts.iter().sum::<Duration>(),
        Duration::from_secs(60),
        "the calls share 60 s: {timeouts:?}"
    );
    let ids = tool_ids();
    let used_up: Vec<&String> = ids
        .iter()
        .filter(|i| text(&h, i).contains(USED_UP))
        .collect();
    let timed_out = ids
        .iter()
        .filter(|i| {
            let t = text(&h, i);
            t.contains("timed out") || t.contains("timeout")
        })
        .count();
    assert_eq!(
        timed_out,
        exec.calls().len(),
        "one timed-out hint per call: {h}"
    );
    assert_eq!(
        used_up.len() + timed_out,
        ids.len(),
        "every tool hint either ran or says the budget is used up: {h}"
    );
    assert!(!used_up.is_empty(), "{h}");
    for id in used_up {
        assert_used_up(&h, id);
    }
    exec.assert_read_only();
}

#[test]
fn a_call_gets_the_time_that_is_left_and_a_second_request_starts_a_new_budget() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let exec = host.exec();
    for tool in ["ethtool", "dcb", "mstconfig"] {
        exec.takes(&[tool], Duration::from_secs(20)); // answers, after 20 s
    }
    let want = [T30, T30, Duration::from_secs(20)];
    let h = hints_json(&host, &two_nic_link());
    assert_eq!(exec.timeouts(), want, "30 s, 30 s, then the 20 s left");
    let used_up = tool_ids()
        .into_iter()
        .filter(|i| text(&h, i).contains(USED_UP))
        .collect::<Vec<_>>();
    assert_eq!(used_up.len(), 3, "{h}");
    for id in &used_up {
        assert_used_up(&h, id);
    }
    // the first NIC was read in full, so its hints are real answers
    assert_commands(
        &h,
        &nic_id("flow_control", NIC0),
        &["ethtool -A ens1f0 rx off tx off"],
    );
    // a new request has its own budget
    hints_json(&host, &two_nic_link());
    let timeouts = exec.timeouts();
    assert_eq!(timeouts.len(), 6, "{timeouts:?}");
    assert_eq!(timeouts[3..], want, "the same again with a fresh budget");
}

#[test]
fn fast_tools_never_touch_the_budget() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let h = hints_json(&host, &two_nic_link());
    for id in tool_ids() {
        assert!(!text(&h, &id).contains(USED_UP), "{id}");
    }
    assert!(host.exec().timeouts().iter().all(|t| *t == T30));
}

/// Runs the three tools of NIC0 on the simulated clock: `ethtool` and `dcb` use up all of the
/// 60 s budget but `left`, and `mstconfig` never answers, so it gets `left` as its timeout.
/// Returns the timeout `mstconfig` got and the text of the firmware hint.
fn firmware_timeout_with(left: Duration) -> (Duration, String) {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let exec = host.exec();
    // flow control off, so the port buffer is read as well
    fake_ethtool(&exec, "ens1f0", eth(false, false, false));
    exec.takes(&["ethtool"], T30);
    exec.takes(&["dcb"], Duration::from_secs(30) - left);
    exec.takes(&["mstconfig"], Duration::from_secs(3600));
    let h = hints_json(&host, &link_on(NIC0));
    let timeouts = exec.timeouts();
    assert_eq!(timeouts.len(), 3, "ethtool, dcb, mstconfig: {timeouts:?}");
    let id = nic_id("firmware", NIC0);
    assert_level(&h, &id, "info", "info");
    assert_no_commands(&h, &id);
    (timeouts[2], text(&h, &id))
}

#[test]
fn a_timeout_message_below_a_second_shows_the_milliseconds_not_zero_seconds() {
    for (left, shown) in [
        (Duration::from_millis(900), "0.9 s"),
        (Duration::from_millis(250), "0.25 s"),
        (Duration::from_millis(1), "0.001 s"),
    ] {
        let (timeout, t) = firmware_timeout_with(left);
        assert_eq!(timeout, left, "the last call gets what the budget has left");
        assert!(
            t.contains(&format!("timed out after {shown}")),
            "{left:?} left: {t}"
        );
        assert!(
            !t.contains("after 0 s") && !t.contains("after 0.0 s"),
            "{left:?} left: {t}"
        );
    }
}

#[test]
fn a_timeout_message_of_a_full_call_still_says_30_s() {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    host.exec().takes(&["ethtool"], Duration::from_secs(3600));
    let h = hints_json(&host, &link_on(NIC0));
    let t = text(&h, &nic_id("flow_control", NIC0));
    assert!(t.contains("timed out after 30 s"), "{t}");
}

/// A tool that waits until the test lets it go.
#[derive(Clone)]
struct Gate(Arc<(AtomicUsize, Mutex<bool>, Condvar)>);

impl Gate {
    fn new() -> Gate {
        Gate(Arc::new((
            AtomicUsize::new(0),
            Mutex::new(false),
            Condvar::new(),
        )))
    }

    /// Called by the fake tool: counts itself in and waits (at most 30 s, so a failed test cannot
    /// hang the run).
    fn pass(&self) {
        self.0 .0.fetch_add(1, Ordering::SeqCst);
        let open = self.0 .1.lock().unwrap();
        drop(
            self.0
                 .2
                .wait_timeout_while(open, Duration::from_secs(30), |o| !*o)
                .unwrap(),
        );
    }

    fn entered(&self) -> usize {
        self.0 .0.load(Ordering::SeqCst)
    }

    fn open(&self) {
        *self.0 .1.lock().unwrap() = true;
        self.0 .2.notify_all();
    }
}

/// Opens the gate when the test ends, also by a failed assertion.
struct OpenOnDrop(Gate);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("gave up waiting for {what}");
}

fn start_body() -> Value {
    setup(vec![link(NIC1, NIC1)]).body()
}

/// Starts a hints request for NIC0 whose `ethtool` waits for `gate`.
fn slow_hints_app(gate: &Gate) -> TestApp {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    host.exec().on_fn(&["ethtool", "--json", "-a", "ens1f0"], {
        let gate = gate.clone();
        move |_| {
            gate.pass();
            Out::Ok(
                json!([{"ifname": "ens1f0", "autonegotiate": false, "rx": true, "tx": true}])
                    .to_string(),
            )
        }
    });
    TestApp::new(host)
}

fn hints_request(router: &axum::Router) -> tokio::task::JoinHandle<Reply> {
    let router = router.clone();
    tokio::spawn(async move {
        call(
            &router,
            "POST",
            "/api/host/hints",
            Some(link_on(NIC0).body()),
        )
        .await
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_issued_during_a_slow_query_is_not_delayed_by_it() {
    let gate = Gate::new();
    let _open = OpenOnDrop(gate.clone());
    let a = slow_hints_app(&gate);
    let router = a.router.clone();
    let query = hints_request(&router);
    until("the query to reach the tool", || gate.entered() >= 1).await;
    // the tool is stuck: the start must go through nevertheless
    let s = tokio::time::timeout(
        Duration::from_secs(5),
        call(&router, "POST", "/api/emulation/start", Some(start_body())),
    )
    .await
    .expect("the start does not wait for the query that is running");
    assert_eq!(s.status, 202, "{}", s.text());
    assert_eq!(a.supervisor.starts_entered(), 1);
    assert_eq!(a.supervisor.plans().len(), 1);
    assert!(
        !query.is_finished(),
        "the query is still waiting for its tool"
    );
    // the query read the state before the start and finishes as asked
    gate.open();
    let q = query.await.unwrap();
    assert_eq!(q.status, 200, "{}", q.text());
    assert_commands(
        &q.json(),
        &nic_id("flow_control", NIC0),
        &["ethtool -A ens1f0 rx off tx off"],
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hints_requests_do_not_queue_behind_each_other_and_a_start_behind_none_of_them() {
    let gate = Gate::new();
    let _open = OpenOnDrop(gate.clone());
    let a = slow_hints_app(&gate);
    let router = a.router.clone();
    let queries: Vec<_> = (0..3).map(|_| hints_request(&router)).collect();
    // all three are inside the tool at the same time: no request holds the others up
    until("three queries to be inside the tool", || {
        gate.entered() >= 3
    })
    .await;
    let s = tokio::time::timeout(
        Duration::from_secs(5),
        call(&router, "POST", "/api/emulation/start", Some(start_body())),
    )
    .await
    .expect("the start does not wait for any of the queries");
    assert_eq!(s.status, 202, "{}", s.text());
    gate.open();
    for q in queries {
        let q = q.await.unwrap();
        assert_eq!(q.status, 200, "{}", q.text());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_during_a_slow_query_is_not_delayed_by_it_either() {
    let gate = Gate::new();
    let _open = OpenOnDrop(gate.clone());
    let a = slow_hints_app(&gate);
    // the query reads the state as Stopped; the emulation runs by the time the stop comes
    let router = a.router.clone();
    let query = hints_request(&router);
    until("the query to reach the tool", || gate.entered() >= 1).await;
    a.supervisor.force_status(status(State::Running));
    let s = tokio::time::timeout(
        Duration::from_secs(5),
        call(&router, "POST", "/api/emulation/stop", Some(json!({}))),
    )
    .await
    .expect("the stop does not wait for the query that is running");
    assert_eq!(s.status, 202, "{}", s.text());
    gate.open();
    assert_eq!(query.await.unwrap().status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hints_request_while_a_start_is_in_progress_runs_no_tools() {
    // the emulation is already `Starting` when the request reads the state: no tool runs and
    // the request does not wait for the start to finish
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let exec = host.exec();
    let a = TestApp::new(host);
    a.supervisor.force_status(status(State::Starting));
    let r = tokio::time::timeout(
        Duration::from_secs(5),
        call(
            &a.router,
            "POST",
            "/api/host/hints",
            Some(link_on(NIC0).body()),
        ),
    )
    .await
    .expect("hints answer at once");
    assert_eq!(r.status, 200, "{}", r.text());
    let h = r.json();
    assert!(exec.calls().is_empty(), "{:?}", exec.calls());
    for kind in ["flow_control", "port_buffer", "firmware"] {
        let id = nic_id(kind, NIC0);
        assert_no_commands(&h, &id);
        assert!(
            text(&h, &id).contains("not checked while an emulation is running"),
            "{id}: {}",
            hint(&h, &id)
        );
    }
}

// ---- the firmware query is cached until a restart or a recheck -------------------------

const HINTS: &str = "/api/host/hints";
const NOTE_START: &str = "(firmware values read ";
const NOTE_END: &str = " s ago; Recheck to read them again)";
/// What the `ethtool` call of NIC1 takes in `fw_app`: a request on NIC1 moves the simulated clock by
/// this much without touching NIC0's firmware.
const TICK: Duration = Duration::from_secs(25);

/// The tools of `all_tools`, a handle on the firmware state of NIC0's card, and a router over it.
fn fw_app() -> (TestApp, Arc<FakeExec>, Arc<Mutex<MstState>>) {
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let exec = host.exec();
    let st = fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    exec.takes(&["ethtool", "--json", "-a", "ens2f0"], TICK);
    (TestApp::new(host), exec, st)
}

/// The firmware queries made so far for the device `bdf`, by either tool.
fn fw_queries(exec: &FakeExec, bdf: &str) -> usize {
    exec.calls()
        .iter()
        .filter(|c| {
            matches!(c[0].as_str(), "mstconfig" | "mlxconfig")
                && c.get(2).map(String::as_str) == Some(bdf)
                && c.last().map(String::as_str) == Some("q")
        })
        .count()
}

async fn post_hints(a: &TestApp, query: &str, s: &Setup) -> Reply {
    a.post(&format!("{HINTS}{query}"), s.body()).await
}

async fn hints_at(a: &TestApp, query: &str, s: &Setup) -> Value {
    let r = post_hints(a, query, s).await;
    assert_eq!(r.status, 200, "{query}: {}", r.text());
    r.json()
}

/// A request on NIC1 only: it moves the simulated clock by `TICK`.
async fn tick(a: &TestApp) {
    hints_at(a, "", &link_on(NIC1)).await;
}

fn fw_detail(h: &Value, id: &str) -> String {
    hint(h, id)["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `<N>` of "(firmware values read <N> s ago; Recheck to read them again)", when the detail has it.
fn age_note(detail: &str) -> Option<f64> {
    let rest = &detail[detail.find(NOTE_START)? + NOTE_START.len()..];
    rest[..rest.find(NOTE_END)?].trim().parse().ok()
}

/// The hint says its values were read about `secs` seconds ago (on the fake clock).
fn assert_read_ago(h: &Value, id: &str, secs: f64) {
    let d = fw_detail(h, id);
    let n = age_note(&d).unwrap_or_else(|| panic!("no age note in the detail of {id}: {d}"));
    assert!(
        (n - secs).abs() < 0.6,
        "read {n} s ago, expected {secs} s: {d}"
    );
    assert!(d.trim_end().ends_with(')'), "the note ends the detail: {d}");
}

/// A hint from a fresh read says nothing of a cache.
fn assert_fresh(h: &Value, id: &str) {
    let d = fw_detail(h, id).to_lowercase();
    assert!(
        !d.contains("firmware values read") && !d.contains("recheck"),
        "a fresh read has no cache note: {d}"
    );
}

/// `later` is `first` computed from the cache: the same level, title, suggestion and commands, and
/// the same detail followed by the age note.
fn assert_same_hint(first: &Value, later: &Value, id: &str) {
    let (a, b) = (hint(first, id), hint(later, id));
    for key in ["level", "title", "suggestion", "commands"] {
        assert_eq!(a.get(key), b.get(key), "{key} of {id}");
    }
    let (d1, d2) = (fw_detail(first, id), fw_detail(later, id));
    let rest = d2
        .strip_prefix(d1.trim_end())
        .unwrap_or_else(|| panic!("the detail keeps its text: {d1:?} then {d2:?}"));
    let rest = rest.trim_start_matches(['.', ' ']);
    assert!(
        rest.starts_with(NOTE_START) && rest.ends_with(NOTE_END),
        "the detail ends with the note: {d2:?}"
    );
}

fn fw_untuned_commands() -> Vec<String> {
    fw_set("mstconfig", BDF0).to_vec()
}

#[tokio::test]
async fn a_second_post_for_the_same_nic_runs_no_firmware_query_and_gives_the_same_hint() {
    let (a, exec, _) = fw_app();
    let id = fw_id();
    let s = link_on(NIC0);
    let first = hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 1);
    assert_eq!(commands_of(&first, &id), fw_untuned_commands());
    assert_fresh(&first, &id);
    let ethtool_before = exec.calls_with("ethtool").len();
    let second = hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 1, "{:?}", exec.calls());
    assert_same_hint(&first, &second, &id);
    assert_read_ago(&second, &id, 0.0);
    assert!(
        exec.calls_with("ethtool").len() > ethtool_before,
        "only the firmware query is cached, flow control is read again"
    );
    let third = hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 1);
    assert_same_hint(&first, &third, &id);
}

#[tokio::test]
async fn a_cached_hint_keeps_the_level_and_the_commands_of_every_firmware_state() {
    let states = [
        ("untuned", MstState::untuned(), "info", 2usize),
        ("tuned", MstState::tuned(), "ok", 0),
        (
            "pending a reset",
            MstState {
                max_cur: 0,
                ..MstState::tuned()
            },
            "info",
            0,
        ),
    ];
    for (name, state, level, commands) in states {
        let (a, exec, st) = fw_app();
        *st.lock().unwrap() = state;
        let id = fw_id();
        let first = hints_at(&a, "", &link_on(NIC0)).await;
        let second = hints_at(&a, "", &link_on(NIC0)).await;
        assert_eq!(fw_queries(&exec, BDF0), 1, "{name}");
        assert_level(&second, &id, level, level);
        assert_eq!(commands_of(&second, &id).len(), commands, "{name}");
        assert_same_hint(&first, &second, &id);
    }
    // the virtual machine note is part of the detail the cache note follows
    let (a, _, _) = fw_app();
    a.host.write("sys/hypervisor/type", "xen\n");
    hints_at(&a, "", &link_on(NIC0)).await;
    let h = hints_at(&a, "", &link_on(NIC0)).await;
    assert!(detail_has(&h, &fw_id(), "hypervisor"), "{h}");
    assert_read_ago(&h, &fw_id(), 0.0);
}

#[tokio::test]
async fn a_cached_hint_comes_from_the_cache_even_when_the_budget_is_used_up() {
    let (a, exec, _) = fw_app();
    let id = fw_id();
    // flow control off, so the port buffer is read as well
    fake_ethtool(&exec, "ens1f0", eth(false, false, false));
    let first = hints_at(&a, "", &link_on(NIC0)).await;
    assert_eq!(commands_of(&first, &id), fw_untuned_commands());
    // ethtool and dcb use the whole budget of 60 s before the firmware is looked at
    exec.takes(&["ethtool"], T30);
    exec.takes(&["dcb"], T30);
    let t0 = exec.now();
    let second = hints_at(&a, "", &link_on(NIC0)).await;
    assert_eq!(
        exec.now() - t0,
        Duration::from_secs(60),
        "the other tools used the budget up"
    );
    assert_eq!(fw_queries(&exec, BDF0), 1);
    assert_same_hint(&first, &second, &id);
    assert!(
        !text(&second, &id).contains(USED_UP),
        "a cached result needs no time: {}",
        hint(&second, &id)
    );
}

#[tokio::test]
async fn the_age_note_follows_the_clock_of_the_controller() {
    let (a, exec, _) = fw_app();
    let id = fw_id();
    let s = link_on(NIC0);
    hints_at(&a, "", &s).await;
    tick(&a).await;
    let h = hints_at(&a, "", &s).await;
    assert_read_ago(&h, &id, 25.0);
    tick(&a).await;
    let h = hints_at(&a, "", &s).await;
    assert_read_ago(&h, &id, 50.0);
    assert_eq!(
        fw_queries(&exec, BDF0),
        1,
        "the age does not make the entry expire"
    );
}

// ---- what is not cached -------------------------------------------------------------------------

/// Asks twice while the query fails (`broken` is registered for NIC0's card first), then lets it
/// succeed: the failure is never stored, the success is.
async fn failure_is_not_cached(broken: Out, what: &str) {
    let (a, exec, _) = fw_app();
    let id = fw_id();
    let s = link_on(NIC0);
    exec.on(&["mstconfig", "-d", BDF0], broken);
    for round in 1..=2 {
        let h = hints_at(&a, "", &s).await;
        assert_eq!(fw_queries(&exec, BDF0), round, "{what}: asked again");
        assert_level(&h, &id, "info", "info");
        assert_no_commands(&h, &id);
        assert_fresh(&h, &id);
    }
    // the tool works again
    fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    let h = hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 3, "{what}");
    assert_eq!(commands_of(&h, &id), fw_untuned_commands(), "{what}");
    assert_fresh(&h, &id);
    let h = hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 3, "{what}: the success is cached");
    assert_read_ago(&h, &id, 0.0);
}

#[tokio::test]
async fn a_failing_query_is_not_cached() {
    failure_is_not_cached(Out::Fail(1, "boom".into()), "failure").await;
}

#[tokio::test]
async fn a_timed_out_query_is_not_cached() {
    failure_is_not_cached(Out::Timeout, "timeout").await;
}

#[tokio::test]
async fn an_answer_without_firmware_settings_is_not_cached() {
    failure_is_not_cached(
        Out::JsonFile(json!({"Device #1": {"tlv_configuration": {}}})),
        "no settings",
    )
    .await;
}

#[tokio::test]
async fn a_missing_tool_is_not_cached() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    for dev in ["ens1f0", "ens2f0"] {
        fake_ethtool(&exec, dev, eth(false, true, true));
    }
    let a = TestApp::new(host);
    let id = fw_id();
    let s = link_on(NIC0);
    for _ in 0..2 {
        let before = fw_queries(&exec, BDF0);
        let h = hints_at(&a, "", &s).await;
        assert_commands(&h, &id, &["apt install mstflint"]);
        assert!(
            fw_queries(&exec, BDF0) > before,
            "a missing tool is looked for again: {:?}",
            exec.calls()
        );
    }
    // mstflint is installed
    fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    let h = hints_at(&a, "", &s).await;
    assert_eq!(commands_of(&h, &id), fw_untuned_commands());
    let n = fw_queries(&exec, BDF0);
    let h = hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), n);
    assert_read_ago(&h, &id, 0.0);
}

#[tokio::test]
async fn a_result_not_read_because_the_budget_was_used_up_is_not_cached() {
    let (a, exec, _) = fw_app();
    let id = fw_id();
    fake_ethtool(&exec, "ens1f0", eth(false, false, false));
    exec.takes(&["ethtool"], T30);
    exec.takes(&["dcb"], T30);
    let s = link_on(NIC0);
    let h = hints_at(&a, "", &s).await;
    assert_used_up(&h, &id);
    assert_eq!(fw_queries(&exec, BDF0), 0);
    exec.takes(&["ethtool"], Duration::ZERO);
    exec.takes(&["dcb"], Duration::ZERO);
    let h = hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 1, "the next request reads it");
    assert_eq!(commands_of(&h, &id), fw_untuned_commands());
    hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 1);
}

#[tokio::test]
async fn the_cache_remembers_which_tool_answered() {
    // only mlxconfig is installed: the cached hint still names it and probes nothing
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(&exec, "mlxconfig", BDF0, MstState::untuned());
    let a = TestApp::new(host);
    let s = link_on(NIC0);
    let first = hints_at(&a, "", &s).await;
    let want: Vec<String> = fw_set("mlxconfig", BDF0).to_vec();
    assert_eq!(commands_of(&first, &fw_id()), want);
    let calls = exec.calls().len();
    let again = hints_at(&a, "", &s).await;
    assert_eq!(commands_of(&again, &fw_id()), want);
    assert!(
        exec.calls()[calls..]
            .iter()
            .all(|c| c[0] != "mstconfig" && c[0] != "mlxconfig"),
        "no firmware tool is run for a cached result: {:?}",
        &exec.calls()[calls..]
    );
}

#[tokio::test]
async fn two_routers_do_not_share_a_cache() {
    // the cache belongs to one controller (router), it is no process-wide static
    let host = FakeHost::bare_metal();
    all_tools(&host);
    let exec = host.exec();
    let (r1, r2) = (router_over(host.roots()), router_over(host.roots()));
    let body = link_on(NIC0).body();
    for router in [&r1, &r2, &r1, &r2] {
        let r = call(router, "POST", HINTS, Some(body.clone())).await;
        assert_eq!(r.status, 200, "{}", r.text());
    }
    assert_eq!(fw_queries(&exec, BDF0), 2, "one query per router");
}

// ---- recheck ------------------------------------------------------------------------------------

#[tokio::test]
async fn recheck_true_queries_again_and_refreshes_the_cache() {
    let (a, exec, st) = fw_app();
    let id = fw_id();
    let s = link_on(NIC0);
    hints_at(&a, "", &s).await;
    tick(&a).await;
    assert_read_ago(&hints_at(&a, "", &s).await, &id, 25.0);
    assert_eq!(fw_queries(&exec, BDF0), 1);
    // an administrator tuned the card and reset it
    *st.lock().unwrap() = MstState::tuned();
    let h = hints_at(&a, "?recheck=true", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 2);
    assert_level(&h, &id, "ok", "ok");
    assert_no_commands(&h, &id);
    assert_fresh(&h, &id);
    // the new result is the cached one, read just now
    let h = hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 2);
    assert_level(&h, &id, "ok", "ok");
    assert_read_ago(&h, &id, 0.0);
    // and it can be refreshed again
    hints_at(&a, "?recheck=true", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 3);
}

#[tokio::test]
async fn recheck_true_on_an_empty_cache_is_a_plain_read() {
    let (a, exec, _) = fw_app();
    let h = hints_at(&a, "?recheck=true", &link_on(NIC0)).await;
    assert_eq!(fw_queries(&exec, BDF0), 1);
    assert_eq!(commands_of(&h, &fw_id()), fw_untuned_commands());
    assert_fresh(&h, &fw_id());
}

#[tokio::test]
async fn a_recheck_that_fails_drops_the_old_result() {
    let (a, exec, _) = fw_app();
    let id = fw_id();
    let s = link_on(NIC0);
    hints_at(&a, "", &s).await;
    exec.on(&["mstconfig", "-d", BDF0], Out::Fail(1, "boom".into()));
    let h = hints_at(&a, "?recheck=true", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 2);
    assert_level(&h, &id, "info", "info");
    assert_no_commands(&h, &id);
    assert_fresh(&h, &id);
    // the stale value is gone: the next plain request asks again
    hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 3, "{:?}", exec.calls());
}

#[tokio::test]
async fn recheck_false_behaves_like_no_parameter() {
    let (a, exec, _) = fw_app();
    let id = fw_id();
    let s = link_on(NIC0);
    // on an empty cache it reads, like any request
    let first = hints_at(&a, "?recheck=false", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 1);
    assert_fresh(&first, &id);
    // with a cached result it uses it
    let second = hints_at(&a, "?recheck=false", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 1);
    assert_same_hint(&first, &second, &id);
    assert_read_ago(&second, &id, 0.0);
}

#[tokio::test]
async fn a_recheck_drops_only_the_nics_of_the_posted_links() {
    let (a, exec, _) = fw_app();
    let both = setup(vec![link(NIC0, NIC1)]);
    hints_at(&a, "", &both).await;
    assert_eq!(fw_queries(&exec, BDF0), 1);
    assert_eq!(fw_queries(&exec, NIC1), 1);
    hints_at(&a, "?recheck=true", &link_on(NIC0)).await;
    assert_eq!(fw_queries(&exec, BDF0), 2);
    assert_eq!(
        fw_queries(&exec, NIC1),
        1,
        "NIC1 was not part of the request"
    );
    let h = hints_at(&a, "", &both).await;
    assert_eq!(fw_queries(&exec, BDF0), 2);
    assert_eq!(fw_queries(&exec, NIC1), 1);
    assert!(
        age_note(&fw_detail(&h, &nic_id("firmware", NIC1))).is_some(),
        "NIC1's hint comes from the cache: {h}"
    );
    // both NICs of the posted link are read again
    hints_at(&a, "?recheck=true", &both).await;
    assert_eq!(fw_queries(&exec, BDF0), 3);
    assert_eq!(fw_queries(&exec, NIC1), 2);
    // a post without links names no NIC, so it drops nothing
    hints_at(&a, "?recheck=true", &Setup::default()).await;
    hints_at(&a, "", &both).await;
    assert_eq!(fw_queries(&exec, BDF0), 3);
    assert_eq!(fw_queries(&exec, NIC1), 2);
}

#[tokio::test]
async fn bad_recheck_values_are_a_400_naming_recheck_before_anything_runs() {
    let (a, exec, _) = fw_app();
    let s = link_on(NIC0);
    hints_at(&a, "", &s).await; // a cached result that no bad request may drop
    let calls = exec.calls().len();
    let bad = [
        "?recheck=1",
        "?recheck=0",
        "?recheck=2",
        "?recheck=yes",
        "?recheck=no",
        "?recheck=True",
        "?recheck=TRUE",
        "?recheck=False",
        "?recheck=truee",
        "?recheck=false1",
        "?recheck=",
        "?recheck",
        "?recheck=true&recheck=true",
        "?recheck=false&recheck=false",
        "?recheck=true&recheck=false",
        "?recheck=false&recheck=true",
        "?recheck=true&x=1",
        "?x=1&recheck=true",
        "?x=1",
        "?x",
        "?Recheck=true",
        "?since_ms=5",
    ];
    for q in bad {
        let r = post_hints(&a, q, &s).await;
        assert_eq!(r.status, 400, "{q}: {}", r.text());
        assert!(r.message().contains("recheck"), "{q}: {}", r.message());
    }
    assert_eq!(exec.calls().len(), calls, "{:?}", &exec.calls()[calls..]);
    hints_at(&a, "", &s).await;
    assert_eq!(fw_queries(&exec, BDF0), 1, "the cache survived the 400s");
}

// ---- while an emulation starts, runs or stops ------------------------------------------------

#[tokio::test]
async fn while_an_emulation_is_busy_a_cached_result_is_used_and_no_tool_runs() {
    for state in [State::Starting, State::Running, State::Stopping] {
        let (a, exec, _) = fw_app();
        let (id0, id1) = (fw_id(), nic_id("firmware", NIC1));
        let s = setup(vec![link(NIC0, NIC1)]);
        let first = hints_at(&a, "", &link_on(NIC0)).await; // only NIC0 is cached
        a.supervisor.force_status(status(state));
        let calls = exec.calls().len();
        for query in ["", "?recheck=true", "?recheck=false"] {
            let h = hints_at(&a, query, &s).await;
            assert_eq!(
                exec.calls().len(),
                calls,
                "{state:?} {query}: {:?}",
                &exec.calls()[calls..]
            );
            // the cached NIC: its hint as it was read, with the note
            assert_same_hint(&first, &h, &id0);
            assert_read_ago(&h, &id0, 0.0);
            assert!(
                !text(&h, &id0).contains("not checked while an emulation is running"),
                "{state:?} {query}: {}",
                hint(&h, &id0)
            );
            // the other NIC: nothing cached, so still not checked
            assert_level(&h, &id1, "info", "info");
            assert_no_commands(&h, &id1);
            assert!(
                text(&h, &id1).contains("not checked while an emulation is running"),
                "{state:?} {query}: {}",
                hint(&h, &id1)
            );
            assert!(age_note(&fw_detail(&h, &id1)).is_none(), "{state:?}");
            // what else needs a tool stays not checked, only the firmware is cached
            for kind in ["flow_control", "port_buffer"] {
                assert!(
                    text(&h, &nic_id(kind, NIC0))
                        .contains("not checked while an emulation is running"),
                    "{state:?} {kind}"
                );
            }
        }
        // the recheck during the run did not drop the entry
        a.supervisor.force_status(status(State::Stopped));
        let h = hints_at(&a, "", &link_on(NIC0)).await;
        assert_eq!(fw_queries(&exec, BDF0), 1, "{state:?}");
        assert_same_hint(&first, &h, &id0);
    }
}

#[tokio::test]
async fn a_result_read_before_a_real_start_is_used_during_the_run() {
    let (a, exec, _) = fw_app();
    let s = link_on(NIC0);
    let first = hints_at(&a, "", &s).await;
    a.start_ok(vec![link(NIC1, NIC1)]).await;
    let calls = exec.calls().len();
    let h = hints_at(&a, "?recheck=true", &s).await;
    assert_eq!(exec.calls().len(), calls);
    assert_same_hint(&first, &h, &fw_id());
}

// ---- one card, one entry --------------------------------------------------------------------------

#[tokio::test]
async fn two_functions_of_one_card_share_a_cache_entry() {
    let (a, exec, _) = fw_app();
    const FN1: &str = "0000:17:00.1";
    a.host.add_nic(&Nic::mlx5(FN1, "ens1f1", Some(0)));
    let id1 = nic_id("firmware", FN1);
    let first = hints_at(&a, "", &link_on(NIC0)).await;
    assert_eq!(fw_queries(&exec, BDF0), 1);
    // the other function: the same card, so no query and the commands of function 0
    let h = hints_at(&a, "", &link_on(FN1)).await;
    assert_eq!(fw_queries(&exec, BDF0), 1, "{:?}", exec.calls_with("q"));
    assert_eq!(commands_of(&h, &id1), fw_untuned_commands());
    assert_eq!(hint(&h, &id1)["level"], hint(&first, &fw_id())["level"]);
    assert_read_ago(&h, &id1, 0.0);
    // a recheck through function 1 drops the card's entry
    hints_at(&a, "?recheck=true", &link_on(FN1)).await;
    assert_eq!(fw_queries(&exec, BDF0), 2);
    hints_at(&a, "", &link_on(NIC0)).await;
    assert_eq!(fw_queries(&exec, BDF0), 2);
}

#[tokio::test]
async fn both_functions_of_a_card_in_one_request_make_one_query() {
    let (a, exec, _) = fw_app();
    const FN1: &str = "0000:17:00.1";
    a.host.add_nic(&Nic::mlx5(FN1, "ens1f1", Some(0)));
    let h = hints_at(&a, "", &setup(vec![link(NIC0, FN1)])).await;
    assert_eq!(fw_queries(&exec, BDF0), 1, "{:?}", exec.calls_with("q"));
    let (a0, a1) = (fw_id(), nic_id("firmware", FN1));
    assert_eq!(commands_of(&h, &a0), fw_untuned_commands());
    assert_eq!(commands_of(&h, &a1), fw_untuned_commands());
    assert_eq!(hint(&h, &a0)["level"], hint(&h, &a1)["level"]);
}

#[tokio::test]
async fn cards_do_not_share_entries() {
    let (a, exec, _) = fw_app();
    hints_at(&a, "", &link_on(NIC0)).await;
    let h = hints_at(&a, "", &link_on(NIC1)).await;
    assert_eq!(fw_queries(&exec, BDF0), 1);
    assert_eq!(fw_queries(&exec, NIC1), 1, "NIC1 is another card");
    assert_fresh(&h, &nic_id("firmware", NIC1));
}

// ---- OpenAPI, the CLI, the real binary ---------------------------------------------------------------

#[test]
fn check_has_no_recheck_and_prints_no_cache_note() {
    let host = FakeHost::bare_metal();
    let run = |extra: &[&str]| {
        controller_cmd()
            .arg("check")
            .args(extra)
            .arg("--sysfs-root")
            .arg(host.sysfs())
            .arg("--procfs-root")
            .arg(host.procfs())
            .output()
            .expect("cannot run dpds-controller")
    };
    let o = run(&["--recheck"]);
    assert_eq!(o.status.code(), Some(2), "an unknown flag is a usage error");
    assert!(String::from_utf8_lossy(&o.stderr).contains("--recheck"));
    let o = run(&[]);
    let out = String::from_utf8_lossy(&o.stdout).to_lowercase();
    assert!(out.contains("swappiness"), "the check ran: {out}");
    assert!(
        !out.contains("recheck") && !out.contains("firmware values read"),
        "{out}"
    );
    let help = run(&["--help"]);
    assert!(!String::from_utf8_lossy(&help.stdout)
        .to_lowercase()
        .contains("recheck"));
}

#[test]
fn the_real_binary_caches_the_firmware_query_until_a_recheck() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("runs");
    let answer = r#"{"Device #1":{"tlv_configuration":{"ADVANCED_PCI_SETTINGS":{"next_value":"False(0)","current_value":"False(0)"},"MAX_ACC_OUT_READ":{"next_value":0,"current_value":0},"CQE_COMPRESSION":{"next_value":"BALANCED(0)","current_value":"BALANCED(0)"}}}}"#;
    let script = dir.path().join("mstconfig");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho run >> '{}'\nprintf '%s' '{answer}' > \"$5\"\n",
            counter.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let host = FakeHost::bare_metal();
    let c = serve_over(&host, &[("PATH", dir.path().to_str().unwrap())]);
    let runs = || fs::read_to_string(&counter).map_or(0, |s| s.lines().count());
    let body = link_on(NIC0).body();
    let post = |query: &str| {
        let (st, v) = http(&c.addr, "POST", &format!("{HINTS}{query}"), Some(&body)).unwrap();
        assert_eq!(st, 200, "{v}");
        v
    };
    // the startup task queries the firmware of both NICs of the host once; a GET waits for it
    let (st, v) = http(&c.addr, "GET", HINTS, None).unwrap();
    assert_eq!(st, 200, "{v}");
    let warm = runs();
    assert_eq!(warm, 2, "the startup task queried both NICs");
    let first = post("");
    assert_eq!(runs(), warm, "the first POST reads the warm cache");
    assert_eq!(commands_of(&first, &fw_id()), fw_untuned_commands());
    let second = post("");
    assert_eq!(runs(), warm, "the second request reads the cache");
    assert!(
        age_note(&fw_detail(&second, &fw_id())).is_some(),
        "{second}"
    );
    post("?recheck=true");
    assert_eq!(
        runs(),
        warm + 1,
        "a recheck queries the posted link's NIC again"
    );
    post("");
    assert_eq!(runs(), warm + 1);
    let (st, v) = http(&c.addr, "POST", &format!("{HINTS}?recheck=1"), Some(&body)).unwrap();
    assert_eq!(st, 400, "{v}");
    assert!(v["message"]
        .as_str()
        .unwrap_or_default()
        .contains("recheck"));
    assert_eq!(runs(), warm + 1);
}

// ---- the sources -----------------------------------------------------------------------------

fn controller_sources() -> Vec<(PathBuf, String)> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    fs::read_dir(&src)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| {
            let text = fs::read_to_string(&p).unwrap();
            (p, text)
        })
        .collect()
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// The lines of code of a source: no comment lines (`//`, `///`, `//!`).
fn code_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect()
}

/// The text between a pair of double quotes of a line, in order (a rough reading of the string
/// literals of one line, enough for argv lists and `format!` strings).
fn literals(line: &str) -> Vec<&str> {
    line.split('"').skip(1).step_by(2).collect()
}

#[test]
fn the_sources_never_start_a_shell_and_never_reset_firmware() {
    let mut hits = Vec::new();
    for (p, text) in controller_sources() {
        for needle in [
            "Command::new(\"sh\")",
            "Command::new(\"bash\")",
            "Command::new(\"/bin/sh\")",
            "arg(\"-c\")",
            "mstfwreset",
            "mlxfwreset",
            "mlxfwmanager",
        ] {
            if text.contains(needle) {
                hits.push(format!("{}: {needle}", p.display()));
            }
        }
    }
    assert!(hits.is_empty(), "{hits:#?}");
}

#[test]
fn no_source_executes_a_mutating_argv_commands_are_display_strings_only() {
    // as a whole string literal in the argument list of a call that runs something (`run(..)`,
    // `.arg(..)`, `Command::new(..)`, and the eight lines after it): a tool that changes the host,
    // or the option or subcommand that makes a read-only tool change it. Inside a longer literal
    // (`format!("ethtool -A {nd} rx off tx off")`) or outside such a call it is a display string.
    // The fake runner of the other tests refuses every executed argv that is not a read-only query.
    const ARGV_ELEMENTS: [&str; 20] = [
        "-A",
        "-s",
        "set",
        "--set",
        "apt",
        "apt-get",
        "sysctl",
        "setpci",
        "echo",
        "tee",
        "modprobe",
        "chmod",
        "chown",
        "kill",
        "sh",
        "bash",
        "systemctl",
        "ip",
        "tc",
        "devlink",
    ];
    let mut hits = Vec::new();
    for (p, text) in controller_sources() {
        let code = code_lines(&text);
        for (n, line) in code.iter().enumerate() {
            // a call that runs something: its next lines are the argument list
            if ["run(", ".arg(", ".args(", "Command::new("]
                .iter()
                .any(|c| line.contains(c))
            {
                let window = code[n..code.len().min(n + 9)].join("\n");
                for lit in literals(&window) {
                    if ARGV_ELEMENTS.contains(&lit) {
                        hits.push(format!("{}:{}: {lit:?} near {line}", file_name(&p), n + 1));
                    }
                }
            }
            if line.contains("Command::new(\"") {
                hits.push(format!(
                    "{}:{}: a command named by a literal: {line}",
                    file_name(&p),
                    n + 1
                ));
            }
        }
    }
    assert!(hits.is_empty(), "{hits:#?}");
}

#[test]
fn no_source_writes_a_file_outside_the_supervisors_links_file_and_the_telemetry_socket() {
    // nothing below /proc or /sys is ever written: no file is, except where the controller owns it
    let mut hits = Vec::new();
    for (p, text) in controller_sources() {
        let name = file_name(&p);
        if name == "supervisor.rs" || name == "telemetry.rs" {
            continue;
        }
        for needle in [
            "fs::write",
            "File::create",
            "OpenOptions",
            "write_all",
            ".write(true)",
            "write_at",
            "FileExt",
            "fs::copy",
            "fs::rename",
            "set_permissions",
        ] {
            if code_lines(&text).iter().any(|l| l.contains(needle)) {
                hits.push(format!("{name}: {needle}"));
            }
        }
    }
    assert!(hits.is_empty(), "{hits:#?}");
}

#[test]
fn the_apply_code_of_spec_32_is_gone() {
    let mut hits = Vec::new();
    for (p, text) in controller_sources() {
        for needle in [
            "HintAction",
            "host_action",
            "known_action_id",
            "/api/host/actions",
            "fn execute(",
            "fn write(&self",
            "Exec::write",
            "runtime_confirm",
            "Step::",
            "Plan::",
        ] {
            if code_lines(&text).iter().any(|l| l.contains(needle)) {
                hits.push(format!("{}: {needle}", file_name(&p)));
            }
        }
    }
    assert!(hits.is_empty(), "{hits:#?}");
}

#[test]
fn the_documentation_of_the_sources_no_longer_talks_about_applying_tunings() {
    let mut hits = Vec::new();
    for (p, text) in controller_sources() {
        for l in text.lines().filter(|l| {
            let t = l.trim_start();
            t.starts_with("//!") || t.starts_with("///")
        }) {
            for needle in [
                "host/actions",
                "Apply",
                "applied to the running system",
                "allow-listed action",
            ] {
                if l.contains(needle) {
                    hits.push(format!("{}: {needle}: {l}", file_name(&p)));
                }
            }
        }
    }
    assert!(hits.is_empty(), "{hits:#?}");
}
