//! The REST API (B2): routes under `/api`, Swagger UI at `/api/docs`, the
//! OpenAPI document at `/api/openapi.json`. Error bodies are `{"message": ...}`.

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::PathBuf;
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
use serde::Serialize;
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
use crate::hints::{compute_hints_cached, Hint, Level};
use crate::host::HostRoots;
use crate::lcores::{assign_lcores, DirectionPlan};
use crate::model::{Link, RunConfig, StartLink, StartRequest, MAX_NAME_CHARS};
use crate::nic_hints::FirmwareCache;
use crate::stats::{
    compute_stats, delay_histogram, delay_stats_on, uptime_ms, Clamped, Counters, DirectionStats,
    ExpectedModel, HistogramView, HistoryLink, HistorySample, HistoryView, Percentiles, PortStats,
    StatsView,
};
use crate::supervisor::{LaunchPlan, State as RunState, Supervisor};
use crate::telemetry::TelemetrySource;
use crate::validate;

/// Largest accepted request body (links with their configs are a few KB).
const BODY_LIMIT: usize = 1024 * 1024;

pub struct Deps {
    pub roots: HostRoots,
    pub supervisor: Arc<dyn Supervisor>,
    pub telemetry: Arc<dyn TelemetrySource>,
    pub dpds_bin: PathBuf,
    pub runtime_dir: PathBuf,
    /// The `dpds` binary was built with the `telemetry` feature.
    pub dpds_has_telemetry: bool,
    pub is_root: bool,
}

/// What the emulation was started with and how it was planned.
#[derive(Clone)]
struct Run {
    config: RunConfig,
    main_lcore: u32,
    directions: Vec<DirectionPlan>,
}

struct App {
    deps: Deps,
    frontend: Frontend,
    /// The last start (reported and used only while the emulation is not stopped).
    run: Mutex<Option<Run>>,
    /// Serialises start and stop: the state check and the launch are one step.
    lifecycle: AsyncMutex<()>,
    /// The firmware query results of the hints, until the controller restarts or a recheck.
    firmware: FirmwareCache,
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
    /// The planned lcores of the run; empty while stopped.
    directions: Vec<DirectionPlan>,
    main_lcore: Option<u32>,
    /// The configuration the emulation was started with; null while stopped.
    config: Option<RunConfig>,
    error: Option<String>,
    log_tail: Vec<String>,
}

