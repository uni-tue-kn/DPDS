//! HTTP surface hardening:
//!
//! - CORS is OFF by default (the controller serves the frontend itself, same-origin): no
//!   `access-control-*` header, also not on a preflight. `--cors-origin <origin>`
//!   (repeatable, env `DPDS_CORS_ORIGIN`) enables it for exactly those origins:
//!   the header echoes the request's origin if it is listed, never `*`, nothing otherwise.
//! - Every POST (POST /api/emulation/start|validate|stop and POST /api/host/hints) requires
//!   `Content-Type: application/json` (axum's `Json`
//!   semantics: `; charset=...` and `+json` are fine): 415 `{"message"}` otherwise, before
//!   anything else is looked at, nothing started.
//! - A body over the size limit is 413 with a `{"message"}` JSON body (the tests use
//!   8 MiB).
//! - OpenAPI documents the error responses where they occur.

use crate::common::*;
use serde_json::{json, Value};

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

const NO_CORS: [&str; 5] = [
    "access-control-allow-origin",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "access-control-allow-credentials",
    "access-control-max-age",
];

// ---- CORS ------------------------------------------------------------------

#[tokio::test]
async fn cors_is_off_by_default_for_requests_and_preflights() {
    let a = app();
    let r = a
        .request_with(
            "GET",
            "/api/online",
            None,
            &[("origin", "http://example.com")],
        )
        .await;
    assert_eq!(r.status, 200);
    for h in NO_CORS {
        assert!(r.headers.get(h).is_none(), "{h}: {:?}", r.headers);
    }
    let r = a
        .request_with(
            "OPTIONS",
            "/api/emulation/start",
            None,
            &[
                ("origin", "http://example.com"),
                ("access-control-request-method", "POST"),
            ],
        )
        .await;
    for h in NO_CORS {
        assert!(r.headers.get(h).is_none(), "preflight {h}: {:?}", r.headers);
    }
}

#[test]
fn only_the_configured_origins_get_cors_headers_flag_form() {
    let c = ControllerProcess::start(
        &[
            "serve",
            "--bind",
            "{addr}",
            "--cors-origin",
            "http://ui.example:8080",
            "--cors-origin",
            "https://other.example",
        ],
        &[],
    );
    for origin in ["http://ui.example:8080", "https://other.example"] {
        let (st, h, _) = get_with_origin(&c.addr, "/api/online", origin);
        assert_eq!(st, 200);
        assert_eq!(
            header_value(&h, "access-control-allow-origin").as_deref(),
            Some(origin),
            "{h:?}"
        );
    }
    for origin in ["http://evil.example", "http://ui.example", "null"] {
        let (st, h, _) = get_with_origin(&c.addr, "/api/online", origin);
        assert_eq!(st, 200);
        assert_eq!(
            header_value(&h, "access-control-allow-origin"),
            None,
            "{origin}: {h:?}"
        );
    }
    // a preflight from a listed origin allows the mutating methods
    let req = format!(
        "OPTIONS /api/emulation/start HTTP/1.1\r\nHost: {}\r\nOrigin: http://ui.example:8080\r\n\
         Access-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type\r\n\
         Connection: close\r\n\r\n",
        c.addr
    );
    let (st, h, _) = http_raw(&c.addr, &req).unwrap();
    assert!(st == 200 || st == 204, "{st} {h:?}");
    assert_eq!(
        header_value(&h, "access-control-allow-origin").as_deref(),
        Some("http://ui.example:8080")
    );
    let methods = header_value(&h, "access-control-allow-methods").unwrap_or_default();
    for m in ["get", "post", "put", "delete"] {
        assert!(methods.contains(m) || methods.contains('*'), "{methods}");
    }
    // and one from an unlisted origin does not
    let req = req.replace("http://ui.example:8080", "http://evil.example");
    let (_, h, _) = http_raw(&c.addr, &req).unwrap();
    assert_eq!(
        header_value(&h, "access-control-allow-origin"),
        None,
        "{h:?}"
    );
}

#[test]
fn the_cors_origin_can_come_from_the_environment() {
    let c = ControllerProcess::start(
        &["serve", "--bind", "{addr}"],
        &[("DPDS_CORS_ORIGIN", "http://env.example")],
    );
    let (_, h, _) = get_with_origin(&c.addr, "/api/online", "http://env.example");
    assert_eq!(
        header_value(&h, "access-control-allow-origin").as_deref(),
        Some("http://env.example"),
        "{h:?}"
    );
    let (_, h, _) = get_with_origin(&c.addr, "/api/online", "http://evil.example");
    assert_eq!(header_value(&h, "access-control-allow-origin"), None);
}

#[test]
fn the_served_binary_has_no_cors_without_the_option() {
    let c = ControllerProcess::start(&["serve", "--bind", "{addr}"], &[]);
    let (st, h, _) = get_with_origin(&c.addr, "/api/online", "http://example.com");
    assert_eq!(st, 200);
    assert_eq!(
        header_value(&h, "access-control-allow-origin"),
        None,
        "{h:?}"
    );
}

