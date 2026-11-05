//! Spec v7 S6: the stop of `dpds` waits for the drain timeout (the existing rule of
//! `eal::drain_timeout`: longest `mean + 6 std`, plus `limit / rate` with the limiter on,
//! plus a 5 s margin; spec20: the limit is the DERIVED `buffer_capacity * 1542 * 8` bit,
//! there is no `limit_bit` any more), but the systemd unit gives the controller only
//! `TimeoutStopSec=600`. A host hint with id `drain_timeout` and level `warn` names every
//! stored link whose drain timeout exceeds 590 s and the 600 s systemd limit. Links that
//! drain faster (all normal ones) produce no such hint.
//!
//! Spec 22: the links come from the body of `POST /api/host/hints`; the limiter is an optional
//! stage (no `activate` flag: present = on), so "limiter off" is a config without a limiter, and
//! a link without delayer and limiter drains in the 5 s margin and gets no hint.

use crate::common::*;
use serde_json::{json, Value};

/// Hints with id `drain_timeout`, joined into one text, and their levels.
fn drain_hints(hints: &Value) -> (Vec<Value>, String) {
    let found: Vec<Value> = hints
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["id"] == "drain_timeout")
        .cloned()
        .collect();
    let text = found
        .iter()
        .map(|h| format!("{} {} {}", h["title"], h["detail"], h["suggestion"]))
        .collect::<Vec<_>>()
        .join(" ");
    (found, text)
}

fn with_config(name: &str, port: &str, cfg: Value) -> Value {
    link_cfg(name, port, port, cfg)
}

/// Limiter stage at `rate_bps` (`on`; absent when not), , buffer of `capacity` packets: the drain takes
/// `capacity * 1542 * 8 / rate_bps` seconds.
fn limited(mean_ns: f64, rate_bps: f64, capacity: u64, on: bool) -> Value {
    let mut c = cfg_json(mean_ns, 0.0);
    c["buffer_capacity"] = json!(capacity);
    if on {
        // a present limiter is active; "off" is a config without the stage
        c["limiter"] = json!({"mode": "Spacer", "rate_bps": rate_bps});
    }
    c
}

#[test]
fn a_slow_limiter_link_gets_a_warning_naming_the_link_and_the_systemd_limit() {
    // capacity 1000: 12.336 Mbit at 1 kb/s = 12336 s
    let host = FakeHost::bare_metal();
    let state = setup(vec![
        with_config("crawler", "net_null0", limited(1.0e6, 1.0e3, 1000, true)),
        link("quick", "net_null1", "net_null1"),
    ]);
    let (found, text) = drain_hints(&hints_json(&host, &state));
    assert!(!found.is_empty(), "no drain_timeout hint");
    for h in &found {
        assert_eq!(h["level"], "warn", "{h}");
    }
    assert!(text.contains("crawler"), "names the link: {text}");
    assert!(!text.contains("quick"), "does not name a fast link: {text}");
    assert!(text.contains("TimeoutStopSec"), "{text}");
    assert!(text.contains("600"), "{text}");
}

#[test]
fn a_slow_link_is_judged_on_its_own_config_not_its_partner_in_a_pair() {
    // a bidirectional pair is two links: only the slow direction is named
    let host = FakeHost::bare_metal();
    let state = setup(vec![
        link_cfg("slow-dir", NIC0, NIC1, limited(1.0e6, 1.0e3, 1000, true)),
        link("fast-dir", NIC1, NIC0),
    ]);
    let (found, text) = drain_hints(&hints_json(&host, &state));
    assert!(!found.is_empty());
    assert!(text.contains("slow-dir"), "{text}");
    assert!(!text.contains("fast-dir"), "{text}");
}

#[test]
fn a_long_delay_alone_triggers_it_and_each_slow_link_is_named() {
    let host = FakeHost::bare_metal();
    let state = setup(vec![
        with_config("slow-a", "net_null0", cfg_json(700.0e9, 0.0)),
        with_config("slow-b", "net_null1", limited(1.0e6, 1.0e3, 1000, true)),
        link("quick", "net_null2", "net_null2"),
    ]);
    let (found, text) = drain_hints(&hints_json(&host, &state));
    assert!(!found.is_empty());
    assert!(text.contains("slow-a") && text.contains("slow-b"), "{text}");
    assert!(!text.contains("quick"), "{text}");
}

