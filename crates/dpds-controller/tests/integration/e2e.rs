//! End-to-end: the real `dpds-controller` binary supervising the real `dpds` (built with
//! `--features telemetry`) on DPDK virtual devices, driven only through the REST API.
//!
//! The tests are `#[ignore]`d because they need `target/release/dpds` and several CPUs:
//!
//! ```text
//! cargo build --release -p dpds --features telemetry
//! cargo test --release -p dpds-controller --test integration e2e -- --ignored --test-threads=1
//! ```
//!
//! When run, a missing or telemetry-less `dpds` binary or too few CPUs is a FAILURE with a clear
//! message, never a silent skip. `DPDS_BIN` overrides the binary path.
//!
//! The controller runs with a fake sysfs/procfs (online CPUs 0..=n, CPUs 1..=n isolated => main
//! lcore 0, one worker lcore per link) so the assignment does not depend on the test host's boot
//! parameters. Every start carries the whole configuration (`Controller::start_run`) and the
//! running configuration is read back from `GET /api/emulation`.
//!
//! The `virtual_network_*` tests run the controller in virtual network mode over two veth pairs (see
//! `VethLab`) and need root (`CAP_NET_ADMIN`, `CAP_NET_RAW`, and for the ping test `CAP_SYS_ADMIN` for
//! network namespaces) and a `dpds` whose DPDK has
//! `net/af_packet`; without permission to `ip link add` they print SKIP and pass. Build the test
//! binary as the normal user, then run it as root with the environment kept:
//!
//! ```text
//! cargo test --release -p dpds-controller --test integration --no-run      # prints the binary
//! sudo env "PATH=$PATH" "LD_LIBRARY_PATH=$LD_LIBRARY_PATH" target/release/deps/integration-<hash> e2e::virtual_network --ignored --test-threads=1
//! ```
//!
//! The tests use `net_null` hairpin links and a three-link ring over three `net_null` vdevs
//! (`net_null` receives an endless stream of packets and discards what it sends, so exact
//! per-link counts are not available; the per-link deterministic delay, the link order, the ports
//! and the lcores are what these tests pin). The links file handed to `dpds` is checked while
//! running.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::common::*;
use serde_json::{json, Value};

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
    _host: FakeHost,
    runtime_dir: PathBuf,
}

impl Controller {
    fn start() -> Controller {
        Controller::start_with_workers(1)
    }

    /// A controller whose fake host has `workers` isolated cores (1..=workers) besides the
    /// main lcore 0.
    fn start_with_workers(workers: usize) -> Controller {
        require_cpus(workers + 1);
        let host = FakeHost::empty();
        let isolated = if workers == 1 {
            "1".to_string()
        } else {
            format!("1-{workers}")
        };
        host.cpus(workers as u32 + 1, Some(&isolated), &[]);
        host.cmdline(&format!(
            "ro isolcpus={isolated} nohz_full={isolated} rcu_nocbs={isolated}"
        ));
        let host_args = [
            "--sysfs-root".into(),
            host.sysfs().into_os_string(),
            "--procfs-root".into(),
            host.procfs().into_os_string(),
        ];
        Controller::launch(host, &host_args)
    }

    /// A controller in virtual network mode over the real `/sys` and `/proc` (the veth ends are
    /// real interfaces): the stored mode comes from the etc dir of a throwaway fake host.
    fn start_virtual() -> Controller {
        require_cpus(3);
        let host = FakeHost::virtual_net(1, &[]);
        let host_args = ["--etc-dir".into(), host.etc_dir().into_os_string()];
        Controller::launch(host, &host_args)
    }

