//! K2 (spec v8): `GET /api/export` and `POST /api/import[?mode=replace]`.
//!
//! Contract (spec text): the export is `{"format": "dpds-controller-export", "version": 1,
//! "settings", "links": [{name, ports, configs, workers}], "presets": [{name, description,
//! config}]}`, without ids and runtime state. Import refuses with 409 unless the emulation
//! is `stopped`, validates the whole document first (format/version, every link, preset and
//! setting by the rules of the individual endpoints, unique names, ports not shared between
//! links), applies it atomically (all or nothing), assigns new ids and replies with the
//! stored summary `{links: n, presets: m}`. Unknown keys are a 400, `mode=replace` is the
//! only mode.
//!
//! ASSUMPTIONs of the tester (where the spec is silent):
//! - import replies 200;
//! - "new ids" means ids from the same never-reused counters: after an import every link
//!   and preset id is larger than any id handed out before, and a later create goes on above;
//! - the 409 covers every state but `stopped` (starting, running, stopping AND failed:
//!   the spec says "not stopped");
//! - duplicate names inside the document are a 400 (a 409 is also accepted, the spec only
//!   says "validates ... unique names"); shared ports are a 400 (stated by the coordinator);
//! - `workers` is absent or null in the export when a link has none, and optional on import;
//! - a request that is invalid AND arrives while running is not tested (which error wins is open).

use crate::common::*;
use dpds_controller::supervisor::State;
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

const FORMAT: &str = "dpds-controller-export";

fn settings_doc() -> Value {
    json!({"main_lcore": 1, "mlx5_devargs": "txq_mpw_en=1",
           "eal_extra_args": ["--no-huge", "-m", "256"], "poll_interval_ms": 250})
}

/// A valid import document: two links (one with workers), two presets.
fn doc() -> Value {
    json!({
        "format": FORMAT,
        "version": 1,
        "settings": settings_doc(),
        "links": [
            {"name": "ab", "ports": [NIC0, NIC1],
             "configs": [cfg_json(1.0e6, 1.0e5), cfg_json(2.0e6, 0.0)], "workers": [2, 6]},
            {"name": "loop", "ports": ["net_null0"], "configs": [cfg_json(5.0e5, 1.0e4)]}
        ],
        "presets": [
            {"name": "sat", "description": "geo", "config": cfg_json(2.5e8, 1.0e6)},
            {"name": "lan", "description": "", "config": cfg_json(1.0e5, 0.0)}
        ]
    })
}

async fn import(a: &TestApp, d: Value) -> Reply {
    a.post("/api/import", d).await
}

async fn export(a: &TestApp) -> Value {
    let r = a.get("/api/export").await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

fn ids(v: &Value) -> Vec<u64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_u64().unwrap())
        .collect()
}

/// Everything an import may change, as the API shows it, plus the bytes of `state.json`.
async fn state_of(a: &TestApp) -> (Value, Value, Value, Option<Vec<u8>>) {
    (
        a.get("/api/links").await.json(),
        a.get("/api/presets").await.json(),
        a.get("/api/settings").await.json(),
        std::fs::read(a.state_dir.path().join("state.json")).ok(),
    )
}

/// Some existing setup, so that "nothing changed" is observable.
async fn populate(a: &TestApp) {
    assert_eq!(a.create_link("old-a", &[NIC0, NIC1]).await.status, 201);
    assert_eq!(a.create_link("old-b", &["net_null5"]).await.status, 201);
    for n in ["old-p1", "old-p2"] {
        let r = a
            .post(
                "/api/presets",
                json!({"name": n, "config": cfg_json(9.0e6, 0.0)}),
            )
            .await;
        assert_eq!(r.status, 201, "{}", r.text());
    }
    let r = a
        .put(
            "/api/settings",
            json!({"main_lcore": null, "mlx5_devargs": "txq_mpw_en=1",
                   "eal_extra_args": [], "poll_interval_ms": 777}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
}

// ---- export ----------------------------------------------------------------------------

#[tokio::test]
async fn the_export_of_an_empty_controller_is_the_documented_skeleton() {
    let a = app();
    let r = a.get("/api/export").await;
    assert_eq!(r.status, 200);
    let ct = r.headers.get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("application/json"), "{ct}");
    let v = r.json();
    let keys: BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        BTreeSet::from(["format", "version", "settings", "links", "presets"]),
        "{v}"
    );
    assert_eq!(v["format"], FORMAT);
    assert_eq!(v["version"], 1);
    assert_eq!(v["links"], json!([]));
    assert_eq!(v["presets"], json!([]));
    assert_eq!(v["settings"], a.get("/api/settings").await.json());
}