#[test]
fn the_threshold_is_590_seconds_of_drain_timeout() {
    // drain_timeout = mean + 5 s margin: 584 s -> 589 s, 586 s -> 591 s
    let host = FakeHost::bare_metal();
    let under = setup(vec![with_config("u", "net_null0", cfg_json(584.0e9, 0.0))]);
    assert!(drain_hints(&hints_json(&host, &under)).0.is_empty());
    let over = setup(vec![with_config("o", "net_null0", cfg_json(586.0e9, 0.0))]);
    assert!(!drain_hints(&hints_json(&host, &over)).0.is_empty());
}

#[test]
fn normal_links_and_an_inactive_limiter_give_no_hint() {
    let host = FakeHost::bare_metal();
    for state in [
        Setup::default(),
        setup(vec![link("a", "net_null0", "net_null0")]),
        setup(vec![link("a-to-b", NIC0, NIC1), link("b-to-a", NIC1, NIC0)]),
        // the limiter is off: its limit / rate does not count (like in drain_timeout)
        setup(vec![with_config(
            "off",
            "net_null0",
            limited(1.0e6, 1.0e3, 1000, false),
        )]),
        // fast limiter, big buffer: 100_000 * 1542 * 8 = 1.2336 Gbit at 100 Gb/s = 12.3 ms
        setup(vec![with_config(
            "fast",
            "net_null0",
            limited(1.0e6, 100.0e9, 100_000, true),
        )]),
    ] {
        let (found, _) = drain_hints(&hints_json(&host, &state));
        assert!(found.is_empty(), "{found:?}");
    }
}

#[test]
fn the_limiter_part_of_the_estimate_is_the_derived_limit_over_the_rate() {
    // drain_timeout = 0.001 s (mean) + capacity * 12336 bit / rate + 5 s margin.
    // capacity 1000 at 21_080 b/s: 12_336_000 / 21_080 = 585.2 s -> 590.2 s: hint.
    // At 21_200 b/s: 581.9 s -> 586.9 s: none. (With a limit of 1522 * 8, 1518 * 8 or
    // 1500 * 8 bit per slot the first would stay below 590 s.)
    let host = FakeHost::bare_metal();
    let over = setup(vec![with_config(
        "over",
        "net_null0",
        limited(1.0e6, 21_080.0, 1000, true),
    )]);
    assert!(
        !drain_hints(&hints_json(&host, &over)).0.is_empty(),
        "590.2 s reaches the threshold"
    );
    let under = setup(vec![with_config(
        "under",
        "net_null0",
        limited(1.0e6, 21_200.0, 1000, true),
    )]);
    assert!(drain_hints(&hints_json(&host, &under)).0.is_empty());
}

#[test]
fn the_estimate_grows_with_the_buffer_capacity() {
    // the same limiter rate: a small buffer drains fast, a big one is named
    let host = FakeHost::bare_metal();
    let state = setup(vec![
        with_config(
            "small",
            "net_null0",
            limited(1.0e6, 1.0e3, 10, true), // 121.76 s
        ),
        with_config(
            "big",
            "net_null1",
            limited(1.0e6, 1.0e3, 1000, true), // 12336 s
        ),
    ]);
    let (found, text) = drain_hints(&hints_json(&host, &state));
    assert!(!found.is_empty());
    assert!(text.contains("big"), "{text}");
    assert!(!text.contains("small"), "{text}");
}

#[test]
fn the_hint_text_no_longer_mentions_limit_bit() {
    let host = FakeHost::bare_metal();
    let state = setup(vec![with_config(
        "crawler",
        "net_null0",
        limited(1.0e6, 1.0e3, 1000, true),
    )]);
    let (found, text) = drain_hints(&hints_json(&host, &state));
    assert!(!found.is_empty());
    assert!(
        !text.contains("limit_bit"),
        "the field is gone, the hint must not send the user to it: {text}"
    );
}

