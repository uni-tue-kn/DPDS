//! Spec 51 (controller side): C1 `frame_sizes` / `out_of_order` in the counters, T1 controller
//! side, and C2 `GET /api/stats/{dir}/iat_histogram`.
//!
//! # Assumptions (the spec gives behaviour, not names)
//!
//! - `dpds` answers `/dpds/iat_hist,<dir>[,<page>]` like `/dpds/delay_hist` (same eight keys,
//!   same paging); the poller reads it for every link with delay statistics and stores it in the
//!   new `Snapshot` fields `iat_hists` (page 0 without `bins`) and `iat_hist_bins` (all bins),
//!   like `hists` / `hist_bins`; `dpds_controller::stats::iat_histogram(&Snapshot, dir)` is the
//!   counterpart of `delay_histogram`.
//! - `Counters.frame_sizes` is `[u64; 8]` (JSON: an array of 8 integers) and `Counters.out_of_order`
//!   a `u64`, both cumulative passthroughs of the `dpds` stats reply (not deltas), 0 when `dpds`
//!   does not report them; `HistoryTotals` carries the same two.
//! - The iat route has the delay route's shape: `dir`, `lo_ns`, `bin_ns`, `bins`, `underflow`,
//!   `overflow`, `edges_ns`, `count`, `percentiles` (same interpolation as the delay histogram),
//!   no `expected*` keys. 404 without data, for an unknown / non-numeric dir, and for a link
//!   without delay statistics. After the stop it answers the copy (refreshed while serving and
//!   right before the stop, cleared at the next start), like spec 50 A.
//! - The `window_s` parameter is not part of the IAT route and not tested.

use std::sync::Arc;

use crate::common::*;
use crate::fake_telemetry::{hist_page, short_tempdir, FakeTelemetryServer, Mode};
use crate::histogram_extras::{app_with, cfg, hist, snap};
use dpds_controller::stats::{compute_stats, iat_histogram, HistoryTotals, Snapshot};
use dpds_controller::supervisor::{State, SupervisorStatus};
use dpds_controller::telemetry::{poll_once, socket_path, Client};
use serde_json::{json, Value};

const URL: &str = "/api/stats/0/iat_histogram";

fn run() -> TestApp {
    app_with(
        cfg("NormalDistribution", 150.0, 30.0),
        cfg("NormalDistribution", 150.0, 30.0),
    )
}

/// A poll whose direction-0 IAT histogram is `[n, 2n]` (100 ns bins from 0, 3 under, 4 over) and
/// direction 1's is `[7, 7]`; the delay histograms are different (`[1, 1]` / `[2, 2]`).
fn shot(n: u64) -> Snapshot {
    let mut s = snap(
        hist(0.0, 100.0, &[1, 1], 0, 0),
        hist(0.0, 100.0, &[2, 2], 0, 0),
    );
    set_iat_hist(&mut s, 0, hist(0.0, 100.0, &[n, 2 * n], 3, 4));
    set_iat_hist(&mut s, 1, hist(0.0, 50.0, &[7, 7], 0, 0));
    s
}

fn bins(v: &Value) -> Value {
    v["bins"].clone()
}

async fn iat(a: &TestApp) -> Reply {
    a.get(URL).await
}

async fn stop(a: &TestApp) {
    let r = a.post("/api/emulation/stop", json!({})).await;
    assert_eq!(r.status, 202, "{}", r.text());
    // what the real poller does when the run ends
    a.telemetry.cell.lock().unwrap().take();
}

// ---- C2: the live reply ----------------------------------------------------------------------------

#[tokio::test]
async fn the_route_answers_the_iat_histogram_of_the_link_not_the_delay_one() {
    let a = run();
    a.telemetry.set(None, shot(10));
    let r = iat(&a).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(v["dir"], 0);
    assert_eq!(bins(&v), json!([10, 20]));
    assert_eq!(
        (v["underflow"].clone(), v["overflow"].clone()),
        (json!(3), json!(4))
    );
    assert_close(f(&v, "lo_ns"), 0.0);
    assert_close(f(&v, "bin_ns"), 100.0);
    assert_eq!(v["edges_ns"], json!([0.0, 100.0, 200.0]));
    assert_eq!(v["count"], 37);
    // link 1 has its own
    let d1 = a.get("/api/stats/1/iat_histogram").await.json();
    assert_eq!((d1["dir"].clone(), bins(&d1)), (json!(1), json!([7, 7])));
    assert_close(f(&d1, "bin_ns"), 50.0);
    // the delay histogram route is untouched
    let d = a.get("/api/stats/0/delay_histogram").await.json();
    assert_eq!(bins(&d), json!([1, 1]));
}

