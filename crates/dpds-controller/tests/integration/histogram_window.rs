//! K3 (spec v8), `?window_s=<n>` on `GET /api/stats/{dir}/delay_histogram`.
//!
//! Spec: n is 10, 60 or 300 (absent or 0 = since start). The controller keeps, per direction,
//! a ring of cumulative histogram snapshots (one per poll, reset on start) and returns
//! `current - snapshot >= n s old` (or since start if the run is younger) with `window_s` and
//! the actual covered `span_ms` in the reply. Every other value of `window_s` is a 400.
//!
//! # NEW Rust API the tests assume (ASSUMPTION, the spec does not name it)
//!
//! The ring has to be reachable from the REST layer through the `TelemetrySource` trait, and
//! the tests need a simulated clock, so the trait gets one more method
//! (the real `Poller` fills the ring, the fake below simulates it):
//!
//! ```ignore
//! pub trait TelemetrySource: Send + Sync + 'static {
//!     fn latest(&self) -> Option<Polled>;
//!     /// Every retained poll, OLDEST FIRST, as (age in ms of that poll, its snapshot): the
//!     /// newest entry is the latest poll. Empty without data (not running, reset on start).
//!     fn history(&self) -> Vec<(u64, Snapshot)>;
//! }
//! ```
//!
//! The REST handler picks the newest entry that is at least `window_s` seconds older than the
//! latest poll (`age_i - age_latest >= window_s * 1000`) and reports `span_ms = age_i -
//! age_latest`; without such an entry it returns the cumulative counts of the latest poll.
//! The same selection feeds `count`, `percentiles` and `expected`.
//!
//! Until `history()` exists this module (and so the whole test binary) does not compile.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::common::*;
use crate::fake_telemetry::{hist_page, short_tempdir, FakeTelemetryServer, Mode};
use crate::histogram_extras::{cfg, hist, num_of, snap};
use dpds_controller::api::{self, Deps};
use dpds_controller::stats::{Polled, Snapshot};
use dpds_controller::supervisor::{State, SupervisorStatus};
use dpds_controller::telemetry::{socket_path, Poller, TelemetrySource};
use serde_json::{json, Value};

// ---- a simulated clock behind the trait ---------------------------------------------------------

struct Inner {
    now_ms: u64,
    polls: Vec<(u64, Snapshot)>,
}

/// Every `poll` advances the simulated clock and records a snapshot; nothing is ever dropped
/// (the real ring is bounded, but 300 s must always fit).
struct ClockTelemetry {
    inner: Mutex<Inner>,
}

impl ClockTelemetry {
    fn new() -> Arc<ClockTelemetry> {
        Arc::new(ClockTelemetry {
            inner: Mutex::new(Inner {
                now_ms: 0,
                polls: Vec::new(),
            }),
        })
    }
    fn poll(&self, advance_ms: u64, s: Snapshot) {
        let mut i = self.inner.lock().unwrap();
        i.now_ms += advance_ms;
        let at = i.now_ms;
        i.polls.push((at, s));
    }
    /// The run ended (or restarted): the cache is gone.
    fn clear(&self) {
        self.inner.lock().unwrap().polls.clear();
    }
}

impl TelemetrySource for ClockTelemetry {
    fn latest(&self) -> Option<Polled> {
        let i = self.inner.lock().unwrap();
        let n = i.polls.len();
        let (at, cur) = i.polls.last()?.clone();
        Some(Polled {
            prev: n.checked_sub(2).map(|k| i.polls[k].1.clone()),
            cur,
            age_ms: i.now_ms - at,
            stale: false,
        })
    }
    fn history(&self) -> Vec<(u64, Snapshot)> {
        let i = self.inner.lock().unwrap();
        i.polls
            .iter()
            .map(|(at, s)| (i.now_ms - at, s.clone()))
            .collect()
    }
}

struct Rig {
    router: axum::Router,
    clock: Arc<ClockTelemetry>,
    _host: FakeHost,
}

