#!/usr/bin/env bash
# dpds.sh - build, install and run the DPDS controller (systemd service) and its web frontend (Docker).
#
# Usage: ./dpds.sh [--nightly|--stable] [--yes] {check|install|update|start|stop|restart|status|uninstall}
#
# The controller (dpds-controller) runs natively on the host as root and starts dpds as its child
# process. Only the web frontend runs in a container. This script never tunes the host: it does not
# touch kernel parameters, hugepages, NIC settings or CPU governors. The controller only reports
# hints (`dpds.sh check`).
#
# Environment overrides:
#   DPDS_PREFIX       install prefix, binaries go to $DPDS_PREFIX/bin   (default /usr/local)
#   DPDS_ETC_DIR      config dir, holds controller.env                   (default /etc/dpds)
#   DPDS_STATE_DIR    controller state dir                               (default /var/lib/dpds-controller)
#   DPDS_SYSTEMD_DIR  where the unit file is installed                   (default /etc/systemd/system)
#   DPDS_BIND         bind address written into a NEW controller.env     (default 0.0.0.0:8100)
#   CARGO_TARGET_DIR  cargo target dir the build output is read from     (default <repo>/target)
#   DPDS_CONTROLLER   where the frontend container proxies /api to; scheme://host:port, no path.
#                     Default: http://<bind host>:<bind port> from controller.env (127.0.0.1 for a
#                     wildcard bind, port 8100 without controller.env)
#   DPDS_FRONTEND_BIND, DPDS_FRONTEND_PORT
#                     frontend listen address and port; passed through to docker compose
#                     (defaults 0.0.0.0 and 8080, see docker-compose.yml)
#
# Exit codes: 0 success, 1 usage error / prerequisite missing / aborted / a step failed.

set -Eeuo pipefail

########################################
# Paths and settings                   #
########################################
SCRIPT_SOURCE="${BASH_SOURCE[0]}"
if command -v readlink >/dev/null 2>&1; then
  SCRIPT_SOURCE="$(readlink -f -- "$SCRIPT_SOURCE" 2>/dev/null || echo "$SCRIPT_SOURCE")"
fi
REPO_ROOT="$(cd -P -- "$(dirname -- "$SCRIPT_SOURCE")" && pwd)"

PREFIX="${DPDS_PREFIX:-/usr/local}"
ETC_DIR="${DPDS_ETC_DIR:-/etc/dpds}"
STATE_DIR="${DPDS_STATE_DIR:-/var/lib/dpds-controller}"
SYSTEMD_DIR="${DPDS_SYSTEMD_DIR:-/etc/systemd/system}"
BIND="${DPDS_BIND:-0.0.0.0:8100}"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

SERVICE="dpds-controller"
UNIT_SRC="$REPO_ROOT/packaging/dpds-controller.service"
UNIT_DEST="$SYSTEMD_DIR/$SERVICE.service"
ENV_FILE="$ETC_DIR/controller.env"
BIN_DIR="$PREFIX/bin"
DEFAULT_PORT=8100

CHANNEL_NIGHTLY=0
CHANNEL_STABLE=0
ASSUME_YES=0

#################
# Pretty output #
#################
ts() { date "+%Y-%m-%d %H:%M:%S%z"; }
info()  { echo "[INFO ] $(ts) $*"; }
warn()  { echo "[WARN ] $(ts) $*" >&2; }
error() { echo "[ERROR] $(ts) $*" >&2; }

########################################
# Helpers                              #
########################################
require_cmd() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    error "Required command not found in PATH: $cmd"
    exit 1
  fi
}

run_privileged() {
  if (( EUID == 0 )); then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    error "Root privileges are required to run: $*"
    return 1
  fi
}

# Prints the major version of the first "N.N[.N]" found on stdin (empty if none).
version_major() {
  local v
  v="$(grep -Eo '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -n 1)" || true
  echo "${v%%.*}"
}

# Escapes a string for the replacement part of a sed s||| command.
sed_escape() {
  printf '%s' "$1" | sed -e 's/[\\|&]/\\&/g'
}

