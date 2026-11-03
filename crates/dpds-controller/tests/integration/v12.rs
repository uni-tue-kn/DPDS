//! Spec v12 (controller): the required correlation initialization rate
//! `delayer.initialization_rate_pps` and the controller review lows (C8).
//!
//! Contract (spec text first, then ASSUMPTIONs of the tester):
//! - `ConfigDto.delayer.initialization_rate_pps` is required everywhere a config is
//!   accepted (links, presets, import): a body without it is a 400 whose message names the
//!   field (a 422 is mapped to 400 by the controller); a value that is not finite or below 1
//!   is a 400 naming the field. It is stored, returned, exported, imported, part of presets
//!   and written into the links file.
//! - A stored state file whose link or preset config has no such key is refused at startup
//!   like the old link shape (no defaults for old files).
//! - `launch_plan` re-validates link names with the rules of create (1 to 32 characters of
//!   `A-Za-z0-9_.-`, unique). ASSUMPTION: refused with 400 or 409 and a message that names the
//!   name or says "link name"/"already exists".
//! - dpds' `/dpds/ports` telemetry dict holds `3 + 5 * distinct ports + 10 * links` entries, at
//!   most 256. The controller checks that on create, update, import and start. ASSUMPTION: 400
//!   or 409, the message contains `256`. Boundaries used (5 * (ports + 2 * links) + 3):
//!   a ring of 16 links over 16 ports is 243 and fits, a ring of 17 is 258 and fails;
//!   16 hairpins fit, 17 fail; 12 one-way links over 24 ports fit (243), 13 fail (263);
//!   18 ports with 16 links (two chains) is exactly 253 and fits, one more link or one more
//!   port than that fails. (A total of 254 to 257 cannot occur: totals are 3 mod 5.)
//! - `check_version`: a version other than 2 gets a message that names the supported version 2;
//!   only version 1 keeps the hint about the old paired links.
//! - OpenAPI: `worker` has `minimum: 0` in `Link`, `LinkInput` and `ExportLink`.

use crate::common::*;
use dpds_controller::api::{router, Deps};
use dpds_controller::model::{Link, StoredState};
use dpds_controller::store::{self, STATE_FILE_NAME};
use serde_json::{json, Value};

const FORMAT: &str = "dpds-controller-export";
const RATE: &str = "initialization_rate_pps";

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

/// 80 CPUs, 1-79 isolated: room for every worker of the largest rings below.
fn big_host() -> FakeHost {
    let h = FakeHost::empty();
    h.cpus(80, Some("1-79"), &[]);
    h
}

fn cfg_rate(rate: Value) -> Value {
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["delayer"][RATE] = rate;
    c
}

fn cfg_without_rate() -> Value {
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["delayer"].as_object_mut().unwrap().remove(RATE);
    c
}

fn link_body_cfg(name: &str, ingress: &str, egress: &str, config: Value) -> Value {
    json!({"name": name, "ingress": ingress, "egress": egress, "config": config})
}

fn assert_400_naming(r: &Reply, what: &str, named: &str) {
    assert_eq!(r.status, 400, "{what}: {} {}", r.status, r.text());
    let m = r.message();
    assert!(m.contains(named), "{what}: the message names {named}: {m}");
}

fn assert_refused_naming(r: &Reply, what: &str, named: &str) {
    assert!(
        r.status == 400 || r.status == 409,
        "{what}: expected 400 or 409, got {} {}",
        r.status,
        r.text()
    );
    let m = r.message();
    assert!(
        m.contains(named),
        "{what}: the message names {named:?}: {m}"
    );
}

// ---- initialization_rate_pps: links ----------------------------------------------------------

