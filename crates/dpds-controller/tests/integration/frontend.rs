//! Spec v13: the controller serves the web frontend itself (embedded at compile time) next to
//! `/api`, and reports what it embeds in `GET /api/version`.
//!
//! # Test-facing API the tests assume (library crate `dpds_controller`)
//!
//! ```ignore
//! pub mod frontend {
//!     /// What answers every non-`/api` GET/HEAD. Cheap to clone.
//!     #[derive(Clone)]
//!     pub struct Frontend;
//!     impl Frontend {
//!         /// What `build.rs` embedded: `$DPDS_FRONTEND_DIST`, else `frontend/dist`, else the placeholder.
//!         pub fn embedded() -> Frontend;
//!         /// The crate's placeholder page, whatever was embedded ("Frontend not bundled: install it
//!         /// with `dpds.sh install`, or run `npm run build` in frontend/").
//!         pub fn placeholder() -> Frontend;
//!         /// A bundled frontend served from `dir` (an `include_dir!` of a dist directory, which has
//!         /// an `index.html`). Tests pass a fixture; `embedded()` is this over the build's dir.
//!         pub fn from_dir(dir: &'static include_dir::Dir<'static>) -> Frontend;
//!     }
//! }
//! pub mod api {
//!     /// Unchanged: the API plus `Frontend::embedded()`.
//!     pub fn router(deps: Deps) -> Router;
//!     /// New: the same router, serving `frontend` for everything outside `/api`.
//!     pub fn router_with(deps: Deps, frontend: Frontend) -> Router;
//! }
//! ```
//!
//! `include_dir` (0.7) is a normal dependency of the crate, so the tests use it directly. The
//! fixture dist is `tests/fixtures/frontend/`; `tests/fixtures/secret.txt` lies outside it.
//! `main` wraps the router from `router`/`router_with` in `with_host_guard` as before, so the
//! guard has to cover the static responses too (the tests wrap it the same way).
//!
//! # HTTP contract
//!
//! - Outside `/api` (`/api` itself and everything below it belong to the API), GET and HEAD
//!   answer from the embedded files; a path that is no file answers `index.html` (SPA fallback,
//!   status 200). Query strings are ignored. Other methods: 405.
//! - `/api/**` is untouched: unknown paths stay a JSON 404 `{"message"}` even when the dist holds
//!   a file with that path, and the OpenAPI document and Swagger UI keep working.
//! - Headers on every static response, fallback included: `Content-Type` from the file name
//!   (`application/octet-stream` for an unknown extension), `X-Content-Type-Options: nosniff`,
//!   a `Content-Security-Policy` with `default-src 'self'` and `frame-ancestors 'none'` and no
//!   `unsafe-eval` (and no `unsafe-inline` for scripts). `Cache-Control`: `/assets/*` is
//!   `public, max-age=31536000, immutable`, `index.html` (also via the fallback) is `no-cache`;
//!   other files are not `immutable`.
//! - `GET /api/version` answers `{"version": <crate version>, "frontend": "bundled"|"placeholder",
//!   "git"?: <non-empty string>}`; it is in the OpenAPI document.
//! - Not pinned (the spec leaves it open): the answer for a missing file below `/assets/`
//!   (the fallback page or a 404), `Content-Encoding`, the body of a 405.
//!
//! The binary tests at the end start the real `dpds-controller`; they work with whatever the
//! build embedded (a developer machine usually has `frontend/dist`, CI has the placeholder), and
//! the startup log must say which of the two it is.
//! The build-script test runs a nested `cargo check` of this crate in a fresh target dir (about
//! 10 s on 6 cores with a warm registry, no network needed, several times that on a loaded or
//! small CI box). It stays `#[ignore]`d on purpose: it compiles the whole dependency tree again
//! inside the default parallel run. (Its CPU burst once made
//! `host_fixes::handlers_that_read_sysfs_do_not_block_the_async_workers` hang in about one run in
//! ten; that test has since been made unable to hang, see its `release_fifo`, but the cost
//! remains.) CI's `-- --ignored --test-threads=1` step runs it alone:
//! `cargo test -p dpds-controller --test integration build_script -- --ignored`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::Router;
use include_dir::{include_dir, Dir};
use serde_json::Value;
use tempfile::TempDir;

use crate::common::*;
use dpds_controller::api::{self, Deps};
use dpds_controller::frontend::Frontend;

