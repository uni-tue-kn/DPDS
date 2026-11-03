//! K1 (spec v8): config presets, `GET|POST /api/presets`, `GET|PUT|DELETE /api/presets/{id}`.
//!
//! Contract the tests assume (spec text first, then ASSUMPTIONs of the tester):
//! - body `{name, description?, config: ConfigDto}`; the reply (POST 201, GET, PUT 200)
//!   is `{id, name, description, config}`; `GET /api/presets` is an array in id order;
//!   DELETE is 204; unknown or non-numeric id is 404 (like links).
//! - name: non-empty, at most 64 CHARACTERS (not bytes), unique (409);
//! - the config is validated like a link direction: 400 whose message names the field;
//! - persisted in `state.json` under `presets`; an old state file without the key
//!   loads as "no presets"; ids are never reused, also across a controller restart.
//! - ASSUMPTION: a missing description reads back as absent, null or "".
//! - ASSUMPTION: preset ids come from their own counter, so the tests only compare
//!   ids with each other (never with link ids, never against a literal).
//! - ASSUMPTION: editing presets while the emulation runs is allowed and does NOT
//!   set `pending_restart` (presets are not part of what a running `dpds` was started with).

use crate::common::*;
use dpds_controller::supervisor::{State, Supervisor};
use serde_json::{json, Value};

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

fn body(name: &str) -> Value {
    json!({ "name": name, "description": format!("about {name}"), "config": cfg_json(1.0e6, 1.0e5) })
}

/// The config as the API stores and returns it (defaults filled in).
fn stored_cfg(mean_ns: f64, std_ns: f64) -> Value {
    serde_json::to_value(cfg_dto(mean_ns, std_ns)).unwrap()
}

async fn create(a: &TestApp, name: &str) -> Value {
    let r = a.post("/api/presets", body(name)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    r.json()
}

fn id_of(v: &Value) -> u64 {
    v["id"]
        .as_u64()
        .unwrap_or_else(|| panic!("no numeric id in {v}"))
}

async fn list(a: &TestApp) -> Vec<Value> {
    let r = a.get("/api/presets").await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json().as_array().expect("an array").clone()
}

fn names(v: &[Value]) -> Vec<&str> {
    v.iter().map(|p| p["name"].as_str().unwrap()).collect()
}

// ---- CRUD ------------------------------------------------------------------------------

#[tokio::test]
async fn there_are_no_presets_at_first() {
    let a = app();
    assert!(list(&a).await.is_empty());
}

#[tokio::test]
async fn create_returns_201_with_id_name_description_and_config() {
    let a = app();
    let p = create(&a, "satellite").await;
    assert_eq!(p["name"], "satellite");
    assert_eq!(p["description"], "about satellite");
    assert_eq!(p["config"], stored_cfg(1.0e6, 1.0e5));
    let _ = id_of(&p);
    let r = a.get(&format!("/api/presets/{}", id_of(&p))).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), p);
}

#[tokio::test]
async fn the_list_is_in_creation_order_with_distinct_ids() {
    let a = app();
    let p1 = create(&a, "one").await;
    let p2 = create(&a, "two").await;
    let p3 = create(&a, "three").await;
    let l = list(&a).await;
    assert_eq!(names(&l), ["one", "two", "three"]);
    assert!(id_of(&p1) < id_of(&p2) && id_of(&p2) < id_of(&p3));
    assert_eq!(l[1], p2);
}