// ---- 415 -------------------------------------------------------------------

/// `(method, path, valid JSON body)` of every endpoint that reads a body.
fn mutating() -> Vec<(&'static str, String, Value)> {
    let body = setup(vec![link("net_null5", "net_null5")]).body();
    vec![
        ("POST", "/api/emulation/start".into(), body.clone()),
        ("POST", "/api/emulation/validate".into(), body.clone()),
        ("POST", "/api/host/hints".into(), body),
        ("POST", "/api/emulation/stop".into(), json!({})),
    ]
}

#[tokio::test]
async fn a_non_json_content_type_is_a_415_with_a_message_on_every_post_endpoint() {
    let a = app();
    for (method, path, body) in mutating() {
        for ct in [
            Some("text/plain"),
            Some("application/x-www-form-urlencoded"),
            Some("multipart/form-data; boundary=x"),
            Some("application/jsonx"),
            None,
        ] {
            let r = a
                .request_raw(method, &path, ct, body.to_string().into_bytes())
                .await;
            assert_eq!(r.status, 415, "{method} {path} with {ct:?}: {}", r.text());
            r.message();
        }
    }
    // nothing happened
    assert!(a.supervisor.plans().is_empty());
    assert_eq!(a.supervisor.stops(), 0);
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

#[tokio::test]
async fn the_content_type_is_checked_before_the_state_of_the_emulation() {
    // stop while stopped is a 409 with JSON, a 415 with text/plain
    let a = app();
    let r = a
        .request_raw(
            "POST",
            "/api/emulation/stop",
            Some("text/plain"),
            b"{}".to_vec(),
        )
        .await;
    assert_eq!(r.status, 415);
    let r = a.post("/api/emulation/stop", json!({})).await;
    assert_eq!(r.status, 409);
    // a start while running with the wrong content type is a 415 too, not a 409
    a.start_ok(vec![link("net_null0", "net_null0")]).await;
    let r = a
        .request_raw(
            "POST",
            "/api/emulation/start",
            Some("text/plain"),
            setup(vec![link("net_null0", "net_null0")])
                .body()
                .to_string()
                .into_bytes(),
        )
        .await;
    assert_eq!(r.status, 415, "{}", r.text());
}

#[tokio::test]
async fn json_content_types_with_parameters_and_suffixes_are_accepted() {
    for ct in [
        "application/json",
        "application/json; charset=utf-8",
        "Application/JSON",
        "application/merge-patch+json",
    ] {
        let a = app();
        let body = setup(vec![link("net_null0", "net_null0")])
            .body()
            .to_string()
            .into_bytes();
        let r = a
            .request_raw("POST", "/api/emulation/validate", Some(ct), body.clone())
            .await;
        assert_eq!(r.status, 200, "{ct}: {}", r.text());
        let r = a
            .request_raw("POST", "/api/emulation/start", Some(ct), body)
            .await;
        assert_eq!(r.status, 202, "{ct}: {}", r.text());
    }
}

// ---- 413 -------------------------------------------------------------------

#[tokio::test]
async fn an_oversized_body_is_a_413_with_a_json_message() {
    let a = app();
    let big = format!(r#"{{"name":"{}"}}"#, "a".repeat(8 * 1024 * 1024));
    for path in [
        "/api/emulation/start",
        "/api/emulation/validate",
        "/api/host/hints",
    ] {
        let r = a
            .request_raw(
                "POST",
                path,
                Some("application/json"),
                big.clone().into_bytes(),
            )
            .await;
        assert_eq!(
            r.status,
            413,
            "POST {path}: {}",
            &r.text()[..r.text().len().min(200)]
        );
        r.message();
    }
    // the service still works afterwards
    assert_eq!(
        a.start_links(vec![link("net_null0", "net_null0")])
            .await
            .status,
        202
    );
}

// ---- OpenAPI ---------------------------------------------------------------

#[tokio::test]
async fn openapi_documents_the_error_responses_where_they_occur() {
    let a = app();
    let v = a.get("/api/openapi.json").await.json();
    // (path, method, status codes that must be documented)
    let expect: &[(&str, &str, &[&str])] = &[
        (
            "/api/emulation/start",
            "post",
            &["202", "400", "409", "413", "415"],
        ),
        (
            "/api/emulation/validate",
            "post",
            &["200", "400", "409", "413", "415"],
        ),
        ("/api/host/hints", "post", &["200", "400", "413", "415"]),
        ("/api/emulation/stop", "post", &["202", "409", "415"]),
        ("/api/stats/{dir}/delay_histogram", "get", &["200", "404"]),
    ];
    for (path, method, codes) in expect {
        let responses = &v["paths"][path][method]["responses"];
        for c in *codes {
            assert!(
                responses.get(c).is_some(),
                "{method} {path}: response {c} is not documented: {responses}"
            );
        }
    }
    // the error responses carry the ErrorBody schema
    let r = &v["paths"]["/api/emulation/start"]["post"]["responses"]["415"];
    assert!(r.to_string().contains("ErrorBody"), "{r}");
}