static FIXTURE: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/tests/fixtures/frontend");

const INDEX_MARKER: &str = "FIXTURE-INDEX";
const JS: &str = "/assets/app-3f9a1c.js";
const CSS: &str = "/assets/app-3f9a1c.css";
const FONT: &str = "/assets/icons-7d2e.woff2";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// A router with the guard of `main`, plus what has to stay alive for it.
struct Site {
    router: Router,
    _host: FakeHost,
}

fn site_with(frontend: Frontend, allowed_hosts: &[String]) -> Site {
    let host = FakeHost::empty();
    let router = api::router_with(
        Deps {
            roots: host.roots(),
            supervisor: FakeSupervisor::new(),
            telemetry: FakeTelemetry::new(),
            dpds_bin: PathBuf::from("/usr/local/bin/dpds"),
            runtime_dir: PathBuf::from("/run/dpds-controller-test"),
            dpds_has_telemetry: true,
            is_root: false,
        },
        frontend,
    );
    Site {
        router: api::with_host_guard(router, allowed_hosts),
        _host: host,
    }
}

fn fixture_site() -> Site {
    site_with(Frontend::from_dir(&FIXTURE), &[])
}

fn placeholder_site() -> Site {
    site_with(Frontend::placeholder(), &[])
}

async fn call(site: &Site, method: &str, path: &str, headers: &[(&str, &str)]) -> Reply {
    let mut req = axum::http::Request::builder().method(method).uri(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    send(&site.router, req.body(axum::body::Body::empty()).unwrap()).await
}

async fn get(site: &Site, path: &str) -> Reply {
    call(site, "GET", path, &[]).await
}

fn header(r: &Reply, name: &str) -> String {
    r.headers
        .get(name)
        .unwrap_or_else(|| panic!("no {name} header; headers: {:?}", r.headers))
        .to_str()
        .unwrap()
        .to_string()
}

fn fixture_file(path: &str) -> &'static [u8] {
    FIXTURE
        .get_file(path)
        .unwrap_or_else(|| panic!("no fixture file {path}"))
        .contents()
}

fn fixture_index() -> String {
    String::from_utf8(fixture_file("index.html").to_vec()).unwrap()
}

fn assert_security_headers(r: &Reply, what: &str) {
    assert_eq!(header(r, "x-content-type-options"), "nosniff", "{what}");
    let csp = header(r, "content-security-policy");
    assert!(csp.contains("default-src 'self'"), "{what}: CSP {csp:?}");
    assert!(
        csp.contains("frame-ancestors 'none'"),
        "{what}: CSP {csp:?}"
    );
    assert!(!csp.contains("unsafe-eval"), "{what}: CSP {csp:?}");
    if let Some(script) = csp
        .split(';')
        .map(str::trim)
        .find(|d| d.starts_with("script-src"))
    {
        assert!(!script.contains("unsafe-inline"), "{what}: CSP {csp:?}");
    }
}

// ---- placeholder ---------------------------------------------------------------------------

#[tokio::test]
async fn the_placeholder_page_tells_how_to_get_the_frontend() {
    let s = placeholder_site();
    let r = get(&s, "/").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(header(&r, "content-type").starts_with("text/html"));
    let page = r.text();
    assert!(page.contains("dpds.sh install"), "{page}");
    assert!(page.contains("npm run build"), "{page}");
    assert!(page.to_lowercase().contains("not bundled"), "{page}");
    assert_eq!(header(&r, "cache-control"), "no-cache");
    assert_security_headers(&r, "placeholder /");
}

#[tokio::test]
async fn the_placeholder_is_also_the_spa_fallback() {
    let s = placeholder_site();
    let root = get(&s, "/").await;
    let r = get(&s, "/links/3").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, root.body);
    let v = get(&s, "/api/version").await;
    assert_eq!(v.status, 200, "{}", v.text());
    assert_eq!(v.json()["frontend"], "placeholder");
}

// ---- a bundled frontend ----------------------------------------------------------------------

#[tokio::test]
async fn the_root_serves_the_index_of_the_bundled_dir() {
    let s = fixture_site();
    let r = get(&s, "/").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(header(&r, "content-type").starts_with("text/html"));
    assert_eq!(r.text(), fixture_index());
    assert!(r.text().contains(INDEX_MARKER));
    assert!(!r.text().contains("dpds.sh install"), "placeholder served");
}