#[tokio::test]
async fn the_percentiles_interpolate_inside_the_bin_like_the_delay_histogram() {
    let a = run();
    let mut s = shot(10);
    set_iat_hist(&mut s, 0, hist(0.0, 100.0, &[10, 20], 0, 0));
    a.telemetry.set(None, s);
    let v = iat(&a).await.json();
    // 30 samples; the rank q * 30 lies in bin 1: 100 + (rank - 10) / 20 * 100
    for (key, want) in [
        ("p50", 125.0),
        ("p90", 185.0),
        ("p99", 198.5),
        ("p999", 199.85),
    ] {
        assert_close(f(&v["percentiles"], key), want);
    }
    // with an underflow of 3 and an overflow of 4 (37 samples) the median rank 18.5 is in bin 1:
    // 100 + (18.5 - 13) / 20 * 100
    a.telemetry.set(None, shot(10));
    let v = iat(&a).await.json();
    assert_close(f(&v["percentiles"], "p50"), 127.5);
}

#[tokio::test]
async fn the_iat_reply_has_no_expected_values() {
    let a = run();
    a.telemetry.set(None, shot(10));
    let v = iat(&a).await.json();
    for (k, val) in v.as_object().unwrap() {
        assert!(
            !k.starts_with("expected") || val.is_null(),
            "{k} must not be in the IAT reply: {v}"
        );
    }
}

// ---- C2: 404 -------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn without_a_run_or_data_there_is_a_404_with_a_message() {
    let a = TestApp::new(FakeHost::bare_metal());
    assert_eq!(iat(&a).await.status, 404);
    let a = run();
    let r = iat(&a).await;
    assert_eq!(r.status, 404, "{}", r.text());
    r.message();
}

#[tokio::test]
async fn a_link_without_delay_statistics_has_no_iat_histogram() {
    let a = run();
    let mut s = shot(10);
    s.ports["dir0_delay_stats"] = json!(1);
    s.ports["dir1_delay_stats"] = json!(0);
    a.telemetry.set(None, s);
    assert_eq!(iat(&a).await.status, 200);
    let r = a.get("/api/stats/1/iat_histogram").await;
    assert_eq!(r.status, 404, "{}", r.text());
    r.message();
}

#[tokio::test]
async fn a_link_whose_poll_carried_no_iat_histogram_is_a_404() {
    let a = run();
    let mut s = shot(10);
    s.iat_hists[0] = Value::Null;
    s.iat_hist_bins[0] = Vec::new();
    a.telemetry.set(None, s);
    assert_eq!(iat(&a).await.status, 404);
    assert_eq!(a.get("/api/stats/1/iat_histogram").await.status, 200);
}

#[tokio::test]
async fn an_unknown_or_malformed_link_is_a_404() {
    let a = run();
    a.telemetry.set(None, shot(10));
    for dir in ["7", "x", "-1", "0x0"] {
        let r = a.get(&format!("/api/stats/{dir}/iat_histogram")).await;
        assert_eq!(r.status, 404, "{dir}: {}", r.text());
    }
}

// ---- C2: the copy after the stop (spec 50 A) -----------------------------------------------------------

#[tokio::test]
async fn the_copy_of_the_last_served_iat_histogram_is_answered_after_the_stop() {
    let a = run();
    a.telemetry.set(None, shot(10));
    assert_eq!(iat(&a).await.status, 200);
    // the stop's own refresh fails: the older copy stays
    a.telemetry.cell.lock().unwrap().take();
    stop(&a).await;
    let r = iat(&a).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!((v["dir"].clone(), bins(&v)), (json!(0), json!([10, 20])));
    assert_eq!(v["count"], 37);
    assert!(v["percentiles"]["p50"].is_number(), "{v}");
}

#[tokio::test]
async fn the_copy_is_refreshed_while_serving_and_right_before_the_stop() {
    let a = run();
    a.telemetry.set(None, shot(10));
    iat(&a).await;
    a.telemetry.set(None, shot(30));
    iat(&a).await;
    a.telemetry.set(None, shot(50)); // nobody asked for this one
    stop(&a).await;
    assert_eq!(bins(&iat(&a).await.json()), json!([50, 100]));
}

#[tokio::test]
async fn a_run_nobody_looked_at_still_has_the_copy_of_both_links() {
    let a = run();
    a.telemetry.set(None, shot(50));
    stop(&a).await;
    assert_eq!(bins(&iat(&a).await.json()), json!([50, 100]));
    assert_eq!(
        bins(&a.get("/api/stats/1/iat_histogram").await.json()),
        json!([7, 7])
    );
}

