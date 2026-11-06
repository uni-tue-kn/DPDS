//! The running configuration (`GET /api/emulation`) and `POST /api/emulation/validate`.
//!
//! `GET /api/emulation` reports, while the emulation is starting, running, stopping or failed,
//! the configuration it was started with in `config`: `{links: [{id, name, ingress, egress,
//! config}]}`. It is the
//! start body as accepted (PCI addresses in lower case, the order kept, optional stages left
//! out as they were sent) with an id per link; `directions` and `main_lcore` are the planned
//! lcores. While stopped, `config` is null or absent, `directions` is `[]` and `main_lcore`
//! null or absent. `pending_restart` does not exist: the frontend compares its own
//! configuration with `config`.
//!
//! `POST /api/emulation/validate` takes the same body and answers what start would answer
//! without starting anything: 200 `{main_lcore, directions: [{dir, link, rx, tx, lcore}]}`, or
//! the same 400/409 `{"message"}`. It does not look at the state of the emulation.

use crate::common::*;
use dpds_controller::supervisor::State;
use serde_json::{json, Value};

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

fn one(name: &str, ingress: &str, egress: &str) -> Value {
    link(name, ingress, egress)
}

fn config_of(v: &Value) -> &Value {
    &v["config"]
}

/// Nulls out, and `delay_stats: true` (the default a DTO may write back) dropped, so a config
/// compares equal whether or not the controller spells out its defaults.
fn normal(v: &Value) -> Value {
    let mut v = strip_nulls(v);
    if let Some(c) = v.get_mut("config").and_then(Value::as_object_mut) {
        if c.get("delay_stats") == Some(&json!(true)) {
            c.remove("delay_stats");
        }
    }
    v
}

// ---- the running configuration -----------------------------------------------------------

#[tokio::test]
async fn a_stopped_controller_reports_no_configuration() {
    let a = app();
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "stopped");
    assert!(absent(&v, "config"), "{v}");
    assert_eq!(v["directions"], json!([]));
    assert!(absent(&v, "main_lcore"), "{v}");
}

#[tokio::test]
async fn the_start_response_already_carries_the_configuration() {
    let a = app();
    let v = a.start_ok(vec![one("a-to-b", NIC0, NIC1)]).await;
    assert_eq!(config_of(&v)["links"][0]["name"], "a-to-b", "{v}");
    assert_eq!(v["directions"].as_array().unwrap().len(), 1, "{v}");
}

#[tokio::test]
async fn the_running_configuration_is_the_start_body_as_accepted() {
    let a = app();
    let mut l0 = link("a-to-b", "0000:17:00.0", "0000:65:00.0");
    l0["config"] = cfg_full(2.0e6, 1.0e5, 4.0e9);
    l0["config"]["seed"] = json!(9);
    l0["config"]["delay_stats"] = json!(false);
    l0["config"]["histogram"] = json!({"bins": 64});
    let l1 = link_cfg("loop", "net_null0", "net_null0", cfg_bare());
    let s = setup(vec![l0.clone(), l1.clone()]);
    let r = a.start(&s).await;
    assert_eq!(r.status, 202, "{}", r.text());
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "running");
    let cfg = strip_nulls(config_of(&v));
    // the echo is `{links}` only
    assert_eq!(
        cfg.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["links"],
        "{v}"
    );
    // links: same order, same fields, plus an id; null/absent stages stay out
    let links = cfg["links"].as_array().unwrap();
    assert_eq!(links.len(), 2, "{v}");
    for (got, sent) in links.iter().zip([&l0, &l1]) {
        let mut got = got.clone();
        let id = got.as_object_mut().unwrap().remove("id");
        assert!(
            id.is_some_and(|i| i.is_u64()),
            "every link has a numeric id: {v}"
        );
        assert_eq!(normal(&got), normal(sent), "{v}");
    }
    assert_ne!(links[0]["id"], links[1]["id"], "ids are unique: {v}");
    // the stages of the second link were never added and are not invented
    assert!(absent(&links[1]["config"], "delayer"), "{v}");
    assert!(absent(&links[1]["config"], "limiter"), "{v}");
    assert!(absent(&links[1]["config"], "loss_model"), "{v}");
}