#[tokio::test]
async fn index_html_is_served_directly_and_never_cached_without_revalidation() {
    let s = fixture_site();
    for path in ["/index.html", "/"] {
        let r = get(&s, path).await;
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.text(), fixture_index(), "{path}");
        assert_eq!(header(&r, "cache-control"), "no-cache", "{path}");
        assert_security_headers(&r, path);
    }
}

#[tokio::test]
async fn unknown_paths_fall_back_to_index_html() {
    let s = fixture_site();
    for path in [
        "/links",
        "/some/client/route",
        "/links/3/edit",
        "/dashboard/",
        "/assets",
        "/assets/",
        "/no.such.file",
        "/some/client/route?tab=2",
    ] {
        let r = get(&s, path).await;
        assert_eq!(r.status, 200, "{path}: {}", r.text());
        assert_eq!(r.text(), fixture_index(), "{path}");
        assert!(
            header(&r, "content-type").starts_with("text/html"),
            "{path}"
        );
        assert_eq!(header(&r, "cache-control"), "no-cache", "{path}");
        assert_security_headers(&r, path);
    }
}

#[tokio::test]
async fn files_come_with_their_content_type_and_exact_bytes() {
    let s = fixture_site();
    let cases: [(&str, &[&str], &str); 7] = [
        (JS, &["javascript"], "assets/app-3f9a1c.js"),
        (CSS, &["text/css"], "assets/app-3f9a1c.css"),
        (
            FONT,
            &["font/woff2", "application/font-woff2"],
            "assets/icons-7d2e.woff2",
        ),
        ("/favicon.svg", &["image/svg+xml"], "favicon.svg"),
        ("/robots.txt", &["text/plain"], "robots.txt"),
        (
            "/data.unknownext",
            &["application/octet-stream"],
            "data.unknownext",
        ),
        ("/index.html", &["text/html"], "index.html"),
    ];
    for (path, types, file) in cases {
        let r = get(&s, path).await;
        assert_eq!(r.status, 200, "{path}");
        let ct = header(&r, "content-type");
        assert!(
            types.iter().any(|t| ct.contains(t)),
            "{path}: content type {ct:?}, want one of {types:?}"
        );
        assert_eq!(r.body, fixture_file(file), "{path}: bytes differ");
        assert_security_headers(&r, path);
    }
}

#[tokio::test]
async fn a_query_string_does_not_change_the_file() {
    let s = fixture_site();
    let plain = get(&s, JS).await;
    let r = get(&s, &format!("{JS}?v=1")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, plain.body);
    assert!(header(&r, "content-type").contains("javascript"));
    assert_eq!(get(&s, "/?x=1").await.text(), fixture_index());
}

#[tokio::test]
async fn hashed_assets_are_immutable_and_other_files_are_not() {
    let s = fixture_site();
    for path in [JS, CSS, FONT] {
        let r = get(&s, path).await;
        assert_eq!(header(&r, "cache-control"), IMMUTABLE, "{path}");
    }
    for path in ["/favicon.svg", "/robots.txt"] {
        let r = get(&s, path).await;
        let cc = r
            .headers
            .get("cache-control")
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        assert!(!cc.contains("immutable"), "{path}: {cc:?}");
        assert!(!cc.contains("31536000"), "{path}: {cc:?}");
    }
}

#[tokio::test]
async fn path_traversal_never_leaves_the_dist() {
    let s = fixture_site();
    for path in [
        "/../secret.txt",
        "/..%2fsecret.txt",
        "/%2e%2e/secret.txt",
        "/assets/../../secret.txt",
        "/assets/%2e%2e/%2e%2e/secret.txt",
        "/assets/..%5c..%5csecret.txt",
        "/%00",
        "//secret.txt",
    ] {
        let r = get(&s, path).await;
        assert!(
            r.status < 500,
            "{path}: status {} body {}",
            r.status,
            r.text()
        );
        assert!(
            !r.text().contains("FIXTURE-SECRET"),
            "{path} leaked a file outside the dist"
        );
    }
}

// ---- /api is not the frontend ----------------------------------------------------------------

