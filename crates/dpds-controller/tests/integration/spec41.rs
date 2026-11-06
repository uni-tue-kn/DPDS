//! Spec 41: no settings, automatic lcores, no extra EAL arguments (controller part C1-C8).
//!
//! The start, validate and hints body is `{links}`. `settings` and a link's `worker` are unknown
//! fields (400 naming them). The main lcore is the lowest online core that is not isolated, the
//! workers are the isolated cores handed out automatically, and the EAL argv has no extras.
//! (The fixed 1000 ms poll is pinned in `telemetry_freshness`.)

use crate::common::*;
use serde_json::{json, Value};

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

fn vdevs(n: u32) -> Setup {
    setup(
        (0..n)
            .map(|i| {
                let p = format!("net_null{i}");
                link(&format!("l{i}"), &p, &p)
            })
            .collect(),
    )
}

const ENDPOINTS: [&str; 3] = [
    "/api/emulation/start",
    "/api/emulation/validate",
    "/api/host/hints",
];

// ---- C1: the body is {links} -----------------------------------------------------------------

#[tokio::test]
async fn a_body_with_settings_is_a_400_naming_it_at_every_endpoint() {
    for settings in [
        json!({}),
        json!({"main_lcore": null, "eal_extra_args": [], "poll_interval_ms": 500}),
        json!({"main_lcore": 1}),
        json!({"eal_extra_args": ["--no-huge"]}),
        json!({"poll_interval_ms": 100}),
        json!(null),
    ] {
        for path in ENDPOINTS {
            let a = app();
            let mut body = vdevs(1).body();
            body["settings"] = settings.clone();
            let r = a.post(path, body).await;
            assert_eq!(r.status, 400, "{path} {settings}: {}", r.text());
            assert!(
                r.message().contains("settings"),
                "{path} {settings}: {}",
                r.text()
            );
            assert!(a.supervisor.plans().is_empty(), "{path}: nothing spawned");
            assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
        }
    }
}

#[tokio::test]
async fn a_links_only_body_is_accepted_at_every_endpoint() {
    let body = vdevs(1).body();
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["links"]
    );
    for (path, status) in [
        (ENDPOINTS[0], 202),
        (ENDPOINTS[1], 200),
        (ENDPOINTS[2], 200),
    ] {
        let a = app();
        let r = a.post(path, body.clone()).await;
        assert_eq!(r.status, status, "{path}: {}", r.text());
    }
}

#[tokio::test]
async fn the_echo_of_a_run_has_links_and_no_settings() {
    let a = app();
    a.start_ok(vec![link("l", NIC0, NIC1)]).await;
    let v = a.get("/api/emulation").await.json();
    let cfg = v["config"].as_object().expect("config while running");
    assert_eq!(cfg.keys().collect::<Vec<_>>(), ["links"], "{v}");
    assert!(
        !v.to_string().contains("settings"),
        "no settings anywhere in {v}"
    );
}

#[tokio::test]
async fn openapi_has_no_settings_and_no_worker() {
    let doc = app().get("/api/openapi.json").await.json();
    let schemas = &doc["components"]["schemas"];
    assert!(schemas.get("Settings").is_none(), "{schemas}");
    let props = |name: &str| -> Vec<String> {
        schemas[name]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("no schema {name}: {schemas}"))
            .keys()
            .cloned()
            .collect()
    };
    assert_eq!(props("StartRequest"), ["links"]);
    assert_eq!(props("RunConfig"), ["links"]);
    for name in ["StartLink", "Link"] {
        assert!(!props(name).contains(&"worker".to_string()), "{name}");
    }
    let text = doc.to_string();
    for gone in ["eal_extra_args", "poll_interval_ms"] {
        assert!(!text.contains(gone), "{gone} still in the OpenAPI document");
    }
}

// ---- C2: no worker on a link -----------------------------------------------------------------

