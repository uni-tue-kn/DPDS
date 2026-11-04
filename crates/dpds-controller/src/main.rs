//! `dpds-controller serve` (default) and `dpds-controller check`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderValue;
use clap::{Args, Parser, Subcommand};
use dpds_controller::api::{self, Deps};
use dpds_controller::frontend::Frontend;
use dpds_controller::hints::{compute_hints, Level};
use dpds_controller::host::{HostRoots, IfAddrs, SystemExec};
use dpds_controller::model::RunConfig;
use dpds_controller::supervisor::ProcessSupervisor;
use dpds_controller::telemetry::{socket_path, Poller};

/// How long a started `dpds` may take to answer on its telemetry socket.
const START_TIMEOUT: Duration = Duration::from_secs(40);

#[derive(Parser)]
#[command(
    version,
    about = "REST controller for DPDS",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// The options of `serve`, which is the default command.
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Run the REST API and supervise `dpds` (default).
    Serve(ServeArgs),
    /// Print the host hints (changes nothing); exit 1 if any is an error.
    Check(CheckArgs),
}

#[derive(Args, Clone)]
struct HostArgs {
    #[arg(long, env = "DPDS_CONTROLLER_SYSFS_ROOT", default_value = "/sys")]
    sysfs_root: PathBuf,
    #[arg(long, env = "DPDS_CONTROLLER_PROCFS_ROOT", default_value = "/proc")]
    procfs_root: PathBuf,
    /// Test only: JSON `[{"netdev": "...", "addr": "..."}]` replacing `getifaddrs`
    /// (an empty list means no addresses).
    #[arg(long, env = "DPDS_CONTROLLER_IFADDRS_FILE")]
    ifaddrs_file: Option<PathBuf>,
}

/// One entry of `--ifaddrs-file`.
#[derive(serde::Deserialize)]
struct IfAddrEntry {
    netdev: String,
    addr: std::net::IpAddr,
}

impl HostArgs {
    fn roots(&self) -> Result<HostRoots, String> {
        let ifaddrs = match &self.ifaddrs_file {
            None => IfAddrs::System,
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("--ifaddrs-file {}: {e}", path.display()))?;
                let list: Vec<IfAddrEntry> = serde_json::from_str(&text)
                    .map_err(|e| format!("--ifaddrs-file {}: {e}", path.display()))?;
                IfAddrs::Fixed(list.into_iter().map(|e| (e.netdev, e.addr)).collect())
            }
        };
        Ok(HostRoots {
            sysfs: self.sysfs_root.clone(),
            procfs: self.procfs_root.clone(),
            ifaddrs,
            exec: Arc::new(SystemExec),
        })
    }
}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    host: HostArgs,
    #[arg(long, env = "DPDS_CONTROLLER_BIND", default_value = "0.0.0.0:8100")]
    bind: String,
    #[arg(
        long,
        env = "DPDS_CONTROLLER_DPDS_BIN",
        default_value = "/usr/local/bin/dpds"
    )]
    dpds_bin: PathBuf,
    /// Owned by the controller: holds the links file and DPDK's runtime files.
    #[arg(
        long,
        env = "DPDS_CONTROLLER_RUNTIME_DIR",
        default_value = "/run/dpds-controller"
    )]
    runtime_dir: PathBuf,
    /// Origin allowed to call the API from a browser (repeatable, or comma
    /// separated in the environment). CORS is off without it.
    #[arg(long = "cors-origin", env = "DPDS_CORS_ORIGIN", value_delimiter = ',')]
    cors_origin: Vec<String>,
    /// Extra `Host` name accepted besides IP addresses, `localhost` and this
    /// machine's hostname (repeatable, or comma separated in the environment);
    /// a request for any other host is refused (DNS-rebinding guard).
    #[arg(
        long = "allowed-host",
        env = "DPDS_ALLOWED_HOST",
        value_delimiter = ','
    )]
    allowed_host: Vec<String>,
}

#[derive(Args)]
struct CheckArgs {
    #[command(flatten)]
    host: HostArgs,
    /// Print the hints as a JSON array.
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Check(a)) => check(&a),
        Some(Command::Serve(a)) => serve(a),
        None => serve(cli.serve),
    };
    result.unwrap_or_else(|e| {
        eprintln!("dpds-controller: {e}");
        ExitCode::from(2)
    })
}