#[tokio::test]
async fn unknown_api_paths_stay_a_json_404_and_never_fall_back() {
    for s in [fixture_site(), placeholder_site()] {
        for (method, path) in [
            ("GET", "/api/unknown"),
            ("GET", "/api"),
            ("GET", "/api/"),
            ("GET", "/api/emulation/extra"),
            ("GET", "/api/assets/app.js"),
            ("POST", "/api/unknown"),
            ("DELETE", "/api/unknown/deep/path"),
            // a file of the dist whose path lies under /api
            ("GET", "/api/leak.json"),
            ("GET", "/api/nothing.js?v=1"),
        ] {
            let r = call(&s, method, path, &[]).await;
            assert_eq!(r.status, 404, "{method} {path}: {}", r.text());
            r.message();
            assert!(!r.text().contains(INDEX_MARKER), "{method} {path}");
            assert!(!r.text().contains("FIXTURE-API-FILE"), "{method} {path}");
        }
    }
}

#[tokio::test]
async fn the_api_keeps_working_next_to_the_frontend() {
    let s = fixture_site();
    let online = get(&s, "/api/online").await;
    assert_eq!(online.status, 200, "{}", online.text());
    online.json();
    let emulation = get(&s, "/api/emulation").await;
    assert_eq!(emulation.status, 200, "{}", emulation.text());
    assert_eq!(emulation.json()["state"], "stopped");
    let openapi = get(&s, "/api/openapi.json").await;
    assert_eq!(openapi.status, 200);
    assert!(openapi.json()["paths"].is_object());
    let docs = get(&s, "/api/docs/").await;
    assert_eq!(docs.status, 200);
    assert!(!docs.text().contains(INDEX_MARKER), "Swagger UI shadowed");
    // wrong methods on API routes are still the API's JSON 405
    let r = call(&s, "DELETE", "/api/emulation", &[]).await;
    assert_eq!(r.status, 405);
    r.message();
}

// ---- methods ---------------------------------------------------------------------------------

#[tokio::test]
async fn only_get_and_head_are_allowed_on_static_paths() {
    let s = fixture_site();
    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        for path in ["/", JS, "/some/client/route", "/index.html"] {
            let r = call(&s, method, path, &[]).await;
            assert_eq!(r.status, 405, "{method} {path}: {}", r.text());
            assert!(!r.text().contains(INDEX_MARKER), "{method} {path}");
            // the JSON-only rule (415) belongs to the API: a JSON body changes nothing here
            let with_body = send(
                &s.router,
                axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await;
            assert_eq!(with_body.status, 405, "{method} {path} with a body");
        }
    }
}

#[tokio::test]
async fn head_answers_like_get_headers() {
    let s = fixture_site();
    for path in ["/", JS, "/some/client/route", "/favicon.svg"] {
        let g = get(&s, path).await;
        let h = call(&s, "HEAD", path, &[]).await;
        assert_eq!(h.status, 200, "HEAD {path}");
        for name in [
            "content-type",
            "cache-control",
            "x-content-type-options",
            "content-security-policy",
        ] {
            assert_eq!(
                h.headers.get(name),
                g.headers.get(name),
                "HEAD {path}: {name} differs from GET"
            );
        }
        if let Some(len) = h.headers.get("content-length") {
            assert_eq!(
                len.to_str().unwrap(),
                g.body.len().to_string(),
                "HEAD {path}"
            );
        }
    }
}

// (the wire-level check that HEAD sends no body is in the binary test below)

// ---- Host guard ------------------------------------------------------------------------------

#[tokio::test]
async fn a_foreign_host_is_refused_on_static_paths_too() {
    let s = fixture_site();
    for method in ["GET", "HEAD"] {
        for path in ["/", "/index.html", JS, "/some/client/route", "/favicon.svg"] {
            for host in [
                "attacker.example",
                "attacker.example:8100",
                "localhost.evil.example",
            ] {
                let r = call(&s, method, path, &[("host", host)]).await;
                assert_eq!(r.status, 421, "{method} {path} Host {host}: {}", r.text());
                if method == "GET" {
                    r.message();
                    assert!(!r.text().contains(INDEX_MARKER), "{path} Host {host}");
                }
            }
        }
    }
}

