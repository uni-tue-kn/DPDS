//! Server-side chart history, `GET /api/stats/history[?since_ms=<t>]`: an in-memory per-second
//! series per link for the current run, so the dashboard charts survive navigation and reload.
//!
//! - Answer: `{"started_at": <unix s or null>, "samples": {"<link index>": [sample, ...]}}`, the
//!   key being the link index in start-body order. Before any run `started_at` is null (or absent)
//!   and `samples` is `{}`.
//! - A sample has `t_ms` (u64 ms since the run started, strictly increasing), the rates
//!   `rx_gbps`, `tx_gbps`, `rx_mpps`, `tx_mpps` (`/api/stats` `rx_bps / 1e9` etc.) and the link's
//!   cumulative drop counters (u64, `/api/stats` `counters`; `tx_drops` is `nic_tx_drops`).
//! - One sample per link per second, from the poller's snapshots (no extra telemetry query), all
//!   links at the same `t_ms`. `since_ms` (digits only) returns the samples with `t_ms > since_ms`,
//!   keeping the key of a known link; a bad `since_ms` or any other parameter is a 400.
//! - Retention is `HISTORY_MAX_SAMPLES` (86400) per link, oldest dropped first. A new start clears
//!   the series; stop and telemetry failures keep it.
//! - `GET /api/stats` has `elapsed_ms` (u64) from a monotonic clock started with the run, frozen at
//!   the stop, null or absent before any run. The history's `t_ms` uses the same clock. The clock may
//!   start up to one poll interval after the start request, so the tests allow 0.6 s below and
//!   0.3 s above the wall time they measured.
//! - Same guards as the other routes (Host allow-list, 405) and documented in OpenAPI.
//!
//! Tests assume `Poller::spawn_with_history_cap(runtime_dir, supervisor, max_samples)` and
//! `HISTORY_MAX_SAMPLES` in `dpds_controller::telemetry`; the route reaches the series through
//! `TelemetrySource`, and a source that records nothing (`FakeTelemetry`) gives the empty answer.
//! They run the real `Poller` against a fake `dpds` socket whose counters grow at constant rates,
//! with real time (one second per sample).

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

/// A sample has the rates and the link's
/// cumulative drop counters (named like `HistoryTotals`; all u64).
const SAMPLE_FIELDS: [&str; 14] = [
    "t_ms",
    "rx_gbps",
    "tx_gbps",
    "rx_mpps",
    "tx_mpps",
    "loss_drops",
    "limiter_drops",
    "buffer_full_drops",
    "nic_tx_drops",
    "nic_rx_missed",
    "nic_rx_nombuf",
    "nic_rx_port_buffer_discards",
    "nic_rx_phy_discards",
    "nic_tx_phy_discards",
];

/// The cumulative counters of a sample (nine causes).
const COUNTER_FIELDS: [&str; 9] = [
    "loss_drops",
    "limiter_drops",
    "buffer_full_drops",
    "nic_tx_drops",
    "nic_rx_missed",
    "nic_rx_nombuf",
    "nic_rx_port_buffer_discards",
    "nic_rx_phy_discards",
    "nic_tx_phy_discards",
];

