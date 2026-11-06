//! Controller: the required correlation initialization rate `delayer.initialization_rate_pps`, the
//! link-name rules and the dpds telemetry dict limit.
//!
//! Contract:
//! - a present `delayer` requires `initialization_rate_pps`: a body without it is a 400 whose
//!   message names the field (a 422 is mapped to 400 by the controller); a value that is not
//!   finite or below 1 is a 400 naming the field. It is reported back by `GET /api/emulation`
//!   (the running configuration) and written into the links file. (A missing delayer needs no
//!   rate: see `optional_stages.rs`.)
//! - link names are 1 to 32 characters of `A-Za-z0-9_.-`, unique within the body: a refused
//!   start is 400 or 409 and the message names the name or says "link name"/"already exists".
//! - dpds' `/dpds/ports` telemetry dict holds `3 + 5 * distinct ports + 10 * links` entries, at
//!   most 256. ASSUMPTION: 400 or 409, the message contains `256`. Boundaries used
//!   (5 * (ports + 2 * links) + 3): a ring of 16 links over 16 ports is 243 and fits, a ring of
//!   17 is 258 and fails; 16 hairpins fit, 17 fail; 12 one-way links over 24 ports fit (243), 13
//!   fail (263); 18 ports with 16 links (two chains) is exactly 253 and fits, one more link or
//!   one more port than that fails. (A total of 254 to 257 cannot occur: totals are 3 mod 5.)
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
    let s = setup(vec![link_cfg(
        "x",
        "net_null0",
        "net_null1",
        cfg_without_rate(),
    )]);
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
            let s = setup(vec![link_cfg("bad", "net_null2", "net_null3", c)]);
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
        "low",
        "net_null0",
        "net_null1",
        cfg_rate(json!(0.9999999)),
    )]);
    assert_400_naming(&a.start(&s).await, "0.9999999", RATE);
    let s = setup(vec![link_cfg(
        "one",
        "net_null0",
        "net_null1",
        cfg_rate(json!(1)),
    )]);
    let r = a.start(&s).await;
    assert_eq!(r.status, 202, "{}", r.text());
}

#[tokio::test]
async fn the_rate_of_every_link_is_reported_and_written_into_the_links_file_at_start() {
    let a = TestApp::new(big_host());
    let r = a
        .start_links(vec![
            link_cfg("slow", "net_null0", "net_null1", cfg_rate(json!(250_000.0))),
            link_cfg("fast", "net_null1", "net_null0", cfg_rate(json!(7.5e6))),
        ])
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
    let plans = a.supervisor.plans();
    assert_eq!(plans.len(), 1);
    let links = &plans[0].links_json["links"];
    assert_eq!(links[0]["name"], "slow");
    assert_eq!(links[0]["config"]["delayer"][RATE], json!(250_000.0));
    assert_eq!(links[1]["name"], "fast");
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

// ---- link names ----------------------------------------------------------------------------

#[tokio::test]
async fn a_link_name_the_rules_forbid_refuses_the_start() {
    let too_long = "n".repeat(33);
    for bad in [
        "",
        "has space",
        "slash/name",
        "semi;colon",
        "quote\"d",
        "uml\u{e4}ut",
        "new\nline",
        too_long.as_str(),
    ] {
        let a = TestApp::new(big_host());
        let s = setup(vec![
            link("fine", "net_null0", "net_null0"),
            link(bad, "net_null1", "net_null1"),
        ]);
        let r = a.start(&s).await;
        assert!(
            r.status == 400 || r.status == 409,
            "{bad:?}: {} {}",
            r.status,
            r.text()
        );
        let m = r.message();
        assert!(
            m.contains("link name") || (!bad.is_empty() && m.contains(bad)),
            "{bad:?}: the message names the rule or the name: {m}"
        );
        assert!(a.supervisor.plans().is_empty(), "{bad:?} was spawned");
        assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
    }
}

#[tokio::test]
async fn names_of_one_to_32_valid_characters_start() {
    let name = "aZ09_.-x".repeat(4);
    assert_eq!(name.len(), 32);
    for n in [name.as_str(), "a", "a_b.c-D9"] {
        let a = TestApp::new(big_host());
        let r = a.start_links(vec![link(n, "net_null0", "net_null1")]).await;
        assert_eq!(r.status, 202, "{n}: {}", r.text());
    }
}

#[tokio::test]
async fn duplicate_link_names_are_a_409_naming_the_name() {
    let a = TestApp::new(big_host());
    let s = setup(vec![
        link("twin", "net_null0", "net_null1"),
        link("twin", "net_null2", "net_null3"),
    ]);
    let r = a.start(&s).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().contains("twin"), "{}", r.text());
    assert_eq!(a.validate(&s).await.status, 409);
    assert!(a.supervisor.plans().is_empty(), "nothing may be spawned");
}

