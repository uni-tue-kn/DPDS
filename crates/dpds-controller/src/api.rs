//! The REST API (B2): routes under `/api`, Swagger UI at `/api/docs`, the
//! OpenAPI document at `/api/openapi.json`. Error bodies are `{"message": ...}`.

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{
    DefaultBodyLimit, FromRequest, FromRequestParts, Path, RawQuery, Request, State,
};
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::discovery::{discover, find, Interface};
use crate::dto::{
    ConfigDto, DelayerDto, DistributionDto, HistogramDto, LimiterDto, LimiterModeDto, LossModelDto,
};
use crate::eal::{drain_timeout, eal_argv, links_file_json, EalInput, EalPort};
use crate::frontend::Frontend;
use crate::hints::{compute_hints, Hint, Level};
use crate::host::HostRoots;
use crate::lcores::{assign_lcores, DirectionPlan};
use crate::model::{Link, Preset, Settings, StoredState, MAX_NAME_CHARS};
use crate::stats::{
    compute_stats, delay_histogram, delay_stats_on, uptime_ms, Clamped, Counters, DirectionStats,
    ExpectedModel, HistogramView, Percentiles, PortStats, StatsView,
};
use crate::store::{self, StoreError};
use crate::supervisor::{LaunchPlan, State as RunState, Supervisor};
use crate::telemetry::TelemetrySource;
use crate::validate;

/// Largest accepted request body (links with their configs are a few KB).
const BODY_LIMIT: usize = 1024 * 1024;

pub struct Deps {
    pub state_dir: PathBuf,
    pub roots: HostRoots,
    pub supervisor: Arc<dyn Supervisor>,
    pub telemetry: Arc<dyn TelemetrySource>,
    pub dpds_bin: PathBuf,
    pub runtime_dir: PathBuf,
    /// The `dpds` binary was built with the `telemetry` feature.
    pub dpds_has_telemetry: bool,
    pub is_root: bool,
}

struct App {
    deps: Deps,
    frontend: Frontend,
    stored: Mutex<StoredState>,
    /// Serialises edits, so the (slow) save runs without holding `stored`.
    edits: Mutex<()>,
    /// Edited since the running emulation started.
    pending_restart: AtomicBool,
    /// Directions and main lcore of the last launch.
    last_plan: Mutex<Option<(u32, Vec<DirectionPlan>)>>,
    /// The per-direction configs of the last launch (the histogram's expected counts
    /// follow what is running, not what was edited since).
    launched_configs: Mutex<Vec<ConfigDto>>,
    /// Serialises start and stop: the state check and the launch are one step.
    lifecycle: AsyncMutex<()>,
}

type Shared = State<Arc<App>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- errors ------------------------------------------------------------------

pub struct ApiError(StatusCode, String);

impl ApiError {
    fn bad(m: impl Into<String>) -> ApiError {
        ApiError(StatusCode::BAD_REQUEST, m.into())
    }
    fn conflict(m: impl Into<String>) -> ApiError {
        ApiError(StatusCode::CONFLICT, m.into())
    }
    fn not_found(m: impl Into<String>) -> ApiError {
        ApiError(StatusCode::NOT_FOUND, m.into())
    }
    fn internal(m: impl Into<String>) -> ApiError {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, m.into())
    }
    /// A part of an import document is wrong: 400 (also for what the single
    /// endpoints answer with 409), with the part named.
    fn in_document(self, part: &str) -> ApiError {
        ApiError(StatusCode::BAD_REQUEST, format!("{part}: {}", self.1))
    }
}

/// Runs blocking file work (sysfs, procfs) off the async workers.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("handler task failed: {e}")))
}

fn is_json_content_type(parts: &Parts) -> bool {
    let Some(ct) = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let essence = ct
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence == "application/json"
        || essence
            .strip_prefix("application/")
            .is_some_and(|s| s.ends_with("+json"))
}

fn not_json() -> ApiError {
    ApiError(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "expected a request with `Content-Type: application/json`".into(),
    )
}

/// A JSON body like axum's `Json`, with `{"message"}` errors: 415 without a JSON
/// content type (checked first), 413 over the body limit, 400 for a bad body.
struct ApiJson<T>(T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(ApiJson(v)),
            Err(r) => {
                let status = match r.status() {
                    StatusCode::UNPROCESSABLE_ENTITY => StatusCode::BAD_REQUEST,
                    other => other,
                };
                let body = r.body_text();
                let message = if status == StatusCode::BAD_REQUEST {
                    format!("invalid request body: {body}")
                } else {
                    body
                };
                Err(ApiError(status, message))
            }
        }
    }
}

/// Requires a JSON content type on a POST that reads no body.
struct RequireJson;

impl<S: Send + Sync> FromRequestParts<S> for RequireJson {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        if is_json_content_type(parts) {
            Ok(RequireJson)
        } else {
            Err(not_json())
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> ApiError {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "message": self.1 }))).into_response()
    }
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    message: String,
}

// ---- views -------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
pub struct OnlineView {
    version: String,
    dpds_bin: String,
    /// The `dpds` binary has the `telemetry` feature (needed for start and stats).
    telemetry: bool,
    root: bool,
}

/// `GET /api/version`.
#[derive(Serialize, ToSchema)]
pub struct VersionView {
    version: String,
    /// `bundled` when the build embedded a real frontend, `placeholder` when it did not.
    frontend: String,
}