#[tokio::test]
async fn a_link_with_a_worker_is_a_400_naming_it_at_every_endpoint() {
    for worker in [json!(2), json!(7), json!(null), json!("two")] {
        for path in ENDPOINTS {
            let a = app();
            let mut l = link("l", NIC0, NIC0);
            l["worker"] = worker.clone();
            let r = a.post(path, setup(vec![l]).body()).await;
            assert_eq!(r.status, 400, "{path} {worker}: {}", r.text());
            assert!(
                r.message().contains("worker"),
                "{path} {worker}: {}",
                r.text()
            );
            assert!(a.supervisor.plans().is_empty(), "{path}: nothing spawned");
        }
    }
}

// ---- C3: the main lcore -----------------------------------------------------------------------

#[test]
fn the_main_lcore_is_the_lowest_online_core_that_is_not_isolated() {
    let nodes = [(0, "0-3"), (1, "4-7")];
    for (online, isolated, main) in [
        ("0-7", "2-3,6-7", 0),
        ("0-7", "0-1,3", 2),
        ("0-7", "0", 1),
        ("0-7", "0-6", 7),
        ("1-7", "2-3", 1), // core 0 is offline
        ("0-7", "1-2,4-7", 0),
    ] {
        let h = FakeHost::bare_metal();
        h.cpus(8, Some(isolated), &nodes);
        h.write("sys/devices/system/cpu/online", &format!("{online}\n"));
        let a = plan_for(&h, &vdevs(1)).unwrap_or_else(|e| panic!("{online}/{isolated}: {e}"));
        assert_eq!(a.main_lcore, main, "online {online}, isolated {isolated}");
        assert!(
            !a.directions.iter().any(|d| d.lcore == a.main_lcore),
            "the main lcore is no worker"
        );
    }
}

#[tokio::test]
async fn with_every_online_core_isolated_the_start_fails_without_naming_settings() {
    let h = FakeHost::bare_metal();
    h.cpus(4, Some("0-3"), &[(0, "0-3")]);
    let a = TestApp::new(h);
    for path in [ENDPOINTS[0], ENDPOINTS[1]] {
        let r = a.post(path, vdevs(1).body()).await;
        assert_eq!(r.status, 400, "{path}: {}", r.text());
        let m = r.message();
        assert!(m.contains("isolated"), "{m}");
        assert!(
            !m.contains("settings") && !m.contains("main_lcore"),
            "the message must not point at a removed setting: {m}"
        );
    }
    assert!(a.supervisor.plans().is_empty());
}

// ---- C4: workers are automatic ------------------------------------------------------------------

#[test]
fn workers_are_the_lowest_free_isolated_cores_in_link_order() {
    let h = FakeHost::bare_metal(); // isolated 2-3 (node 0), 6-7 (node 1)
    let a = plan_for(&h, &vdevs(3)).unwrap();
    assert_eq!(a.main_lcore, 0);
    assert_eq!(
        a.directions.iter().map(|d| d.lcore).collect::<Vec<_>>(),
        [2, 3, 6]
    );
    assert_eq!(
        a.directions
            .iter()
            .map(|d| d.link.as_str())
            .collect::<Vec<_>>(),
        ["l0", "l1", "l2"]
    );
}

#[test]
fn workers_prefer_the_numa_node_of_the_ingress_nic_then_the_lowest_id() {
    let h = FakeHost::bare_metal();
    // ingress on node 1, node 1, node 0
    let st = setup(vec![
        link("a", NIC1, NIC0),
        link("b", INTEL, "net_null0"),
        link("c", NIC0, "net_null1"),
    ]);
    let a = plan_for(&h, &st).unwrap();
    assert_eq!(
        a.directions.iter().map(|d| d.lcore).collect::<Vec<_>>(),
        [6, 7, 2]
    );
}

#[test]
fn every_link_gets_its_own_isolated_core() {
    let h = FakeHost::bare_metal();
    let a = plan_for(&h, &vdevs(4)).unwrap();
    let mut l: Vec<u32> = a.directions.iter().map(|d| d.lcore).collect();
    l.sort_unstable();
    assert_eq!(l, [2, 3, 6, 7]);
}

