//! Spec 29: server-side chart history, `GET /api/stats/history[?since_ms=<t>]`.
//!
//! The controller keeps an in-memory per-second series per link for the current run (runtime
//! data, not stored configuration), so the dashboard charts survive navigation and reload.
//!
//! # Contract the tests pin (the spec leaves some of it open; the choices are the tester's)
//!
//! - `GET /api/stats/history` answers 200 `{"started_at": <unix seconds or null>, "samples":
//!   {"<link index>": [sample, ...]}}`. The key is the link index in the order of the start body
//!   ("0", "1", ...), like `{dir}` of the histogram route. `started_at` is the run's start as
//!   `/api/emulation` reports it. Nothing ran since the controller started: `started_at` null (or
//!   absent) and `samples` `{}`.
//! - A sample is `{t_ms, rx_gbps, tx_gbps, rx_mpps, tx_mpps, loss_pct, limiter_pct,
//!   buffer_full_pct, nic_tx_pct, nic_rx_pct}`, all numbers. `t_ms` is a whole number of
//!   milliseconds since the start of the run, strictly increasing (u64, never a fraction). The rates are `/api/stats`' `rx_bps / 1e9`, `tx_bps / 1e9`,
//!   `rx_pps / 1e6`, `tx_pps / 1e6`; the five shares are `/api/stats`' drop shares of the same
//!   names, in percent, one per cause (the frontend sums the overload causes).
//! - At most one sample per link per second, taken from the poller's snapshots with no extra
//!   telemetry query; every link gets a sample at the same `t_ms`.
//! - `since_ms` (a u64: digits only, no sign, fraction, exponent or hex) returns only samples with
//!   `t_ms > since_ms`; the key of a known link stays, with `[]` when nothing is newer. Without
//!   `since_ms` everything is returned. A bad `since_ms` or any other query parameter is a 400
//!   `{"message"}` (the message of a bad `since_ms` names `since_ms`).
//! - Retention: the whole run up to `HISTORY_MAX_SAMPLES` (86400) samples per link, the oldest
//!   dropped first. The series is cleared when a new run starts (a new start request, with a new
//!   pid and start time) and kept after stop and after telemetry failures until then.
//! - Same guards as the other routes: Host allow-list 421, wrong method 405, documented in the
//!   OpenAPI document.
//!
//! # Second round (user 2026-11-04): relative timestamps, like P4TG
//!
//! - `GET /api/stats` has `elapsed_ms` (u64, whole milliseconds) since the run started, from a monotonic clock
//!   (`Instant`) taken when the run starts. Null or absent while nothing has run. Frozen at the stop
//!   instant after a stop (the same value on every later read), also in the stopped payload with
//!   no directions. A new run restarts it near 0. It keeps counting when the stats cache is
//!   cleared by failed polls (the run is still going). OpenAPI lists it on `StatsView`.
//! - The history's `t_ms` is measured from that same clock: the newest `t_ms` is never above
//!   `elapsed_ms` read afterwards, and at most about one sampling period below it.
//! - Tolerance: the clock may start in the start handler or at the poller's first look at the
//!   running state (up to one poll interval later), so the tests allow 0.6 s below and 0.3 s above
//!   the wall time they measured around the start request.
//!
//! # NEW Rust API the tests assume (ASSUMPTION: the spec names none)
//!
//! ```ignore
//! // dpds_controller::telemetry
//! pub const HISTORY_MAX_SAMPLES: usize = 86_400;
//! impl Poller {
//!     /// Like `Poller::spawn`, with the per-link retention cap of the series.
//!     pub fn spawn_with_history_cap(runtime_dir: PathBuf, supervisor: Arc<dyn Supervisor>,
//!                                   max_samples: usize) -> Arc<Poller>;
//! }
//! ```
//!
//! `Poller::spawn` uses `HISTORY_MAX_SAMPLES`. The REST route reaches the series through
//! `TelemetrySource` (whatever method the implementation adds); a source that records nothing, like
//! `FakeTelemetry`, gives the empty answer. The tests run the real `Poller` against a fake
//! `dpds` socket whose counters grow at constant rates, so every poll (and every sample) shows the
//! same rates, and use real time: one second per sample, a few seconds per test.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::common::*;
use crate::fake_telemetry::{hist_page, short_tempdir, FakeTelemetryServer, Mode};
use dpds_controller::api::{self, Deps};
use dpds_controller::supervisor::{LaunchPlan, State, Supervisor, SupervisorStatus};
use dpds_controller::telemetry::{socket_path, Poller, TelemetrySource, HISTORY_MAX_SAMPLES};
use serde_json::{json, Value};

const SAMPLE_FIELDS: [&str; 10] = [
    "t_ms",
    "rx_gbps",
    "tx_gbps",
    "rx_mpps",
    "tx_mpps",
    "loss_pct",
    "limiter_pct",
    "buffer_full_pct",
    "nic_tx_pct",
    "nic_rx_pct",
];

/// Poll interval of every rig below.
const POLL_MS: u64 = 100;

// ---- a fake dpds whose counters grow at constant rates -----------------------------------------

/// What link 0 and link 1 do, per second. Different numbers everywhere, so a mixed-up link, port
/// or unit shows.
struct Expect {
    rx_gbps: f64,
    tx_gbps: f64,
    rx_mpps: f64,
    tx_mpps: f64,
    loss_pct: f64,
    limiter_pct: f64,
    buffer_full_pct: f64,
    nic_tx_pct: f64,
    nic_rx_pct: f64,
}

const EXPECT: [Expect; 2] = [
    Expect {
        rx_gbps: 8.0,
        tx_gbps: 7.6,
        rx_mpps: 2.0,
        tx_mpps: 1.9,
        loss_pct: 2.0,
        limiter_pct: 1.0,
        buffer_full_pct: 0.5,
        nic_tx_pct: 0.25,
        nic_rx_pct: 0.15,
    },
    Expect {
        rx_gbps: 4.0,
        tx_gbps: 3.2,
        rx_mpps: 1.0,
        tx_mpps: 0.99,
        loss_pct: 0.5,
        limiter_pct: 0.0,
        buffer_full_pct: 4.0,
        nic_tx_pct: 0.0,
        nic_rx_pct: 0.0,
    },
];

/// The snapshot of `dpds` after `uptime_ns` of running: link 0 is port 0 -> port 1, link 1 is
/// port 1 -> port 0, counters = rate * uptime (rounded down).
fn at(uptime_ns: u64) -> dpds_controller::stats::Snapshot {
    let secs = uptime_ns as f64 / 1e9;
    let n = |rate_per_s: f64| (rate_per_s * secs) as u64;
    let dir = |rx_pps: f64, tx_pps: f64, loss: f64, lim: f64, full: f64, txd: f64| DirSample {
        uptime_ns,
        rx_pkts: n(rx_pps),
        tx_sent: n(tx_pps),
        loss: n(rx_pps * loss / 100.0),
        limiter: n(rx_pps * lim / 100.0),
        full: n(rx_pps * full / 100.0),
        tx_drops: n(rx_pps * txd / 100.0),
    };
    let (a, b) = (&EXPECT[0], &EXPECT[1]);
    let d0 = dir(
        a.rx_mpps * 1e6,
        a.tx_mpps * 1e6,
        a.loss_pct,
        a.limiter_pct,
        a.buffer_full_pct,
        a.nic_tx_pct,
    );
    let d1 = dir(
        b.rx_mpps * 1e6,
        b.tx_mpps * 1e6,
        b.loss_pct,
        b.limiter_pct,
        b.buffer_full_pct,
        b.nic_tx_pct,
    );
    // imissed 0.1 % and rx_nombuf 0.05 % of link 0's packets: 0.15 % together
    let miss0 = a.rx_mpps * 1e6;
    let bytes = |gbps: f64| n(gbps * 1e9 / 8.0);
    snapshot(
        [d0, d1],
        [
            // port 0: link 0 receives, link 1 sends
            PortSample {
                ibytes: bytes(a.rx_gbps),
                obytes: bytes(b.tx_gbps),
                imissed: n(miss0 * 0.001),
                rx_nombuf: n(miss0 * 0.0005),
            },
            // port 1: link 1 receives, link 0 sends
            PortSample {
                ibytes: bytes(b.rx_gbps),
                obytes: bytes(a.tx_gbps),
                imissed: 0,
                rx_nombuf: 0,
            },
        ],
    )
}