#[tokio::test]
async fn the_export_has_no_ids_and_no_runtime_state() {
    let a = app();
    populate(&a).await;
    // a running emulation must not leak into the export
    assert_eq!(a.post("/api/emulation/start", json!({})).await.status, 202);
    let v = export(&a).await;
    let text = v.to_string();
    for forbidden in [
        "next_id",
        "\"pid\"",
        "pending_restart",
        "log_tail",
        "started_at",
    ] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
    }
    for l in v["links"].as_array().unwrap() {
        assert!(l.get("id").is_none(), "link with an id: {l}");
        let keys: BTreeSet<&str> = l.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(
            keys.is_subset(&BTreeSet::from(["name", "ports", "configs", "workers"]))
                && ["name", "ports", "configs"]
                    .iter()
                    .all(|k| keys.contains(k)),
            "{l}"
        );
    }
    for p in v["presets"].as_array().unwrap() {
        assert!(p.get("id").is_none(), "preset with an id: {p}");
        let keys: BTreeSet<&str> = p.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(
            keys.is_subset(&BTreeSet::from(["name", "description", "config"]))
                && keys.contains("name")
                && keys.contains("config"),
            "{p}"
        );
    }
}

#[tokio::test]
async fn the_export_mirrors_links_presets_and_settings_in_order() {
    let a = app();
    populate(&a).await;
    let v = export(&a).await;
    let links = a.get("/api/links").await.json();
    let presets = a.get("/api/presets").await.json();
    let strip = |x: &Value, drop: &str| {
        let mut x = strip_nulls(x);
        x.as_object_mut().unwrap().remove(drop);
        x
    };
    let want_links: Vec<Value> = links
        .as_array()
        .unwrap()
        .iter()
        .map(|l| strip(l, "id"))
        .collect();
    let got_links: Vec<Value> = v["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(strip_nulls)
        .collect();
    assert_eq!(got_links, want_links);
    assert_eq!(got_links.len(), 2);
    assert_eq!(got_links[0]["name"], "old-a");
    let want_presets: Vec<Value> = presets
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let mut p = strip(p, "id");
            if p["description"].is_null() {
                p["description"] = json!("");
            }
            p
        })
        .collect();
    let got_presets: Vec<Value> = v["presets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let mut p = strip_nulls(p);
            if p.get("description").is_none() {
                p["description"] = json!("");
            }
            p
        })
        .collect();
    assert_eq!(got_presets, want_presets);
    assert_eq!(v["settings"], a.get("/api/settings").await.json());
    assert_eq!(v["settings"]["poll_interval_ms"], 777);
}

#[tokio::test]
async fn exporting_does_not_modify_anything() {
    let a = app();
    populate(&a).await;
    let before = state_of(&a).await;
    let _ = export(&a).await;
    let _ = export(&a).await;
    assert_eq!(state_of(&a).await, before);
}

// ---- import: the happy path ----------------------------------------------------------------

#[tokio::test]
async fn import_into_an_empty_controller_stores_everything_and_reports_the_counts() {
    let a = app();
    let r = import(&a, doc()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"links": 2, "presets": 2}));
    let links = a.get("/api/links").await.json();
    assert_eq!(links[0]["name"], "ab");
    assert_eq!(links[0]["workers"], json!([2, 6]));
    assert_eq!(links[1]["name"], "loop");
    assert!(absent(&links[1], "workers"));
    assert_eq!(
        links[0]["configs"][1],
        serde_json::to_value(cfg_dto(2.0e6, 0.0)).unwrap()
    );
    let presets = a.get("/api/presets").await.json();
    assert_eq!(presets[0]["name"], "sat");
    assert_eq!(presets[0]["description"], "geo");
    assert_eq!(presets[1]["name"], "lan");
    assert_eq!(a.get("/api/settings").await.json(), settings_doc());
    // each id is usable
    for id in ids(&links) {
        assert_eq!(a.get(&format!("/api/links/{id}")).await.status, 200);
    }
    for id in ids(&presets) {
        assert_eq!(a.get(&format!("/api/presets/{id}")).await.status, 200);
    }
}

