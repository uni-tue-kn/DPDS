//! Start failures are reported through the UI.
//!
//! `POST /api/emulation/start` compares the links' estimated mbuf-pool need with the free
//! hugepage memory of the (fake) host. Need above free: 400 `{"message"}` naming the need, the
//! free memory, the link with the largest need and its buffer capacity; dpds is not launched.
//! Need equal to free fits. A host without any hugepage information is not refused (dpds reports
//! it). `POST /api/emulation/validate` never does this check.
//!
//! The need of one link is `(buffer_capacity + 9024) * 1920` bytes (the estimator of the
//! hugepage hint); the numbers below are worked out from it.
//!
//! When dpds exits with a non-zero status the error is `dpds exited with <exit status N |
//! signal S>: <reason>`, the reason being the last captured line that starts with `dpds: ` and
//! is not a `dpds: warning:` line, without the prefix.
//!
//! Not covered here: a restart (no restart route), and a host that reports
//! hugepages with total 0.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::common::*;
use crate::fake_telemetry::{short_tempdir, FakeTelemetryServer, Mode};
use dpds_controller::lcores::DirectionPlan;
use dpds_controller::supervisor::{
    LaunchPlan, ProcessSupervisor, State, Supervisor, SupervisorStatus,
};
use dpds_controller::telemetry::socket_path;

// ---- hugepage check of start ----------------------------------------------------------------------------------------

/// One link with the given buffer capacity.
fn sized(name: &str, port: &str, capacity: u64) -> Value {
    let mut cfg = cfg_json(1.0e6, 1.0e5);
    cfg["buffer_capacity"] = json!(capacity);
    link_cfg(name, port, port, cfg)
}

/// 2 MiB hugepages on a host that otherwise starts fine: `free` of `total` pages, global.
fn app_with_pages(total: u64, free: u64) -> TestApp {
    let host = FakeHost::bare_metal();
    host.hugepages(&[], total, free);
    TestApp::new(host)
}

// need of a 10 M packet buffer: (10_000_000 + 9024) * 1920 = 19_217_326_080 B = 18327 MiB
const BIG: u64 = 10_000_000;
const BIG_MIB: &str = "18327 MiB";
// need of 1_629_376 packets: (1_629_376 + 9024) * 1920 = 3_145_728_000 B = exactly 3000 MiB
const EXACT: u64 = 1_629_376;

#[tokio::test]
async fn a_start_that_needs_more_hugepages_than_are_free_is_a_400_naming_the_numbers() {
    let a = app_with_pages(1024, 1000); // 2048 MiB total, 2000 MiB free
    let r = a.start_links(vec![sized("big", "net_null0", BIG)]).await;
    assert_eq!(r.status, 400, "{}", r.text());
    let m = r.message();
    for needle in [
        "hugepage",
        BIG_MIB,
        "2000 MiB",
        "\"big\"",
        "buffer capacity 10000000 packets",
    ] {
        assert!(m.contains(needle), "{needle:?} missing from {m:?}");
    }
}

#[tokio::test]
async fn the_message_has_the_wording_of_the_spec() {
    let a = app_with_pages(1024, 1000);
    let m = a
        .start_links(vec![sized("big", "net_null0", BIG)])
        .await
        .message();
    assert_eq!(
        m,
        "the links need about 18327 MiB of hugepage memory for their mbuf pools, but only 2000 MiB are free; \
         link \"big\" needs about 18327 MiB (buffer capacity 10000000 packets). \
         Lower its buffer capacity or reserve more hugepages."
    );
}

#[tokio::test]
async fn a_refused_start_does_not_launch_dpds_and_leaves_the_emulation_stopped() {
    let a = app_with_pages(1024, 1000);
    let r = a.start_links(vec![sized("big", "net_null0", BIG)]).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert_eq!(a.supervisor.starts_entered(), 0);
    assert!(a.supervisor.plans().is_empty());
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "stopped", "{v}");
    // nothing sticks: a start that fits is accepted afterwards
    let r = a.start_links(vec![sized("ok", "net_null0", 100_000)]).await;
    assert_eq!(r.status, 202, "{}", r.text());
}

