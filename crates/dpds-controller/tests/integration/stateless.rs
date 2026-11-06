//! The controller is stateless. It stores nothing on disk (no `state.json`, no state
//! directory, no `--state-dir` / `DPDS_CONTROLLER_STATE_DIR`, no store module); the whole
//! configuration travels in the body of `POST /api/emulation/start`; there are no stored-config
//! routes (`/api/links*`, `/api/presets*`, `/api/settings`, `/api/export`, `/api/import`).
//! A controller starts empty, always (a leftover state file cannot stop it).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::common::*;
use serde_json::{json, Value};

/// A directory that cannot be created below: `/proc` takes no new entries. A controller that
/// tried to create a state directory there would fail.
const UNWRITABLE: &str = "/proc/dpds-controller-test-nonexistent/state";

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// ---- no state anywhere ---------------------------------------------------------------------

#[test]
fn the_controller_starts_empty_whatever_the_former_state_dir_is() {
    // a retired env var points at a place nothing can be written to: it is not read
    let c = ControllerProcess::start(
        &["serve", "--bind", "{addr}"],
        &[("DPDS_CONTROLLER_STATE_DIR", UNWRITABLE)],
    );
    let (st, v) = http(&c.addr, "GET", "/api/emulation", None).unwrap();
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["state"], "stopped", "{v}");
    assert!(absent(&v, "config"), "{v}");
    assert_eq!(v["directions"], json!([]), "{v}");
    assert!(!Path::new(UNWRITABLE).exists());
}

#[test]
fn a_leftover_state_file_of_the_old_layout_is_never_read() {
    // a state.json that an older controller refused (an unknown key, a broken shape) must not matter
    let dir = tempfile::tempdir().unwrap();
    for garbage in [
        "{garbage".to_string(),
        r#"{"links":[],"settings":{"main_lcore":null,"eal_extra_args":[],"poll_interval_ms":500,"mlx5_devargs":"x"},"next_id":1}"#.to_string(),
        json!({"links":[{"id":1,"ports":["net_null0"],"configs":[cfg_json(1.0e6,0.0)]}],
               "settings":{"main_lcore":null,"eal_extra_args":[],"poll_interval_ms":500},
               "next_id":2}).to_string(),
    ] {
        std::fs::write(dir.path().join("state.json"), &garbage).unwrap();
        let c = ControllerProcess::start(
            &["serve", "--bind", "{addr}"],
            &[("DPDS_CONTROLLER_STATE_DIR", dir.path().to_str().unwrap())],
        );
        let (st, v) = http(&c.addr, "GET", "/api/emulation", None).unwrap();
        assert_eq!(st, 200, "{v}");
        assert_eq!(v["state"], "stopped");
        // and it neither rewrote nor deleted the file
        assert_eq!(
            std::fs::read_to_string(dir.path().join("state.json")).unwrap(),
            garbage
        );
    }
}

#[test]
fn serving_and_a_start_attempt_write_no_file_into_the_working_directory_or_home() {
    let cwd = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let rt = tempfile::tempdir().unwrap();
    let addr = free_addr();
    let mut child = Command::new(env!("CARGO_BIN_EXE_dpds-controller"))
        .args(["serve", "--bind", &addr, "--runtime-dir"])
        .arg(rt.path().join("rt"))
        .current_dir(cwd.path())
        .env("HOME", home.path())
        .env("DPDS_CONTROLLER_STATE_DIR", UNWRITABLE)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t0 = std::time::Instant::now();
    while !matches!(http(&addr, "GET", "/api/online", None), Ok((200, _))) {
        assert!(t0.elapsed().as_secs() < 15, "no answer on {addr}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // reads, hints and a start that the host refuses (no dpds binary here)
    for (m, p, b) in [
        ("GET", "/api/interfaces", None),
        ("GET", "/api/host/hints", None),
        ("GET", "/api/stats", None),
        (
            "POST",
            "/api/host/hints",
            Some(setup(vec![link("net_null0", "net_null0")]).body()),
        ),
        (
            "POST",
            "/api/emulation/start",
            Some(setup(vec![link("net_null0", "net_null0")]).body()),
        ),
    ] {
        let _ = http(&addr, m, p, b.as_ref()).unwrap();
    }
    let _ = child.kill();
    let _ = child.wait();
    for (what, d) in [("cwd", cwd.path()), ("home", home.path())] {
        let left: Vec<_> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            left.is_empty(),
            "the controller wrote into the {what}: {left:?}"
        );
    }
}

#[test]
fn a_state_dir_option_is_a_usage_error() {
    // gone: `--state-dir` on `serve`, on the default command and on `check`
    for args in [
        vec!["serve", "--state-dir", "/tmp/x"],
        vec!["--state-dir", "/tmp/x"],
        vec!["check", "--state-dir", "/tmp/x"],
        vec!["serve", "--state-dir=/tmp/x"],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_dpds-controller"))
            .args(&args)
            .args(if args[0] == "check" {
                vec![]
            } else {
                vec!["--bind", "127.0.0.1:0"]
            })
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}: {o:?}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("--state-dir"), "{args:?}: {err}");
    }
}

