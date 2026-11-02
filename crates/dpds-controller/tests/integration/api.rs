//! REST API in-process (fake supervisor, fake telemetry, fake sysfs).
//! See the header of `common/mod.rs` for the HTTP contract assumed.

use std::time::Duration;

use crate::common::*;
use dpds_controller::dto::ConfigDto;
use dpds_controller::eal::{drain_timeout, eal_argv, links_file_json, EalInput, EalPort};
use dpds_controller::store;
use dpds_controller::supervisor::State;
use serde_json::{json, Value};

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

async fn start_ok(a: &TestApp) -> Value {
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 202, "{}", r.text());
    r.json()
}

// ---- online / interfaces / hints ----------------------------------------

#[tokio::test]
async fn online_reports_version_binary_telemetry_and_root() {
    let a = app();
    let r = a.get("/api/online").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["dpds_bin"], a.dpds_bin.to_str().unwrap());
    assert_eq!(v["telemetry"], true);
    assert_eq!(v["root"], false);
}

#[tokio::test]
async fn interfaces_lists_the_discovered_nics() {
    let a = app();
    let r = a.get("/api/interfaces").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    let pcis: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["pci"].as_str().unwrap())
        .collect();
    assert_eq!(pcis, [MGMT, NIC0, NIC1, INTEL]);
    assert_eq!(v[1]["dpds_usable"], true);
}

#[tokio::test]
async fn host_hints_are_the_shared_hint_array() {
    let a = app();
    let r = a.get("/api/host/hints").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    for id in HINT_IDS {
        let _ = hint(&v, id);
    }
    let direct = hints_json(&a.host, &Default::default());
    assert_eq!(
        v, direct,
        "the API serves compute_hints() for the stored state"
    );
}

// ---- links ---------------------------------------------------------------

#[tokio::test]
async fn links_start_empty() {
    let r = app().get("/api/links").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), json!([]));
}

#[tokio::test]
async fn create_get_list_update_delete_link() {
    let a = app();
    let r = a.create_link("a-b", &[NIC0, NIC1]).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let created = r.json();
    let id = created["id"].as_u64().expect("numeric id");
    assert_eq!(created["name"], "a-b");
    assert_eq!(created["ports"], json!([NIC0, NIC1]));
    assert_eq!(created["configs"].as_array().unwrap().len(), 2);
    assert_eq!(created["configs"][0]["delayer"]["mean_ns"], 1.0e6);
    assert!(absent(&created, "workers"));

    let r = a.get(&format!("/api/links/{id}")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), created);
    assert_eq!(a.get("/api/links").await.json(), json!([created]));

    let mut upd = link_body("a-b2", &[NIC0, NIC1]);
    upd["workers"] = json!([2, 6]);
    upd["configs"][1] = cfg_json(3.0e6, 0.0);
    let r = a.put(&format!("/api/links/{id}"), upd).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let u = r.json();
    assert_eq!(u["id"], id);
    assert_eq!(u["name"], "a-b2");
    assert_eq!(u["workers"], json!([2, 6]));
    assert_eq!(u["configs"][1]["delayer"]["mean_ns"], 3.0e6);
    assert_eq!(a.get(&format!("/api/links/{id}")).await.json(), u);

    let r = a.delete(&format!("/api/links/{id}")).await;
    assert_eq!(r.status, 204);
    let r = a.get(&format!("/api/links/{id}")).await;
    assert_eq!(r.status, 404);
    r.message();
    assert_eq!(a.get("/api/links").await.json(), json!([]));
}

#[tokio::test]
async fn unknown_link_id_is_404_with_message_for_get_put_delete() {
    let a = app();
    let r = a.get("/api/links/99").await;
    assert_eq!(r.status, 404);
    r.message();
    let r = a.put("/api/links/99", link_body("x", &["net_null0"])).await;
    assert_eq!(r.status, 404);
    r.message();
    let r = a.delete("/api/links/99").await;
    assert_eq!(r.status, 404);
    r.message();
}

