#!/usr/bin/env bash
# dpds.sh - build, install and run the DPDS controller (a systemd service that also serves the web frontend).
#
# Usage: ./dpds.sh [--nightly|--stable] [--yes] {check|install|update|start|stop|restart|status|uninstall}
#
# The controller (dpds-controller) runs natively on the host as root and starts dpds as its child
# process. It embeds the web frontend at build time and serves it on its own port. install and
# update build the frontend from the checked-out tree (npm ci, then npm run build in frontend/)
# and build the controller with the resulting frontend/dist. The host therefore needs Node and
# npm next to Rust, clang/lld and DPDK; check, install and update verify all of them first and
# list every missing one (nothing is ever installed for you). This script only builds, installs
# and runs the service and never changes host settings. The controller only reports hints
# (`dpds.sh check`).
#
# Environment overrides:
#   DPDS_PREFIX       install prefix, binaries go to $DPDS_PREFIX/bin   (default /usr/local)
#   DPDS_ETC_DIR      config dir, holds controller.env                   (default /etc/dpds)
#   DPDS_STATE_DIR    controller state dir                               (default /var/lib/dpds-controller)
#   DPDS_SYSTEMD_DIR  where the unit file is installed                   (default /etc/systemd/system)
#   DPDS_BIND         bind address written into a NEW controller.env     (default 0.0.0.0:8100)
#   CARGO_TARGET_DIR  cargo target dir the build output is read from     (default <repo>/target)
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
# The controller is built with the frontend this script builds, never with one from the caller's
# environment.
unset DPDS_FRONTEND_DIST
FRONTEND_DIR="$REPO_ROOT/frontend"
FRONTEND_DIST="$FRONTEND_DIR/dist"
# Minimum DPDK version; crates/dpdk-sys/build.rs requires the same.
DPDK_MIN="26.07"
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
PREREQ_FAILURES=0
prereq_ok() { echo "[OK   ] $(ts) $*"; }
prereq_fail() {
  error "$*"
  PREREQ_FAILURES=$((PREREQ_FAILURES + 1))
}

