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
        .map(|i| {
            let p = format!("net_null{i}");
            link(i + 1, &format!("l{i}"), &p, &p, None)
        })
        .collect();
    store::save(state.path(), &state_with(links)).unwrap();
    let o = run(&host, state.path(), false);
    assert_eq!(
        o.status.code(),
        Some(1),
        "5 links need 5 isolated cores: {:?}",
        lines(&o)
    );
}

#[test]
fn the_check_prints_no_mprq_line_in_text_or_json() {
    for host in [FakeHost::bare_metal(), FakeHost::bare()] {
        let state = tempfile::tempdir().unwrap();
        let j = run(&host, state.path(), true);
        let hints: Value = serde_json::from_slice(&j.stdout).unwrap();
        assert_no_devargs_hint(&hints);
        let t = run(&host, state.path(), false);
        for l in lines(&t) {
            let l = l.to_lowercase();
            assert!(!l.contains("mprq") && !l.contains("devargs"), "{l}");
        }
    }
}

// ---- spec19 item 4: an unloadable state file ----------------------------------------------
//
// `dpds.sh install` tells this failure from ordinary hints by exit code 2 (hints exit 0 or 1)
// and the message on stderr, so both are pinned here.

#[test]
fn an_invalid_state_file_exits_two_with_the_error_and_the_advice_on_stderr() {
    let host = FakeHost::bare_metal();
    let state = tempfile::tempdir().unwrap();
    let path = state.path().join(store::STATE_FILE_NAME);
    std::fs::write(
        &path,
        r#"{"links":[],"settings":{"main_lcore":null,"eal_extra_args":[],"poll_interval_ms":500,"mlx5_devargs":"x"},"next_id":1}"#,
    )
    .unwrap();
    for json in [false, true] {
        let o = run(&host, state.path(), json);
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        assert_eq!(o.status.code(), Some(2), "json={json}: {err}");
        assert!(err.contains("is not a valid state file"), "{err}");
        assert!(err.contains(&path.display().to_string()), "{err}");
        let advice: Vec<&str> = err.lines().skip(1).collect();
        let advice = advice.join("\n").to_lowercase();
        assert!(
            advice.contains("mlx5_devargs"),
            "key named in the advice: {err}"
        );
        assert!(
            advice.contains("remove") && advice.contains("move"),
            "both remedies: {err}"
        );
        // nothing but hints on stdout, and no hints at all when the state cannot be loaded
        assert!(lines(&o).is_empty(), "stdout: {:?}", lines(&o));
    }
    // the operator's file is untouched
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("mlx5_devargs"));
}
