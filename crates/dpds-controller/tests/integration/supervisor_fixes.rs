//! C5, supervisor part: the real `ProcessSupervisor` with shell scripts standing in for
//! `dpds` and a fake telemetry socket (`fake_telemetry.rs`) for the readiness probe.
//!
//! - the readiness probe drops and reconnects its client after a transport error (a
//!   connection that dies after the greeting must not wedge the start), and the timeout
//!   message carries the last probe error;
//! - a child that exits while the state is `stopping` ends in `stopped`, not `failed`;
//! - the log ring caps one line at 4 KiB (4096 bytes), keeping the beginning;
//! - the runtime cleanup only ever removes the dedicated `<runtime-dir>/instance`
//!   subdirectory (DPDK's runtime dir, `RUNTIME_DIRECTORY`, lives inside it): unrelated
//!   files and a foreign `<runtime-dir>/dpdk` survive a failed start and a stop. The fake
//!   telemetry server must create its socket only after the spawn (the spawn cleans the
//!   instance dir), at `telemetry::socket_path(runtime_dir)`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::fake_telemetry::{short_tempdir, FakeTelemetryServer, Mode};
use dpds_controller::lcores::DirectionPlan;
use dpds_controller::supervisor::{
    LaunchPlan, ProcessSupervisor, State, Supervisor, SupervisorStatus,
};
use dpds_controller::telemetry::socket_path;
use serde_json::{json, Value};

fn script(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("fake-dpds.sh");
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// A script that runs until SIGINT.
const SLEEPER: &str = "while :; do sleep 0.1; done";

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
        drain_timeout: Duration::from_secs(2),
    }
}

fn ports_reply(cmd: &str) -> Option<Value> {
    (cmd == "/dpds/ports").then(|| {
        json!({"num_ports": 1, "num_links": 1, "num_directions": 1,
               "port0_name": "net_null0", "dir0_link": "l", "dir0_rx_name": "net_null0",
               "dir0_tx_name": "net_null0", "dir0_rx_port": 0, "dir0_tx_port": 0,
               "dir0_lcore": 1})
    })
}