#[tokio::test]
async fn a_link_without_the_rate_is_a_400_naming_the_field() {
    let a = app();
    let r = a
        .post(
            "/api/links",
            link_body_cfg("x", "net_null0", "net_null1", cfg_without_rate()),
        )
        .await;
    assert_400_naming(&r, "POST", RATE);
    assert_eq!(a.get("/api/links").await.json(), json!([]));

    // and on update
    let id = a.create_link("ok", "net_null0", "net_null1").await.json()["id"]
        .as_u64()
        .unwrap();
    let r = a
        .put(
            &format!("/api/links/{id}"),
            link_body_cfg("ok", "net_null0", "net_null1", cfg_without_rate()),
        )
        .await;
    assert_400_naming(&r, "PUT", RATE);
}

#[tokio::test]
async fn an_invalid_rate_is_a_400_naming_the_field_on_create_and_update() {
    let a = app();
    let id = a.create_link("ok", "net_null0", "net_null1").await.json()["id"]
        .as_u64()
        .unwrap();
    for bad in [
        json!(0),
        json!(0.5),
        json!(0.999),
        json!(-1),
        json!(-15.0e6),
    ] {
        let r = a
            .post(
                "/api/links",
                link_body_cfg("bad", "net_null2", "net_null3", cfg_rate(bad.clone())),
            )
            .await;
        assert_400_naming(&r, &format!("POST rate {bad}"), RATE);
        let r = a
            .put(
                &format!("/api/links/{id}"),
                link_body_cfg("ok", "net_null0", "net_null1", cfg_rate(bad.clone())),
            )
            .await;
        assert_400_naming(&r, &format!("PUT rate {bad}"), RATE);
    }
    // nothing changed
    let links = a.get("/api/links").await.json();
    assert_eq!(links.as_array().unwrap().len(), 1);
    assert_eq!(links[0]["config"]["delayer"][RATE], json!(15.0e6));
}