/// Links `a-to-b` (direction 0, Deterministic(150 ns): all expected mass in bin 1 of 0..400 / 100)
/// and `b-to-a` (direction 1, Normal), started through the API (the configuration the histogram's
/// expected counts follow is the one the emulation was started with).
fn rig() -> Rig {
    let l0 = link_cfg(
        "a-to-b",
        NIC0,
        NIC1,
        cfg("DeterministicDistribution", 150.0, 0.0),
    );
    let l1 = link_cfg("b-to-a", NIC1, NIC0, cfg("NormalDistribution", 150.0, 30.0));
    let clock = ClockTelemetry::new();
    let host = FakeHost::bare_metal();
    let router = api::router(Deps {
        roots: host.roots(),
        supervisor: FakeSupervisor::new(),
        telemetry: clock.clone(),
        dpds_bin: PathBuf::from("/usr/local/bin/dpds"),
        runtime_dir: PathBuf::from("/run/dpds-controller-test"),
        dpds_has_telemetry: true,
        is_root: false,
    });
    let start = setup(vec![l0, l1]).body();
    let r = block_on({
        let router = router.clone();
        async move { call(&router, "POST", "/api/emulation/start", Some(start)).await }
    });
    assert_eq!(r.status, 202, "{}", r.text());
    Rig {
        router,
        clock,
        _host: host,
    }
}

/// The cumulative histogram `t` seconds into the run (lo 0, 4 bins of 100 ns):
/// bins `[t^2, 2t, 0, t]`, underflow `3t`, overflow a constant 5.
fn at_second(t: u64) -> Snapshot {
    snap(
        hist(0.0, 100.0, &[t * t, 2 * t, 0, t], 3 * t, 5),
        hist(0.0, 100.0, &[t * t, 2 * t, 0, t], 3 * t, 5),
    )
}

/// Polls at the given seconds (the clock advances by the gaps).
fn poll_at(r: &Rig, seconds: &[u64]) {
    let mut last = 0;
    for (i, t) in seconds.iter().enumerate() {
        let gap = if i == 0 { 0 } else { (t - last) * 1000 };
        r.clock.poll(gap, at_second(*t));
        last = *t;
    }
}

async fn get(r: &Rig, query: &str) -> Reply {
    router_get(&r.router, &format!("/api/stats/0/delay_histogram{query}")).await
}

async fn get_ok(r: &Rig, query: &str) -> Value {
    let reply = get(r, query).await;
    assert_eq!(reply.status, 200, "{query}: {}", reply.text());
    reply.json()
}

fn assert_counts(v: &Value, bins: [u64; 4], under: u64, over: u64) {
    assert_eq!(v["bins"], json!(bins), "{v}");
    assert_eq!(v["underflow"], under, "{v}");
    assert_eq!(v["overflow"], over, "{v}");
    assert_eq!(
        v["count"],
        bins.iter().sum::<u64>() + under + over,
        "count includes under- and overflow: {v}"
    );
}

// ---- differences over the window ----------------------------------------------------------------------

/// 401 polls, one per second (t = 0..=400); the counters are quadratic in t, so the difference
/// tells which snapshot was used: `t1^2 - t0^2`.
fn long_run() -> Rig {
    let r = rig();
    poll_at(&r, &(0..=400).collect::<Vec<_>>());
    r
}

#[tokio::test]
async fn window_10_is_the_difference_to_the_snapshot_10_s_ago() {
    let r = long_run();
    // t0 = 390: bins [400^2-390^2, 2*10, 0, 10] = [7900, 20, 0, 10], underflow 30, overflow 0
    let v = get_ok(&r, "?window_s=10").await;
    assert_counts(&v, [7900, 20, 0, 10], 30, 0);
    assert_eq!(v["window_s"], 10);
    assert_eq!(v["span_ms"], 10_000);
}

#[tokio::test]
async fn window_60_is_the_difference_to_the_snapshot_60_s_ago() {
    let r = long_run();
    // t0 = 340: [160000-115600, 120, 0, 60] = [44400, 120, 0, 60], underflow 180
    let v = get_ok(&r, "?window_s=60").await;
    assert_counts(&v, [44_400, 120, 0, 60], 180, 0);
    assert_eq!(v["window_s"], 60);
    assert_eq!(v["span_ms"], 60_000);
}

#[tokio::test]
async fn window_300_is_the_difference_to_the_snapshot_300_s_ago() {
    let r = long_run();
    // t0 = 100: [160000-10000, 600, 0, 300] = [150000, 600, 0, 300], underflow 900
    let v = get_ok(&r, "?window_s=300").await;
    assert_counts(&v, [150_000, 600, 0, 300], 900, 0);
    assert_eq!(v["window_s"], 300);
    assert_eq!(v["span_ms"], 300_000);
}

