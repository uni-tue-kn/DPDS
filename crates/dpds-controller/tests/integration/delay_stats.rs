//! C7: the per-direction `delay_stats` switch in the controller.
//!
//! - `ConfigDto` gets `delay_stats: bool` (default true when absent), kept by the API
//!   (reported in the running configuration) and written into the v2 links file as `"delay_stats": false` of a link
//!   (one direction per link; when the link has it on the key may be absent or `true`);
//! - `/dpds/ports` reports `dir<d>_delay_stats` (0/1; absent in older builds = on);
//!   `/api/stats` gives every direction a `delay_stats` bool and, for an off direction,
//!   no `delay_*` / `lateness_*` values (absent or null) while counters, rates and
//!   drop shares stay;
//! - `/api/stats/{dir}/delay_histogram` of an off direction is either 404 with a
//!   `{"message"}` or 200 with `delay_stats: false` and no bins (ASSUMPTION: the
//!   spec leaves the choice open, both are accepted).

use crate::common::*;
use dpds_controller::eal::links_file_json;
use dpds_controller::lcores::DirectionPlan;
use dpds_controller::stats::{compute_stats, Snapshot};
use serde_json::{json, Value};

const DELAY_FIELDS: [&str; 6] = [
    "delay_mean_ns",
    "delay_std_ns",
    "delay_min_ns",
    "delay_max_ns",
    "lateness_mean_ns",
    "lateness_max_ns",
];

fn on(v: &Value) -> bool {
    v.get("delay_stats").is_none_or(|x| x == true)
}

// ---- DTO, API, links file ---------------------------------------------------------------

#[test]
fn delay_stats_default_to_true_and_roundtrip_when_false() {
    let c = dto_from(cfg_json(1.0e6, 0.0));
    assert!(on(&serde_json::to_value(&c).unwrap()));
    let mut j = cfg_json(1.0e6, 0.0);
    j["delay_stats"] = json!(false);
    let c = dto_from(j);
    let back = serde_json::to_value(&c).unwrap();
    assert_eq!(back["delay_stats"], false);
    assert!(c.to_core().is_ok());
}

#[tokio::test]
async fn the_api_keeps_delay_stats_per_direction() {
    let a = TestApp::new(FakeHost::bare_metal());
    let mut off = link("a-to-b", NIC0, NIC1);
    off["config"]["delay_stats"] = json!(false);
    let r = a.start_links(vec![off, link("b-to-a", NIC1, NIC0)]).await;
    assert_eq!(r.status, 202, "{}", r.text());
    // the links file dpds receives carries the switch of the link that has it off
    let plan = &a.supervisor.plans()[0];
    assert_eq!(plan.links_json["links"][0]["delay_stats"], json!(false));
    assert!(is_on(&plan.links_json["links"][1]["delay_stats"]));
    // and the running configuration reports it per link
    let v = a.get("/api/emulation").await.json();
    assert_eq!(
        v["config"]["links"][0]["config"]["delay_stats"], false,
        "{v}"
    );
    assert!(on(&v["config"]["links"][1]["config"]), "{v}");
}

fn plan(dir: u32, link: &str, rx: &str, tx: &str, lcore: u32) -> DirectionPlan {
    DirectionPlan {
        dir,
        link: link.into(),
        rx: rx.into(),
        tx: tx.into(),
        lcore,
    }
}

fn is_on(v: &Value) -> bool {
    v.is_null() || *v == json!(true)
}

#[test]
fn the_links_file_carries_the_switch_per_link() {
    let mut off = cfg_json(1.0e6, 0.0);
    off["delay_stats"] = json!(false);
    let a = model_link(1, &link_cfg("a-to-b", "net_null0", "net_null1", off));
    let b = model_link(2, &link("b-to-a", "net_null1", "net_null0"));
    let c = model_link(3, &link("loop", "net_null2", "net_null2"));
    let p = vec![
        plan(0, "a-to-b", "net_null0", "net_null1", 2),
        plan(1, "b-to-a", "net_null1", "net_null0", 3),
        plan(2, "loop", "net_null2", "net_null2", 4),
    ];
    let v = links_file_json(&[a, b, c], &p).unwrap();
    assert_eq!(v["links"][0]["delay_stats"], json!(false), "{v}");
    assert!(is_on(&v["links"][1]["delay_stats"]), "{v}");
    assert!(is_on(&v["links"][2]["delay_stats"]), "{v}");
}

#[test]
fn an_all_on_link_writes_no_false() {
    let l = model_link(1, &link("a-to-b", "net_null0", "net_null1"));
    let v = links_file_json(&[l], &[plan(0, "a-to-b", "net_null0", "net_null1", 2)]).unwrap();
    assert!(is_on(&v["links"][0]["delay_stats"]), "{v}");
}

