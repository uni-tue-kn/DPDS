//! Spec 33: DPDS never changes the host. A host hint that can be fixed suggests the exact root
//! commands instead (`commands`), and an administrator with a shell runs them. This replaces
//! spec 32's Apply (`action`, `POST /api/host/actions/{id}`, `Exec::write`).
//!
//! What stays from spec 32: the checks (is the tool installed, is the tuning in effect, current
//! against target or device maximum), the hint ids, the per-NIC hints, the hint-only kernel
//! command line checks.
//!
//! # Contract the tests assume
//!
//! ## Hints (`Hint`)
//!
//! - `commands`: optional array of strings, omitted (never null, never `[]`) when a hint suggests
//!   nothing. Each element is one trimmed shell line, exactly what an administrator runs as root
//!   on this host, computed from what the controller read: netdev names, PCI addresses, IRQ
//!   numbers, cpulists, page counts. File paths in commands are the real ones (`/proc/...`,
//!   `/sys/...`), never the root a test points the controller at. There is no `action` member.
//! - the commands per check (`<...>` is computed from the host, everything else is literal):
//!   - `swappiness`: `sysctl -w vm.swappiness=0`; `zone_reclaim_mode`:
//!     `sysctl -w vm.zone_reclaim_mode=0`
//!   - `irq_affinity`, one per NIC interrupt on an isolated core (`isolated` and `nohz_full`),
//!     ascending by IRQ: `echo <cpulist> > /proc/irq/<n>/smp_affinity_list`, the list being the
//!     online cores minus the isolated ones (any valid cpulist spelling)
//!   - `governor`, one per core, ascending: `echo performance >
//!     /sys/devices/system/cpu/cpu<n>/cpufreq/scaling_governor` for the workers and the isolated
//!     cores that list `performance`
//!   - `hugepages`: `echo <n> > <path>` with `<path>` = `/sys/devices/system/node/node<k>/
//!     hugepages/hugepages-<size>kB/nr_hugepages` for the node of the NIC when known, else
//!     `/sys/kernel/mm/hugepages/hugepages-<size>kB/nr_hugepages`; `<n>` is the new total, never
//!     below the current one and never adding more than half of `MemAvailable`. When that cap
//!     keeps the commands from the full target the detail names `MemAvailable`; without a
//!     `MemAvailable` line there is no command.
//!   - `flow_control:<pci>`: `ethtool -A <netdev> rx off tx off`, with `autoneg off` as well when
//!     autonegotiation is on (the three pairs in any order)
//!   - `pcie_maxreadreq:<pci>`: `setpci -s <pci> CAP_EXP+8.w=5000:7000`
//!   - `port_buffer:<pci>`: `dcb buffer set dev <netdev> buffer-size all:0 <i>:<total>`
//!   - `firmware:<pci>`: `<tool> -d <function 0> -y set ADVANCED_PCI_SETTINGS=1` and then
//!     `<tool> -d <function 0> -y set MAX_ACC_OUT_READ=256 CQE_COMPRESSION=1`, `<tool>` being
//!     `mstconfig` or, when only that is installed, `mlxconfig`
//!   - a missing tool: `["apt install <package>"]` (`ethtool`, `iproute2` for `dcb`, `mstflint`),
//!     level info, and nothing else
//!   - kernel command line hints (`isolation`, `isolated_cores`, `rcu_nocb_poll`,
//!     `boot_hugepages`): no commands, the detail names the parameters
//! - Conditions for commands are those of spec 32 (dcb only with one shared buffer and flow
//!   control off, no firmware command while a reset is pending, no IRQ command without isolated
//!   cores, ...); the wording notes of spec 32's confirmations are now part of the detail: NVIDIA's
//!   advice against the dcb change on ConnectX-6 Dx, firmware settings are NVRAM of the whole card,
//!   persist and need a reset (from the hypervisor host in a VM), MaxReadReq in a VM.
//! - No commands for a NIC the host uses (`uplink`) or that is not `dpds_usable`: the detail says
//!   why (it contains the interface's `reason`). No MaxReadReq command for a virtual function
//!   (sysfs `physfn`). No command, and no tool call at all, for a netdev whose name starts with
//!   `-`. The netdev and PCI address in a command come from sysfs, never from the request body
//!   (the body's PCI address may differ in case).
//! - Hugepage need and target are those of the hint (spec 32); the cap is half of `MemAvailable`.
//!
//! ## Read-only
//!
//! Hint computation runs only `ethtool --json -a <netdev>` (or the text form `ethtool -a`),
//! `dcb -j buffer show dev <netdev>` and `mstconfig|mlxconfig -d <bdf> -e -j <file> q`. The seam
//! in `host.rs` is
//!
//! ```ignore
//! pub trait Exec: Send + Sync + std::fmt::Debug {
//!     /// Runs `argv[0]` with the other elements as arguments (no shell), waits at most
//!     /// `timeout`, kills the process when that runs out and returns `Err(ErrorKind::TimedOut)`.
//!     /// `Err(ErrorKind::NotFound)` = the tool is not installed (no other probe is made).
//!     fn run(&self, argv: &[String], timeout: Duration) -> io::Result<CommandOutput>;
//! }
//! ```
//!
//! There is no `write`. Every call of hint computation passes `Duration::from_secs(30)`. A tool
//! that times out (or fails any other way) gives an info hint saying so, with no commands. While
//! an emulation is starting, running or stopping no tool runs; the per-NIC hints that need one
//! (`flow_control`, `port_buffer`, `firmware`) are info and say "not checked while an emulation
//! is running". The other hints are computed as always.
//!
//! ## Route and body
//!
//! `POST /api/host/actions/<anything>` is a 404 like every unknown route, and OpenAPI has no such
//! path. The body of `POST /api/host/hints` is validated like the start body: 400 naming the
//! field (the 409 rules of start, for example an uplink port, stay hints).
//!
//! ## Sources
//!
//! No source file executes a mutating argv (`ethtool -A`, `... set`), writes below `/proc` or
//! `/sys` (or writes files at all, apart from the supervisor's links file and the telemetry
//! socket), or has the Apply code of spec 32. Commands appear only as display strings.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::common::*;
use dpds_controller::host::{parse_cpulist, Exec, SystemExec};
use dpds_controller::supervisor::State;
use serde_json::{json, Value};

