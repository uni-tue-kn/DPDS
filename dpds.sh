#!/usr/bin/env bash
# dpds.sh - build, install and run the DPDS controller (a systemd service that also serves the web frontend).
#
# Usage: [DPDS_CHANNEL=latest|nightly|vX.Y.Z] ./dpds.sh [--nightly|--stable] [--yes] {check|install|update|start|stop|restart|status|uninstall}
#
# The controller (dpds-controller) runs natively on the host as root and starts dpds as its child
# process. It embeds the web frontend, built at install and update time from the checked-out tree
# (npm ci, then npm run build in frontend/), and serves it on its own port. The host therefore
# needs Node and npm next to Rust, clang/lld and DPDK; check, install and update verify all of
# them first and list every missing one (nothing is ever installed for you). This script only
# builds, installs and runs the service and never changes host settings. The controller only
# reports hints (`dpds.sh check`).
#
# Channels: DPDS_CHANNEL is latest (the default), nightly or a release tag such as v1.0.0.
# latest follows the branch main, nightly the branch nightly. --nightly and --stable
# override the variable. update switches to the channel's branch (git fetch, checkout, pull --ff-only);
# install builds the tree as it is and only warns when it is on another branch.
#
# A release tag (vX.Y.Z, exists once that version has reached main) pins one release:
# DPDS_CHANNEL=v1.0.0 ./dpds.sh update runs git fetch --tags and checks out that tag (detached, no
# pull), so later updates keep that version until the channel changes. install warns when HEAD is not
# at the tag.
#
# Environment overrides:
#   DPDS_CHANNEL      release channel: latest, nightly or a tag (v1.0.0) (default latest)
#   DPDS_PREFIX       install prefix, binaries go to $DPDS_PREFIX/bin   (default /usr/local)
#   DPDS_ETC_DIR      config dir, holds controller.env, installed-commit (default /etc/dpds)
#   DPDS_SYSTEMD_DIR  where the unit file is installed                   (default /etc/systemd/system)
#   DPDS_BIND         bind address written into a NEW controller.env     (default 0.0.0.0:8100)
#   CARGO_TARGET_DIR  cargo target dir the build output is read from     (default <repo>/target)
#
# Run it as your user, not with sudo or as root: it calls sudo itself for the privileged steps.
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
SYSTEMD_DIR="${DPDS_SYSTEMD_DIR:-/etc/systemd/system}"
BIND="${DPDS_BIND:-0.0.0.0:8100}"
# Never embed a frontend from the caller's environment.
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
# Holds the commit hash of the installed tree; update compares it with HEAD.
STAMP_FILE="$ETC_DIR/installed-commit"
BIN_DIR="$PREFIX/bin"
DEFAULT_PORT=8100

CHANNEL="${DPDS_CHANNEL:-}"
CHANNEL_OVERRIDE=""
CHANNEL_BRANCH=""
CHANNEL_TAG=""
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
  if command -v sudo >/dev/null 2>&1; then
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
  for name in PREFIX ETC_DIR SYSTEMD_DIR; do
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

