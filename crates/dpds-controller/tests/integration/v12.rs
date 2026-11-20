//! Controller: the required correlation initialization rate `delayer.initialization_rate_pps`, the
//! dpds telemetry dict limit.
//!
//! Contract:
//! - a present `delayer` requires `initialization_rate_pps`: a body without it is a 400 whose
//!   message names the field (a 422 is mapped to 400 by the controller); a value that is not
//!   finite or below 1 is a 400 naming the field. It is reported back by `GET /api/emulation`
//!   (the running configuration) and written into the links file. (A missing delayer needs no
//!   rate: see `optional_stages.rs`.)
//! - dpds' `/dpds/ports` telemetry dict holds `3 + 5 * distinct ports + 9 * links` entries, at
//!   most 256. ASSUMPTION: 400 or 409, the message contains `256`. Boundaries used: a ring of 18
//!   links over 18 ports is 255 and fits, a ring of 19 is 269 and fails; 18 hairpins fit, 19
//!   fail; 13 one-way links over 26 ports fit (250), 14 fail (269); 17 links over 20 ports (three
//!   chains) is exactly 256 and fits, one more link or one more port than that fails.
//! - OpenAPI: every schema with a `worker` property gives it `minimum: 0`.

use crate::common::*;
use serde_json::{json, Value};

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

// ---- initialization_rate_pps -----------------------------------------------------------------

