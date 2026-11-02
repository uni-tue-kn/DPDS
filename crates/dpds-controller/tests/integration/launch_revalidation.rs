//! Spec v7 S4: `POST /api/emulation/start` re-validates what is stored (ports,
//! `eal_extra_args`, `mlx5_devargs`) with the same rules as the REST edits, so a
//! hand-edited `state.json` or one written by an older version (when `net_pcap` or
//! `,key=value` vdevs were still valid) cannot put a forbidden argument on the `dpds`
//! command line. The start is refused (400, or 409) with a `{"message"}` that names the
//! offending value; nothing is spawned and the emulation stays stopped. The controller
//! itself still starts on such a file (the refusal is at start).

use crate::common::*;
use dpds_controller::model::StoredState;
use dpds_controller::store;
use serde_json::json;

/// A controller restarted over a state file that was written behind its back.
fn app_with(state: &StoredState) -> TestApp {
    let mut a = TestApp::new(FakeHost::bare_metal());
    store::save(a.state_dir.path(), state).unwrap();
    a.restart();
    a
}

async fn assert_refused(a: &TestApp, named: &str) {
    let r = a.post("/api/emulation/start", json!({})).await;
    assert!(
        r.status == 400 || r.status == 409,
        "start must be refused, got {}: {}",
        r.status,
        r.text()
    );
    let m = r.message();
    assert!(m.contains(named), "the message names {named:?}: {m}");
    assert!(a.supervisor.plans().is_empty(), "nothing may be spawned");
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

#[tokio::test]
async fn a_valid_stored_state_starts() {
    // control: the refusals below are about the stored values, not about this setup
    let a = app_with(&state_with(vec![link(1, "ok", &["net_null0"], None)]));
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(a.supervisor.plans().len(), 1);
}

#[tokio::test]
async fn stored_ports_that_the_rules_forbid_refuse_the_start() {
    for bad in [
        "net_pcap0,rx_pcap=/tmp/in.pcap,tx_pcap=/tmp/out.pcap",
        "net_pcap0,tx_pcap=/etc/cron.d/x",
        "net_pcap0",
        "net_null0,size=64",
        "net_ring0,iface=eth0",
        "--vdev=net_null1",
        "net_null0 --vdev=net_null1",
        "0000:65:00.9",
        "",
    ] {
        let a = app_with(&state_with(vec![link(1, "old", &[bad], None)]));
        let r = a.post("/api/emulation/start", json!({})).await;
        assert!(
            r.status == 400 || r.status == 409,
            "{bad:?}: {} {}",
            r.status,
            r.text()
        );
        let m = r.message();
        assert!(m.contains(bad.trim()) || m.contains("port"), "{bad:?}: {m}");
        assert!(a.supervisor.plans().is_empty(), "{bad:?} was spawned");
        assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
    }
}

#[tokio::test]
async fn a_bad_second_port_or_a_bad_second_link_refuses_the_start() {
    let a = app_with(&state_with(vec![
        link(1, "fine", &["net_null0"], None),
        link(2, "mixed", &["net_null1", "net_pcap2,iface=eth0"], None),
    ]));
    assert_refused(&a, "net_pcap2").await;
}

#[tokio::test]
async fn stored_eal_extra_args_outside_the_allowlist_refuse_the_start() {
    for (args, named) in [
        (vec!["-d", "/tmp/evil.so"], "-d"),
        (vec!["--file-prefix=x"], "--file-prefix"),
        (vec!["--vdev", "net_pcap0,tx_pcap=/etc/x"], "--vdev"),
        (vec!["--no-huge", "-m", "abc"], "abc"),
    ] {
        let mut s = state_with(vec![link(1, "l", &["net_null0"], None)]);
        s.settings.eal_extra_args = args.iter().map(|a| a.to_string()).collect();
        let a = app_with(&s);
        assert_refused(&a, named).await;
    }
}

#[tokio::test]
async fn stored_mlx5_devargs_outside_the_allowlist_refuse_the_start() {
    for (devargs, named) in [
        ("mprq_en=1", "mprq"),
        ("txq_mpw_en=1,mprq_en=1", "mprq"),
        ("bogus_key=1", "bogus_key"),
    ] {
        let mut s = state_with(vec![link(1, "l", &["net_null0"], None)]);
        s.settings.mlx5_devargs = devargs.into();
        let a = app_with(&s);
        assert_refused(&a, named).await;
    }
}

#[tokio::test]
async fn the_refusal_does_not_block_a_later_start_after_the_state_is_fixed() {
    let a = app_with(&state_with(vec![link(
        1,
        "old",
        &["net_pcap0,iface=eth0"],
        None,
    )]));
    assert!(a.post("/api/emulation/start", json!({})).await.status >= 400);
    let id = a.get("/api/links").await.json()[0]["id"].as_u64().unwrap();
    let r = a
        .put(
            &format!("/api/links/{id}"),
            link_body("old", &["net_null0"]),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let r = a.post("/api/emulation/start", json!({})).await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(a.supervisor.plans().len(), 1);
}
