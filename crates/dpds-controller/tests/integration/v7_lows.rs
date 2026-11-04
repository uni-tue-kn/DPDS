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
//! Spec 22: the configs arrive in the body of `POST /api/emulation/start` (and validate, hints);
//! there is no store and no stored link any more. `--runtime-dir` too long is in
//! `startup_errors.rs`.

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
        let state = setup(vec![link("a", "net_null0", "net_null0", None)]);
        // in a debug build an unchecked multiplication panics here
        let hints = hints_json(&h, &state);
        let hp = hint(&hints, "hugepages");
        assert_eq!(hp["level"], "ok", "{total}/{free}/{size_kb}: {hp}");
    }
}

// ---- unknown /api routes and wrong methods are JSON (paths outside /api are the frontend, see frontend.rs) -----------------------------------------

#[tokio::test]
async fn unknown_routes_are_a_404_with_a_json_message() {
    let a = app();
    for (method, path) in [
        ("GET", "/api/nope"),
        ("GET", "/api/emulation/extra"),
        ("GET", "/api/host/hints/extra"),
        ("POST", "/api/nope"),
        ("GET", "/api"),
        ("GET", "/api/"),
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
        ("DELETE", "/api/host/hints"),
        ("POST", "/api/online"),
        ("PUT", "/api/stats"),
        ("GET", "/api/emulation/start"),
        ("GET", "/api/emulation/stop"),
        ("GET", "/api/emulation/validate"),
        ("PUT", "/api/emulation/start"),
        ("POST", "/api/emulation"),
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
            Box::new(|c| {
                c["limiter"] = json!({"mode": "Spacer", "rate_bps": 1.0e9, "rate_bpss": 1.0})
            }),
        ),
        (
            "limiter activate (gone)",
            Box::new(|c| {
                c["limiter"] = json!({"mode": "Spacer", "activate": true, "rate_bps": 1.0e9})
            }),
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
        let mut body = link("x", "net_null0", "net_null0", None);
        edit(&mut body["config"]);
        let s = setup(vec![body]);
        let r = a.start(&s).await;
        assert_eq!(r.status, 400, "{what}: {}", r.text());
        r.message();
        assert!(a.supervisor.plans().is_empty(), "{what} was started");
        let r = a.validate(&s).await;
        assert_eq!(r.status, 400, "{what} on validate: {}", r.text());
        let r = a.post("/api/host/hints", s.body()).await;
        assert_eq!(r.status, 400, "{what} on hints: {}", r.text());
    }
}

#[tokio::test]
async fn the_message_names_a_typo_in_the_seed() {
    let a = app();
    let mut body = link("x", "net_null0", "net_null0", None);
    body["config"]["seeed"] = json!(7);
    let r = a.start_links(vec![body]).await;
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
    for path in [
        "/api/emulation/start",
        "/api/emulation/validate",
        "/api/host/hints",
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
                    "POST",
                    path,
                    Some("application/json"),
                    body.as_bytes().to_vec(),
                )
                .await;
            assert_eq!(r.status, 400, "POST {path} body {body:?}: {}", r.text());
            r.message();
        }
    }
    assert!(a.supervisor.plans().is_empty());
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

// ---- spec20: `limit_bit` is gone, the limit is derived from the buffer capacity ------------

#[tokio::test]
async fn a_link_with_limit_bit_is_a_400_naming_the_field() {
    let a = app();
    let mut body = link("x", "net_null0", "net_null0", None);
    body["config"]["limiter"] = json!({"mode": "Spacer", "rate_bps": 1.0e9, "limit_bit": 1.0e6});
    let r = a.start_links(vec![body]).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("limit_bit"), "{}", r.text());
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn a_running_links_limiter_has_no_limit_bit_and_no_activate() {
    let a = app();
    let mut l = link("plain", "net_null0", "net_null0", None);
    l["config"]["limiter"] = json!({"mode": "Spacer", "rate_bps": 1.0e9});
    a.start_ok(vec![l]).await;
    let v = a.get("/api/emulation").await.json();
    let text = v.to_string();
    assert!(!text.contains("limit_bit"), "{text}");
    let lim = v["config"]["links"][0]["config"]["limiter"]
        .as_object()
        .unwrap();
    let mut keys: Vec<&str> = lim.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["mode", "rate_bps"], "{v}");
}

#[tokio::test]
async fn the_openapi_document_has_no_limit_bit() {
    let a = app();
    let v = a.get("/api/openapi.json").await.json();
    let text = v.to_string();
    assert!(
        !text.contains("limit_bit"),
        "OpenAPI still documents limit_bit"
    );
    let props = v["components"]["schemas"]["LimiterDto"]["properties"]
        .as_object()
        .expect("LimiterDto properties");
    let mut keys: Vec<&str> = props.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["mode", "rate_bps"]);
}

#[test]
fn the_config_dto_rejects_limit_bit_and_does_not_serialize_it() {
    let mut c = cfg_json(1.0e6, 0.0);
    c["limiter"] = json!({"mode": "Spacer", "rate_bps": 1.0e9, "limit_bit": 1.0e6});
    let err = serde_json::from_value::<dpds_controller::dto::ConfigDto>(c)
        .expect_err("limit_bit must be an unknown field")
        .to_string();
    assert!(err.contains("limit_bit"), "{err}");
    let ok = serde_json::to_value(cfg_dto(1.0e6, 0.0)).unwrap();
    assert!(!ok.to_string().contains("limit_bit"), "{ok}");
}