    fn launch(host: FakeHost, host_args: &[std::ffi::OsString]) -> Controller {
        let dpds = dpds_bin();
        // short path: the telemetry socket path must stay below 108 bytes
        let runtime_dir = PathBuf::from(format!("/tmp/dpdsc-{}", std::process::id()));
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let addr = format!("127.0.0.1:{port}");
        let child = controller_cmd()
            .args(["serve", "--bind", &addr])
            .arg("--dpds-bin")
            .arg(&dpds)
            .arg("--runtime-dir")
            .arg(&runtime_dir)
            .args(host_args)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("cannot spawn dpds-controller");
        let c = Controller {
            child,
            addr,
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

    /// `POST /api/emulation/start` with `links`.
    fn start_run(&self, links: Vec<Value>) -> (u16, Value) {
        let body = json!({ "links": links });
        self.call("POST", "/api/emulation/start", Some(body))
    }

    /// Waits until every link `0..n` has seen packets; returns the last `/api/stats`.
    fn wait_for_traffic(&self, n: usize) -> Value {
        let t0 = Instant::now();
        loop {
            let (s, v) = self.call("GET", "/api/stats", None);
            assert_eq!(s, 200);
            let seen = (0..n)
                .filter(|d| {
                    v["directions"][*d]["counters"]["tx_sent"]
                        .as_u64()
                        .unwrap_or(0)
                        > 0
                })
                .count();
            if seen == n {
                return v;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(30),
                "only {seen} of {n} links forwarded packets: {v}"
            );
            thread::sleep(Duration::from_millis(250));
        }
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

/// The hairpin link `loop` on `net_null0` with `config`.
fn loop_link(config: Value) -> Value {
    json!({"ingress": "net_null0", "egress": "net_null0", "config": config})
}

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
    // a fresh controller has no configuration: nothing is stored, nothing is running
    let idle = c.emulation();
    assert_eq!(idle["state"], "stopped", "{idle}");
    assert!(absent(&idle, "config"), "{idle}");

    let (s, v) = c.start_run(vec![loop_link(loop_config())]);
    assert_eq!(s, 202, "{v}");
    let run = c.wait_state(&["running"], START_TIMEOUT);
    let pid = run["pid"].as_i64().expect("running state has a pid") as i32;
    assert!(pid > 0 && !process_gone(pid));
    assert_eq!(run["directions"].as_array().unwrap().len(), 1);
    assert_eq!(run["directions"][0]["rx"], "net_null0");
    assert_eq!(run["directions"][0]["tx"], "net_null0");
    assert_eq!(run["directions"][0]["lcore"], 1);
    assert_eq!(run["main_lcore"], 0);
    assert!(run.get("pending_restart").is_none(), "{run}");
    // the running configuration is the one the start carried
    assert_eq!(run["config"]["links"][0]["ingress"], "net_null0", "{run}");
    assert!(absent(&run["config"], "settings"), "{run}");

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
        absent(&stopped, "config"),
        "a stopped controller reports no configuration: {stopped}"
    );
    assert!(
        process_gone(pid),
        "dpds child {pid} must be gone after stop"
    );
    assert!(
        !rt.join("instance").exists(),
        "the instance dir below {} must be removed after stop",
        rt.display()
    );

    // and it can be started again, with a body again: the controller kept nothing
    let (s, v) = c.start_run(vec![loop_link(loop_config())]);
    assert_eq!(s, 202, "{v}");
    c.wait_state(&["running"], START_TIMEOUT);
}

#[test]
#[ignore = "needs target/release/dpds built with --features telemetry; run with -- --ignored"]
fn failed_start_is_reported_with_the_stderr_tail() {
    let _g = serial();
    let c = Controller::start();
    // a PCI address no device answers to: the controller accepts it, `dpds` cannot open the port
    let (s, v) = c.start_run(vec![json!({"ingress": "0000:ff:00.0",
                                         "egress": "0000:ff:00.0", "config": loop_config()})]);
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
    c.start_run(vec![loop_link(loop_config())]);
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

/// A deterministic delay of `ms` milliseconds and a small buffer (fits `--no-huge -m 256`).
fn delay_config(ms: f64) -> Value {
    let mut c = loop_config();
    c["delayer"]["mean_ns"] = json!(ms * 1.0e6);
    c
}

fn assert_mean_delay(d: &Value, ms: f64) {
    let mean = f(d, "delay_mean_ns");
    assert!(
        (0.9 * ms * 1e6..1.1 * ms * 1e6).contains(&mean),
        "link {}: deterministic {ms} ms delay, got mean {mean} ns",
        d["dir"]
    );
}

#[test]
#[ignore = "needs target/release/dpds built with --features telemetry; run with -- --ignored"]
fn two_hairpin_links_run_with_their_own_config_and_the_v2_links_file() {
    let _g = serial();
    let c = Controller::start_with_workers(2);
    let (s, v) = c.start_run(vec![
        json!({"ingress": "net_null0", "egress": "net_null0",
                   "config": delay_config(1.0)}),
        json!({"ingress": "net_null1", "egress": "net_null1",
                   "config": delay_config(3.0)}),
    ]);
    assert_eq!(s, 202, "{v}");
    let run = c.wait_state(&["running"], START_TIMEOUT);
    let dirs = run["directions"].as_array().unwrap();
    assert_eq!(dirs.len(), 2, "{run}");
    let mut lcores: Vec<u64> = dirs.iter().map(|d| d["lcore"].as_u64().unwrap()).collect();
    lcores.sort_unstable();
    assert_eq!(lcores, [1, 2], "one worker per link: {run}");

    // the links file dpds was started with: v2, explicit lcores
    let file: Value =
        serde_json::from_slice(&std::fs::read(c.runtime_dir.join("links.json")).unwrap()).unwrap();
    assert_eq!(file["links"].as_array().unwrap().len(), 2, "{file}");
    for (i, port) in ["net_null0", "net_null1"].iter().enumerate() {
        let l = &file["links"][i];
        assert!(l.get("name").is_none(), "{l}");
        assert_eq!(l["ingress"], *port);
        assert_eq!(l["egress"], *port);
        assert_eq!(l["lcore"], dirs[i]["lcore"]);
        assert!(l["config"].is_object(), "{l}");
        assert!(
            l.get("ports").is_none() && l.get("configs").is_none(),
            "{l}"
        );
    }

    let v = c.wait_for_traffic(2);
    thread::sleep(Duration::from_millis(1500));
    let (_, v2) = c.call("GET", "/api/stats", None);
    for d in 0..2 {
        assert!(
            v2["directions"][d]["counters"]["rx_pkts"].as_u64().unwrap()
                > v["directions"][d]["counters"]["rx_pkts"].as_u64().unwrap(),
            "link {d} must keep forwarding: {v2}"
        );
    }
    assert_eq!(v2["directions"][0]["rx"], "net_null0");
    assert_eq!(v2["directions"][0]["tx"], "net_null0");
    assert_eq!(v2["directions"][1]["rx"], "net_null1");
    assert_mean_delay(&v2["directions"][0], 1.0);
    assert_mean_delay(&v2["directions"][1], 3.0);
    for d in 0..2 {
        let (s, h) = c.call("GET", &format!("/api/stats/{d}/delay_histogram"), None);
        assert_eq!(s, 200, "{h}");
        assert_eq!(h["dir"], d);
    }
    let (s, _) = c.call("GET", "/api/stats/2/delay_histogram", None);
    assert_eq!(s, 404, "there are two links, no third histogram");

    assert_eq!(
        c.call("POST", "/api/emulation/stop", Some(json!({}))).0,
        202
    );
    c.wait_state(&["stopped"], STOP_TIMEOUT);
}

#[test]
#[ignore = "needs target/release/dpds built with --features telemetry; run with -- --ignored"]
fn a_three_link_ring_over_three_vdevs_runs_one_worker_per_link() {
    let _g = serial();
    let c = Controller::start_with_workers(3);
    // A -> B, B -> C, C -> A: each port is ingress once and egress once
    let ring =
        |i: &str, e: &str, ms: f64| json!({"ingress": i, "egress": e, "config": delay_config(ms)});
    let (s, v) = c.start_run(vec![
        ring("net_null0", "net_null1", 1.0),
        ring("net_null1", "net_null2", 2.0),
        ring("net_null2", "net_null0", 3.0),
    ]);
    assert_eq!(s, 202, "{v}");
    let run = c.wait_state(&["running"], START_TIMEOUT);
    let dirs = run["directions"].as_array().unwrap();
    assert_eq!(dirs.len(), 3, "{run}");
    let mut lcores: Vec<u64> = dirs.iter().map(|d| d["lcore"].as_u64().unwrap()).collect();
    lcores.sort_unstable();
    assert_eq!(lcores, [1, 2, 3], "{run}");

    c.wait_for_traffic(3);
    thread::sleep(Duration::from_millis(1500));
    let (_, v) = c.call("GET", "/api/stats", None);
    let ring = [
        ("net_null0", "net_null1", 1.0),
        ("net_null1", "net_null2", 2.0),
        ("net_null2", "net_null0", 3.0),
    ];
    assert_eq!(v["directions"].as_array().unwrap().len(), 3, "{v}");
    for (d, (rx, tx, ms)) in ring.iter().enumerate() {
        let x = &v["directions"][d];
        assert_eq!(x["dir"], d);
        assert_eq!(x["rx"], *rx, "{x}");
        assert_eq!(x["tx"], *tx, "{x}");
        assert!(x["counters"]["rx_pkts"].as_u64().unwrap() > 0, "{x}");
        assert_mean_delay(x, *ms);
        let (s, h) = c.call("GET", &format!("/api/stats/{d}/delay_histogram"), None);
        assert_eq!(s, 200, "{h}");
    }
    // /api/stats lists no ports
    assert!(v.get("ports").is_none(), "{v}");

    // the links file carries the ring literally
    let file: Value =
        serde_json::from_slice(&std::fs::read(c.runtime_dir.join("links.json")).unwrap()).unwrap();
    for (i, (rx, tx, _)) in ring.iter().enumerate() {
        assert_eq!(file["links"][i]["ingress"], *rx);
        assert_eq!(file["links"][i]["egress"], *tx);
    }

    assert_eq!(
        c.call("POST", "/api/emulation/stop", Some(json!({}))).0,
        202
    );
    c.wait_state(&["stopped"], STOP_TIMEOUT);
}

#[test]
#[ignore = "needs target/release/dpds built with --features telemetry; run with -- --ignored"]
fn a_link_without_any_stage_runs_with_zero_delay() {
    // optional stages: the missing delayer, limiter and loss model become the no-op core
    // stages in the links file, and the real dpds accepts and runs them (the internal
    // `initialization_rate_pps` of 1 is valid for it and never used without correlation)
    let _g = serial();
    let c = Controller::start();
    let bare = json!({"buffer_capacity": 4096, "reordering": false});
    let (s, v) = c.start_run(vec![loop_link(bare.clone())]);
    assert_eq!(s, 202, "{v}");
    let run = c.wait_state(&["running"], START_TIMEOUT);
    // the running configuration is the request as accepted: no stage appears
    let cfg = &run["config"]["links"][0]["config"];
    assert!(
        absent(cfg, "delayer") && absent(cfg, "limiter") && absent(cfg, "loss_model"),
        "{run}"
    );
    let file: Value =
        serde_json::from_slice(&std::fs::read(c.runtime_dir.join("links.json")).unwrap()).unwrap();
    assert_eq!(
        file["links"][0]["config"]["limiter"]["activate"], false,
        "{file}"
    );
    assert_eq!(
        file["links"][0]["config"]["delayer"]["mean_ns"], 0.0,
        "{file}"
    );
    let v = c.wait_for_traffic(1);
    let d0 = &v["directions"][0];
    // the measured delay includes the TX staging wait (up to TX_MAX_WAIT_NS = 10 us in
    // crates/dpds/src/consts.rs) and the loop latency of a shared CI runner
    assert!(
        f(d0, "delay_mean_ns") < 2.0 * 10_000.0,
        "zero intended delay: {d0}"
    );
    assert_eq!(
        d0["counters"]["loss_drops"].as_u64().unwrap_or(0),
        0,
        "{d0}"
    );
    assert_eq!(
        c.call("POST", "/api/emulation/stop", Some(json!({}))).0,
        202
    );
    c.wait_state(&["stopped"], STOP_TIMEOUT);
}

// ---- virtual network mode over veth pairs ----------------------------------------------------

/// `ip <args>`: stdout, or stderr of a failure.
fn ip(args: &[&str]) -> Result<String, String> {
    let o = Command::new("ip")
        .args(args)
        .output()
        .map_err(|e| format!("cannot run ip: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).into_owned())
    }
}

/// The veth pairs of a virtual network test: `vdpds0` and `vdpds1` in the root namespace carry no
/// address and no route, so the controller offers them. With `hosts`, `vdpds0` is paired with
/// `eth0` of the namespace `vdpds-a` (10.77.0.1/24) and `vdpds1` with `eth0` of `vdpds-b`
/// (10.77.0.2/24), two hosts to ping each other through the links; without, the peers `vdpds0p`
/// and `vdpds1p` stay in the root namespace (no network namespaces needed). Dropping removes
/// everything again, also after a panic.
struct VethLab;

impl VethLab {
    /// None (after printing SKIP) when this process may not create network interfaces or, with
    /// `hosts`, network namespaces (a container without `CAP_SYS_ADMIN` cannot).
    fn create(hosts: bool) -> Option<VethLab> {
        VethLab::remove();
        let permission = |e: &str| e.contains("not permitted") || e.contains("Permission denied");
        if let Err(e) = ip(&[
            "link",
            "add",
            "vdpds-probe",
            "type",
            "veth",
            "peer",
            "name",
            "vdpds-probe2",
        ]) {
            if permission(&e) {
                eprintln!("SKIP: `ip link add` is not permitted ({}); run as root with CAP_NET_ADMIN and CAP_NET_RAW, e.g. under sudo", e.trim());
                return None;
            }
            panic!("ip link add failed: {e}");
        }
        let _ = ip(&["link", "del", "vdpds-probe"]);
        let lab = VethLab;
        for (ns, root_end, addr) in [
            ("vdpds-a", "vdpds0", "10.77.0.1/24"),
            ("vdpds-b", "vdpds1", "10.77.0.2/24"),
        ] {
            let step = |args: &[&str]| {
                ip(args).unwrap_or_else(|e| panic!("ip {args:?}: {e}"));
            };
            if hosts {
                match ip(&["netns", "add", ns]) {
                    Ok(_) => {}
                    Err(e) if permission(&e) => {
                        eprintln!("SKIP: network namespaces are not permitted ({}); this test needs CAP_SYS_ADMIN", e.trim());
                        return None;
                    }
                    Err(e) => panic!("ip netns add {ns}: {e}"),
                }
                step(&[
                    "link", "add", root_end, "type", "veth", "peer", "name", "eth0", "netns", ns,
                ]);
                step(&[
                    "netns", "exec", ns, "ip", "addr", "add", addr, "dev", "eth0",
                ]);
                step(&["netns", "exec", ns, "ip", "link", "set", "eth0", "up"]);
                step(&["netns", "exec", ns, "ip", "link", "set", "lo", "up"]);
            } else {
                let peer = format!("{root_end}p");
                step(&[
                    "link", "add", root_end, "type", "veth", "peer", "name", &peer,
                ]);
                step(&["link", "set", &peer, "up"]);
            }
            step(&["link", "set", root_end, "up"]);
            // no IPv6 noise on the root ends (best effort)
            let _ = std::fs::write(
                format!("/proc/sys/net/ipv6/conf/{root_end}/disable_ipv6"),
                "1",
            );
        }
        Some(lab)
    }

    fn remove() {
        for ns in ["vdpds-a", "vdpds-b"] {
            let _ = ip(&["netns", "del", ns]);
        }
        for dev in ["vdpds0", "vdpds1", "vdpds-probe"] {
            let _ = ip(&["link", "del", dev]);
        }
    }
}

impl Drop for VethLab {
    fn drop(&mut self) {
        VethLab::remove();
    }
}

/// The two links of the lab, `vdpds0` -> `vdpds1` and back, with a deterministic `ms` delay.
fn lab_links(ms: f64, buffer_capacity: u64) -> Vec<Value> {
    let mut cfg = delay_config(ms);
    cfg["buffer_capacity"] = json!(buffer_capacity);
    vec![
        json!({"ingress": "vdpds0", "egress": "vdpds1", "config": cfg.clone()}),
        json!({"ingress": "vdpds1", "egress": "vdpds0", "config": cfg}),
    ]
}

/// The first number of the `rtt min/avg/max/mdev = a/b/c/d ms` line of ping, in ms.
fn ping_min_rtt_ms(out: &str) -> f64 {
    let line = out
        .lines()
        .find(|l| l.contains("min/avg/max"))
        .unwrap_or_else(|| panic!("no rtt summary in the ping output: {out}"));
    let after = line.split('=').nth(1).expect("rtt values");
    after
        .trim()
        .split('/')
        .next()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("cannot parse the rtt line {line:?}"))
}

#[test]
#[ignore = "needs root (CAP_NET_ADMIN, CAP_NET_RAW) and target/release/dpds built with --features telemetry and DPDK net/af_packet; see the header of this file"]
fn virtual_network_ping_through_two_links_sees_the_deterministic_delay() {
    let _g = serial();
    let Some(_lab) = VethLab::create(true) else {
        return;
    };
    let c = Controller::start_virtual();
    let online = c.call("GET", "/api/online", None).1;
    assert_eq!(online["network"], "virtual", "{online}");
    // the veth ends are offered by name, the namespaces' ends are not even visible
    let (s, ifaces) = c.call("GET", "/api/interfaces", None);
    assert_eq!(s, 200);
    for name in ["vdpds0", "vdpds1"] {
        let i = ifaces
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["port"] == name)
            .unwrap_or_else(|| panic!("{name} not offered: {ifaces}"));
        assert_eq!(i["dpds_usable"], true, "{i}");
    }

    let (s, v) = c.start_run(lab_links(20.0, 4096));
    assert_eq!(s, 202, "{v}");
    let run = c.wait_state(&["running"], START_TIMEOUT);
    // the user sees interface names in the configuration, DPDK devices in the plan
    assert_eq!(run["config"]["links"][0]["ingress"], "vdpds0", "{run}");
    assert_eq!(run["directions"][0]["rx"], "net_af_packet0", "{run}");
    assert_eq!(run["directions"][0]["tx"], "net_af_packet1", "{run}");

    let ping = Command::new("ip")
        .args([
            "netns",
            "exec",
            "vdpds-a",
            "ping",
            "-c",
            "3",
            "-W",
            "2",
            "10.77.0.2",
        ])
        .output()
        .expect("cannot run ping");
    let out = String::from_utf8_lossy(&ping.stdout).into_owned();
    assert!(
        ping.status.success(),
        "ping failed: {out} {}",
        String::from_utf8_lossy(&ping.stderr)
    );
    let rtt = ping_min_rtt_ms(&out);
    assert!(
        rtt >= 40.0,
        "two 20 ms links must add at least 40 ms, got {rtt} ms: {out}"
    );

    let (s, stats) = c.call("GET", "/api/stats", None);
    assert_eq!(s, 200);
    for d in 0..2 {
        let counters = &stats["directions"][d]["counters"];
        assert!(
            counters["rx_pkts"].as_u64().unwrap_or(0) > 0,
            "link {d} saw no packet: {stats}"
        );
        assert!(
            counters["tx_sent"].as_u64().unwrap_or(0) > 0,
            "link {d} sent no packet: {stats}"
        );
    }
    assert_eq!(
        stats["directions"][0]["rx"], "vdpds0",
        "stats name the interfaces: {stats}"
    );
    let (s, hist) = c.call("GET", "/api/stats/history", None);
    assert_eq!(s, 200);
    assert_eq!(hist["links"][0]["ingress"], "vdpds0", "{hist}");

    assert_eq!(
        c.call("POST", "/api/emulation/stop", Some(json!({}))).0,
        202
    );
    c.wait_state(&["stopped"], STOP_TIMEOUT);
}

#[test]
#[ignore = "needs root (CAP_NET_ADMIN, CAP_NET_RAW) and target/release/dpds built with --features telemetry and DPDK net/af_packet; see the header of this file"]
fn virtual_network_memory_formula_fits_a_large_buffer() {
    // 2^18 packets: with `--no-huge` the mbuf pool takes 1059 MiB (a 4 KiB page per mbuf), so a
    // `-m` computed from the 2432 B mbuf size alone fails the start with a pool error. A hairpin:
    // an egress-only port would add an RX ring of mbufs.
    let _g = serial();
    let Some(_lab) = VethLab::create(false) else {
        return;
    };
    let c = Controller::start_virtual();
    let capacity: u64 = 1 << 18;
    let mut cfg = delay_config(1.0);
    cfg["buffer_capacity"] = json!(capacity);
    let (s, v) = c.start_run(vec![
        json!({"ingress": "vdpds0", "egress": "vdpds0", "config": cfg}),
    ]);
    assert_eq!(s, 202, "{v}");
    let run = c.wait_state(&["running"], START_TIMEOUT);
    let pid = run["pid"].as_i64().expect("a running state has a pid");
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).expect("cmdline of dpds");
    let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
    let m = args
        .windows(2)
        .find(|w| w[0] == b"-m")
        .map(|w| String::from_utf8_lossy(w[1]).into_owned())
        .unwrap_or_else(|| panic!("no -m in the dpds command line"));
    assert_eq!(m, virtual_mem_mib(&[capacity]).to_string());
    assert_eq!(
        c.call("POST", "/api/emulation/stop", Some(json!({}))).0,
        202
    );
    c.wait_state(&["stopped"], STOP_TIMEOUT);
}