#[tokio::test]
async fn pci_ports_are_reported_in_lower_case() {
    let a = app();
    a.start_ok(vec![one("up", "0000:B3:00.0", "0000:B3:00.0")])
        .await;
    let v = a.get("/api/emulation").await.json();
    assert_eq!(config_of(&v)["links"][0]["ingress"], INTEL, "{v}");
    assert_eq!(config_of(&v)["links"][0]["egress"], INTEL, "{v}");
}

#[tokio::test]
async fn directions_and_main_lcore_are_the_planned_lcores_of_the_running_config() {
    let a = app();
    let s = setup(vec![one("a-to-b", NIC0, NIC1), one("b-to-a", NIC1, NIC0)]);
    assert_eq!(a.start(&s).await.status, 202);
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["main_lcore"], 0);
    let d = v["directions"].as_array().unwrap();
    assert_eq!(d.len(), 2);
    for (i, (link, rx, tx)) in [("a-to-b", NIC0, NIC1), ("b-to-a", NIC1, NIC0)]
        .iter()
        .enumerate()
    {
        assert_eq!(
            (
                d[i]["dir"].clone(),
                d[i]["link"].clone(),
                d[i]["rx"].clone(),
                d[i]["tx"].clone()
            ),
            (json!(i), json!(link), json!(rx), json!(tx)),
            "{d:?}"
        );
    }
    // the worker prefers the NUMA node of the INGRESS NIC (NIC0: node 0, NIC1: node 1)
    assert!([2, 3].contains(&(d[0]["lcore"].as_u64().unwrap() as u32)));
    assert!([6, 7].contains(&(d[1]["lcore"].as_u64().unwrap() as u32)));
    let plan = &a.supervisor.plans()[0];
    assert_eq!(
        plan.directions[0].lcore as u64,
        d[0]["lcore"].as_u64().unwrap()
    );
}

#[tokio::test]
async fn there_is_no_pending_restart_any_more() {
    let a = app();
    assert!(a
        .get("/api/emulation")
        .await
        .json()
        .get("pending_restart")
        .is_none());
    a.start_ok(vec![one("l", "net_null0", "net_null0")]).await;
    assert!(a
        .get("/api/emulation")
        .await
        .json()
        .get("pending_restart")
        .is_none());
    // a refused start while running changes nothing the controller reports
    let before = a.get("/api/emulation").await.json();
    let r = a
        .start_links(vec![one("other", "net_null1", "net_null1")])
        .await;
    assert_eq!(r.status, 409);
    assert_eq!(a.get("/api/emulation").await.json(), before);
}

#[tokio::test]
async fn the_configuration_is_reported_in_every_state_but_stopped() {
    for (state, reported) in [
        (State::Starting, true),
        (State::Running, true),
        (State::Stopping, true),
        (State::Failed, true),
        (State::Stopped, false),
    ] {
        let a = app();
        a.start_ok(vec![one("l", "net_null0", "net_null0")]).await;
        a.supervisor.force_status(status(state));
        let v = a.get("/api/emulation").await.json();
        assert_eq!(
            !absent(&v, "config"),
            reported,
            "{state:?}: config {}",
            v["config"]
        );
        if reported {
            assert_eq!(config_of(&v)["links"][0]["name"], "l", "{state:?}");
            assert_eq!(v["directions"].as_array().unwrap().len(), 1, "{state:?}");
        } else {
            assert_eq!(v["directions"], json!([]), "{state:?}");
        }
    }
}

#[tokio::test]
async fn a_failed_start_still_reports_what_it_tried_to_run() {
    let a = app();
    a.supervisor
        .fail_next_start("dpds exited with status 1", &["EAL: Error"]);
    a.start_ok(vec![one("l", "net_null0", "net_null0")]).await;
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "failed");
    assert_eq!(config_of(&v)["links"][0]["name"], "l", "{v}");
    // a retry with another body replaces it
    a.start_ok(vec![one("other", "net_null1", "net_null1")])
        .await;
    let v = a.get("/api/emulation").await.json();
    assert_eq!(config_of(&v)["links"][0]["name"], "other", "{v}");
    assert_eq!(config_of(&v)["links"].as_array().unwrap().len(), 1, "{v}");
}