fn wait_for(
    sup: &ProcessSupervisor,
    what: &str,
    timeout: Duration,
    pred: impl Fn(&SupervisorStatus) -> bool,
) -> SupervisorStatus {
    let t0 = Instant::now();
    loop {
        let s = sup.status();
        if pred(&s) {
            return s;
        }
        assert!(
            t0.elapsed() < timeout,
            "timed out waiting for {what}; last status {:?} error {:?} tail {:?}",
            s.state,
            s.error,
            s.log_tail
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_pid(sup: &ProcessSupervisor) -> u32 {
    wait_for(sup, "a pid", Duration::from_secs(10), |s| s.pid.is_some())
        .pid
        .unwrap()
}

// ---- readiness probe ------------------------------------------------------------

#[test]
fn a_connection_that_dies_after_the_greeting_is_replaced_and_the_start_succeeds() {
    let tmp = short_tempdir();
    let rt = tmp.path().join("rt");
    let bin = script(tmp.path(), SLEEPER);
    let sup = ProcessSupervisor::new(Duration::from_secs(30));
    sup.start(plan(bin, &rt));
    let pid = wait_pid(&sup);
    // first connection: greeting, then EOF; the second one works
    let server = FakeTelemetryServer::start(
        &socket_path(&rt),
        pid,
        vec![Mode::CloseAfterGreeting, Mode::Serve],
        ports_reply,
    );
    let s = wait_for(&sup, "running", Duration::from_secs(12), |s| {
        s.state == State::Running || s.state == State::Failed
    });
    assert_eq!(s.state, State::Running, "{:?} {:?}", s.error, s.log_tail);
    assert!(
        server.queries().iter().any(|q| q == "/dpds/ports"),
        "{:?}",
        server.queries()
    );
    sup.stop();
    wait_for(&sup, "stopped", Duration::from_secs(15), |s| {
        s.state == State::Stopped
    });
}

#[test]
fn the_timeout_message_names_the_last_probe_error() {
    let tmp = short_tempdir();
    let rt = tmp.path().join("rt");
    let bin = script(tmp.path(), SLEEPER);
    let sup = ProcessSupervisor::new(Duration::from_secs(2));
    sup.start(plan(bin, &rt));
    let pid = wait_pid(&sup);
    // answers every request with null: "no reply for /dpds/ports"
    let _server =
        FakeTelemetryServer::start(&socket_path(&rt), pid, vec![Mode::Nulls], ports_reply);
    let s = wait_for(&sup, "failed (timeout)", Duration::from_secs(15), |s| {
        s.state == State::Failed
    });
    let err = s.error.unwrap();
    assert!(err.to_lowercase().contains("timeout"), "{err}");
    assert!(
        err.contains("/dpds/ports"),
        "the last probe error must be in the message: {err}"
    );
}

#[test]
fn a_child_that_exits_while_stopping_ends_stopped_not_failed() {
    let tmp = short_tempdir();
    let rt = tmp.path().join("rt");
    let trigger = tmp.path().join("exit-now");
    let bin = script(
        tmp.path(),
        &format!(
            "while [ ! -e {} ]; do sleep 0.05; done; exit 1",
            trigger.display()
        ),
    );
    let sup = ProcessSupervisor::new(Duration::from_secs(30));
    sup.start(plan(bin, &rt));
    let pid = wait_pid(&sup);
    // the server never answers: the supervisor's worker is blocked in the probe's read
    let server = FakeTelemetryServer::start(&socket_path(&rt), pid, vec![Mode::Hang], ports_reply);
    let t0 = Instant::now();
    while server.queries().is_empty() {
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "no probe query seen"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // the child exits by itself, and the stop request arrives after that
    fs::write(&trigger, "").unwrap();
    std::thread::sleep(Duration::from_millis(500));
    sup.stop();
    let s = wait_for(&sup, "the end of stopping", Duration::from_secs(20), |s| {
        s.state != State::Stopping && s.state != State::Starting
    });
    assert_eq!(s.state, State::Stopped, "{:?} {:?}", s.error, s.log_tail);
    assert!(s.error.is_none(), "{:?}", s.error);
}

// ---- log lines -------------------------------------------------------------------

#[test]
fn one_log_line_is_capped_at_4_kib_and_later_lines_are_unaffected() {
    let tmp = short_tempdir();
    let rt = tmp.path().join("rt");
    let bin = script(
        tmp.path(),
        "head -c 200000 /dev/zero | tr '\\0' 'a'; echo; echo after; exit 1",
    );
    let sup = ProcessSupervisor::new(Duration::from_secs(20));
    sup.start(plan(bin, &rt));
    let s = wait_for(&sup, "failed", Duration::from_secs(10), |s| {
        s.state == State::Failed
    });
    assert!(
        s.log_tail.iter().all(|l| l.len() <= 4096),
        "longest line: {}",
        s.log_tail.iter().map(String::len).max().unwrap_or(0)
    );
    let long = s
        .log_tail
        .iter()
        .find(|l| l.starts_with("aaaa"))
        .expect("the beginning of the long line is kept");
    assert!(long.len() >= 1000, "{}", long.len());
    assert!(
        s.log_tail.iter().any(|l| l == "after"),
        "{:?}",
        s.log_tail.len()
    );
}

// ---- runtime directory ---------------------------------------------------------------

fn plant(rt: &Path) -> [PathBuf; 3] {
    let files = [
        rt.join("keep.txt"),
        rt.join("dpdk/foreign-app/data.txt"),
        rt.join("other-dir/x"),
    ];
    for f in &files {
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, "mine").unwrap();
    }
    files
}

#[test]
fn a_failed_start_removes_only_the_instance_dir_of_the_runtime_dir() {
    let tmp = short_tempdir();
    let rt = tmp.path().join("rt");
    let kept = plant(&rt);
    let bin = script(tmp.path(), "exit 3");
    let sup = ProcessSupervisor::new(Duration::from_secs(20));
    sup.start(plan(bin, &rt));
    wait_for(&sup, "failed", Duration::from_secs(10), |s| {
        s.state == State::Failed
    });
    for f in &kept {
        assert!(f.exists(), "{} must survive", f.display());
    }
    assert!(!rt.join("instance").exists());
}

#[test]
fn a_stop_removes_only_the_instance_dir_and_dpdks_runtime_dir_is_inside_it() {
    let tmp = short_tempdir();
    let rt = tmp.path().join("rt");
    let kept = plant(&rt);
    let bin = script(tmp.path(), SLEEPER);
    let sup = ProcessSupervisor::new(Duration::from_secs(30));
    sup.start(plan(bin, &rt));
    let pid = wait_pid(&sup);
    assert!(
        socket_path(&rt).starts_with(rt.join("instance")),
        "{}",
        socket_path(&rt).display()
    );
    let _server =
        FakeTelemetryServer::start(&socket_path(&rt), pid, vec![Mode::Serve], ports_reply);
    wait_for(&sup, "running", Duration::from_secs(12), |s| {
        s.state == State::Running
    });
    assert!(rt.join("instance").is_dir());
    sup.stop();
    wait_for(&sup, "stopped", Duration::from_secs(15), |s| {
        s.state == State::Stopped
    });
    for f in &kept {
        assert!(f.exists(), "{} must survive", f.display());
    }
    assert!(!rt.join("instance").exists());
}
