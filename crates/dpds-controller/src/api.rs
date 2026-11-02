//! The REST API (B2): routes under `/api`, Swagger UI at `/api/docs`, the
//! OpenAPI document at `/api/openapi.json`. Error bodies are `{"message": ...}`.

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Path, Request, State};
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
use crate::hints::{compute_hints, Hint, Level};
use crate::host::HostRoots;
use crate::lcores::{assign_lcores, DirectionPlan};
use crate::model::{Link, Settings, StoredState};
use crate::stats::{
    compute_stats, delay_histogram, delay_stats_on, Counters, DirectionStats, HistogramView,
    PortStats, StatsView,
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
    stored: Mutex<StoredState>,
    /// Serialises edits, so the (slow) save runs without holding `stored`.
    edits: Mutex<()>,
    /// Edited since the running emulation started.
    pending_restart: AtomicBool,
    /// Directions and main lcore of the last launch.
    last_plan: Mutex<Option<(u32, Vec<DirectionPlan>)>>,
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
    /// One port (loopback) or two ports.
    ports: Vec<String>,
    /// One config per port, i.e. per direction.
    configs: Vec<ConfigDto>,
    workers: Option<Vec<u32>>,
}

// ---- helpers -----------------------------------------------------------------

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 32
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Validates a link and returns its ports, normalised (PCI addresses in lower case).
fn validate_link(input: &LinkInput, others: &[&Link]) -> Result<Vec<String>, ApiError> {
    if !valid_name(&input.name) {
        return Err(ApiError::bad(
            "link name must be 1 to 32 characters of A-Z a-z 0-9 _ . -",
        ));
    }
    let n = input.ports.len();
    if !(1..=2).contains(&n) {
        return Err(ApiError::bad("a link has one port (loopback) or two ports"));
    }
    let ports = input
        .ports
        .iter()
        .map(|p| validate::port_name(p).map_err(ApiError::bad))
        .collect::<Result<Vec<_>, _>>()?;
    if n == 2 && ports[0] == ports[1] {
        return Err(ApiError::bad("the two ports of a link must differ"));
    }
    if input.configs.len() != n {
        return Err(ApiError::bad(format!(
            "{} config(s) for {n} port(s): one config per direction is required",
            input.configs.len()
        )));
    }
    if input.workers.as_ref().is_some_and(|w| w.len() != n) {
        return Err(ApiError::bad(format!(
            "workers must list one lcore per direction ({n})"
        )));
    }
    for (i, c) in input.configs.iter().enumerate() {
        c.to_core()
            .map_err(|e| ApiError::bad(format!("configs[{i}]: {e}")))?;
    }
    for o in others {
        if o.name == input.name {
            return Err(ApiError::conflict(format!(
                "a link named {} already exists",
                input.name
            )));
        }
        if let Some(p) = ports.iter().find(|p| o.ports.contains(p)) {
            return Err(ApiError::conflict(format!(
                "port {p} is already used by link {}",
                o.name
            )));
        }
    }
    Ok(ports)
}

fn parse_id(s: &str) -> Result<u32, ApiError> {
    s.parse()
        .map_err(|_| ApiError::not_found(format!("no link {s}")))
}

