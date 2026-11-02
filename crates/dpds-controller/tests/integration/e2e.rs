//! End-to-end: the real `dpds-controller` binary supervising the real `dpds`
//! (built with `--features telemetry`) on DPDK virtual devices, driven only
//! through the REST API.
//!
//! These tests are `#[ignore]`d because they need `target/release/dpds`
//! (`cargo build --release -p dpds --features telemetry`) and several CPUs.
//! Run them explicitly:
//!
//! ```text
//! cargo build --release -p dpds --features telemetry
//! cargo test --release -p dpds-controller --test integration e2e -- --ignored --test-threads=1
//! ```
//!
//! When run, a missing / telemetry-less `dpds` binary or too few CPUs is a
//! FAILURE with a clear message, never a silent skip (same rule as
//! `crates/dpds/tests/e2e.rs`). `DPDS_BIN` overrides the binary path.
//!
//! The controller runs with a fake sysfs/procfs (online CPUs 0-1, CPU 1
//! isolated => main lcore 0, worker lcore 1) so the assignment does not depend
//! on the test host's boot parameters; the EAL flags `--no-huge -m 256
//! --no-pci` come from `settings.eal_extra_args`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::common::*;
use serde_json::{json, Value};

const CONTROLLER: &str = env!("CARGO_BIN_EXE_dpds-controller");
const START_TIMEOUT: Duration = Duration::from_secs(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(40);

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn require_cpus(n: usize) {
    let have = thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1);
    assert!(have >= n, "these e2e tests need {n} CPUs (lcores 0..{n}) but the host has {have}; run them on a bigger machine instead of skipping them");
}

/// Path of the telemetry-enabled `dpds`; fails (does not skip) when missing.
fn dpds_bin() -> PathBuf {
    let p = std::env::var_os("DPDS_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/dpds"));
    let bytes = std::fs::read(&p).unwrap_or_else(|e| {
        panic!(
            "{} is missing ({e}). Build it first: `cargo build --release -p dpds --features telemetry` \
             (or set DPDS_BIN). This test must not be skipped silently.",
            p.display()
        )
    });
    let needle = b"/dpds/stats";
    assert!(
        bytes.windows(needle.len()).any(|w| w == needle),
        "{} was built WITHOUT the `telemetry` feature (no `/dpds/stats` in the binary); rebuild with `cargo build --release -p dpds --features telemetry`",
        p.display()
    );
    p.canonicalize().unwrap()
}

struct Controller {
    child: Child,
    addr: String,
    _tmp: tempfile::TempDir,
    _host: FakeHost,
    runtime_dir: PathBuf,
}