#[tokio::test]
async fn the_hint_is_served_by_the_api() {
    let a = TestApp::new(FakeHost::bare_metal());
    let mut slow = link("crawler", "net_null0", "net_null1");
    slow["config"]["limiter"] = json!({"mode": "Spacer", "rate_bps": 1.0e3});
    slow["config"]["buffer_capacity"] = json!(1000);
    let r = a.post("/api/host/hints", setup(vec![slow]).body()).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let (found, text) = drain_hints(&r.json());
    assert!(!found.is_empty());
    assert!(
        text.contains("crawler") && text.contains("TimeoutStopSec"),
        "{text}"
    );
    // GET has no links to judge
    assert!(drain_hints(&a.get("/api/host/hints").await.json())
        .0
        .is_empty());

    let b = TestApp::new(FakeHost::bare_metal());
    let r = b
        .post(
            "/api/host/hints",
            setup(vec![link("quick", "net_null0", "net_null1")]).body(),
        )
        .await;
    assert!(drain_hints(&r.json()).0.is_empty());
}

// ---- optional stages: a missing delayer or limiter adds nothing to the drain ---------------

#[test]
fn links_without_a_limiter_or_a_delayer_give_no_drain_hint() {
    let host = FakeHost::bare_metal();
    let mut no_delayer = cfg_bare();
    no_delayer["loss_model"] = json!({"type": "Random", "p": 0.1});
    let mut limiter_only = cfg_bare();
    limiter_only["limiter"] = json!({"mode": "Spacer", "rate_bps": 100.0e9});
    for (what, cfg) in [
        ("no stage at all", cfg_bare()),
        ("no delayer", no_delayer),
        ("limiter only, fast", limiter_only),
    ] {
        let s = setup(vec![with_config("l", "net_null0", cfg)]);
        let hints = hints_json(&host, &s);
        let (found, _) = drain_hints(&hints);
        assert!(found.is_empty(), "{what}: {found:?}");
        let text = hints.to_string();
        assert!(
            !text.contains("NaN") && !text.contains("inf s"),
            "{what}: {text}"
        );
    }
}

#[test]
fn a_slow_limiter_alone_is_named_even_without_a_delayer() {
    // the limiter term does not depend on the delayer stage: capacity 1000 at 1 kb/s = 12336 s
    let host = FakeHost::bare_metal();
    let mut cfg = cfg_bare();
    cfg["buffer_capacity"] = json!(1000);
    cfg["limiter"] = json!({"mode": "Spacer", "rate_bps": 1.0e3});
    let (found, text) = drain_hints(&hints_json(
        &host,
        &setup(vec![with_config("crawler", "net_null0", cfg)]),
    ));
    assert!(!found.is_empty());
    assert!(text.contains("crawler"), "{text}");
}

#[test]
fn a_delayer_alone_is_named_when_it_is_long_even_without_a_limiter() {
    let host = FakeHost::bare_metal();
    let mut cfg = cfg_bare();
    cfg["delayer"] = json!({"distribution": "DeterministicDistribution", "mean_ns": 700.0e9,
        "std_ns": 0.0, "activate_correlation": false, "half_life_period_ns": 0.0,
        "initialization_rate_pps": 1.0});
    let (found, text) = drain_hints(&hints_json(
        &host,
        &setup(vec![with_config("slow", "net_null0", cfg)]),
    ));
    assert!(!found.is_empty());
    assert!(text.contains("slow"), "{text}");
}

#[tokio::test]
async fn the_drain_timeout_of_a_link_without_stages_is_the_margin_alone() {
    // no delayer (mean 0, std 0) and no limiter (inactive): nothing to wait for but the 5 s margin
    let mut dly_only = cfg_bare();
    dly_only["delayer"] = json!({"distribution": "DeterministicDistribution", "mean_ns": 2.0e9,
        "std_ns": 0.0, "activate_correlation": false, "half_life_period_ns": 0.0,
        "initialization_rate_pps": 1.0});
    for (cfg, want) in [(cfg_bare(), 5.0), (dly_only, 7.0)] {
        let a2 = TestApp::new(FakeHost::bare_metal());
        a2.start_ok(vec![link_cfg("l", "net_null0", "net_null0", cfg)])
            .await;
        let t = a2.supervisor.plans()[0].drain_timeout.as_secs_f64();
        assert!((t - want).abs() < 1e-6, "{t} vs {want}");
    }
}