/// Answer of `POST /api/emulation/validate`: the plan a start would use.
#[derive(Serialize, ToSchema)]
pub struct PlanView {
    main_lcore: u32,
    directions: Vec<DirectionPlan>,
}

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
/// refuses to start above 256: refuse such a setup here, before it is launched.
fn check_dict_limit(links: &[Link]) -> Result<(), ApiError> {
    let mut ports: Vec<&str> = Vec::new();
    let count = links.len();
    for l in links {
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

/// Validates a link of the configuration against the links before it.
fn validate_link(l: &Link, before: &[Link]) -> Result<(), ApiError> {
    check_link_name(&l.name)?;
    validate::port_name(&l.ingress).map_err(ApiError::bad)?;
    validate::port_name(&l.egress).map_err(ApiError::bad)?;
    l.config
        .to_core()
        .map_err(|e| ApiError::bad(format!("config: {e}")))?;
    if let Some(o) = before.iter().find(|o| o.name == l.name) {
        return Err(ApiError::conflict(format!(
            "a link named {} already exists",
            o.name
        )));
    }
    check_ports_free(l, before)
}

/// A port is the ingress of at most one link and the egress of at most one link.
fn check_ports_free(l: &Link, others: &[Link]) -> Result<(), ApiError> {
    for o in others {
        if o.ingress == l.ingress {
            return Err(ApiError::conflict(format!(
                "port {} is already the ingress of link {} (link {})",
                l.ingress, o.name, l.name
            )));
        }
        if o.egress == l.egress {
            return Err(ApiError::conflict(format!(
                "port {} is already the egress of link {} (link {})",
                l.egress, o.name, l.name
            )));
        }
    }
    Ok(())
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

/// The validated configuration of a start body.
fn validate_config(config: &RunConfig) -> Result<(), ApiError> {
    if config.links.is_empty() {
        return Err(ApiError::bad("nothing to start: the body has no links"));
    }
    validate_body(config)
}

/// The checks of a start body that do not need a link: every link and the telemetry limit.
fn validate_body(config: &RunConfig) -> Result<(), ApiError> {
    for (i, l) in config.links.iter().enumerate() {
        validate_link(l, &config.links[..i])?;
    }
    check_dict_limit(&config.links)
}

impl App {
    fn emulation(&self) -> EmulationView {
        let st = self.deps.supervisor.status();
        let run = if st.state == RunState::Stopped {
            None
        } else {
            lock(&self.run).clone()
        };
        EmulationView {
            state: st.state,
            pid: st.pid,
            started_at: st.started_at,
            main_lcore: run.as_ref().map(|r| r.main_lcore),
            directions: run.as_ref().map_or_else(Vec::new, |r| r.directions.clone()),
            config: run.map(|r| r.config),
            error: st.error,
            log_tail: st.log_tail,
        }
    }

    /// The config of direction `d` of the running emulation.
    fn direction_config(&self, d: usize) -> Option<ConfigDto> {
        if self.deps.supervisor.status().state == RunState::Stopped {
            return None;
        }
        let run = lock(&self.run);
        Some(run.as_ref()?.config.links.get(d)?.config.clone())
    }

    /// Validates the body and plans the launch (what start and validate share).
    fn launch_plan(&self, req: StartRequest) -> Result<(LaunchPlan, RunConfig), ApiError> {
        let config = req.into_run_config();
        validate_config(&config)?;
        let interfaces = discover(&self.deps.roots);
        let mut ports: Vec<EalPort> = Vec::new();
        for (link, port) in config
            .links
            .iter()
            .flat_map(|l| [(l, &l.ingress), (l, &l.egress)])
        {
            let found = find(&interfaces, port);
            if let Some(i) = found.filter(|i| i.uplink) {
                return Err(ApiError::conflict(format!(
                    "link {}: port {port} cannot be used: {}",
                    link.name,
                    i.reason.as_deref().unwrap_or("it is a host uplink")
                )));
            }
            if !ports.iter().any(|p| p.name == *port) {
                ports.push(EalPort {
                    name: port.clone(),
                    driver: found.and_then(|i| i.driver.clone()),
                });
            }
        }
        let assignment = assign_lcores(&self.deps.roots, &config)
            .map_err(|e| ApiError::bad(format!("cannot assign lcores: {e}")))?;
        let links_json = links_file_json(&config.links, &assignment.directions)
            .map_err(|e| ApiError::bad(e.to_string()))?;
        let configs = config
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
        Ok((plan, config))
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

/// The hints that depend on the host only.
#[utoipa::path(
    get, path = "/api/host/hints",
    responses((status = 200, body = Vec<Hint>), (status = 500, body = ErrorBody))
)]
async fn host_hints(State(app): Shared) -> Result<Json<Vec<Hint>>, ApiError> {
    hints_of(app, RunConfig::default(), false).await
}

/// The host hints plus the ones that depend on the links of a configuration. The body is
/// validated like the start body (400 naming the field, 409 for a duplicate link name or a reused
/// port; links may be empty, which gives the host hints), but nothing is planned, started or
/// kept, so a port the host uses is a hint, not a 409. Hints suggest root commands for an
/// administrator; DPDS runs none of them. The firmware query of a NIC is cached until the controller
/// restarts; `recheck=true` drops the cached results of the posted links' NICs first.
#[utoipa::path(
    post, path = "/api/host/hints", request_body = StartRequest,
    params(("recheck" = Option<bool>, Query, description = "true: read the firmware settings of the posted links' NICs again instead of using the cached result (default false); any other value is a 400")),
    responses(
        (status = 200, body = Vec<Hint>), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 415, body = ErrorBody), (status = 500, body = ErrorBody)
    )
)]
async fn host_hints_for(
    State(app): Shared,
    RawQuery(query): RawQuery,
    ApiJson(req): ApiJson<StartRequest>,
) -> Result<Json<Vec<Hint>>, ApiError> {
    let recheck = parse_recheck(query.as_deref())?;
    let mut config = req.into_run_config();
    validate_body(&config)?;
    // validation accepts upper-case PCI addresses; sysfs has them in lower case
    for l in &mut config.links {
        l.ingress.make_ascii_lowercase();
        l.egress.make_ascii_lowercase();
    }
    hints_of(app, config, recheck).await
}