// ---- the dpds telemetry dict limit, 3 + 5 * ports + 10 * links <= 256 ------------------------

type Triple = (String, String, String);

fn nul(i: usize) -> String {
    format!("net_null{i}")
}

fn bodies(links: &[Triple]) -> Vec<Value> {
    links.iter().map(|(n, i, o)| link(n, i, o)).collect()
}

/// `n` links `r0..` closing a ring over `n` ports.
fn ring(n: usize) -> Vec<Triple> {
    (0..n)
        .map(|i| (format!("r{i}"), nul(i), nul((i + 1) % n)))
        .collect()
}

/// `n` hairpins over `n` ports.
fn hairpins(n: usize) -> Vec<Triple> {
    (0..n).map(|i| (format!("h{i}"), nul(i), nul(i))).collect()
}

/// `n` one-way links over `2n` ports.
fn one_way(n: usize) -> Vec<Triple> {
    (0..n)
        .map(|i| (format!("w{i}"), nul(2 * i), nul(2 * i + 1)))
        .collect()
}

/// Two chains: 9 links over ports 0..=9 and 7 links over ports 10..=17: 18 ports and 16 links,
/// 3 + 90 + 160 = 253 entries.
fn two_chains() -> Vec<Triple> {
    let mut v: Vec<Triple> = (0..9)
        .map(|i| (format!("a{i}"), nul(i), nul(i + 1)))
        .collect();
    v.extend((0..7).map(|i| (format!("b{i}"), nul(10 + i), nul(11 + i))));
    v
}

async fn fits(links: &[Triple]) -> Reply {
    TestApp::new(big_host()).start_links(bodies(links)).await
}

#[tokio::test]
async fn a_ring_of_16_links_fits_and_a_ring_of_17_does_not() {
    // 16 links over 16 ports: 3 + 80 + 160 = 243
    let r = fits(&ring(16)).await;
    assert_eq!(r.status, 202, "{}", r.text());
    // 17 links over 17 ports: 3 + 85 + 170 = 258. The first 16 (a chain over 17 ports:
    // 3 + 85 + 160 = 248) fit, the ring does not.
    let links = ring(17);
    let r = fits(&links[..16]).await;
    assert_eq!(r.status, 202, "{}", r.text());
    let a = TestApp::new(big_host());
    let r = a.start_links(bodies(&links)).await;
    assert_refused_naming(&r, "ring of 17", "256");
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn sixteen_hairpins_fit_and_the_seventeenth_does_not() {
    let links = hairpins(17);
    assert_eq!(fits(&links[..16]).await.status, 202);
    assert_refused_naming(&fits(&links).await, "17 hairpins", "256");
}

#[tokio::test]
async fn twelve_one_way_links_fit_and_the_thirteenth_does_not() {
    let links = one_way(13);
    assert_eq!(fits(&links[..12]).await.status, 202);
    assert_refused_naming(&fits(&links).await, "13 one-way links", "256");
}

#[tokio::test]
async fn exactly_253_entries_fit_and_one_more_link_or_port_does_not() {
    let base = two_chains(); // 18 ports, 16 links: 253
    assert_eq!(fits(&base).await.status, 202);

    // a link between existing ports (end of chain a -> start of chain b) adds 10: 263
    let mut bridged = base.clone();
    bridged.push(("bridge".into(), nul(9), nul(10)));
    assert_refused_naming(&fits(&bridged).await, "bridge link", "256");
    // a hairpin on a new port adds 15
    let mut extra = base.clone();
    extra.push(("extra".into(), nul(40), nul(40)));
    assert_refused_naming(&fits(&extra).await, "extra hairpin", "256");
}

#[tokio::test]
async fn the_limit_counts_the_ports_of_the_body_once() {
    // same link count, one more distinct port: a4 goes to nul(0) (a port in use) or to a new one
    let mut same = two_chains();
    let i = same.iter().position(|l| l.0 == "a4").unwrap();
    same[i] = ("a4".into(), nul(4), nul(0));
    // nul(0) is the ingress of a0 only, so using it as an egress is legal
    assert_eq!(fits(&same).await.status, 202);
    let mut new_port = two_chains();
    new_port[i] = ("a4".into(), nul(4), nul(99));
    assert_refused_naming(&fits(&new_port).await, "a new port makes 19 ports", "256");
}

#[tokio::test]
async fn validate_applies_the_limit_too() {
    let a = TestApp::new(big_host());
    let r = a.validate(&setup(bodies(&ring(17)))).await;
    assert_refused_naming(&r, "validate ring of 17", "256");
    let r = a.validate(&setup(bodies(&ring(16)))).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["directions"].as_array().unwrap().len(), 16);
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