#[tokio::test]
async fn link_ids_are_not_reused() {
    let a = app();
    let first = a.create_link("one", &["net_null0"]).await.json()["id"]
        .as_u64()
        .unwrap();
    a.delete(&format!("/api/links/{first}")).await;
    let second = a.create_link("one", &["net_null0"]).await.json()["id"]
        .as_u64()
        .unwrap();
    assert_ne!(first, second);
}

#[tokio::test]
async fn link_name_validation() {
    let a = app();
    let ok32 = "a".repeat(32);
    for (i, name) in [ok32.as_str(), "a_b.c-D9"].iter().enumerate() {
        let r = a.create_link(name, &[&format!("net_null{i}")]).await;
        assert_eq!(r.status, 201, "{name}: {}", r.text());
    }
    for bad in ["", &"a".repeat(33), "a b", "a/b", "ä", "a,b"] {
        let r = a.create_link(bad, &["net_null9"]).await;
        assert_eq!(r.status, 400, "name {bad:?}");
        r.message();
    }
}

#[tokio::test]
async fn link_structure_validation() {
    let a = app();
    let cases = [
        ("no ports", json!({"name":"x","ports":[],"configs":[]})),
        (
            "three ports",
            json!({"name":"x","ports":["net_null0","net_null1","net_null2"],
            "configs":[cfg_json(1e6,0.0),cfg_json(1e6,0.0),cfg_json(1e6,0.0)]}),
        ),
        (
            "configs != ports",
            json!({"name":"x","ports":["net_null0","net_null1"],"configs":[cfg_json(1e6,0.0)]}),
        ),
        (
            "workers != directions",
            json!({"name":"x","ports":["net_null0"],"configs":[cfg_json(1e6,0.0)],"workers":[2,3]}),
        ),
        (
            "empty port name",
            json!({"name":"x","ports":[""],"configs":[cfg_json(1e6,0.0)]}),
        ),
        (
            "invalid config",
            json!({"name":"x","ports":["net_null0"],"configs":[cfg_json(1e6,-1.0)]}),
        ),
    ];
    for (what, body) in cases {
        let r = a.post("/api/links", body).await;
        assert_eq!(r.status, 400, "{what}: {}", r.text());
        let m = r.message();
        if what == "invalid config" {
            assert!(m.contains("std_ns"), "{m}");
        }
    }
    assert_eq!(
        a.get("/api/links").await.json(),
        json!([]),
        "rejected links must not be stored"
    );
}

#[tokio::test]
async fn malformed_bodies_get_a_json_message() {
    let a = app();
    let r = a.request("POST", "/api/links", None).await; // empty body, no content type
    assert_eq!(r.status, 415, "{}", r.status);
    r.message();
    let r = a.post("/api/links", json!({"name": "x"})).await; // missing fields
    assert!(r.status == 400 || r.status == 422, "{}", r.status);
    r.message();
    let mut body = link_body("x", &["net_null0"]);
    body["configs"][0]["delayer"]["distribution"] = json!("Bogus");
    let r = a.post("/api/links", body).await;
    assert!(r.status == 400 || r.status == 422, "{}", r.status);
    r.message();
}

#[tokio::test]
async fn conflicts_duplicate_name_and_port_in_use() {
    let a = app();
    assert_eq!(a.create_link("a", &[NIC0]).await.status, 201);
    let r = a.create_link("a", &[NIC1]).await;
    assert_eq!(r.status, 409);
    r.message();
    let r = a.create_link("b", &[NIC0]).await;
    assert_eq!(r.status, 409);
    assert!(r.message().contains(NIC0));
    // updating a link keeps its own name and ports
    let id = a.get("/api/links").await.json()[0]["id"].as_u64().unwrap();
    assert_eq!(
        a.put(&format!("/api/links/{id}"), link_body("a", &[NIC0]))
            .await
            .status,
        200
    );
    // renaming onto another link's name conflicts
    let other = a.create_link("c", &[NIC1]).await.json()["id"]
        .as_u64()
        .unwrap();
    let r = a
        .put(&format!("/api/links/{other}"), link_body("a", &[NIC1]))
        .await;
    assert_eq!(r.status, 409);
}

