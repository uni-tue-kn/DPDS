//! The delay histogram of a link stays available after the run ended.
//!
//! The controller keeps, per link of the run, a copy of the last since-start histogram it got
//! from dpds: refreshed whenever a since-start histogram is served while running, and once more
//! for every link with delay statistics right before dpds is stopped (a failure there keeps the
//! older copy). The copy carries `mean_ns` / `std_ns`, the link's `delay_mean_ns` /
//! `delay_std_ns` of the last stats poll (null when unknown; the live reply has them too).
//! After the run ended (stopped or failed) `GET /api/stats/{dir}/delay_histogram` answers the
//! copy for any valid `window_s`, with `window_s: 0`; 404 without a copy. The copies are cleared
//! at the next start.
//!
//! The fake telemetry stands in for dpds: what `FakeTelemetry::latest` answers is what dpds
//! "still answers" right before the stop. A real poller clears its cache when the run ends, so
//! the tests set the fake to `None` after the stop. `None` at the stop models the failing
//! refresh.
//!
//! Not covered here: the stop half of a restart (the controller has no restart route), a windowed
//! reply that was really subtracted becoming the copy (the fake has no ring), and the real poller.

use crate::common::*;
use crate::histogram_extras::{app_with, cfg, get_hist, hist, snap};
use dpds_controller::stats::Snapshot;
use dpds_controller::supervisor::{State, Supervisor, SupervisorStatus};
use serde_json::{json, Value};

const URL: &str = "/api/stats/0/delay_histogram";

/// A started two-link run (both Normal); the tests feed its telemetry with [`shot`].
fn run() -> TestApp {
    app_with(
        cfg("NormalDistribution", 150.0, 30.0),
        cfg("NormalDistribution", 150.0, 30.0),
    )
}

/// A poll whose direction-0 histogram has `n` samples in bin 0 (bins `[n, 2n]`, 3 under, 4 over)
/// and whose direction-1 histogram is `[7, 7]`; the stats of direction 0 say `mean` / `std`.
fn shot(n: u64, mean: f64, std: f64) -> Snapshot {
    let mut s = snap(
        hist(0.0, 100.0, &[n, 2 * n], 3, 4),
        hist(0.0, 100.0, &[7, 7], 0, 0),
    );
    s.dirs[0]["delay_mean_ns"] = json!(mean.to_string());
    s.dirs[0]["delay_std_ns"] = json!(std.to_string());
    s
}

fn bins(v: &Value) -> Value {
    v["bins"].clone()
}

async fn stop(a: &TestApp) {
    let r = a.post("/api/emulation/stop", json!({})).await;
    assert_eq!(r.status, 202, "{}", r.text());
    // what the real poller does when the run ends
    a.telemetry.cell.lock().unwrap().take();
}

async fn after(a: &TestApp, query: &str) -> Reply {
    a.get(&format!("{URL}{query}")).await
}

// ---- the copy is taken while serving ------------------------------------------------------------

#[tokio::test]
async fn the_copy_of_the_last_served_histogram_is_answered_after_the_stop() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    assert_eq!(bins(&get_hist(&a, 0).await), json!([10, 20]));
    // the stop's own refresh fails: the older copy stays
    a.telemetry.cell.lock().unwrap().take();
    stop(&a).await;
    let r = after(&a, "").await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(v["dir"], 0);
    assert_eq!(bins(&v), json!([10, 20]));
    assert_eq!(v["underflow"], 3);
    assert_eq!(v["overflow"], 4);
    assert_eq!(v["count"], 37);
    assert_eq!(v["window_s"], 0);
    assert_close(f(&v, "lo_ns"), 0.0);
    assert_close(f(&v, "bin_ns"), 100.0);
    assert!(v["percentiles"]["p50"].is_number(), "{v}");
}

#[tokio::test]
async fn every_since_start_reply_while_running_refreshes_the_copy() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    get_hist(&a, 0).await;
    a.telemetry.set(None, shot(30, 2000.0, 200.0));
    get_hist(&a, 0).await;
    a.telemetry.cell.lock().unwrap().take();
    stop(&a).await;
    let v = after(&a, "").await.json();
    assert_eq!(bins(&v), json!([30, 60]));
    assert_close(f(&v, "mean_ns"), 2000.0);
}

#[tokio::test]
async fn the_copy_is_refreshed_right_before_the_stop() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    get_hist(&a, 0).await;
    // dpds moved on without any request in between; it still answers at the stop
    a.telemetry.set(None, shot(50, 3000.0, 300.0));
    stop(&a).await;
    let v = after(&a, "").await.json();
    assert_eq!(bins(&v), json!([50, 100]), "{v}");
    assert_close(f(&v, "mean_ns"), 3000.0);
    assert_close(f(&v, "std_ns"), 300.0);
}