#[derive(Serialize, ToSchema)]
pub struct EmulationView {
    state: RunState,
    pid: Option<u32>,
    /// Unix seconds.
    started_at: Option<u64>,
    directions: Vec<DirectionPlan>,
    main_lcore: Option<u32>,
    /// Links or settings changed since the emulation started.
    pending_restart: bool,
    error: Option<String>,
    log_tail: Vec<String>,
}

/// Body of `POST /api/links` and `PUT /api/links/{id}`.
#[derive(Deserialize, ToSchema)]
pub struct LinkInput {
    name: String,
    /// DPDK device name packets are received on (PCI address or vdev).
    ingress: String,
    /// DPDK device name packets leave on; may equal `ingress` (hairpin). A port is
    /// the ingress of at most one link and the egress of at most one link.
    egress: String,
    config: ConfigDto,
    /// Worker lcore; automatic when absent.
    worker: Option<u32>,
}

/// Body of `POST /api/presets` and `PUT /api/presets/{id}`.
#[derive(Deserialize, ToSchema)]
pub struct PresetInput {
    /// 1 to 64 characters, unique.
    name: String,
    description: Option<String>,
    config: ConfigDto,
}

/// `settings` of an export document (the fields of [`Settings`], unknown keys refused).
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportSettings {
    main_lcore: Option<u32>,
    eal_extra_args: Vec<String>,
    poll_interval_ms: u64,
}

/// A link of an export document: [`LinkInput`] without ids, unknown keys refused.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportLink {
    name: String,
    ingress: String,
    egress: String,
    config: ConfigDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worker: Option<u32>,
}

/// A preset of an export document: [`PresetInput`], unknown keys refused.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportPreset {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    config: ConfigDto,
}

/// `GET /api/export` and the body of `POST /api/import`: the stored setup without
/// ids and runtime state.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportDoc {
    /// Always `dpds-controller-export`.
    format: String,
    /// Always 2 (version 1 had two-port links).
    version: u32,
    settings: ExportSettings,
    links: Vec<ExportLink>,
    presets: Vec<ExportPreset>,
}

/// What an import stored.
#[derive(Serialize, ToSchema)]
pub struct ImportSummary {
    links: usize,
    presets: usize,
}

const EXPORT_FORMAT: &str = "dpds-controller-export";
const EXPORT_VERSION: u32 = 2;
const MAX_PRESET_NAME_CHARS: usize = 64;

// ---- helpers -----------------------------------------------------------------

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= MAX_NAME_CHARS
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// The link-name rule shared by every path that accepts or launches links.
fn check_link_name(name: &str) -> Result<(), ApiError> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(ApiError::bad(format!(
            "link name {name:?} is invalid: a link name must be 1 to {MAX_NAME_CHARS} characters of A-Z a-z 0-9 _ . -"
        )))
    }
}

/// Most keys of the `/dpds/ports` telemetry dict of `dpds` (DPDK's
/// `RTE_TEL_MAX_DICT_ENTRIES`).
const MAX_TELEMETRY_DICT_ENTRIES: usize = 256;

/// `dpds` publishes `3 + 5 * distinct ports + 10 * links` keys in one telemetry dict and
/// refuses to start above 256: refuse such a setup here, before it is stored or launched.
fn check_dict_limit<'a>(links: impl Iterator<Item = &'a Link>) -> Result<(), ApiError> {
    let mut ports: Vec<&str> = Vec::new();
    let mut count = 0;
    for l in links {
        count += 1;
        for p in [l.ingress.as_str(), l.egress.as_str()] {
            if !ports.contains(&p) {
                ports.push(p);
            }
        }
    }
    let entries = 3 + 5 * ports.len() + 10 * count;
    if entries > MAX_TELEMETRY_DICT_ENTRIES {
        return Err(ApiError::bad(format!(
            "{count} links over {} ports need {entries} telemetry entries (3 + 5 per port + 10 per link), more than the limit of {MAX_TELEMETRY_DICT_ENTRIES} of dpds",
            ports.len()
        )));
    }
    Ok(())
}

/// Validates a link and returns its ingress and egress, normalised (PCI addresses
/// in lower case).
fn validate_link(input: &LinkInput, others: &[&Link]) -> Result<(String, String), ApiError> {
    check_link_name(&input.name)?;
    let ingress = validate::port_name(&input.ingress).map_err(ApiError::bad)?;
    let egress = validate::port_name(&input.egress).map_err(ApiError::bad)?;
    input
        .config
        .to_core()
        .map_err(|e| ApiError::bad(format!("config: {e}")))?;
    if let Some(o) = others.iter().find(|o| o.name == input.name) {
        return Err(ApiError::conflict(format!(
            "a link named {} already exists",
            o.name
        )));
    }
    check_ports_free(&input.name, &ingress, &egress, others)?;
    Ok((ingress, egress))
}

/// A port is the ingress of at most one link and the egress of at most one link.
fn check_ports_free(
    name: &str,
    ingress: &str,
    egress: &str,
    others: &[&Link],
) -> Result<(), ApiError> {
    for o in others {
        if o.ingress == ingress {
            return Err(ApiError::conflict(format!(
                "port {ingress} is already the ingress of link {} (link {name})",
                o.name
            )));
        }
        if o.egress == egress {
            return Err(ApiError::conflict(format!(
                "port {egress} is already the egress of link {} (link {name})",
                o.name
            )));
        }
    }
    Ok(())
}

