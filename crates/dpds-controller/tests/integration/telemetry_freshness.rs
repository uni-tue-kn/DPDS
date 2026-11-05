//! C4: telemetry freshness, through the REAL `Poller` against a fake telemetry socket
//! (`fake_telemetry.rs`) behind the REST router.
//!
//! `GET /api/stats` carries `age_ms` (milliseconds since the last successful poll,
//! a number) and `stale` (bool): fresh data is `stale: false`; after 5 consecutive poll
//! failures the cache is cleared and the endpoint answers `directions: []`,
//! `ports: []`, `stale: true` (and the histogram endpoint 404). A poll that works again
//! makes the data fresh again. Fewer than 5 failures keep the last data (with a growing
//! `age_ms`).
//!
//! `Polled` itself carries the same two values (`age_ms: u64`, `stale: bool`); the tests
//! only use them through the endpoint, so the fake `FakeTelemetry` of the other API tests
//! must construct `Polled` with `age_ms: 0, stale: false`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::common::*;
use crate::fake_telemetry::{short_tempdir, FakeTelemetryServer, Mode};
use dpds_controller::api::{self, Deps};
use dpds_controller::supervisor::{State, SupervisorStatus};
use dpds_controller::telemetry::{socket_path, Poller, TelemetrySource};
use serde_json::{json, Value};

struct Rig {
    router: axum::Router,
    server: FakeTelemetryServer,
    _poller: Arc<Poller>,
    _dir: tempfile::TempDir,
}

/// Spec 41: the poll interval is a fixed 500 ms; `Poller::spawn(runtime_dir, supervisor)` uses
/// it. The tests keep `Poller::set_poll_interval` as a speed hook (ASSUMPTION of the tester: it
/// stays, as an inherent method, for the tests that must not wait whole seconds).
fn rig(interval_ms: u64) -> Rig {
    let rt = short_tempdir();
    let supervisor = FakeSupervisor::new();
    supervisor.force_status(SupervisorStatus {
        pid: Some(4242),
        started_at: Some(1_700_000_000),
        ..status(State::Running)
    });
    let snap = snapshot(
        [
            DirSample {
                uptime_ns: 5_000_000_000,
                rx_pkts: 100,
                tx_sent: 100,
                loss: 0,
                limiter: 0,
                full: 0,
                tx_drops: 0,
            },
            DirSample {
                uptime_ns: 5_000_000_000,
                rx_pkts: 50,
                tx_sent: 50,
                loss: 0,
                limiter: 0,
                full: 0,
                tx_drops: 0,
            },
        ],
        [
            PortSample {
                ibytes: 10_000,
                obytes: 5_000,
                imissed: 0,
                rx_nombuf: 0,
            },
            PortSample {
                ibytes: 5_000,
                obytes: 10_000,
                imissed: 0,
                rx_nombuf: 0,
            },
        ],
    );
    let runtime_dir: PathBuf = rt.path().to_path_buf();
    let server = FakeTelemetryServer::serving(&socket_path(&runtime_dir), 4242, snap);
    let poller = Poller::spawn(runtime_dir.clone(), supervisor.clone());
    poller.set_poll_interval(interval_ms);
    let router = api::router(Deps {
        roots: FakeHost::bare_metal().roots(),
        supervisor,
        telemetry: poller.clone(),
        dpds_bin: PathBuf::from("/usr/local/bin/dpds"),
        runtime_dir,
        dpds_has_telemetry: true,
        is_root: false,
    });
    Rig {
        router,
        server,
        _poller: poller,
        _dir: rt,
    }
}

async fn stats(r: &Rig) -> Value {
    let reply = router_get(&r.router, "/api/stats").await;
    assert_eq!(reply.status, 200);
    reply.json()
}

fn has_data(v: &Value) -> bool {
    !v["directions"].as_array().unwrap().is_empty()
}