#[tokio::test]
async fn a_failed_run_serves_the_copy_taken_while_serving() {
    let a = run();
    a.telemetry.set(None, shot(10));
    iat(&a).await;
    a.telemetry.cell.lock().unwrap().take();
    a.supervisor.force_status(SupervisorStatus {
        error: Some("dpds exited".into()),
        ..status(State::Failed)
    });
    let r = iat(&a).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(bins(&r.json()), json!([10, 20]));
}

#[tokio::test]
async fn a_link_without_delay_statistics_has_no_copy_either() {
    let a = run();
    let mut s = shot(10);
    s.ports["dir1_delay_stats"] = json!(0);
    a.telemetry.set(None, s);
    stop(&a).await;
    assert_eq!(iat(&a).await.status, 200);
    assert_eq!(a.get("/api/stats/1/iat_histogram").await.status, 404);
}

#[tokio::test]
async fn a_run_that_never_had_data_has_no_copy() {
    let a = run();
    stop(&a).await;
    assert_eq!(iat(&a).await.status, 404);
}

#[tokio::test]
async fn while_running_the_copy_does_not_stand_in_for_missing_live_data() {
    let a = run();
    a.telemetry.set(None, shot(10));
    iat(&a).await;
    a.telemetry.cell.lock().unwrap().take();
    assert_eq!(iat(&a).await.status, 404);
}

#[tokio::test]
async fn the_copy_is_cleared_at_the_next_start() {
    let a = run();
    a.telemetry.set(None, shot(10));
    stop(&a).await;
    assert_eq!(iat(&a).await.status, 200);
    let r = a.start_links(two_links()).await; // run 2 never gets data
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(iat(&a).await.status, 404, "while running");
    stop(&a).await;
    let r = iat(&a).await;
    assert_eq!(r.status, 404, "the first run's copy leaked: {}", r.text());
}

#[tokio::test]
async fn a_new_run_copies_its_own_iat_histogram() {
    let a = run();
    a.telemetry.set(None, shot(10));
    stop(&a).await;
    let r = a.start_links(two_links()).await;
    assert_eq!(r.status, 202, "{}", r.text());
    a.telemetry.set(None, shot(3));
    assert_eq!(bins(&iat(&a).await.json()), json!([3, 6]));
    stop(&a).await;
    assert_eq!(bins(&iat(&a).await.json()), json!([3, 6]));
}

// ---- C2: the poller reads /dpds/iat_hist -------------------------------------------------------------

#[test]
fn poll_once_reads_the_iat_histogram_of_every_link_with_delay_statistics() {
    let mut s = shot(10);
    s.ports["dir0_delay_stats"] = json!(1);
    s.ports["dir1_delay_stats"] = json!(0);
    let delay = full_hists(&s);
    let iats: Vec<Value> = (0..2)
        .map(|d| {
            let mut full = s.iat_hists[d].clone();
            full["bins"] = json!(s.iat_hist_bins[d]);
            full
        })
        .collect();
    let snapshot = Arc::new(s);
    let sn = snapshot.clone();
    let dir = short_tempdir();
    let path = socket_path(dir.path());
    let server = FakeTelemetryServer::start(&path, 4242, vec![Mode::Serve], move |cmd| {
        let (name, param) = cmd.split_once(',').unwrap_or((cmd, ""));
        let idx: usize = param.parse().unwrap_or(usize::MAX);
        match name {
            "/dpds/ports" => Some(sn.ports.clone()),
            "/dpds/stats" => sn.dirs.get(idx).cloned(),
            "/dpds/delay_hist" => hist_page(&delay, param),
            "/dpds/iat_hist" => hist_page(&iats, param),
            "/ethdev/xstats" => sn.eth.get(idx).cloned(),
            _ => None,
        }
    });
    let (mut client, _) = Client::connect(&path).expect("connect");
    let polled = poll_once(&mut client).expect("a poll");
    let h = iat_histogram(&polled, 0).expect("link 0 has an IAT histogram");
    assert_eq!(h.bins, vec![10, 20]);
    assert_eq!((h.underflow, h.overflow), (3, 4));
    assert_eq!(h.bin_ns, 100.0);
    assert!(iat_histogram(&polled, 1).is_none(), "delay statistics off");
    assert!(
        server
            .queries()
            .iter()
            .any(|q| q.starts_with("/dpds/iat_hist,0")),
        "{:?}",
        server.queries()
    );
}

// ---- C1 / T1: frame_sizes and out_of_order -----------------------------------------------------------------

