//! REST API in-process (fake supervisor, fake telemetry, fake sysfs).
//! See the header of `common.rs` for the HTTP contract assumed. Spec 22: the configuration is
//! the body of `POST /api/emulation/start`; the validation rules of the former links and
//! settings routes apply to that body (more of them in `input_validation.rs`, `v12.rs`,
//! `launch_revalidation.rs`, `management_guard.rs`); the running configuration and
//! `POST /api/emulation/validate` are in `start_body.rs`.

use std::time::Duration;

use crate::common::*;
use dpds_controller::api::{self, Deps};
use dpds_controller::dto::ConfigDto;
use dpds_controller::eal::{drain_timeout, eal_argv, links_file_json, EalInput, EalPort};
use dpds_controller::supervisor::State;
use serde_json::{json, Value};

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

async fn start_ok(a: &TestApp, links: Vec<Value>) -> Value {
    a.start_ok(links).await
}

fn one(name: &str, ingress: &str, egress: &str) -> Value {
    link(name, ingress, egress)
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
    for id in ["isolation", "governor", "irq_affinity", "hugepages"] {
        let _ = hint(&v, id);
    }
    let direct = hints_json(&a.host, &Default::default());
    assert_eq!(
        v, direct,
        "GET serves the host-only hints, whatever was started"
    );
}

#[tokio::test]
async fn host_hints_have_no_mprq_hint_and_never_discuss_devargs() {
    let a = app();
    let v = a.get("/api/host/hints").await.json();
    assert_no_devargs_hint(&v);
}

// ---- the start body: structure ---------------------------------------------------

#[tokio::test]
async fn start_with_a_whole_body_is_a_202_with_the_emulation_view() {
    let a = app();
    let r = a.start_links(vec![one("a-to-b", NIC0, NIC1)]).await;
    assert_eq!(r.status, 202, "{}", r.text());
    let v = r.json();
    assert!(
        ["starting", "running"].contains(&v["state"].as_str().unwrap()),
        "{v}"
    );
}

#[tokio::test]
async fn link_name_validation() {
    let ok32 = "a".repeat(32);
    for (i, name) in [ok32.as_str(), "a_b.c-D9"].iter().enumerate() {
        let a = app();
        let p = format!("net_null{i}");
        let r = a.start_links(vec![one(name, &p, &p)]).await;
        assert_eq!(r.status, 202, "{name}: {}", r.text());
    }
    for bad in ["", &"a".repeat(33), "a b", "a/b", "ä", "a,b"] {
        let a = app();
        let r = a
            .start_links(vec![one(bad, "net_null9", "net_null9")])
            .await;
        assert_eq!(r.status, 400, "name {bad:?}");
        r.message();
        assert!(a.supervisor.plans().is_empty());
    }
}

