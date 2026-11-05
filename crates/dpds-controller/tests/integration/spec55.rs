//! Spec 55: the host hints are computed in the background when the controller starts serving,
//! `GET /api/host/hints` answers the stored result, `GET ?recheck=true` computes it again.
//!
//! # Assumptions (the spec leaves the hook open)
//!
//! - `dpds_controller::api::router_with_startup(deps: Deps, frontend: Frontend) -> axum::Router`
//!   is `router_with` plus the startup task of H1, spawned on the tokio runtime the caller is in
//!   (so it must be called inside a runtime) and started before it returns. `main.rs` serves this
//!   router. `router` / `router_with` stay without the task and keep today's behaviour (a plain GET
//!   computes on demand), so the other tests, which count tool calls, do not change. All of this
//!   file's tests reach the task through [`warm_app`], so changing the name is a one-line edit.
//! - The task computes the host hints (no links) and queries the firmware of every mlx5 NIC of the
//!   fake host (`0000:17:00.0` and `0000:65:00.0`; the tg3, ice and NVMe devices are not queried)
//!   through the existing `FirmwareCache`. `GET` waits for it while it runs.
//! - A failed firmware query is not cached (like today: only successful results are), and it does
//!   not stop the task.
//! - `GET ?recheck=true` while an emulation is starting, running or stopping computes the host
//!   hints without tools: it reads the host again but runs no command and keeps the cache.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::common::*;
use dpds_controller::api::{self, Deps};
use dpds_controller::frontend::Frontend;
use dpds_controller::supervisor::State;
use serde_json::{json, Value};

const HINTS: &str = "/api/host/hints";
const BDF0: &str = "0000:17:00.0";
const NOTE: &str = "firmware values read";

/// A host whose mlx5 NICs have an untuned firmware, a router with the startup task over it, and
/// a handle on the firmware state of NIC0's card.
fn warm_app() -> (TestApp, Arc<FakeExec>, Arc<Mutex<MstState>>) {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    let st = fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    fake_mst(&exec, "mstconfig", NIC1, MstState::untuned());
    (warm_over(host, State::Stopped), exec, st)
}

/// The router with the startup task, built while the emulation is in `state`.
fn warm_over(host: FakeHost, state: State) -> TestApp {
    let supervisor = FakeSupervisor::new();
    supervisor.force_status(status(state));
    let telemetry = FakeTelemetry::new();
    let runtime_dir = std::path::PathBuf::from("/run/dpds-controller-test");
    let dpds_bin = std::path::PathBuf::from("/usr/local/bin/dpds");
    let router = api::router_with_startup(
        Deps {
            roots: host.roots(),
            supervisor: supervisor.clone(),
            telemetry: telemetry.clone(),
            dpds_bin: dpds_bin.clone(),
            runtime_dir: runtime_dir.clone(),
            dpds_has_telemetry: true,
            is_root: false,
        },
        Frontend::embedded(),
    );
    TestApp {
        router,
        host,
        supervisor,
        telemetry,
        runtime_dir,
        dpds_bin,
    }
}

/// The firmware queries made so far for the device `bdf`, by either tool.
fn mst(exec: &FakeExec, bdf: &str) -> usize {
    exec.calls()
        .iter()
        .filter(|c| {
            matches!(c[0].as_str(), "mstconfig" | "mlxconfig")
                && c.get(2).map(String::as_str) == Some(bdf)
                && c.last().map(String::as_str) == Some("q")
        })
        .count()
}

/// Polls `cond` for up to 10 s of real time.
async fn until(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..1000 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// A GET that must not hang: the stored result, 200.
async fn get_ok(a: &TestApp, query: &str) -> Value {
    let r = tokio::time::timeout(Duration::from_secs(10), a.get(&format!("{HINTS}{query}")))
        .await
        .unwrap_or_else(|_| panic!("GET {query} did not answer within 10 s"));
    assert_eq!(r.status, 200, "GET {query}: {}", r.text());
    r.json()
}

async fn post_ok(a: &TestApp, query: &str, s: &Setup) -> Value {
    let r = a.post(&format!("{HINTS}{query}"), s.body()).await;
    assert_eq!(r.status, 200, "POST {query}: {}", r.text());
    r.json()
}

fn link_on(pci: &str) -> Setup {
    setup(vec![link("l", pci, pci)])
}

fn detail(h: &Value, id: &str) -> String {
    hint(h, id)["detail"].as_str().unwrap_or_default().into()
}

/// What a firmware query returns when the card is tuned.
fn tune(st: &Mutex<MstState>) {
    *st.lock().unwrap() = MstState::tuned();
}

/// A `mstconfig` query of NIC0's card that blocks until the gate opens (the call is recorded
/// first, so a test sees it enter). Dropping the gate opens it, so a failing test cannot hang.
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn closed_on(exec: &FakeExec) -> Gate {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let g = gate.clone();
        exec.on_fn(&["mstconfig", "-d", BDF0, "-e", "-j"], move |_| {
            let (open, cv) = &*g;
            let (_open, timeout) = cv
                .wait_timeout_while(open.lock().unwrap(), Duration::from_secs(30), |o| !*o)
                .unwrap();
            assert!(!timeout.timed_out(), "the gate was never opened");
            Out::JsonFile(mst_json(BDF0, &MstState::untuned()))
        });
        Gate(gate)
    }
    fn open(&self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.open();
    }
}