#[tokio::test]
async fn links_are_persisted_and_survive_a_controller_restart() {
    let mut a = app();
    a.create_link("a-b", &[NIC0, NIC1]).await;
    let stored = store::load(a.state_dir.path()).unwrap();
    assert_eq!(stored.links.len(), 1);
    assert_eq!(stored.links[0].name, "a-b");
    let before = a.get("/api/links").await.json();
    a.restart();
    assert_eq!(a.get("/api/links").await.json(), before);
    // ids keep counting after a restart
    let id2 = a.create_link("c", &["net_null0"]).await.json()["id"]
        .as_u64()
        .unwrap();
    assert!(id2 > before[0]["id"].as_u64().unwrap());
}

#[tokio::test]
async fn router_refuses_a_corrupt_state_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(store::STATE_FILE_NAME), "{garbage").unwrap();
    let host = FakeHost::bare_metal();
    let r = dpds_controller::api::router(dpds_controller::api::Deps {
        state_dir: tmp.path().to_path_buf(),
        roots: host.roots(),
        supervisor: FakeSupervisor::new(),
        telemetry: FakeTelemetry::new(),
        dpds_bin: "/x".into(),
        runtime_dir: "/y".into(),
        dpds_has_telemetry: false,
        is_root: false,
    });
    assert!(r.is_err());
}

// ---- settings ------------------------------------------------------------

#[tokio::test]
async fn settings_defaults_put_and_persistence() {
    let a = app();
    let r = a.get("/api/settings").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    assert!(absent(&v, "main_lcore"));
    assert_eq!(v["mlx5_devargs"], "txq_mpw_en=1");
    assert_eq!(v["eal_extra_args"], json!([]));
    assert_eq!(v["poll_interval_ms"], 500);

    let new = json!({"main_lcore": 1, "mlx5_devargs": "txq_mpw_en=1,rxq_cqe_comp_en=0",
                     "eal_extra_args": ["--no-huge","-m","256"], "poll_interval_ms": 250});
    let r = a.put("/api/settings", new.clone()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(strip_nulls(&r.json()), new);
    assert_eq!(a.get("/api/settings").await.json(), r.json());
    assert_eq!(
        store::load(a.state_dir.path())
            .unwrap()
            .settings
            .poll_interval_ms,
        250
    );
}

#[tokio::test]
async fn settings_validation() {
    let a = app();
    let base = json!({"main_lcore": null, "mlx5_devargs": "txq_mpw_en=1", "eal_extra_args": [], "poll_interval_ms": 500});
    let mut zero = base.clone();
    zero["poll_interval_ms"] = json!(0);
    let r = a.put("/api/settings", zero).await;
    assert_eq!(r.status, 400);
    r.message();
    let mut mprq = base.clone();
    mprq["mlx5_devargs"] = json!("txq_mpw_en=1,mprq_en=1");
    let r = a.put("/api/settings", mprq).await;
    assert_eq!(r.status, 400);
    assert!(r.message().to_lowercase().contains("mprq"));
    assert_eq!(
        a.get("/api/settings").await.json()["mlx5_devargs"],
        "txq_mpw_en=1"
    );
}

// ---- emulation -----------------------------------------------------------

#[tokio::test]
async fn emulation_when_stopped_and_empty() {
    let a = app();
    let r = a.get("/api/emulation").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["state"], "stopped");
    assert!(absent(&v, "pid"));
    assert!(absent(&v, "started_at"));
    assert!(absent(&v, "error"));
    assert_eq!(v["directions"], json!([]));
    assert_eq!(v["pending_restart"], false);
    assert_eq!(v["log_tail"], json!([]));
}

