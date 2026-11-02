//! Spec v7 S5 (the Lows that are visible from outside):
//!
//! - the hugepage figures from `/proc/meminfo` saturate instead of overflowing;
//! - unknown routes (404) and wrong methods (405) answer `{"message"}` JSON;
//! - the histogram endpoint carries `age_ms` and `stale` like `/api/stats`, and a direction
//!   with delay statistics off is a 404 whose message says so;
//! - `ConfigDto` and its nested DTOs deny unknown fields: a typo such as `seeed` is a 400
//!   naming it, not a silently dropped setting;
//! - an empty or invalid body WITH `Content-Type: application/json` is a 400 `{"message"}`.
//!
//! Not covered here (not observable without a slow disk): the store's fsync running outside
//! the state mutex. `--runtime-dir` too long is in `startup_errors.rs`.

use crate::common::*;
use dpds_controller::dto::ConfigDto;
use dpds_controller::stats::{Polled, Snapshot};
use serde_json::{json, Value};

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

// ---- hugepages from /proc/meminfo saturate ---------------------------------------------

fn meminfo_only_host(total: &str, free: &str, size_kb: &str) -> FakeHost {
    let h = FakeHost::bare_metal();
    // no sysfs hugepage directories: the controller falls back to /proc/meminfo
    h.remove("sys/kernel/mm/hugepages");
    h.remove("sys/devices/system/node/node0/hugepages");
    h.remove("sys/devices/system/node/node1/hugepages");
    h.write(
        "proc/meminfo",
        &format!(
            "MemTotal:       263939432 kB\nHugePages_Total:    {total}\n\
             HugePages_Free:     {free}\nHugepagesize:       {size_kb} kB\n"
        ),
    );
    h
}

#[test]
fn absurd_meminfo_hugepage_figures_saturate_instead_of_overflowing() {
    let max = u64::MAX.to_string();
    for (total, free, size_kb) in [
        (max.as_str(), max.as_str(), "2048"), // pages * page size overflows
        ("1024", "1024", max.as_str()),       // kB * 1024 overflows
        (max.as_str(), max.as_str(), max.as_str()),
    ] {
        let h = meminfo_only_host(total, free, size_kb);
        let state = state_with(vec![link(1, "a", &["net_null0"], None)]);
        // in a debug build an unchecked multiplication panics here
        let hints = hints_json(&h, &state);
        let hp = hint(&hints, "hugepages");
        assert_eq!(hp["level"], "ok", "{total}/{free}/{size_kb}: {hp}");
    }
}

// ---- unknown routes and wrong methods are JSON -----------------------------------------

#[tokio::test]
async fn unknown_routes_are_a_404_with_a_json_message() {
    let a = app();
    for (method, path) in [
        ("GET", "/api/nope"),
        ("GET", "/api/links/1/extra"),
        ("POST", "/api/nope"),
        ("GET", "/nope"),
        ("GET", "/"),
    ] {
        let r = a.request(method, path, None).await;
        assert_eq!(r.status, 404, "{method} {path}: {}", r.text());
        r.message();
    }
}

#[tokio::test]
async fn wrong_methods_are_a_405_with_a_json_message() {
    let a = app();
    for (method, path) in [
        ("DELETE", "/api/settings"),
        ("POST", "/api/online"),
        ("PUT", "/api/stats"),
        ("GET", "/api/emulation/start"),
        ("GET", "/api/emulation/stop"),
        ("PATCH", "/api/links"),
        ("POST", "/api/links/1"),
        ("DELETE", "/api/emulation"),
    ] {
        let r = a.request(method, path, None).await;
        assert_eq!(r.status, 405, "{method} {path}: {}", r.text());
        r.message();
    }
    // with a JSON body too
    let r = a.request("POST", "/api/online", Some(json!({}))).await;
    assert_eq!(r.status, 405, "{}", r.text());
    r.message();
}

// ---- histogram: freshness and delay_stats off ---------------------------------------------

fn hist_snapshot() -> Snapshot {
    let d = || DirSample {
        uptime_ns: 1,
        rx_pkts: 0,
        tx_sent: 0,
        loss: 0,
        limiter: 0,
        full: 0,
        tx_drops: 0,
    };
    let p = || PortSample {
        ibytes: 0,
        obytes: 0,
        imissed: 0,
        rx_nombuf: 0,
    };
    snapshot([d(), d()], [p(), p()])
}

fn set_polled(a: &TestApp, age_ms: u64, stale: bool, cur: Snapshot) {
    *a.telemetry.cell.lock().unwrap() = Some(Polled {
        prev: None,
        cur,
        age_ms,
        stale,
    });
}

#[tokio::test]
async fn the_histogram_carries_age_ms_and_stale() {
    let a = app();
    set_polled(&a, 1234, false, hist_snapshot());
    let h = a.get("/api/stats/0/delay_histogram").await;
    assert_eq!(h.status, 200, "{}", h.text());
    let h = h.json();
    assert_eq!(h["age_ms"], 1234, "{h}");
    assert_eq!(h["stale"], false, "{h}");

    set_polled(&a, 9000, true, hist_snapshot());
    let h = a.get("/api/stats/1/delay_histogram").await.json();
    assert_eq!(h["age_ms"], 9000, "{h}");
    assert_eq!(h["stale"], true, "{h}");
    // the bins are still there
    assert_eq!(h["bins"].as_array().unwrap().len(), 100);
}