#[tokio::test]
async fn a_run_nobody_looked_at_still_has_its_copy_from_the_stop() {
    let a = run();
    a.telemetry.set(None, shot(50, 3000.0, 300.0));
    stop(&a).await;
    let v = after(&a, "").await.json();
    assert_eq!(bins(&v), json!([50, 100]), "{v}");
    // the other link has its copy too
    let d1 = get_hist(&a, 1).await;
    assert_eq!(bins(&d1), json!([7, 7]));
    assert_eq!(d1["dir"], 1);
}

#[tokio::test]
async fn a_run_that_failed_serves_the_copy_taken_while_serving() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    get_hist(&a, 0).await;
    a.telemetry.cell.lock().unwrap().take();
    a.supervisor.force_status(SupervisorStatus {
        error: Some("dpds exited".into()),
        ..status(State::Failed)
    });
    let r = after(&a, "").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(bins(&r.json()), json!([10, 20]));
}

// ---- after the stop: any window, window_s 0 ---------------------------------------------------------

#[tokio::test]
async fn after_the_stop_any_window_gets_the_copy_with_window_s_zero() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    stop(&a).await;
    for q in [
        "",
        "?window_s=0",
        "?window_s=10",
        "?window_s=60",
        "?window_s=300",
    ] {
        let r = after(&a, q).await;
        assert_eq!(r.status, 200, "{q}: {}", r.text());
        let v = r.json();
        assert_eq!(v["window_s"], 0, "{q}: {v}");
        assert_eq!(bins(&v), json!([10, 20]), "{q}");
        assert_eq!(v["count"], 37, "{q}");
    }
}

#[tokio::test]
async fn after_the_stop_an_invalid_window_is_still_a_400() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    stop(&a).await;
    let r = after(&a, "?window_s=7").await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("window_s"));
}

// ---- 404 without a copy --------------------------------------------------------------------------------------

#[tokio::test]
async fn without_a_run_there_is_no_copy() {
    let a = TestApp::new(FakeHost::bare_metal());
    assert_eq!(after(&a, "").await.status, 404);
}

#[tokio::test]
async fn a_run_that_never_had_data_has_no_copy() {
    let a = run();
    stop(&a).await;
    for q in ["", "?window_s=10"] {
        let r = after(&a, q).await;
        assert_eq!(r.status, 404, "{q}: {}", r.text());
        r.message();
    }
}

#[tokio::test]
async fn a_link_without_delay_statistics_has_no_copy_and_the_other_one_has() {
    let a = run();
    let mut s = shot(10, 1000.0, 100.0);
    s.ports["dir0_delay_stats"] = json!(1);
    s.ports["dir1_delay_stats"] = json!(0);
    a.telemetry.set(None, s);
    stop(&a).await;
    assert_eq!(after(&a, "").await.status, 200);
    let r = a.get("/api/stats/1/delay_histogram").await;
    assert_eq!(r.status, 404, "{}", r.text());
}

#[tokio::test]
async fn an_unknown_link_has_no_copy() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    stop(&a).await;
    assert_eq!(a.get("/api/stats/7/delay_histogram").await.status, 404);
    assert_eq!(a.get("/api/stats/x/delay_histogram").await.status, 404);
}

#[tokio::test]
async fn while_running_the_copy_is_not_served_in_place_of_missing_live_data() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    get_hist(&a, 0).await;
    a.telemetry.cell.lock().unwrap().take();
    let r = after(&a, "").await;
    assert_eq!(r.status, 404, "{}", r.text());
}

#[tokio::test]
async fn while_running_the_reply_is_the_live_one_not_the_copy() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    get_hist(&a, 0).await;
    a.telemetry.set(None, shot(11, 1000.0, 100.0));
    assert_eq!(bins(&get_hist(&a, 0).await), json!([11, 22]));
}

// ---- mean_ns and std_ns --------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_live_reply_carries_mean_and_std_of_the_latest_stats_poll() {
    let a = run();
    a.telemetry.set(None, shot(10, 1234.5, 67.25));
    let v = get_hist(&a, 0).await;
    assert_close(f(&v, "mean_ns"), 1234.5);
    assert_close(f(&v, "std_ns"), 67.25);
    // a windowed reply has them too
    let w = after(&a, "?window_s=10").await.json();
    assert_close(f(&w, "mean_ns"), 1234.5);
    assert_close(f(&w, "std_ns"), 67.25);
}