#[tokio::test]
async fn emulation_when_stopped_shows_the_planned_lcores() {
    let a = app();
    a.create_link("a-b", &[NIC0, NIC1]).await;
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "stopped");
    assert_eq!(v["main_lcore"], 0);
    let d = v["directions"].as_array().unwrap();
    assert_eq!(d.len(), 2);
    assert_eq!(
        (
            d[0]["dir"].clone(),
            d[0]["link"].clone(),
            d[0]["rx"].clone(),
            d[0]["tx"].clone()
        ),
        (json!(0), json!("a-b"), json!(NIC0), json!(NIC1))
    );
    assert!([2, 3].contains(&(d[0]["lcore"].as_u64().unwrap() as u32)));
    assert!([6, 7].contains(&(d[1]["lcore"].as_u64().unwrap() as u32)));
}

#[tokio::test]
async fn start_without_links_is_a_400() {
    let a = app();
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 400);
    r.message();
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn start_hands_the_supervisor_a_complete_launch_plan() {
    let a = app();
    let mut b = link_body("a-b", &[NIC0, NIC1]);
    b["configs"][1] = cfg_json(2.0e6, 1.0e5);
    assert_eq!(a.post("/api/links", b).await.status, 201);
    assert_eq!(a.create_link("loop", &["net_null0"]).await.status, 201);

    let v = start_ok(&a).await;
    assert!(
        ["starting", "running"].contains(&v["state"].as_str().unwrap()),
        "{v}"
    );
    let plans = a.supervisor.plans();
    assert_eq!(plans.len(), 1);
    let plan = &plans[0];

    assert_eq!(plan.dpds_bin, a.dpds_bin);
    assert_eq!(plan.runtime_dir, a.runtime_dir);
    assert_eq!(plan.links_file, a.runtime_dir.join("links.json"));
    assert_eq!(plan.main_lcore, 0);

    let stored = store::load(a.state_dir.path()).unwrap();
    assert_eq!(
        plan.links_json,
        links_file_json(&stored.links, &plan.directions).unwrap()
    );

    assert_eq!(plan.directions.len(), 3);
    let mut distinct: Vec<u32> = plan.directions.iter().map(|d| d.lcore).collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 3, "worker lcores must be distinct");
    assert!(
        [2, 3].contains(&plan.directions[0].lcore),
        "{:?}",
        plan.directions
    );
    assert!(
        [6, 7].contains(&plan.directions[1].lcore),
        "{:?}",
        plan.directions
    );
    // the generated links file carries exactly these lcores
    assert_eq!(
        plan.links_json["links"][0]["lcores"],
        json!([plan.directions[0].lcore, plan.directions[1].lcore])
    );
    assert_eq!(
        plan.links_json["links"][1]["lcores"],
        json!([plan.directions[2].lcore])
    );
    for d in &plan.directions {
        assert!([2, 3, 6, 7].contains(&d.lcore));
    }
    assert_eq!(plan.directions[2].link, "loop");

    let mut workers: Vec<u32> = plan.directions.iter().map(|d| d.lcore).collect();
    workers.sort_unstable();
    let expect = eal_argv(&EalInput {
        main_lcore: 0,
        worker_lcores: workers,
        ports: vec![
            EalPort {
                name: NIC0.into(),
                driver: Some("mlx5_core".into()),
            },
            EalPort {
                name: NIC1.into(),
                driver: Some("mlx5_core".into()),
            },
            EalPort {
                name: "net_null0".into(),
                driver: None,
            },
        ],
        mlx5_devargs: "txq_mpw_en=1".into(),
        eal_extra_args: vec![],
        links_file: a.runtime_dir.join("links.json"),
    });
    // `-l` is a set: the order of its members is not significant, everything else is exact
    let lset = |v: &[String]| {
        let mut x: Vec<u32> = v[1].split(',').map(|n| n.parse().unwrap()).collect();
        x.sort_unstable();
        x
    };
    assert_eq!(plan.argv[0], "-l");
    assert_eq!(lset(&plan.argv), lset(&expect));
    assert_eq!(plan.argv[2..], expect[2..]);
    assert!(plan.argv.contains(&format!("{NIC0},txq_mpw_en=1")));
    assert!(plan.argv.windows(2).any(|w| w == ["--vdev", "net_null0"]));

    let cores: Vec<dpds_core::Config> = stored
        .links
        .iter()
        .flat_map(|l| l.configs.iter().map(|c: &ConfigDto| c.to_core().unwrap()))
        .collect();
    assert_eq!(plan.drain_timeout, drain_timeout(&cores));
    assert!(plan.drain_timeout >= Duration::from_secs(5));
}

