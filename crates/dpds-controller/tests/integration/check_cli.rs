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
    // no links are known to `check`: nothing about a management NIC or a drain timeout
    let host = FakeHost::bare_metal();
    let j = run(&host, true);
    let hints: Value = serde_json::from_slice(&j.stdout).unwrap();
    for h in hints.as_array().unwrap() {
        assert_ne!(h["id"], "management_nic", "{h}");
        assert_ne!(h["id"], "drain_timeout", "{h}");
    }
}