#[test]
fn the_help_does_not_mention_a_state_dir() {
    for args in [
        vec!["--help"],
        vec!["serve", "--help"],
        vec!["check", "--help"],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_dpds-controller"))
            .args(&args)
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&o.stdout).to_lowercase();
        assert!(
            !out.contains("state-dir") && !out.contains("state_dir"),
            "{args:?}: {out}"
        );
        assert!(!out.contains("/var/lib"), "{args:?}: {out}");
    }
}

#[test]
fn check_works_without_any_state() {
    let host = FakeHost::bare_metal();
    let o = Command::new(env!("CARGO_BIN_EXE_dpds-controller"))
        .arg("check")
        .arg("--sysfs-root")
        .arg(host.sysfs())
        .arg("--procfs-root")
        .arg(host.procfs())
        .arg("--json")
        .env("DPDS_CONTROLLER_STATE_DIR", UNWRITABLE)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let hints: Value = serde_json::from_slice(&o.stdout).unwrap();
    // host-only hints: the same ones `GET /api/host/hints` serves
    let a = TestApp::new(host);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let served = rt.block_on(a.get("/api/host/hints")).json();
    assert_eq!(hints, served);
}

#[test]
fn the_crate_has_no_store_module_and_no_state_dir_code() {
    let src = manifest().join("src");
    assert!(
        !src.join("store.rs").exists(),
        "src/store.rs must be deleted"
    );
    let mut hits = Vec::new();
    for e in std::fs::read_dir(&src).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            let text = std::fs::read_to_string(&p).unwrap();
            for needle in [
                "mod store",
                "store::",
                "state_dir",
                "STATE_DIR",
                "STATE_FILE_NAME",
                "StoredState",
            ] {
                if text.contains(needle) {
                    hits.push(format!("{}: {needle}", p.display()));
                }
            }
        }
    }
    assert!(hits.is_empty(), "leftovers of the stored state: {hits:#?}");
}

// ---- no stored-config routes -------------------------------------------------------

#[tokio::test]
async fn the_stored_config_routes_are_404() {
    let a = TestApp::new(FakeHost::bare_metal());
    let link_body = link("net_null0", "net_null0");
    let preset = json!({"name": "p", "config": cfg_json(1.0e6, 0.0)});
    let doc = json!({"format": "dpds-controller-export", "version": 2,
                     "links": [], "presets": []});
    let routes: Vec<(&str, &str, Option<Value>)> = vec![
        ("GET", "/api/links", None),
        ("POST", "/api/links", Some(link_body.clone())),
        ("GET", "/api/links/1", None),
        ("PUT", "/api/links/1", Some(link_body)),
        ("DELETE", "/api/links/1", None),
        ("GET", "/api/settings", None),
        (
            "PUT",
            "/api/settings",
            Some(json!({"poll_interval_ms": 500})),
        ),
        ("GET", "/api/presets", None),
        ("POST", "/api/presets", Some(preset.clone())),
        ("GET", "/api/presets/1", None),
        ("PUT", "/api/presets/1", Some(preset)),
        ("DELETE", "/api/presets/1", None),
        ("GET", "/api/export", None),
        ("POST", "/api/import", Some(doc.clone())),
        ("POST", "/api/import?mode=replace", Some(doc)),
    ];
    for (method, path, body) in routes {
        let r = a.request(method, path, body).await;
        assert_eq!(r.status, 404, "{method} {path}: {}", r.text());
        r.message();
    }
    // nothing was stored by any of them
    assert!(a.supervisor.plans().is_empty());
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

#[tokio::test]
async fn the_removed_routes_stay_404_while_an_emulation_runs() {
    let a = TestApp::new(FakeHost::bare_metal());
    a.start_ok(vec![link("net_null0", "net_null0")]).await;
    for p in ["/api/links", "/api/presets", "/api/settings", "/api/export"] {
        assert_eq!(a.get(p).await.status, 404, "{p}");
    }
}

// ---- nothing is remembered between starts --------------------------------------------------

#[tokio::test]
async fn nothing_is_remembered_between_starts() {
    let a = TestApp::new(FakeHost::bare_metal());
    a.start_ok(vec![link(NIC0, NIC1)]).await;
    assert_eq!(a.post("/api/emulation/stop", json!({})).await.status, 202);
    // after the stop the configuration is gone from the controller
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["state"], "stopped", "{v}");
    assert!(absent(&v, "config"), "{v}");
    assert_eq!(v["directions"], json!([]), "{v}");
    // the next start uses the next body, not a merge with the previous one: the ingress the
    // first body used is free again
    a.start_ok(vec![link(NIC0, INTEL)]).await;
    let plans = a.supervisor.plans();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[1].links_json["links"].as_array().unwrap().len(), 1);
    assert_eq!(plans[1].links_json["links"][0]["ingress"], NIC0);
    assert_eq!(plans[1].links_json["links"][0]["egress"], INTEL);
}

#[tokio::test]
async fn a_second_controller_over_the_same_host_knows_nothing_of_the_first() {
    // two routers = two controller processes: the configuration of one is invisible to the other
    let host = FakeHost::bare_metal();
    let first = TestApp::new(FakeHost::bare_metal());
    first.start_ok(vec![link(NIC0, NIC1)]).await;
    let second = router_over(host.roots());
    let v = call(&second, "GET", "/api/emulation", None).await.json();
    assert_eq!(v["state"], "stopped", "{v}");
    assert!(absent(&v, "config"), "{v}");
}