/// A fake `dpds` socket. Its uptime is the time since `restart_clock` (or `live`); the clock is
/// read once per poll (at `/dpds/ports`), so all replies of a poll belong together.
struct Live {
    server: FakeTelemetryServer,
    t0: Arc<Mutex<Instant>>,
}

impl Live {
    fn restart_clock(&self) {
        *self.t0.lock().unwrap() = Instant::now();
    }
}

fn live(runtime_dir: &Path, pid: u32) -> Live {
    let t0 = Arc::new(Mutex::new(Instant::now()));
    let latched = Arc::new(AtomicU64::new(1));
    let hists = full_hists(&at(1));
    let t = t0.clone();
    let server = FakeTelemetryServer::start(
        &socket_path(runtime_dir),
        pid,
        vec![Mode::Serve],
        move |cmd| {
            let (name, param) = cmd.split_once(',').unwrap_or((cmd, ""));
            if name == "/dpds/ports" {
                let ns = t.lock().unwrap().elapsed().as_nanos() as u64;
                latched.store(ns.max(1), Ordering::SeqCst);
            }
            let snap = at(latched.load(Ordering::SeqCst));
            let idx: usize = param.parse().unwrap_or(usize::MAX);
            match name {
                "/dpds/ports" => Some(snap.ports),
                "/dpds/stats" => snap.dirs.get(idx).cloned(),
                "/dpds/delay_hist" => hist_page(&hists, param),
                "/ethdev/stats" => snap.eth.get(idx).cloned(),
                _ => None,
            }
        },
    );
    Live { server, t0 }
}

// ---- the rig: real Poller, fake dpds, fake supervisor, real router -------------------------------

struct Rig {
    router: axum::Router,
    supervisor: Arc<FakeSupervisor>,
    live: Option<Live>,
    runtime_dir: PathBuf,
    _host: FakeHost,
    _poller: Arc<Poller>,
    _dir: tempfile::TempDir,
}

fn rig_with(cap: Option<usize>) -> Rig {
    let rt = short_tempdir();
    let runtime_dir: PathBuf = rt.path().to_path_buf();
    let supervisor = FakeSupervisor::new();
    let poller = match cap {
        Some(c) => Poller::spawn_with_history_cap(runtime_dir.clone(), supervisor.clone(), c),
        None => Poller::spawn(runtime_dir.clone(), supervisor.clone()),
    };
    let host = FakeHost::bare_metal();
    let router = api::router(Deps {
        roots: host.roots(),
        supervisor: supervisor.clone(),
        telemetry: poller.clone(),
        dpds_bin: PathBuf::from("/usr/local/bin/dpds"),
        runtime_dir: runtime_dir.clone(),
        dpds_has_telemetry: true,
        is_root: false,
    });
    Rig {
        router,
        supervisor,
        live: None,
        runtime_dir,
        _host: host,
        _poller: poller,
        _dir: rt,
    }
}

fn rig() -> Rig {
    rig_with(None)
}

impl Rig {
    /// A fake `dpds` with `pid` goes up and the REST start is called: a new run.
    async fn start_run(&mut self, pid: u32, started_at: u64) {
        self.start_run_links(
            pid,
            started_at,
            vec![link("l", "net_null0", "net_null0", None)],
        )
        .await;
    }

    /// Like `start_run`, with the links of the start body (the fake `dpds` always reports two
    /// directions, whatever the body has).
    async fn start_run_links(&mut self, pid: u32, started_at: u64, links: Vec<Value>) {
        self.live = None; // the old socket goes first
        self.live = Some(live(&self.runtime_dir, pid));
        self.supervisor.set_next_run(pid, started_at);
        let s = setup(links);
        let mut body = s.body();
        body["settings"]["poll_interval_ms"] = json!(POLL_MS);
        let r = call(&self.router, "POST", "/api/emulation/start", Some(body)).await;
        assert_eq!(r.status, 202, "{}", r.text());
        if let Some(l) = &self.live {
            l.restart_clock();
        }
    }

    async fn stop_run(&self) {
        let r = call(&self.router, "POST", "/api/emulation/stop", Some(json!({}))).await;
        assert_eq!(r.status, 202, "{}", r.text());
    }

    async fn history(&self, query: &str) -> Value {
        let r = router_get(&self.router, &format!("/api/stats/history{query}")).await;
        assert_eq!(r.status, 200, "{query}: {}", r.text());
        r.json()
    }

    async fn stats(&self) -> Value {
        let r = router_get(&self.router, "/api/stats").await;
        assert_eq!(r.status, 200);
        r.json()
    }