# Resolves the channel (flags override DPDS_CHANNEL, default latest) and its branch, or its tag.
resolve_channel() {
  CHANNEL="${CHANNEL_OVERRIDE:-${CHANNEL:-latest}}"
  case "$CHANNEL" in
    latest) CHANNEL_BRANCH="main" ;;
    nightly) CHANNEL_BRANCH="nightly" ;;
    *)
      if [[ "$CHANNEL" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        CHANNEL_TAG="$CHANNEL"
      else
        error "Invalid DPDS channel '$CHANNEL' (expected 'latest', 'nightly' or a release tag like 'v1.0.0')"
        exit 1
      fi
      ;;
  esac
}

# Prints the checked-out branch, nothing for a detached HEAD. Fails outside a git work tree
# (a source tarball).
checkout_branch() {
  git branch --show-current 2>/dev/null
}

# warn_branch_mismatch HINT: warns, naming both branches, when the checkout is not on the channel's
# branch. Silent when it is, and when there is no git work tree.
warn_branch_mismatch() {
  local current where
  current="$(checkout_branch)" || return 0
  if [[ "$current" == "$CHANNEL_BRANCH" ]]; then
    return 0
  fi
  if [[ -z "$current" ]]; then
    where="HEAD is detached"
  else
    where="The checkout is on branch $current"
  fi
  warn "$where, but the $CHANNEL channel follows branch $CHANNEL_BRANCH. $1"
}

# warn_tag_mismatch HINT: warns, naming the tag and the checkout, when HEAD is not at the tag's
# commit (also when the tag is not known locally yet). Silent when it is, and when there is no git
# work tree.
warn_tag_mismatch() {
  local current where tag_commit="" head
  current="$(checkout_branch)" || return 0
  tag_commit="$(git rev-parse -q --verify "refs/tags/$CHANNEL_TAG^{commit}" 2>/dev/null)" || tag_commit=""
  head="$(git rev-parse HEAD 2>/dev/null)" || head=""
  if [[ -n "$tag_commit" && "$head" == "$tag_commit" ]]; then
    return 0
  fi
  if [[ -z "$current" ]]; then
    where="HEAD is detached"
  else
    where="The checkout is on branch $current"
  fi
  warn "$where, not at the release tag $CHANNEL_TAG that the channel names. $1"
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

  # No stamp while the installation is half done.
  run_privileged rm -f "$STAMP_FILE"
  info "Installing binaries to $BIN_DIR"
  run_privileged install -d -m 0755 "$BIN_DIR"
  run_privileged install -m 0755 "$release_dir/dpds" "$BIN_DIR/dpds"
  run_privileged install -m 0755 "$release_dir/dpds-controller" "$BIN_DIR/dpds-controller"
  info "Installing $UNIT_DEST"
  install_unit
  install_env_file
  run_privileged systemctl daemon-reload
  local head
  if head="$(git rev-parse HEAD 2>/dev/null)"; then
    echo "$head" | run_privileged tee "$STAMP_FILE" >/dev/null
  fi
  info "Installed. Start it with: $0 start"
}

# True when the installed commit (the stamp) is HEAD and the installed files are all there.
installed_is_current() {
  local head
  head="$(git rev-parse HEAD 2>/dev/null)" || return 1
  [[ -f "$STAMP_FILE" && "$(<"$STAMP_FILE")" == "$head" ]] || return 1
  [[ -f "$BIN_DIR/dpds" && -f "$BIN_DIR/dpds-controller" && -f "$UNIT_DEST" ]]
}

# The working tree must be clean before git moves HEAD: a checkout with local changes can fail
# halfway or silently build something other than the branch or tag.
refuse_dirty_tree() {
  local dirty
  dirty="$(git status --porcelain --untracked-files=no)"
  if [[ -n "$dirty" ]]; then
    error "The working tree has uncommitted changes; commit or stash them first:"
    echo "$dirty" >&2
    exit 1
  fi
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
  if [[ -n "$CHANNEL_TAG" ]]; then
    warn_tag_mismatch "Installing the tree as it is; run $0 update to switch."
  else
    warn_branch_mismatch "Installing the tree as it is; run $0 update to switch."
  fi
  install_flow
}

# Fails with a hint to install first when the unit file of `install` is missing, instead of
# passing on systemctl's "Unit ... does not exist".
require_installed() {
  if [[ ! -f "$UNIT_DEST" ]]; then
    error "$SERVICE is not installed: run ./dpds.sh install first (no $UNIT_DEST)."
    return 1
  fi
}

cmd_start() {
  require_installed || return 1
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
  require_installed || return 1
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
  if ! require_installed; then
    return 0
  fi
  local addr checkout
  addr="$(controller_address)"
  if checkout="$(checkout_branch)"; then
    checkout="branch ${checkout:-(detached HEAD)}"
  else
    checkout="unknown (not a git work tree)"
  fi
  if [[ -n "$CHANNEL_TAG" ]]; then
    info "Channel: $CHANNEL (tag $CHANNEL_TAG), checkout: $checkout"
  else
    info "Channel: $CHANNEL (branch $CHANNEL_BRANCH), checkout: $checkout"
  fi
  info "Service"
  systemctl status --no-pager "$SERVICE" || true
  info "Controller API and web frontend (http://$addr/)"
  if command -v curl >/dev/null 2>&1; then
    local answer version
    if answer="$(curl -fsS --max-time 5 "http://$addr/api/online")"; then
      echo "$answer"
      version="$(first_line "$(sed -n 's/.*"version" *: *"\([^"]*\)".*/\1/p' <<<"$answer")")"
      if [[ -n "$version" ]]; then
        info "Installed version: $version"
      fi
    else
      warn "Controller API is not reachable."
    fi
  else
    warn "curl not found, skipping the API probe."
  fi
  return 0
}

cmd_update() {
  require_prerequisites

  # Refuse before touching anything.
  local was_active=0
  refuse_dirty_tree
  if [[ -z "$CHANNEL_TAG" ]]; then
    warn_branch_mismatch "Switching to $CHANNEL_BRANCH."
  fi

  if systemctl is-active --quiet "$SERVICE"; then
    was_active=1
  fi
  if [[ -n "$CHANNEL_TAG" ]]; then
    info "Updating the repository to the release tag $CHANNEL_TAG"
    git fetch --tags
    if ! git rev-parse -q --verify "refs/tags/$CHANNEL_TAG^{commit}" >/dev/null; then
      error "The release tag $CHANNEL_TAG does not exist (tags appear once a version has reached main). Nothing was changed."
      exit 1
    fi
    git checkout --detach "$CHANNEL_TAG"
  else
    info "Updating the repository to the $CHANNEL channel (branch $CHANNEL_BRANCH)"
    git fetch
    git checkout "$CHANNEL_BRANCH"
    git pull --ff-only
  fi
  if installed_is_current; then
    info "Already up to date at commit $(git rev-parse --short HEAD); nothing to do. $0 install rebuilds and reinstalls anyway."
    return 0
  fi
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
  info "Removing binaries, unit and $STAMP_FILE"
  run_privileged rm -f "$BIN_DIR/dpds" "$BIN_DIR/dpds-controller" "$UNIT_DEST" "$STAMP_FILE"
  run_privileged systemctl daemon-reload || warn "systemctl daemon-reload failed."
  local leftover=/var/lib/dpds-controller
  if [[ -e "$leftover" ]]; then
    info "Removing $leftover, left behind by an older version"
    run_privileged rm -rf -- "$leftover"
  fi
  info "Kept $ENV_FILE; remove it by hand if you do not need it."
}

usage() {
  echo "Usage: [DPDS_CHANNEL=latest|nightly|vX.Y.Z] $0 [--nightly|--stable] [--yes] {check|install|update|start|stop|restart|status|uninstall}"
  echo ""
  echo "Channel: DPDS_CHANNEL is latest (the default, branch main), nightly (branch nightly) or a release tag like v1.0.0."
  echo "  DPDS_CHANNEL=v1.0.0 $0 update installs exactly that release (git fetch --tags, checkout --detach, no pull);"
  echo "  later updates keep it until the channel changes. A release tag exists once that version has reached main."
  echo ""
  echo "Options:"
  echo "  --nightly    Use the nightly channel (branch nightly), overriding DPDS_CHANNEL"
  echo "  --stable     Use the latest channel (branch main), overriding DPDS_CHANNEL"
  echo "  --yes        Do not ask for confirmation (install, update)"
  echo ""
  echo "Commands:"
  echo "  check        Check the prerequisites (Rust, clang/lld, DPDK $DPDK_MIN, Node/npm), then print the host hints (dpds-controller check)"
  echo "  install      Check prerequisites, build the frontend (npm ci, npm run build) and the binaries, run the host check, ask to continue, install binaries and unit (always rebuilds, so it forces a rebuild)"
  echo "  update       Switch to the channel's branch (git fetch, checkout, pull --ff-only) or release tag (git fetch --tags, checkout --detach), rebuild the frontend and the binaries, reinstall, restart if it was running; does nothing when the installed commit is already the channel's head"
  echo "  start        Enable and start the controller service (it serves the web frontend)"
  echo "  stop         Disable and stop the controller service"
  echo "  restart      Stop, then start"
  echo "  status       Show service and API status"
  echo "  uninstall    Stop and remove binaries and unit (controller.env is kept)"
}

main() {
  local cmd="" arg flag=""
  if (( EUID == 0 )); then
    error "Run dpds.sh as your user, not with sudo: it calls sudo itself for the privileged steps; under sudo, root's PATH lacks cargo/rustc/clang."
    exit 1
  fi
  for arg in "$@"; do
    case "$arg" in
      --nightly|--stable)
        if [[ -n "$flag" && "$flag" != "$arg" ]]; then
          error "--nightly and --stable cannot be combined."
          exit 1
        fi
        flag="$arg"
        ;;
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

  case "$cmd" in
    check|install|update|start|stop|restart|status|uninstall) ;;
    "") usage >&2; exit 1 ;;
    *) error "Unknown command: $cmd"; usage >&2; exit 1 ;;
  esac

  case "$flag" in
    --nightly) CHANNEL_OVERRIDE="nightly" ;;
    --stable) CHANNEL_OVERRIDE="latest" ;;
  esac
  resolve_channel

  # cargo and git work on the repository this script lives in.
  cd "$REPO_ROOT"

  "cmd_$cmd"
}

main "$@"