#[tokio::test]
async fn without_a_window_or_with_zero_the_counts_are_cumulative() {
    let r = long_run();
    for q in ["", "?window_s=0"] {
        let v = get_ok(&r, q).await;
        assert_counts(&v, [160_000, 800, 0, 400], 1200, 5);
        assert_eq!(v["window_s"], 0, "{q}: {v}");
        assert!(v["span_ms"].is_number(), "{q}: {v}");
    }
}

#[tokio::test]
async fn the_geometry_and_freshness_fields_stay_with_a_window() {
    let r = long_run();
    let v = get_ok(&r, "?window_s=10").await;
    assert_eq!(v["dir"], 0);
    assert_eq!(v["edges_ns"], json!([0.0, 100.0, 200.0, 300.0, 400.0]));
    assert_close(f(&v, "lo_ns"), 0.0);
    assert_close(f(&v, "bin_ns"), 100.0);
    assert_eq!(v["age_ms"], 0);
    assert_eq!(v["stale"], false);
}

#[tokio::test]
async fn the_snapshot_used_is_the_newest_one_that_is_at_least_n_seconds_old() {
    // polls at 0, 3, 8, 12, 16 s; latest = 16 s; snapshots >= 10 s old: 0 and 3 -> the newest is
    // t = 3 (13 s), not the closest to 10 s (t = 8 is 8 s old: too young)
    let r = rig();
    poll_at(&r, &[0, 3, 8, 12, 16]);
    let v = get_ok(&r, "?window_s=10").await;
    // [256-9, 2*13, 0, 13], underflow 39, overflow 0
    assert_counts(&v, [247, 26, 0, 13], 39, 0);
    assert_eq!(v["span_ms"], 13_000);
}

#[tokio::test]
async fn a_snapshot_exactly_n_seconds_old_qualifies() {
    let r = rig();
    poll_at(&r, &[0, 4, 10]);
    let v = get_ok(&r, "?window_s=10").await;
    // against t = 0: [100, 20, 0, 10], underflow 30, overflow 0
    assert_counts(&v, [100, 20, 0, 10], 30, 0);
    assert_eq!(v["span_ms"], 10_000);
}

#[tokio::test]
async fn a_snapshot_a_millisecond_too_young_does_not_qualify() {
    let r = rig();
    r.clock.poll(0, at_second(0));
    r.clock.poll(9_999, at_second(10));
    let v = get_ok(&r, "?window_s=10").await;
    // only 9.999 s of run: since start, the cumulative counts (overflow 5 included)
    assert_counts(&v, [100, 20, 0, 10], 30, 5);
}

#[tokio::test]
async fn a_younger_run_than_the_window_returns_everything_since_start() {
    let r = rig();
    poll_at(&r, &[0, 1, 2, 3, 4, 5]);
    for w in [10u64, 60, 300] {
        let v = get_ok(&r, &format!("?window_s={w}")).await;
        // cumulative at t = 5: [25, 10, 0, 5], underflow 15, overflow 5
        assert_counts(&v, [25, 10, 0, 5], 15, 5);
        assert_eq!(v["window_s"], w, "{v}");
        let span = v["span_ms"]
            .as_u64()
            .unwrap_or_else(|| panic!("span_ms: {v}"));
        assert!(span <= 5_000, "covers at most the 5 s of the run: {v}");
    }
}

#[tokio::test]
async fn a_single_poll_is_since_start() {
    let r = rig();
    poll_at(&r, &[7]);
    let v = get_ok(&r, "?window_s=10").await;
    assert_counts(&v, [49, 14, 0, 7], 21, 5);
}

#[tokio::test]
async fn a_longer_window_than_the_history_falls_back_to_since_start() {
    // 100 s of polls, window 300
    let r = rig();
    poll_at(&r, &(0..=100).collect::<Vec<_>>());
    let v = get_ok(&r, "?window_s=300").await;
    assert_counts(&v, [10_000, 200, 0, 100], 300, 5);
}

