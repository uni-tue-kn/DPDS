//! The optional `iat_histogram` of a link config, the same type as `histogram`
//! (`HistogramDto {bins, lo_ns, hi_ns}`).
//!
//! # Contract
//!
//! - `ConfigDto.iat_histogram: Option<HistogramDto>`, serialized only when set (like
//!   `histogram`), so the running configuration echoes it.
//! - `ConfigDto::to_core` (hence start and validate) validates it like `histogram` (bins
//!   1..=4096, finite, not negative, lo < hi); every message names "IAT histogram". The messages
//!   of the delay histogram do not.
//! - The links file entry has `iat_histogram` beside `histogram` (link level), only when the
//!   config has one.
//! - OpenAPI: `ConfigDto` has an `iat_histogram` property.

use crate::common::*;
use dpds_controller::eal::links_file_json;
use dpds_controller::lcores::DirectionPlan;
use serde_json::{json, Value};

fn with_iat(h: Value) -> Value {
    let mut c = cfg_bare();
    c["iat_histogram"] = h;
    c
}

fn bad_iat() -> Vec<Value> {
    vec![
        json!({"bins": 0}),
        json!({"bins": 4097}),
        json!({"lo_ns": -1.0}),
        json!({"hi_ns": -5.0}),
        json!({"lo_ns": 5.0, "hi_ns": 5.0}),
        json!({"lo_ns": 9.0, "hi_ns": 5.0}),
    ]
}

// ---- the DTO --------------------------------------------------------------------------------

#[test]
fn the_dto_accepts_and_serializes_an_iat_histogram() {
    let h = json!({"bins": 64, "lo_ns": 0.0, "hi_ns": 1000.0});
    let dto = dto_from(with_iat(h.clone()));
    dto.to_core().unwrap();
    let out = serde_json::to_value(&dto).unwrap();
    assert_eq!(out["iat_histogram"], h, "{out}");
    // omitted fields stay omitted, an omitted histogram is not serialized
    let out = serde_json::to_value(dto_from(with_iat(json!({"bins": 8})))).unwrap();
    assert_eq!(out["iat_histogram"], json!({"bins": 8}), "{out}");
    let out = serde_json::to_value(dto_from(cfg_bare())).unwrap();
    assert!(out.get("iat_histogram").is_none_or(Value::is_null), "{out}");
}

#[test]
fn an_empty_iat_histogram_is_valid_and_the_limits_are_inclusive() {
    for h in [
        json!({}),
        json!({"bins": 1}),
        json!({"bins": 4096}),
        json!({"lo_ns": 0.0}),
        json!({"lo_ns": 0.0, "hi_ns": 0.001}),
    ] {
        dto_from(with_iat(h.clone()))
            .to_core()
            .unwrap_or_else(|e| panic!("{h}: {e}"));
    }
}

#[test]
fn the_iat_histogram_is_validated_like_the_delay_one_with_messages_naming_it() {
    for h in bad_iat() {
        let e = dto_from(with_iat(h.clone()))
            .to_core()
            .expect_err(&h.to_string())
            .to_string();
        assert!(e.contains("IAT histogram"), "{h}: {e}");
        // the same value in the delay histogram is an error too, not naming the IAT one
        let mut c = cfg_bare();
        c["histogram"] = h.clone();
        let e = dto_from(c).to_core().expect_err(&h.to_string()).to_string();
        assert!(!e.contains("IAT"), "{h}: {e}");
    }
    let e = dto_from(with_iat(json!({"bins": 4097})))
        .to_core()
        .unwrap_err()
        .to_string();
    assert!(e.contains("4096"), "names the maximum: {e}");
}

#[test]
fn an_unknown_key_in_the_iat_histogram_does_not_deserialize() {
    let mut c = cfg_bare();
    c["iat_histogram"] = json!({"bin_ns": 10});
    assert!(serde_json::from_value::<dpds_controller::dto::ConfigDto>(c).is_err());
}

// ---- start / validate -----------------------------------------------------------------------