/// `recheck` of the hints query: absent, `recheck=true` or `recheck=false`, each at most once.
fn parse_recheck(query: Option<&str>) -> Result<bool, ApiError> {
    let mut recheck = None;
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let value = match pair.split_once('=') {
            Some(("recheck", "true")) if recheck.is_none() => true,
            Some(("recheck", "false")) if recheck.is_none() => false,
            _ => {
                return Err(ApiError::bad(format!(
                    "query {pair:?}: only recheck=true or recheck=false, given once, is accepted"
                )))
            }
        };
        recheck = Some(value);
    }
    Ok(recheck.unwrap_or(false))
}

/// Computes the hints. The emulation state is read once: tools run only while no emulation is
/// starting, running or stopping. No lock is taken, so a start or stop is never delayed by a
/// running query and queries do not queue behind each other.
async fn hints_of(
    app: Arc<App>,
    config: RunConfig,
    recheck: bool,
) -> Result<Json<Vec<Hint>>, ApiError> {
    Ok(Json(
        blocking(move || {
            let tools = tools_may_run(&app);
            compute_hints_cached(&app.deps.roots, &config, tools, &app.firmware, recheck)
        })
        .await?,
    ))
}

/// Host tools only run while no emulation is starting, running or stopping.
fn tools_may_run(app: &App) -> bool {
    matches!(
        app.deps.supervisor.status().state,
        RunState::Stopped | RunState::Failed
    )
}

#[utoipa::path(
    get, path = "/api/emulation",
    responses((status = 200, body = EmulationView), (status = 500, body = ErrorBody))
)]
async fn get_emulation(State(app): Shared) -> Result<Json<EmulationView>, ApiError> {
    Ok(Json(blocking(move || app.emulation()).await?))
}

/// Starts the emulation with the configuration in the body.
#[utoipa::path(
    post, path = "/api/emulation/start", request_body = StartRequest,
    responses(
        (status = 202, body = EmulationView), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 415, body = ErrorBody), (status = 500, body = ErrorBody)
    )
)]
async fn start(
    State(app): Shared,
    ApiJson(req): ApiJson<StartRequest>,
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
    let (plan, config) = blocking(move || a.launch_plan(req)).await??;
    app.deps.telemetry.set_run_links(
        config
            .links
            .iter()
            .map(|l| HistoryLink {
                name: l.name.clone(),
                ingress: l.ingress.clone(),
                egress: l.egress.clone(),
            })
            .collect(),
    );
    *lock(&app.run) = Some(Run {
        config,
        main_lcore: plan.main_lcore,
        directions: plan.directions.clone(),
    });
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

/// Checks a configuration like start does and answers the lcores it would run on. It
/// ignores the state of the emulation and starts nothing.
#[utoipa::path(
    post, path = "/api/emulation/validate", request_body = StartRequest,
    responses(
        (status = 200, body = PlanView), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody),
        (status = 413, body = ErrorBody), (status = 415, body = ErrorBody), (status = 500, body = ErrorBody)
    )
)]
async fn validate_start(
    State(app): Shared,
    ApiJson(req): ApiJson<StartRequest>,
) -> Result<Json<PlanView>, ApiError> {
    let (plan, _) = blocking(move || app.launch_plan(req)).await??;
    Ok(Json(PlanView {
        main_lcore: plan.main_lcore,
        directions: plan.directions,
    }))
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
    let elapsed_ms = app.deps.telemetry.elapsed_ms();
    Json(match app.deps.telemetry.latest() {
        Some(p) => StatsView {
            elapsed_ms,
            age_ms: Some(p.age_ms),
            stale: p.stale,
            ..compute_stats(p.prev.as_ref(), &p.cur)
        },
        // no data (never polled, not running, or cleared after failed polls)
        None => StatsView {
            stale: true,
            elapsed_ms,
            ..StatsView::default()
        },
    })
}