#[tokio::test]
async fn import_replaces_links_presets_and_settings() {
    let a = app();
    populate(&a).await;
    let r = import(&a, doc()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&a.get("/api/links").await.json()), ["ab", "loop"]);
    assert_eq!(names(&a.get("/api/presets").await.json()), ["sat", "lan"]);
    assert_eq!(a.get("/api/settings").await.json(), settings_doc());
}

#[tokio::test]
async fn an_empty_document_clears_links_and_presets() {
    let a = app();
    populate(&a).await;
    let d = json!({"format": FORMAT, "version": 1, "settings": settings_doc(),
                   "links": [], "presets": []});
    let r = import(&a, d).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"links": 0, "presets": 0}));
    assert_eq!(a.get("/api/links").await.json(), json!([]));
    assert_eq!(a.get("/api/presets").await.json(), json!([]));
}

#[tokio::test]
async fn mode_replace_is_accepted_and_is_the_default() {
    let a = app();
    let r = a.post("/api/import?mode=replace", doc()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = a.post("/api/import", doc()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        a.get("/api/links").await.json().as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn any_other_mode_is_a_400_and_changes_nothing() {
    let a = app();
    populate(&a).await;
    let before = state_of(&a).await;
    for m in ["merge", "append"] {
        let r = a.post(&format!("/api/import?mode={m}"), doc()).await;
        assert_eq!(r.status, 400, "mode={m:?}: {}", r.text());
        assert!(r.message().contains("mode"), "{}", r.text());
    }
    assert_eq!(state_of(&a).await, before);
}

#[tokio::test]
async fn imported_ids_are_new_and_the_counters_go_on_above_them() {
    let a = app();
    populate(&a).await;
    let old_links = ids(&a.get("/api/links").await.json());
    let old_presets = ids(&a.get("/api/presets").await.json());
    assert_eq!(import(&a, doc()).await.status, 200);
    let links = ids(&a.get("/api/links").await.json());
    let presets = ids(&a.get("/api/presets").await.json());
    let max = |v: &[u64]| *v.iter().max().unwrap();
    assert!(
        links.iter().all(|i| *i > max(&old_links)),
        "{links:?} vs {old_links:?}"
    );
    assert!(
        presets.iter().all(|i| *i > max(&old_presets)),
        "{presets:?} vs {old_presets:?}"
    );
    assert_eq!(links.len(), links.iter().collect::<BTreeSet<_>>().len());
    // later creates continue above the imported ids
    let l = a.create_link("later", &["net_null9"]).await.json();
    assert!(l["id"].as_u64().unwrap() > max(&links));
    let p = a
        .post(
            "/api/presets",
            json!({"name": "later", "config": cfg_json(1.0e6, 0.0)}),
        )
        .await
        .json();
    assert!(p["id"].as_u64().unwrap() > max(&presets));
}

#[tokio::test]
async fn an_import_is_persisted_and_survives_a_router_rebuild() {
    let mut a = app();
    populate(&a).await;
    assert_eq!(import(&a, doc()).await.status, 200);
    let before = state_of(&a).await;
    a.restart();
    let after = state_of(&a).await;
    assert_eq!(
        (&before.0, &before.1, &before.2),
        (&after.0, &after.1, &after.2)
    );
    let v: Value = serde_json::from_slice(&before.3.unwrap()).unwrap();
    assert_eq!(v["links"].as_array().unwrap().len(), 2);
    assert_eq!(v["presets"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn import_does_not_touch_the_supervisor_and_sets_no_pending_restart() {
    let a = app();
    assert_eq!(import(&a, doc()).await.status, 200);
    assert!(a.supervisor.plans().is_empty());
    assert_eq!(a.supervisor.stops(), 0);
    let e = a.get("/api/emulation").await.json();
    assert_eq!(e["state"], "stopped");
    assert_eq!(e["pending_restart"], false);
}

#[tokio::test]
async fn export_then_import_then_export_is_identical_modulo_ids() {
    let a = app();
    // a setup built through the individual endpoints
    populate(&a).await;
    let mut body = link_body("w", &["net_null7"]);
    body["workers"] = json!([3]);
    assert_eq!(a.post("/api/links", body).await.status, 201);
    let mut c = cfg_json(3.0e6, 3.0e5);
    c["delayer"]["distribution"] = json!("LogNormalDistribution");
    c["limiter"] = json!({"mode": "TokenBucketFilter", "activate": true, "rate_bps": 2.0e9, "limit_bit": 1.0e6});
    c["loss_model"] = json!({"type": "GilbertElliot", "p": 0.01, "r": 0.5, "k": 0.9, "h": 0.1});
    c["seed"] = json!(7);
    c["histogram"] = json!({"bins": 32, "lo_ns": 0.0, "hi_ns": 1.0e7});
    c["delay_stats"] = json!(false);
    let r = a
        .post(
            "/api/presets",
            json!({"name": "rich", "description": "all the knobs", "config": c}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let e1 = export(&a).await;
    let r = import(&a, e1.clone()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!({"links": 3, "presets": 3}));
    let e2 = export(&a).await;
    assert_eq!(e1, e2);
    // a fresh controller ends up with the same export, too
    let b = app();
    assert_eq!(import(&b, e1.clone()).await.status, 200);
    assert_eq!(export(&b).await, e1);
}

#[tokio::test]
async fn pci_ports_are_normalised_to_lower_case_like_on_the_links_endpoint() {
    let a = app();
    let mut d = doc();
    d["links"][0]["ports"] = json!([NIC0.to_uppercase(), NIC1.to_uppercase()]);
    let r = import(&a, d).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        a.get("/api/links").await.json()[0]["ports"],
        json!([NIC0, NIC1])
    );
}

// ---- import: refused while the emulation is not stopped ---------------------------------------

#[tokio::test]
async fn import_while_running_is_a_409_and_changes_nothing() {
    let a = app();
    populate(&a).await;
    assert_eq!(a.post("/api/emulation/start", json!({})).await.status, 202);
    let before = state_of(&a).await;
    let r = import(&a, doc()).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().to_lowercase().contains("stop"), "{}", r.text());
    assert_eq!(state_of(&a).await, before);
    let e = a.get("/api/emulation").await.json();
    assert_eq!(e["pending_restart"], false);
    // stopping makes it work
    assert_eq!(a.post("/api/emulation/stop", json!({})).await.status, 202);
    assert_eq!(import(&a, doc()).await.status, 200);
}

#[tokio::test]
async fn import_is_a_409_in_every_state_but_stopped() {
    for st in [
        State::Starting,
        State::Running,
        State::Stopping,
        State::Failed,
    ] {
        let a = app();
        populate(&a).await;
        a.supervisor.force_status(status(st));
        let before = state_of(&a).await;
        let r = import(&a, doc()).await;
        assert_eq!(r.status, 409, "{st:?}: {}", r.text());
        r.message();
        assert_eq!(state_of(&a).await, before, "{st:?}");
    }
}

// ---- import: validation, all or nothing -------------------------------------------------------

/// `(label, mutation of the valid document)`: each makes the whole import a 400.
fn bad_documents() -> Vec<(&'static str, Value)> {
    let mut out: Vec<(&'static str, Value)> = Vec::new();
    let mut add = |label: &'static str, f: &dyn Fn(&mut Value)| {
        let mut d = doc();
        f(&mut d);
        out.push((label, d));
    };
    add("wrong format", &|d| d["format"] = json!("something-else"));
    add("missing format", &|d| {
        d.as_object_mut().unwrap().remove("format");
    });
    add("version 2", &|d| d["version"] = json!(2));
    add("version 0", &|d| d["version"] = json!(0));
    add("version as string", &|d| d["version"] = json!("1"));
    add("missing version", &|d| {
        d.as_object_mut().unwrap().remove("version");
    });
    add("unknown top-level key", &|d| d["extra"] = json!(true));
    add("link with an id", &|d| d["links"][0]["id"] = json!(5));
    add("unknown link key", &|d| {
        d["links"][1]["colour"] = json!("red")
    });
    add("preset with an id", &|d| d["presets"][0]["id"] = json!(3));
    add("unknown preset key", &|d| d["presets"][1]["x"] = json!(1));
    add("unknown settings key", &|d| {
        d["settings"]["surprise"] = json!(1)
    });
    add("unknown key in a link config", &|d| {
        d["links"][1]["configs"][0]["seeed"] = json!(1)
    });
    add("unknown key in a preset config", &|d| {
        d["presets"][0]["config"]["mean_nss"] = json!(1)
    });
    // the second link / preset is the broken one: the first, valid one must not be stored
    add("invalid config in the second link", &|d| {
        d["links"][1]["configs"][0]["delayer"]["mean_ns"] = json!(-1.0)
    });
    add("loss probability above 1", &|d| {
        d["links"][0]["configs"][1]["loss_model"] = json!({"type": "Random", "p": 2.0})
    });
    add("invalid link name", &|d| {
        d["links"][1]["name"] = json!("bad name!")
    });
    add("empty link name", &|d| d["links"][0]["name"] = json!(""));
    add("three ports", &|d| {
        d["links"][0]["ports"] = json!([NIC0, NIC1, INTEL])
    });
    add("no ports", &|d| d["links"][1]["ports"] = json!([]));
    add("port with devargs", &|d| {
        d["links"][1]["ports"] = json!(["net_null0,iface=eth0"])
    });
    add("same port twice in a link", &|d| {
        d["links"][0]["ports"] = json!([NIC0, NIC0])
    });
    add("config count != port count", &|d| {
        d["links"][0]["configs"] = json!([cfg_json(1.0e6, 0.0)])
    });
    add("workers count != port count", &|d| {
        d["links"][0]["workers"] = json!([2])
    });
    add("port shared between links", &|d| {
        d["links"][1]["ports"] = json!([NIC1])
    });
    add("port shared between links, other case", &|d| {
        d["links"][1]["ports"] = json!([NIC0.to_uppercase()])
    });
    add("empty preset name", &|d| {
        d["presets"][1]["name"] = json!("")
    });
    add("preset name of 65 characters", &|d| {
        d["presets"][1]["name"] = json!("n".repeat(65))
    });
    add("invalid preset config", &|d| {
        d["presets"][1]["config"]["limiter"] =
            json!({"mode": "Spacer", "activate": true, "rate_bps": -1.0, "limit_bit": 1.0})
    });
    add("poll interval 0", &|d| {
        d["settings"]["poll_interval_ms"] = json!(0)
    });
    add("mprq devargs", &|d| {
        d["settings"]["mlx5_devargs"] = json!("mprq_en=1")
    });
    add("disallowed EAL argument", &|d| {
        d["settings"]["eal_extra_args"] = json!(["--vdev=net_pcap0,tx_pcap=/tmp/x"])
    });
    add("links is not an array", &|d| d["links"] = json!("none"));
    add("settings missing a field", &|d| {
        d["settings"]
            .as_object_mut()
            .unwrap()
            .remove("poll_interval_ms");
    });
    out.push(("an array instead of an object", json!([])));
    out.push(("a string instead of an object", json!("hello")));
    out
}

#[tokio::test]
async fn an_invalid_document_is_a_400_and_nothing_changes() {
    let a = app();
    populate(&a).await;
    let before = state_of(&a).await;
    for (label, d) in bad_documents() {
        let r = import(&a, d).await;
        assert!(
            r.status == 400,
            "{label}: expected 400, got {} {}",
            r.status,
            r.text()
        );
        r.message();
        assert_eq!(state_of(&a).await, before, "{label}: state changed");
    }
    // and the controller still takes a good document afterwards
    assert_eq!(import(&a, doc()).await.status, 200);
}

#[tokio::test]
async fn an_invalid_document_into_an_empty_controller_stores_nothing() {
    let a = app();
    for (label, d) in bad_documents() {
        assert_eq!(import(&a, d).await.status, 400, "{label}");
    }
    assert_eq!(a.get("/api/links").await.json(), json!([]));
    assert_eq!(a.get("/api/presets").await.json(), json!([]));
    assert_eq!(a.get("/api/settings").await.json(), {
        let fresh = app();
        fresh.get("/api/settings").await.json()
    });
}

#[tokio::test]
async fn bad_documents_do_not_burn_ids() {
    let a = app();
    for (_, d) in bad_documents() {
        assert_eq!(import(&a, d).await.status, 400);
    }
    // nothing was applied, so the first link gets the same id as on a fresh controller
    let fresh = app();
    let want = fresh.create_link("x", &["net_null0"]).await.json()["id"].clone();
    let got = a.create_link("x", &["net_null0"]).await.json()["id"].clone();
    assert_eq!(got, want);
}

#[tokio::test]
async fn the_message_of_a_rejected_part_names_it() {
    let a = app();
    let mut d = doc();
    d["links"][1]["configs"][0]["delayer"]["mean_ns"] = json!(-1.0);
    assert!(import(&a, d).await.message().contains("mean_ns"));
    let mut d = doc();
    d["links"][1]["ports"] = json!([NIC1]);
    assert!(import(&a, d).await.message().contains(NIC1));
    let mut d = doc();
    d["format"] = json!("nope");
    assert!(import(&a, d).await.message().contains("format"));
    let mut d = doc();
    d["version"] = json!(9);
    assert!(import(&a, d).await.message().contains("version"));
}

#[tokio::test]
async fn duplicate_names_inside_the_document_are_refused() {
    let a = app();
    populate(&a).await;
    let before = state_of(&a).await;
    let mut d = doc();
    d["links"][1]["name"] = json!("ab");
    let r = import(&a, d).await;
    assert!(r.status == 400 || r.status == 409, "{}", r.status);
    assert!(r.message().contains("ab"));
    let mut d = doc();
    d["presets"][1]["name"] = json!("sat");
    let r = import(&a, d).await;
    assert!(r.status == 400 || r.status == 409, "{}", r.status);
    assert!(r.message().contains("sat"));
    assert_eq!(state_of(&a).await, before);
}

#[tokio::test]
async fn names_of_the_replaced_setup_do_not_conflict_with_the_document() {
    // the old links/presets are gone after the import, so reusing their names is fine
    let a = app();
    populate(&a).await;
    let mut d = doc();
    d["links"][0]["name"] = json!("old-a");
    d["links"][0]["ports"] = json!([NIC0, NIC1]);
    d["presets"][0]["name"] = json!("old-p1");
    let r = import(&a, d).await;
    assert_eq!(r.status, 200, "{}", r.text());
}

// ---- hardening ----------------------------------------------------------------------------

#[tokio::test]
async fn import_needs_a_json_content_type_and_obeys_the_body_limit() {
    let a = app();
    populate(&a).await;
    let before = state_of(&a).await;
    let r = a
        .request_raw(
            "POST",
            "/api/import",
            Some("text/plain"),
            doc().to_string().into_bytes(),
        )
        .await;
    assert_eq!(r.status, 415, "{}", r.text());
    r.message();
    let r = a
        .request_raw("POST", "/api/import", None, doc().to_string().into_bytes())
        .await;
    assert_eq!(r.status, 415);
    // a valid document padded past 1 MiB with a long preset description
    let mut big = doc();
    big["presets"][0]["description"] = json!("x".repeat(2 * 1024 * 1024));
    let r = import(&a, big).await;
    assert_eq!(r.status, 413, "{}", &r.text()[..r.text().len().min(200)]);
    r.message();
    assert_eq!(state_of(&a).await, before);
}

#[tokio::test]
async fn malformed_json_is_a_400_with_a_message() {
    let a = app();
    let r = a
        .request_raw(
            "POST",
            "/api/import",
            Some("application/json"),
            b"{\"format\": ".to_vec(),
        )
        .await;
    assert!(r.status == 400 || r.status == 422, "{}", r.status);
    r.message();
}

#[tokio::test]
async fn export_and_import_only_answer_their_methods() {
    let a = app();
    assert_eq!(a.post("/api/export", json!({})).await.status, 405);
    assert_eq!(a.get("/api/import").await.status, 405);
    assert_eq!(a.delete("/api/export").await.status, 405);
}

// ---- OpenAPI ------------------------------------------------------------------------------

#[tokio::test]
async fn openapi_documents_export_and_import() {
    let a = app();
    let v = a.get("/api/openapi.json").await.json();
    let expect: &[(&str, &str, &[&str])] = &[
        ("/api/export", "get", &["200"]),
        (
            "/api/import",
            "post",
            &["200", "400", "409", "413", "415", "500"],
        ),
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
    let params = v["paths"]["/api/import"]["post"]["parameters"].to_string();
    assert!(
        params.contains("mode"),
        "the mode query parameter is documented: {params}"
    );
}