#[tokio::test]
async fn a_rate_below_one_is_refused_and_exactly_one_is_accepted() {
    let a = app();
    let r = a
        .post(
            "/api/links",
            link_body_cfg("low", "net_null0", "net_null1", cfg_rate(json!(0.9999999))),
        )
        .await;
    assert_400_naming(&r, "0.9999999", RATE);
    let r = a
        .post(
            "/api/links",
            link_body_cfg("one", "net_null0", "net_null1", cfg_rate(json!(1))),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
}

#[tokio::test]
async fn the_rate_is_stored_returned_and_changed_by_update() {
    let a = app();
    let r = a
        .post(
            "/api/links",
            link_body_cfg("ab", "net_null0", "net_null1", cfg_rate(json!(2.5e6))),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let created = r.json();
    assert_eq!(created["config"]["delayer"][RATE], json!(2.5e6));
    let id = created["id"].as_u64().unwrap();
    let got = a.get(&format!("/api/links/{id}")).await.json();
    assert_eq!(got["config"]["delayer"][RATE], json!(2.5e6));
    let listed = a.get("/api/links").await.json();
    assert_eq!(listed[0]["config"]["delayer"][RATE], json!(2.5e6));

    let r = a
        .put(
            &format!("/api/links/{id}"),
            link_body_cfg("ab", "net_null0", "net_null1", cfg_rate(json!(300000))),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["config"]["delayer"][RATE], json!(300000.0));
    // two links keep their own rate
    let r = a
        .post(
            "/api/links",
            link_body_cfg("ba", "net_null1", "net_null0", cfg_rate(json!(9.0e6))),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let listed = a.get("/api/links").await.json();
    assert_eq!(listed[0]["config"]["delayer"][RATE], json!(300000.0));
    assert_eq!(listed[1]["config"]["delayer"][RATE], json!(9.0e6));
}

#[tokio::test]
async fn the_rate_survives_a_controller_restart() {
    let mut a = app();
    let r = a
        .post(
            "/api/links",
            link_body_cfg("ab", "net_null0", "net_null1", cfg_rate(json!(1.25e6))),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    a.restart();
    let listed = a.get("/api/links").await.json();
    assert_eq!(listed[0]["config"]["delayer"][RATE], json!(1.25e6));
}

#[tokio::test]
async fn the_rate_of_every_link_is_written_into_the_links_file_at_start() {
    let a = TestApp::new(big_host());
    for (name, i, o, rate) in [
        ("slow", "net_null0", "net_null1", 250_000.0),
        ("fast", "net_null1", "net_null0", 7.5e6),
    ] {
        let r = a
            .post(
                "/api/links",
                link_body_cfg(name, i, o, cfg_rate(json!(rate))),
            )
            .await;
        assert_eq!(r.status, 201, "{}", r.text());
    }
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 202, "{}", r.text());
    let plans = a.supervisor.plans();
    assert_eq!(plans.len(), 1);
    let links = &plans[0].links_json["links"];
    assert_eq!(links[0]["name"], "slow");
    assert_eq!(links[0]["config"]["delayer"][RATE], json!(250_000.0));
    assert_eq!(links[1]["name"], "fast");
    assert_eq!(links[1]["config"]["delayer"][RATE], json!(7.5e6));
}

// ---- initialization_rate_pps: presets, export, import ---------------------------------------

#[tokio::test]
async fn presets_require_validate_and_return_the_rate() {
    let a = app();
    let r = a
        .post(
            "/api/presets",
            json!({"name": "p", "config": cfg_without_rate()}),
        )
        .await;
    assert_400_naming(&r, "preset without the rate", RATE);
    for bad in [json!(0), json!(0.5), json!(-3)] {
        let r = a
            .post(
                "/api/presets",
                json!({"name": "p", "config": cfg_rate(bad.clone())}),
            )
            .await;
        assert_400_naming(&r, &format!("preset rate {bad}"), RATE);
    }
    let r = a
        .post(
            "/api/presets",
            json!({"name": "p", "config": cfg_rate(json!(2.5e6))}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let p = r.json();
    assert_eq!(p["config"]["delayer"][RATE], json!(2.5e6));
    let id = p["id"].as_u64().unwrap();
    // update: missing and invalid are refused, a new value is stored
    let r = a
        .put(
            &format!("/api/presets/{id}"),
            json!({"name": "p", "config": cfg_without_rate()}),
        )
        .await;
    assert_400_naming(&r, "preset update without the rate", RATE);
    let r = a
        .put(
            &format!("/api/presets/{id}"),
            json!({"name": "p", "config": cfg_rate(json!(0.1))}),
        )
        .await;
    assert_400_naming(&r, "preset update with rate 0.1", RATE);
    let r = a
        .put(
            &format!("/api/presets/{id}"),
            json!({"name": "p", "config": cfg_rate(json!(4.0e6))}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["config"]["delayer"][RATE], json!(4.0e6));
    assert_eq!(
        a.get("/api/presets").await.json()[0]["config"]["delayer"][RATE],
        json!(4.0e6)
    );
}

fn doc_with(link_rate: Value, preset_rate: Value) -> Value {
    json!({
        "format": FORMAT,
        "version": 2,
        "settings": {"main_lcore": null,
                     "eal_extra_args": [], "poll_interval_ms": 250},
        "links": [{"name": "ab", "ingress": "net_null0", "egress": "net_null1",
                   "config": cfg_rate(link_rate)}],
        "presets": [{"name": "sat", "config": cfg_rate(preset_rate)}]
    })
}

#[tokio::test]
async fn export_carries_the_rate_and_import_restores_it() {
    let a = app();
    let r = a
        .post(
            "/api/links",
            link_body_cfg("ab", "net_null0", "net_null1", cfg_rate(json!(2.5e6))),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let r = a
        .post(
            "/api/presets",
            json!({"name": "sat", "config": cfg_rate(json!(80000.0))}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let exported = a.get("/api/export").await.json();
    assert_eq!(
        exported["links"][0]["config"]["delayer"][RATE],
        json!(2.5e6),
        "{exported}"
    );
    assert_eq!(
        exported["presets"][0]["config"]["delayer"][RATE],
        json!(80000.0),
        "{exported}"
    );
    // into a fresh controller
    let b = app();
    let r = b.post("/api/import", exported.clone()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        b.get("/api/links").await.json()[0]["config"]["delayer"][RATE],
        json!(2.5e6)
    );
    assert_eq!(
        b.get("/api/presets").await.json()[0]["config"]["delayer"][RATE],
        json!(80000.0)
    );
    // and the export of the importer is the same document
    assert_eq!(b.get("/api/export").await.json(), exported);
}

#[tokio::test]
async fn import_requires_and_validates_the_rate_of_links_and_presets() {
    let a = app();
    assert_eq!(
        a.create_link("keep", "net_null7", "net_null8").await.status,
        201
    );
    let snapshot = || async {
        (
            a.get("/api/links").await.json(),
            a.get("/api/presets").await.json(),
            std::fs::read(a.state_dir.path().join(STATE_FILE_NAME)).ok(),
        )
    };
    let before = snapshot().await;

    // control: the document is fine with valid rates
    let ok = doc_with(json!(1e6), json!(2e6));
    assert_eq!(app().post("/api/import", ok).await.status, 200);

    let mut missing_link = doc_with(json!(1e6), json!(2e6));
    missing_link["links"][0]["config"] = cfg_without_rate();
    let mut missing_preset = doc_with(json!(1e6), json!(2e6));
    missing_preset["presets"][0]["config"] = cfg_without_rate();
    for (what, d) in [
        ("link without the rate", missing_link),
        ("preset without the rate", missing_preset),
        ("link rate 0", doc_with(json!(0), json!(2e6))),
        ("link rate 0.5", doc_with(json!(0.5), json!(2e6))),
        ("link rate -1", doc_with(json!(-1), json!(2e6))),
        ("preset rate 0", doc_with(json!(1e6), json!(0))),
        ("preset rate -5", doc_with(json!(1e6), json!(-5))),
    ] {
        let r = a.post("/api/import", d).await;
        assert_400_naming(&r, what, RATE);
        assert_eq!(snapshot().await, before, "{what}: nothing may change");
    }
}

// ---- a state file from before the field: refused, left untouched ----------------------------

fn router_over(dir: &std::path::Path) -> Result<axum::Router, store::StoreError> {
    let host = FakeHost::bare_metal();
    router(Deps {
        state_dir: dir.to_path_buf(),
        roots: host.roots(),
        supervisor: FakeSupervisor::new(),
        telemetry: FakeTelemetry::new(),
        dpds_bin: "/x".into(),
        runtime_dir: "/y".into(),
        dpds_has_telemetry: false,
        is_root: false,
    })
}

#[test]
fn a_state_file_whose_config_lacks_the_rate_is_refused_naming_the_file_and_left_untouched() {
    for place in ["link", "preset"] {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(STATE_FILE_NAME);
        let mut s = serde_json::to_value(StoredState::default()).unwrap();
        let broken = cfg_without_rate();
        if place == "link" {
            s["links"] = json!([{"id": 1, "name": "a", "ingress": "net_null0",
                                 "egress": "net_null1", "config": broken}]);
            s["next_id"] = json!(2);
        } else {
            s["presets"] = json!([{"id": 1, "name": "p", "config": broken}]);
            s["next_preset_id"] = json!(2);
        }
        let bytes = serde_json::to_vec_pretty(&s).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let err = match router_over(tmp.path()) {
            Ok(_) => panic!("{place}: the router must refuse a config without the rate"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains(STATE_FILE_NAME), "{place}: {err}");
        assert!(
            err.contains(RATE),
            "{place}: the error names the field: {err}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "{place}: file untouched"
        );
        assert!(store::load(tmp.path()).is_err(), "{place}");
    }
}

// ---- C8: launch_plan re-validates link names ------------------------------------------------

/// A controller restarted over a state file written behind its back.
fn app_with(state: &StoredState) -> TestApp {
    let mut a = TestApp::new(big_host());
    store::save(a.state_dir.path(), state).unwrap();
    a.restart();
    a
}

async fn assert_start_refused(a: &TestApp, named: &str) {
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_refused_naming(&r, "start", named);
    assert!(a.supervisor.plans().is_empty(), "nothing may be spawned");
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

#[tokio::test]
async fn a_stored_link_name_the_rules_forbid_refuses_the_start() {
    let too_long = "n".repeat(33);
    for bad in [
        "",
        "has space",
        "slash/name",
        "semi;colon",
        "quote\"d",
        "uml\u{e4}ut",
        "new\nline",
        too_long.as_str(),
    ] {
        let a = app_with(&state_with(vec![
            link(1, "fine", "net_null0", "net_null0", None),
            link(2, bad, "net_null1", "net_null1", None),
        ]));
        let r = a.post("/api/emulation/start", json!({})).await;
        assert!(
            r.status == 400 || r.status == 409,
            "{bad:?}: {} {}",
            r.status,
            r.text()
        );
        let m = r.message();
        assert!(
            m.contains("link name") || (!bad.is_empty() && m.contains(bad)),
            "{bad:?}: the message names the rule or the name: {m}"
        );
        assert!(a.supervisor.plans().is_empty(), "{bad:?} was spawned");
        assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
    }
}

#[tokio::test]
async fn a_stored_name_of_exactly_32_valid_characters_starts() {
    let name = "aZ09_.-x".repeat(4);
    assert_eq!(name.len(), 32);
    let a = app_with(&state_with(vec![link(
        1,
        &name,
        "net_null0",
        "net_null1",
        None,
    )]));
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 202, "{}", r.text());
}

#[tokio::test]
async fn duplicate_stored_link_names_refuse_the_start_naming_the_name() {
    let a = app_with(&state_with(vec![
        link(1, "twin", "net_null0", "net_null1", None),
        link(2, "twin", "net_null2", "net_null3", None),
    ]));
    assert_start_refused(&a, "twin").await;
}

// ---- C8: the dpds telemetry dict limit, 3 + 5 * ports + 10 * links <= 256 --------------------

/// Creates the links `(name, ingress, egress)` in order; all must be accepted.
async fn create_all(a: &TestApp, links: &[(String, String, String)]) {
    for (n, i, o) in links {
        let r = a.create_link(n, i, o).await;
        assert_eq!(r.status, 201, "{n}: {} {}", r.status, r.text());
    }
}

fn nul(i: usize) -> String {
    format!("net_null{i}")
}

/// `n` links `r0..` closing a ring over `n` ports.
fn ring(n: usize) -> Vec<(String, String, String)> {
    (0..n)
        .map(|i| (format!("r{i}"), nul(i), nul((i + 1) % n)))
        .collect()
}

/// `n` hairpins over `n` ports.
fn hairpins(n: usize) -> Vec<(String, String, String)> {
    (0..n).map(|i| (format!("h{i}"), nul(i), nul(i))).collect()
}

/// `n` one-way links over `2n` ports.
fn one_way(n: usize) -> Vec<(String, String, String)> {
    (0..n)
        .map(|i| (format!("w{i}"), nul(2 * i), nul(2 * i + 1)))
        .collect()
}

/// Two chains: 9 links over ports 0..=9 and 7 links over ports 10..=17: 18 ports and 16 links,
/// 3 + 90 + 160 = 253 entries.
fn two_chains() -> Vec<(String, String, String)> {
    let mut v: Vec<(String, String, String)> = (0..9)
        .map(|i| (format!("a{i}"), nul(i), nul(i + 1)))
        .collect();
    v.extend((0..7).map(|i| (format!("b{i}"), nul(10 + i), nul(11 + i))));
    v
}

#[tokio::test]
async fn a_ring_of_16_links_fits_and_the_17th_link_of_a_ring_of_17_does_not() {
    // 16 links over 16 ports: 3 + 80 + 160 = 243
    let a = app();
    create_all(&a, &ring(16)).await;
    assert_eq!(
        a.get("/api/links").await.json().as_array().unwrap().len(),
        16
    );

    // 17 links over 17 ports: 3 + 85 + 170 = 258. The first 16 (a chain over 17 ports:
    // 3 + 85 + 160 = 248) fit, closing the ring does not.
    let a = app();
    let links = ring(17);
    create_all(&a, &links[..16]).await;
    let (n, i, o) = &links[16];
    let r = a.create_link(n, i, o).await;
    assert_refused_naming(&r, "17th link of the ring", "256");
    assert_eq!(
        a.get("/api/links").await.json().as_array().unwrap().len(),
        16
    );
}

#[tokio::test]
async fn sixteen_hairpins_fit_and_the_seventeenth_does_not() {
    let a = app();
    let links = hairpins(17);
    create_all(&a, &links[..16]).await;
    let (n, i, o) = &links[16];
    assert_refused_naming(&a.create_link(n, i, o).await, "17th hairpin", "256");
}

#[tokio::test]
async fn twelve_one_way_links_fit_and_the_thirteenth_does_not() {
    let a = app();
    let links = one_way(13);
    create_all(&a, &links[..12]).await;
    let (n, i, o) = &links[12];
    assert_refused_naming(&a.create_link(n, i, o).await, "13th one-way link", "256");
}

#[tokio::test]
async fn exactly_253_entries_fit_and_one_more_link_or_port_does_not() {
    let a = app();
    create_all(&a, &two_chains()).await; // 18 ports, 16 links: 253

    // a link between existing ports (end of chain a -> start of chain b) adds 10: 263
    let r = a.create_link("bridge", &nul(9), &nul(10)).await;
    assert_refused_naming(&r, "bridge link", "256");
    // a hairpin on a new port adds 15
    let r = a.create_link("extra", &nul(40), &nul(40)).await;
    assert_refused_naming(&r, "extra hairpin", "256");
    assert_eq!(
        a.get("/api/links").await.json().as_array().unwrap().len(),
        16
    );
}

#[tokio::test]
async fn an_update_is_checked_against_the_limit_without_counting_the_old_version_twice() {
    let a = app();
    create_all(&a, &two_chains()).await; // 253
    let links = a.get("/api/links").await.json();
    let a4 = links
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["name"] == "a4")
        .unwrap()
        .clone();
    let id = a4["id"].as_u64().unwrap();
    let url = format!("/api/links/{id}");

    // control: same port count (the new egress is a port already in use): accepted
    let r = a
        .put(
            &url,
            link_body_cfg("a4", &nul(4), &nul(0), cfg_json(1.0e6, 1.0e5)),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    // a new egress port makes 19 ports: 3 + 95 + 160 = 258
    let r = a
        .put(
            &url,
            link_body_cfg("a4", &nul(4), &nul(99), cfg_json(1.0e6, 1.0e5)),
        )
        .await;
    assert_refused_naming(&r, "update to a new port", "256");
    // the stored link is still the control's
    let got = a.get(&url).await.json();
    assert_eq!(got["egress"], nul(0));
}

fn ring_doc(n: usize) -> Value {
    let links: Vec<Value> = ring(n)
        .into_iter()
        .map(|(name, i, o)| link_body_cfg(&name, &i, &o, cfg_json(1.0e6, 1.0e5)))
        .collect();
    json!({
        "format": FORMAT,
        "version": 2,
        "settings": {"main_lcore": null,
                     "eal_extra_args": [], "poll_interval_ms": 250},
        "links": links,
        "presets": []
    })
}

#[tokio::test]
async fn import_checks_the_limit_and_changes_nothing_when_it_is_exceeded() {
    let a = app();
    assert_eq!(
        a.create_link("keep", "net_null90", "net_null91")
            .await
            .status,
        201
    );
    let before = a.get("/api/links").await.json();

    let r = a.post("/api/import", ring_doc(17)).await;
    assert_refused_naming(&r, "ring of 17", "256");
    assert_eq!(
        a.get("/api/links").await.json(),
        before,
        "nothing may change"
    );

    let r = a.post("/api/import", ring_doc(16)).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        a.get("/api/links").await.json().as_array().unwrap().len(),
        16
    );
}

fn state_of_links(links: Vec<(String, String, String)>) -> StoredState {
    let ls: Vec<Link> = links
        .iter()
        .enumerate()
        .map(|(k, (n, i, o))| link(k as u32 + 1, n, i, o, None))
        .collect();
    state_with(ls)
}

#[tokio::test]
async fn start_checks_the_limit_of_a_hand_edited_state() {
    // control: 16 links over 16 ports (243) start
    let a = app_with(&state_of_links(ring(16)));
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(a.supervisor.plans()[0].directions.len(), 16);

    // 17 links over 17 ports (258) are refused naming the limit, nothing is spawned
    let a = app_with(&state_of_links(ring(17)));
    assert_start_refused(&a, "256").await;

    // 13 one-way links (263) as well
    let a = app_with(&state_of_links(one_way(13)));
    assert_start_refused(&a, "256").await;
}

// ---- C8: check_version -----------------------------------------------------------------------

#[tokio::test]
async fn a_version_other_than_1_or_2_just_names_the_supported_version() {
    for v in [0u64, 3, 9, 100] {
        let a = app();
        let mut d = doc_with(json!(1e6), json!(2e6));
        d["version"] = json!(v);
        let r = a.post("/api/import", d).await;
        assert_eq!(r.status, 400, "version {v}: {}", r.text());
        let m = r.message();
        let lower = m.to_lowercase();
        assert!(lower.contains("version"), "version {v}: {m}");
        assert!(
            m.contains(&v.to_string()),
            "version {v}: names the given version: {m}"
        );
        assert!(
            lower.contains("version 2") || m.contains("= 2") || m.contains(" 2 "),
            "version {v}: names the supported version 2: {m}"
        );
        for stale in ["version 1", "two ports", "paired", "re-create", "ingress"] {
            assert!(
                !lower.contains(stale),
                "version {v}: the old-document hint `{stale}` is only for version 1: {m}"
            );
        }
        assert_eq!(a.get("/api/links").await.json(), json!([]));
    }
}

#[tokio::test]
async fn version_1_still_gets_a_message_naming_both_versions() {
    let a = app();
    let mut d = doc_with(json!(1e6), json!(2e6));
    d["version"] = json!(1);
    let r = a.post("/api/import", d).await;
    assert_eq!(r.status, 400, "{}", r.text());
    let m = r.message().to_lowercase();
    assert!(
        m.contains("version") && m.contains('1') && m.contains('2'),
        "{m}"
    );
}

// ---- C8: OpenAPI `worker` minimum ------------------------------------------------------------

/// The `minimum` found anywhere inside `v` (the property may be wrapped in `oneOf`/`anyOf`/
/// `allOf` or have a type array, depending on how the schema is generated).
fn minimums(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "minimum" {
                    out.push(x.clone());
                } else {
                    minimums(x, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| minimums(x, out)),
        _ => {}
    }
}

#[tokio::test]
async fn openapi_worker_has_minimum_zero_in_link_linkinput_and_exportlink() {
    let a = app();
    let v = a.get("/api/openapi.json").await.json();
    let schemas = &v["components"]["schemas"];
    for name in ["Link", "LinkInput", "ExportLink"] {
        let worker = &schemas[name]["properties"]["worker"];
        assert!(
            worker.is_object(),
            "{name}.worker is missing: {}",
            schemas[name]
        );
        let mut found = Vec::new();
        minimums(worker, &mut found);
        assert!(
            found.len() == 1 && found[0].as_f64() == Some(0.0),
            "{name}.worker must have exactly one `minimum`, 0: {found:?} in {worker}"
        );
    }
}

#[tokio::test]
async fn openapi_documents_the_required_initialization_rate() {
    let a = app();
    let v = a.get("/api/openapi.json").await.json();
    let d = &v["components"]["schemas"]["DelayerDto"];
    let prop = &d["properties"][RATE];
    assert!(prop.is_object(), "DelayerDto.{RATE} is missing: {d}");
    assert!(
        prop.to_string().contains("number"),
        "{RATE} is a number: {prop}"
    );
    let required: Vec<&str> = d["required"]
        .as_array()
        .expect("DelayerDto.required")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        required.contains(&RATE),
        "{RATE} must be required: {required:?}"
    );
}
