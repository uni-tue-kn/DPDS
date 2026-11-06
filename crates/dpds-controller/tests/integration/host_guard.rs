//! The DNS-rebinding guard. A request whose `Host` header (port stripped) is not
//!
//! - an IP literal (IPv4, or IPv6 in brackets, with or without a port),
//! - `localhost`,
//! - the machine's hostname (`gethostname`, as returned and without the domain part), or
//! - a value of the repeatable `--allowed-host` (env `DPDS_ALLOWED_HOST`, comma separated),
//!
//! gets 421 or 403 with a `{"message"}` body, on every `/api` route (reads, mutating routes,
//! the OpenAPI document and Swagger UI). A rejected mutating request changes nothing.
//!
//! The tests run the real binary and speak raw HTTP so the check is exercised wherever the
//! implementation puts it (router or `main`). A request without any Host header is not
//! tested: the in-process harness of the other tests sends none.

use crate::common::*;
use serde_json::{json, Value};

/// One raw request with an explicit `Host` header and a JSON body; returns (status, body).
fn call(addr: &str, method: &str, path: &str, host: &str, body: Option<&Value>) -> (u16, String) {
    let payload = body.map(Value::to_string).unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let (st, _, body) = http_raw(addr, &req).unwrap_or_else(|e| panic!("{method} {path}: {e}"));
    (st, body)
}

fn rejected(status: u16) -> bool {
    status == 421 || status == 403
}

fn machine_hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for `buf.len()` bytes; gethostname NUL-terminates it
    // (or fails, which the assert catches).
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    assert_eq!(rc, 0, "gethostname failed");
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// `(method, path, body)` of every `/api` route, the POSTs included.
fn all_routes() -> Vec<(&'static str, &'static str, Option<Value>)> {
    let body = setup(vec![link("evil", "net_null0", "net_null0")]).body();
    vec![
        ("GET", "/api/online", None),
        ("GET", "/api/interfaces", None),
        ("GET", "/api/host/hints", None),
        ("GET", "/api/emulation", None),
        ("GET", "/api/stats", None),
        ("GET", "/api/stats/history", None),
        ("GET", "/api/stats/0/delay_histogram", None),
        ("GET", "/api/version", None),
        ("GET", "/api/openapi.json", None),
        ("GET", "/api/docs/", None),
        ("POST", "/api/host/hints", Some(body.clone())),
        // no such route, and a foreign Host still gets the guard's answer, not a 404
        ("POST", "/api/host/actions/swappiness", Some(body.clone())),
        ("POST", "/api/emulation/validate", Some(body.clone())),
        ("POST", "/api/emulation/start", Some(body)),
        ("POST", "/api/emulation/stop", Some(json!({}))),
    ]
}

