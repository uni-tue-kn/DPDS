//! `dpds-controller check` (B6): text lines, exit code, `--json`.
//! Assumed flags: `--sysfs-root`, `--procfs-root`, `--state-dir`, `--json`.

use std::process::{Command, Output};

use crate::common::*;
use dpds_controller::store;
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_dpds-controller");

fn run(host: &FakeHost, state_dir: &std::path::Path, json: bool) -> Output {
    let mut c = Command::new(BIN);
    c.arg("check")
        .arg("--sysfs-root")
        .arg(host.sysfs())
        .arg("--procfs-root")
        .arg(host.procfs())
        .arg("--state-dir")
        .arg(state_dir);
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
    let state = tempfile::tempdir().unwrap();
    let o = run(&host, &state.path().join("nonexistent"), false);
    assert_eq!(
        o.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let l = lines(&o);
    assert!(l.len() >= HINT_IDS.len(), "{l:?}");
    assert!(l.iter().any(|x| x.contains("OK")), "{l:?}");
    assert!(l.iter().any(|x| x.contains("INFO")), "{l:?}"); // the MPRQ note
    assert!(!l.iter().any(|x| x.contains("ERROR")), "{l:?}");
}

#[test]
fn json_output_is_the_hint_array_and_text_matches_it() {
    let host = FakeHost::bare();
    let state = tempfile::tempdir().unwrap();
    let j = run(&host, state.path(), true);
    let hints: Value = serde_json::from_slice(&j.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}",
            String::from_utf8_lossy(&j.stdout)
        )
    });
    for id in HINT_IDS {
        let _ = hint(&hints, id);
    }
    let t = run(&host, state.path(), false);
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
    let state = tempfile::tempdir().unwrap();
    let o = run(&host, state.path(), false);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        lines(&o).iter().any(|l| l.contains("ERROR")),
        "{:?}",
        lines(&o)
    );
    let o = run(&host, state.path(), true);
    assert_eq!(o.status.code(), Some(1), "--json keeps the exit code");
}

#[test]
fn warnings_alone_do_not_fail_the_check() {
    let host = FakeHost::bare_metal();
    host.cmdline("ro isolcpus=2-3,6-7"); // isolation: warn (isolcpus without nohz_full/rcu_nocbs)
    let state = tempfile::tempdir().unwrap();
    let o = run(&host, state.path(), false);
    assert_eq!(o.status.code(), Some(0));
    assert!(lines(&o).iter().any(|l| l.contains("WARN")));
}

#[test]
fn stored_links_are_taken_into_account() {
    let host = FakeHost::bare_metal(); // 4 isolated cores
    let state = tempfile::tempdir().unwrap();
    let links: Vec<_> = (0..5)
        .map(|i| link(i + 1, &format!("l{i}"), &[&format!("net_null{i}")], None))
        .collect();
    store::save(state.path(), &state_with(links)).unwrap();
    let o = run(&host, state.path(), false);
    assert_eq!(
        o.status.code(),
        Some(1),
        "5 directions need 5 isolated cores: {:?}",
        lines(&o)
    );
}