# version_at_least HAVE WANT: compares the first three components numerically; a suffix after the
# digits of a component (1.99.0-nightly) is ignored.
version_at_least() {
  local -a have want
  local i h w
  IFS=. read -ra have <<<"${1#v}"
  IFS=. read -ra want <<<"$2"
  for i in 0 1 2; do
    h="${have[i]:-0}"; h="${h%%[!0-9]*}"
    w="${want[i]:-0}"; w="${w%%[!0-9]*}"
    if (( 10#${h:-0} > 10#${w:-0} )); then return 0; fi
    if (( 10#${h:-0} < 10#${w:-0} )); then return 1; fi
  done
  return 0
}

# first_line TEXT: the first line of TEXT, without piping into head.
first_line() {
  local line=""
  read -r line <<<"$1" || true
  echo "$line"
}

# check_llvm_tool TOOL RUST_LLVM: TOOL (clang, ld.lld) must exist and be of rustc's LLVM major.
check_llvm_tool() {
  local tool="$1" want="$2" out major
  if ! command -v "$tool" >/dev/null 2>&1; then
    prereq_fail "$tool not found: install $tool of LLVM ${want:-the version rustc uses}."
    return 0
  fi
  [[ -n "$want" ]] || return 0
  out="$(first_line "$("$tool" --version)")"
  major="$(version_major <<<"$out")"
  if [[ "$major" != "$want" ]]; then
    prereq_fail "$tool is LLVM ${major:-unknown} but rustc uses LLVM $want: install $tool of LLVM $want or put it first in PATH."
  else
    prereq_ok "$tool is LLVM $major, like rustc"
  fi
}

# Checks Rust, clang/lld, DPDK and Node/npm and reports every unmet one before exiting with 1.
# Capture each tool's output first: piping it into `head` can SIGPIPE the tool, and pipefail
# then fails the whole assignment at random.
require_prerequisites() {
  PREREQ_FAILURES=0
  local out rust_ver="" rust_llvm="" rust_min node_min node_ver found

  if command -v cargo >/dev/null 2>&1; then
    prereq_ok "cargo found"
  else
    prereq_fail "cargo not found: install Rust (rustup)."
  fi
  rust_min="$(sed -n 's/^rust-version *= *"\([0-9][0-9.]*\)".*/\1/p' "$REPO_ROOT/crates/dpds-core/Cargo.toml" 2>/dev/null)" || rust_min=""
  rust_min="$(first_line "$rust_min")"
  if command -v rustc >/dev/null 2>&1; then
    out="$(rustc -vV)" || out=""
    rust_ver="$(first_line "$(sed -n 's/^release: *//p' <<<"$out")")"
    rust_llvm="$(sed -n 's/^LLVM version: *//p' <<<"$out" | version_major)"
    if [[ -z "$rust_ver" ]]; then
      prereq_fail "Cannot determine the version of rustc ('rustc -vV' has no 'release' line): install Rust (rustup)."
    elif [[ -n "$rust_min" ]] && ! version_at_least "$rust_ver" "$rust_min"; then
      prereq_fail "rustc $rust_ver is older than the $rust_min the workspace requires: update Rust (rustup update)."
    else
      prereq_ok "rustc $rust_ver"
    fi
    if [[ -z "$rust_llvm" ]]; then
      prereq_fail "Cannot determine the LLVM version of rustc ('rustc -vV' has no 'LLVM version' line)."
    fi
  else
    prereq_fail "rustc not found: install Rust (rustup)."
  fi

  check_llvm_tool clang "$rust_llvm"
  check_llvm_tool ld.lld "$rust_llvm"

  if ! command -v pkg-config >/dev/null 2>&1; then
    prereq_fail "pkg-config not found: install pkg-config."
  elif ! pkg-config --exists libdpdk >/dev/null 2>&1; then
    prereq_fail "libdpdk not found: 'pkg-config --exists libdpdk' failed. Install DPDK $DPDK_MIN or newer and set PKG_CONFIG_PATH."
  else
    found="$(pkg-config --modversion libdpdk 2>/dev/null)" || found=""
    found="$(first_line "$found")"
    if pkg-config --atleast-version="$DPDK_MIN" libdpdk >/dev/null 2>&1; then
      prereq_ok "libdpdk ${found:-found} (at least $DPDK_MIN)"
    else
      prereq_fail "libdpdk ${found:-of an unknown version} is older than the required $DPDK_MIN: install DPDK $DPDK_MIN or newer and set PKG_CONFIG_PATH."
    fi
  fi

  node_min="$(sed -n 's/.*"node"[[:space:]]*:[[:space:]]*">=\([0-9][0-9.]*\)".*/\1/p' "$FRONTEND_DIR/package.json" 2>/dev/null)" || node_min=""
  node_min="$(first_line "$node_min")"
  if ! command -v node >/dev/null 2>&1; then
    prereq_fail "node not found: install Node.js ${node_min:-the engines.node minimum} or newer (it includes npm)."
  elif [[ -z "$node_min" ]]; then
    prereq_fail "Cannot read the minimum node version (engines.node) from frontend/package.json."
  else
    node_ver="$(first_line "$(node --version)")"
    node_ver="${node_ver#v}"
    if version_at_least "$node_ver" "$node_min"; then
      prereq_ok "node $node_ver (at least $node_min)"
    else
      prereq_fail "node $node_ver is older than the required $node_min: install Node.js $node_min or newer."
    fi
  fi
  if command -v npm >/dev/null 2>&1; then
    prereq_ok "npm $(first_line "$(npm --version)")"
  else
    prereq_fail "npm not found: install npm (it comes with Node.js)."
  fi

  if (( PREREQ_FAILURES > 0 )); then
    error "$PREREQ_FAILURES prerequisite(s) not met, see above. Nothing was changed."
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

# Builds the frontend of the checked-out tree into frontend/dist. The previous dist is removed only
# after npm ci succeeded (so a failed npm ci, e.g. offline, keeps it), and before npm run build, so
# the index.html check below proves that this build wrote it.
build_frontend() {
  info "Building the frontend (npm ci, npm run build in $FRONTEND_DIR)"
  if ! (cd "$FRONTEND_DIR" && npm ci); then
    error "npm ci failed in $FRONTEND_DIR. Nothing was installed."
    exit 1
  fi
  rm -rf -- "$FRONTEND_DIST"
  if ! (cd "$FRONTEND_DIR" && npm run build); then
    error "The frontend build failed (npm run build in $FRONTEND_DIR). Nothing was installed."
    exit 1
  fi
  if [[ ! -f "$FRONTEND_DIST/index.html" ]]; then
    error "The frontend build wrote no index.html to $FRONTEND_DIST. Nothing was installed."
    exit 1
  fi
}

# build_controller [DIST]: DIST is the frontend dist directory to embed; without it the controller
# build uses frontend/dist if present, else its placeholder page.
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
  build_frontend
  build_dpds
  build_controller "$FRONTEND_DIST"

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
      build_controller
    fi
  fi
  "$ctl" check
}

cmd_install() {
  require_prerequisites
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
  echo "Commands:"
  echo "  check        Check the prerequisites (Rust, clang/lld, DPDK $DPDK_MIN, Node/npm), then print the host hints (dpds-controller check)"
  echo "  install      Check prerequisites, build the frontend (npm ci, npm run build) and the binaries, run the host check, ask to continue, install binaries and unit"
  echo "  update       git pull nightly, rebuild the frontend and the binaries, reinstall, restart if it was running"
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