fn assert_all_rejected(addr: &str, host: &str) {
    for (method, path, body) in all_routes() {
        let (st, text) = call(addr, method, path, host, body.as_ref());
        assert!(
            rejected(st),
            "{method} {path} with Host {host:?}: status {st}, body {text:?}"
        );
        let v: Value = serde_json::from_str(text.trim())
            .unwrap_or_else(|e| panic!("{method} {path} Host {host:?}: not JSON ({e}): {text:?}"));
        assert!(
            v["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{method} {path} Host {host:?}: no message in {v}"
        );
    }
}

fn assert_online(addr: &str, host: &str) {
    let (st, text) = call(addr, "GET", "/api/online", host, None);
    assert_eq!(st, 200, "Host {host:?} must be accepted: {text:?}");
}

#[test]
fn a_foreign_host_is_rejected_on_every_api_route() {
    let c = ControllerProcess::start(&["serve", "--bind", "{addr}"], &[]);
    for host in [
        "attacker.example",
        "attacker.example:8100",
        "Attacker.Example:80",
        // look-alikes of the accepted names
        "127.0.0.1.attacker.example",
        "localhost.attacker.example:8100",
        "notlocalhost",
    ] {
        assert_all_rejected(&c.addr, host);
    }
}

#[test]
fn a_rejected_start_changes_nothing() {
    let c = ControllerProcess::start(&["serve", "--bind", "{addr}"], &[]);
    let good = c.addr.clone();
    let body = setup(vec![link("evil", "net_null0", "net_null0")]).body();
    for path in [
        "/api/emulation/start",
        "/api/emulation/validate",
        "/api/host/hints",
    ] {
        let (st, _) = call(&c.addr, "POST", path, "attacker.example", Some(&body));
        assert!(rejected(st), "{path}: {st}");
    }
    let (st, text) = call(&c.addr, "GET", "/api/emulation", &good, None);
    assert_eq!(st, 200);
    let v: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(v["state"], "stopped", "{v}");
    assert!(v.get("config").is_none_or(Value::is_null), "{v}");

    // the same POST with the right Host is answered by the API (what it answers depends on the
    // machine running the tests: no isolated cores, no dpds binary), not by the guard
    let (st, text) = call(&c.addr, "POST", "/api/emulation/start", &good, Some(&body));
    assert!(!rejected(st) && st != 202, "{st} {text}");
}

#[test]
fn ip_literals_localhost_and_the_machine_hostname_are_accepted() {
    let c = ControllerProcess::start(&["serve", "--bind", "{addr}"], &[]);
    let hostname = machine_hostname();
    let short = hostname.split('.').next().unwrap_or(&hostname).to_string();
    let mut hosts: Vec<String> = [
        "127.0.0.1",
        "127.0.0.1:8100",
        "10.1.2.3",
        "192.168.0.5:1234",
        "[::1]",
        "[::1]:8100",
        "[2001:db8::1]:80",
        "[fe80::1]",
        "localhost",
        "localhost:8100",
        "Localhost:8100",
        "LOCALHOST",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for h in [&hostname, &short] {
        hosts.push(h.clone());
        hosts.push(format!("{h}:8100"));
    }
    for host in &hosts {
        assert_online(&c.addr, host);
    }
    // and the POST routes get past the guard with them (their own answer depends on the machine)
    let (st, text) = call(
        &c.addr,
        "POST",
        "/api/host/hints",
        "localhost:8100",
        Some(&setup(vec![link("ok", "net_null0", "net_null0")]).body()),
    );
    assert_eq!(st, 200, "{text}");
}

#[test]
fn the_port_is_stripped_before_the_comparison() {
    let c = ControllerProcess::start(&["serve", "--bind", "{addr}"], &[]);
    // a port does not make a foreign name acceptable ...
    for host in [
        "attacker.example:80",
        "attacker.example:8100",
        "attacker.example:1",
    ] {
        let (st, _) = call(&c.addr, "GET", "/api/emulation", host, None);
        assert!(rejected(st), "{host}: {st}");
    }
    // ... and any port keeps an accepted one accepted
    for host in [
        "localhost:1",
        "localhost:65535",
        "127.0.0.1:1",
        "[::1]:65535",
    ] {
        assert_online(&c.addr, host);
    }
}

#[test]
fn allowed_host_flags_add_names_and_nothing_else() {
    let c = ControllerProcess::start(
        &[
            "serve",
            "--bind",
            "{addr}",
            "--allowed-host",
            "dpds.lab.example",
            "--allowed-host",
            "Other.Example",
        ],
        &[],
    );
    for host in [
        "dpds.lab.example",
        "dpds.lab.example:8080",
        "other.example",
        "other.example:3000",
    ] {
        assert_online(&c.addr, host);
    }
    for host in [
        "attacker.example",
        "lab.example",
        "sub.dpds.lab.example",
        "dpds.lab.example.attacker.example",
    ] {
        let (st, _) = call(&c.addr, "GET", "/api/online", host, None);
        assert!(rejected(st), "{host}: {st}");
    }
    // the built-in names stay accepted
    assert_online(&c.addr, "localhost");
    assert_online(&c.addr, "10.0.0.1:8100");
}

#[test]
fn allowed_hosts_can_come_from_the_environment_comma_separated() {
    let c = ControllerProcess::start(
        &["serve", "--bind", "{addr}"],
        &[("DPDS_ALLOWED_HOST", "a.example,b.example")],
    );
    for host in ["a.example", "b.example:8080"] {
        assert_online(&c.addr, host);
    }
    let (st, _) = call(&c.addr, "GET", "/api/online", "c.example", None);
    assert!(rejected(st), "{st}");
}