#[tokio::test]
async fn settings_extra_args_and_devargs_reach_the_argv() {
    let a = app();
    a.put(
        "/api/settings",
        json!({"main_lcore": 1, "mlx5_devargs": "txq_mpw_en=1,rxq_cqe_comp_en=0",
        "eal_extra_args": ["--no-huge", "-m", "256"], "poll_interval_ms": 500}),
    )
    .await;
    a.create_link("l", &[NIC0]).await;
    start_ok(&a).await;
    let plan = &a.supervisor.plans()[0];
    assert_eq!(plan.main_lcore, 1);
    assert!(plan
        .argv
        .contains(&format!("{NIC0},txq_mpw_en=1,rxq_cqe_comp_en=0")));
    assert!(plan
        .argv
        .windows(3)
        .any(|w| w == ["--no-huge", "-m", "256"]));
    assert!(!plan.argv.join(" ").contains("mprq"));
}

#[tokio::test]
async fn start_stop_state_transitions() {
    let a = app();
    a.create_link("loop", &["net_null0"]).await;
    start_ok(&a).await;

    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "running");
    assert_eq!(v["pid"], 4242);
    assert!(v["started_at"].is_u64());
    assert_eq!(v["directions"].as_array().unwrap().len(), 1);
    assert_eq!(v["main_lcore"], 0);
    assert!(!v["log_tail"].as_array().unwrap().is_empty());
    assert_eq!(v["pending_restart"], false);

    // can't start twice
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 409);
    r.message();
    assert_eq!(a.supervisor.plans().len(), 1);

    let r = a.post("/api/emulation/stop", json!({})).await;
    assert_eq!(r.status, 202);
    assert!(["stopping", "stopped"].contains(&r.json()["state"].as_str().unwrap()));
    assert_eq!(a.supervisor.stops(), 1);
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");

    let r = a.post("/api/emulation/stop", json!({})).await;
    assert_eq!(r.status, 409);
    r.message();
    assert_eq!(a.supervisor.stops(), 1);
}

#[tokio::test]
async fn start_is_refused_while_starting_or_stopping() {
    let a = app();
    a.create_link("loop", &["net_null0"]).await;
    for s in [State::Starting, State::Stopping] {
        a.supervisor.force_status(status(s));
        let r = a.post("/api/emulation/start", json!({})).await;
        assert_eq!(r.status, 409, "{s:?}");
    }
    assert!(a.supervisor.plans().is_empty());
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "stopping");
}

#[tokio::test]
async fn failed_start_is_reported_with_error_and_log_tail_and_can_be_retried() {
    let a = app();
    a.create_link("loop", &["net_null0"]).await;
    a.supervisor.fail_next_start(
        "dpds exited with status 1",
        &["EAL: Error", "dpds: cannot create mbuf pool"],
    );
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 202);

    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "failed");
    assert_eq!(v["error"], "dpds exited with status 1");
    assert_eq!(
        v["log_tail"],
        json!(["EAL: Error", "dpds: cannot create mbuf pool"])
    );
    assert!(absent(&v, "pid"));

    // retry from failed
    start_ok(&a).await;
    assert_eq!(a.get("/api/emulation").await.json()["state"], "running");
    assert_eq!(a.supervisor.plans().len(), 2);
}

