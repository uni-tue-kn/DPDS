#!/usr/bin/env bash
# dpds.sh - build, install and run the DPDS controller (a systemd service that also serves the web frontend).
#
# Usage: ./dpds.sh [--nightly|--stable] [--yes] {check|install|update|start|stop|restart|status|uninstall}
#
# The controller (dpds-controller) runs natively on the host as root and starts dpds as its child
# process. It embeds the web frontend at build time and serves it on its own port. The frontend is
# built by CI: install and update download the dist matching the checked-out frontend/ tree from
# the GitHub release `frontend-builds` (dpds-frontend-<tree>.tar.gz plus a .sha256 file), verify
# it and build the controller with it. The host needs neither Node nor a container runtime. This
# script never tunes the host: it does not touch kernel parameters, hugepages, NIC settings or CPU
# governors. The controller only reports hints (`dpds.sh check`).
#
# Environment overrides:
#   DPDS_PREFIX       install prefix, binaries go to $DPDS_PREFIX/bin   (default /usr/local)
#   DPDS_ETC_DIR      config dir, holds controller.env                   (default /etc/dpds)
#   DPDS_STATE_DIR    controller state dir                               (default /var/lib/dpds-controller)
#   DPDS_SYSTEMD_DIR  where the unit file is installed                   (default /etc/systemd/system)
#   DPDS_BIND         bind address written into a NEW controller.env     (default 0.0.0.0:8100)
#   CARGO_TARGET_DIR  cargo target dir the build output is read from     (default <repo>/target)
#   DPDS_FRONTEND_DIST       a frontend dist directory (with index.html) that you built yourself
#                     (`npm run build` in frontend/); install and update then download nothing and
#                     build the controller with it. The directory is never modified or removed.
#   DPDS_FRONTEND_ASSET_URL  base URL of the frontend release assets
#                     (default https://github.com/uni-tue-kn/DPDS/releases/download/frontend-builds)
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
FRONTEND_ASSET_URL="${DPDS_FRONTEND_ASSET_URL:-https://github.com/uni-tue-kn/DPDS/releases/download/frontend-builds}"
# The build script of the controller runs in the crate directory, so a relative path of the caller
# is made absolute here, before the script changes into the repository.
FRONTEND_DIST="${DPDS_FRONTEND_DIST:-}"
if [[ -n "$FRONTEND_DIST" && "$FRONTEND_DIST" != /* ]]; then
  FRONTEND_DIST="$PWD/$FRONTEND_DIST"
fi
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
WORK_DIR=""   # temporary download and unpack directory, removed when the script exits

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

cleanup() {
  if [[ -n "$WORK_DIR" ]]; then
    rm -rf -- "$WORK_DIR"
  fi
}
trap cleanup EXIT

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

# curl, sha256sum and tar fetch and unpack the frontend asset; `check` and a caller-provided
# DPDS_FRONTEND_DIST need none of them.
require_frontend_tools() {
  if [[ -n "$FRONTEND_DIST" ]]; then
    return 0
  fi
  require_cmd curl
  require_cmd sha256sum
  require_cmd tar
}

# A caller-provided DPDS_FRONTEND_DIST must be a built frontend. Checked first by install and
# update, so that nothing is fetched, built or restarted for a wrong path.
require_frontend_dist_dir() {
  if [[ -n "$FRONTEND_DIST" && ! -f "$FRONTEND_DIST/index.html" ]]; then
    error "DPDS_FRONTEND_DIST=$FRONTEND_DIST has no index.html: point it at a built frontend (npm run build in frontend/ writes frontend/dist) or unset it."
    exit 1
  fi
}

# Refuses a tarball that could write outside the directory it is unpacked into: absolute paths,
# ".." path components, symbolic links and hard links. A name that merely contains two dots
# (app..min.js) is fine. Nothing is extracted here.
check_asset_members() {
  local tarball="$1" asset="$2" line types names
  types="$(LC_ALL=C tar -tvzf "$tarball")" || { error "Cannot list the contents of $asset."; exit 1; }
  names="$(LC_ALL=C tar -tzf "$tarball")" || { error "Cannot list the contents of $asset."; exit 1; }
  while IFS= read -r line; do
    case "$line" in
      l*|h*)
        error "Refusing $asset: it contains a symbolic or hard link ($line)."
        exit 1 ;;
    esac
  done <<<"$types"
  while IFS= read -r line; do
    if [[ "$line" == /* ]]; then
      error "Refusing $asset: it contains the absolute path $line."
      exit 1
    fi
    if [[ "/$line/" == *"/../"* ]]; then
      error "Refusing $asset: it contains the path $line with a '..' component."
      exit 1
    fi
  done <<<"$names"
}

########################################
# Build, check, install                #
########################################
build_dpds() {
  info "Building dpds (with telemetry)"
  cargo build --release -p dpds --features telemetry
}

# Sets FRONTEND_BUILD_DIST to the frontend dist the controller is built with (empty: none, the
# controller build then uses frontend/dist if present, else its placeholder page). The dist is the
# caller's DPDS_FRONTEND_DIST as is, or the CI asset of the checked-out frontend/ tree, unpacked
# into a temporary directory outside the repository.
prepare_frontend() {
  FRONTEND_BUILD_DIST=""
  if [[ -n "$FRONTEND_DIST" ]]; then
    info "Using the frontend dist $FRONTEND_DIST"
    FRONTEND_BUILD_DIST="$FRONTEND_DIST"
    return 0
  fi

  local tree base asset dl rc=0
  tree="$(git rev-parse HEAD:frontend)" || tree=""
  if [[ ! "$tree" =~ ^[0-9a-f]{40,64}$ ]]; then
    error "Cannot determine the frontend tree: 'git rev-parse HEAD:frontend' failed."
    exit 1
  fi
  base="${FRONTEND_ASSET_URL%/}"
  asset="dpds-frontend-$tree.tar.gz"
  WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/dpds-frontend.XXXXXX")"
  dl="$WORK_DIR/download"
  mkdir "$dl" "$WORK_DIR/dist"

  # An https base is never downgraded, not even by a redirect. An explicit http:// base (a local
  # mirror) is left alone.
  local -a fetch=(curl -fsSL)
  if [[ "$base" == https://* ]]; then
    fetch+=(--proto '=https' --proto-redir '=https')
  fi

  info "Downloading $asset from $base"
  "${fetch[@]}" -o "$dl/$asset" "$base/$asset" || rc=$?
  if (( rc == 0 )); then
    "${fetch[@]}" -o "$dl/$asset.sha256" "$base/$asset.sha256" || rc=$?
    if (( rc == 22 )); then
      error "The checksum file $asset.sha256 is missing at $base, but the tarball exists: CI did not finish uploading $asset."
      error "Re-run the CI job (frontend-asset-publish) or wait a moment, or set DPDS_FRONTEND_DIST to a locally built frontend dist directory."
      exit 1
    fi
  fi
  if (( rc == 22 )); then
    error "The frontend asset $asset is not available at $base (HTTP error, usually 404)."
    error "CI builds it for each push to nightly or main: push your frontend/ changes and wait for CI, or build the frontend yourself (npm run build in frontend/) and set DPDS_FRONTEND_DIST to its dist directory."
    exit 1
  elif (( rc != 0 )); then
    error "Downloading the frontend asset $asset failed (curl exit code $rc)."
    error "Check the network, or set DPDS_FRONTEND_DIST to a locally built frontend dist directory."
    exit 1
  fi

  if ! (cd "$dl" && sha256sum --check --status "$asset.sha256"); then
    error "Checksum mismatch: $asset does not match $asset.sha256 (sha256sum failed). Not using it."
    exit 1
  fi
  check_asset_members "$dl/$asset" "$asset"
  tar -xzf "$dl/$asset" --no-same-owner -C "$WORK_DIR/dist"
  if [[ ! -f "$WORK_DIR/dist/index.html" ]]; then
    error "$asset has no index.html at its top level."
    exit 1
  fi
  FRONTEND_BUILD_DIST="$WORK_DIR/dist"
}

# build_controller [DIST]: DIST is the frontend dist directory to embed.
build_controller() {
  info "Building dpds-controller"
  if [[ -n "${1:-}" ]]; then
    DPDS_FRONTEND_DIST="$1" cargo build --release -p dpds-controller
  else
    cargo build --release -p dpds-controller
  fi
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
  prepare_frontend
  build_dpds
  build_controller "$FRONTEND_BUILD_DIST"

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
      build_controller "$FRONTEND_DIST"
    fi
  fi
  "$ctl" check
}

cmd_install() {
  require_prerequisites
  require_frontend_tools
  require_frontend_dist_dir
  if [[ -z "$FRONTEND_DIST" && -n "$(git status --porcelain -- frontend)" ]]; then
    warn "frontend/ has uncommitted changes. The CI asset of the committed frontend/ is used; to try your local changes, build the frontend (npm run build in frontend/) and set DPDS_FRONTEND_DIST to its dist directory."
  fi
  install_flow
}

cmd_start() {
  info "Enabling and starting $SERVICE"
  run_privileged systemctl enable --now "$SERVICE"
}

cmd_stop() {
  info "Disabling and stopping $SERVICE (this drains the delay buffers and can take a while)"
  run_privileged systemctl disable --now "$SERVICE"
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

cmd_status() {
  local addr
  addr="$(controller_address)"
  info "Service"
  systemctl status --no-pager "$SERVICE" || true
  info "Controller API and web frontend (http://$addr/)"
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
  require_frontend_tools
  require_frontend_dist_dir

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
  info "Stopping $SERVICE"
  run_privileged systemctl disable --now "$SERVICE" || warn "Could not disable $SERVICE (not installed?)."
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
  echo "Set DPDS_FRONTEND_DIST to a frontend dist directory you built yourself to skip the download."
  echo ""
  echo "Commands:"
  echo "  check        Check prerequisites, then print the host hints (dpds-controller check)"
  echo "  install      Fetch the CI-built frontend, build, run the host check, ask to continue, install binaries and unit"
  echo "  update       git pull nightly, rebuild with the matching frontend, reinstall, restart if it was running"
  echo "  start        Enable and start the controller service (it serves the web frontend)"
  echo "  stop         Disable and stop the controller service"
  echo "  restart      Stop, then start"
  echo "  status       Show service and API status"
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

  # cargo and git work on the repository this script lives in.
  cd "$REPO_ROOT"

  "cmd_$cmd"
}

main "$@"