#[tokio::test]
async fn allowed_hosts_get_the_frontend() {
    let s = site_with(Frontend::from_dir(&FIXTURE), &["dpds.lab".to_string()]);
    for host in [
        "localhost:8100",
        "127.0.0.1",
        "[::1]:8100",
        "10.1.2.3:80",
        "DPDS.lab:8100",
    ] {
        let r = call(&s, "GET", "/", &[("host", host)]).await;
        assert_eq!(r.status, 200, "Host {host}");
        assert_eq!(r.text(), fixture_index(), "Host {host}");
    }
    // a request without a Host header (HTTP/1.0, the in-process harness) is let through
    assert_eq!(get(&s, "/").await.status, 200);
}

// ---- /api/version ----------------------------------------------------------------------------

fn assert_version(v: &Value, frontend: &str) {
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"), "{v}");
    assert_eq!(v["frontend"], frontend, "{v}");
    if let Some(git) = v.get("git") {
        assert!(git.as_str().is_some_and(|g| !g.is_empty()), "git: {v}");
    }
    for key in v.as_object().expect("an object").keys() {
        assert!(
            ["version", "git", "frontend"].contains(&key.as_str()),
            "unexpected key {key} in {v}"
        );
    }
}

#[tokio::test]
async fn the_version_endpoint_reports_the_crate_version_and_the_frontend_kind() {
    let r = get(&fixture_site(), "/api/version").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(header(&r, "content-type").starts_with("application/json"));
    assert_version(&r.json(), "bundled");

    let r = get(&placeholder_site(), "/api/version").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_version(&r.json(), "placeholder");
}

#[tokio::test]
async fn the_version_endpoint_is_read_only_and_guarded() {
    let s = fixture_site();
    for method in ["POST", "PUT", "DELETE"] {
        let r = call(&s, method, "/api/version", &[]).await;
        assert_eq!(r.status, 405, "{method}");
        r.message();
    }
    let r = call(&s, "GET", "/api/version", &[("host", "attacker.example")]).await;
    assert_eq!(r.status, 421);
    r.message();
}

/// Follows a `$ref` into `components.schemas`.
fn schema<'a>(doc: &'a Value, s: &'a Value) -> &'a Value {
    match s["$ref"].as_str() {
        Some(r) => &doc["components"]["schemas"][r.rsplit('/').next().unwrap()],
        None => s,
    }
}

#[tokio::test]
async fn openapi_documents_the_version_endpoint() {
    let doc = get(&fixture_site(), "/api/openapi.json").await.json();
    let op = &doc["paths"]["/api/version"]["get"];
    assert!(
        op.is_object(),
        "GET /api/version missing: {:?}",
        doc["paths"]
    );
    let ok = &op["responses"]["200"]["content"]["application/json"]["schema"];
    let s = schema(&doc, ok);
    for key in ["version", "frontend"] {
        assert!(s["properties"][key].is_object(), "{key} missing in {s}");
        let required = s["required"].as_array().expect("required list");
        assert!(required.iter().any(|r| r == key), "{key} not required: {s}");
    }
}

// ---- what the build embedded -------------------------------------------------------------------

#[tokio::test]
async fn the_embedded_frontend_is_what_the_version_endpoint_says() {
    let host = FakeHost::empty();
    let router = api::router(Deps {
        roots: host.roots(),
        supervisor: FakeSupervisor::new(),
        telemetry: FakeTelemetry::new(),
        dpds_bin: PathBuf::from("/usr/local/bin/dpds"),
        runtime_dir: PathBuf::from("/run/dpds-controller-test"),
        dpds_has_telemetry: true,
        is_root: false,
    });
    let kind = router_get(&router, "/api/version").await.json()["frontend"]
        .as_str()
        .unwrap()
        .to_string();
    let page = router_get(&router, "/").await;
    assert_eq!(page.status, 200);
    let html = page.text();
    match kind.as_str() {
        "placeholder" => assert!(html.contains("dpds.sh install"), "{html}"),
        "bundled" => {
            assert!(html.contains("id=\"root\""), "{html}");
            assert!(!html.contains("dpds.sh install"), "{html}");
        }
        other => panic!("frontend kind {other:?}"),
    }
    assert_eq!(router_get(&router, "/some/client/route").await.text(), html);
    let unknown = router_get(&router, "/api/unknown").await;
    assert_eq!(unknown.status, 404);
    unknown.message();
}

// ---- the real binary -------------------------------------------------------------------------

/// The binary on a free port with its stderr in a file; killed on drop.
struct Logged {
    child: std::process::Child,
    addr: String,
    stderr: PathBuf,
    _dir: TempDir,
}