// ---- H1: the startup task ---------------------------------------------------------------------

#[tokio::test]
async fn the_startup_task_queries_the_firmware_of_every_mlx5_nic_once_without_a_request() {
    let (a, exec, _) = warm_app();
    until("the startup queries", || {
        mst(&exec, BDF0) >= 1 && mst(&exec, NIC1) >= 1
    })
    .await;
    settle().await;
    assert_eq!(mst(&exec, BDF0), 1, "{:?}", exec.calls());
    assert_eq!(mst(&exec, NIC1), 1, "{:?}", exec.calls());
    // no other card is queried: the tg3, the ice and the NVMe device are not mlx5 NICs
    for c in exec.calls() {
        if matches!(c[0].as_str(), "mstconfig" | "mlxconfig") {
            let dev = c.get(2).map(String::as_str);
            assert!(dev == Some(BDF0) || dev == Some(NIC1), "{c:?}");
        }
    }
    // requests add no query
    get_ok(&a, "").await;
    get_ok(&a, "").await;
    assert_eq!(mst(&exec, BDF0), 1);
    assert_eq!(mst(&exec, NIC1), 1);
}

#[tokio::test]
async fn a_running_startup_task_does_not_delay_the_rest_of_the_service() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(&exec, "mstconfig", NIC1, MstState::untuned());
    let gate = Gate::closed_on(&exec);
    let a = warm_over(host, State::Stopped);
    until("the blocked query", || mst(&exec, BDF0) >= 1).await;
    for path in ["/api/version", "/api/online", "/api/interfaces"] {
        let r = tokio::time::timeout(Duration::from_secs(5), a.get(path))
            .await
            .unwrap_or_else(|_| panic!("{path} waits for the startup task"));
        assert_eq!(r.status, 200, "{path}: {}", r.text());
    }
    gate.open();
}

#[tokio::test]
async fn a_failing_startup_query_does_not_stop_the_task_and_is_not_cached() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    fake_mst(&exec, "mstconfig", NIC1, MstState::untuned());
    exec.on(&["mstconfig", "-d", BDF0], Out::Fail(1, "boom".into()));
    let a = warm_over(host, State::Stopped);
    until("both queries", || {
        mst(&exec, BDF0) >= 1 && mst(&exec, NIC1) >= 1
    })
    .await;
    settle().await;
    assert_eq!(mst(&exec, BDF0), 1, "tried once, no retry loop");
    assert_eq!(mst(&exec, NIC1), 1, "the other NIC was still queried");
    get_ok(&a, "").await; // the task finished
                          // like today's results: the failed card is asked again by a request, the good one is cached
    let h = post_ok(&a, "", &link_on(NIC0)).await;
    assert_eq!(mst(&exec, BDF0), 2);
    assert_no_commands(&h, &nic_id("firmware", NIC0));
    let h = post_ok(&a, "", &link_on(NIC1)).await;
    assert_eq!(mst(&exec, NIC1), 1);
    assert!(detail(&h, &nic_id("firmware", NIC1)).contains(NOTE));
}

// ---- H2: GET answers the stored result ----------------------------------------------------------

#[tokio::test]
async fn get_answers_the_stored_result_without_recomputing() {
    let (a, exec, _) = warm_app();
    let first = get_ok(&a, "").await;
    let swappiness = hint(&first, "swappiness").clone();
    // the host changes after the stored result was computed
    a.host.sysctl("swappiness", Some("60"));
    let calls = exec.calls().len();
    for query in ["", "", "?recheck=false"] {
        let h = get_ok(&a, query).await;
        assert_eq!(h, first, "GET {query} is the stored answer");
        assert_eq!(hint(&h, "swappiness"), &swappiness);
    }
    assert_eq!(
        exec.calls().len(),
        calls,
        "no tool ran: {:?}",
        &exec.calls()[calls..]
    );
}