fn parse_id(kind: &str, s: &str) -> Result<u32, ApiError> {
    s.parse()
        .map_err(|_| ApiError::not_found(format!("no {kind} {s}")))
}

/// Validates a preset against the others: name 1 to 64 characters and unique, the
/// config like a link direction.
fn validate_preset(name: &str, config: &ConfigDto, others: &[&Preset]) -> Result<(), ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::bad("preset name must not be empty"));
    }
    if name.chars().count() > MAX_PRESET_NAME_CHARS {
        return Err(ApiError::bad(format!(
            "preset name must be at most {MAX_PRESET_NAME_CHARS} characters"
        )));
    }
    config
        .to_core()
        .map_err(|e| ApiError::bad(format!("config: {e}")))?;
    if others.iter().any(|o| o.name == name) {
        return Err(ApiError::conflict(format!(
            "a preset named {name} already exists"
        )));
    }
    Ok(())
}

fn validate_settings(settings: &Settings) -> Result<(), ApiError> {
    if settings.poll_interval_ms == 0 {
        return Err(ApiError::bad("poll_interval_ms must be at least 1"));
    }
    validate::eal_extra_args(&settings.eal_extra_args).map_err(ApiError::bad)
}

/// The value of `key` in a raw query string (an empty one for `key` without `=`).
fn query_param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    query?
        .split('&')
        .map(|kv| kv.split_once('=').unwrap_or((kv, "")))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// `window_s`: 0 (absent) for since start, or 10, 60, 300 seconds.
fn parse_window(query: Option<&str>) -> Result<u32, ApiError> {
    let Some(v) = query_param(query, "window_s") else {
        return Ok(0);
    };
    match v.parse::<u32>() {
        Ok(n) if v.bytes().all(|b| b.is_ascii_digit()) && matches!(n, 0 | 10 | 60 | 300) => Ok(n),
        _ => Err(ApiError::bad(format!(
            "window_s = {v:?}: must be 0 (since start), 10, 60 or 300 seconds"
        ))),
    }
}

impl App {
    fn stored(&self) -> MutexGuard<'_, StoredState> {
        lock(&self.stored)
    }

    /// Applies `edit` to a copy of the stored state, saves it, then swaps it in.
    /// Runs on a blocking thread (the save syncs to disk), to completion even if
    /// the request is dropped; readers of `stored` never wait for the disk.
    ///
    /// `touches_run`: the edit changes what a running emulation was started with, so
    /// it sets `pending_restart` (presets do not).
    async fn update<T: Send + 'static>(
        self: &Arc<Self>,
        touches_run: bool,
        edit: impl FnOnce(&mut StoredState) -> Result<T, ApiError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let app = self.clone();
        blocking(move || app.update_blocking(touches_run, edit)).await?
    }

    fn update_blocking<T>(
        &self,
        touches_run: bool,
        edit: impl FnOnce(&mut StoredState) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let _serial = lock(&self.edits);
        let mut next = self.stored().clone();
        let out = edit(&mut next)?;
        store::save(&self.deps.state_dir, &next)?;
        *self.stored() = next;
        if touches_run
            && matches!(
                self.deps.supervisor.status().state,
                RunState::Starting | RunState::Running
            )
        {
            self.pending_restart.store(true, Ordering::Relaxed);
        }
        Ok(out)
    }

    fn emulation(&self) -> EmulationView {
        let st = self.deps.supervisor.status();
        let launched = if st.state == RunState::Stopped {
            None
        } else {
            lock(&self.last_plan).clone()
        };
        let (main_lcore, directions) = match launched {
            Some((m, d)) => (Some(m), d),
            None => match assign_lcores(&self.deps.roots, &self.stored()) {
                Ok(a) => (Some(a.main_lcore), a.directions),
                Err(_) => (None, Vec::new()),
            },
        };
        EmulationView {
            state: st.state,
            pid: st.pid,
            started_at: st.started_at,
            directions,
            main_lcore,
            pending_restart: self.pending_restart.load(Ordering::Relaxed)
                && matches!(st.state, RunState::Starting | RunState::Running),
            error: st.error,
            log_tail: st.log_tail,
        }
    }

    /// The config of global direction `d`: what the running emulation was started
    /// with, else the stored one.
    fn direction_config(&self, d: usize) -> Option<ConfigDto> {
        if self.deps.supervisor.status().state != RunState::Stopped {
            let launched = lock(&self.launched_configs);
            if !launched.is_empty() {
                return launched.get(d).cloned();
            }
        }
        self.stored().links.get(d).map(|l| l.config.clone())
    }

    fn launch_plan(&self) -> Result<(LaunchPlan, u32, Vec<ConfigDto>), ApiError> {
        let stored = self.stored().clone();
        if stored.links.is_empty() {
            return Err(ApiError::bad("no links are configured"));
        }
        // the state file may have been edited by hand or written by an older
        // version: what reaches dpds' command line passes the same rules as an edit
        for (i, l) in stored.links.iter().enumerate() {
            check_link_name(&l.name)?;
            if stored.links[..i].iter().any(|o| o.name == l.name) {
                return Err(ApiError::conflict(format!(
                    "a link named {} already exists",
                    l.name
                )));
            }
            validate::port_name(&l.ingress).map_err(ApiError::bad)?;
            validate::port_name(&l.egress).map_err(ApiError::bad)?;
            let before: Vec<&Link> = stored.links[..i].iter().collect();
            check_ports_free(&l.name, &l.ingress, &l.egress, &before)?;
        }
        check_dict_limit(stored.links.iter())?;
        validate::eal_extra_args(&stored.settings.eal_extra_args).map_err(ApiError::bad)?;
        let interfaces = discover(&self.deps.roots);
        let mut ports: Vec<EalPort> = Vec::new();
        for port in stored.links.iter().flat_map(|l| [&l.ingress, &l.egress]) {
            let found = find(&interfaces, port);
            if let Some(i) = found.filter(|i| i.kernel_route) {
                return Err(ApiError::conflict(format!(
                    "port {port} cannot be used: {}",
                    i.reason.as_deref().unwrap_or("it carries a kernel route")
                )));
            }
            if !ports.iter().any(|p| p.name == *port) {
                ports.push(EalPort {
                    name: port.clone(),
                    driver: found.and_then(|i| i.driver.clone()),
                });
            }
        }
        let assignment = assign_lcores(&self.deps.roots, &stored)
            .map_err(|e| ApiError::bad(format!("cannot assign lcores: {e}")))?;
        let links_json = links_file_json(&stored.links, &assignment.directions)
            .map_err(|e| ApiError::bad(e.to_string()))?;
        let configs = stored
            .links
            .iter()
            .map(|l| l.config.to_core().map_err(|e| ApiError::bad(e.to_string())))
            .collect::<Result<Vec<_>, _>>()?;
        let mut workers: Vec<u32> = assignment.directions.iter().map(|d| d.lcore).collect();
        workers.sort_unstable();
        let links_file = self.deps.runtime_dir.join("links.json");
        let argv = eal_argv(&EalInput {
            main_lcore: assignment.main_lcore,
            worker_lcores: workers,
            ports,
            eal_extra_args: stored.settings.eal_extra_args.clone(),
            links_file: links_file.clone(),
        });
        let plan = LaunchPlan {
            dpds_bin: self.deps.dpds_bin.clone(),
            runtime_dir: self.deps.runtime_dir.clone(),
            links_file,
            links_json,
            argv,
            directions: assignment.directions,
            main_lcore: assignment.main_lcore,
            drain_timeout: drain_timeout(&configs),
        };
        let dtos = stored.links.iter().map(|l| l.config.clone()).collect();
        Ok((plan, assignment.main_lcore, dtos))
    }
}

