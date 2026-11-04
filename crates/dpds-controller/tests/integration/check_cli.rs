//! `dpds-controller check` (B6): text lines, exit code, `--json`.
//! Assumed flags: `--sysfs-root`, `--procfs-root`, `--json`. Spec 22: `check` has no state (no
//! `--state-dir`, no stored links to look at) and prints the host-only hints; the old tests of a
//! stored link on a routed NIC, of 5 stored links needing 5 cores and of an unloadable state file
//! (exit 2) have no counterpart any more (see `stateless.rs` for the removed flag).

use std::process::{Command, Output};

use crate::common::*;
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_dpds-controller");

fn run(host: &FakeHost, json: bool) -> Output {
    let mut c = Command::new(BIN);
    c.arg("check")
        .arg("--sysfs-root")
        .arg(host.sysfs())
        .arg("--procfs-root")
        .arg(host.procfs());
    if json {
        c.arg("--json");
    }
    c.output().expect("cannot run dpds-controller")
}

fn lines(o: &Output) -> Vec<String> {
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn tuned_host_exits_zero_and_prints_one_line_per_hint() {
    let host = FakeHost::bare_metal();
    let o = run(&host, false);
    assert_eq!(
        o.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let l = lines(&o);
    assert!(l.len() >= HINT_IDS.len(), "{l:?}");
    assert!(l.iter().any(|x| x.contains("OK")), "{l:?}");
    assert!(!l.iter().any(|x| x.contains("ERROR")), "{l:?}");
}

#[test]
fn json_output_is_the_hint_array_and_text_matches_it() {
    let host = FakeHost::bare();
    let j = run(&host, true);
    let hints: Value = serde_json::from_slice(&j.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}",
            String::from_utf8_lossy(&j.stdout)
        )
    });
    for id in HINT_IDS {
        let _ = hint(&hints, id);
    }
    let t = run(&host, false);
    let text = lines(&t);
    for h in hints.as_array().unwrap() {
        let tag = h["level"].as_str().unwrap().to_ascii_uppercase();
        let title = h["title"].as_str().unwrap();
        let n = text.iter().filter(|l| l.contains(title)).count();
        assert_eq!(
            n, 1,
            "title {title:?} must appear on exactly one line: {text:?}"
        );
        let line = text.iter().find(|l| l.contains(title)).unwrap();
        assert!(line.contains(&tag), "line {line:?} lacks level {tag}");
    }
    assert_eq!(j.status.code(), t.status.code());
}

#[test]
fn error_hint_gives_exit_code_one_and_an_error_line() {
    let host = FakeHost::bare(); // no isolation, no hugepages
    let o = run(&host, false);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        lines(&o).iter().any(|l| l.contains("ERROR")),
        "{:?}",
        lines(&o)
    );
    let o = run(&host, true);
    assert_eq!(o.status.code(), Some(1), "--json keeps the exit code");
}

#[test]
fn warnings_alone_do_not_fail_the_check() {
    let host = FakeHost::bare_metal();
    host.cmdline("ro isolcpus=2-3,6-7"); // isolation: warn (isolcpus without nohz_full/rcu_nocbs)
    let o = run(&host, false);
    assert_eq!(o.status.code(), Some(0));
    assert!(lines(&o).iter().any(|l| l.contains("WARN")));
}

#[test]
fn the_check_prints_no_mprq_line_in_text_or_json() {
    for host in [FakeHost::bare_metal(), FakeHost::bare()] {
        let j = run(&host, true);
        let hints: Value = serde_json::from_slice(&j.stdout).unwrap();
        assert_no_devargs_hint(&hints);
        let t = run(&host, false);
        for l in lines(&t) {
            let l = l.to_lowercase();
            assert!(!l.contains("mprq") && !l.contains("devargs"), "{l}");
        }
    }
}

// ---- spec 22: nothing to load, nothing that can fail to load ----------------------------------

#[test]
fn the_exit_code_is_never_two_because_of_a_state_file() {
    // `dpds.sh install` used to treat exit 2 as "the controller would not start (state file)":
    // the controller has no state file, so a leftover one changes nothing about the hints
    let host = FakeHost::bare_metal();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("state.json"), "{garbage").unwrap();
    for json in [false, true] {
        let o = Command::new(BIN)
            .arg("check")
            .arg("--sysfs-root")
            .arg(host.sysfs())
            .arg("--procfs-root")
            .arg(host.procfs())
            .args(if json { vec!["--json"] } else { vec![] })
            .env("DPDS_CONTROLLER_STATE_DIR", dir.path())
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(0),
            "json={json}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(!lines(&o).is_empty());
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("state.json")).unwrap(),
        "{garbage"
    );
}

#[test]
fn the_check_has_no_link_dependent_hints() {
    // no links are known to `check`: nothing about an uplink or a drain timeout
    let host = FakeHost::bare_metal();
    let j = run(&host, true);
    let hints: Value = serde_json::from_slice(&j.stdout).unwrap();
    for h in hints.as_array().unwrap() {
        assert_ne!(h["id"], "uplink", "{h}");
        assert_ne!(h["id"], "drain_timeout", "{h}");
    }
}