#[tokio::test]
async fn get_waits_for_a_running_startup_task_instead_of_starting_a_second_computation() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(&exec, "mstconfig", NIC1, MstState::untuned());
    let gate = Gate::closed_on(&exec);
    let a = warm_over(host, State::Stopped);
    until("the blocked query", || mst(&exec, BDF0) >= 1).await;

    let (first, second) = (a.get(HINTS), a.get(HINTS));
    tokio::pin!(first, second);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut first)
            .await
            .is_err(),
        "GET answered while the startup task still ran"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut second)
            .await
            .is_err(),
        "a second GET answered while the startup task still ran"
    );
    assert_eq!(mst(&exec, BDF0), 1, "{:?}", exec.calls());

    gate.open();
    let (r1, r2) = tokio::time::timeout(Duration::from_secs(10), async {
        (first.await, second.await)
    })
    .await
    .expect("the GETs answer once the task is done");
    assert_eq!(
        (r1.status, r2.status),
        (200, 200),
        "{} {}",
        r1.text(),
        r2.text()
    );
    assert_eq!(r1.json(), r2.json());
    assert!(r1.json().is_array());
    // one computation: one query per card, and nothing more for the answers
    assert_eq!(mst(&exec, BDF0), 1, "{:?}", exec.calls());
    assert_eq!(mst(&exec, NIC1), 1, "{:?}", exec.calls());
    let calls = exec.calls().len();
    get_ok(&a, "").await;
    assert_eq!(exec.calls().len(), calls);
}

#[tokio::test]
async fn get_recheck_true_recomputes_requeries_and_replaces_the_stored_result() {
    let (a, exec, st) = warm_app();
    let first = get_ok(&a, "").await;
    assert_eq!((mst(&exec, BDF0), mst(&exec, NIC1)), (1, 1));
    let fw = nic_id("firmware", NIC0);
    let before = post_ok(&a, "", &link_on(NIC0)).await;
    assert!(!commands_of(&before, &fw).is_empty(), "untuned: commands");
    // an administrator tuned the card and changed a sysctl
    tune(&st);
    a.host.sysctl("swappiness", Some("60"));

    let h = get_ok(&a, "?recheck=true").await;
    assert_ne!(
        hint(&h, "swappiness"),
        hint(&first, "swappiness"),
        "the host hints were computed again"
    );
    assert_eq!(mst(&exec, BDF0), 2, "{:?}", exec.calls());
    assert_eq!(
        mst(&exec, NIC1),
        2,
        "the firmware of all NICs is queried again"
    );
    // the new result is the stored one
    let calls = exec.calls().len();
    assert_eq!(get_ok(&a, "").await, h);
    assert_eq!(get_ok(&a, "?recheck=false").await, h);
    assert_eq!(exec.calls().len(), calls);
    // and the firmware cache has the new values
    let after = post_ok(&a, "", &link_on(NIC0)).await;
    assert_eq!(mst(&exec, BDF0), 2, "the POST uses the refreshed cache");
    assert_no_commands(&after, &fw);
}

#[tokio::test]
async fn get_recheck_true_on_a_failing_card_queries_it_and_still_answers() {
    let (a, exec, _) = warm_app();
    get_ok(&a, "").await;
    exec.on(&["mstconfig", "-d", BDF0], Out::Fail(1, "boom".into()));
    let h = get_ok(&a, "?recheck=true").await;
    assert!(h.is_array());
    assert_eq!(mst(&exec, BDF0), 2);
    assert_eq!(mst(&exec, NIC1), 2);
}

#[tokio::test]
async fn get_takes_recheck_by_the_rules_of_the_post() {
    let (a, exec, _) = warm_app();
    let first = get_ok(&a, "").await;
    let calls = exec.calls().len();
    for q in [
        "?recheck=1",
        "?recheck=0",
        "?recheck=yes",
        "?recheck=True",
        "?recheck=TRUE",
        "?recheck=False",
        "?recheck=truee",
        "?recheck=",
        "?recheck",
        "?recheck=true&recheck=true",
        "?recheck=false&recheck=false",
        "?recheck=true&recheck=false",
        "?recheck=true&x=1",
        "?x=1&recheck=true",
        "?x=1",
        "?x",
        "?Recheck=true",
    ] {
        let r = a.get(&format!("{HINTS}{q}")).await;
        assert_eq!(r.status, 400, "GET {q}: {}", r.text());
        assert!(r.message().contains("recheck"), "GET {q}: {}", r.message());
    }
    assert_eq!(exec.calls().len(), calls, "nothing ran for a 400");
    assert_eq!(
        get_ok(&a, "").await,
        first,
        "the stored result is untouched"
    );
}