/// The per-cause shares, which a sample does not have.
const GONE_FIELDS: [&str; 5] = [
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
    let mut snap = snapshot(
        [d0, d1],
        [
            // port 0: link 0 receives, link 1 sends
            PortSample {
                ibytes: bytes(a.rx_gbps),
                obytes: bytes(b.tx_gbps),
                rx_missed_errors: n(miss0 * 0.001),
                rx_mbuf_allocation_errors: n(miss0 * 0.0005),
            },
            // port 1: link 1 receives, link 0 sends
            PortSample {
                ibytes: bytes(b.rx_gbps),
                obytes: bytes(a.tx_gbps),
                rx_missed_errors: 0,
                rx_mbuf_allocation_errors: 0,
            },
        ],
    );
    // the other loss xstats, per second: link 0 (rx port 0, tx port 1) and link 1 (rx
    // port 1, tx port 0) get different numbers. Port-buffer discards are the sum of several
    // priorities, buf and cong.
    set_xstats(
        &mut snap,
        0,
        &[
            ("rx_prio0_buf_discard_packets", n(3000.0)),
            ("rx_prio3_cong_discard_packets", n(1500.0)),
            ("rx_phy_discard_packets", n(700.0)),
            ("tx_phy_discard_packets", n(600.0)),
            ("rx_phy_crc_errors", n(11.0)),
        ],
    );
    set_xstats(
        &mut snap,
        1,
        &[
            ("rx_prio2_buf_discard_packets", n(800.0)),
            ("rx_prio7_cong_discard_packets", n(200.0)),
            ("rx_phy_discard_packets", n(400.0)),
            ("tx_phy_discard_packets", n(300.0)),
        ],
    );
    snap
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
                "/ethdev/xstats" => snap.eth.get(idx).cloned(),
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
        self.start_run_links(pid, started_at, vec![link("l", "net_null0", "net_null0")])
            .await;
    }

    /// Like `start_run`, with the links of the start body (the fake `dpds` always reports two
    /// directions, whatever the body has).
    async fn start_run_links(&mut self, pid: u32, started_at: u64, links: Vec<Value>) {
        self.live = None; // the old socket goes first
        self.live = Some(live(&self.runtime_dir, pid));
        self.supervisor.set_next_run(pid, started_at);
        let body = setup(links).body();
        let r = call(&self.router, "POST", "/api/emulation/start", Some(body)).await;
        assert_eq!(r.status, 202, "{}", r.text());
        if let Some(l) = &self.live {
            l.restart_clock();
        }
        self.speed_up_poller();
    }

    /// The start body has no poll interval (a fixed 1000 ms); this test hook speeds the
    /// poller up after a start so that the tests need not wait whole seconds.
    fn speed_up_poller(&self) {
        self._poller.set_poll_interval(POLL_MS);
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
            for key in COUNTER_FIELDS {
                assert!(s[key].is_u64(), "{key} must be a whole number in {s}");
            }
            for key in GONE_FIELDS {
                assert!(s.get(key).is_none(), "{key} is gone (spec 43): {s}");
            }
            assert!(s["t_ms"].is_u64(), "t_ms is a whole number of ms in {s}");
            assert!(s.get("t_s").is_none(), "the old t_s is gone: {s}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_samples_equal_the_rates_of_api_stats() {
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
        }
        assert!(
            checked >= 2,
            "link {link}: only {checked} samples at t >= 1 s: {v}"
        );
    }
}