impl Logged {
    fn start() -> Logged {
        let dir = tempfile::tempdir().unwrap();
        let addr = free_addr();
        let stderr = dir.path().join("stderr.log");
        let child = std::process::Command::new(env!("CARGO_BIN_EXE_dpds-controller"))
            .args(["serve", "--bind", &addr])
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .expect("cannot spawn dpds-controller");
        let mut p = Logged {
            child,
            addr,
            stderr,
            _dir: dir,
        };
        let t0 = Instant::now();
        while !matches!(http(&p.addr, "GET", "/api/online", None), Ok((200, _))) {
            if let Ok(Some(st)) = p.child.try_wait() {
                panic!("dpds-controller exited early ({st}): {}", p.log());
            }
            assert!(t0.elapsed() < Duration::from_secs(15), "{}", p.log());
            std::thread::sleep(Duration::from_millis(100));
        }
        p
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.stderr).unwrap_or_default()
    }

    /// (status, lower-cased header lines, body)
    fn raw(&self, method: &str, path: &str, host: &str) -> (u16, Vec<String>, String) {
        http_raw(
            &self.addr,
            &format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        )
        .unwrap_or_else(|e| panic!("{method} {path}: {e}"))
    }
}

impl Drop for Logged {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_binary_serves_the_frontend_on_its_own_port_behind_the_host_guard() {
    let c = Logged::start();
    let (st, _, version) = c.raw("GET", "/api/version", "127.0.0.1");
    assert_eq!(st, 200, "{version}");
    let version: Value = serde_json::from_str(version.trim()).expect("JSON version");
    assert_version(&version, version["frontend"].as_str().unwrap_or("?"));
    let kind = version["frontend"].as_str().unwrap().to_string();

    let (st, headers, root) = c.raw("GET", "/", "127.0.0.1");
    assert_eq!(st, 200, "{root}");
    assert!(header_value(&headers, "content-type").is_some_and(|t| t.starts_with("text/html")));
    assert_eq!(
        header_value(&headers, "x-content-type-options").as_deref(),
        Some("nosniff")
    );
    assert!(header_value(&headers, "content-security-policy")
        .is_some_and(|c| c.contains("default-src 'self'") && c.contains("frame-ancestors 'none'")));
    assert_eq!(
        header_value(&headers, "cache-control").as_deref(),
        Some("no-cache")
    );
    if kind == "placeholder" {
        assert!(root.contains("dpds.sh install"), "{root}");
    }

    let (st, _, route) = c.raw("GET", "/some/client/route", "localhost:8100");
    assert_eq!(st, 200);
    assert_eq!(route, root);

    let (st, _, body) = c.raw("GET", "/api/unknown", "127.0.0.1");
    assert_eq!(st, 404, "{body}");
    assert!(serde_json::from_str::<Value>(body.trim()).is_ok_and(|v| v["message"].is_string()));

    let (st, _, _) = c.raw("POST", "/", "127.0.0.1");
    assert_eq!(st, 405);

    let (st, _, body) = c.raw("HEAD", "/", "127.0.0.1");
    assert_eq!(st, 200);
    assert!(body.is_empty(), "HEAD has a body: {body:?}");

    for path in ["/", "/some/client/route", "/index.html"] {
        let (st, _, body) = c.raw("GET", path, "attacker.example");
        assert_eq!(st, 421, "{path}: {body}");
        assert!(
            serde_json::from_str::<Value>(body.trim()).is_ok_and(|v| v["message"].is_string()),
            "{path}: {body}"
        );
    }
}

#[test]
fn the_startup_log_says_which_frontend_is_embedded() {
    let c = Logged::start();
    let (_, _, version) = c.raw("GET", "/api/version", "127.0.0.1");
    let kind = serde_json::from_str::<Value>(version.trim()).unwrap()["frontend"]
        .as_str()
        .unwrap()
        .to_string();
    let log = c.log();
    let line = log
        .lines()
        .find(|l| l.to_lowercase().contains("frontend"))
        .unwrap_or_else(|| panic!("no startup line mentions the frontend: {log}"))
        .to_lowercase();
    match kind.as_str() {
        "placeholder" => assert!(line.contains("placeholder"), "{line}"),
        "bundled" => {
            assert!(line.contains("bundled"), "{line}");
            assert!(!line.contains("placeholder"), "{line}");
        }
        other => panic!("frontend kind {other:?}"),
    }
}

// ---- build.rs --------------------------------------------------------------------------------

fn cargo_check(dist: impl AsRef<std::ffi::OsStr>, target: &Path) -> std::process::Output {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["check", "--lib", "--manifest-path"])
        .arg(manifest)
        .env("DPDS_FRONTEND_DIST", dist.as_ref())
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("cannot run cargo")
}