#[tokio::test]
async fn start_and_validate_refuse_a_bad_iat_histogram_naming_it() {
    for h in bad_iat() {
        let a = TestApp::new(FakeHost::bare_metal());
        let l = link_cfg("l", "net_null0", "net_null0", with_iat(h.clone()));
        let r = a.start_links(vec![l.clone()]).await;
        assert_eq!(r.status, 400, "{h}: {}", r.text());
        assert!(
            r.message().contains("IAT histogram"),
            "{h}: {}",
            r.message()
        );
        let r = a.validate(&setup(vec![l])).await;
        assert_eq!(r.status, 400, "{h}: {}", r.text());
        assert!(
            r.message().contains("IAT histogram"),
            "{h}: {}",
            r.message()
        );
    }
}

#[tokio::test]
async fn start_accepts_an_iat_histogram_and_the_running_config_echoes_it() {
    let a = TestApp::new(FakeHost::bare_metal());
    let h = json!({"bins": 32, "lo_ns": 0.0, "hi_ns": 500.0});
    let l = link_cfg("l", "net_null0", "net_null0", with_iat(h.clone()));
    assert_eq!(a.validate(&setup(vec![l.clone()])).await.status, 200);
    let r = a.start_links(vec![l]).await;
    assert_eq!(r.status, 202, "{}", r.text());
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["config"]["links"][0]["config"]["iat_histogram"], h, "{v}");
}

// ---- the links file -------------------------------------------------------------------------

fn plan(dir: u32, link: &str, rx: &str, tx: &str, lcore: u32) -> DirectionPlan {
    DirectionPlan {
        dir,
        link: link.into(),
        rx: rx.into(),
        tx: tx.into(),
        lcore,
    }
}

#[test]
fn the_links_file_carries_the_iat_histogram_beside_the_delay_one() {
    let mut c = cfg_bare();
    c["histogram"] = json!({"bins": 16});
    c["iat_histogram"] = json!({"bins": 32, "hi_ns": 500.0});
    let l = model_link(1, &link_cfg("a-to-b", "net_null0", "net_null1", c));
    let v = links_file_json(&[l], &[plan(0, "a-to-b", "net_null0", "net_null1", 2)]).unwrap();
    let e = &v["links"][0];
    assert_eq!(
        e["iat_histogram"],
        json!({"bins": 32, "hi_ns": 500.0}),
        "{v}"
    );
    assert_eq!(e["histogram"], json!({"bins": 16}), "{v}");
    assert!(
        e["config"].get("iat_histogram").is_none(),
        "link level, not in config: {v}"
    );
}

#[test]
fn the_links_file_has_no_iat_histogram_when_the_config_has_none() {
    for c in [cfg_bare(), {
        let mut c = cfg_bare();
        c["histogram"] = json!({"bins": 16});
        c
    }] {
        let l = model_link(1, &link_cfg("a-to-b", "net_null0", "net_null1", c));
        let v = links_file_json(&[l], &[plan(0, "a-to-b", "net_null0", "net_null1", 2)]).unwrap();
        assert!(v["links"][0].get("iat_histogram").is_none(), "{v}");
    }
}

#[tokio::test]
async fn the_started_links_file_has_the_iat_histogram() {
    let a = TestApp::new(FakeHost::bare_metal());
    let l = link_cfg(
        "l",
        "net_null0",
        "net_null0",
        with_iat(json!({"bins": 32, "lo_ns": 0.0, "hi_ns": 1000.0})),
    );
    assert_eq!(a.start_links(vec![l]).await.status, 202);
    let file = &a.supervisor.plans()[0].links_json["links"][0];
    assert_eq!(
        file["iat_histogram"],
        json!({"bins": 32, "lo_ns": 0.0, "hi_ns": 1000.0}),
        "{file}"
    );
}

// ---- OpenAPI --------------------------------------------------------------------------------

#[tokio::test]
async fn openapi_lists_the_iat_histogram_of_the_config() {
    let a = TestApp::new(FakeHost::bare_metal());
    let v = a.get("/api/openapi.json").await.json();
    let props = &v["components"]["schemas"]["ConfigDto"]["properties"];
    assert!(props.get("iat_histogram").is_some(), "{props}");
    assert!(props.get("histogram").is_some(), "{props}");
}