fn counters_of(s: &Snapshot, prev: Option<&Snapshot>) -> Vec<Value> {
    let v = serde_json::to_value(compute_stats(prev, s)).unwrap();
    v["directions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["counters"].clone())
        .collect()
}

#[test]
fn counters_carry_the_frame_size_buckets_and_out_of_order_of_each_link() {
    let mut s = shot(1);
    s.dirs[0]["frame_sizes"] = json!([1, 2, 3, 4, 5, 6, 7, 8]);
    s.dirs[0]["out_of_order"] = json!(42);
    s.dirs[1]["frame_sizes"] = json!([80, 70, 60, 50, 40, 30, 20, 10]);
    s.dirs[1]["out_of_order"] = json!(0);
    let c = counters_of(&s, None);
    assert_eq!(c[0]["frame_sizes"], json!([1, 2, 3, 4, 5, 6, 7, 8]));
    assert_eq!(c[0]["out_of_order"], 42);
    assert_eq!(c[1]["frame_sizes"], json!([80, 70, 60, 50, 40, 30, 20, 10]));
    assert_eq!(c[1]["out_of_order"], 0);
}

#[test]
fn the_counters_are_cumulative_not_window_deltas() {
    let mut prev = shot(1);
    prev.dirs[0]["frame_sizes"] = json!([1, 1, 1, 1, 1, 1, 1, 1]);
    prev.dirs[0]["out_of_order"] = json!(5);
    let mut cur = shot(1);
    cur.dirs[0]["frame_sizes"] = json!([3, 4, 5, 6, 7, 8, 9, 10]);
    cur.dirs[0]["out_of_order"] = json!(9);
    let c = counters_of(&cur, Some(&prev));
    assert_eq!(c[0]["frame_sizes"], json!([3, 4, 5, 6, 7, 8, 9, 10]));
    assert_eq!(c[0]["out_of_order"], 9);
}

#[test]
fn a_dpds_that_does_not_report_them_gives_zeros_not_a_missing_key() {
    let mut s = shot(1);
    for d in &mut s.dirs {
        let o = d.as_object_mut().unwrap();
        o.remove("frame_sizes");
        o.remove("out_of_order");
    }
    for c in counters_of(&s, None) {
        assert_eq!(c["frame_sizes"], json!([0, 0, 0, 0, 0, 0, 0, 0]), "{c}");
        assert_eq!(c["out_of_order"], 0, "{c}");
    }
}

#[tokio::test]
async fn api_stats_serves_them_under_counters() {
    let a = run();
    let mut s = shot(1);
    s.dirs[0]["frame_sizes"] = json!([9, 8, 7, 6, 5, 4, 3, 2]);
    s.dirs[0]["out_of_order"] = json!(123);
    a.telemetry.set(None, s);
    let v = a.get("/api/stats").await.json();
    let c = &v["directions"][0]["counters"];
    assert_eq!(c["frame_sizes"], json!([9, 8, 7, 6, 5, 4, 3, 2]), "{v}");
    assert_eq!(c["out_of_order"], 123);
}

#[test]
fn the_history_totals_carry_them_too() {
    let mut s = shot(1);
    s.dirs[0]["frame_sizes"] = json!([1, 2, 3, 4, 5, 6, 7, 8]);
    s.dirs[0]["out_of_order"] = json!(17);
    let view = compute_stats(None, &s);
    let t = serde_json::to_value(HistoryTotals::from(&view.directions[0].counters)).unwrap();
    assert_eq!(t["frame_sizes"], json!([1, 2, 3, 4, 5, 6, 7, 8]));
    assert_eq!(t["out_of_order"], 17);
}

#[tokio::test]
async fn openapi_documents_the_new_fields_and_the_route() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    for (name, marker) in [("Counters", "tx_drops"), ("HistoryTotals", "nic_tx_drops")] {
        let s = schemas
            .get(name)
            .unwrap_or_else(|| panic!("no {name} schema: {:?}", schemas.keys()));
        let props = s["properties"].as_object().unwrap();
        assert!(props.contains_key(marker), "{name}");
        assert!(props.contains_key("out_of_order"), "{name}: {props:?}");
        let fs = &props["frame_sizes"];
        assert_eq!(fs["type"], "array", "{name}: {fs}");
    }
    let op = &doc["paths"]["/api/stats/{dir}/iat_histogram"]["get"];
    assert!(op.is_object(), "no GET /api/stats/{{dir}}/iat_histogram");
    for code in ["200", "404"] {
        assert!(op["responses"].get(code).is_some(), "{code}: {op}");
    }
}

fn two_links() -> Vec<Value> {
    vec![
        link_cfg("a-to-b", NIC0, NIC1, cfg("NormalDistribution", 150.0, 30.0)),
        link_cfg("b-to-a", NIC1, NIC0, cfg("NormalDistribution", 150.0, 30.0)),
    ]
}