#[tokio::test]
async fn the_copy_carries_mean_and_std() {
    let a = run();
    a.telemetry.set(None, shot(10, 1234.5, 67.25));
    stop(&a).await;
    let v = after(&a, "").await.json();
    assert_close(f(&v, "mean_ns"), 1234.5);
    assert_close(f(&v, "std_ns"), 67.25);
    // the real fake values (decimal strings from dpds) come through as numbers
    let b = run();
    b.telemetry.set(
        None,
        snap(hist(0.0, 100.0, &[1], 0, 0), hist(0.0, 100.0, &[1], 0, 0)),
    );
    stop(&b).await;
    let v = after(&b, "").await.json();
    assert_close(f(&v, "mean_ns"), 10_000_000.25);
    assert_close(f(&v, "std_ns"), 1_000_000.50);
}

#[tokio::test]
async fn mean_and_std_are_per_link() {
    let a = run();
    let mut s = shot(10, 1000.0, 100.0);
    s.dirs[1]["delay_mean_ns"] = json!("2000");
    s.dirs[1]["delay_std_ns"] = json!("200");
    a.telemetry.set(None, s);
    stop(&a).await;
    let d1 = a.get("/api/stats/1/delay_histogram").await.json();
    assert_close(f(&d1, "mean_ns"), 2000.0);
    assert_close(f(&d1, "std_ns"), 200.0);
}

#[tokio::test]
async fn unknown_mean_and_std_are_null_not_missing() {
    let a = run();
    let mut s = shot(10, 1000.0, 100.0);
    s.dirs[0]["delay_mean_ns"] = Value::Null;
    s.dirs[0].as_object_mut().unwrap().remove("delay_std_ns");
    a.telemetry.set(None, s);
    let live = get_hist(&a, 0).await;
    stop(&a).await;
    let copy = after(&a, "").await.json();
    for v in [live, copy] {
        let o = v.as_object().unwrap();
        assert!(o.get("mean_ns").is_some_and(Value::is_null), "{v}");
        assert!(o.get("std_ns").is_some_and(Value::is_null), "{v}");
    }
}

#[tokio::test]
async fn mean_and_std_come_from_the_last_poll_not_from_an_older_serve() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    get_hist(&a, 0).await;
    a.telemetry.set(None, shot(10, 5000.0, 500.0));
    stop(&a).await;
    let v = after(&a, "").await.json();
    assert_close(f(&v, "mean_ns"), 5000.0);
    assert_close(f(&v, "std_ns"), 500.0);
}

#[tokio::test]
async fn the_histogram_view_documents_mean_and_std_in_openapi() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    let props = &doc["components"]["schemas"]["HistogramView"]["properties"];
    for k in ["mean_ns", "std_ns"] {
        assert!(props.get(k).is_some(), "{k} missing: {props}");
    }
}

// ---- cleared at the next start ----------------------------------------------------------------------------------

#[tokio::test]
async fn the_copy_is_cleared_at_the_next_start() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    stop(&a).await;
    assert_eq!(after(&a, "").await.status, 200);
    // run 2 never gets any data
    let r = a.start_links(two_links()).await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(after(&a, "").await.status, 404, "while running");
    stop(&a).await;
    let r = after(&a, "").await;
    assert_eq!(r.status, 404, "the first run's copy leaked: {}", r.text());
}

#[tokio::test]
async fn a_new_run_serves_and_copies_its_own_histogram() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    stop(&a).await;
    let r = a.start_links(two_links()).await;
    assert_eq!(r.status, 202, "{}", r.text());
    a.telemetry.set(None, shot(3, 9000.0, 900.0));
    assert_eq!(bins(&get_hist(&a, 0).await), json!([3, 6]));
    a.telemetry.cell.lock().unwrap().take();
    stop(&a).await;
    let v = after(&a, "").await.json();
    assert_eq!(bins(&v), json!([3, 6]));
    assert_close(f(&v, "mean_ns"), 9000.0);
}

#[tokio::test]
async fn a_start_that_fails_leaves_no_copy_of_the_run_before() {
    let a = run();
    a.telemetry.set(None, shot(10, 1000.0, 100.0));
    stop(&a).await;
    a.supervisor.fail_next_start("boom", &[]);
    let r = a.start_links(two_links()).await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(a.supervisor.status().state, State::Failed);
    let r = after(&a, "").await;
    assert_eq!(r.status, 404, "{}", r.text());
}

fn two_links() -> Vec<Value> {
    vec![
        link_cfg(NIC0, NIC1, cfg("NormalDistribution", 150.0, 30.0)),
        link_cfg(NIC1, NIC0, cfg("NormalDistribution", 150.0, 30.0)),
    ]
}