#[tokio::test]
async fn the_description_is_optional() {
    let a = app();
    let r = a
        .post(
            "/api/presets",
            json!({"name": "bare", "config": cfg_json(1.0e6, 0.0)}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let d = &r.json()["description"];
    assert!(
        d.is_null() || d == "",
        "a missing description is absent, null or empty, got {d}"
    );
}

#[tokio::test]
async fn put_replaces_name_description_and_config_and_keeps_the_id() {
    let a = app();
    let p = create(&a, "old").await;
    let id = id_of(&p);
    let other = create(&a, "other").await;
    let r = a
        .put(
            &format!("/api/presets/{id}"),
            json!({"name": "new", "description": "changed", "config": cfg_json(7.0e6, 2.0e5)}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(id_of(&v), id);
    assert_eq!(v["name"], "new");
    assert_eq!(v["description"], "changed");
    assert_eq!(v["config"], stored_cfg(7.0e6, 2.0e5));
    assert_eq!(a.get(&format!("/api/presets/{id}")).await.json(), v);
    // the other preset is untouched
    assert_eq!(
        a.get(&format!("/api/presets/{}", id_of(&other)))
            .await
            .json(),
        other
    );
}

#[tokio::test]
async fn put_may_keep_its_own_name() {
    let a = app();
    let p = create(&a, "same").await;
    let r = a
        .put(
            &format!("/api/presets/{}", id_of(&p)),
            json!({"name": "same", "description": "edited", "config": cfg_json(2.0e6, 0.0)}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["description"], "edited");
}

#[tokio::test]
async fn delete_is_204_then_404() {
    let a = app();
    let p = create(&a, "gone").await;
    let keep = create(&a, "keep").await;
    let path = format!("/api/presets/{}", id_of(&p));
    assert_eq!(a.delete(&path).await.status, 204);
    let r = a.get(&path).await;
    assert_eq!(r.status, 404);
    r.message();
    let r = a.delete(&path).await;
    assert_eq!(r.status, 404);
    r.message();
    assert_eq!(list(&a).await, vec![keep]);
}

#[tokio::test]
async fn unknown_and_malformed_ids_are_404_with_a_message() {
    let a = app();
    for id in ["999", "0", "abc", "-1", "1.5"] {
        let path = format!("/api/presets/{id}");
        for r in [
            a.get(&path).await,
            a.put(&path, body("x")).await,
            a.delete(&path).await,
        ] {
            assert_eq!(r.status, 404, "{path}: {}", r.text());
            r.message();
        }
    }
}

// ---- names -----------------------------------------------------------------------------

#[tokio::test]
async fn an_empty_name_is_a_400_naming_the_field() {
    let a = app();
    let r = a.post("/api/presets", body("")).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("name"), "{}", r.text());
    assert!(list(&a).await.is_empty());
}

#[tokio::test]
async fn names_up_to_64_characters_are_fine_longer_ones_are_a_400() {
    let a = app();
    assert_eq!(
        a.post("/api/presets", body(&"a".repeat(64))).await.status,
        201
    );
    let r = a.post("/api/presets", body(&"b".repeat(65))).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("name"));
    assert_eq!(list(&a).await.len(), 1);
}

#[tokio::test]
async fn the_limit_counts_characters_not_bytes() {
    let a = app();
    // 64 two-byte characters = 128 bytes
    let ok = a.post("/api/presets", body(&"é".repeat(64))).await;
    assert_eq!(ok.status, 201, "{}", ok.text());
    let bad = a.post("/api/presets", body(&"é".repeat(65))).await;
    assert_eq!(bad.status, 400, "{}", bad.text());
}

#[tokio::test]
async fn names_may_contain_spaces_and_punctuation() {
    let a = app();
    let p = create(&a, "LEO sat. (Starlink) #1 / 40 ms").await;
    assert_eq!(p["name"], "LEO sat. (Starlink) #1 / 40 ms");
}

#[tokio::test]
async fn a_duplicate_name_is_a_409_on_create_and_changes_nothing() {
    let a = app();
    let p = create(&a, "dup").await;
    let r = a.post("/api/presets", body("dup")).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().contains("dup"));
    assert_eq!(list(&a).await, vec![p]);
}

#[tokio::test]
async fn renaming_onto_another_presets_name_is_a_409_and_changes_nothing() {
    let a = app();
    let p1 = create(&a, "first").await;
    let p2 = create(&a, "second").await;
    let r = a
        .put(&format!("/api/presets/{}", id_of(&p2)), body("first"))
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(list(&a).await, vec![p1, p2]);
}

#[tokio::test]
async fn a_deleted_presets_name_is_free_again() {
    let a = app();
    let p = create(&a, "reuse").await;
    assert_eq!(
        a.delete(&format!("/api/presets/{}", id_of(&p)))
            .await
            .status,
        204
    );
    let q = create(&a, "reuse").await;
    assert_ne!(id_of(&q), id_of(&p), "ids are never reused");
}

#[tokio::test]
async fn concurrent_creates_of_one_name_yield_exactly_one_preset() {
    let a = app();
    let (r1, r2, r3, r4) = tokio::join!(
        a.post("/api/presets", body("race")),
        a.post("/api/presets", body("race")),
        a.post("/api/presets", body("race")),
        a.post("/api/presets", body("race")),
    );
    let statuses = [r1.status, r2.status, r3.status, r4.status];
    assert_eq!(
        statuses.iter().filter(|s| **s == 201).count(),
        1,
        "{statuses:?}"
    );
    assert_eq!(
        statuses.iter().filter(|s| **s == 409).count(),
        3,
        "{statuses:?}"
    );
    assert_eq!(list(&a).await.len(), 1);
}

// ---- config validation -----------------------------------------------------------------

/// `(what to change in cfg_json, text the 400 message must contain)`.
fn bad_configs() -> Vec<(Value, &'static str)> {
    let mut v = Vec::new();
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["delayer"]["mean_ns"] = json!(-1.0);
    v.push((c, "mean_ns"));
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["delayer"]["std_ns"] = json!(-5.0);
    v.push((c, "std_ns"));
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["loss_model"] = json!({"type": "Random", "p": 1.5});
    v.push((c, "loss_model"));
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["limiter"] = json!({"mode": "Spacer", "activate": true, "rate_bps": 0.0, "limit_bit": 1.0e6});
    v.push((c, "rate_bps"));
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["histogram"] = json!({"bins": 0});
    v.push((c, "bins"));
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["buffer_capacity"] = json!(1u64 << 32);
    v.push((c, "buffer_capacity"));
    v
}

#[tokio::test]
async fn an_invalid_config_is_a_400_naming_the_field_on_create() {
    let a = app();
    for (cfg, field) in bad_configs() {
        let r = a
            .post("/api/presets", json!({"name": "bad", "config": cfg}))
            .await;
        assert_eq!(r.status, 400, "{field}: {}", r.text());
        assert!(r.message().contains(field), "{field}: {}", r.text());
    }
    assert!(list(&a).await.is_empty());
}

#[tokio::test]
async fn an_invalid_config_is_a_400_on_update_and_leaves_the_preset_alone() {
    let a = app();
    let p = create(&a, "good").await;
    for (cfg, field) in bad_configs() {
        let r = a
            .put(
                &format!("/api/presets/{}", id_of(&p)),
                json!({"name": "good", "config": cfg}),
            )
            .await;
        assert_eq!(r.status, 400, "{field}: {}", r.text());
        assert!(r.message().contains(field), "{field}: {}", r.text());
    }
    assert_eq!(list(&a).await, vec![p]);
}

#[tokio::test]
async fn unknown_keys_in_the_config_missing_config_and_malformed_bodies_are_a_400() {
    let a = app();
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["surprise"] = json!(1);
    for b in [
        json!({"name": "x", "config": c}),
        json!({"name": "x"}),
        json!({"config": cfg_json(1.0e6, 1.0e5)}),
        json!({"name": 5, "config": cfg_json(1.0e6, 1.0e5)}),
        json!({"name": "x", "config": "nope"}),
    ] {
        let r = a.post("/api/presets", b.clone()).await;
        assert!(
            r.status == 400 || r.status == 422,
            "{b}: status {}",
            r.status
        );
        r.message();
    }
    assert!(list(&a).await.is_empty());
}

#[tokio::test]
async fn a_valid_but_unusual_config_is_stored_verbatim() {
    let a = app();
    let mut c = cfg_json(2.0e6, 5.0e5);
    c["delayer"]["distribution"] = json!("GammaDistribution");
    c["limiter"] = json!({"mode": "TokenBucketFilter", "activate": true, "rate_bps": 5.0e9, "limit_bit": 2.0e6});
    c["loss_model"] = json!({"type": "GilbertElliot", "p": 0.1, "r": 0.2, "k": 0.3, "h": 0.4});
    c["seed"] = json!(42);
    c["histogram"] = json!({"bins": 64, "lo_ns": 0.0, "hi_ns": 1.0e7});
    c["delay_stats"] = json!(false);
    let r = a
        .post("/api/presets", json!({"name": "wild", "config": c.clone()}))
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let got = r.json()["config"].clone();
    let want = serde_json::to_value(dto_from(c)).unwrap();
    assert_eq!(strip_nulls(&got), strip_nulls(&want));
    assert_eq!(got["seed"], 42);
    assert_eq!(got["histogram"]["bins"], 64);
    assert_eq!(got["delay_stats"], false);
}

// ---- hardening, concurrency with the emulation ----------------------------------------------

#[tokio::test]
async fn presets_need_a_json_content_type_and_obey_the_body_limit() {
    let a = app();
    let r = a
        .request_raw(
            "POST",
            "/api/presets",
            Some("text/plain"),
            body("x").to_string().into_bytes(),
        )
        .await;
    assert_eq!(r.status, 415, "{}", r.text());
    r.message();
    let big = format!(r#"{{"name":"{}"}}"#, "a".repeat(2 * 1024 * 1024));
    for (m, p) in [("POST", "/api/presets"), ("PUT", "/api/presets/1")] {
        let r = a
            .request_raw(m, p, Some("application/json"), big.clone().into_bytes())
            .await;
        assert_eq!(r.status, 413, "{m} {p}");
        r.message();
    }
    assert!(list(&a).await.is_empty());
}

#[tokio::test]
async fn presets_can_be_edited_while_running_without_a_pending_restart() {
    let a = app();
    assert_eq!(a.create_link("ab", NIC0, NIC1).await.status, 201);
    assert_eq!(a.post("/api/emulation/start", json!({})).await.status, 202);
    let p = create(&a, "live").await;
    let id = id_of(&p);
    assert_eq!(
        a.put(&format!("/api/presets/{id}"), body("live2"))
            .await
            .status,
        200
    );
    assert_eq!(a.delete(&format!("/api/presets/{id}")).await.status, 204);
    let e = a.get("/api/emulation").await.json();
    assert_eq!(e["state"], "running", "{e}");
    assert_eq!(
        e["pending_restart"], false,
        "presets are not part of a run: {e}"
    );
}

// ---- ids and persistence ------------------------------------------------------------------

#[tokio::test]
async fn ids_are_never_reused_also_after_deleting_the_newest() {
    let a = app();
    let _p1 = create(&a, "a").await;
    let p2 = create(&a, "b").await;
    assert_eq!(
        a.delete(&format!("/api/presets/{}", id_of(&p2)))
            .await
            .status,
        204
    );
    let p3 = create(&a, "c").await;
    assert!(id_of(&p3) > id_of(&p2), "{p3} vs {p2}");
}

#[tokio::test]
async fn presets_and_ids_survive_a_router_rebuild() {
    let mut a = app();
    let p1 = create(&a, "keep-1").await;
    let p2 = create(&a, "keep-2").await;
    let p3 = create(&a, "deleted").await;
    assert_eq!(
        a.delete(&format!("/api/presets/{}", id_of(&p3)))
            .await
            .status,
        204
    );
    // an edit is persisted too
    let r = a
        .put(
            &format!("/api/presets/{}", id_of(&p2)),
            json!({"name": "keep-2", "description": "edited", "config": cfg_json(3.0e6, 0.0)}),
        )
        .await;
    assert_eq!(r.status, 200);
    let p2 = r.json();
    a.restart();
    assert_eq!(list(&a).await, vec![p1, p2.clone()]);
    assert_eq!(
        a.get(&format!("/api/presets/{}", id_of(&p2))).await.json(),
        p2
    );
    // the id counter was persisted: the deleted newest id is not handed out again
    let p4 = create(&a, "after-restart").await;
    assert!(id_of(&p4) > id_of(&p3), "{p4} vs {p3}");
    // and the name of the deleted preset is free
    create(&a, "deleted").await;
}

#[tokio::test]
async fn state_json_carries_the_presets_array() {
    let a = app();
    create(&a, "stored").await;
    let bytes = std::fs::read(a.state_dir.path().join("state.json")).unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let presets = v["presets"].as_array().unwrap_or_else(|| panic!("{v}"));
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0]["name"], "stored");
    assert!(presets[0].get("config").is_some());
    // links and settings are still there
    assert!(v.get("links").is_some() && v.get("settings").is_some());
}

#[tokio::test]
async fn an_old_state_file_without_presets_loads_as_no_presets() {
    let mut a = app();
    // what the controller wrote before v8, with the link in the directed (v2) shape
    let old = json!({
        "links": [{
            "id": 3, "name": "ab", "ingress": NIC0, "egress": NIC1,
            "config": cfg_json(1.0e6, 1.0e5)
        }],
        "settings": {"main_lcore": null,
                     "eal_extra_args": [], "poll_interval_ms": 500},
        "next_id": 4
    });
    std::fs::write(
        a.state_dir.path().join("state.json"),
        serde_json::to_vec_pretty(&old).unwrap(),
    )
    .unwrap();
    a.restart();
    assert!(list(&a).await.is_empty());
    let links = a.get("/api/links").await.json();
    assert_eq!(links.as_array().unwrap().len(), 1);
    assert_eq!(links[0]["id"], 3);
    // presets work from here on and the link id counter is intact
    let p = create(&a, "first").await;
    assert_eq!(list(&a).await, vec![p]);
    let l = a.create_link("cd", "net_null0", "net_null1").await.json();
    assert_eq!(l["id"], 4);
}

#[tokio::test]
async fn presets_do_not_touch_links_or_settings() {
    let a = app();
    assert_eq!(a.create_link("ab", NIC0, NIC1).await.status, 201);
    let links = a.get("/api/links").await.json();
    let settings = a.get("/api/settings").await.json();
    let p = create(&a, "x").await;
    assert_eq!(
        a.delete(&format!("/api/presets/{}", id_of(&p)))
            .await
            .status,
        204
    );
    assert_eq!(a.get("/api/links").await.json(), links);
    assert_eq!(a.get("/api/settings").await.json(), settings);
    // a stopped emulation, so the supervisor was never involved
    assert_eq!(a.supervisor.status().state, State::Stopped);
}

// ---- OpenAPI ------------------------------------------------------------------------------

#[tokio::test]
async fn openapi_documents_the_preset_routes_and_their_errors() {
    let a = app();
    let v = a.get("/api/openapi.json").await.json();
    let expect: &[(&str, &str, &[&str])] = &[
        ("/api/presets", "get", &["200"]),
        (
            "/api/presets",
            "post",
            &["201", "400", "409", "413", "415", "500"],
        ),
        ("/api/presets/{id}", "get", &["200", "404"]),
        (
            "/api/presets/{id}",
            "put",
            &["200", "400", "404", "409", "413", "415", "500"],
        ),
        ("/api/presets/{id}", "delete", &["204", "404", "500"]),
    ];
    for (path, method, codes) in expect {
        let op = &v["paths"][path][method];
        assert!(
            op.is_object(),
            "{method} {path} missing from the OpenAPI document"
        );
        for c in *codes {
            assert!(
                op["responses"].get(c).is_some(),
                "{method} {path}: response {c} is not documented: {}",
                op["responses"]
            );
        }
    }
    let schemas = v["components"]["schemas"].as_object().unwrap();
    assert!(
        schemas.keys().any(|k| k.to_lowercase().contains("preset")),
        "a preset schema is documented: {:?}",
        schemas.keys().collect::<Vec<_>>()
    );
}