#[tokio::test]
async fn start_with_not_enough_isolated_cores_is_a_400_and_does_not_spawn() {
    let a = app();
    for i in 0..5 {
        a.create_link(&format!("l{i}"), &[&format!("net_null{i}")])
            .await;
    }
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 400);
    assert!(r.message().contains("isolated"));
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn edits_while_running_are_stored_and_flag_pending_restart() {
    let a = app();
    let id = a.create_link("l", &["net_null0"]).await.json()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(
        a.get("/api/emulation").await.json()["pending_restart"],
        false
    );
    // edit while stopped: no flag
    a.put(&format!("/api/links/{id}"), link_body("l2", &["net_null0"]))
        .await;
    assert_eq!(
        a.get("/api/emulation").await.json()["pending_restart"],
        false
    );

    start_ok(&a).await;
    assert_eq!(
        a.get("/api/emulation").await.json()["pending_restart"],
        false
    );

    let mut edited = link_body("l3", &["net_null0"]);
    edited["configs"][0] = cfg_json(9.0e6, 0.0);
    let r = a.put(&format!("/api/links/{id}"), edited).await;
    assert_eq!(r.status, 200);
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "running");
    assert_eq!(v["pending_restart"], true);
    assert_eq!(
        a.get(&format!("/api/links/{id}")).await.json()["name"],
        "l3"
    );
    // the running plan was not touched
    assert_eq!(a.supervisor.plans().len(), 1);

    // restart picks the edit up and clears the flag
    a.post("/api/emulation/stop", json!({})).await;
    start_ok(&a).await;
    assert_eq!(
        a.get("/api/emulation").await.json()["pending_restart"],
        false
    );
    assert_eq!(a.supervisor.plans()[1].links_json["links"][0]["name"], "l3");
}

#[tokio::test]
async fn every_kind_of_edit_while_running_flags_pending_restart() {
    for what in ["create", "delete", "settings"] {
        let a = app();
        let id = a.create_link("l", &["net_null0"]).await.json()["id"]
            .as_u64()
            .unwrap();
        start_ok(&a).await;
        match what {
            "create" => assert_eq!(a.create_link("m", &["net_null1"]).await.status, 201),
            "delete" => assert_eq!(a.delete(&format!("/api/links/{id}")).await.status, 204),
            _ => {
                let r = a
                    .put(
                        "/api/settings",
                        json!({"main_lcore": 1, "mlx5_devargs": "txq_mpw_en=1",
                    "eal_extra_args": [], "poll_interval_ms": 500}),
                    )
                    .await;
                assert_eq!(r.status, 200);
            }
        }
        assert_eq!(
            a.get("/api/emulation").await.json()["pending_restart"],
            true,
            "{what}"
        );
    }
}

// ---- stats ---------------------------------------------------------------

#[tokio::test]
async fn stats_without_telemetry_data_are_empty() {
    let a = app();
    let r = a.get("/api/stats").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["directions"], json!([]));
    assert_eq!(v["ports"], json!([]));
    // no data is stale data (C4); there is no age without a poll
    assert_eq!(v["stale"], true);
    assert!(absent(&v, "age_ms"), "{v}");
    let r = a.get("/api/stats/0/delay_histogram").await;
    assert_eq!(r.status, 404);
    r.message();
}

