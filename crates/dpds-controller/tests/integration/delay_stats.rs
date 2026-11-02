//! C7: the per-direction `delay_stats` switch in the controller.
//!
//! - `ConfigDto` gets `delay_stats: bool` (default true when absent), kept by the API
//!   and the store, and written into the links file as `"delay_stats": [bool, ..]` per
//!   link (one entry per direction; when every direction is on the key may be absent);
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
    let from_core = dpds_controller::dto::ConfigDto::from_core(&c.to_core().unwrap());
    assert!(
        on(&serde_json::to_value(from_core).unwrap()),
        "from_core is on"
    );
}

#[tokio::test]
async fn the_api_keeps_delay_stats_per_direction() {
    let a = TestApp::new(FakeHost::bare_metal());
    let mut body = link_body("ab", &[NIC0, NIC1]);
    body["configs"][1]["delay_stats"] = json!(false);
    let r = a.post("/api/links", body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let v = r.json();
    assert!(on(&v["configs"][0]), "{v}");
    assert_eq!(v["configs"][1]["delay_stats"], false, "{v}");
    let id = v["id"].as_u64().unwrap();
    assert_eq!(a.get(&format!("/api/links/{id}")).await.json(), v);
    // survives a controller restart
    let mut a = a;
    a.restart();
    assert_eq!(a.get(&format!("/api/links/{id}")).await.json(), v);
}

fn plans(link: &str) -> Vec<DirectionPlan> {
    (0..2)
        .map(|d| DirectionPlan {
            dir: d,
            link: link.into(),
            rx: format!("net_null{d}"),
            tx: format!("net_null{}", 1 - d),
            lcore: 2 + d,
        })
        .collect()
}

#[test]
fn the_links_file_carries_the_switch_per_direction() {
    let mut l = link(1, "ab", &["net_null0", "net_null1"], None);
    let mut off = cfg_json(1.0e6, 0.0);
    off["delay_stats"] = json!(false);
    l.configs[1] = dto_from(off);
    let v = links_file_json(&[l], &plans("ab")).unwrap();
    assert_eq!(v["links"][0]["delay_stats"], json!([true, false]), "{v}");

    // two links, the second all on
    let a = link(1, "ab", &["net_null0", "net_null1"], None);
    let mut b = link(2, "c", &["net_null2"], None);
    let mut off = cfg_json(1.0e6, 0.0);
    off["delay_stats"] = json!(false);
    b.configs[0] = dto_from(off);
    let mut p = plans("ab");
    p.push(DirectionPlan {
        dir: 2,
        link: "c".into(),
        rx: "net_null2".into(),
        tx: "net_null2".into(),
        lcore: 4,
    });
    let v = links_file_json(&[a, b], &p).unwrap();
    let first = &v["links"][0]["delay_stats"];
    assert!(first.is_null() || *first == json!([true, true]), "{v}");
    assert_eq!(v["links"][1]["delay_stats"], json!([false]), "{v}");
}

#[test]
fn an_all_on_link_writes_no_false() {
    let l = link(1, "ab", &["net_null0", "net_null1"], None);
    let v = links_file_json(&[l], &plans("ab")).unwrap();
    let f = &v["links"][0]["delay_stats"];
    assert!(f.is_null() || *f == json!([true, true]), "{v}");
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
        imissed: 0,
        rx_nombuf: 0,
    }
}

/// Direction 0 on, direction 1 off, like a `dpds` started with `"delay_stats": [true, false]`.
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
    s.hists[1] = empty_hist_json();
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