#[tokio::test]
async fn too_few_isolated_cores_names_the_need_the_cores_and_isolcpus_but_no_override() {
    let a = app(); // isolated 2,3,6,7
    for path in [ENDPOINTS[0], ENDPOINTS[1]] {
        let r = a.post(path, vdevs(5).body()).await;
        assert_eq!(r.status, 400, "{path}: {}", r.text());
        let m = r.message();
        assert!(m.contains("5 link"), "names the number needed: {m}");
        assert!(m.contains("2,3,6,7"), "names the isolated cores: {m}");
        assert!(m.contains("isolcpus"), "hints at isolcpus: {m}");
        let low = m.to_lowercase();
        assert!(
            !low.contains("override") && !low.contains("pin"),
            "no worker override any more: {m}"
        );
    }
    assert!(a.supervisor.plans().is_empty());
}

#[test]
fn without_any_isolated_core_the_error_says_none_and_isolcpus() {
    let e = plan_for(&FakeHost::bare(), &vdevs(1)).unwrap_err();
    assert!(e.contains("none"), "{e}");
    assert!(e.contains("isolcpus"), "{e}");
    let low = e.to_lowercase();
    assert!(!low.contains("override") && !low.contains("pin"), "{e}");
}

#[test]
fn the_isolated_cores_hint_does_not_offer_to_pin_workers() {
    let h = hints_json(&FakeHost::bare_metal(), &vdevs(5));
    let text = hint(&h, "isolated_cores").to_string().to_lowercase();
    assert!(
        !text.contains("pin") && !text.contains("override"),
        "{text}"
    );
    assert_level(&h, "isolated_cores", "error", "error");
}

// ---- C5: no extra EAL arguments ------------------------------------------------------------------

#[tokio::test]
async fn the_argv_is_what_the_controller_computes_and_nothing_else() {
    let a = app();
    a.start_ok(vec![link("l", "net_null0", "net_null0")]).await;
    let argv = &a.supervisor.plans()[0].argv;
    let (links_file, head) = argv.split_last().unwrap();
    assert!(links_file.ends_with("links.json"), "{argv:?}");
    let want: Vec<String> = [
        "-l",
        "0,2",
        "--main-lcore",
        "0",
        "--in-memory",
        "--file-prefix",
        "dpds-ctl",
        "--vdev",
        "net_null0",
        "--",
        "--links",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(head, want.as_slice(), "{argv:?}");
}

#[tokio::test]
async fn an_eal_argument_has_no_way_in() {
    // not as settings (C1), not as a top-level or link member
    let a = app();
    let mut body = vdevs(1).body();
    body["eal_extra_args"] = json!(["--no-huge"]);
    let r = a.post(ENDPOINTS[0], body).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("eal_extra_args"), "{}", r.text());
    let mut l = link("l", "net_null0", "net_null0");
    l["eal_extra_args"] = json!(["--no-huge"]);
    let r = a.start_links(vec![l]).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(a.supervisor.plans().is_empty());
}

// ---- C7: hints and check keep working with the automatic plan ---------------------------------

#[tokio::test]
async fn hints_with_links_on_a_host_with_every_core_isolated_answer_200() {
    let h = FakeHost::bare_metal();
    h.cpus(4, Some("0-3"), &[(0, "0-3")]);
    let a = TestApp::new(h);
    let r = a.post(ENDPOINTS[2], vdevs(1).body()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let hints: Value = r.json();
    assert_level(&hints, "isolated_cores", "error", "error");
    assert!(
        !hint(&hints, "isolated_cores")
            .to_string()
            .contains("settings"),
        "{hints}"
    );
}

#[test]
fn the_check_command_runs_on_a_host_with_every_core_isolated() {
    let h = FakeHost::bare_metal();
    h.cpus(4, Some("0-3"), &[(0, "0-3")]);
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_dpds-controller"))
        .arg("check")
        .arg("--sysfs-root")
        .arg(h.sysfs())
        .arg("--procfs-root")
        .arg(h.procfs())
        .output()
        .expect("cannot run dpds-controller");
    assert!(
        matches!(o.status.code(), Some(0 | 1)),
        "exit {:?}, stderr: {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!o.stdout.is_empty());
}