// ---- handlers ----------------------------------------------------------------

#[utoipa::path(get, path = "/api/online", responses((status = 200, body = OnlineView)))]
async fn online(State(app): Shared) -> Json<OnlineView> {
    Json(OnlineView {
        version: env!("CARGO_PKG_VERSION").into(),
        dpds_bin: app.deps.dpds_bin.to_string_lossy().into_owned(),
        telemetry: app.deps.dpds_has_telemetry,
        root: app.deps.is_root,
    })
}

#[utoipa::path(get, path = "/api/version", responses((status = 200, body = VersionView)))]
async fn version(State(app): Shared) -> Json<VersionView> {
    Json(VersionView {
        version: env!("CARGO_PKG_VERSION").into(),
        frontend: app.frontend.kind().into(),
    })
}

#[utoipa::path(
    get, path = "/api/interfaces",
    responses((status = 200, body = Vec<Interface>), (status = 500, body = ErrorBody))
)]
async fn interfaces(State(app): Shared) -> Result<Json<Vec<Interface>>, ApiError> {
    Ok(Json(blocking(move || discover(&app.deps.roots)).await?))
}

#[utoipa::path(
    get, path = "/api/host/hints",
    responses((status = 200, body = Vec<Hint>), (status = 500, body = ErrorBody))
)]
async fn host_hints(State(app): Shared) -> Result<Json<Vec<Hint>>, ApiError> {
    let stored = app.stored().clone();
    Ok(Json(
        blocking(move || compute_hints(&app.deps.roots, &stored)).await?,
    ))
}

#[utoipa::path(get, path = "/api/links", responses((status = 200, body = Vec<Link>)))]
async fn list_links(State(app): Shared) -> Json<Vec<Link>> {
    Json(app.stored().links.clone())
}

#[utoipa::path(
    post, path = "/api/links", request_body = LinkInput,
    responses(
        (status = 201, body = Link), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 415, body = ErrorBody), (status = 500, body = ErrorBody)
    )
)]
async fn create_link(
    State(app): Shared,
    ApiJson(input): ApiJson<LinkInput>,
) -> Result<(StatusCode, Json<Link>), ApiError> {
    let link = app
        .update(true, move |s| {
            let (ingress, egress) = validate_link(&input, &s.links.iter().collect::<Vec<_>>())?;
            let link = Link {
                id: s.next_id,
                name: input.name,
                ingress,
                egress,
                config: input.config,
                worker: input.worker,
            };
            check_dict_limit(s.links.iter().chain([&link]))?;
            s.next_id += 1;
            s.links.push(link.clone());
            Ok(link)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(link)))
}

