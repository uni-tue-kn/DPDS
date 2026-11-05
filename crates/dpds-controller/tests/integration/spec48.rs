//! Spec 48: `GET /api/online` counts the OTHER GUI sessions (controller part C1-C2).
//!
//! A session is whoever polled `/api/online` in the last 15 s, identified by the `X-Session-Id` header (any case),
//! else by the client IP (`ConnectInfo<SocketAddr>`), else by the fixed id "unknown" (in-process tests have no
//! ConnectInfo). The id is cut to at most 64 bytes. The reply is the `OnlineView` plus `connected_clients`
//! (the number of other sessions, never negative). Only `/api/online` tracks.
//!
//! The 15 s expiry is pinned here with real time (about 16 s, run in parallel with the other tests). The exact
//! inclusive boundary (age == 15 s still counts) cannot be reached over HTTP: it needs a unit test of the tracker
//! with an injected clock next to its code.

use crate::common::*;
use std::net::SocketAddr;
use std::time::Duration;

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

/// Polls `/api/online` with `X-Session-Id: id` and returns `connected_clients`.
async fn poll(a: &TestApp, id: &str) -> u64 {
    let r = a
        .request_with("GET", "/api/online", None, &[("X-Session-Id", id)])
        .await;
    count(&r)
}

fn count(r: &Reply) -> u64 {
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()["connected_clients"]
        .as_u64()
        .unwrap_or_else(|| panic!("connected_clients is not a number: {}", r.text()))
}

/// Polls `/api/online` without the header, as coming from `peer` (None = no ConnectInfo at all).
async fn poll_from(a: &TestApp, peer: Option<&str>) -> u64 {
    let mut req = axum::http::Request::builder()
        .method("GET")
        .uri("/api/online")
        .body(axum::body::Body::empty())
        .unwrap();
    if let Some(p) = peer {
        let addr: SocketAddr = p.parse().unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(addr));
    }
    count(&send(&a.router, req).await)
}

// ---- the JSON ---------------------------------------------------------------------------------

#[tokio::test]
async fn online_has_connected_clients_next_to_the_old_fields() {
    let a = app();
    let r = a.get("/api/online").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["dpds_bin"], a.dpds_bin.to_str().unwrap());
    assert_eq!(v["telemetry"], true);
    assert_eq!(v["root"], false);
    assert_eq!(
        v["connected_clients"], 0,
        "a lone poller has no other session"
    );
}

// ---- counting ---------------------------------------------------------------------------------

#[tokio::test]
async fn a_lone_session_sees_zero_however_often_it_polls() {
    let a = app();
    for _ in 0..5 {
        assert_eq!(poll(&a, "only-me").await, 0);
    }
}

#[tokio::test]
async fn other_sessions_are_counted_and_the_own_one_is_not() {
    let a = app();
    assert_eq!(poll(&a, "a").await, 0);
    assert_eq!(poll(&a, "b").await, 1);
    assert_eq!(poll(&a, "c").await, 2);
    assert_eq!(poll(&a, "a").await, 2);
    assert_eq!(poll(&a, "b").await, 2);
    assert_eq!(poll(&a, "d").await, 3);
}

#[tokio::test]
async fn the_header_name_is_case_insensitive() {
    let a = app();
    for name in ["x-session-id", "X-SESSION-ID", "X-Session-Id"] {
        let r = a
            .request_with("GET", "/api/online", None, &[(name, name)])
            .await;
        count(&r);
    }
    // three distinct ids were tracked: a fourth one sees three others
    assert_eq!(poll(&a, "fourth").await, 3);
}

#[tokio::test]
async fn each_app_has_its_own_tracker() {
    let a = app();
    let b = app();
    assert_eq!(poll(&a, "x").await, 0);
    assert_eq!(
        poll(&b, "y").await,
        0,
        "sessions of another router do not leak"
    );
}

#[tokio::test]
async fn sessions_expire_after_15_seconds_and_a_poll_refreshes_them() {
    let a = app();
    // t = 0: A and B are seen.
    assert_eq!(poll(&a, "A").await, 0);
    assert_eq!(poll(&a, "B").await, 1);
    tokio::time::sleep(Duration::from_secs(9)).await;
    // t = 9: A polls again (refresh); B is 9 s old and still counts.
    assert_eq!(poll(&a, "A").await, 1);
    tokio::time::sleep(Duration::from_secs(7)).await;
    // t = 16: B is 16 s old and gone; A was refreshed 7 s ago and stays.
    assert_eq!(poll(&a, "C").await, 1, "only A is left, B expired");
    assert_eq!(
        poll(&a, "B").await,
        2,
        "B is back: A (7 s old) and C (0 s) count"
    );
}

#[tokio::test]
async fn the_count_saturates_at_zero_after_everybody_else_expired() {
    let a = app();
    assert_eq!(poll(&a, "gone-1").await, 0);
    assert_eq!(poll(&a, "gone-2").await, 1);
    tokio::time::sleep(Duration::from_secs(16)).await;
    assert_eq!(
        poll(&a, "late").await,
        0,
        "both others expired, no underflow"
    );
    assert_eq!(poll(&a, "late").await, 0);
}