#[tokio::test]
async fn openapi_knows_the_delay_stats_field() {
    let a = TestApp::new(FakeHost::bare_metal());
    let v = a.get("/api/openapi.json").await.json();
    assert!(
        v["components"]["schemas"]["ConfigDto"]["properties"]
            .get("delay_stats")
            .is_some(),
        "{}",
        v["components"]["schemas"]["ConfigDto"]
    );
}

// ---- stats and histogram -----------------------------------------------------------------

fn sample(rx: u64) -> DirSample {
    DirSample {
        uptime_ns: 2_000_000_000,
        rx_pkts: rx,
        tx_sent: rx,
        loss: 0,
        limiter: 0,
        full: 0,
        tx_drops: 0,
    }
}

fn port(bytes: u64) -> PortSample {
    PortSample {
        ibytes: bytes,
        obytes: bytes,
        rx_missed_errors: 0,
        rx_mbuf_allocation_errors: 0,
    }
}

/// Link 0 (dir 0) with delay stats on, link 1 (dir 1) with `"delay_stats": false`.
fn off_snapshot(uptime_step: u64) -> Snapshot {
    let mut s = snapshot(
        [
            DirSample {
                uptime_ns: 1_000_000_000 + uptime_step,
                ..sample(1_000 + uptime_step / 1_000_000)
            },
            DirSample {
                uptime_ns: 1_000_000_000 + uptime_step,
                ..sample(500 + uptime_step / 1_000_000)
            },
        ],
        [port(100_000 + uptime_step), port(50_000 + uptime_step)],
    );
    s.ports["dir0_delay_stats"] = json!(1);
    s.ports["dir1_delay_stats"] = json!(0);
    let obj = s.dirs[1].as_object_mut().unwrap();
    for k in DELAY_FIELDS {
        obj.remove(k);
    }
    set_hist(&mut s, 1, empty_hist_json());
    s
}

#[test]
fn an_off_direction_has_no_delay_values_but_everything_else() {
    let v = serde_json::to_value(compute_stats(
        Some(&off_snapshot(0)),
        &off_snapshot(1_000_000_000),
    ))
    .unwrap();
    let (d0, d1) = (&v["directions"][0], &v["directions"][1]);
    assert_eq!(d0["delay_stats"], true, "{d0}");
    assert_eq!(d1["delay_stats"], false, "{d1}");
    for k in DELAY_FIELDS {
        assert!(!absent(d0, k), "dir 0 keeps {k}: {d0}");
        assert!(absent(d1, k), "dir 1 must not report {k}: {d1}");
    }
    // counters, rates and shares are unaffected
    assert_eq!(d1["counters"]["rx_pkts"], 1_000 / 2 + 1_000);
    assert!(f(d1, "rx_pps") > 0.0, "{d1}");
    assert!(d1.get("loss_pct").is_some());
}

#[test]
fn a_missing_switch_in_ports_means_on() {
    // an older dpds: no dir<d>_delay_stats keys at all
    let s = snapshot([sample(10), sample(20)], [port(1_000), port(2_000)]);
    assert!(s.ports.get("dir0_delay_stats").is_none());
    let v = serde_json::to_value(compute_stats(None, &s)).unwrap();
    for d in v["directions"].as_array().unwrap() {
        assert_eq!(d["delay_stats"], true, "{d}");
        assert!(!absent(d, "delay_mean_ns"), "{d}");
    }
}

#[tokio::test]
async fn the_endpoints_report_the_off_direction() {
    let a = TestApp::new(FakeHost::bare_metal());
    a.telemetry
        .set(Some(off_snapshot(0)), off_snapshot(1_000_000_000));
    let v = a.get("/api/stats").await.json();
    assert_eq!(v["directions"][0]["delay_stats"], true);
    assert_eq!(v["directions"][1]["delay_stats"], false);
    for k in DELAY_FIELDS {
        assert!(absent(&v["directions"][1], k), "{k}");
    }

    // histogram: the on direction as before ...
    let r = a.get("/api/stats/0/delay_histogram").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["bins"].as_array().unwrap().len(), 256);
    // ... the off direction has none
    let r = a.get("/api/stats/1/delay_histogram").await;
    match r.status {
        404 => {
            r.message();
        }
        200 => {
            let h = r.json();
            assert_eq!(h["delay_stats"], false, "{h}");
            assert!(h["bins"].as_array().is_none_or(Vec::is_empty), "{h}");
        }
        other => panic!("histogram of an off direction: status {other}"),
    }
}