#[tokio::test]
async fn the_need_of_all_links_is_summed_and_the_largest_link_is_named() {
    // 3 M packets: 5509 MiB, 5 M packets: 9171 MiB, together 14681 MiB; 12000 MiB are free:
    // each link fits alone, both together do not
    let a = app_with_pages(8192, 6000);
    let alone = a
        .start_links(vec![sized("small", "net_null0", 3_000_000)])
        .await;
    assert_eq!(alone.status, 202, "{}", alone.text());
    a.post("/api/emulation/stop", json!({})).await;
    let r = a
        .start_links(vec![
            sized("small", "net_null0", 3_000_000),
            sized("large", "net_null1", 5_000_000),
        ])
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    let m = r.message();
    for needle in [
        "about 14681 MiB of hugepage",
        "only 12000 MiB are free",
        "link \"large\" needs about 9171 MiB (buffer capacity 5000000 packets)",
    ] {
        assert!(m.contains(needle), "{needle:?} missing from {m:?}");
    }
    assert!(!m.contains("\"small\""), "{m}");
}

#[tokio::test]
async fn a_start_that_fits_is_accepted() {
    let a = app_with_pages(4096, 4096); // 8192 MiB free
    let r = a
        .start_links(vec![sized("l", "net_null0", 1_000_000)]) // 1_009_024 * 1920 = 1847 MiB
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(a.supervisor.plans().len(), 1);
}

#[tokio::test]
async fn need_equal_to_free_fits_and_one_page_less_is_refused() {
    // EXACT needs 3000 MiB = 1500 pages of 2 MiB
    let a = app_with_pages(1500, 1500);
    let r = a.start_links(vec![sized("l", "net_null0", EXACT)]).await;
    assert_eq!(r.status, 202, "{}", r.text());

    let a = app_with_pages(1500, 1499);
    let r = a.start_links(vec![sized("l", "net_null0", EXACT)]).await;
    assert_eq!(r.status, 400, "{}", r.text());
    let m = r.message();
    assert!(
        m.contains("3000 MiB") && m.contains("only 2998 MiB are free"),
        "{m}"
    );
}

#[tokio::test]
async fn a_host_without_hugepage_information_is_not_refused() {
    let host = FakeHost::bare_metal();
    host.remove("sys/kernel/mm/hugepages");
    host.remove("proc/meminfo");
    let a = TestApp::new(host);
    let r = a.start_links(vec![sized("big", "net_null0", BIG)]).await;
    assert_eq!(
        r.status,
        202,
        "dpds reports it, not the controller: {}",
        r.text()
    );
    assert_eq!(a.supervisor.plans().len(), 1);
}