#[tokio::test]
async fn stats_serve_the_computed_view_of_the_cached_snapshots() {
    let a = app();
    let mk = |t: u64, rx: u64| {
        snapshot(
            [
                DirSample {
                    uptime_ns: t,
                    rx_pkts: rx,
                    tx_sent: rx,
                    loss: 0,
                    limiter: 0,
                    full: 0,
                    tx_drops: 0,
                },
                DirSample {
                    uptime_ns: t,
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
                    ibytes: rx * 100,
                    obytes: 0,
                    imissed: 0,
                    rx_nombuf: 0,
                },
                PortSample {
                    ibytes: 0,
                    obytes: rx * 100,
                    imissed: 0,
                    rx_nombuf: 0,
                },
            ],
        )
    };
    a.telemetry
        .set(Some(mk(1_000_000_000, 1_000)), mk(2_000_000_000, 5_000));
    let r = a.get("/api/stats").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["directions"].as_array().unwrap().len(), 2);
    assert_close(f(&v["directions"][0], "rx_pps"), 4000.0);
    assert_close(f(&v["directions"][0], "rx_bps"), 4000.0 * 100.0 * 8.0);
    assert_eq!(v["ports"].as_array().unwrap().len(), 2);
    assert_eq!(v["directions"][0]["link"], "a-b");
}

#[tokio::test]
async fn delay_histogram_endpoint() {
    let a = app();
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
                imissed: 0,
                rx_nombuf: 0,
            },
            PortSample {
                ibytes: 0,
                obytes: 0,
                imissed: 0,
                rx_nombuf: 0,
            },
        ],
    );
    a.telemetry.set(None, s);
    let r = a.get("/api/stats/1/delay_histogram").await;
    assert_eq!(r.status, 200);
    let h = r.json();
    assert_eq!(h["dir"], 1);
    assert_close(f(&h, "lo_ns"), 2000.0);
    assert_close(f(&h, "bin_ns"), 20.0);
    // the fake dpds sends 100 bins for direction 1 (like a link with `histograms`)
    assert_eq!(h["bins"].as_array().unwrap().len(), 100);
    assert_eq!(h["edges_ns"].as_array().unwrap().len(), 101);
    assert_eq!(h["underflow"], 3);
    assert_eq!(h["overflow"], 4);
    let r = a.get("/api/stats/7/delay_histogram").await;
    assert_eq!(r.status, 404);
    r.message();
}

// ---- docs, OpenAPI (CORS is off by default: see http_hardening.rs) -------

#[tokio::test]
async fn swagger_ui_is_served_under_api_docs() {
    let a = app();
    let r = a.get("/api/docs/").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().to_lowercase().contains("swagger"));
    let r = a.get("/api/docs").await;
    assert!(
        r.status == 200 || (300..400).contains(&r.status),
        "/api/docs must serve or redirect to the UI, got {}",
        r.status
    );
    if r.status != 200 {
        let loc = r
            .headers
            .get("location")
            .expect("redirect has a Location")
            .to_str()
            .unwrap()
            .to_string();
        assert!(loc.ends_with("/api/docs/"), "{loc}");
    }
}

#[tokio::test]
async fn openapi_json_documents_every_path() {
    let a = app();
    let r = a.get("/api/openapi.json").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    let paths = v["paths"].as_object().expect("paths");
    let expect: &[(&str, &[&str])] = &[
        ("/api/online", &["get"]),
        ("/api/interfaces", &["get"]),
        ("/api/host/hints", &["get"]),
        ("/api/links", &["get", "post"]),
        ("/api/links/{id}", &["get", "put", "delete"]),
        ("/api/settings", &["get", "put"]),
        ("/api/emulation/start", &["post"]),
        ("/api/emulation/stop", &["post"]),
        ("/api/emulation", &["get"]),
        ("/api/stats", &["get"]),
        ("/api/stats/{dir}/delay_histogram", &["get"]),
    ];
    for (path, methods) in expect {
        let p = paths.get(*path).unwrap_or_else(|| {
            panic!(
                "{path} missing from OpenAPI: {:?}",
                paths.keys().collect::<Vec<_>>()
            )
        });
        for m in *methods {
            assert!(p.get(*m).is_some(), "{m} {path} missing");
        }
    }
    let schemas = v["components"]["schemas"].as_object().expect("schemas");
    assert!(
        schemas.contains_key("ConfigDto"),
        "{:?}",
        schemas.keys().collect::<Vec<_>>()
    );
}