#[tokio::test]
async fn stop_clears_the_configuration() {
    let a = app();
    a.start_ok(vec![one("l", "net_null0", "net_null0")]).await;
    assert_eq!(a.post("/api/emulation/stop", json!({})).await.status, 202);
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "stopped");
    assert!(absent(&v, "config"), "{v}");
}

#[tokio::test]
async fn the_second_start_while_running_changes_nothing() {
    let a = app();
    a.start_ok(vec![one("first", NIC0, NIC1)]).await;
    let before = a.get("/api/emulation").await.json();
    for links in [
        vec![one("first", NIC0, NIC1)],
        vec![one("second", INTEL, INTEL)],
    ] {
        let r = a.start_links(links).await;
        assert_eq!(r.status, 409, "{}", r.text());
        r.message();
    }
    assert_eq!(a.get("/api/emulation").await.json(), before);
    assert_eq!(a.supervisor.plans().len(), 1);
}

#[tokio::test]
async fn the_histogram_expects_the_running_configuration() {
    // the only configuration the controller has is the one it was started with: a link without a
    // delayer expects zero delay, one with a Normal delay expects its distribution
    let a = app();
    a.start_ok(vec![
        link_cfg("a-to-b", NIC0, NIC1, cfg_bare()),
        link_cfg("b-to-a", NIC1, NIC0, cfg_json(1.0e6, 1.0e5)),
    ])
    .await;
    let s = snapshot(
        [
            DirSample {
                uptime_ns: 1,
                rx_pkts: 0,
                tx_sent: 0,
                loss: 0,
                limiter: 0,
                full: 0,
                tx_drops: 0,
            },
            DirSample {
                uptime_ns: 1,
                rx_pkts: 0,
                tx_sent: 0,
                loss: 0,
                limiter: 0,
                full: 0,
                tx_drops: 0,
            },
        ],
        [
            PortSample {
                ibytes: 0,
                obytes: 0,
                rx_missed_errors: 0,
                rx_mbuf_allocation_errors: 0,
            },
            PortSample {
                ibytes: 0,
                obytes: 0,
                rx_missed_errors: 0,
                rx_mbuf_allocation_errors: 0,
            },
        ],
    );
    a.telemetry.set(None, s);
    let r = a.get("/api/stats/0/delay_histogram").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json().get("expected").is_some(), "{}", r.text());
}

// ---- POST /api/emulation/validate ------------------------------------------------------------

#[tokio::test]
async fn validate_answers_the_plan_without_starting_anything() {
    let a = app();
    let s = setup(vec![one("a-to-b", NIC0, NIC1), one("b-to-a", NIC1, NIC0)]);
    let r = a.validate(&s).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let v = r.json();
    assert_eq!(v["main_lcore"], 0, "{v}");
    let d = v["directions"].as_array().unwrap();
    assert_eq!(d.len(), 2, "{v}");
    assert_eq!(
        (d[0]["dir"].clone(), d[0]["link"].clone()),
        (json!(0), json!("a-to-b"))
    );
    assert_eq!(
        (d[1]["dir"].clone(), d[1]["link"].clone()),
        (json!(1), json!("b-to-a"))
    );
    assert_eq!(
        (d[0]["rx"].clone(), d[0]["tx"].clone()),
        (json!(NIC0), json!(NIC1))
    );
    assert!(
        [2, 3].contains(&(d[0]["lcore"].as_u64().unwrap() as u32)),
        "{v}"
    );
    assert!(
        [6, 7].contains(&(d[1]["lcore"].as_u64().unwrap() as u32)),
        "{v}"
    );
    // nothing started, nothing stored, nothing reported
    assert!(a.supervisor.plans().is_empty());
    let e = a.get("/api/emulation").await.json();
    assert_eq!(e["state"], "stopped", "{e}");
    assert!(absent(&e, "config"), "{e}");
}