/// Per second the fake `dpds` adds this many drops to link 0 and link 1:
/// loss, limiter, buffer full, tx_drops, rx_missed, rx_nombuf, port buffer (sum of prio buf+cong),
/// rx phy, tx phy of the egress port (the first four are `EXPECT`'s shares of the rx rate).
const DROPS_PER_S: [[f64; 9]; 2] = [
    [
        40_000.0, 20_000.0, 10_000.0, 5_000.0, 2_000.0, 1_000.0, 4_500.0, 700.0, 300.0,
    ],
    [5_000.0, 0.0, 40_000.0, 0.0, 0.0, 0.0, 1_000.0, 400.0, 600.0],
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_samples_carry_the_cumulative_drop_counters_of_the_link() {
    let mut r = rig();
    r.start_run(4242, 1_700_000_000).await;
    let v = r
        .wait_history("a sample at 3 s", 15, |v| newest_t(v) >= 3.0)
        .await;
    for (i, link) in ["0", "1"].iter().enumerate() {
        let all = samples(&v, link);
        let counts = |s: &Value| COUNTER_FIELDS.map(|k| s[k].as_u64().unwrap_or(u64::MAX));
        let mut checked = 0;
        for w in all.windows(2) {
            let (a, b) = (counts(&w[0]), counts(&w[1]));
            let dt = t_secs(&w[1]) - t_secs(&w[0]);
            for k in 0..9 {
                assert!(
                    a[k] <= b[k],
                    "{} never decreases: {} -> {}",
                    COUNTER_FIELDS[k],
                    w[0],
                    w[1]
                );
                // a cumulative count grows by the rate over the sample spacing (not a per-second
                // share of the window, not a rate)
                let want = DROPS_PER_S[i][k] * dt;
                let got = (b[k] - a[k]) as f64;
                assert!(
                    (got - want).abs() <= want * 0.35 + 3.0,
                    "link {link} {}: grew by {got}, expected about {want} over {dt} s: {} -> {}",
                    COUNTER_FIELDS[k],
                    w[0],
                    w[1]
                );
            }
            checked += 1;
        }
        assert!(
            checked >= 2,
            "link {link}: {checked} consecutive pairs: {v}"
        );
        // causes that never happen stay 0, the others are well above 0 after 2 s
        let last = counts(all.last().unwrap());
        for k in 0..9 {
            if DROPS_PER_S[i][k] == 0.0 {
                assert_eq!(last[k], 0, "link {link} {}", COUNTER_FIELDS[k]);
            } else {
                assert!(
                    last[k] as f64 >= DROPS_PER_S[i][k],
                    "link {link} {}: {last:?}",
                    COUNTER_FIELDS[k]
                );
            }
        }
    }
    // the newest sample is never above what /api/stats reports afterwards (same counters)
    let stats = r.stats().await;
    for (i, link) in ["0", "1"].iter().enumerate() {
        let c = &stats["directions"][i]["counters"];
        let s = samples(&v, link).pop().unwrap();
        for (key, stat) in [
            ("loss_drops", "loss_drops"),
            ("limiter_drops", "limiter_drops"),
            ("buffer_full_drops", "buffer_full_drops"),
            ("nic_tx_drops", "tx_drops"),
            ("nic_rx_missed", "nic_rx_missed"),
            ("nic_rx_nombuf", "nic_rx_nombuf"),
            ("nic_rx_port_buffer_discards", "nic_rx_port_buffer_discards"),
            ("nic_rx_phy_discards", "nic_rx_phy_discards"),
            ("nic_tx_phy_discards", "nic_tx_phy_discards"),
        ] {
            let (a, b) = (s[key].as_u64().unwrap(), c[stat].as_u64().unwrap());
            assert!(a <= b, "link {link} {key}: sample {a} > stats {stat} {b}");
        }
    }
}

/// A port that is an SR-IOV virtual function has no physical-port counters: the fields are JSON
/// null in the samples, the totals and `/api/stats`, while the counters valid on a VF and the
/// other port's stay numbers. Link 0 is NIC0 (a VF, device 0x101e) -> NIC1 (a PF), link 1 the
/// reverse; the fake `dpds` counts every loss xstat on both ports.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vf_port_has_null_physical_counters_in_samples_totals_and_stats() {
    let mut r = rig();
    r._host.add_nic(&Nic {
        device: "0x101e".into(),
        ..Nic::mlx5(NIC0, "ens1f0", Some(0))
    });
    let links = vec![link("a", NIC0, NIC1), link("b", NIC1, NIC0)];
    r.start_run_links(4242, 1_700_000_000, links).await;
    let v = r
        .wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    let rx_side = ["nic_rx_port_buffer_discards", "nic_rx_phy_discards"];
    let tx_side = ["nic_tx_phy_discards"];
    let stats = r.stats().await;
    // (link, the fields that are null, the ones that stay numbers)
    for (i, null, number) in [
        ("0", rx_side.to_vec(), tx_side.to_vec()),
        ("1", tx_side.to_vec(), rx_side.to_vec()),
    ] {
        let check = |what: &str, s: &Value| {
            for k in &null {
                assert!(s[*k].is_null(), "link {i} {what}.{k} is null: {s}");
            }
            for k in number.iter().chain(&["nic_rx_missed", "nic_rx_nombuf"]) {
                assert!(s[*k].is_u64(), "link {i} {what}.{k} is a number: {s}");
            }
        };
        let all = samples(&v, i);
        assert!(all.len() >= 2, "link {i}: {v}");
        for s in &all {
            check("sample", s);
        }
        check("totals", &v["totals"][i]);
        let d = i.parse::<usize>().unwrap();
        check("stats", &stats["directions"][d]["counters"]);
        assert!(
            number
                .iter()
                .all(|k| all.last().unwrap()[*k].as_u64() > Some(0)),
            "the numbers are real counts: {v}"
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
        // page 0 of the IAT histogram of each link
        "/dpds/iat_hist,0",
        "/dpds/iat_hist,1",
        "/ethdev/xstats,0",
        "/ethdev/xstats,1",
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
            && count("/ethdev/xstats,1").abs_diff(polls) <= 1,
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
    // run's samples are gone (no start time of the previous run, at most one fresh sample)
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
        // an unknown parameter
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
    // the histogram route next to it still works
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
    let props = sample.unwrap()["properties"].as_object().unwrap();
    for key in GONE_FIELDS {
        assert!(
            !props.contains_key(key),
            "{key} is gone from the schema: {props:?}"
        );
    }
}

// ---- elapsed_ms on /api/stats, one clock for stats and history ----------------------------

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

// ---- run_id and links ------------------------------------------------------------
//
// `GET /api/stats/history` also returns `run_id` (u64, one more with every new run the controller
// sees, null before any run) and `links: [{name, ingress, egress}]` of that run in link-index order,
// both kept after the stop like the samples and replaced together when a new run begins. Before any run `links` is `[]`; the link objects have
// exactly the three fields (no `config`, no `worker`); a new run's id is the previous one plus 1;
// an answer never pairs the `run_id` of one run with the `links` or `started_at` of another; a
// request that is refused (400) changes neither.

/// The two links of the runs below, with names and ports that differ from run to run so that a
/// mixed-up run or link order shows.
fn links_a() -> Vec<Value> {
    vec![
        link("alpha", "net_null0", "net_null1"),
        link("beta", "net_null1", "net_null0"),
    ]
}

fn links_b() -> Vec<Value> {
    vec![
        link("gamma", "net_null2", "net_null3"),
        link("delta", "net_null3", "net_null2"),
        link("epsilon", "net_null4", "net_null4"),
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
        setup(vec![link("zeta", "bad port!", "net_null0")]).body(),
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

// ---- a reader never applies a stale supervisor status -----------------------------
//
// A reader (`run_history`, `elapsed_ms`) reads the supervisor
// status after it took the run lock. The supervisor below stalls one reader inside `status()` after
// that call has already decided what to return, as a thread descheduled between reading the
// status and using it. With the status read BEFORE the lock another reader
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

// ---- `totals`, the run's final cumulative counters --------------------------------
//
// Contract the tests pin:
//
// - `GET /api/stats/history` also carries `totals`: `{"<link index>": {rx_pkts, loss_drops,
//   limiter_drops, buffer_full_drops, nic_tx_drops, nic_rx_missed, nic_rx_nombuf}}`, the key set
//   of the samples (the order of the start body), each object with exactly these ten u64 fields.
//   They are the cumulative counters since the `dpds` process started, taken from the NEWEST
//   successful poll of the run (not from the newest once-a-second sample): `rx_pkts`,
//   `loss_drops`, `limiter_drops`, `buffer_full_drops` are the counters of `/api/stats` of the
//   same names; `nic_tx_drops` is its `counters.tx_drops` (the link's `tx_drops` of `/dpds/stats`,
//   there is no counter called `nic_tx_drops`); `nic_rx_missed` and `nic_rx_nombuf` are
//   `counters.nic_rx_missed` and `counters.nic_rx_nombuf` (the receiving port's cumulative
//   `imissed` and `rx_nombuf`).
// - Every answer has it, incremental ones (`since_ms`) too, also when no sample is newer.
// - Before any run it is `{}` (present, not null and not omitted). It is kept after the stop with
//   the values of the last poll while the run was running, and when failed polls clear the stats
//   cache, like the samples. A new run (a new start) drops the old totals; they are `{}` or the new
//   run's own until its first poll gives numbers. A refused start changes nothing.
// - OpenAPI: `totals` is a property of the 200 body, an object whose values have exactly these ten
//   integer properties.
//
// The fake `dpds` below ticks per POLL, not per second: its uptime is 100 ms times the number of
// `/dpds/ports` queries so far, so a counter value names the poll it came from, and the test can
// tell the newest poll from the newest sample.

/// Uptime step of one poll of the counting fake.
const COUNT_STEP_NS: u64 = 100_000_000;

/// Packets per poll of link 0 and link 1 (`EXPECT`'s rates times 100 ms).
const RX_PER_POLL: [u64; 2] = [200_000, 100_000];

/// `out_of_order` is an integer and `frame_sizes` an array of 8 integers.
const TOTALS_FIELDS: [&str; 12] = [
    "rx_pkts",
    "loss_drops",
    "limiter_drops",
    "buffer_full_drops",
    "nic_tx_drops",
    "nic_rx_missed",
    "nic_rx_nombuf",
    "nic_rx_port_buffer_discards",
    "nic_rx_phy_discards",
    "nic_tx_phy_discards",
    "out_of_order",
    "tx_sent",
];

/// `TOTALS_FIELDS` and the array `frame_sizes`.
fn all_totals_fields() -> Vec<&'static str> {
    let mut v = TOTALS_FIELDS.to_vec();
    v.push("frame_sizes");
    v.sort_unstable();
    v
}

/// A fake `dpds` whose uptime advances by [`COUNT_STEP_NS`] with every `/dpds/ports` query: the
/// k-th poll sees the counters of `at(k * COUNT_STEP_NS)`.
fn counting(runtime_dir: &Path, pid: u32) -> Live {
    let polls = Arc::new(AtomicU64::new(0));
    let hists = full_hists(&at(1));
    let server = FakeTelemetryServer::start(
        &socket_path(runtime_dir),
        pid,
        vec![Mode::Serve],
        move |cmd| {
            let (name, param) = cmd.split_once(',').unwrap_or((cmd, ""));
            if name == "/dpds/ports" {
                polls.fetch_add(1, Ordering::SeqCst);
            }
            let snap = at(polls.load(Ordering::SeqCst).max(1) * COUNT_STEP_NS);
            let idx: usize = param.parse().unwrap_or(usize::MAX);
            match name {
                "/dpds/ports" => Some(snap.ports),
                "/dpds/stats" => snap.dirs.get(idx).cloned(),
                "/dpds/delay_hist" => hist_page(&hists, param),
                "/ethdev/xstats" => snap.eth.get(idx).cloned(),
                _ => None,
            }
        },
    );
    Live {
        server,
        t0: Arc::new(Mutex::new(Instant::now())),
    }
}

impl Rig {
    /// Like `start_run`, with the counting fake.
    async fn start_counting_run(&mut self, pid: u32, started_at: u64) {
        self.live = None;
        self.live = Some(counting(&self.runtime_dir, pid));
        self.supervisor.set_next_run(pid, started_at);
        let body = setup(vec![link("l", "net_null0", "net_null0")]).body();
        let r = call(&self.router, "POST", "/api/emulation/start", Some(body)).await;
        assert_eq!(r.status, 202, "{}", r.text());
        self.speed_up_poller();
    }

    /// Number of polls the counting fake has served.
    fn polls_served(&self) -> u64 {
        self.live
            .as_ref()
            .unwrap()
            .server
            .queries()
            .iter()
            .filter(|q| q.as_str() == "/dpds/ports")
            .count() as u64
    }
}

/// The poll (1-based) whose `rx_pkts` of `link` is `rx`; panics when no poll has that value.
fn poll_of(link: usize, rx: u64, what: &Value) -> u64 {
    let guess = rx / RX_PER_POLL[link];
    (guess.saturating_sub(1).max(1)..=guess + 1)
        .find(|k| at(k * COUNT_STEP_NS).dirs[link]["rx_pkts"] == json!(rx))
        .unwrap_or_else(|| panic!("rx_pkts {rx} of link {link} is no poll's counter: {what}"))
}

/// What the ten totals of `link` are after poll `k`, from the fake's own reply: link `d`
/// receives on port `d` in this fake.
fn totals_after(k: u64, link: usize) -> Value {
    let s = at(k * COUNT_STEP_NS);
    let d = &s.dirs[link];
    let port = &s.eth[link];
    let egress = &s.eth[1 - link];
    let prio_sum = |p: &Value| -> u64 {
        p.as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with("rx_prio") && k.ends_with("_discard_packets"))
            .map(|(_, v)| v.as_u64().unwrap())
            .sum()
    };
    json!({
        "rx_pkts": d["rx_pkts"],
        "loss_drops": d["loss_drops"],
        "limiter_drops": d["limiter_drops"],
        "buffer_full_drops": d["buffer_full_drops"],
        "nic_tx_drops": d["tx_drops"],
        "nic_rx_missed": port["rx_missed_errors"],
        "nic_rx_nombuf": port["rx_mbuf_allocation_errors"],
        "nic_rx_port_buffer_discards": prio_sum(port),
        "nic_rx_phy_discards": port["rx_phy_discard_packets"],
        "nic_tx_phy_discards": egress["tx_phy_discard_packets"],
        "out_of_order": d["out_of_order"],
        "tx_sent": d["tx_sent"],
        "frame_sizes": d["frame_sizes"],
    })
}

/// Checks the `totals` of an answer: both links, exactly the ten u64 fields, all of one poll
/// that matches the fake's counters; returns that poll.
fn check_totals(v: &Value) -> u64 {
    let totals = v["totals"]
        .as_object()
        .unwrap_or_else(|| panic!("totals is not an object in {v}"));
    let mut keys: Vec<&str> = totals.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["0", "1"], "a totals entry per link: {v}");
    let mut polls = Vec::new();
    for (i, key) in ["0", "1"].iter().enumerate() {
        let t = &v["totals"][*key];
        let o = t
            .as_object()
            .unwrap_or_else(|| panic!("totals[{key}] is not an object in {v}"));
        let mut fields: Vec<&str> = o.keys().map(String::as_str).collect();
        fields.sort_unstable();
        assert_eq!(fields, all_totals_fields(), "exactly these fields: {t}");
        for f in TOTALS_FIELDS {
            assert!(t[f].is_u64(), "{f} is a u64 in {t}");
        }
        assert_eq!(t["frame_sizes"].as_array().map(Vec::len), Some(8), "{t}");
        let k = poll_of(i, t["rx_pkts"].as_u64().unwrap(), v);
        assert_eq!(
            *t,
            totals_after(k, i),
            "link {i}: the counters of poll {k}, each from the right source: {v}"
        );
        polls.push(k);
    }
    assert_eq!(polls[0], polls[1], "both links are from the same poll: {v}");
    polls[0]
}

/// The poll `/api/stats` shows, from its counters of link 0 (also checks the two new counters).
fn stats_poll(stats: &Value) -> u64 {
    let dirs = stats["directions"].as_array().unwrap();
    assert_eq!(dirs.len(), 2, "{stats}");
    let mut polls = Vec::new();
    for (i, d) in dirs.iter().enumerate() {
        let c = &d["counters"];
        let k = poll_of(i, c["rx_pkts"].as_u64().unwrap(), stats);
        let want = totals_after(k, i);
        assert_eq!(
            c["nic_rx_missed"], want["nic_rx_missed"],
            "link {i}: the receiving port's rx_missed_errors: {c}"
        );
        assert_eq!(
            c["nic_rx_nombuf"], want["nic_rx_nombuf"],
            "link {i}: the receiving port's rx_mbuf_allocation_errors: {c}"
        );
        assert_eq!(
            c["tx_sent"], want["tx_sent"],
            "link {i}: totals' tx_sent is counters.tx_sent: {c}"
        );
        for key in [
            "nic_rx_port_buffer_discards",
            "nic_rx_phy_discards",
            "nic_tx_phy_discards",
        ] {
            assert_eq!(c[key], want[key], "link {i}: {key}: {c}");
        }
        polls.push(k);
    }
    polls[0]
}

fn totals_of(v: &Value) -> &Value {
    &v["totals"]
}

#[tokio::test]
async fn totals_are_an_empty_object_before_any_run() {
    // a source that records nothing (the common fake)
    let a = TestApp::new(FakeHost::bare_metal());
    let v = a.get("/api/stats/history").await.json();
    assert_eq!(totals_of(&v), &json!({}), "{v}");

    // the real poller, never started
    let r = rig();
    tokio::time::sleep(Duration::from_millis(300)).await;
    for q in ["", "?since_ms=0", "?since_ms=99999999"] {
        let v = r.history(q).await;
        assert_eq!(totals_of(&v), &json!({}), "{q}: {v}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn totals_are_the_newest_counters_of_the_last_poll_not_of_the_last_sample() {
    let mut r = rig();
    r.start_counting_run(4242, 1_700_000_000).await;
    r.wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    // ten reads over a second: a stats read before and one after each history read bracket the
    // poll the totals may come from, give or take the poll that lands between the reads. A series
    // that only changes once a second (the samples) would trail by up to ten polls.
    let mut seen = Vec::new();
    for _ in 0..10 {
        let s1 = stats_poll(&r.stats().await);
        let h = r.history("").await;
        let s2 = stats_poll(&r.stats().await);
        let k = check_totals(&h);
        assert!(
            k + 1 >= s1 && k <= s2 + 1,
            "totals come from poll {k}, /api/stats showed polls {s1} and {s2}: {h}"
        );
        assert!(k >= 15, "poll {k} after 2 s of a run at 100 ms: {h}");
        seen.push(k);
        tokio::time::sleep(Duration::from_millis(130)).await;
    }
    assert!(
        seen.windows(2).all(|w| w[0] <= w[1]),
        "the totals never go back: {seen:?}"
    );
    assert!(
        seen.last() > seen.first(),
        "the totals follow the polls while the run goes on: {seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incremental_answers_carry_the_totals_too() {
    let mut r = rig();
    r.start_counting_run(4242, 1_700_000_000).await;
    let full = r
        .wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    let k_full = check_totals(&full);
    for q in ["?since_ms=0", "?since_ms=99999999"] {
        let inc = r.history(q).await;
        let k = check_totals(&inc);
        assert!(
            k >= k_full,
            "{q}: totals from poll {k}, the full answer had {k_full}"
        );
    }
    // with no newer sample the answer still has them
    let none = r.history("?since_ms=99999999").await;
    assert_eq!(none["samples"], json!({"0": [], "1": []}), "{none}");
    check_totals(&none);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn totals_are_kept_after_the_stop_with_the_last_poll() {
    let mut r = rig();
    r.start_counting_run(4242, 1_700_000_000).await;
    r.wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    r.stop_run().await;
    // the poller sees the stopped state, no poll is made after that
    tokio::time::sleep(Duration::from_millis(6 * POLL_MS)).await;
    let polls = r.polls_served();
    let stopped = r.history("").await;
    let k = check_totals(&stopped);
    // the newest poll of the run (a poll in flight at the stop may not count)
    assert!(
        k + 1 >= polls && k <= polls,
        "{polls} polls were made, the totals are from poll {k}: {stopped}"
    );
    // later, and incrementally, the same numbers; no poll came in between
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(r.polls_served(), polls, "a stopped run is not polled");
    let later = r.history("").await;
    assert_eq!(totals_of(&later), totals_of(&stopped), "{later}");
    let inc = r.history(&format!("?since_ms={}", newest_ms(&later))).await;
    assert_eq!(totals_of(&inc), totals_of(&stopped), "{inc}");

    // a refused start changes nothing
    for body in [
        setup(vec![]).body(),
        setup(vec![link("zeta", "bad port!", "net_null0")]).body(),
    ] {
        let resp = call(&r.router, "POST", "/api/emulation/start", Some(body)).await;
        assert_eq!(resp.status, 400, "{}", resp.text());
    }
    tokio::time::sleep(Duration::from_millis(5 * POLL_MS)).await;
    assert_eq!(totals_of(&r.history("").await), totals_of(&stopped));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_polls_that_clear_the_stats_do_not_clear_the_totals() {
    let mut r = rig();
    r.start_counting_run(4242, 1_700_000_000).await;
    let before = r
        .wait_history("a sample at 2 s", 15, |v| newest_t(v) >= 2.0)
        .await;
    let k_before = check_totals(&before);
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
    // ... the run is the same, so are its numbers: those of the last good poll
    let after = r.history("").await;
    let k_after = check_totals(&after);
    assert!(k_after >= k_before, "{k_after} vs {k_before}: {after}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        totals_of(&r.history("").await),
        totals_of(&after),
        "no successful poll, no new numbers"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_run_drops_the_old_totals() {
    let mut r = rig();
    r.start_counting_run(4242, 1_700_000_000).await;
    let run1 = r
        .wait_history("a sample at 3 s", 20, |v| newest_t(v) >= 3.0)
        .await;
    let id1 = run_id(&run1);
    r.stop_run().await;
    tokio::time::sleep(Duration::from_millis(6 * POLL_MS)).await;
    let stopped = r.history("").await;
    let k1 = check_totals(&stopped);
    assert!(k1 >= 20, "3 s of polls at 100 ms, got poll {k1}");

    // run 2: another pid and start time, its fake counts its polls from 1 again
    r.start_counting_run(4243, 1_700_000_900).await;
    let t0 = Instant::now();
    let run2 = loop {
        let v = r.history("").await;
        let served = r.polls_served();
        if v["run_id"].is_null() {
            // a start in progress has no run, so no numbers
            assert_eq!(totals_of(&v), &json!({}), "{v}");
        } else if run_id(&v) == id1 {
            // not seen the new run yet: still the old run's numbers
            assert_eq!(totals_of(&v), totals_of(&stopped), "{v}");
        } else {
            // the new run: nothing yet, or its own numbers; never the old run's
            let t = totals_of(&v);
            if t.as_object().is_some_and(|o| !o.is_empty()) {
                let k = check_totals(&v);
                assert!(
                    k <= served,
                    "poll {k} of the new run, only {served} were made: {v}"
                );
                assert!(k < k1, "the old run's totals are gone: {v}");
                if newest_t(&v) >= 1.0 {
                    break v;
                }
            } else {
                assert_eq!(t, &json!({}), "{v}");
            }
        }
        assert!(
            t0.elapsed() < Duration::from_secs(15),
            "no totals in the new run; last answer {v}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(run2["started_at"], json!(1_700_000_900), "{run2}");
}

#[tokio::test]
async fn openapi_documents_the_totals() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    let body = resolve(
        &doc,
        &doc["paths"]["/api/stats/history"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"],
    );
    let props = body["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("no properties: {body}"));
    let totals = resolve(
        &doc,
        props
            .get("totals")
            .unwrap_or_else(|| panic!("no totals in {body}")),
    );
    assert_eq!(totals["type"], "object", "{totals}");
    let item = resolve(&doc, &totals["additionalProperties"]);
    let item_props = item["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("the values of totals have no properties: {totals}"));
    let mut names: Vec<&str> = item_props.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, all_totals_fields(), "{item}");
    for (name, schema) in item_props {
        let schema = resolve(&doc, schema);
        let want = match name.as_str() {
            "frame_sizes" => json!("array"),
            "nic_rx_port_buffer_discards" | "nic_rx_phy_discards" | "nic_tx_phy_discards" => {
                json!(["integer", "null"])
            }
            _ => json!("integer"),
        };
        assert_eq!(schema["type"], want, "{name}: {schema}");
    }
}

#[tokio::test]
async fn openapi_documents_the_port_counters_of_a_direction() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    // the schema of `counters`: the one with `tx_drops` and `loss_drops`
    let counters = schemas
        .values()
        .find(|s| {
            s["properties"]
                .as_object()
                .is_some_and(|p| p.contains_key("tx_drops") && p.contains_key("loss_drops"))
        })
        .unwrap_or_else(|| panic!("no counters schema: {:?}", schemas.keys()));
    for name in [
        "nic_rx_missed",
        "nic_rx_nombuf",
        "nic_rx_port_buffer_discards",
        "nic_rx_phy_discards",
        "nic_tx_phy_discards",
        "nic_rx_crc_errors",
        "nic_rx_symbol_errors",
        "nic_rx_length_errors",
    ] {
        let p = resolve(&doc, &counters["properties"][name]);
        let want = if name == "nic_rx_missed" || name == "nic_rx_nombuf" {
            json!("integer")
        } else {
            json!(["integer", "null"])
        };
        assert_eq!(p["type"], want, "{name} is documented: {counters}");
    }
}