#[tokio::test]
async fn link_structure_validation() {
    // control: the same shape with valid values is accepted (so the 400s below are about the values)
    assert_eq!(
        app()
            .start_links(vec![one("x", "net_null0", "net_null1")])
            .await
            .status,
        202
    );
    let a = app();
    let cfg = || cfg_json(1e6, 0.0);
    let cases = [
        (
            "empty ingress",
            json!({"name":"x","ingress":"","egress":"net_null0","config":cfg()}),
        ),
        (
            "empty egress",
            json!({"name":"x","ingress":"net_null0","egress":"","config":cfg()}),
        ),
        (
            "bad ingress",
            json!({"name":"x","ingress":"eth0","egress":"net_null0","config":cfg()}),
        ),
        (
            "bad egress",
            json!({"name":"x","ingress":"net_null0","egress":"0000:65:00","config":cfg()}),
        ),
        (
            "pcap vdev as egress",
            json!({"name":"x","ingress":"net_null0","egress":"net_pcap0","config":cfg()}),
        ),
        (
            "vdev with devargs as ingress",
            json!({"name":"x","ingress":"net_null0,size=64","egress":"net_null1","config":cfg()}),
        ),
        (
            "invalid config",
            json!({"name":"x","ingress":"net_null0","egress":"net_null1","config":cfg_json(1e6,-1.0)}),
        ),
    ];
    for (what, l) in cases {
        let r = a.start_links(vec![l]).await;
        assert_eq!(r.status, 400, "{what}: {}", r.text());
        let m = r.message();
        match what {
            "invalid config" => assert!(m.contains("std_ns"), "{m}"),
            "empty ingress" | "bad ingress" => assert!(m.to_lowercase().contains("port"), "{m}"),
            _ => {}
        }
    }
    assert!(
        a.supervisor.plans().is_empty(),
        "rejected links must not start"
    );
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

#[tokio::test]
async fn the_paired_link_shape_is_refused() {
    let a = app();
    let cfg = cfg_json(1e6, 0.0);
    let cases = [
        json!({"name":"x","ports":["net_null0","net_null1"],"configs":[cfg.clone(),cfg.clone()]}),
        json!({"name":"x","ports":["net_null0"],"configs":[cfg.clone()],"workers":[2]}),
    ];
    for l in cases {
        let r = a.start_links(vec![l.clone()]).await;
        assert_eq!(r.status, 400, "{l}: {}", r.text());
        r.message();
    }
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn incomplete_directed_links_are_refused() {
    let a = app();
    let cfg = cfg_json(1e6, 0.0);
    let cases = [
        json!({"name":"x","egress":"net_null1","config":cfg.clone()}),
        json!({"name":"x","ingress":"net_null1","config":cfg.clone()}),
        json!({"name":"x","ingress":"net_null0","egress":"net_null1"}),
        json!({"name":"x","ingress":"net_null0","egress":"net_null1","config":cfg.clone(),"worker":"two"}),
        json!({"name":"x","ingress":"net_null0","egress":"net_null1","config":cfg.clone(),"worker":[2]}),
        json!({"name":"x","ingress":["net_null0"],"egress":"net_null1","config":cfg.clone()}),
        json!({"name":"x","ingress":"net_null0","egress":"net_null1","config":cfg.clone(),"worker":-1}),
    ];
    for l in cases {
        let r = a.start_links(vec![l.clone()]).await;
        assert_eq!(r.status, 400, "{l}: {}", r.text());
        r.message();
    }
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn a_link_in_the_body_has_no_id_and_no_other_extras() {
    // ids are assigned by the controller (see start_body.rs): a client-supplied one is an unknown
    // field, like every other key the API does not know
    let a = app();
    for (extra, value) in [
        ("id", json!(7)),
        ("comment", json!("x")),
        ("lcore", json!(2)),
    ] {
        let mut l = one("x", "net_null0", "net_null0");
        l[extra] = value;
        let r = a.start_links(vec![l]).await;
        assert_eq!(r.status, 400, "{extra}: {}", r.text());
        assert!(r.message().contains(extra), "{extra}: {}", r.text());
    }
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn the_body_denies_unknown_fields_at_the_top_level() {
    let a = app();
    let base = || setup(vec![one("x", "net_null0", "net_null0")]).body();
    for key in [
        "presets",
        "version",
        "format",
        "pending_restart",
        "settings",
    ] {
        let mut b = base();
        b[key] = json!(1);
        let r = a.post("/api/emulation/start", b).await;
        assert_eq!(r.status, 400, "{key}: {}", r.text());
        assert!(r.message().contains(key), "{key}: {}", r.text());
    }
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn an_empty_link_list_is_nothing_to_start() {
    let a = app();
    for r in [
        a.start(&setup(vec![])).await,
        a.validate(&setup(vec![])).await,
    ] {
        assert_eq!(r.status, 400, "{}", r.text());
        assert!(
            r.message().to_lowercase().contains("nothing to start"),
            "{}",
            r.text()
        );
    }
    assert!(a.supervisor.plans().is_empty());
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

#[tokio::test]
async fn malformed_bodies_get_a_json_message() {
    let a = app();
    let r = a.request("POST", "/api/emulation/start", None).await; // empty body, no content type
    assert_eq!(r.status, 415, "{}", r.status);
    r.message();
    let r = a.post("/api/emulation/start", json!({"name": "x"})).await; // wrong shape
    assert_eq!(r.status, 400, "{}", r.status);
    r.message();
    let r = a
        .post("/api/emulation/start", json!({"links": "none"}))
        .await;
    assert_eq!(r.status, 400, "{}", r.status);
    r.message();
    let mut l = one("x", "net_null0", "net_null0");
    l["config"]["delayer"]["distribution"] = json!("Bogus");
    let r = a.start_links(vec![l]).await;
    assert_eq!(r.status, 400, "{}", r.status);
    r.message();
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn duplicate_link_names_in_a_body_are_a_409() {
    let a = app();
    let r = a
        .start_links(vec![
            one("a", NIC0, NIC1),
            one("a", "net_null0", "net_null1"),
        ])
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    r.message();
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn a_port_reused_as_ingress_is_a_409_naming_the_other_link_and_the_port() {
    let a = app();
    // NIC0 is already the ingress of `first`; the egress differs
    let r = a
        .start_links(vec![one("first", NIC0, NIC1), one("second", NIC0, INTEL)])
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    let m = r.message();
    assert!(
        m.contains("first"),
        "the message must name the other link: {m}"
    );
    assert!(m.contains(NIC0), "the message must name the port: {m}");
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn a_port_reused_as_egress_is_a_409_naming_the_other_link_and_the_port() {
    let a = app();
    let r = a
        .start_links(vec![one("first", NIC0, NIC1), one("second", INTEL, NIC1)])
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    let m = r.message();
    assert!(
        m.contains("first"),
        "the message must name the other link: {m}"
    );
    assert!(m.contains(NIC1), "the message must name the port: {m}");
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn a_hairpin_conflicts_like_any_other_link() {
    // net_null0 is taken as ingress and as egress by `loop`
    for other in [
        one("x", "net_null0", "net_null1"),
        one("y", "net_null1", "net_null0"),
    ] {
        let a = app();
        let r = a
            .start_links(vec![one("loop", "net_null0", "net_null0"), other])
            .await;
        assert_eq!(r.status, 409, "{}", r.text());
        assert!(r.message().contains("loop"));
    }
    // a different port is fine
    let a = app();
    let r = a
        .start_links(vec![
            one("loop", "net_null0", "net_null0"),
            one("z", "net_null1", "net_null1"),
        ])
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
}

#[tokio::test]
async fn a_port_may_be_ingress_of_one_link_and_egress_of_another() {
    let a = app();
    // the classic bidirectional pair is simply two links, and INTEL is free for a hairpin
    let r = a
        .start_links(vec![
            one("a-to-b", NIC0, NIC1),
            one("b-to-a", NIC1, NIC0),
            one("c", INTEL, INTEL),
        ])
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
}

#[tokio::test]
async fn a_hairpin_link_is_accepted_and_reported() {
    let a = app();
    let r = a.start_links(vec![one("loop", NIC0, NIC0)]).await;
    assert_eq!(r.status, 202, "{}", r.text());
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["config"]["links"][0]["ingress"], NIC0, "{v}");
    assert_eq!(v["config"]["links"][0]["egress"], NIC0, "{v}");
}

#[tokio::test]
async fn a_three_link_ring_is_accepted_in_body_order() {
    let a = app();
    let ring = [
        ("a-to-b", NIC0, NIC1),
        ("b-to-c", NIC1, INTEL),
        ("c-to-a", INTEL, NIC0),
    ];
    a.start_ok(ring.iter().map(|(n, i, e)| one(n, i, e)).collect())
        .await;
    let v = a.get("/api/emulation").await.json();
    let got: Vec<(String, String, String)> = v["config"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["name"].as_str().unwrap().into(),
                l["ingress"].as_str().unwrap().into(),
                l["egress"].as_str().unwrap().into(),
            )
        })
        .collect();
    let want: Vec<(String, String, String)> = ring
        .iter()
        .map(|(n, i, e)| (n.to_string(), i.to_string(), e.to_string()))
        .collect();
    assert_eq!(got, want);
}

// ---- the launch plan -------------------------------------------------------------

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
    assert!(absent(&v, "config"), "{v}");
    assert!(absent(&v, "main_lcore"), "{v}");
    assert_eq!(v["directions"], json!([]));
    assert!(v.get("pending_restart").is_none(), "{v}");
    assert_eq!(v["log_tail"], json!([]));
}

#[tokio::test]
async fn start_hands_the_supervisor_a_complete_launch_plan() {
    let a = app();
    let mut b = one("b-to-a", NIC1, NIC0);
    b["config"] = cfg_json(2.0e6, 1.0e5);
    let links = vec![
        one("a-to-b", NIC0, NIC1),
        b,
        one("loop", "net_null0", "net_null0"),
    ];

    let v = start_ok(&a, links.clone()).await;
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

    // the links file is the v2 file of exactly the links of the body, in order
    let typed: Vec<_> = links
        .iter()
        .enumerate()
        .map(|(i, l)| model_link(i as u32 + 1, l))
        .collect();
    assert_eq!(
        plan.links_json,
        links_file_json(&typed, &plan.directions).unwrap()
    );

    // one direction (= one worker) per link, numbered in link order
    assert_eq!(plan.directions.len(), 3);
    assert_eq!(
        plan.directions
            .iter()
            .map(|d| d.link.as_str())
            .collect::<Vec<_>>(),
        ["a-to-b", "b-to-a", "loop"]
    );
    assert_eq!(
        plan.directions.iter().map(|d| d.dir).collect::<Vec<_>>(),
        [0, 1, 2]
    );
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
    // the generated links file carries exactly these lcores, one per link
    for (i, d) in plan.directions.iter().enumerate() {
        assert_eq!(plan.links_json["links"][i]["lcore"], d.lcore, "link {i}");
        assert!(plan.links_json["links"][i].get("lcores").is_none());
        assert!([2, 3, 6, 7].contains(&d.lcore));
    }
    assert_eq!(plan.links_json["links"][0]["ingress"], NIC0);
    assert_eq!(plan.links_json["links"][0]["egress"], NIC1);
    assert_eq!(plan.links_json["links"][1]["ingress"], NIC1);
    assert_eq!(plan.links_json["links"][1]["egress"], NIC0);
    assert_eq!(plan.links_json["links"][2]["ingress"], "net_null0");
    assert_eq!(plan.links_json["links"][2]["egress"], "net_null0");

    let mut workers: Vec<u32> = plan.directions.iter().map(|d| d.lcore).collect();
    workers.sort_unstable();
    // one `-a`/`--vdev` per DISTINCT port, in the order the ports first appear in the links
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

    let cores: Vec<dpds_core::Config> = links
        .iter()
        .map(|l| {
            let c: ConfigDto = serde_json::from_value(l["config"].clone()).unwrap();
            c.to_core().unwrap()
        })
        .collect();
    assert_eq!(plan.drain_timeout, drain_timeout(&cores));
    assert!(plan.drain_timeout >= Duration::from_secs(5));
}

#[tokio::test]
async fn a_three_link_ring_plans_three_workers_and_three_ports_once() {
    let a = app();
    start_ok(
        &a,
        vec![
            one("a-to-b", NIC0, NIC1),
            one("b-to-c", NIC1, INTEL),
            one("c-to-a", INTEL, NIC0),
        ],
    )
    .await;
    let plan = &a.supervisor.plans()[0];
    assert_eq!(plan.directions.len(), 3);
    let rxtx: Vec<(&str, &str)> = plan
        .directions
        .iter()
        .map(|d| (d.rx.as_str(), d.tx.as_str()))
        .collect();
    assert_eq!(rxtx, [(NIC0, NIC1), (NIC1, INTEL), (INTEL, NIC0)]);
    // every port appears once in the argv although it is ingress of one link and egress of another
    let allowed: Vec<&String> = plan
        .argv
        .iter()
        .enumerate()
        .filter(|(i, a)| *a == "-a" && *i + 1 < plan.argv.len())
        .map(|(i, _)| &plan.argv[i + 1])
        .collect();
    assert_eq!(allowed.len(), 3, "{:?}", plan.argv);
    for port in [NIC0, NIC1, INTEL] {
        assert_eq!(
            allowed.iter().filter(|a| a.starts_with(port)).count(),
            1,
            "{port} in {:?}",
            plan.argv
        );
    }
    // 1 main + 3 workers
    assert_eq!(plan.argv[1].split(',').count(), 4, "{:?}", plan.argv);
    assert_eq!(plan.links_json["links"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn a_hairpin_is_one_port_one_worker() {
    let a = app();
    start_ok(&a, vec![one("loop", "net_null0", "net_null0")]).await;
    let plan = &a.supervisor.plans()[0];
    assert_eq!(plan.directions.len(), 1);
    assert_eq!(
        plan.argv[1].split(',').count(),
        2,
        "main + one worker: {:?}",
        plan.argv
    );
    assert_eq!(
        plan.argv.iter().filter(|x| *x == "net_null0").count(),
        1,
        "{:?}",
        plan.argv
    );
    assert_eq!(plan.argv.iter().filter(|x| *x == "--vdev").count(), 1);
}

#[tokio::test]
async fn the_devargs_stay_fixed_and_the_main_lcore_is_automatic() {
    let a = app();
    let r = a.start(&setup(vec![one("l", NIC0, NIC0)])).await;
    assert_eq!(r.status, 202, "{}", r.text());
    let plan = &a.supervisor.plans()[0];
    assert_eq!(plan.main_lcore, 0);
    assert!(plan.argv.contains(&format!("{NIC0},txq_mpw_en=1")));
    assert!(!plan.argv.join(" ").contains("mprq"));
}

#[tokio::test]
async fn start_stop_state_transitions() {
    let a = app();
    start_ok(&a, vec![one("loop", "net_null0", "net_null0")]).await;

    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "running");
    assert_eq!(v["pid"], 4242);
    assert!(v["started_at"].is_u64());
    assert_eq!(v["directions"].as_array().unwrap().len(), 1);
    assert_eq!(v["main_lcore"], 0);
    assert!(!v["log_tail"].as_array().unwrap().is_empty());

    // can't start twice, not even with the same body
    let r = a
        .start_links(vec![one("loop", "net_null0", "net_null0")])
        .await;
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
    for s in [State::Starting, State::Stopping] {
        a.supervisor.force_status(status(s));
        let r = a
            .start_links(vec![one("loop", "net_null0", "net_null0")])
            .await;
        assert_eq!(r.status, 409, "{s:?}");
    }
    assert!(a.supervisor.plans().is_empty());
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "stopping");
}

#[tokio::test]
async fn failed_start_is_reported_with_error_and_log_tail_and_can_be_retried() {
    let a = app();
    a.supervisor.fail_next_start(
        "dpds exited with status 1",
        &["EAL: Error", "dpds: cannot create mbuf pool"],
    );
    let r = a
        .start_links(vec![one("loop", "net_null0", "net_null0")])
        .await;
    assert_eq!(r.status, 202);

    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "failed");
    assert_eq!(v["error"], "dpds exited with status 1");
    assert_eq!(
        v["log_tail"],
        json!(["EAL: Error", "dpds: cannot create mbuf pool"])
    );
    assert!(absent(&v, "pid"));

    // retry from failed, with a body again
    start_ok(&a, vec![one("loop", "net_null0", "net_null0")]).await;
    assert_eq!(a.get("/api/emulation").await.json()["state"], "running");
    assert_eq!(a.supervisor.plans().len(), 2);
}

#[tokio::test]
async fn start_with_not_enough_isolated_cores_is_a_400_and_does_not_spawn() {
    let a = app();
    let links = (0..5)
        .map(|i| {
            let p = format!("net_null{i}");
            one(&format!("l{i}"), &p, &p)
        })
        .collect();
    let r = a.start_links(links).await;
    assert_eq!(r.status, 400);
    assert!(r.message().contains("isolated"));
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn start_without_the_telemetry_build_of_dpds_is_a_400() {
    let host = FakeHost::bare_metal();
    let router = api::router(Deps {
        roots: host.roots(),
        supervisor: FakeSupervisor::new(),
        telemetry: FakeTelemetry::new(),
        dpds_bin: "/usr/local/bin/dpds".into(),
        runtime_dir: "/run/dpds-controller-test".into(),
        dpds_has_telemetry: false,
        is_root: false,
    });
    let body = setup(vec![one("l", "net_null0", "net_null0")]).body();
    let r = call(&router, "POST", "/api/emulation/start", Some(body)).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("telemetry"), "{}", r.text());
}

// ---- stats ---------------------------------------------------------------

#[tokio::test]
async fn stats_without_telemetry_data_are_empty() {
    let a = app();
    let r = a.get("/api/stats").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["directions"], json!([]));
    assert!(v.get("ports").is_none(), "{v}");
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
    assert!(v.get("ports").is_none(), "{v}");
    assert_eq!(v["directions"][0]["link"], "a-to-b");
    assert_eq!(v["directions"][1]["link"], "b-to-a");
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
        ("/api/host/hints", &["get", "post"]),
        ("/api/emulation/start", &["post"]),
        ("/api/emulation/validate", &["post"]),
        ("/api/emulation/stop", &["post"]),
        ("/api/emulation", &["get"]),
        ("/api/stats", &["get"]),
        ("/api/stats/{dir}/delay_histogram", &["get"]),
        ("/api/version", &["get"]),
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
    // spec 22: the stored-config API is gone from the document, schemas included
    for gone in [
        "/api/links",
        "/api/links/{id}",
        "/api/presets",
        "/api/presets/{id}",
        "/api/settings",
        "/api/export",
        "/api/import",
    ] {
        assert!(paths.get(gone).is_none(), "{gone} is still documented");
    }
    for gone in [
        "Preset",
        "PresetInput",
        "ExportDoc",
        "ImportSummary",
        "LinkInput",
    ] {
        assert!(
            !schemas.contains_key(gone),
            "schema {gone} is still documented"
        );
    }
}