#[utoipa::path(
    get, path = "/api/links/{id}", params(("id" = u32, Path, description = "link id")),
    responses((status = 200, body = Link), (status = 404, body = ErrorBody))
)]
async fn get_link(State(app): Shared, Path(id): Path<String>) -> Result<Json<Link>, ApiError> {
    let id = parse_id("link", &id)?;
    let stored = app.stored();
    stored
        .links
        .iter()
        .find(|l| l.id == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no link {id}")))
}

#[utoipa::path(
    put, path = "/api/links/{id}", params(("id" = u32, Path, description = "link id")), request_body = LinkInput,
    responses(
        (status = 200, body = Link), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 413, body = ErrorBody), (status = 415, body = ErrorBody),
        (status = 500, body = ErrorBody)
    )
)]
async fn update_link(
    State(app): Shared,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<LinkInput>,
) -> Result<Json<Link>, ApiError> {
    let id = parse_id("link", &id)?;
    let link = app
        .update(true, move |s| {
            if !s.links.iter().any(|l| l.id == id) {
                return Err(ApiError::not_found(format!("no link {id}")));
            }
            let (ingress, egress) = validate_link(
                &input,
                &s.links.iter().filter(|l| l.id != id).collect::<Vec<_>>(),
            )?;
            let link = Link {
                id,
                name: input.name,
                ingress,
                egress,
                config: input.config,
                worker: input.worker,
            };
            check_dict_limit(s.links.iter().filter(|l| l.id != id).chain([&link]))?;
            for l in &mut s.links {
                if l.id == id {
                    *l = link.clone();
                }
            }
            Ok(link)
        })
        .await?;
    Ok(Json(link))
}

#[utoipa::path(
    delete, path = "/api/links/{id}", params(("id" = u32, Path, description = "link id")),
    responses((status = 204), (status = 404, body = ErrorBody), (status = 500, body = ErrorBody))
)]
async fn delete_link(State(app): Shared, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let id = parse_id("link", &id)?;
    app.update(true, move |s| {
        let before = s.links.len();
        s.links.retain(|l| l.id != id);
        if s.links.len() == before {
            return Err(ApiError::not_found(format!("no link {id}")));
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/settings", responses((status = 200, body = Settings)))]
async fn get_settings(State(app): Shared) -> Json<Settings> {
    Json(app.stored().settings.clone())
}

#[utoipa::path(
    put, path = "/api/settings", request_body = Settings,
    responses(
        (status = 200, body = Settings), (status = 400, body = ErrorBody), (status = 413, body = ErrorBody),
        (status = 415, body = ErrorBody), (status = 500, body = ErrorBody)
    )
)]
async fn put_settings(
    State(app): Shared,
    ApiJson(settings): ApiJson<Settings>,
) -> Result<Json<Settings>, ApiError> {
    validate_settings(&settings)?;
    let stored = settings.clone();
    app.update(true, move |s| {
        s.settings = stored;
        Ok(())
    })
    .await?;
    Ok(Json(settings))
}

#[utoipa::path(
    get, path = "/api/emulation",
    responses((status = 200, body = EmulationView), (status = 500, body = ErrorBody))
)]
async fn get_emulation(State(app): Shared) -> Result<Json<EmulationView>, ApiError> {
    Ok(Json(blocking(move || app.emulation()).await?))
}

#[utoipa::path(
    post, path = "/api/emulation/start",
    responses(
        (status = 202, body = EmulationView), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 415, body = ErrorBody), (status = 500, body = ErrorBody)
    )
)]
async fn start(
    State(app): Shared,
    _json: RequireJson,
) -> Result<(StatusCode, Json<EmulationView>), ApiError> {
    let _step = app.lifecycle.lock().await;
    let state = app.deps.supervisor.status().state;
    if !matches!(state, RunState::Stopped | RunState::Failed) {
        return Err(ApiError::conflict(format!(
            "cannot start while the emulation is {state:?}"
        )));
    }
    if !app.deps.dpds_has_telemetry {
        return Err(ApiError::bad(format!(
            "{} was built without the telemetry feature; rebuild dpds with --features telemetry",
            app.deps.dpds_bin.display()
        )));
    }
    let a = app.clone();
    let (plan, main_lcore, configs) = blocking(move || a.launch_plan()).await??;
    *lock(&app.last_plan) = Some((main_lcore, plan.directions.clone()));
    *lock(&app.launched_configs) = configs;
    app.pending_restart.store(false, Ordering::Relaxed);
    app.deps.supervisor.start(plan);
    Ok((
        StatusCode::ACCEPTED,
        Json(
            blocking({
                let a = app.clone();
                move || a.emulation()
            })
            .await?,
        ),
    ))
}

#[utoipa::path(
    post, path = "/api/emulation/stop",
    responses(
        (status = 202, body = EmulationView), (status = 409, body = ErrorBody),
        (status = 415, body = ErrorBody), (status = 500, body = ErrorBody)
    )
)]
async fn stop(
    State(app): Shared,
    _json: RequireJson,
) -> Result<(StatusCode, Json<EmulationView>), ApiError> {
    let _step = app.lifecycle.lock().await;
    let state = app.deps.supervisor.status().state;
    if !matches!(state, RunState::Starting | RunState::Running) {
        return Err(ApiError::conflict(format!(
            "cannot stop while the emulation is {state:?}"
        )));
    }
    app.deps.supervisor.stop();
    Ok((
        StatusCode::ACCEPTED,
        Json(
            blocking({
                let a = app.clone();
                move || a.emulation()
            })
            .await?,
        ),
    ))
}

#[utoipa::path(get, path = "/api/stats", responses((status = 200, body = StatsView)))]
async fn stats(State(app): Shared) -> Json<StatsView> {
    Json(match app.deps.telemetry.latest() {
        Some(p) => StatsView {
            age_ms: Some(p.age_ms),
            stale: p.stale,
            ..compute_stats(p.prev.as_ref(), &p.cur)
        },
        // no data (never polled, not running, or cleared after failed polls)
        None => StatsView {
            stale: true,
            ..StatsView::default()
        },
    })
}

