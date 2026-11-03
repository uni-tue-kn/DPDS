//! Spec v7 S6: the stop of `dpds` waits for the drain timeout (the existing rule of
//! `eal::drain_timeout`: longest `mean + 6 std`, plus `limit / rate` with the limiter on,
//! plus a 5 s margin; spec20: the limit is the DERIVED `buffer_capacity * 1542 * 8` bit,
//! there is no `limit_bit` any more), but the systemd unit gives the controller only
//! `TimeoutStopSec=600`. A host hint with id `drain_timeout` and level `warn` names every
//! stored link whose drain timeout exceeds 590 s and the 600 s systemd limit. Links that
//! drain faster (all normal ones) produce no such hint.

use crate::common::*;
use dpds_controller::model::{Link, StoredState};
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

fn with_config(id: u32, name: &str, port: &str, cfg: Value) -> Link {
    link_cfg(id, name, port, port, cfg)
}

/// Limiter on at `rate_bps`, buffer of `capacity` packets: the drain takes
/// `capacity * 1542 * 8 / rate_bps` seconds.
fn limited(mean_ns: f64, rate_bps: f64, capacity: u64, on: bool) -> Value {
    let mut c = cfg_json(mean_ns, 0.0);
    c["buffer_capacity"] = json!(capacity);
    c["limiter"] = json!({"mode": "Spacer", "activate": on, "rate_bps": rate_bps});
    c
}

#[test]
fn a_slow_limiter_link_gets_a_warning_naming_the_link_and_the_systemd_limit() {
    // capacity 1000: 12.336 Mbit at 1 kb/s = 12336 s
    let host = FakeHost::bare_metal();
    let state = state_with(vec![
        with_config(1, "crawler", "net_null0", limited(1.0e6, 1.0e3, 1000, true)),
        link(2, "quick", "net_null1", "net_null1", None),
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
    let state = state_with(vec![
        link_cfg(1, "slow-dir", NIC0, NIC1, limited(1.0e6, 1.0e3, 1000, true)),
        link(2, "fast-dir", NIC1, NIC0, None),
    ]);
    let (found, text) = drain_hints(&hints_json(&host, &state));
    assert!(!found.is_empty());
    assert!(text.contains("slow-dir"), "{text}");
    assert!(!text.contains("fast-dir"), "{text}");
}

#[test]
fn a_long_delay_alone_triggers_it_and_each_slow_link_is_named() {
    let host = FakeHost::bare_metal();
    let state = state_with(vec![
        with_config(1, "slow-a", "net_null0", cfg_json(700.0e9, 0.0)),
        with_config(2, "slow-b", "net_null1", limited(1.0e6, 1.0e3, 1000, true)),
        link(3, "quick", "net_null2", "net_null2", None),
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
    let under = state_with(vec![with_config(
        1,
        "u",
        "net_null0",
        cfg_json(584.0e9, 0.0),
    )]);
    assert!(drain_hints(&hints_json(&host, &under)).0.is_empty());
    let over = state_with(vec![with_config(
        1,
        "o",
        "net_null0",
        cfg_json(586.0e9, 0.0),
    )]);
    assert!(!drain_hints(&hints_json(&host, &over)).0.is_empty());
}

#[test]
fn normal_links_and_an_inactive_limiter_give_no_hint() {
    let host = FakeHost::bare_metal();
    for state in [
        StoredState::default(),
        state_with(vec![link(1, "a", "net_null0", "net_null0", None)]),
        state_with(vec![
            link(1, "a-to-b", NIC0, NIC1, None),
            link(2, "b-to-a", NIC1, NIC0, None),
        ]),
        // the limiter is off: its limit / rate does not count (like in drain_timeout)
        state_with(vec![with_config(
            1,
            "off",
            "net_null0",
            limited(1.0e6, 1.0e3, 1000, false),
        )]),
        // fast limiter, big buffer: 100_000 * 1542 * 8 = 1.2336 Gbit at 100 Gb/s = 12.3 ms
        state_with(vec![with_config(
            1,
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
    let over = state_with(vec![with_config(
        1,
        "over",
        "net_null0",
        limited(1.0e6, 21_080.0, 1000, true),
    )]);
    assert!(
        !drain_hints(&hints_json(&host, &over)).0.is_empty(),
        "590.2 s reaches the threshold"
    );
    let under = state_with(vec![with_config(
        1,
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
    let state = state_with(vec![
        with_config(
            1,
            "small",
            "net_null0",
            limited(1.0e6, 1.0e3, 10, true), // 121.76 s
        ),
        with_config(
            2,
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
    let state = state_with(vec![with_config(
        1,
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
    let mut body = link_body("crawler", "net_null0", "net_null1");
    body["config"]["limiter"] = json!({"mode": "Spacer", "activate": true, "rate_bps": 1.0e3});
    body["config"]["buffer_capacity"] = json!(1000);
    assert_eq!(a.post("/api/links", body).await.status, 201);
    let (found, text) = drain_hints(&a.get("/api/host/hints").await.json());
    assert!(!found.is_empty());
    assert!(
        text.contains("crawler") && text.contains("TimeoutStopSec"),
        "{text}"
    );

    let b = TestApp::new(FakeHost::bare_metal());
    assert_eq!(
        b.create_link("quick", "net_null0", "net_null1")
            .await
            .status,
        201
    );
    assert!(drain_hints(&b.get("/api/host/hints").await.json())
        .0
        .is_empty());
}
