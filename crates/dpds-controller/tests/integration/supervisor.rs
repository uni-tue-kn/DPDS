//! The real `ProcessSupervisor` against shell scripts standing in for `dpds`
//! (no DPDK needed). Running/stop/drain with a real `dpds` is in `e2e.rs`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dpds_controller::lcores::DirectionPlan;
use dpds_controller::supervisor::{
    LaunchPlan, ProcessSupervisor, State, Supervisor, SupervisorStatus,
};
use serde_json::json;

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
        links_json: json!({"links":[{"ingress":"net_null0","egress":"net_null0","config":{},"lcore":1}]}),
        argv: vec![
            "-l".into(),
            "0,1".into(),
            "--".into(),
            "--links".into(),
            links_file.to_str().unwrap().into(),
        ],
        directions: vec![DirectionPlan {
            dir: 0,
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
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn pid_alive(pid: i32) -> bool {
    // a zombie still answers kill(0); read the state from /proc
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => !s.rsplit(") ").next().unwrap_or("").starts_with('Z'),
        Err(_) => false,
    }
}

#[test]
fn immediate_exit_is_a_failed_start_with_stderr_tail_and_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = tmp.path().join("rt");
    let bin = script(
        tmp.path(),
        "echo 'EAL: out of memory' >&2; echo 'dpds: boom' >&2; exit 3",
    );
    let sup = ProcessSupervisor::new(Duration::from_secs(20));
    sup.start(plan(bin, &rt));
    let s = wait_for(&sup, "failed", Duration::from_secs(10), |s| {
        s.state == State::Failed
    });
    let err = s.error.expect("failed state carries an error");
    assert!(err.contains('3'), "exit code should be in the error: {err}");
    let tail = s.log_tail.join("\n");
    assert!(
        tail.contains("EAL: out of memory") && tail.contains("dpds: boom"),
        "{tail}"
    );
    assert_eq!(s.pid, None);
}

#[test]
fn links_file_is_written_before_spawn_with_the_plan_json() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = tmp.path().join("rt");
    let out = tmp.path().join("seen.json");
    let bin = script(
        tmp.path(),
        &format!(
            "for last; do :; done; cat \"$last\" > {}; exit 1",
            out.display()
        ),
    );
    let sup = ProcessSupervisor::new(Duration::from_secs(20));
    let p = plan(bin, &rt);
    sup.start(p.clone());
    wait_for(&sup, "failed", Duration::from_secs(10), |s| {
        s.state == State::Failed
    });
    let seen: serde_json::Value =
        serde_json::from_slice(&fs::read(&out).expect("script must see the links file")).unwrap();
    assert_eq!(seen, p.links_json);
}

#[test]
fn child_gets_the_runtime_dir_and_its_own_process_group() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = tmp.path().join("rt");
    let bin = script(
        tmp.path(),
        "echo \"rtdir=$RUNTIME_DIRECTORY xdg=$XDG_RUNTIME_DIR\" >&2; \
         echo \"pid=$$ pgrp=$(awk '{print $5}' /proc/$$/stat)\" >&2; exit 1",
    );
    let sup = ProcessSupervisor::new(Duration::from_secs(20));
    sup.start(plan(bin, &rt));
    let s = wait_for(&sup, "failed", Duration::from_secs(10), |s| {
        s.state == State::Failed
    });
    let tail = s.log_tail.join("\n");
    // DPDK's runtime dir lives in the dedicated `<runtime-dir>/instance` subdirectory
    // that the controller creates and removes (never the runtime dir itself)
    let rtd = rt.join("instance");
    let rtd = rtd.to_str().unwrap();
    assert!(
        tail.contains(&format!("rtdir={rtd} ")),
        "RUNTIME_DIRECTORY must be <runtime-dir>/instance: {tail}"
    );
    let pid_line = tail.lines().find(|l| l.starts_with("pid=")).unwrap();
    let mut it = pid_line
        .split_whitespace()
        .map(|x| x.split('=').nth(1).unwrap());
    let (pid, pgrp) = (it.next().unwrap(), it.next().unwrap());
    assert_eq!(
        pid, pgrp,
        "child must lead its own process group: {pid_line}"
    );
}

#[test]
fn log_tail_is_a_ring_of_the_last_500_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = tmp.path().join("rt");
    let bin = script(
        tmp.path(),
        "i=1; while [ $i -le 1200 ]; do echo \"line $i\"; i=$((i+1)); done; exit 1",
    );
    let sup = ProcessSupervisor::new(Duration::from_secs(20));
    sup.start(plan(bin, &rt));
    let s = wait_for(&sup, "failed", Duration::from_secs(10), |s| {
        s.state == State::Failed
    });
    assert_eq!(s.log_tail.len(), 500, "ring buffer of 500 lines");
    assert_eq!(s.log_tail.first().unwrap(), "line 701");
    assert_eq!(s.log_tail.last().unwrap(), "line 1200");
}

#[test]
fn start_timeout_without_telemetry_fails_and_kills_the_child() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = tmp.path().join("rt");
    let pidfile = tmp.path().join("pid");
    // ignores SIGINT/SIGTERM: only SIGKILL gets rid of it
    let bin = script(
        tmp.path(),
        &format!(
            "trap '' INT TERM; echo $$ > {}; echo started >&2; while :; do sleep 1; done",
            pidfile.display()
        ),
    );
    let sup = ProcessSupervisor::new(Duration::from_secs(2));
    sup.start(plan(bin, &rt));
    let s = wait_for(&sup, "failed (timeout)", Duration::from_secs(15), |s| {
        s.state == State::Failed
    });
    let err = s.error.unwrap().to_lowercase();
    assert!(
        err.contains("timeout") || err.contains("timed out"),
        "{err}"
    );
    assert!(s.log_tail.iter().any(|l| l.contains("started")));
    let pid: i32 = fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let t0 = Instant::now();
    while pid_alive(pid) {
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "child {pid} still alive after a failed start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn spawn_failure_is_a_failed_state_not_a_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let sup = ProcessSupervisor::new(Duration::from_secs(5));
    sup.start(plan(
        tmp.path().join("does-not-exist"),
        &tmp.path().join("rt"),
    ));
    let s = wait_for(&sup, "failed", Duration::from_secs(10), |s| {
        s.state == State::Failed
    });
    assert!(!s.error.unwrap().is_empty());
}

#[test]
fn a_fresh_supervisor_is_stopped_and_stop_is_harmless() {
    let sup = ProcessSupervisor::new(Duration::from_secs(5));
    assert_eq!(sup.status().state, State::Stopped);
    sup.stop();
    assert_eq!(sup.status().state, State::Stopped);
}