#[test]
#[ignore = "nested cargo check; run with --ignored (CPU burst, see the module header)"]
fn build_script_rejects_a_dist_without_index_html_and_accepts_one_with() {
    let target = tempfile::tempdir().unwrap();
    let dist = tempfile::tempdir().unwrap();
    std::fs::write(dist.path().join("app.js"), "x").unwrap();

    let out = cargo_check(dist.path(), target.path());
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(!out.status.success(), "built without an index.html: {err}");
    assert!(err.contains("DPDS_FRONTEND_DIST"), "{err}");
    assert!(err.contains("index.html"), "{err}");
    assert!(
        err.contains(dist.path().to_str().unwrap()),
        "the message must name the directory: {err}"
    );

    std::fs::write(dist.path().join("index.html"), "<html></html>").unwrap();
    let out = cargo_check(dist.path(), target.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // An empty `DPDS_FRONTEND_DIST=` counts as unset (shell wrappers and CI matrices export it
    // empty): the build falls back to `frontend/dist` or the placeholder instead of failing on
    // the empty path.
    let out = cargo_check("", target.path());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "an empty DPDS_FRONTEND_DIST must count as unset: {err}"
    );
}

// ---- spec 18: the browser checks for a new release ---------------------------------------------

/// The directives of a CSP as `(name, sorted sources)`, white space normalised.
fn directives(csp: &str) -> std::collections::BTreeMap<String, Vec<String>> {
    csp.split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| {
            let mut parts = d.split_whitespace();
            let name = parts.next().unwrap().to_string();
            let mut sources: Vec<String> = parts.map(str::to_string).collect();
            sources.sort();
            (name, sources)
        })
        .collect()
}

#[tokio::test]
async fn connect_src_allows_the_raw_githubusercontent_version_file_and_nothing_else_new() {
    for (what, site) in [
        ("fixture", fixture_site()),
        ("placeholder", placeholder_site()),
    ] {
        // the placeholder dist has no assets: /assets/... would only be the 404/fallback there
        let paths: &[&str] = if what == "fixture" {
            &["/", "/index.html", "/links/3", JS]
        } else {
            &["/", "/index.html", "/links/3"]
        };
        for &path in paths {
            let r = get(&site, path).await;
            assert_eq!(r.status, 200, "{what} {path}");
            let csp = header(&r, "content-security-policy");
            let d = directives(&csp);
            assert_eq!(
                d["connect-src"],
                ["'self'", "https://raw.githubusercontent.com"],
                "{what} {path}: {csp}"
            );
            // every other directive is what it was before the update check
            let expected: &[(&str, &[&str])] = &[
                ("default-src", &["'self'"]),
                ("script-src", &["'self'"]),
                ("style-src", &["'self'", "'unsafe-inline'"]),
                ("img-src", &["'self'", "data:"]),
                ("font-src", &["'self'"]),
                ("object-src", &["'none'"]),
                ("base-uri", &["'self'"]),
                ("form-action", &["'self'"]),
                ("frame-ancestors", &["'none'"]),
            ];
            for (name, sources) in expected {
                let mut want: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
                want.sort();
                assert_eq!(d[*name], want, "{what} {path}: {name} in {csp}");
            }
            assert_eq!(
                d.len(),
                expected.len() + 1,
                "{what} {path}: extra directive in {csp}"
            );
            assert!(!csp.contains('*'), "{what} {path}: wildcard in {csp}");
        }
    }
}

#[tokio::test]
async fn the_version_endpoint_reports_the_release_version_with_a_bundled_frontend() {
    let v = get(&fixture_site(), "/api/version").await.json();
    assert_eq!(v["version"], "1.0.0", "{v}");
    let v = get(&placeholder_site(), "/api/version").await.json();
    assert_eq!(v["version"], "1.0.0", "{v}");
}