#[tokio::test]
async fn validate_does_not_check_the_hugepages() {
    let a = app_with_pages(1024, 0); // nothing free, e.g. a running emulation holds the pages
    let r = a
        .validate(&setup(vec![sized("big", "net_null0", BIG)]))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn a_start_that_fails_validation_keeps_its_own_error() {
    // not a hugepage message: an empty link list is "nothing to start" however few pages are free
    let a = app_with_pages(1024, 0);
    let r = a.start_links(vec![]).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(!r.message().contains("hugepage"), "{}", r.message());
}

// ---- dpds exit error ----------------------------------------------------------------------------------------

fn script(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("fake-dpds.sh");
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn plan(bin: PathBuf, runtime_dir: &Path) -> LaunchPlan {
    let links_file = runtime_dir.join("links.json");
    LaunchPlan {
        dpds_bin: bin,
        runtime_dir: runtime_dir.to_path_buf(),
        links_file: links_file.clone(),
        links_json: json!({"links":[{"name":"l","ingress":"net_null0","egress":"net_null0","config":{},"lcore":1}]}),
        argv: vec![
            "--".into(),
            "--links".into(),
            links_file.to_str().unwrap().into(),
        ],
        directions: vec![DirectionPlan {
            dir: 0,
            link: "l".into(),
            rx: "net_null0".into(),
            tx: "net_null0".into(),
            lcore: 1,
        }],
        main_lcore: 0,
        drain_timeout: Duration::from_secs(1),
    }
}

fn wait_for(
    sup: &ProcessSupervisor,
    what: &str,
    pred: impl Fn(&SupervisorStatus) -> bool,
) -> SupervisorStatus {
    let t0 = Instant::now();
    loop {
        let s = sup.status();
        if pred(&s) {
            return s;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(12),
            "timed out waiting for {what}; last status {:?} error {:?} tail {:?}",
            s.state,
            s.error,
            s.log_tail
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Runs a fake dpds with this shell `body` to its failure and returns (error, log_tail).
fn failed_start(body: &str) -> (String, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    let bin = script(tmp.path(), body);
    let sup = ProcessSupervisor::new(Duration::from_secs(20));
    sup.start(plan(bin, &tmp.path().join("rt")));
    let s = wait_for(&sup, "failed", |s| s.state == State::Failed);
    (
        s.error.expect("a failed state carries an error"),
        s.log_tail,
    )
}

#[test]
fn the_error_carries_the_dpds_line_without_its_prefix() {
    let (err, tail) = failed_start(
        "echo 'dpds: direction 0: cannot create mbuf pool dpds_pool_0 with 100009024 mbufs: \
         Cannot allocate memory (code -12)' >&2; exit 1",
    );
    assert_eq!(
        err,
        "dpds exited with exit status 1: direction 0: cannot create mbuf pool dpds_pool_0 \
         with 100009024 mbufs: Cannot allocate memory (code -12)"
    );
    // the log_tail is unchanged: the full line with its prefix
    assert!(
        tail.iter()
            .any(|l| l.starts_with("dpds: direction 0: cannot create")),
        "{tail:?}"
    );
}

#[test]
fn the_last_dpds_line_wins_and_other_lines_are_ignored() {
    let (err, _) = failed_start(
        "echo 'dpds: first problem' >&2; echo 'EAL: something' >&2; \
         echo 'dpds: second problem' >&2; echo 'EAL: Error - exiting with code: 1' >&2; exit 2",
    );
    assert_eq!(err, "dpds exited with exit status 2: second problem");
}

#[test]
fn warnings_are_skipped_even_when_they_come_last() {
    let (err, tail) = failed_start(
        "echo 'dpds: cannot open port 3' >&2; echo 'dpds: warning: using the slow path' >&2; \
         echo 'dpds: warning: second warning' >&2; exit 1",
    );
    assert_eq!(err, "dpds exited with exit status 1: cannot open port 3");
    assert!(
        tail.iter().any(|l| l == "dpds: warning: second warning"),
        "{tail:?}"
    );
}

#[test]
fn the_dpds_line_may_come_from_stdout_too() {
    let (err, _) = failed_start("echo 'dpds: from stdout'; exit 1");
    assert_eq!(err, "dpds exited with exit status 1: from stdout");
}

#[test]
fn without_a_dpds_line_the_error_stays_as_before() {
    let (err, _) = failed_start("echo 'EAL: out of memory' >&2; exit 1");
    assert_eq!(err, "dpds exited with exit status 1");
}

#[test]
fn only_warnings_leave_the_old_text_too() {
    let (err, _) = failed_start("echo 'dpds: warning: only this' >&2; exit 1");
    assert_eq!(err, "dpds exited with exit status 1");
}

#[test]
fn lines_that_do_not_start_with_the_dpds_prefix_do_not_count() {
    let (err, _) = failed_start(
        "echo 'see dpds: not at the start' >&2; echo ' dpds: indented' >&2; \
         echo 'dpds:no space' >&2; echo 'DPDS: upper case' >&2; exit 1",
    );
    assert_eq!(err, "dpds exited with exit status 1");
}

#[test]
fn a_death_by_signal_names_the_signal_and_the_reason() {
    let (err, _) = failed_start("echo 'dpds: lost the device' >&2; kill -KILL $$");
    assert_eq!(err, "dpds exited with signal 9: lost the device");
}

#[test]
fn an_exit_while_running_carries_the_reason_too() {
    let tmp = short_tempdir();
    let rt = tmp.path().join("rt");
    let flag = tmp.path().join("die");
    let bin = script(
        tmp.path(),
        &format!(
            "while [ ! -e {} ]; do sleep 0.05; done; echo 'dpds: lcore 3 died' >&2; exit 1",
            flag.display()
        ),
    );
    let sup = ProcessSupervisor::new(Duration::from_secs(30));
    sup.start(plan(bin, &rt));
    let pid = wait_for(&sup, "a pid", |s| s.pid.is_some()).pid.unwrap();
    let _server =
        FakeTelemetryServer::start(&socket_path(&rt), pid, vec![Mode::Serve], |cmd: &str| {
            (cmd == "/dpds/ports").then(|| {
                json!({"num_ports": 1, "num_links": 1, "num_directions": 1,
                       "port0_name": "net_null0", "dir0_link": "l", "dir0_rx_name": "net_null0",
                       "dir0_tx_name": "net_null0", "dir0_rx_port": 0, "dir0_tx_port": 0,
                       "dir0_lcore": 1})
            })
        });
    wait_for(&sup, "running", |s| s.state == State::Running);
    fs::write(&flag, "").unwrap();
    let s = wait_for(&sup, "failed", |s| s.state == State::Failed);
    let err = s.error.unwrap();
    assert!(
        err.starts_with("dpds exited") && err.ends_with("exit status 1: lcore 3 died"),
        "{err}"
    );
}