impl Controller {
    fn start() -> Controller {
        require_cpus(2);
        let dpds = dpds_bin();
        let host = FakeHost::empty();
        host.cpus(2, Some("1"), &[]);
        host.cmdline("ro isolcpus=1 nohz_full=1 rcu_nocbs=1");
        let tmp = tempfile::tempdir().unwrap();
        // short path: the telemetry socket path must stay below 108 bytes
        let runtime_dir = PathBuf::from(format!("/tmp/dpdsc-{}", std::process::id()));
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let addr = format!("127.0.0.1:{port}");
        let child = Command::new(CONTROLLER)
            .args(["serve", "--bind", &addr])
            .arg("--state-dir")
            .arg(tmp.path().join("state"))
            .arg("--dpds-bin")
            .arg(&dpds)
            .arg("--runtime-dir")
            .arg(&runtime_dir)
            .arg("--sysfs-root")
            .arg(host.sysfs())
            .arg("--procfs-root")
            .arg(host.procfs())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("cannot spawn dpds-controller");
        let c = Controller {
            child,
            addr,
            _tmp: tmp,
            _host: host,
            runtime_dir,
        };
        let t0 = Instant::now();
        loop {
            if let Ok((200, v)) = http(&c.addr, "GET", "/api/online", None) {
                assert_eq!(
                    v["telemetry"], true,
                    "controller must detect the telemetry-enabled dpds: {v}"
                );
                return c;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(15),
                "controller did not come up on {}",
                c.addr
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        http(&self.addr, method, path, body.as_ref())
            .unwrap_or_else(|e| panic!("{method} {path}: {e}"))
    }

    fn emulation(&self) -> Value {
        let (s, v) = self.call("GET", "/api/emulation", None);
        assert_eq!(s, 200);
        v
    }

    fn wait_state(&self, want: &[&str], timeout: Duration) -> Value {
        let t0 = Instant::now();
        loop {
            let v = self.emulation();
            let st = v["state"].as_str().unwrap().to_string();
            if want.contains(&st.as_str()) {
                return v;
            }
            assert!(
                st != "failed" || want.contains(&"failed"),
                "emulation failed: error {} log_tail {}",
                v["error"],
                v["log_tail"]
            );
            assert!(
                t0.elapsed() < timeout,
                "timeout waiting for {want:?}; last: {v}"
            );
            thread::sleep(Duration::from_millis(200));
        }
    }

    fn configure(&self, eal_extra: &[&str], link_cfg: Value) {
        let (s, v) = self.call(
            "PUT",
            "/api/settings",
            Some(json!({
                "main_lcore": null, "mlx5_devargs": "txq_mpw_en=1",
                "eal_extra_args": eal_extra, "poll_interval_ms": 250
            })),
        );
        assert_eq!(s, 200, "{v}");
        let (s, v) = self.call(
            "POST",
            "/api/links",
            Some(json!({
                "name": "loop", "ports": ["net_null0"], "configs": [link_cfg]
            })),
        );
        assert_eq!(s, 201, "{v}");
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        // best effort: SIGTERM lets the controller stop its child; then make sure
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(20) {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                let _ = std::fs::remove_dir_all(&self.runtime_dir);
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.runtime_dir);
    }
}

fn process_gone(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s.rsplit(") ").next().unwrap_or("").starts_with('Z'),
        Err(_) => true,
    }
}

const EAL: [&str; 4] = ["--no-huge", "-m", "256", "--no-pci"];

fn loop_config() -> Value {
    let mut c = cfg_json(1.0e6, 0.0);
    c["buffer_capacity"] = json!(4096);
    c["delayer"]["distribution"] = json!("DeterministicDistribution");
    c["seed"] = json!(1);
    c
}

#[test]
#[ignore = "needs target/release/dpds built with --features telemetry; run with -- --ignored"]
fn full_rest_flow_create_start_stats_stop_drain() {
    let _g = serial();
    let c = Controller::start();
    c.configure(&EAL, loop_config());

    let (s, v) = c.call("POST", "/api/emulation/start", Some(json!({})));
    assert_eq!(s, 202, "{v}");
    let run = c.wait_state(&["running"], START_TIMEOUT);
    let pid = run["pid"].as_i64().expect("running state has a pid") as i32;
    assert!(pid > 0 && !process_gone(pid));
    assert_eq!(run["directions"].as_array().unwrap().len(), 1);
    assert_eq!(run["directions"][0]["lcore"], 1);
    assert_eq!(run["main_lcore"], 0);
    assert_eq!(run["pending_restart"], false);

    // counters move
    let first_rx = {
        let t0 = Instant::now();
        loop {
            let (s, v) = c.call("GET", "/api/stats", None);
            assert_eq!(s, 200);
            if let Some(rx) = v["directions"][0]["counters"]["rx_pkts"].as_u64() {
                if rx > 0 {
                    break rx;
                }
            }
            assert!(
                t0.elapsed() < Duration::from_secs(20),
                "no packets seen: {v}"
            );
            thread::sleep(Duration::from_millis(250));
        }
    };
    thread::sleep(Duration::from_millis(1500));
    let (_, v) = c.call("GET", "/api/stats", None);
    let d0 = &v["directions"][0];
    assert!(
        d0["counters"]["rx_pkts"].as_u64().unwrap() > first_rx,
        "rx_pkts must keep growing: {v}"
    );
    assert!(f(d0, "rx_pps") > 0.0, "rx_pps must be positive: {d0}");
    let mean = f(d0, "delay_mean_ns");
    assert!(
        (0.9e6..1.1e6).contains(&mean),
        "deterministic 1 ms delay, got mean {mean} ns"
    );
    assert_eq!(d0["link"], "loop");

    // histogram has samples
    let (s, h) = c.call("GET", "/api/stats/0/delay_histogram", None);
    assert_eq!(s, 200, "{h}");
    assert_eq!(h["bins"].as_array().unwrap().len(), 256);
    let total: u64 = h["bins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap())
        .sum();
    assert!(total > 0, "histogram must be non-empty: {h}");

    // stop drains; the child is gone afterwards
    let rt = c.runtime_dir.clone();
    let (s, v) = c.call("POST", "/api/emulation/stop", Some(json!({})));
    assert_eq!(s, 202, "{v}");
    let stopped = c.wait_state(&["stopped"], STOP_TIMEOUT);
    assert!(absent(&stopped, "pid"));
    assert!(
        process_gone(pid),
        "dpds child {pid} must be gone after stop"
    );
    assert!(
        !rt.join("instance").exists(),
        "the instance dir below {} must be removed after stop",
        rt.display()
    );

    // and it can be started again
    let (s, v) = c.call("POST", "/api/emulation/start", Some(json!({})));
    assert_eq!(s, 202, "{v}");
    c.wait_state(&["running"], START_TIMEOUT);
}

#[test]
#[ignore = "needs target/release/dpds built with --features telemetry; run with -- --ignored"]
fn failed_start_is_reported_with_the_stderr_tail() {
    let _g = serial();
    let c = Controller::start();
    // an argument that passes the controller's allowlist but that EAL rejects
    // (`-n 0`: "invalid memory channel parameter")
    c.configure(
        &["--no-huge", "-m", "256", "--no-pci", "-n", "0"],
        loop_config(),
    );
    let (s, v) = c.call("POST", "/api/emulation/start", Some(json!({})));
    assert_eq!(s, 202, "{v}");
    let v = c.wait_state(&["failed"], START_TIMEOUT);
    assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()), "{v}");
    let tail = v["log_tail"].as_array().unwrap();
    assert!(
        !tail.is_empty(),
        "failed start must carry the child's output: {v}"
    );
    assert!(absent(&v, "pid"));
    // the controller survived and still answers
    assert_eq!(c.call("GET", "/api/online", None).0, 200);
}

#[test]
#[ignore = "needs target/release/dpds built with --features telemetry; run with -- --ignored"]
fn controller_sigterm_stops_the_child_before_exiting() {
    let _g = serial();
    let mut c = Controller::start();
    c.configure(&EAL, loop_config());
    c.call("POST", "/api/emulation/start", Some(json!({})));
    let run = c.wait_state(&["running"], START_TIMEOUT);
    let pid = run["pid"].as_i64().unwrap() as i32;

    unsafe { libc::kill(c.child.id() as i32, libc::SIGTERM) };
    let t0 = Instant::now();
    let status = loop {
        if let Some(s) = c.child.try_wait().unwrap() {
            break s;
        }
        assert!(
            t0.elapsed() < STOP_TIMEOUT,
            "controller did not exit after SIGTERM"
        );
        thread::sleep(Duration::from_millis(100));
    };
    assert!(
        status.success() || status.code().is_some(),
        "controller must exit normally, got {status}"
    );
    assert!(
        process_gone(pid),
        "dpds child {pid} must not outlive the controller"
    );
}