fn check(a: &CheckArgs) -> Result<ExitCode, String> {
    let hints = compute_hints(&a.host.roots()?, &RunConfig::default(), true);
    if a.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&hints).map_err(|e| e.to_string())?
        );
    } else {
        for h in &hints {
            let mut line = format!("{:<5} {}: {}", h.level.tag(), h.title, h.detail);
            if !h.suggestion.is_empty() {
                line.push_str(&format!(" -> {}", h.suggestion));
            }
            println!("{line}");
            if !h.commands.is_empty() {
                println!("  # run as root:");
                for c in &h.commands {
                    println!("  {c}");
                }
            }
        }
    }
    let failed = hints.iter().any(|h| h.level == Level::Error);
    Ok(ExitCode::from(u8::from(failed)))
}

/// Whether the binary contains the telemetry commands (`--features telemetry`).
fn has_telemetry(dpds_bin: &Path) -> bool {
    let needle = b"/dpds/stats";
    std::fs::read(dpds_bin).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle))
}

/// Longest unix socket path (`sun_path` is 108 bytes with the NUL).
const MAX_SOCKET_PATH: usize = 107;

/// Fails if DPDK's telemetry socket under `runtime_dir` could not be bound.
fn check_runtime_dir(runtime_dir: &Path) -> Result<(), String> {
    let socket = socket_path(runtime_dir);
    let len = socket.as_os_str().len();
    if len > MAX_SOCKET_PATH {
        return Err(format!(
            "--runtime-dir {} is too long: DPDK's telemetry socket {} would be {len} bytes, over the limit of {MAX_SOCKET_PATH}",
            runtime_dir.display(),
            socket.display()
        ));
    }
    Ok(())
}

/// `scheme://host[:port]`, as a browser sends it in `Origin`: no `*`, no path.
fn cors_origin(o: &str) -> Result<HeaderValue, String> {
    let bad = |why: &str| format!("invalid --cors-origin {o:?}: {why}");
    match o.split_once("://") {
        Some((scheme, host)) if !scheme.is_empty() && !host.is_empty() => {
            if host.contains(['/', '*']) {
                return Err(bad(
                    "an origin is scheme://host[:port] without a path, trailing slash or wildcard",
                ));
            }
        }
        _ => return Err(bad("an origin is scheme://host[:port] (no wildcard)")),
    }
    HeaderValue::from_str(o).map_err(|e| bad(&e.to_string()))
}

fn serve(a: ServeArgs) -> Result<ExitCode, String> {
    let roots = a.host.roots()?;
    check_runtime_dir(&a.runtime_dir)?;
    let origins = a
        .cors_origin
        .iter()
        .map(|o| cors_origin(o))
        .collect::<Result<Vec<_>, _>>()?;
    let supervisor = Arc::new(ProcessSupervisor::new(START_TIMEOUT));
    let telemetry = Poller::spawn(a.runtime_dir.clone(), supervisor.clone());
    let dpds_has_telemetry = has_telemetry(&a.dpds_bin);
    if !dpds_has_telemetry {
        eprintln!(
            "dpds-controller: warning: {} is missing or was built without the telemetry feature",
            a.dpds_bin.display()
        );
    }
    let frontend = Frontend::embedded();
    let router = api::router_with(
        Deps {
            roots,
            supervisor: supervisor.clone(),
            telemetry,
            dpds_bin: a.dpds_bin,
            runtime_dir: a.runtime_dir,
            dpds_has_telemetry,
            // SAFETY: geteuid has no preconditions.
            is_root: unsafe { libc::geteuid() } == 0,
        },
        frontend.clone(),
    );
    let router = api::with_host_guard(router, &a.allowed_host);
    let router = if origins.is_empty() {
        router
    } else {
        api::with_cors(router, origins)
    };

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(async {
            let listener = tokio::net::TcpListener::bind(&a.bind)
                .await
                .map_err(|e| format!("cannot listen on {}: {e}", a.bind))?;
            eprintln!("dpds-controller: listening on {}", a.bind);
            if frontend.kind() == "bundled" {
                eprintln!("dpds-controller: serving the bundled frontend at /");
            } else {
                eprintln!(
                    "dpds-controller: serving the placeholder page at /, the frontend is not \
                     bundled (install it with dpds.sh install, or run npm run build in frontend/)"
                );
            }
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown_signal())
                .await
                .map_err(|e| e.to_string())
        })?;

    // stop dpds the same way as POST /api/emulation/stop before exiting
    eprintln!("dpds-controller: shutting down, stopping dpds");
    supervisor.shutdown();
    Ok(ExitCode::SUCCESS)
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}