#[utoipa::path(
    get, path = "/api/stats/{dir}/delay_histogram",
    params(
        ("dir" = u32, Path, description = "link index, in the order of GET /api/links"),
        ("window_s" = Option<u32>, Query, description = "0 (default, since start), 10, 60 or 300: only the samples of the last n seconds")
    ),
    responses((status = 200, body = HistogramView), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
async fn histogram(
    State(app): Shared,
    Path(dir): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Json<HistogramView>, ApiError> {
    let window_s = parse_window(query.as_deref())?;
    let missing = || ApiError::not_found(format!("no delay histogram for direction {dir}"));
    let d: usize = dir.parse().map_err(|_| missing())?;
    let p = app.deps.telemetry.latest().ok_or_else(missing)?;
    if !delay_stats_on(&p.cur.ports, d) {
        return Err(ApiError::not_found(format!(
            "delay statistics are off for direction {d}, so it has no delay histogram"
        )));
    }
    // the newest poll, and the newest older one that is at least `window_s` old
    let ring = if window_s > 0 {
        app.deps.telemetry.history()
    } else {
        Vec::new()
    };
    let (cur, base) = match ring.split_last() {
        Some(((age_now, newest), older)) => (
            newest,
            older
                .iter()
                .rev()
                .map(|(age, snap)| (age.saturating_sub(*age_now), snap))
                .find(|(span, _)| *span >= u64::from(window_s) * 1000),
        ),
        None => (&p.cur, None),
    };
    let mut h = delay_histogram(cur, d).ok_or_else(missing)?;
    let windowed_span = base.and_then(|(span, snap)| {
        let b = delay_histogram(snap, d)?;
        h.subtract(&b).then_some(span)
    });
    h.span_ms = windowed_span
        .or_else(|| uptime_ms(cur, d))
        .unwrap_or_default();
    h.window_s = window_s;
    h.age_ms = p.age_ms;
    h.stale = p.stale;
    h.summarise();
    if let Some(config) = app.direction_config(d) {
        h.add_expected(&config);
    }
    Ok(Json(h))
}

// ---- presets -----------------------------------------------------------------

#[utoipa::path(get, path = "/api/presets", responses((status = 200, body = Vec<Preset>)))]
async fn list_presets(State(app): Shared) -> Json<Vec<Preset>> {
    Json(app.stored().presets.clone())
}

#[utoipa::path(
    post, path = "/api/presets", request_body = PresetInput,
    responses(
        (status = 201, body = Preset), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 415, body = ErrorBody), (status = 500, body = ErrorBody)
    )
)]
async fn create_preset(
    State(app): Shared,
    ApiJson(input): ApiJson<PresetInput>,
) -> Result<(StatusCode, Json<Preset>), ApiError> {
    let preset = app
        .update(false, move |s| {
            validate_preset(
                &input.name,
                &input.config,
                &s.presets.iter().collect::<Vec<_>>(),
            )?;
            let preset = Preset {
                id: s.next_preset_id,
                name: input.name,
                description: input.description,
                config: input.config,
            };
            s.next_preset_id += 1;
            s.presets.push(preset.clone());
            Ok(preset)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(preset)))
}

#[utoipa::path(
    get, path = "/api/presets/{id}", params(("id" = u32, Path, description = "preset id")),
    responses((status = 200, body = Preset), (status = 404, body = ErrorBody))
)]
async fn get_preset(State(app): Shared, Path(id): Path<String>) -> Result<Json<Preset>, ApiError> {
    let id = parse_id("preset", &id)?;
    app.stored()
        .presets
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no preset {id}")))
}

#[utoipa::path(
    put, path = "/api/presets/{id}", params(("id" = u32, Path, description = "preset id")), request_body = PresetInput,
    responses(
        (status = 200, body = Preset), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody), (status = 413, body = ErrorBody), (status = 415, body = ErrorBody),
        (status = 500, body = ErrorBody)
    )
)]
async fn update_preset(
    State(app): Shared,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<PresetInput>,
) -> Result<Json<Preset>, ApiError> {
    let id = parse_id("preset", &id)?;
    let preset = app
        .update(false, move |s| {
            if !s.presets.iter().any(|p| p.id == id) {
                return Err(ApiError::not_found(format!("no preset {id}")));
            }
            validate_preset(
                &input.name,
                &input.config,
                &s.presets.iter().filter(|p| p.id != id).collect::<Vec<_>>(),
            )?;
            let preset = Preset {
                id,
                name: input.name,
                description: input.description,
                config: input.config,
            };
            for p in &mut s.presets {
                if p.id == id {
                    *p = preset.clone();
                }
            }
            Ok(preset)
        })
        .await?;
    Ok(Json(preset))
}