const NEED_PER_MBUF: u64 = 1920;
const PAGE: u64 = 2 * 1024 * 1024;
const T30: Duration = Duration::from_secs(30);

fn link_on(pci: &str) -> Setup {
    setup(vec![link("l", pci, pci, None)])
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
    host.irq_effective(101, "3");
    let h = hints_json(&host, &Setup::default());
    assert_level(&h, "irq_affinity", "ok", "ok");
    assert_no_commands(&h, "irq_affinity");
}

#[test]
fn a_nic_irq_on_an_isolated_core_warns_and_names_the_commands() {
    let host = FakeHost::bare_metal();
    host.irq_effective(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "warn", "warn");
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (101, vec![0, 1, 4, 5]));
    assert!(detail_has(&h, "irq_affinity", "101"), "names the irq: {h}");
    assert!(!detail_has(&h, "irq_affinity", "irq 100"), "{h}");
}

#[test]
fn the_irq_hint_reads_the_smp_affinity_when_there_is_no_effective_list() {
    let host = FakeHost::bare_metal();
    host.remove("proc/irq/101/effective_affinity_list");
    host.write("proc/irq/101/smp_affinity_list", "3\n");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "warn", "warn");
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]).0, 101);
}

#[test]
fn only_the_irqs_of_the_used_nics_count() {
    let host = FakeHost::bare_metal();
    host.irq_effective(120, "2"); // INTEL, on an isolated core
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "ok", "ok");
    let h = hints_json(&host, &link_on(INTEL));
    assert_level(&h, "irq_affinity", "warn", "warn");
    // the egress NIC counts as well
    let h = hints_json(&host, &setup(vec![link("l", NIC0, INTEL, None)]));
    assert_level(&h, "irq_affinity", "warn", "warn");
}

#[test]
fn without_isolated_cores_there_is_nothing_to_move_away_from() {
    let host = FakeHost::bare_metal();
    host.cpus(8, Some(""), &[(0, "0-3"), (1, "4-7")]);
    host.irq_effective(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "ok", "info");
    assert_no_commands(&h, "irq_affinity");
}

#[test]
fn nohz_full_cores_count_as_isolated_too() {
    let host = FakeHost::bare_metal();
    host.cpus(8, Some(""), &[(0, "0-3"), (1, "4-7")]);
    host.write("sys/devices/system/cpu/nohz_full", "2-3\n");
    host.irq_effective(101, "3");
    let h = hints_json(&host, &link_on(NIC0));
    assert_level(&h, "irq_affinity", "warn", "warn");
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (101, vec![0, 1, 4, 5, 6, 7]));
}

#[test]
fn a_running_irqbalance_is_named_in_the_detail() {
    let host = FakeHost::bare_metal();
    host.irq_effective(101, "3");
    host.processes(&[(812, "irqbalance")]);
    let h = hints_json(&host, &link_on(NIC0));
    assert!(detail_has(&h, "irq_affinity", "irqbalance"), "{h}");
}