#[tokio::test]
async fn each_direction_has_its_own_ring_content() {
    let r = rig();
    // direction 1 stands still, direction 0 grows
    for t in 0..=20u64 {
        r.clock.poll(
            if t == 0 { 0 } else { 1000 },
            snap(
                hist(0.0, 100.0, &[t * t, 2 * t, 0, t], 3 * t, 5),
                hist(0.0, 100.0, &[7, 0, 0, 0], 0, 0),
            ),
        );
    }
    let d0 = get_ok(&r, "?window_s=10").await;
    assert_counts(&d0, [400 - 100, 20, 0, 10], 30, 0);
    let d1 = router_get(&r.router, "/api/stats/1/delay_histogram?window_s=10").await;
    assert_eq!(d1.status, 200, "{}", d1.text());
    assert_counts(&d1.json(), [0, 0, 0, 0], 0, 0);
}

// ---- invalid windows ---------------------------------------------------------------------------------------

#[tokio::test]
async fn any_other_window_is_a_400_naming_the_parameter() {
    let r = long_run();
    for q in [
        "5",
        "1",
        "11",
        "30",
        "100",
        "301",
        "3600",
        "-10",
        "10.0",
        "10.5",
        "abc",
        "",
        "1e1",
        "+10",
        "010x",
        "99999999999999999999",
    ] {
        let reply = get(&r, &format!("?window_s={q}")).await;
        assert_eq!(reply.status, 400, "window_s={q:?}: {}", reply.text());
        assert!(
            reply.message().contains("window_s"),
            "window_s={q:?}: {}",
            reply.text()
        );
    }
}

#[tokio::test]
async fn a_bad_window_does_not_depend_on_having_data() {
    // no data at all: either the 400 or the 404 is defensible, but never a 200
    let r = rig();
    let reply = get(&r, "?window_s=7").await;
    assert!(
        reply.status == 400 || reply.status == 404,
        "{}",
        reply.status
    );
    reply.message();
}

#[tokio::test]
async fn an_unknown_direction_with_a_window_is_still_a_404() {
    let r = long_run();
    let reply = router_get(&r.router, "/api/stats/7/delay_histogram?window_s=10").await;
    assert_eq!(reply.status, 404);
}

// ---- reset on start ------------------------------------------------------------------------------------------

#[tokio::test]
async fn after_a_reset_the_old_run_is_not_part_of_the_window() {
    let r = long_run();
    r.clock.clear();
    // the new run: 5 s old, cumulative counters start again from 0
    poll_at(&r, &[0, 1, 2, 3, 4, 5]);
    let v = get_ok(&r, "?window_s=10").await;
    assert_counts(&v, [25, 10, 0, 5], 15, 5);
    // no data between the reset and the first poll
    r.clock.clear();
    assert_eq!(get(&r, "?window_s=10").await.status, 404);
}

// ---- percentiles and expected follow the window ---------------------------------------------------------------

#[tokio::test]
async fn the_percentiles_are_taken_over_the_windowed_counts() {
    let r = long_run();
    // window 10: [7900, 20, 0, 10], underflow 30, count 7960, bins of 100 from 0
    let v = get_ok(&r, "?window_s=10").await;
    assert_eq!(v["count"], 7960);
    let want = [
        ("p50", 50.0),
        ("p90", 90.303_797_468_354_43),
        ("p99", 99.372_151_898_734_17),
        ("p999", 320.4),
    ];
    for (k, w) in want {
        assert_close(num_of(&v["percentiles"][k]), w);
    }
    // the same data since start gives other values (rank 0.5 * 162405 = 81202.5 in bin 0)
    let cum = get_ok(&r, "").await;
    assert_eq!(cum["count"], 162_405);
    assert_close(num_of(&cum["percentiles"]["p50"]), 50.001_562_5);
    assert_close(num_of(&cum["percentiles"]["p999"]), 360.648_75);
}

#[tokio::test]
async fn the_expected_counts_use_the_windowed_sample_count() {
    let r = long_run();
    // direction 0 is Deterministic(150 ns): everything in bin 1 (100..200)
    let v = get_ok(&r, "?window_s=10").await;
    let e = v["expected"].as_array().unwrap();
    assert_eq!(e.len(), 4);
    assert_close(num_of(&e[1]), 7960.0);
    for i in [0usize, 2, 3] {
        assert_close(num_of(&e[i]), 0.0);
    }
    assert_close(num_of(&v["expected_underflow"]), 0.0);
    assert_close(num_of(&v["expected_overflow"]), 0.0);
    let cum = get_ok(&r, "").await;
    assert_close(num_of(&cum["expected"][1]), 162_405.0);
}