#[utoipa::path(
    delete, path = "/api/presets/{id}", params(("id" = u32, Path, description = "preset id")),
    responses((status = 204), (status = 404, body = ErrorBody), (status = 500, body = ErrorBody))
)]
async fn delete_preset(State(app): Shared, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let id = parse_id("preset", &id)?;
    app.update(false, move |s| {
        let before = s.presets.len();
        s.presets.retain(|p| p.id != id);
        if s.presets.len() == before {
            return Err(ApiError::not_found(format!("no preset {id}")));
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- export / import -----------------------------------------------------------

#[utoipa::path(get, path = "/api/export", responses((status = 200, body = ExportDoc)))]
async fn export(State(app): Shared) -> Json<ExportDoc> {
    let s = app.stored();
    Json(ExportDoc {
        format: EXPORT_FORMAT.into(),
        version: EXPORT_VERSION,
        settings: ExportSettings {
            main_lcore: s.settings.main_lcore,
            eal_extra_args: s.settings.eal_extra_args.clone(),
            poll_interval_ms: s.settings.poll_interval_ms,
        },
        links: s
            .links
            .iter()
            .map(|l| ExportLink {
                name: l.name.clone(),
                ingress: l.ingress.clone(),
                egress: l.egress.clone(),
                config: l.config.clone(),
                worker: l.worker,
            })
            .collect(),
        presets: s
            .presets
            .iter()
            .map(|p| ExportPreset {
                name: p.name.clone(),
                description: p.description.clone(),
                config: p.config.clone(),
            })
            .collect(),
    })
}

/// Refuses a document of another version before its parts are parsed, so that an old
/// document gets a message about the version, not about an unknown link field.
fn check_version(raw: &serde_json::Value) -> Result<(), ApiError> {
    match raw.get("version").and_then(serde_json::Value::as_u64) {
        Some(1) => Err(ApiError::bad(format!(
            "version = 1: only version {EXPORT_VERSION} is supported (version 1 had links between two ports; re-create them as one link per direction, ingress to egress)"
        ))),
        Some(v) if v != u64::from(EXPORT_VERSION) => Err(ApiError::bad(format!(
            "version = {v}: only version {EXPORT_VERSION} is supported"
        ))),
        _ => Ok(()),
    }
}

/// Validates the whole document and replaces links, presets and settings of `s` by it,
/// with new ids from the counters of `s`. On an error `s` is unchanged.
fn apply_import(s: &mut StoredState, doc: ExportDoc) -> Result<(), ApiError> {
    if doc.format != EXPORT_FORMAT {
        return Err(ApiError::bad(format!(
            "format = {:?}: expected {EXPORT_FORMAT:?}",
            doc.format
        )));
    }
    let settings = Settings {
        main_lcore: doc.settings.main_lcore,
        eal_extra_args: doc.settings.eal_extra_args,
        poll_interval_ms: doc.settings.poll_interval_ms,
    };
    validate_settings(&settings).map_err(|e| e.in_document("settings"))?;
    let mut links: Vec<Link> = Vec::new();
    for (i, l) in doc.links.into_iter().enumerate() {
        let input = LinkInput {
            name: l.name,
            ingress: l.ingress,
            egress: l.egress,
            config: l.config,
            worker: l.worker,
        };
        let (ingress, egress) = validate_link(&input, &links.iter().collect::<Vec<_>>())
            .map_err(|e| e.in_document(&format!("links[{i}]")))?;
        links.push(Link {
            id: s.next_id + u32::try_from(i).unwrap_or(u32::MAX),
            name: input.name,
            ingress,
            egress,
            config: input.config,
            worker: input.worker,
        });
    }
    check_dict_limit(links.iter()).map_err(|e| e.in_document("links"))?;
    let mut presets: Vec<Preset> = Vec::new();
    for (i, p) in doc.presets.into_iter().enumerate() {
        validate_preset(&p.name, &p.config, &presets.iter().collect::<Vec<_>>())
            .map_err(|e| e.in_document(&format!("presets[{i}]")))?;
        presets.push(Preset {
            id: s.next_preset_id + u32::try_from(i).unwrap_or(u32::MAX),
            name: p.name,
            description: p.description,
            config: p.config,
        });
    }
    s.next_id += u32::try_from(links.len()).unwrap_or(u32::MAX);
    s.next_preset_id += u32::try_from(presets.len()).unwrap_or(u32::MAX);
    s.links = links;
    s.presets = presets;
    s.settings = settings;
    Ok(())
}

#[utoipa::path(
    post, path = "/api/import",
    params(("mode" = Option<String>, Query, description = "`replace` (the default and only mode): links, presets and settings are replaced")),
    request_body = ExportDoc,
    responses(
        (status = 200, body = ImportSummary), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 415, body = ErrorBody), (status = 500, body = ErrorBody)
    )
)]
async fn import(
    State(app): Shared,
    RawQuery(query): RawQuery,
    ApiJson(raw): ApiJson<serde_json::Value>,
) -> Result<Json<ImportSummary>, ApiError> {
    if let Some(mode) = query_param(query.as_deref(), "mode").filter(|m| *m != "replace") {
        return Err(ApiError::bad(format!(
            "mode = {mode:?}: only mode=replace is supported"
        )));
    }
    check_version(&raw)?;
    let doc: ExportDoc = serde_json::from_value(raw)
        .map_err(|e| ApiError::bad(format!("invalid request body: {e}")))?;
    // start and stop hold this lock too: the emulation cannot leave `stopped` during the import
    let _step = app.lifecycle.lock().await;
    let state = app.deps.supervisor.status().state;
    if state != RunState::Stopped {
        return Err(ApiError::conflict(format!(
            "cannot import while the emulation is {state:?}; stop it first"
        )));
    }
    let summary = app
        .update(true, move |s| {
            apply_import(s, doc)?;
            Ok(ImportSummary {
                links: s.links.len(),
                presets: s.presets.len(),
            })
        })
        .await?;
    Ok(Json(summary))
}

// ---- router ------------------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    info(
        title = "DPDS controller API",
        description = "Stores DPDS links, supervises dpds and serves its telemetry."
    ),
    paths(
        online,
        version,
        interfaces,
        host_hints,
        list_links,
        create_link,
        get_link,
        update_link,
        delete_link,
        get_settings,
        put_settings,
        get_emulation,
        start,
        stop,
        stats,
        histogram,
        list_presets,
        create_preset,
        get_preset,
        update_preset,
        delete_preset,
        export,
        import
    ),
    components(schemas(
        ConfigDto,
        DelayerDto,
        DistributionDto,
        HistogramDto,
        LimiterDto,
        LimiterModeDto,
        LossModelDto,
        Link,
        LinkInput,
        Settings,
        Interface,
        Hint,
        Level,
        DirectionPlan,
        EmulationView,
        OnlineView,
        VersionView,
        ErrorBody,
        StatsView,
        DirectionStats,
        Counters,
        PortStats,
        HistogramView,
        Percentiles,
        Clamped,
        ExpectedModel,
        Preset,
        PresetInput,
        ExportDoc,
        ExportSettings,
        ExportLink,
        ExportPreset,
        ImportSummary
    ))
)]
struct ApiDoc;