#[tokio::test]
async fn openapi_documents_recheck_on_the_hints_get_and_post() {
    let a = TestApp::new(FakeHost::bare_metal());
    let doc = a.get("/api/openapi.json").await.json();
    for method in ["get", "post"] {
        let params = doc["paths"][HINTS][method]["parameters"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let recheck = params
            .iter()
            .find(|p| p["name"] == "recheck")
            .unwrap_or_else(|| panic!("no `recheck` parameter on {method} {HINTS}: {params:?}"));
        assert_eq!(recheck["in"], "query", "{method}: {recheck}");
        assert_ne!(recheck["required"], json!(true), "{method}: {recheck}");
        assert!(
            recheck["description"]
                .as_str()
                .is_some_and(|d| !d.trim().is_empty()),
            "{method}: described: {recheck}"
        );
    }
}

// ---- H3: POST with the warm cache -----------------------------------------------------------------

#[tokio::test]
async fn post_uses_the_warm_cache_and_runs_no_firmware_query() {
    let (a, exec, _) = warm_app();
    get_ok(&a, "").await;
    let fw = nic_id("firmware", NIC0);
    for s in [link_on(NIC0), setup(vec![link("l", NIC0, NIC1)])] {
        let h = post_ok(&a, "", &s).await;
        assert_eq!(
            (mst(&exec, BDF0), mst(&exec, NIC1)),
            (1, 1),
            "{:?}",
            exec.calls()
        );
        assert_eq!(
            commands_of(&h, &fw),
            [
                format!("mstconfig -d {BDF0} -y set ADVANCED_PCI_SETTINGS=1"),
                format!("mstconfig -d {BDF0} -y set MAX_ACC_OUT_READ=256 CQE_COMPRESSION=1"),
            ]
        );
        assert!(detail(&h, &fw).contains(NOTE), "{}", detail(&h, &fw));
    }
}

#[tokio::test]
async fn post_recheck_true_queries_only_the_posted_nics_again() {
    let (a, exec, st) = warm_app();
    get_ok(&a, "").await;
    tune(&st);
    let h = post_ok(&a, "?recheck=true", &link_on(NIC0)).await;
    assert_eq!(mst(&exec, BDF0), 2);
    assert_eq!(mst(&exec, NIC1), 1, "NIC1 is not in the posted links");
    let fw = nic_id("firmware", NIC0);
    assert_no_commands(&h, &fw);
    assert!(
        !detail(&h, &fw).contains(NOTE),
        "a fresh read has no cache note"
    );
}

// ---- H4: the tools rule ---------------------------------------------------------------------------

#[tokio::test]
async fn a_recheck_while_an_emulation_is_busy_computes_without_tools_and_keeps_the_cache() {
    for state in [State::Starting, State::Running, State::Stopping] {
        let (a, exec, _) = warm_app();
        let first = get_ok(&a, "").await;
        let swappiness = hint(&first, "swappiness").clone();
        a.supervisor.force_status(status(state));
        a.host.sysctl("swappiness", Some("60"));
        let calls = exec.calls().len();
        let h = get_ok(&a, "?recheck=true").await;
        assert_eq!(
            exec.calls().len(),
            calls,
            "{state:?}: {:?}",
            &exec.calls()[calls..]
        );
        assert_ne!(
            hint(&h, "swappiness"),
            &swappiness,
            "{state:?}: computed again"
        );
        // plain GETs and POSTs run no tool either
        get_ok(&a, "").await;
        post_ok(&a, "?recheck=true", &link_on(NIC0)).await;
        assert_eq!(exec.calls().len(), calls, "{state:?}");
        // the firmware cache survived
        a.supervisor.force_status(status(State::Stopped));
        post_ok(&a, "", &link_on(NIC0)).await;
        assert_eq!(mst(&exec, BDF0), 1, "{state:?}");
    }
}

#[tokio::test]
async fn a_startup_task_with_an_emulation_running_runs_no_tool_and_a_later_recheck_does() {
    let host = FakeHost::bare_metal();
    let exec = host.exec();
    fake_mst(&exec, "mstconfig", BDF0, MstState::untuned());
    fake_mst(&exec, "mstconfig", NIC1, MstState::untuned());
    let a = warm_over(host, State::Running);
    let h = get_ok(&a, "").await;
    assert!(h.is_array());
    settle().await;
    assert!(exec.calls().is_empty(), "{:?}", exec.calls());
    a.supervisor.force_status(status(State::Stopped));
    get_ok(&a, "?recheck=true").await;
    assert_eq!(
        (mst(&exec, BDF0), mst(&exec, NIC1)),
        (1, 1),
        "{:?}",
        exec.calls()
    );
}