// ---- the real poller fills and resets the ring ------------------------------------------------------------------

struct PollerRig {
    poller: Arc<Poller>,
    supervisor: Arc<FakeSupervisor>,
    serial: Arc<AtomicU64>,
    _server: FakeTelemetryServer,
    _dir: tempfile::TempDir,
}

/// A real `Poller` (50 ms interval) against a fake socket whose `/dpds/stats,0` answers
/// `rx_pkts` = a serial number that grows with every query, so each snapshot is identifiable.
fn poller_rig() -> PollerRig {
    let rt = short_tempdir();
    let supervisor = FakeSupervisor::new();
    supervisor.force_status(SupervisorStatus {
        pid: Some(4242),
        started_at: Some(1_700_000_000),
        ..status(State::Running)
    });
    let base = at_second(3);
    let serial = Arc::new(AtomicU64::new(0));
    let s2 = serial.clone();
    let runtime_dir: PathBuf = rt.path().to_path_buf();
    let server = FakeTelemetryServer::start(
        &socket_path(&runtime_dir),
        4242,
        vec![Mode::Serve],
        move |cmd| {
            let (name, param) = cmd.split_once(',').unwrap_or((cmd, ""));
            let idx: usize = param.parse().unwrap_or(usize::MAX);
            match name {
                "/dpds/ports" => Some(base.ports.clone()),
                "/dpds/stats" => base.dirs.get(idx).cloned().map(|mut d| {
                    if idx == 0 {
                        d["rx_pkts"] = json!(s2.fetch_add(1, Ordering::SeqCst) + 1);
                    }
                    d
                }),
                "/dpds/delay_hist" => hist_page(&full_hists(&base), param),
                "/ethdev/stats" => base.eth.get(idx).cloned(),
                _ => None,
            }
        },
    );
    let poller = Poller::spawn(runtime_dir, supervisor.clone());
    // the interval comes with the start body in a real run; nothing stores it any more
    poller.set_poll_interval(50);
    PollerRig {
        poller,
        supervisor,
        serial,
        _server: server,
        _dir: rt,
    }
}

fn serials(h: &[(u64, Snapshot)]) -> Vec<u64> {
    h.iter()
        .map(|(_, s)| s.dirs[0]["rx_pkts"].as_u64().unwrap())
        .collect()
}

async fn wait_until(what: &str, secs: u64, mut pred: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !pred() {
        assert!(
            t0.elapsed() < Duration::from_secs(secs),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_poller_keeps_one_snapshot_per_poll_oldest_first() {
    let r = poller_rig();
    wait_until("four polls", 10, || r.poller.history().len() >= 4).await;
    let h = r.poller.history();
    let s = serials(&h);
    assert!(
        s.windows(2).all(|w| w[0] < w[1]),
        "oldest first, one per poll: {s:?}"
    );
    let ages: Vec<u64> = h.iter().map(|(a, _)| *a).collect();
    assert!(
        ages.windows(2).all(|w| w[0] > w[1]),
        "ages shrink towards the newest poll: {ages:?}"
    );
    // the newest entry is the latest poll
    let latest = r.poller.latest().unwrap();
    assert!(
        latest.cur.dirs[0]["rx_pkts"].as_u64().unwrap() >= *s.last().unwrap(),
        "latest() is at least as new as the newest history entry"
    );
    assert!(ages.last().unwrap() < &1_000, "{ages:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ring_is_empty_when_the_emulation_stops_and_starts_afresh_on_the_next_run() {
    let r = poller_rig();
    wait_until("four polls", 10, || r.poller.history().len() >= 4).await;
    r.supervisor.force_status(status(State::Stopped));
    wait_until("the ring to be cleared", 10, || {
        r.poller.history().is_empty()
    })
    .await;
    assert!(r.poller.latest().is_none());
    let last_old = r.serial.load(Ordering::SeqCst);
    r.supervisor.force_status(SupervisorStatus {
        pid: Some(4242),
        started_at: Some(1_700_000_100),
        ..status(State::Running)
    });
    wait_until("the new run's first polls", 10, || {
        r.poller.history().len() >= 2
    })
    .await;
    let s = serials(&r.poller.history());
    assert!(
        s.iter().all(|x| *x > last_old),
        "snapshots of the previous run leaked into the new ring: {s:?} (old run ended at {last_old})"
    );
}