#[test]
fn check_takes_the_test_only_ifaddrs_file_and_stays_host_only() {
    // an address on a NIC that no link uses changes nothing: `check` has no links
    let host = FakeHost::bare_metal();
    host.addr("ens1f0", "192.168.1.5");
    let file = host.ifaddrs_file();
    for json in [false, true] {
        let mut c = Command::new(BIN);
        c.arg("check")
            .arg("--sysfs-root")
            .arg(host.sysfs())
            .arg("--procfs-root")
            .arg(host.procfs())
            .arg("--ifaddrs-file")
            .arg(&file);
        if json {
            c.arg("--json");
        }
        let o = c.output().unwrap();
        assert_eq!(
            o.status.code(),
            Some(0),
            "json={json}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(!lines(&o).is_empty());
    }
}

// ---- spec 33: the commands of a hint, under its line ---------------------------------------
//
// `check` prints each hint's `commands` under the hint's own line: first one line that, without
// its indentation, is exactly `# run as root:`, then one line per command, in order, each
// indented and exactly the command after the indentation. The next hint's line is not indented.
// A hint without commands has no such block, and `check` changes nothing on the host (it runs no
// tool either: it knows no NIC).

const HEADER: &str = "# run as root:";

fn indented(l: &str) -> bool {
    l.starts_with("  ")
}

/// Expected block of a hint: the header and its commands.
fn expected_block(commands: &[String]) -> Vec<String> {
    let mut v = vec![HEADER.to_string()];
    v.extend(commands.iter().cloned());
    v
}

#[test]
fn the_commands_of_a_hint_are_printed_indented_under_a_run_as_root_line() {
    let host = FakeHost::bare_metal();
    host.sysctl("swappiness", Some("60"));
    host.sysctl("zone_reclaim_mode", Some("1"));
    host.governor_all(8, "powersave");
    let hints: Value = serde_json::from_slice(&run(&host, true).stdout).unwrap();
    let text = lines(&run(&host, false));
    let mut with = 0;
    for h in hints.as_array().unwrap() {
        let (title, tag) = (
            h["title"].as_str().unwrap(),
            h["level"].as_str().unwrap().to_ascii_uppercase(),
        );
        let at: Vec<usize> = (0..text.len())
            .filter(|i| !indented(&text[*i]) && text[*i].contains(title) && text[*i].contains(&tag))
            .collect();
        assert_eq!(at.len(), 1, "{title}: {text:?}");
        let commands: Vec<String> = h["commands"]
            .as_array()
            .map(|c| c.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        let block: Vec<&String> = text[at[0] + 1..]
            .iter()
            .take_while(|l| indented(l))
            .collect();
        if commands.is_empty() {
            assert!(block.is_empty(), "{title} has no commands: {block:?}");
            continue;
        }
        with += 1;
        let got: Vec<String> = block.iter().map(|l| l.trim().to_string()).collect();
        assert_eq!(got, expected_block(&commands), "{title}: {text:?}");
        // the header is indented, every command at least as deep as the header
        let depth = |l: &str| l.len() - l.trim_start().len();
        assert!(depth(block[0]) >= 2, "{block:?}");
        assert!(
            block.iter().all(|l| depth(l) >= depth(block[0])),
            "{block:?}"
        );
    }
    assert_eq!(
        with, 3,
        "swappiness, zone_reclaim_mode and the governor: {hints}"
    );
    assert_commands(&hints, "swappiness", &["sysctl -w vm.swappiness=0"]);
    assert_eq!(
        text.iter().filter(|l| l.trim() == HEADER).count(),
        3,
        "one header per hint with commands: {text:?}"
    );
    assert!(
        text.iter().all(|l| !l.contains("Apply")),
        "no Apply note any more: {text:?}"
    );
    // the indented lines are only ever commands and headers
    for l in text.iter().filter(|l| indented(l)) {
        assert!(
            l.trim() == HEADER
                || hints.as_array().unwrap().iter().any(|h| h["commands"]
                    .as_array()
                    .is_some_and(|c| c.iter().any(|x| x.as_str() == Some(l.trim())))),
            "an indented line that is neither header nor command: {l:?}"
        );
    }
}

#[test]
fn a_tuned_host_prints_no_command_block() {
    let o = run(&FakeHost::bare_metal(), false);
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(!text.contains(HEADER), "{text}");
    assert!(lines(&o).iter().all(|l| !indented(l)), "{text}");
}

#[test]
fn the_json_output_has_the_commands_and_no_text_block() {
    let host = FakeHost::bare_metal();
    host.sysctl("swappiness", Some("60"));
    let o = run(&host, true);
    let hints: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_commands(&hints, "swappiness", &["sysctl -w vm.swappiness=0"]);
    assert_no_commands(&hints, "zone_reclaim_mode");
    assert_no_action_members(&hints);
    assert!(!String::from_utf8_lossy(&o.stdout).contains(HEADER));
}

#[test]
fn check_changes_nothing_on_the_host() {
    let host = FakeHost::bare_metal();
    host.sysctl("swappiness", Some("60"));
    host.governor_all(8, "powersave");
    let _ = run(&host, false);
    let _ = run(&host, true);
    assert_eq!(host.read("proc/sys/vm/swappiness"), "60");
    assert_eq!(
        host.read("sys/devices/system/cpu/cpu2/cpufreq/scaling_governor"),
        "powersave"
    );
}