async fn wait_for(r: &Rig, what: &str, secs: u64, pred: impl Fn(&Value) -> bool) -> Value {
    let t0 = Instant::now();
    loop {
        let v = stats(r).await;
        if pred(&v) {
            return v;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(secs),
            "timed out waiting for {what}; last {v}; queries so far {}",
            r.server.queries().len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_data_has_a_small_age_and_is_not_stale() {
    let r = rig(100);
    let v = wait_for(&r, "the first poll", 10, has_data).await;
    assert_eq!(v["stale"], false, "{v}");
    let age = v["age_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("age_ms is a number: {v}"));
    assert!(age < 2_000, "{v}");
    assert_eq!(v["directions"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_age_grows_while_polls_fail_and_five_failures_clear_the_cache() {
    let r = rig(400);
    wait_for(&r, "the first poll", 10, has_data).await;
    // every new connection now answers null (a failed poll), the open one is dropped
    r.server.set_modes(vec![Mode::Nulls]);
    r.server.break_all();
    let t_break = Instant::now();

    // after ~1 s at most 3 polls failed: the last data is still served, and old
    tokio::time::sleep(Duration::from_millis(900)).await;
    let v = stats(&r).await;
    assert!(
        has_data(&v),
        "fewer than 5 failures must keep the cache: {v} after {:?}",
        t_break.elapsed()
    );
    let age = v["age_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("age_ms: {v}"));
    assert!(
        age >= 800,
        "age after >= 900 ms without a successful poll: {v}"
    );

    // the 5th failure (at most ~2.5 s after the break) clears it
    let v = wait_for(&r, "the cache to be cleared", 8, |v| !has_data(v)).await;
    assert_eq!(v["stale"], true, "{v}");
    assert_eq!(v["directions"], json!([]));
    assert_eq!(v["ports"], json!([]));
    assert_eq!(
        router_get(&r.router, "/api/stats/0/delay_histogram")
            .await
            .status,
        404
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_working_poll_makes_the_data_fresh_again() {
    let r = rig(200);
    wait_for(&r, "the first poll", 10, has_data).await;
    r.server.set_modes(vec![Mode::Nulls]);
    r.server.break_all();
    wait_for(&r, "the cache to be cleared", 10, |v| !has_data(v)).await;
    r.server.set_modes(vec![Mode::Serve]);
    r.server.break_all();
    let v = wait_for(&r, "fresh data", 10, has_data).await;
    assert_eq!(v["stale"], false, "{v}");
    assert!(v["age_ms"].as_u64().unwrap() < 2_000, "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_errors_count_as_failures_too() {
    let r = rig(200);
    wait_for(&r, "the first poll", 10, has_data).await;
    r.server.set_modes(vec![Mode::CloseAfterGreeting]);
    r.server.break_all();
    let v = wait_for(&r, "the cache to be cleared", 10, |v| !has_data(v)).await;
    assert_eq!(v["stale"], true, "{v}");
}

fn one_dir() -> DirSample {
    DirSample {
        uptime_ns: 1,
        rx_pkts: 1,
        tx_sent: 1,
        loss: 0,
        limiter: 0,
        full: 0,
        tx_drops: 0,
    }
}

fn one_port() -> PortSample {
    PortSample {
        ibytes: 1,
        obytes: 1,
        imissed: 0,
        rx_nombuf: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_poller_polls_every_500_ms_by_default() {
    // a real poller with its default interval, a fake dpds socket and a fake supervisor
    let rt = short_tempdir();
    let runtime_dir: PathBuf = rt.path().to_path_buf();
    let server = FakeTelemetryServer::serving(
        &socket_path(&runtime_dir),
        4242,
        snapshot([one_dir(), one_dir()], [one_port(), one_port()]),
    );
    let supervisor = FakeSupervisor::new();
    supervisor.force_status(SupervisorStatus {
        pid: Some(4242),
        started_at: Some(1_700_000_000),
        ..status(State::Running)
    });
    let _poller = Poller::spawn(runtime_dir, supervisor);
    tokio::time::sleep(Duration::from_millis(600)).await; // the first poll has happened
    let count = || {
        server
            .queries()
            .iter()
            .filter(|q| q.starts_with("/dpds/ports"))
            .count()
    };
    let before = count();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let polls = count() - before;
    assert!(
        (3..=5).contains(&polls),
        "500 ms must give 3 to 5 polls in 2 s, got {polls}"
    );
}