/// The API plus the frontend the build embedded.
pub fn router(deps: Deps) -> Result<Router, StoreError> {
    router_with(deps, Frontend::embedded())
}

/// The API, and `frontend` for every GET and HEAD outside `/api`. Fails if the stored state
/// cannot be loaded (a corrupt file is never reset silently).
pub fn router_with(deps: Deps, frontend: Frontend) -> Result<Router, StoreError> {
    let stored = store::load(&deps.state_dir)?;
    let site = frontend.clone();
    let app = Arc::new(App {
        deps,
        frontend,
        stored: Mutex::new(stored),
        edits: Mutex::new(()),
        pending_restart: AtomicBool::new(false),
        last_plan: Mutex::new(None),
        launched_configs: Mutex::new(Vec::new()),
        lifecycle: AsyncMutex::new(()),
    });
    let api = Router::new()
        .route("/api/online", get(online))
        .route("/api/version", get(version))
        .route("/api/interfaces", get(interfaces))
        .route("/api/host/hints", get(host_hints))
        .route("/api/links", get(list_links).post(create_link))
        .route(
            "/api/links/{id}",
            get(get_link).put(update_link).delete(delete_link),
        )
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/emulation", get(get_emulation))
        .route("/api/emulation/start", post(start))
        .route("/api/emulation/stop", post(stop))
        .route("/api/stats", get(stats))
        .route("/api/stats/{dir}/delay_histogram", get(histogram))
        .route("/api/presets", get(list_presets).post(create_preset))
        .route(
            "/api/presets/{id}",
            get(get_preset).put(update_preset).delete(delete_preset),
        )
        .route("/api/export", get(export))
        .route("/api/import", post(import))
        .with_state(app);
    Ok(api
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
        .fallback(move |req: Request| {
            let site = site.clone();
            async move {
                let path = req.uri().path();
                if path == "/api" || path.starts_with("/api/") {
                    ApiError::not_found("no such route").into_response()
                } else {
                    site.respond(req.method(), path)
                }
            }
        })
        .method_not_allowed_fallback(|| async {
            ApiError(
                StatusCode::METHOD_NOT_ALLOWED,
                "method not allowed on this route".into(),
            )
        })
        .layer(DefaultBodyLimit::max(BODY_LIMIT)))
}

/// DNS-rebinding guard: only requests whose `Host` (port stripped, case
/// ignored) is an IP literal, `localhost`, this machine's hostname (as returned
/// and without the domain) or one of `allowed` get through; the others get 421.
/// A request without a `Host` header is let through (HTTP/1.0, in-process tests).
pub fn with_host_guard(router: Router, allowed: &[String]) -> Router {
    let mut names: HashSet<String> = allowed
        .iter()
        .map(|h| host_without_port(h).to_ascii_lowercase())
        .collect();
    names.insert("localhost".into());
    if let Some(h) = hostname() {
        let h = h.to_ascii_lowercase();
        names.extend(h.split('.').next().map(str::to_owned));
        names.insert(h);
    }
    router.layer(middleware::from_fn_with_state(Arc::new(names), host_guard))
}

fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname NUL-terminates on success.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let end = buf.iter().position(|b| *b == 0)?;
    (rc == 0).then(|| String::from_utf8_lossy(&buf[..end]).into_owned())
}

/// The host part of a `Host` value, without the port (`[::1]:80` gives `::1`).
fn host_without_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    }
}

async fn host_guard(
    State(names): State<Arc<HashSet<String>>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(value) = req.headers().get(header::HOST) {
        let host = value
            .to_str()
            .map(|h| host_without_port(h).to_ascii_lowercase())
            .unwrap_or_default();
        if host.parse::<IpAddr>().is_err() && !names.contains(&host) {
            return ApiError(
                StatusCode::MISDIRECTED_REQUEST,
                format!(
                    "host {host:?} is not allowed; add it with --allowed-host (DPDS_ALLOWED_HOST)"
                ),
            )
            .into_response();
        }
    }
    next.run(req).await
}

/// CORS for exactly `origins` (the request's origin is echoed if listed). Off by
/// default: the frontend is served by the controller itself (same origin), so [`router`] has no CORS layer.
pub fn with_cors(router: Router, origins: Vec<HeaderValue>) -> Router {
    router.layer(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods(Any)
            .allow_headers([header::CONTENT_TYPE]),
    )
}