validate_settings() {
  local name value
  for name in PREFIX ETC_DIR STATE_DIR SYSTEMD_DIR; do
    value="${!name}"
    if [[ "$value" != /* || "$value" =~ [[:space:]] ]]; then
      error "Path must be absolute and contain no whitespace: $name=$value"
      exit 1
    fi
  done
  if [[ ! "$BIND" =~ ^[^[:space:]\"\']+:[0-9]+$ ]]; then
    error "DPDS_BIND must look like HOST:PORT, got: $BIND"
    exit 1
  fi
}

confirm() {
  local answer=""
  printf '%s [y/N] ' "$1"
  read -r answer || answer=""
  echo
  case "${answer,,}" in
    y|yes) ;;
    *) error "Aborted by user."; exit 1 ;;
  esac
}

########################################
# Prerequisites (never installs anything)
########################################
require_prerequisites() {
  require_cmd cargo
  require_cmd rustc
  require_cmd clang
  require_cmd ld.lld
  require_cmd pkg-config

  # Capture each tool's output first: piping it into `head` can SIGPIPE the tool, and pipefail
  # then fails the whole assignment at random.
  local rust_llvm clang_llvm lld_llvm out
  out="$(rustc -vV)"
  rust_llvm="$(sed -n 's/^LLVM version: *//p' <<<"$out" | version_major)"
  if [[ -z "$rust_llvm" ]]; then
    error "Cannot determine the LLVM version of rustc (rustc -vV has no 'LLVM version' line)."
    exit 1
  fi
  out="$(clang --version)"
  clang_llvm="$(head -n 1 <<<"$out" | version_major)"
  if [[ "$clang_llvm" != "$rust_llvm" ]]; then
    error "clang is LLVM ${clang_llvm:-unknown} but rustc uses LLVM $rust_llvm. Put a matching clang first in PATH."
    exit 1
  fi
  out="$(ld.lld --version)"
  lld_llvm="$(head -n 1 <<<"$out" | version_major)"
  if [[ "$lld_llvm" != "$rust_llvm" ]]; then
    error "ld.lld is LLVM ${lld_llvm:-unknown} but rustc uses LLVM $rust_llvm. Put a matching ld.lld first in PATH."
    exit 1
  fi

  if ! pkg-config --exists libdpdk >/dev/null 2>&1; then
    error "DPDK development files not found: 'pkg-config --exists libdpdk' failed. Install DPDK and set PKG_CONFIG_PATH."
    exit 1
  fi
}

# Docker is only needed to build and run the frontend image (install, update), not by `check`.
require_docker() {
  require_cmd docker
  if ! docker compose version >/dev/null 2>&1; then
    error "The docker compose plugin is not available ('docker compose version' failed)."
    exit 1
  fi
  if ! docker info >/dev/null 2>&1; then
    error "Cannot reach the docker daemon ('docker info' failed). Is it running, and may this user use it?"
    exit 1
  fi
}

########################################
# Build, check, install                #
########################################
build_dpds() {
  info "Building dpds (with telemetry)"
  cargo build --release -p dpds --features telemetry
}

build_controller() {
  info "Building dpds-controller"
  cargo build --release -p dpds-controller
}

# Runs the controller's host check as root (some hints are only readable by root); its exit code
# is returned to the caller.
run_host_check() {
  local ctl="$1" rc=0
  info "Host check ($ctl check)"
  run_privileged "$ctl" check || rc=$?
  return "$rc"
}

install_unit() {
  local exec_line env_line
  exec_line="$(sed_escape "$BIN_DIR/dpds-controller serve")"
  env_line="$(sed_escape "-$ENV_FILE")"
  run_privileged install -d -m 0755 "$SYSTEMD_DIR"
  sed -e "s|^ExecStart=.*|ExecStart=$exec_line|" \
      -e "s|^EnvironmentFile=.*|EnvironmentFile=$env_line|" \
      "$UNIT_SRC" | run_privileged tee "$UNIT_DEST" >/dev/null
}

install_env_file() {
  run_privileged install -d -m 0755 "$ETC_DIR"
  if [[ -e "$ENV_FILE" ]]; then
    info "Keeping existing $ENV_FILE"
    return 0
  fi
  info "Writing $ENV_FILE"
  {
    echo "# Written by dpds.sh install. Never overwritten; edit freely, then restart the service."
    echo "DPDS_CONTROLLER_BIND=$BIND"
    echo "DPDS_CONTROLLER_DPDS_BIN=$BIN_DIR/dpds"
    echo "DPDS_CONTROLLER_STATE_DIR=$STATE_DIR"
  } | run_privileged tee "$ENV_FILE" >/dev/null
}

# Build, host check, confirmation, installation. Prerequisites must have been checked.
install_flow() {
  validate_settings
  build_dpds
  build_controller

  local release_dir="$TARGET_DIR/release" rc=0
  run_host_check "$release_dir/dpds-controller" || rc=$?
  if (( rc != 0 )); then
    warn "Host check reported problems (exit code $rc). They are hints about performance, not blockers."
  fi
  if (( ! ASSUME_YES )); then
    confirm "Continue with the installation?"
  fi

  info "Installing binaries to $BIN_DIR"
  run_privileged install -d -m 0755 "$BIN_DIR"
  run_privileged install -m 0755 "$release_dir/dpds" "$BIN_DIR/dpds"
  run_privileged install -m 0755 "$release_dir/dpds-controller" "$BIN_DIR/dpds-controller"
  info "Installing $UNIT_DEST"
  install_unit
  install_env_file
  run_privileged install -d -m 0755 "$STATE_DIR"
  run_privileged systemctl daemon-reload

  info "Building the frontend image"
  export_frontend_env
  docker compose build
  info "Installed. Start it with: $0 start"
}

########################################
# Commands                             #
########################################
cmd_check() {
  validate_settings
  require_prerequisites
  local ctl="$BIN_DIR/dpds-controller"
  if [[ ! -x "$ctl" ]]; then
    ctl="$TARGET_DIR/release/dpds-controller"
    if [[ ! -x "$ctl" ]]; then
      build_controller
    fi
  fi
  "$ctl" check
}

cmd_install() {
  require_prerequisites
  require_docker
  install_flow
}

cmd_start() {
  info "Enabling and starting $SERVICE"
  run_privileged systemctl enable --now "$SERVICE" || return 1
  info "Starting the frontend container"
  export_frontend_env
  docker compose up -d
}

cmd_stop() {
  local rc=0
  info "Disabling and stopping $SERVICE (this drains the delay buffers and can take a while)"
  run_privileged systemctl disable --now "$SERVICE" || rc=1
  info "Stopping the frontend container"
  docker compose down || rc=1
  return "$rc"
}

# A failed stop must not leave a running service stopped: start is attempted anyway and the
# failure is reported through the exit code.
cmd_restart() {
  local rc=0
  cmd_stop || rc=1
  if (( rc )); then
    error "Stopping failed; trying to start anyway."
  fi
  cmd_start || rc=1
  return "$rc"
}

# Prints the controller's bind address (HOST:PORT) from controller.env, or the default.
controller_bind() {
  local bind=""
  if [[ -r "$ENV_FILE" ]]; then
    bind="$(sed -n 's/^DPDS_CONTROLLER_BIND=//p' "$ENV_FILE" | tail -n 1 | tr -d '"')" || true
  fi
  if [[ ! "$bind" =~ ^.+:[0-9]+$ ]]; then
    bind="127.0.0.1:$DEFAULT_PORT"
  fi
  echo "$bind"
}

# Prints the address clients on this host use to reach the controller: the bind address, with
# a wildcard host replaced by 127.0.0.1.
controller_address() {
  local bind host port
  bind="$(controller_bind)"
  host="${bind%:*}"
  port="${bind##*:}"
  case "$host" in
    0.0.0.0|::|"[::]") host="127.0.0.1" ;;
  esac
  echo "$host:$port"
}

# The frontend container proxies /api to DPDS_CONTROLLER; docker compose reads it (and
# DPDS_FRONTEND_BIND / DPDS_FRONTEND_PORT, which the caller may set) from the environment.
export_frontend_env() {
  DPDS_CONTROLLER="${DPDS_CONTROLLER:-http://$(controller_address)}"
  export DPDS_CONTROLLER
}

cmd_status() {
  local addr
  addr="$(controller_address)"
  info "Service"
  systemctl status --no-pager "$SERVICE" || true
  info "frontend container"
  docker compose ps || true
  info "Controller API (http://$addr/api/online)"
  if command -v curl >/dev/null 2>&1; then
    curl -fsS --max-time 5 "http://$addr/api/online" || warn "Controller API is not reachable."
    echo
  else
    warn "curl not found, skipping the API probe."
  fi
  return 0
}

cmd_update() {
  require_prerequisites
  require_docker

  # Refuse before touching anything: a checkout or pull with local changes can fail halfway or
  # silently build something other than the branch.
  local dirty branch was_active=0
  dirty="$(git status --porcelain --untracked-files=no)"
  if [[ -n "$dirty" ]]; then
    error "The working tree has uncommitted changes; commit or stash them first:"
    echo "$dirty" >&2
    exit 1
  fi
  branch="$(git symbolic-ref --short -q HEAD 2>/dev/null)" || branch=""
  if [[ -z "$branch" ]]; then
    warn "HEAD is detached; switching to nightly."
  elif [[ "$branch" != nightly ]]; then
    warn "Currently on branch $branch; switching to nightly."
  fi

  if systemctl is-active --quiet "$SERVICE"; then
    was_active=1
  fi
  info "Updating the repository to the nightly branch"
  git fetch
  git checkout nightly
  git pull --ff-only
  install_flow
  if (( was_active )); then
    info "$SERVICE was running, restarting it"
    cmd_restart
  else
    info "$SERVICE was not running, leaving it stopped"
  fi
}

cmd_uninstall() {
  info "Stopping $SERVICE and the frontend container"
  run_privileged systemctl disable --now "$SERVICE" || warn "Could not disable $SERVICE (not installed?)."
  docker compose down || warn "Could not stop the frontend container."
  info "Removing binaries and unit"
  run_privileged rm -f "$BIN_DIR/dpds" "$BIN_DIR/dpds-controller" "$UNIT_DEST"
  run_privileged systemctl daemon-reload || warn "systemctl daemon-reload failed."
  info "Kept $STATE_DIR and $ENV_FILE; remove them by hand if you do not need them."
}

usage() {
  echo "Usage: $0 [--nightly|--stable] [--yes] {check|install|update|start|stop|restart|status|uninstall}"
  echo ""
  echo "Options:"
  echo "  --nightly    Use the nightly branch (default)"
  echo "  --stable     There is no Rust stable release yet; this option only prints that"
  echo "  --yes        Do not ask for confirmation (install, update)"
  echo ""
  echo "Commands:"
  echo "  check        Check prerequisites, then print the host hints (dpds-controller check)"
  echo "  install      Build, run the host check, ask to continue, install binaries, unit and frontend image"
  echo "  update       git pull nightly, rebuild, reinstall, restart if it was running"
  echo "  start        Enable and start the controller service and the frontend container"
  echo "  stop         Disable and stop the controller service and the frontend container"
  echo "  restart      Stop, then start"
  echo "  status       Show service, container and API status"
  echo "  uninstall    Stop and remove binaries and unit (state dir and controller.env are kept)"
}

main() {
  local cmd="" arg
  for arg in "$@"; do
    case "$arg" in
      --nightly) CHANNEL_NIGHTLY=1 ;;
      --stable) CHANNEL_STABLE=1 ;;
      --yes|-y) ASSUME_YES=1 ;;
      -h|--help) usage; exit 0 ;;
      -*) error "Unknown option: $arg"; usage >&2; exit 1 ;;
      *)
        if [[ -n "$cmd" ]]; then
          error "Unexpected argument: $arg"
          usage >&2
          exit 1
        fi
        cmd="$arg"
        ;;
    esac
  done

  if (( CHANNEL_NIGHTLY && CHANNEL_STABLE )); then
    error "--nightly and --stable cannot be combined."
    exit 1
  fi
  if (( CHANNEL_STABLE )); then
    error "No Rust stable release exists yet. Use --nightly (the default)."
    exit 1
  fi

  case "$cmd" in
    check|install|update|start|stop|restart|status|uninstall) ;;
    "") usage >&2; exit 1 ;;
    *) error "Unknown command: $cmd"; usage >&2; exit 1 ;;
  esac

  # cargo, git and docker compose all work on the repository this script lives in.
  cd "$REPO_ROOT"

  "cmd_$cmd"
}

main "$@"