#[test]
fn the_irq_commands_move_only_the_irqs_on_isolated_cores_to_the_housekeeping_cores() {
    let host = FakeHost::bare_metal(); // isolated 2-3,6-7 of 8 CPUs
    host.irq_effective(100, "2");
    host.irq_effective(101, "3");
    host.irq_effective(110, "2"); // NIC1: not used by the link
    let h = hints_json(&host, &link_on(NIC0));
    let cmds = commands_of(&h, "irq_affinity");
    let parsed: Vec<(u32, Vec<u32>)> = cmds.iter().map(|c| irq_command(c)).collect();
    assert_eq!(
        parsed,
        [(100, vec![0, 1, 4, 5]), (101, vec![0, 1, 4, 5])],
        "the NIC's IRQs on isolated cores, ascending, no others: {cmds:?}"
    );
    for c in &cmds {
        assert!(
            c.ends_with(" > /proc/irq/100/smp_affinity_list")
                || c.ends_with(" > /proc/irq/101/smp_affinity_list"),
            "the real /proc path, not the fake root: {c}"
        );
    }
    assert!(
        !host
            .tmp
            .path()
            .join("proc/irq/100/smp_affinity_list")
            .exists(),
        "nothing is written"
    );
}

#[test]
fn the_housekeeping_cores_are_the_online_cores_minus_the_isolated_ones() {
    let host = FakeHost::bare_metal();
    host.cpus(8, Some("1-2"), &[(0, "0-3"), (1, "4-7")]);
    host.irq_effective(110, "1");
    let h = hints_json(&host, &link_on(NIC1));
    let cmds = commands_of(&h, "irq_affinity");
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert_eq!(irq_command(&cmds[0]), (110, vec![0, 3, 4, 5, 6, 7]));
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
    let h = hints_json(&host, &setup(vec![link("l", NIC0, NIC1, None)]));
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
    // no cpufreq at all (a VM): info and no command, as before
    let vm = hints_json(&FakeHost::vm(), &Setup::default());
    assert_level(&vm, "governor", "info", "info");
    assert_no_commands(&vm, "governor");
    // already performance: ok
    let ok = hints_json(&FakeHost::bare_metal(), &Setup::default());
    assert_level(&ok, "governor", "ok", "ok");
    assert_no_commands(&ok, "governor");
}

#[test]
fn the_governor_commands_cover_the_worker_and_isolated_cores_that_offer_performance() {
    let host = FakeHost::bare_metal(); // isolated 2-3,6-7
    host.governor_all(8, "powersave");
    host.write(
        "sys/devices/system/cpu/cpu3/cpufreq/scaling_available_governors",
        "powersave\n",
    );
    // the worker runs on the non-isolated core 4
    let h = hints_json(&host, &setup(vec![link("l", NIC1, NIC1, Some(4))]));
    assert_commands(
        &h,
        "governor",
        &[&gov_cmd(2), &gov_cmd(4), &gov_cmd(6), &gov_cmd(7)],
    );
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
fn no_hugepages_are_used_with_no_huge_so_nothing_is_suggested() {
    let st = setup_eal(vec![link("l", NIC0, NIC0, None)], json!(["--no-huge"]));
    let h = hints_json(&FakeHost::bare(), &st);
    assert_level(&h, "hugepages", "info", "info");
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
    let mut bad_cfg = link("c", "net_null0", "net_null1", None);
    bad_cfg["config"]["delayer"]["std_ns"] = json!(-1.0);
    let mut no_rate = link("r", "net_null0", "net_null1", None);
    no_rate["config"]["delayer"]
        .as_object_mut()
        .unwrap()
        .remove("initialization_rate_pps");
    let mut poll0 = setup(vec![link("x", "net_null0", "net_null0", None)]);
    poll0.settings["poll_interval_ms"] = json!(0);
    let cases: Vec<(&str, Setup, &str)> = vec![
        (
            "bad name",
            setup(vec![link("a b", "net_null0", "net_null0", None)]),
            "link name",
        ),
        (
            "bad port",
            setup(vec![link("x", "eth0", "net_null0", None)]),
            "port",
        ),
        (
            "a port that is shell syntax",
            setup(vec![link("x", "0000:17:00.0;reboot", "net_null0", None)]),
            "port",
        ),
        ("bad config", setup(vec![bad_cfg]), "std_ns"),
        ("no rate", setup(vec![no_rate]), "initialization_rate_pps"),
        ("poll interval 0", poll0, "poll_interval_ms"),
        (
            "eal arg",
            setup_eal(
                vec![link("x", "net_null0", "net_null0", None)],
                json!(["-d", "x.so"]),
            ),
            "-d",
        ),
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
    let h = hints_json(&host, &setup(vec![link("l", NIC0, NIC1, None)]));
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
            .post(
                "/api/host/hints",
                setup(vec![link("l", NIC0, NIC1, None)]).body(),
            )
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
    a.start_ok(vec![link("l", NIC1, NIC1, None)]).await; // a real start through the route
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
    let h = hints_json(&host, &setup(vec![link("l", NIC0, NIC1, None)]));
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
    host.irq_effective(101, "3");
    set_mrr(&host, NIC0, 512);
    let h = hints_json(&host, &setup(vec![link("l", NIC0, NIC1, None)]));
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

// ---- the route is gone -----------------------------------------------------------------------

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