#[tokio::test]
async fn a_delayer_without_the_rate_is_a_400_naming_the_field() {
    let a = app();
    let s = setup(vec![link_cfg("net_null0", "net_null1", cfg_without_rate())]);
    assert_400_naming(&a.start(&s).await, "start", RATE);
    assert_400_naming(&a.validate(&s).await, "validate", RATE);
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn an_invalid_rate_is_a_400_naming_the_field() {
    let a = app();
    for bad in [
        json!(0),
        json!(0.5),
        json!(0.999),
        json!(-1),
        json!(-15.0e6),
    ] {
        for correlation in [false, true] {
            let mut c = cfg_rate(bad.clone());
            c["delayer"]["activate_correlation"] = json!(correlation);
            let s = setup(vec![link_cfg("net_null2", "net_null3", c)]);
            let r = a.start(&s).await;
            assert_400_naming(&r, &format!("rate {bad}, correlation {correlation}"), RATE);
        }
    }
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn a_rate_below_one_is_refused_and_exactly_one_is_accepted() {
    let a = app();
    let s = setup(vec![link_cfg(
        "net_null0",
        "net_null1",
        cfg_rate(json!(0.9999999)),
    )]);
    assert_400_naming(&a.start(&s).await, "0.9999999", RATE);
    let s = setup(vec![link_cfg("net_null0", "net_null1", cfg_rate(json!(1)))]);
    let r = a.start(&s).await;
    assert_eq!(r.status, 202, "{}", r.text());
}

#[tokio::test]
async fn the_rate_of_every_link_is_reported_and_written_into_the_links_file_at_start() {
    let a = TestApp::new(big_host());
    let r = a
        .start_links(vec![
            link_cfg("net_null0", "net_null1", cfg_rate(json!(250_000.0))),
            link_cfg("net_null1", "net_null0", cfg_rate(json!(7.5e6))),
        ])
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
    let plans = a.supervisor.plans();
    assert_eq!(plans.len(), 1);
    let links = &plans[0].links_json["links"];
    assert_eq!(links[0]["ingress"], "net_null0");
    assert_eq!(links[0]["config"]["delayer"][RATE], json!(250_000.0));
    assert_eq!(links[1]["ingress"], "net_null1");
    assert_eq!(links[1]["config"]["delayer"][RATE], json!(7.5e6));
    // and the running configuration says what each link started with
    let v = a.get("/api/emulation").await.json();
    let running = &v["config"]["links"];
    assert_eq!(
        running[0]["config"]["delayer"][RATE],
        json!(250_000.0),
        "{v}"
    );
    assert_eq!(running[1]["config"]["delayer"][RATE], json!(7.5e6), "{v}");
}

// ---- the dpds telemetry dict limit, 3 + 5 * ports + 9 * links <= 256 -------------------------

type Pair = (String, String);

fn nul(i: usize) -> String {
    format!("net_null{i}")
}

fn bodies(links: &[Pair]) -> Vec<Value> {
    links.iter().map(|(i, o)| link(i, o)).collect()
}

/// `n` links closing a ring over `n` ports.
fn ring(n: usize) -> Vec<Pair> {
    (0..n).map(|i| (nul(i), nul((i + 1) % n))).collect()
}

/// `n` hairpins over `n` ports.
fn hairpins(n: usize) -> Vec<Pair> {
    (0..n).map(|i| (nul(i), nul(i))).collect()
}

/// `n` one-way links over `2n` ports.
fn one_way(n: usize) -> Vec<Pair> {
    (0..n).map(|i| (nul(2 * i), nul(2 * i + 1))).collect()
}

/// Three chains of 6, 6 and 5 links over ports 0..=6, 7..=13 and 14..=19: 20 ports and 17
/// links, 3 + 100 + 153 = 256 entries.
fn three_chains() -> Vec<Pair> {
    let mut v = Vec::new();
    for (first, len) in [(0, 6), (7, 6), (14, 5)] {
        v.extend((0..len).map(|i| (nul(first + i), nul(first + i + 1))));
    }
    v
}

async fn fits(links: &[Pair]) -> Reply {
    TestApp::new(big_host()).start_links(bodies(links)).await
}

#[tokio::test]
async fn a_ring_of_18_links_fits_and_a_ring_of_19_does_not() {
    // 18 links over 18 ports: 3 + 90 + 162 = 255
    let r = fits(&ring(18)).await;
    assert_eq!(r.status, 202, "{}", r.text());
    // 19 links over 19 ports: 3 + 95 + 171 = 269
    let a = TestApp::new(big_host());
    let r = a.start_links(bodies(&ring(19))).await;
    assert_refused_naming(&r, "ring of 19", "256");
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn eighteen_hairpins_fit_and_the_nineteenth_does_not() {
    let links = hairpins(19);
    assert_eq!(fits(&links[..18]).await.status, 202);
    assert_refused_naming(&fits(&links).await, "19 hairpins", "256");
}

#[tokio::test]
async fn thirteen_one_way_links_fit_and_the_fourteenth_does_not() {
    let links = one_way(14);
    assert_eq!(fits(&links[..13]).await.status, 202);
    assert_refused_naming(&fits(&links).await, "14 one-way links", "256");
}

#[tokio::test]
async fn exactly_256_entries_fit_and_one_more_link_or_port_does_not() {
    let base = three_chains(); // 20 ports, 17 links: 256
    assert_eq!(fits(&base).await.status, 202);

    // a link between existing ports (end of chain 1 -> start of chain 2) adds 9: 265
    let mut bridged = base.clone();
    bridged.push((nul(6), nul(7)));
    assert_refused_naming(&fits(&bridged).await, "bridge link", "256");
    // a hairpin on a new port adds 14
    let mut extra = base.clone();
    extra.push((nul(40), nul(40)));
    assert_refused_naming(&fits(&extra).await, "extra hairpin", "256");
}

#[tokio::test]
async fn the_limit_counts_the_ports_of_the_body_once() {
    // same link count, one more distinct port: link 4 goes to nul(0) (a port in use) or to a
    // new one
    let mut same = three_chains();
    same[4] = (nul(4), nul(0));
    // nul(0) is the ingress of link 0 only, so using it as an egress is legal
    assert_eq!(fits(&same).await.status, 202);
    let mut new_port = three_chains();
    new_port[4] = (nul(4), nul(99));
    assert_refused_naming(&fits(&new_port).await, "a new port makes 21 ports", "256");
}

#[tokio::test]
async fn validate_applies_the_limit_too() {
    let a = TestApp::new(big_host());
    let r = a.validate(&setup(bodies(&ring(19)))).await;
    assert_refused_naming(&r, "validate ring of 19", "256");
    let r = a.validate(&setup(bodies(&ring(18)))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["directions"].as_array().unwrap().len(), 18);
}

// ---- OpenAPI -----------------------------------------------------------------------------

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