    /// Polls the history until `pred` holds.
    async fn wait_history(&self, what: &str, secs: u64, pred: impl Fn(&Value) -> bool) -> Value {
        let t0 = Instant::now();
        loop {
            let v = self.history("").await;
            if pred(&v) {
                return v;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(secs),
                "timed out after {secs} s waiting for {what}; last answer {v}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn samples(v: &Value, link: &str) -> Vec<Value> {
    v["samples"][link]
        .as_array()
        .unwrap_or_else(|| panic!("samples[{link}] is not an array: {v}"))
        .clone()
}

/// `t_ms` of a sample, an integer number of milliseconds.
fn t_ms(s: &Value) -> u64 {
    s["t_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("sample without an integer t_ms: {s}"))
}

/// `t_ms` of a sample in seconds.
fn t_secs(s: &Value) -> f64 {
    t_ms(s) as f64 / 1000.0
}

/// The `t_ms` values of a link, in seconds (the tests think in seconds; the wire has whole ms).
fn ts(v: &Value, link: &str) -> Vec<f64> {
    samples(v, link).iter().map(t_secs).collect()
}

/// The newest `t_ms` of link 0 in seconds; -1 while it has no samples (or no series at all yet).
fn newest_t(v: &Value) -> f64 {
    v["samples"]["0"]
        .as_array()
        .and_then(|a| a.last())
        .map_or(-1.0, t_secs)
}

/// `t_ms` of the newest sample of link 0 in ms; panics without samples.
fn newest_ms(v: &Value) -> u64 {
    t_ms(
        v["samples"]["0"]
            .as_array()
            .and_then(|a| a.last())
            .unwrap_or_else(|| panic!("no samples: {v}")),
    )
}

/// Close within 2 % (or 0.02 absolute for values near 0): the fake's counters are whole numbers.
fn near(actual: f64, expected: f64, what: &str, sample: &Value) {
    let tol = (expected.abs() * 0.02).max(0.02);
    assert!(
        (actual - expected).abs() <= tol,
        "{what}: {actual} is not {expected} (+-{tol}) in {sample}"
    );
}

fn field(s: &Value, key: &str) -> f64 {
    s[key]
        .as_f64()
        .unwrap_or_else(|| panic!("{key} is not a number in {s}"))
}

// ---- constants and the empty answer ------------------------------------------------------------

#[test]
fn the_cap_is_24_hours_at_one_second() {
    assert_eq!(HISTORY_MAX_SAMPLES, 86_400);
}

#[tokio::test]
async fn nothing_ran_gives_no_samples() {
    // a source that records nothing (the common fake)
    let a = TestApp::new(FakeHost::bare_metal());
    let r = a.get("/api/stats/history").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert!(v.get("started_at").is_none_or(Value::is_null), "{v}");
    assert_eq!(v["samples"], json!({}), "{v}");

    // the real poller, never started
    let r = rig();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let v = r.history("").await;
    assert!(v.get("started_at").is_none_or(Value::is_null), "{v}");
    assert_eq!(v["samples"], json!({}), "{v}");
    let v = r.history("?since_ms=0").await;
    assert_eq!(v["samples"], json!({}), "{v}");
}

// ---- one sample per link per second ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_sample_per_link_per_second_at_the_same_times() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    // the poll interval is 100 ms, so a sample per poll would be ten times too many
    let v = r
        .wait_history("a sample at 3 s", 15, |v| newest_t(v) >= 3.0)
        .await;
    assert_eq!(v["started_at"], json!(1_700_000_000), "{v}");
    let keys: Vec<&String> = v["samples"].as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 2, "a series per link, {v}");
    let (t0, t1) = (ts(&v, "0"), ts(&v, "1"));
    assert_eq!(t0, t1, "both links are sampled at the same times: {v}");
    assert!(
        t0.len() >= 3 && t0.len() <= 5,
        "3 s of a run gives 3 to 5 samples, got {}: {t0:?}",
        t0.len()
    );
    assert!(
        t0[0] >= 0.0 && t0[0] < 2.0,
        "the first sample is near 0: {t0:?}"
    );
    for w in t0.windows(2) {
        assert!(
            w[1] - w[0] >= 0.9 && w[1] - w[0] <= 2.5,
            "one sample per second, got {t0:?}"
        );
    }
    for link in ["0", "1"] {
        for s in samples(&v, link) {
            for key in SAMPLE_FIELDS {
                assert!(s[key].is_number(), "{key} must be a number in {s}");
            }
            assert!(s["t_ms"].is_u64(), "t_ms is a whole number of ms in {s}");
            assert!(s.get("t_s").is_none(), "the old t_s is gone: {s}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_samples_equal_the_rates_and_shares_of_api_stats() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    let v = r
        .wait_history("a sample at 3 s", 15, |v| newest_t(v) >= 3.0)
        .await;
    let stats = r.stats().await;
    let dirs = stats["directions"].as_array().unwrap();
    assert_eq!(dirs.len(), 2, "{stats}");
    for (i, link) in ["0", "1"].iter().enumerate() {
        let want = &EXPECT[i];
        let d = &dirs[i];
        let mut checked = 0;
        for s in samples(&v, link) {
            // a sample before the first rate window (t < 1 s) may be all zero
            if t_secs(&s) < 1.0 {
                continue;
            }
            checked += 1;
            // the configured rates, in the units of the charts
            near(field(&s, "rx_gbps"), want.rx_gbps, "rx_gbps", &s);
            near(field(&s, "tx_gbps"), want.tx_gbps, "tx_gbps", &s);
            near(field(&s, "rx_mpps"), want.rx_mpps, "rx_mpps", &s);
            near(field(&s, "tx_mpps"), want.tx_mpps, "tx_mpps", &s);
            near(field(&s, "loss_pct"), want.loss_pct, "loss_pct", &s);
            near(
                field(&s, "limiter_pct"),
                want.limiter_pct,
                "limiter_pct",
                &s,
            );
            near(
                field(&s, "buffer_full_pct"),
                want.buffer_full_pct,
                "buffer_full_pct",
                &s,
            );
            near(field(&s, "nic_tx_pct"), want.nic_tx_pct, "nic_tx_pct", &s);
            near(field(&s, "nic_rx_pct"), want.nic_rx_pct, "nic_rx_pct", &s);
            // and what /api/stats reports now (constant rates: the same at every instant)
            near(
                field(&s, "rx_gbps"),
                field(d, "rx_bps") / 1e9,
                "rx_gbps vs stats",
                &s,
            );
            near(
                field(&s, "tx_gbps"),
                field(d, "tx_bps") / 1e9,
                "tx_gbps vs stats",
                &s,
            );
            near(
                field(&s, "rx_mpps"),
                field(d, "rx_pps") / 1e6,
                "rx_mpps vs stats",
                &s,
            );
            near(
                field(&s, "tx_mpps"),
                field(d, "tx_pps") / 1e6,
                "tx_mpps vs stats",
                &s,
            );
            for (key, stat) in [
                ("loss_pct", "loss_pct"),
                ("limiter_pct", "limiter_pct"),
                ("buffer_full_pct", "buffer_full_pct"),
                ("nic_tx_pct", "nic_tx_pct"),
                ("nic_rx_pct", "nic_rx_pct"),
            ] {
                near(field(&s, key), field(d, stat), key, &s);
            }
        }
        assert!(
            checked >= 2,
            "link {link}: only {checked} samples at t >= 1 s: {v}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recording_the_series_asks_dpds_for_nothing_extra() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    r.wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    for _ in 0..20 {
        r.history("").await;
    }
    let queries = r.live.as_ref().unwrap().server.queries();
    let known = [
        "/dpds/ports",
        "/dpds/stats,0",
        "/dpds/stats,1",
        "/dpds/delay_hist,0",
        "/dpds/delay_hist,1",
        "/ethdev/stats,0",
        "/ethdev/stats,1",
    ];
    for q in &queries {
        assert!(
            known.iter().any(|k| q == k),
            "{q:?} is a query a poll does not make; all: {queries:?}"
        );
    }
    let count = |c: &str| queries.iter().filter(|q| q.as_str() == c).count();
    let polls = count("/dpds/ports");
    assert!(polls >= 10, "{polls} polls in 2 s at 100 ms");
    assert!(
        count("/dpds/stats,0").abs_diff(polls) <= 1
            && count("/ethdev/stats,1").abs_diff(polls) <= 1,
        "one set of queries per poll: {queries:?}"
    );
}

// ---- since_ms -------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn since_ms_gives_only_newer_samples() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    let all = r
        .wait_history("a sample at 4 s", 20, |v| newest_t(v) >= 4.0)
        .await;
    let t = ts(&all, "0");
    assert!(t.len() >= 4, "{t:?}");
    let mid = t_ms(&samples(&all, "0")[1]);

    // strictly newer, for both links, and exactly the tail of the full answer
    let tail = r.history(&format!("?since_ms={mid}")).await;
    for link in ["0", "1"] {
        let want: Vec<Value> = samples(&all, link)
            .into_iter()
            .filter(|s| t_ms(s) > mid)
            .collect();
        let got = samples(&tail, link);
        assert!(!got.is_empty(), "samples after {mid} exist: {tail}");
        assert!(
            got.iter().all(|s| t_ms(s) > mid),
            "since_ms = {mid} must exclude t_ms <= {mid}: {tail}"
        );
        // the full answer was read before: every sample of it newer than `mid` is in the tail
        // unchanged (the tail may hold newer ones)
        assert_eq!(&got[..want.len()], &want[..], "link {link}");
    }
    assert_eq!(tail["started_at"], json!(1_700_000_000), "{tail}");

    // since_ms = 0 leaves out a sample at exactly 0 and nothing else
    let from0 = r.history("?since_ms=0").await;
    assert!(ts(&from0, "0").iter().all(|t| *t > 0.0), "{from0}");
    assert!(ts(&from0, "0").len() >= t.iter().filter(|t| **t > 0.0).count());

    // without since_ms everything, from the first sample on
    assert_eq!(ts(&r.history("").await, "0")[..t.len()], t[..]);

    // one millisecond before a sample still excludes the ones before it and includes it
    let sample = t_ms(&samples(&all, "0")[2]);
    let just_before = r.history(&format!("?since_ms={}", sample - 1)).await;
    assert_eq!(
        t_ms(&samples(&just_before, "0")[0]),
        sample,
        "{just_before}"
    );
    let at = r.history(&format!("?since_ms={sample}")).await;
    assert!(
        samples(&at, "0").iter().all(|s| t_ms(s) > sample),
        "a sample at exactly since_ms is not newer: {at}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incremental_answers_add_up_to_the_full_history() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    let first = r
        .wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    let last = newest_ms(&first);
    // the next increment, once a newer sample exists
    let t0 = Instant::now();
    let inc = loop {
        let v = r.history(&format!("?since_ms={last}")).await;
        if !samples(&v, "0").is_empty() {
            break v;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "no newer sample: {v}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let full = r.history("").await;
    for link in ["0", "1"] {
        let mut joined = samples(&first, link);
        joined.extend(samples(&inc, link));
        let all = samples(&full, link);
        assert_eq!(
            &all[..joined.len()],
            &joined[..],
            "link {link}: the first answer plus the increment is the full history, no gap and no duplicate"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn since_ms_past_the_newest_sample_keeps_the_links_with_empty_series() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    r.wait_history("a sample at 1 s", 15, |v| newest_t(v) >= 1.0)
        .await;
    let v = r.history("?since_ms=99999999").await;
    assert_eq!(v["samples"], json!({"0": [], "1": []}), "{v}");
    assert_eq!(v["started_at"], json!(1_700_000_000), "{v}");
}

// ---- retention -----------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn beyond_the_cap_the_oldest_samples_are_dropped() {
    let mut r = rig_with(Some(3));
    r.start_run(4242, 1_700_000_000).await;
    // 5 s of a run have produced about 5 samples; only 3 are kept
    let v = r
        .wait_history("a sample at 5 s", 20, |v| newest_t(v) >= 5.0)
        .await;
    for link in ["0", "1"] {
        let t = ts(&v, link);
        assert_eq!(
            t.len(),
            3,
            "a cap of 3 keeps 3 samples of link {link}: {t:?}"
        );
        assert!(
            t[0] >= 2.0,
            "the oldest samples (the first seconds) are the ones dropped: {t:?}"
        );
        assert!(t.windows(2).all(|w| w[0] < w[1]), "{t:?}");
    }
    // the cap holds later too
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let v = r.history("").await;
    assert_eq!(ts(&v, "0").len(), 3, "{v}");
    assert!(newest_t(&v) >= 5.0, "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_history_is_kept_after_stop() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    r.wait_history("a sample at 3 s", 15, |v| newest_t(v) >= 3.0)
        .await;
    r.stop_run().await;
    // the poller sees the stopped state within a few polls
    tokio::time::sleep(Duration::from_millis(5 * POLL_MS)).await;
    let stopped = r.history("").await;
    assert!(ts(&stopped, "0").len() >= 3, "{stopped}");
    // the final charts stay visible: unchanged later, with the run's start time
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let later = r.history("").await;
    assert_eq!(stopped, later, "a stopped run records nothing more");
    assert_eq!(later["started_at"], json!(1_700_000_000), "{later}");
    // the live stats are gone though
    assert_eq!(r.stats().await["directions"], json!([]));
    // and an increment after the last sample is empty, not an error
    let inc = r.history(&format!("?since_ms={}", newest_ms(&later))).await;
    assert_eq!(inc["samples"], json!({"0": [], "1": []}), "{inc}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_polls_that_clear_the_stats_do_not_clear_the_history() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    let before = r
        .wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    let server = &r.live.as_ref().unwrap().server;
    server.set_modes(vec![Mode::Nulls]);
    server.break_all();
    // five failed polls clear the cached stats ...
    let t0 = Instant::now();
    loop {
        if r.stats().await["directions"].as_array().unwrap().is_empty() {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "stats not cleared");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // ... the run is still going, so its clock is too
    assert!(r.stats().await["elapsed_ms"].is_u64());
    // ... but it is the same run: the series stays
    let after = r.history("").await;
    let (b, a) = (ts(&before, "0"), ts(&after, "0"));
    assert!(a.len() >= b.len(), "{a:?} vs {b:?}");
    assert_eq!(a[..b.len()], b[..], "the samples taken are still there");
    assert_eq!(after["started_at"], json!(1_700_000_000), "{after}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_start_clears_the_history() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    let run1 = r
        .wait_history("a sample at 3 s", 15, |v| newest_t(v) >= 3.0)
        .await;
    let old_newest = newest_t(&run1);
    r.stop_run().await;
    tokio::time::sleep(Duration::from_millis(5 * POLL_MS)).await;
    let kept = ts(&r.history("").await, "0");
    assert_eq!(
        kept[..ts(&run1, "0").len()],
        ts(&run1, "0")[..],
        "kept after stop"
    );

    // run 2: another pid, another start time, the clock of the fake restarts at 0
    r.start_run(4243, 1_700_000_900).await;
    // the poller may take up to a poll interval to notice the new run; from then on the old
    // run's samples are gone (no start time of the old run, at most one fresh sample)
    let now = r
        .wait_history("the old run to be cleared", 5, |v| {
            v["started_at"] != json!(1_700_000_000)
        })
        .await;
    assert!(
        now["started_at"].is_null() || now["started_at"] == json!(1_700_000_900),
        "{now}"
    );
    let early: Vec<f64> = now["samples"]["0"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s["t_ms"].as_u64())
                .map(|t| t as f64 / 1000.0)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        early.len() <= 1 && early.iter().all(|t| *t < 1.0),
        "the old run's samples are gone: {now}"
    );
    // new samples start from 0 again, not after the old run's last t_ms
    let run2 = r
        .wait_history("a sample at 1 s in the new run", 15, |v| newest_t(v) >= 1.0)
        .await;
    let t = ts(&run2, "0");
    assert!(
        t[0] < 1.5 && newest_t(&run2) < old_newest,
        "the series of the new run starts at 0: {t:?} (old run ended at {old_newest})"
    );
    assert_eq!(run2["started_at"], json!(1_700_000_900), "{run2}");
}

// ---- guards --------------------------------------------------------------------------------------

#[tokio::test]
async fn a_bad_query_is_a_400_with_a_message() {
    let a = TestApp::new(FakeHost::bare_metal());
    for q in [
        "?since_ms=x",
        "?since_ms=-1",
        "?since_ms=-0.5",
        "?since_ms=",
        "?since_ms",
        "?since_ms=nan",
        "?since_ms=NaN",
        "?since_ms=inf",
        "?since_ms=1,5",
        // whole milliseconds only: fractions are refused, exponents too
        "?since_ms=12.5",
        "?since_ms=1.0",
        "?since_ms=1e3",
        "?since_ms=+5",
        "?since_ms=0x10",
        "?since_ms=18446744073709551616", // above u64
    ] {
        let r = a.get(&format!("/api/stats/history{q}")).await;
        assert_eq!(r.status, 400, "{q}: {}", r.text());
        assert!(
            r.message().contains("since_ms"),
            "{q}: the message names since_ms: {}",
            r.text()
        );
    }
    // unknown parameters, alone or next to a good one
    for q in [
        "?foo=1",
        "?since=1",
        "?since_ms=1&foo=2",
        "?foo=2&since_ms=1",
        // the old name is just an unknown parameter now
        "?since_s=1",
        "?since_s=1.5",
        "?since_ms=1&since_s=1",
        "?window_s=10",
        "?dir=0",
    ] {
        let r = a.get(&format!("/api/stats/history{q}")).await;
        assert_eq!(r.status, 400, "{q}: {}", r.text());
        r.message();
    }
    // good ones
    for q in [
        "",
        "?since_ms=0",
        "?since_ms=1",
        "?since_ms=1500",
        "?since_ms=12345",
        "?since_ms=18446744073709551615", // u64::MAX
    ] {
        let r = a.get(&format!("/api/stats/history{q}")).await;
        assert_eq!(r.status, 200, "{q}: {}", r.text());
    }
}

#[tokio::test]
async fn other_methods_are_a_405_and_other_paths_a_404() {
    let a = TestApp::new(FakeHost::bare_metal());
    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        let r = a.request(method, "/api/stats/history", None).await;
        assert_eq!(r.status, 405, "{method}: {}", r.text());
        r.message();
        let r = a
            .request(method, "/api/stats/history", Some(json!({})))
            .await;
        assert_eq!(r.status, 405, "{method} with a body: {}", r.text());
        r.message();
    }
    for path in ["/api/stats/history/0", "/api/stats/history/extra/path"] {
        let r = a.get(path).await;
        assert_eq!(r.status, 404, "{path}: {}", r.text());
        r.message();
    }
    // the histogram route next to it still works as before
    let r = a.get("/api/stats/0/delay_histogram").await;
    assert_eq!(r.status, 404, "{}", r.text());
}

#[test]
fn a_foreign_host_gets_421_and_a_good_one_the_history() {
    let c = ControllerProcess::start(&["serve", "--bind", "{addr}"], &[]);
    let get = |host: &str, path: &str| {
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        http_raw(&c.addr, &req).unwrap_or_else(|e| panic!("GET {path}: {e}"))
    };
    for host in [
        "attacker.example",
        "attacker.example:8100",
        "127.0.0.1.attacker.example",
    ] {
        for path in ["/api/stats/history", "/api/stats/history?since_ms=1"] {
            let (st, _, body) = get(host, path);
            assert_eq!(st, 421, "Host {host:?} {path}: {body:?}");
            let v: Value = serde_json::from_str(body.trim()).unwrap();
            assert!(v["message"].as_str().is_some_and(|m| !m.is_empty()), "{v}");
        }
    }
    let (st, _, body) = get(&c.addr, "/api/stats/history");
    assert_eq!(st, 200, "{body:?}");
    let v: Value = serde_json::from_str(body.trim()).unwrap();
    assert_eq!(v["samples"], json!({}), "{v}");
    // the real binary rejects a bad query with its JSON 400 as well
    let (st, _, body) = get(&c.addr, "/api/stats/history?since_ms=x");
    assert_eq!(st, 400, "{body:?}");
}

// ---- OpenAPI -------------------------------------------------------------------------------------

/// Resolves a local `$ref` (`#/components/schemas/X`).
fn resolve<'a>(doc: &'a Value, v: &'a Value) -> &'a Value {
    match v["$ref"].as_str().and_then(|r| r.strip_prefix("#/")) {
        Some(path) => resolve(doc, path.split('/').fold(doc, |d, k| &d[k])),
        None => v,
    }
}

#[tokio::test]
async fn openapi_documents_the_history_route() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    let op = &doc["paths"]["/api/stats/history"]["get"];
    assert!(
        op.is_object(),
        "GET /api/stats/history is not documented: {}",
        doc["paths"]
    );
    assert!(
        doc["paths"]["/api/stats/history"]
            .as_object()
            .unwrap()
            .keys()
            .all(|k| k == "get" || k == "parameters" || k == "summary" || k == "description"),
        "only GET is documented: {}",
        doc["paths"]["/api/stats/history"]
    );
    // the query parameter since_ms, optional, an integer
    let params = op["parameters"].as_array().expect("parameters");
    let p = params
        .iter()
        .find(|p| p["name"] == "since_ms")
        .unwrap_or_else(|| panic!("no since_ms parameter: {params:?}"));
    assert_eq!(p["in"], "query", "{p}");
    assert!(p["required"].is_null() || p["required"] == false, "{p}");
    let schema = resolve(&doc, &p["schema"]);
    let is_integer = |s: &Value| {
        let t = &s["type"];
        t == "integer"
            || t.as_array()
                .is_some_and(|a| a.iter().any(|x| x == "integer"))
    };
    assert!(
        is_integer(schema)
            || schema["oneOf"]
                .as_array()
                .or_else(|| schema["anyOf"].as_array())
                .is_some_and(|a| a.iter().any(|x| is_integer(resolve(&doc, x)))),
        "since_ms is an integer: {p}"
    );
    // the answers
    let responses = &op["responses"];
    for code in ["200", "400"] {
        assert!(
            responses.get(code).is_some(),
            "{code} is not documented: {responses}"
        );
    }
    assert!(
        responses["400"].to_string().contains("ErrorBody"),
        "{}",
        responses["400"]
    );
    // the 200 body is a schema with started_at and samples, and some schema describes a sample
    let body = resolve(
        &doc,
        &responses["200"]["content"]["application/json"]["schema"],
    );
    let props = body["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("no properties: {body}"));
    assert!(props.contains_key("started_at"), "{body}");
    assert!(props.contains_key("samples"), "{body}");
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    let sample = schemas.values().find(|s| {
        s["properties"]
            .as_object()
            .is_some_and(|p| SAMPLE_FIELDS.iter().all(|f| p.contains_key(*f)))
    });
    assert!(
        sample.is_some(),
        "no schema with all of {SAMPLE_FIELDS:?}: {:?}",
        schemas.keys().collect::<Vec<_>>()
    );
}

// ---- second round: elapsed_ms on /api/stats, one clock for stats and history ----------------------------

/// `elapsed_ms` of a stats reply in seconds; it must be an integer number of ms.
fn elapsed(v: &Value) -> Option<f64> {
    let e = v.get("elapsed_ms").filter(|e| !e.is_null())?;
    assert!(e.is_u64(), "elapsed_ms is an integer: {v}");
    e.as_u64().map(|ms| ms as f64 / 1000.0)
}

/// The raw `elapsed_ms` of a stats reply.
fn elapsed_ms_of(v: &Value) -> Option<u64> {
    v.get("elapsed_ms").and_then(Value::as_u64)
}

/// Seconds between `t0` and now.
fn since(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64()
}

#[tokio::test]
async fn elapsed_ms_is_absent_before_any_run() {
    let a = TestApp::new(FakeHost::bare_metal());
    let v = a.get("/api/stats").await.json();
    assert!(v.get("elapsed_ms").is_none_or(Value::is_null), "{v}");

    let r = rig();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let v = r.stats().await;
    assert!(v.get("elapsed_ms").is_none_or(Value::is_null), "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elapsed_ms_counts_the_seconds_since_the_run_started() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    let t0 = Instant::now();
    r.wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    let w1 = since(t0);
    let stats = r.stats().await;
    assert!(
        stats.get("elapsed_s").is_none(),
        "the old elapsed_s is gone: {stats}"
    );
    assert!(
        elapsed_ms_of(&stats).is_some(),
        "elapsed_ms is a u64 while running: {stats}"
    );
    let e1 = elapsed(&stats).expect("elapsed_ms is a number while running");
    assert!(
        e1 >= w1 - 0.6 && e1 <= w1 + 0.3,
        "elapsed_ms {e1} after {w1} s of running"
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    let w2 = since(t0);
    let e2 = elapsed(&r.stats().await).unwrap();
    assert!(
        e2 >= w2 - 0.6 && e2 <= w2 + 0.3,
        "elapsed_ms {e2} after {w2} s of running"
    );
    assert!(
        e2 - e1 >= 0.4 && e2 - e1 <= 0.9,
        "it advances with real time: {e1} then {e2}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_history_and_elapsed_ms_share_one_clock() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    for _ in 0..3 {
        r.wait_history("a sample at 1 s", 15, |v| newest_t(v) >= 1.0)
            .await;
        let h = r.history("").await;
        let last = newest_t(&h);
        let e = elapsed(&r.stats().await).unwrap();
        assert!(
            last <= e + 1e-6,
            "a sample cannot be from after the clock read next: t_ms {last} s, elapsed_ms {e} s"
        );
        assert!(
            e - last <= 2.0,
            "the newest sample is at most about a sampling period old: t_ms {last} s, elapsed_ms {e} s"
        );
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elapsed_ms_is_frozen_at_the_stop_instant() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    let t0 = Instant::now();
    r.wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    r.stop_run().await;
    let ran = since(t0);
    tokio::time::sleep(Duration::from_millis(5 * POLL_MS)).await;
    let stopped = r.stats().await;
    assert_eq!(stopped["directions"], json!([]));
    let e1 = elapsed(&stopped).expect("elapsed_ms is still reported after a stop");
    assert!(
        e1 >= ran - 0.7 && e1 <= ran + 0.3,
        "frozen at the stop instant: {e1} for a run of {ran} s"
    );
    let newest = newest_t(&r.history("").await);
    assert!(newest <= e1 + 1e-6, "{newest} vs {e1}");
    // it does not move any more
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let e2 = elapsed(&r.stats().await).unwrap();
    assert_eq!(e1, e2, "a stopped run's clock is frozen");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(elapsed(&r.stats().await), Some(e1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_start_restarts_elapsed_ms() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    r.wait_history("a sample at 3 s", 15, |v| newest_t(v) >= 3.0)
        .await;
    r.stop_run().await;
    tokio::time::sleep(Duration::from_millis(5 * POLL_MS)).await;
    let frozen = elapsed(&r.stats().await).unwrap();
    assert!(frozen >= 2.5, "{frozen}");

    r.start_run(4243, 1_700_000_900).await;
    let t1 = Instant::now();
    r.wait_history("the old run to be cleared", 5, |v| {
        v["started_at"] != json!(1_700_000_000)
    })
    .await;
    let e = elapsed(&r.stats().await).expect("elapsed_ms of the new run");
    assert!(
        e < 1.5,
        "the new run's clock starts near 0, not at {frozen}: {e}"
    );
    r.wait_history("a sample at 1 s of the new run", 15, |v| newest_t(v) >= 1.0)
        .await;
    let w = since(t1);
    let e = elapsed(&r.stats().await).unwrap();
    assert!(
        e >= w - 0.6 && e <= w + 0.3,
        "counting from the new start: {e} after {w} s"
    );
    assert!(e < frozen, "{e} vs {frozen}");
}

#[tokio::test]
async fn openapi_lists_elapsed_ms_on_the_stats_view() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    let p = &doc["components"]["schemas"]["StatsView"]["properties"]["elapsed_ms"];
    assert!(
        p.is_object(),
        "StatsView has no elapsed_ms: {}",
        doc["components"]["schemas"]["StatsView"]
    );
    let is_integer = |t: &Value| {
        t == "integer"
            || t.as_array()
                .is_some_and(|a| a.iter().any(|x| x == "integer"))
    };
    assert!(
        is_integer(&p["type"])
            || p["oneOf"]
                .as_array()
                .or_else(|| p["anyOf"].as_array())
                .is_some_and(|a| a.iter().any(|x| is_integer(&x["type"]))),
        "elapsed_ms is an integer (u64): {p}"
    );
    // not required: it is absent before any run
    let required = doc["components"]["schemas"]["StatsView"]["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!required.iter().any(|k| k == "elapsed_ms"), "{required:?}");
}

// ---- spec 29b item 1: run_id and links ------------------------------------------------------------
//
// `GET /api/stats/history` also returns `run_id` (u64, one more with every new run the controller
// sees, null before any run) and `links: [{name, ingress, egress}]` of that run in link-index order,
// both kept after the stop like the samples and replaced together when a new run begins. Choices of
// the tester (the spec leaves them open): before any run `links` is `[]`; the link objects have
// exactly the three fields (no `config`, no `worker`); a new run's id is the previous one plus 1;
// an answer never pairs the `run_id` of one run with the `links` or `started_at` of another; a
// request that is refused (400) changes neither.

/// The two links of the runs below, with names and ports that differ from run to run so that a
/// mixed-up run or link order shows.
fn links_a() -> Vec<Value> {
    vec![
        link("alpha", "net_null0", "net_null1", None),
        link("beta", "net_null1", "net_null0", None),
    ]
}

fn links_b() -> Vec<Value> {
    vec![
        link("gamma", "net_null2", "net_null3", None),
        link("delta", "net_null3", "net_null2", None),
        link("epsilon", "net_null4", "net_null4", None),
    ]
}

fn links_a_json() -> Value {
    json!([
        {"name": "alpha", "ingress": "net_null0", "egress": "net_null1"},
        {"name": "beta", "ingress": "net_null1", "egress": "net_null0"},
    ])
}

fn links_b_json() -> Value {
    json!([
        {"name": "gamma", "ingress": "net_null2", "egress": "net_null3"},
        {"name": "delta", "ingress": "net_null3", "egress": "net_null2"},
        {"name": "epsilon", "ingress": "net_null4", "egress": "net_null4"},
    ])
}

/// `run_id` of a history answer; panics unless it is a u64.
fn run_id(v: &Value) -> u64 {
    v["run_id"]
        .as_u64()
        .unwrap_or_else(|| panic!("run_id is not a u64 in {v}"))
}

#[tokio::test]
async fn before_any_run_there_is_no_run_id_and_no_links() {
    // a source that records nothing (the common fake)
    let a = TestApp::new(FakeHost::bare_metal());
    let v = a.get("/api/stats/history").await.json();
    assert!(v.get("run_id").is_none_or(Value::is_null), "{v}");
    assert_eq!(v["links"], json!([]), "{v}");

    // the real poller, never started
    let r = rig();
    tokio::time::sleep(Duration::from_millis(300)).await;
    for q in ["", "?since_ms=0"] {
        let v = r.history(q).await;
        assert!(v.get("run_id").is_none_or(Value::is_null), "{q}: {v}");
        assert_eq!(v["links"], json!([]), "{q}: {v}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_has_a_run_id_and_its_links_in_link_order() {
    let mut r = rig();
    r.start_run_links(4242, 1_700_000_000, links_a()).await;
    let v = r
        .wait_history("a sample at 1 s", 15, |v| newest_t(v) >= 1.0)
        .await;
    run_id(&v);
    assert_eq!(
        v["links"],
        links_a_json(),
        "the links of the run's configuration, in the order of the start body: {v}"
    );
    // the link list is one-to-one with the series keys the fake dpds produces for the first links
    assert!(
        v["samples"]["0"].is_array() && v["samples"]["1"].is_array(),
        "{v}"
    );

    // every incremental answer carries them too, also when it holds no sample
    let id = run_id(&v);
    for q in ["?since_ms=0", "?since_ms=99999999"] {
        let inc = r.history(q).await;
        assert_eq!(run_id(&inc), id, "{q}: {inc}");
        assert_eq!(inc["links"], links_a_json(), "{q}: {inc}");
    }
    // and the id stays put while the run goes on
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(run_id(&r.history("").await), id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_id_and_links_are_kept_after_the_stop() {
    let mut r = rig();
    r.start_run_links(4242, 1_700_000_000, links_a()).await;
    let live = r
        .wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    let id = run_id(&live);
    r.stop_run().await;
    tokio::time::sleep(Duration::from_millis(5 * POLL_MS)).await;
    let stopped = r.history("").await;
    assert_eq!(run_id(&stopped), id, "{stopped}");
    assert_eq!(stopped["links"], links_a_json(), "{stopped}");
    // later and incremental answers too: the stopped run's charts stay readable
    tokio::time::sleep(Duration::from_millis(1200)).await;
    for q in ["", "?since_ms=99999999"] {
        let later = r.history(q).await;
        assert_eq!(run_id(&later), id, "{q}: {later}");
        assert_eq!(later["links"], links_a_json(), "{q}: {later}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_run_gets_the_next_run_id_and_its_own_links() {
    let mut r = rig();
    r.start_run_links(4242, 1_700_000_000, links_a()).await;
    let run1 = r
        .wait_history("a sample at 1 s", 15, |v| newest_t(v) >= 1.0)
        .await;
    let id1 = run_id(&run1);
    r.stop_run().await;
    tokio::time::sleep(Duration::from_millis(5 * POLL_MS)).await;
    assert_eq!(run_id(&r.history("").await), id1, "kept after stop");

    // run 2 has other links, another pid and start time. Every answer from the start request on
    // is one run or the other, never a mix, and never goes back.
    r.start_run_links(4243, 1_700_000_900, links_b()).await;
    let t0 = Instant::now();
    let mut seen_new = false;
    let run2 = loop {
        let v = r.history("").await;
        match v.get("run_id").filter(|i| !i.is_null()) {
            None => {
                assert!(
                    v["links"] == json!([]) && v["samples"] == json!({}),
                    "an answer without a run_id claims nothing: {v}"
                );
            }
            Some(i) if *i == json!(id1) => {
                assert!(!seen_new, "the old run_id came back after the new one: {v}");
                assert_eq!(
                    v["links"],
                    links_a_json(),
                    "run {id1} has its own links: {v}"
                );
                assert_eq!(v["started_at"], json!(1_700_000_000), "{v}");
            }
            Some(i) => {
                assert_eq!(*i, json!(id1 + 1), "a new run is the next id: {v}");
                assert_eq!(
                    v["links"],
                    links_b_json(),
                    "the new run's id comes with the new run's links, at once: {v}"
                );
                seen_new = true;
                if newest_t(&v) >= 1.0 {
                    break v;
                }
            }
        }
        assert!(
            t0.elapsed() < Duration::from_secs(15),
            "no sample at 1 s in the new run; last answer {v}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(run2["started_at"], json!(1_700_000_900), "{run2}");
    assert_eq!(run_id(&run2), id1 + 1, "{run2}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_start_changes_neither_run_id_nor_links() {
    let mut r = rig();
    r.start_run_links(4242, 1_700_000_000, links_a()).await;
    let live = r
        .wait_history("a sample at 1 s", 15, |v| newest_t(v) >= 1.0)
        .await;
    let id = run_id(&live);
    r.stop_run().await;
    tokio::time::sleep(Duration::from_millis(5 * POLL_MS)).await;

    // 400: no links at all; 400: a port name the start refuses
    for body in [
        setup(vec![]).body(),
        setup(vec![link("zeta", "bad port!", "net_null0", None)]).body(),
    ] {
        let resp = call(&r.router, "POST", "/api/emulation/start", Some(body)).await;
        assert_eq!(resp.status, 400, "{}", resp.text());
    }
    tokio::time::sleep(Duration::from_millis(5 * POLL_MS)).await;
    let v = r.history("").await;
    assert_eq!(run_id(&v), id, "{v}");
    assert_eq!(v["links"], links_a_json(), "{v}");
    assert_eq!(v["started_at"], json!(1_700_000_000), "{v}");
}

#[tokio::test]
async fn openapi_documents_run_id_and_links_on_the_history() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    let op = &doc["paths"]["/api/stats/history"]["get"];
    let body = resolve(
        &doc,
        &op["responses"]["200"]["content"]["application/json"]["schema"],
    );
    let props = body["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("no properties: {body}"));
    let is_integer = |s: &Value| {
        let t = &s["type"];
        t == "integer"
            || t.as_array()
                .is_some_and(|a| a.iter().any(|x| x == "integer"))
    };
    let run_id = props
        .get("run_id")
        .unwrap_or_else(|| panic!("no run_id in {body}"));
    let run_id = resolve(&doc, run_id);
    assert!(
        is_integer(run_id)
            || run_id["oneOf"]
                .as_array()
                .or_else(|| run_id["anyOf"].as_array())
                .is_some_and(|a| a.iter().any(|x| is_integer(resolve(&doc, x)))),
        "run_id is an integer: {run_id}"
    );
    let links = resolve(
        &doc,
        props
            .get("links")
            .unwrap_or_else(|| panic!("no links in {body}")),
    );
    assert_eq!(links["type"], "array", "{links}");
    let item = resolve(&doc, &links["items"]);
    let item_props = item["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("the items of links have no properties: {links}"));
    for k in ["name", "ingress", "egress"] {
        assert!(item_props.contains_key(k), "a link has {k}: {item}");
    }
    assert_eq!(
        item_props.len(),
        3,
        "a link is {{name, ingress, egress}}: {item}"
    );
}

// ---- spec 29b item 2: a reader never applies a stale supervisor status -----------------------------
//
// Observable part of the locking change: a reader (`run_history`, `elapsed_ms`) reads the supervisor
// status after it took the run lock. The supervisor below stalls one reader inside `status()` after
// that call has already decided what to return, as a thread descheduled between reading the
// status and using it. With the status read BEFORE the lock (the spec 29 code) another reader
// applies the newer status meanwhile and the stalled one then applies its stale one and throws the
// newer run away. With the status read AFTER taking the lock the other reader waits for the stalled
// one, and no stale status is ever applied.
//
// NOT observable from outside, so not pinned here (review or measurement): that the poller records a
// sample outside the stats-cell lock (a reader that holds the run lock, as required here, delays
// `/api/stats` through `elapsed_ms` whatever the poller does) and that `run_history` copies only the
// requested range under the lock (it needs a series of up to 86400 samples per link, which only
// 24 h of running or a test hook produce).

struct Gate {
    armed: bool,
    entered: bool,
    open: bool,
}

/// A supervisor that reports a settable status and can stall one call made by a request handler
/// (any named thread, i.e. a tokio worker; the poller runs on its own unnamed thread and is never stalled), after
/// the answer of that call is fixed.
struct GatedSupervisor {
    status: Mutex<SupervisorStatus>,
    gate: Mutex<Gate>,
    changed: std::sync::Condvar,
}

fn running(pid: u32, started_at: u64) -> SupervisorStatus {
    SupervisorStatus {
        state: State::Running,
        pid: Some(pid),
        started_at: Some(started_at),
        error: None,
        log_tail: vec![],
    }
}

impl GatedSupervisor {
    fn new(s: SupervisorStatus) -> Arc<GatedSupervisor> {
        Arc::new(GatedSupervisor {
            status: Mutex::new(s),
            gate: Mutex::new(Gate {
                armed: false,
                entered: false,
                open: false,
            }),
            changed: std::sync::Condvar::new(),
        })
    }

    fn set(&self, s: SupervisorStatus) {
        *self.status.lock().unwrap() = s;
    }

    /// The next `status()` call of a request handler stalls until `open_gate`.
    fn arm(&self) {
        *self.gate.lock().unwrap() = Gate {
            armed: true,
            entered: false,
            open: false,
        };
    }

    fn open_gate(&self) {
        self.gate.lock().unwrap().open = true;
        self.changed.notify_all();
    }

    async fn wait_entered(&self) {
        let t0 = Instant::now();
        while !self.gate.lock().unwrap().entered {
            assert!(
                t0.elapsed() < Duration::from_secs(5),
                "no request handler asked the supervisor for its status"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

impl Supervisor for GatedSupervisor {
    fn start(&self, _plan: LaunchPlan) {}
    fn stop(&self) {}
    fn status(&self) -> SupervisorStatus {
        let answer = self.status.lock().unwrap().clone();
        // every thread but the poller's (`std::thread::spawn`, no name) is a request handler's
        let handler = std::thread::current().name().is_some();
        let mut g = self.gate.lock().unwrap();
        if g.armed && handler {
            g.armed = false;
            g.entered = true;
            self.changed.notify_all();
            let t0 = Instant::now();
            while !g.open && t0.elapsed() < Duration::from_secs(10) {
                g = self
                    .changed
                    .wait_timeout(g, Duration::from_millis(50))
                    .unwrap()
                    .0;
            }
        }
        answer
    }
}

struct GatedRig {
    router: axum::Router,
    supervisor: Arc<GatedSupervisor>,
    _poller: Arc<Poller>,
    _host: FakeHost,
    _dir: tempfile::TempDir,
}

fn gated_rig() -> GatedRig {
    let rt = short_tempdir();
    let supervisor = GatedSupervisor::new(running(1, 100));
    let poller = Poller::spawn(rt.path().to_path_buf(), supervisor.clone());
    let host = FakeHost::bare_metal();
    let router = api::router(Deps {
        roots: host.roots(),
        supervisor: supervisor.clone(),
        telemetry: poller.clone(),
        dpds_bin: PathBuf::from("/usr/local/bin/dpds"),
        runtime_dir: rt.path().to_path_buf(),
        dpds_has_telemetry: true,
        is_root: false,
    });
    GatedRig {
        router,
        supervisor,
        _poller: poller,
        _host: host,
        _dir: rt,
    }
}

/// What `stalled_reader` saw, by run identity (never by the order in which answers happened to
/// travel back through the router, which no reader orders).
struct Stalled {
    /// `run_id` of run 100 before anything stalls.
    id0: u64,
    /// The stalled reader's answer (a history or a stats body).
    first: Value,
    /// The history answer of the reader that came in meanwhile.
    second: Value,
    /// A history read after both are done.
    after: Value,
}

/// Stalls the first reader `first` (a path) inside the supervisor's `status()` with run 100's
/// status in hand, lets run 200 take over, runs a history read meanwhile, then releases the first.
/// The poller is kept out of it (one wake-up at the start, then an interval of a minute), so only
/// the readers apply statuses.
async fn stalled_reader(first: &str) -> Stalled {
    let g = gated_rig();
    g._poller.set_poll_interval(60_000);
    // the poller's first wake-up (at most 500 ms after the spawn) has passed; it reads the
    // interval above for every later one
    tokio::time::sleep(Duration::from_millis(700)).await;
    // run 100 is current: its series exists
    let v = router_get(&g.router, "/api/stats/history").await.json();
    assert_eq!(v["started_at"], json!(100), "{v}");
    let id0 = run_id(&v);

    g.supervisor.arm();
    let r1 = {
        let (router, path) = (g.router.clone(), first.to_string());
        tokio::spawn(async move { router_get(&router, &path).await.json() })
    };
    g.supervisor.wait_entered().await;

    // run 200 takes over while the first reader is stalled with run 100's status in hand
    g.supervisor.set(running(2, 200));
    let r2 = {
        let router = g.router.clone();
        tokio::spawn(async move { router_get(&router, "/api/stats/history").await.json() })
    };
    // long enough for a reader that is not held back to finish, and for the first to be
    // measurably older than the run it holds
    tokio::time::sleep(Duration::from_millis(700)).await;
    g.supervisor.open_gate();
    let (first, second): (Value, Value) = (r1.await.unwrap(), r2.await.unwrap());
    let after = router_get(&g.router, "/api/stats/history").await.json();
    Stalled {
        id0,
        first,
        second,
        after,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_history_reader_does_not_bring_back_an_older_run() {
    let s = stalled_reader("/api/stats/history").await;
    // Run 200 is the second run the controller sees, so its id is `id0 + 1`, and it is the one
    // every later read reports. A stalled reader that applies its stale status (run 100) after
    // run 200 was taken over makes a third run out of it (`id0 + 2`), and the next read a
    // fourth. Which of the two readers answers first is not ordered and not asserted: the
    // stalled one may legitimately be linearised first (it holds the run lock) and answer run 100.
    for (what, v) in [
        ("stalled", &s.first),
        ("second", &s.second),
        ("after", &s.after),
    ] {
        assert!(
            run_id(v) <= s.id0 + 1,
            "{what} reader reports run_id {} (only {} and {} exist): a stale status was applied: {v}",
            run_id(v),
            s.id0,
            s.id0 + 1
        );
    }
    assert_eq!(s.second["started_at"], json!(200), "{}", s.second);
    assert_eq!(run_id(&s.second), s.id0 + 1, "{}", s.second);
    assert_eq!(s.after["started_at"], json!(200), "{}", s.after);
    assert_eq!(run_id(&s.after), s.id0 + 1, "{}", s.after);
    // the stalled answer is one of the two runs, whole: its id and start time belong together
    let (id, started) = (run_id(&s.first), &s.first["started_at"]);
    assert!(
        (id == s.id0 && *started == json!(100)) || (id == s.id0 + 1 && *started == json!(200)),
        "{}",
        s.first
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_elapsed_reader_does_not_restart_the_clock_of_an_older_run() {
    let s = stalled_reader("/api/stats").await;
    assert_eq!(s.second["started_at"], json!(200), "{}", s.second);
    assert_eq!(run_id(&s.second), s.id0 + 1, "{}", s.second);
    assert_eq!(run_id(&s.after), s.id0 + 1, "{}", s.after);
    let e = s.first["elapsed_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("no elapsed_ms: {}", s.first));
    // The stalled reader held run 100's status and, holding the run lock, is first in line: run 100
    // had been running for the 700 ms of the stall at least. A stale status applied after run 200
    // took over would make a brand new run out of it (elapsed near 0, run_id id0 + 2).
    assert!(
        e >= 500,
        "elapsed_ms {e} ms: the stalled reader applied a stale status and restarted a clock: {}",
        s.first
    );
}