#[tokio::test]
async fn a_direction_with_delay_stats_off_is_a_404_that_says_so() {
    let a = app();
    let mut s = hist_snapshot();
    s.ports["dir0_delay_stats"] = json!(1);
    s.ports["dir1_delay_stats"] = json!(0);
    s.hists[1] = empty_hist_json();
    set_polled(&a, 0, false, s);

    assert_eq!(a.get("/api/stats/0/delay_histogram").await.status, 200);
    let r = a.get("/api/stats/1/delay_histogram").await;
    assert_eq!(r.status, 404, "{}", r.text());
    let m = r.message().to_lowercase();
    assert!(m.contains("delay"), "{m}");
    assert!(
        m.contains("off") || m.contains("disabled") || m.contains("not enabled"),
        "the message says delay statistics are off for the direction: {m}"
    );
    // an unknown direction stays a 404 without that claim
    let r = a.get("/api/stats/7/delay_histogram").await;
    assert_eq!(r.status, 404);
    let m = r.message().to_lowercase();
    assert!(!m.contains("disabled") && !m.contains(" off"), "{m}");
}

// ---- unknown fields in the DTOs ----------------------------------------------------------

/// `(what, edit)`: one typo at each level of the config.
type Typo = (&'static str, Box<dyn Fn(&mut Value)>);

fn typos() -> Vec<Typo> {
    vec![
        ("top level", Box::new(|c| c["seeed"] = json!(1))),
        (
            "delayer",
            Box::new(|c| c["delayer"]["mean_nss"] = json!(1.0)),
        ),
        (
            "limiter",
            Box::new(|c| c["limiter"]["rate_bpss"] = json!(1.0)),
        ),
        (
            "histogram",
            Box::new(|c| {
                c["histogram"] = json!({"bins": 10, "lo_ns": 0.0, "hi_ns": 1000.0, "binz": 3})
            }),
        ),
        (
            "loss model",
            Box::new(|c| c["loss_model"] = json!({"type": "Random", "p": 0.1, "pp": 0.2})),
        ),
    ]
}

#[tokio::test]
async fn a_typo_in_a_config_is_a_400_naming_the_field() {
    for (what, edit) in typos() {
        let a = app();
        let mut body = link_body("x", &["net_null0"]);
        edit(&mut body["configs"][0]);
        let r = a.post("/api/links", body.clone()).await;
        assert_eq!(r.status, 400, "{what}: {}", r.text());
        r.message();
        assert_eq!(a.get("/api/links").await.json(), json!([]), "{what} stored");
        // on update as well
        let id = a.create_link("y", &["net_null1"]).await.json()["id"]
            .as_u64()
            .unwrap();
        let r = a.put(&format!("/api/links/{id}"), body).await;
        assert_eq!(r.status, 400, "{what} on update: {}", r.text());
    }
}

#[tokio::test]
async fn the_message_names_a_typo_in_the_seed() {
    let a = app();
    let mut body = link_body("x", &["net_null0"]);
    body["configs"][0]["seeed"] = json!(7);
    let r = a.post("/api/links", body).await;
    assert_eq!(r.status, 400);
    assert!(r.message().contains("seeed"), "{}", r.text());
}

#[test]
fn the_dtos_do_not_deserialize_with_unknown_fields() {
    for (what, edit) in typos() {
        let mut c = cfg_json(1.0e6, 0.0);
        edit(&mut c);
        assert!(
            serde_json::from_value::<ConfigDto>(c).is_err(),
            "{what}: unknown field accepted"
        );
    }
    // the known optional fields still work
    let mut c = cfg_json(1.0e6, 0.0);
    c["seed"] = json!(7);
    c["delay_stats"] = json!(false);
    c["histogram"] = json!({"bins": 10, "lo_ns": 0.0, "hi_ns": 1000.0});
    assert!(serde_json::from_value::<ConfigDto>(c).is_ok());
}

// ---- JSON content type with an empty or invalid body -------------------------------------

#[tokio::test]
async fn an_empty_or_invalid_json_body_is_a_400_with_a_message() {
    let a = app();
    let id = a.create_link("l", &["net_null0"]).await.json()["id"]
        .as_u64()
        .unwrap();
    let put_link = format!("/api/links/{id}");
    for (method, path) in [
        ("POST", "/api/links"),
        ("PUT", put_link.as_str()),
        ("PUT", "/api/settings"),
    ] {
        for body in [
            "",
            "{",
            "not json",
            "[]",
            "null",
            r#"{"name": 5}"#,
            r#"{"a":"#,
        ] {
            let r = a
                .request_raw(
                    method,
                    path,
                    Some("application/json"),
                    body.as_bytes().to_vec(),
                )
                .await;
            assert_eq!(r.status, 400, "{method} {path} body {body:?}: {}", r.text());
            r.message();
        }
    }
    // nothing was changed
    assert_eq!(
        a.get("/api/links").await.json().as_array().unwrap().len(),
        1
    );
}