impl App {
    fn stored(&self) -> MutexGuard<'_, StoredState> {
        lock(&self.stored)
    }

    /// Applies `edit` to a copy of the stored state, saves it, then swaps it in.
    /// Runs on a blocking thread (the save syncs to disk), to completion even if
    /// the request is dropped; readers of `stored` never wait for the disk.
    async fn update<T: Send + 'static>(
        self: &Arc<Self>,
        edit: impl FnOnce(&mut StoredState) -> Result<T, ApiError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let app = self.clone();
        blocking(move || app.update_blocking(edit)).await?
    }

    fn update_blocking<T>(
        &self,
        edit: impl FnOnce(&mut StoredState) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let _serial = lock(&self.edits);
        let mut next = self.stored().clone();
        let out = edit(&mut next)?;
        store::save(&self.deps.state_dir, &next)?;
        *self.stored() = next;
        if matches!(
            self.deps.supervisor.status().state,
            RunState::Starting | RunState::Running
        ) {
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

    fn launch_plan(&self) -> Result<(LaunchPlan, u32), ApiError> {
        let stored = self.stored().clone();
        if stored.links.is_empty() {
            return Err(ApiError::bad("no links are configured"));
        }
        // the state file may have been edited by hand or written by an older
        // version: what reaches dpds' command line passes the same rules as an edit
        for port in stored.links.iter().flat_map(|l| &l.ports) {
            validate::port_name(port).map_err(ApiError::bad)?;
        }
        validate::mlx5_devargs(&stored.settings.mlx5_devargs).map_err(ApiError::bad)?;
        validate::eal_extra_args(&stored.settings.eal_extra_args).map_err(ApiError::bad)?;
        let interfaces = discover(&self.deps.roots);
        for port in stored.links.iter().flat_map(|l| &l.ports) {
            if let Some(i) = find(&interfaces, port).filter(|i| i.kernel_route) {
                return Err(ApiError::conflict(format!(
                    "port {port} cannot be used: {}",
                    i.reason.as_deref().unwrap_or("it carries a kernel route")
                )));
            }
        }
        let assignment = assign_lcores(&self.deps.roots, &stored)
            .map_err(|e| ApiError::bad(format!("cannot assign lcores: {e}")))?;
        let links_json = links_file_json(&stored.links, &assignment.directions)
            .map_err(|e| ApiError::bad(e.to_string()))?;
        let configs = stored
            .links
            .iter()
            .flat_map(|l| &l.configs)
            .map(|c| c.to_core().map_err(|e| ApiError::bad(e.to_string())))
            .collect::<Result<Vec<_>, _>>()?;
        let ports: Vec<EalPort> = stored
            .links
            .iter()
            .flat_map(|l| &l.ports)
            .map(|name| EalPort {
                name: name.clone(),
                driver: find(&interfaces, name).and_then(|i| i.driver.clone()),
            })
            .collect();
        let mut workers: Vec<u32> = assignment.directions.iter().map(|d| d.lcore).collect();
        workers.sort_unstable();
        let links_file = self.deps.runtime_dir.join("links.json");
        let argv = eal_argv(&EalInput {
            main_lcore: assignment.main_lcore,
            worker_lcores: workers,
            ports,
            mlx5_devargs: stored.settings.mlx5_devargs.clone(),
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
        Ok((plan, assignment.main_lcore))
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
        .update(move |s| {
            let ports = validate_link(&input, &s.links.iter().collect::<Vec<_>>())?;
            let link = Link {
                id: s.next_id,
                name: input.name,
                ports,
                configs: input.configs,
                workers: input.workers,
            };
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
    let id = parse_id(&id)?;
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
    let id = parse_id(&id)?;
    let link = app
        .update(move |s| {
            if !s.links.iter().any(|l| l.id == id) {
                return Err(ApiError::not_found(format!("no link {id}")));
            }
            let ports = validate_link(
                &input,
                &s.links.iter().filter(|l| l.id != id).collect::<Vec<_>>(),
            )?;
            let link = Link {
                id,
                name: input.name,
                ports,
                configs: input.configs,
                workers: input.workers,
            };
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
    let id = parse_id(&id)?;
    app.update(move |s| {
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
    if settings.poll_interval_ms == 0 {
        return Err(ApiError::bad("poll_interval_ms must be at least 1"));
    }
    validate::mlx5_devargs(&settings.mlx5_devargs).map_err(ApiError::bad)?;
    validate::eal_extra_args(&settings.eal_extra_args).map_err(ApiError::bad)?;
    let stored = settings.clone();
    app.update(move |s| {
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
    let (plan, main_lcore) = blocking(move || a.launch_plan()).await??;
    *lock(&app.last_plan) = Some((main_lcore, plan.directions.clone()));
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
    get, path = "/api/stats/{dir}/delay_histogram", params(("dir" = u32, Path, description = "direction")),
    responses((status = 200, body = HistogramView), (status = 404, body = ErrorBody))
)]
async fn histogram(
    State(app): Shared,
    Path(dir): Path<String>,
) -> Result<Json<HistogramView>, ApiError> {
    let missing = || ApiError::not_found(format!("no delay histogram for direction {dir}"));
    let d: usize = dir.parse().map_err(|_| missing())?;
    let p = app.deps.telemetry.latest().ok_or_else(missing)?;
    if !delay_stats_on(&p.cur.ports, d) {
        return Err(ApiError::not_found(format!(
            "delay statistics are off for direction {d}, so it has no delay histogram"
        )));
    }
    let h = delay_histogram(&p.cur, d).ok_or_else(missing)?;
    Ok(Json(HistogramView {
        age_ms: p.age_ms,
        stale: p.stale,
        ..h
    }))
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
        LinkInput,
        Settings,
        Interface,
        Hint,
        Level,
        DirectionPlan,
        EmulationView,
        OnlineView,
        ErrorBody,
        StatsView,
        DirectionStats,
        Counters,
        PortStats,
        HistogramView
    ))
)]
struct ApiDoc;

/// Fails if the stored state cannot be loaded (a corrupt file is never reset silently).
pub fn router(deps: Deps) -> Result<Router, StoreError> {
    let stored = store::load(&deps.state_dir)?;
    let app = Arc::new(App {
        deps,
        stored: Mutex::new(stored),
        edits: Mutex::new(()),
        pending_restart: AtomicBool::new(false),
        last_plan: Mutex::new(None),
        lifecycle: AsyncMutex::new(()),
    });
    let api = Router::new()
        .route("/api/online", get(online))
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
        .with_state(app);
    Ok(api
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
        .fallback(|| async { ApiError::not_found("no such route") })
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
/// default: the frontend's nginx proxy is same-origin, so [`router`] has no CORS layer.
pub fn with_cors(router: Router, origins: Vec<HeaderValue>) -> Router {
    router.layer(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods(Any)
            .allow_headers([header::CONTENT_TYPE]),
    )
}