#[tokio::test]
async fn validate_plans_what_start_launches() {
    let a = app();
    let s = setup(vec![one("a-to-b", NIC0, NIC1), one("b-to-a", NIC1, NIC0)]);
    let planned = parse_plan(&a.validate(&s).await.json());
    assert_eq!(a.start(&s).await.status, 202);
    let plan = &a.supervisor.plans()[0];
    assert_eq!(planned.main_lcore, plan.main_lcore);
    let launched: Vec<(u32, u32)> = plan.directions.iter().map(|d| (d.dir, d.lcore)).collect();
    let validated: Vec<(u32, u32)> = planned
        .directions
        .iter()
        .map(|d| (d.dir, d.lcore))
        .collect();
    assert_eq!(validated, launched);
}

#[tokio::test]
async fn validate_gives_the_same_errors_as_start() {
    let a = app();
    let mut bad_cfg = one("c", "net_null0", "net_null1");
    bad_cfg["config"]["delayer"]["std_ns"] = json!(-1.0);
    let mut no_rate = one("r", "net_null0", "net_null1");
    no_rate["config"]["delayer"]
        .as_object_mut()
        .unwrap()
        .remove("initialization_rate_pps");
    let cases: Vec<(&str, Setup, u16, &str)> = vec![
        (
            "bad name",
            setup(vec![one("a b", "net_null0", "net_null0")]),
            400,
            "link name",
        ),
        (
            "bad port",
            setup(vec![one("x", "eth0", "net_null0")]),
            400,
            "port",
        ),
        ("bad config", setup(vec![bad_cfg]), 400, "std_ns"),
        (
            "no rate",
            setup(vec![no_rate]),
            400,
            "initialization_rate_pps",
        ),
        (
            "port reuse",
            setup(vec![one("first", NIC0, NIC1), one("second", NIC0, INTEL)]),
            409,
            "first",
        ),
        (
            "duplicate name",
            setup(vec![one("a", NIC0, NIC1), one("a", INTEL, INTEL)]),
            409,
            "a",
        ),
        (
            "management nic",
            setup(vec![one("m", MGMT, NIC1)]),
            409,
            MGMT,
        ),
        ("nothing", setup(vec![]), 400, "nothing to start"),
    ];
    for (what, s, status, named) in cases {
        let v = a.validate(&s).await;
        let st = a.start(&s).await;
        assert_eq!(v.status, status, "validate {what}: {}", v.text());
        assert_eq!(st.status, status, "start {what}: {}", st.text());
        assert!(v.message().contains(named), "validate {what}: {}", v.text());
        assert!(st.message().contains(named), "start {what}: {}", st.text());
    }
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn validate_does_not_care_about_the_state_of_the_emulation() {
    // a frontend validates its configuration while another one runs
    let a = app();
    a.start_ok(vec![one("running", NIC0, NIC1)]).await;
    let r = a.validate(&setup(vec![one("next", INTEL, INTEL)])).await;
    assert_eq!(r.status, 200, "{}", r.text());
    for st in [State::Starting, State::Stopping, State::Failed] {
        a.supervisor.force_status(status(st));
        let r = a.validate(&setup(vec![one("next", INTEL, INTEL)])).await;
        assert_eq!(r.status, 200, "{st:?}: {}", r.text());
    }
    // ... and it neither started nor changed anything
    assert_eq!(a.supervisor.plans().len(), 1);
}

#[tokio::test]
async fn validate_needs_a_json_content_type_and_a_known_body() {
    let a = app();
    let body = setup(vec![one("l", "net_null0", "net_null0")]).body();
    let r = a
        .request_raw(
            "POST",
            "/api/emulation/validate",
            Some("text/plain"),
            body.to_string().into_bytes(),
        )
        .await;
    assert_eq!(r.status, 415, "{}", r.text());
    let mut unknown = body;
    unknown["links"][0]["id"] = json!(3);
    let r = a.post("/api/emulation/validate", unknown).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("id"), "{}", r.text());
    // GET is not allowed
    let r = a.get("/api/emulation/validate").await;
    assert_eq!(r.status, 405, "{}", r.text());
}