// ---- the id: header, IP, "unknown", 64 bytes --------------------------------------------------

#[tokio::test]
async fn without_header_and_without_connect_info_the_id_is_unknown() {
    let a = app();
    assert_eq!(poll_from(&a, None).await, 0);
    assert_eq!(
        poll_from(&a, None).await,
        0,
        "the same fixed id twice is one session"
    );
    assert_eq!(
        poll(&a, "real").await,
        1,
        "'unknown' counts as one other session"
    );
    assert_eq!(
        poll_from(&a, None).await,
        1,
        "and 'real' is the other one for it"
    );
}

#[tokio::test]
async fn without_header_the_client_ip_identifies_the_session() {
    let a = app();
    assert_eq!(poll_from(&a, Some("10.0.0.1:1111")).await, 0);
    assert_eq!(
        poll_from(&a, Some("10.0.0.1:2222")).await,
        0,
        "same IP, other port: the same session"
    );
    assert_eq!(poll_from(&a, Some("10.0.0.2:1111")).await, 1);
    assert_eq!(poll_from(&a, Some("10.0.0.1:3333")).await, 1);
}

#[tokio::test]
async fn the_header_wins_over_the_ip() {
    let a = app();
    for peer in ["10.0.0.1:1", "10.0.0.2:1"] {
        let mut req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/online")
            .header("x-session-id", "same-tab")
            .body(axum::body::Body::empty())
            .unwrap();
        req.extensions_mut().insert(axum::extract::ConnectInfo(
            peer.parse::<SocketAddr>().unwrap(),
        ));
        assert_eq!(
            count(&send(&a.router, req).await),
            0,
            "{peer}: one header id is one session whatever the IP"
        );
    }
}

#[tokio::test]
async fn two_tabs_behind_one_ip_are_two_sessions() {
    let a = app();
    let mut seen = Vec::new();
    for id in ["tab-1", "tab-2"] {
        let mut req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/online")
            .header("x-session-id", id)
            .body(axum::body::Body::empty())
            .unwrap();
        req.extensions_mut().insert(axum::extract::ConnectInfo(
            "10.0.0.9:5000".parse::<SocketAddr>().unwrap(),
        ));
        seen.push(count(&send(&a.router, req).await));
    }
    assert_eq!(seen, [0, 1]);
}

#[tokio::test]
async fn ids_are_cut_to_64_bytes() {
    let a = app();
    let base = "k".repeat(64);
    assert_eq!(poll(&a, &format!("{base}1")).await, 0);
    assert_eq!(
        poll(&a, &format!("{base}2-and-more")).await,
        0,
        "ids that differ only after byte 64 are one session"
    );
    // a difference inside the first 64 bytes still separates them
    let other = format!("{}z", "k".repeat(63));
    assert_eq!(poll(&a, &other).await, 1);
    // an id of exactly 64 bytes is the session the two longer ids were cut to
    assert_eq!(poll(&a, &base).await, 1, "one other session: 'other'");
}

#[tokio::test]
async fn a_very_long_id_is_answered_normally() {
    let a = app();
    let long = "x".repeat(7000);
    assert_eq!(poll(&a, &long).await, 0);
    assert_eq!(poll(&a, "y").await, 1);
}

#[tokio::test]
async fn the_cut_never_splits_a_multibyte_character() {
    // 63 ASCII bytes + a 2-byte char straddle byte 64: the cut must fall back to byte 63 and must not panic.
    let a = app();
    let one = format!("{}\u{e9}x", "m".repeat(63));
    let two = format!("{}\u{fc}y", "m".repeat(63));
    for id in [&one, &two] {
        let r = a
            .request_with("GET", "/api/online", None, &[("x-session-id", id)])
            .await;
        assert_eq!(r.status, 200, "{}", r.text());
    }
}

// ---- only /api/online tracks ------------------------------------------------------------------

#[tokio::test]
async fn other_routes_do_not_track() {
    let a = app();
    for path in [
        "/api/version",
        "/api/interfaces",
        "/api/host/hints",
        "/api/emulation",
        "/api/stats",
        "/api/openapi.json",
        "/",
        "/api/no/such/route",
    ] {
        let r = a
            .request_with("GET", path, None, &[("X-Session-Id", "browser")])
            .await;
        assert!(r.status > 0, "{path}");
    }
    assert_eq!(
        poll(&a, "poller").await,
        0,
        "the other routes left no session behind"
    );
}

#[tokio::test]
async fn a_non_get_to_online_does_not_track() {
    let a = app();
    let r = a
        .request_with("POST", "/api/online", None, &[("X-Session-Id", "poster")])
        .await;
    assert_eq!(r.status, 405);
    assert_eq!(poll(&a, "poller").await, 0);
}

#[tokio::test]
async fn the_openapi_document_lists_connected_clients() {
    let a = app();
    let r = a.get("/api/openapi.json").await;
    assert_eq!(r.status, 200);
    let v = r.json();
    let props = &v["components"]["schemas"]["OnlineView"]["properties"];
    assert!(
        props.get("connected_clients").is_some(),
        "OnlineView in the OpenAPI document: {props}"
    );
}