/// `since_ms` of the history query: absent, or digits only (a u64). Any other parameter is an error.
fn parse_since_ms(query: Option<&str>) -> Result<Option<u64>, ApiError> {
    let mut since = None;
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key != "since_ms" {
            return Err(ApiError::bad(format!(
                "unknown query parameter {key:?}: only since_ms is accepted"
            )));
        }
        let ms = (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            .then(|| value.parse::<u64>().ok())
            .flatten();
        let Some(ms) = ms.filter(|_| since.is_none()) else {
            return Err(ApiError::bad(format!(
                "since_ms = {value:?}: must be given once, as a non-negative integer number of milliseconds"
            )));
        };
        since = Some(ms);
    }
    Ok(since)
}

/// The chart history of the current or last run, per link, one sample per second.
#[utoipa::path(
    get, path = "/api/stats/history",
    params(("since_ms" = Option<u64>, Query, description = "only the samples with t_ms above this, in milliseconds since the run started (default: all)")),
    responses((status = 200, body = HistoryView), (status = 400, body = ErrorBody))
)]
async fn stats_history(
    State(app): Shared,
    RawQuery(query): RawQuery,
) -> Result<Json<HistoryView>, ApiError> {
    let since_ms = parse_since_ms(query.as_deref())?;
    Ok(Json(app.deps.telemetry.run_history(since_ms)))
}

#[utoipa::path(
    get, path = "/api/stats/{dir}/delay_histogram",
    params(
        ("dir" = u32, Path, description = "link index, in the order of the start body"),
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

// ---- router ------------------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    info(
        title = "DPDS controller API",
        description = "Starts and stops dpds with the configuration of the request and serves its telemetry. The controller stores no configuration."
    ),
    paths(
        online,
        version,
        interfaces,
        host_hints,
        host_hints_for,
        get_emulation,
        start,
        validate_start,
        stop,
        stats,
        stats_history,
        histogram
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
        StartLink,
        StartRequest,
        RunConfig,
        PlanView,
        Interface,
        Hint,
        Level,
        DirectionPlan,
        EmulationView,
        OnlineView,
        VersionView,
        ErrorBody,
        StatsView,
        HistoryView,
        HistorySample,
        HistoryLink,
        DirectionStats,
        Counters,
        PortStats,
        HistogramView,
        Percentiles,
        Clamped,
        ExpectedModel
    ))
)]
struct ApiDoc;

/// The API plus the frontend the build embedded.
pub fn router(deps: Deps) -> Router {
    router_with(deps, Frontend::embedded())
}

/// The API, and `frontend` for every GET and HEAD outside `/api`.
pub fn router_with(deps: Deps, frontend: Frontend) -> Router {
    let site = frontend.clone();
    let app = Arc::new(App {
        deps,
        frontend,
        run: Mutex::new(None),
        lifecycle: AsyncMutex::new(()),
        firmware: FirmwareCache::default(),
    });
    let api = Router::new()
        .route("/api/online", get(online))
        .route("/api/version", get(version))
        .route("/api/interfaces", get(interfaces))
        .route("/api/host/hints", get(host_hints).post(host_hints_for))
        .route("/api/emulation", get(get_emulation))
        .route("/api/emulation/start", post(start))
        .route("/api/emulation/validate", post(validate_start))
        .route("/api/emulation/stop", post(stop))
        .route("/api/stats", get(stats))
        .route("/api/stats/history", get(stats_history))
        .route("/api/stats/{dir}/delay_histogram", get(histogram))
        .with_state(app);
    api.merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
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
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
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
