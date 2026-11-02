#!/usr/bin/env bash
# Tests for dpds.sh (repo root) and packaging/dpds-controller.service.
#
# Run:  bash packaging/tests/dpds_sh.test.sh [test_name_substring ...]
# Plain bash, no dependencies (no bats, no shellcheck). Exit 0 iff every test passes.
#
# =====================================================================================
# CONTRACT (this header is the spec the tests enforce; dpds.sh must satisfy it)
# =====================================================================================
#
# Command line:   dpds.sh [--nightly|--stable] [--yes] COMMAND
#   COMMAND is one of: check install update start stop restart status uninstall.
#   Flags come before the command (any order between them).
#   - no arguments, unknown command or unknown option: exit 1, usage text (containing the word
#     "Usage" and every command name) on stdout or stderr, nothing else is executed.
#   - --nightly is the default channel and is accepted explicitly.
#   - --stable: error on stderr saying that no Rust stable release exists yet ("stable release"),
#     exit non-zero, nothing is executed (no cargo/git/systemctl/docker).
#   - --nightly together with --stable: exit non-zero, nothing is executed.
#   - Errors go to stderr. The script may be started from any working directory: it works on the
#     repository it lives in (cargo, git and `docker compose` run in the repo root).
#
# Environment overrides (so tests never touch the real system):
#   DPDS_PREFIX       install prefix; binaries go to $DPDS_PREFIX/bin     (default /usr/local)
#   DPDS_ETC_DIR      config dir; env file is $DPDS_ETC_DIR/controller.env (default /etc/dpds)
#   DPDS_STATE_DIR    controller state dir                                (default /var/lib/dpds-controller)
#   DPDS_SYSTEMD_DIR  where the unit is installed                         (default /etc/systemd/system)
#   DPDS_BIND         bind address written into a NEW env file            (default 0.0.0.0:8100)
#   CARGO_TARGET_DIR  standard cargo variable; built binaries are read from
#                     $CARGO_TARGET_DIR/release/{dpds,dpds-controller}    (default <repo>/target)
#   Privileged operations go through sudo when EUID != 0 (like p4tg.sh's run_privileged). Tests
#   do not require that sudo is used, only that the effects happen.
#
# Prerequisites (checked by `check`, `install`, `update`; NOT by start/stop/restart/status/
# uninstall, which only need systemctl/docker/curl). Each missing prerequisite is an error on
# stderr that names the tool, exit non-zero, and nothing is built or installed. docker, the
# compose plugin and a reachable daemon (spec v7 P3) are needed only by `install` and `update`:
# `check` builds and runs the controller and works on a host without docker.
#   cargo, rustc                         on PATH
#   clang and ld.lld                     whose LLVM major equals the one reported by
#                                        `rustc -vV` ("LLVM version: N.x.y"); the error names the
#                                        tool and both majors. Minor/patch/vendor prefix ignored.
#   pkg-config with libdpdk              `pkg-config --exists libdpdk`; error names "libdpdk"
#   docker, and the compose plugin       `docker compose version`; error names "compose"
#                                        (install/update only)
#   a reachable docker daemon            `docker info` must succeed (install/update only, before
#                                        anything is built); the error names "docker" and "daemon"
#   No system packages are ever installed.
#
# check
#   Prerequisites, then runs `dpds-controller check` and passes its stdout and exit code through.
#   Binary used: $DPDS_PREFIX/bin/dpds-controller if executable; otherwise the one in
#   $CARGO_TARGET_DIR/release if present; otherwise it first runs
#   `cargo build --release -p dpds-controller` (the build is in the repo root) and uses that.
#
# install  (order: prerequisites, build, host check, confirmation, installation)
#   1. `cargo build --release ... dpds ... --features telemetry` (one invocation naming only dpds)
#      and `cargo build --release ... dpds-controller` (a separate invocation).
#   2. runs `dpds-controller check` on the freshly built controller, through the privileged runner
#      (sudo when EUID != 0) so root-only hints work; its output is shown. Its exit
#      code is feedback only and never aborts the install (hints are not errors for installing).
#   3. unless --yes: asks "continue?" on stdin. y/yes (any case) continues; anything else
#      including EOF aborts: exit 1, nothing installed, no daemon-reload, no docker build.
#   4. installs dpds and dpds-controller (executable) into $DPDS_PREFIX/bin; the unit from
#      packaging/dpds-controller.service into $DPDS_SYSTEMD_DIR/dpds-controller.service with
#      `ExecStart=$DPDS_PREFIX/bin/dpds-controller serve` and
#      `EnvironmentFile=-$DPDS_ETC_DIR/controller.env` substituted for the defaults;
#      creates $DPDS_ETC_DIR/controller.env ONLY if absent (never overwritten) containing
#        DPDS_CONTROLLER_BIND=<DPDS_BIND or 0.0.0.0:8100>
#        DPDS_CONTROLLER_DPDS_BIN=$DPDS_PREFIX/bin/dpds
#        DPDS_CONTROLLER_STATE_DIR=$DPDS_STATE_DIR
#      (values may be double-quoted); creates $DPDS_STATE_DIR.
#   5. `systemctl daemon-reload`, then `docker compose build` (frontend image, repo root).
#   6. Does NOT enable or start anything (no `systemctl enable|start`, no `docker compose up`).
#   7. Re-running install is fine (idempotent). Failure of a build/compose step: exit non-zero.
#
# start    `systemctl enable --now dpds-controller`, then `docker compose up -d`. Before docker
#          compose up/build (also in restart, update and install) the script exports
#          DPDS_CONTROLLER=http://127.0.0.1:<port>, <port> from DPDS_CONTROLLER_BIND in
#          $DPDS_ETC_DIR/controller.env (default 8100), so the frontend proxies to the configured
#          port (spec v7 P1). DPDS_FRONTEND_BIND / DPDS_FRONTEND_PORT set by the caller reach compose.
# stop     `systemctl disable --now dpds-controller`, then `docker compose down` (both run).
# restart  stop followed by start. When the stop fails, start is still attempted (a running
#          service must not be left stopped) and the failure is reported: exit non-zero.
#   A failing systemctl/docker call makes the command exit non-zero.
# status   `systemctl status dpds-controller`, `docker compose ps`, and
#          `curl ... http://<host>:<port>/api/online` where host:port is the bind address in
#          $DPDS_ETC_DIR/controller.env (default 127.0.0.1:8100; 0.0.0.0 is probed as 127.0.0.1).
#          Always runs all three, prints their
#          output, exits 0 even if the service is inactive or the API is unreachable.
# update   `systemctl is-active dpds-controller` is queried; a dirty working tree is refused
#          (error, non-zero, before any checkout/pull/build); a warning names the current branch
#          (or "detached") when it is not nightly; then `git fetch`, `git checkout nightly`,
#          `git pull --ff-only` (in the repo root; a failing git call aborts before building);
#          then the install flow (same prompt unless --yes); then, only if the service was
#          active, restart (= stop + start). If it was not active nothing is started.
# uninstall (needs --yes semantic only for install-like prompts; none here)
#          `systemctl disable --now dpds-controller` (failure tolerated), `docker compose down`,
#          removes $DPDS_PREFIX/bin/{dpds,dpds-controller} and the unit, `systemctl daemon-reload`.
#          KEEPS $DPDS_STATE_DIR and $DPDS_ETC_DIR/controller.env untouched. Nothing installed
#          => still exit 0.
#
# Never tunes the host: no ethtool, setpci, sysctl, mlxconfig, cpupower, hugepage/proc/sys writes,
# neither in the script nor in the unit file (the controller only reports hints).
#
# packaging/dpds-controller.service
#   runs as root (no User= or User=root), `EnvironmentFile=-/etc/dpds/controller.env`,
#   ExecStart=/usr/local/bin/dpds-controller serve, KillSignal=SIGTERM, TimeoutStopSec >= 120 s,
#   Restart=on-failure, [Install] WantedBy=multi-user.target; parses without warnings under
#   `systemd-analyze verify`.
#
# Stub conventions used by this file (PATH contains only stubs + whitelisted base utilities):
#   every stub appends "<tool> <argv>" to $STUB_LOG ("sudo" is transparent and only logged to
#   $STUB_LOG.sudo); "<tool> <argv> @ <dir>" with the effective directory goes to $STUB_LOG.dirs.
# =====================================================================================

set -u

HERE="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -P -- "$HERE/../.." && pwd)"
SCRIPT="$REPO_ROOT/dpds.sh"
UNIT="$REPO_ROOT/packaging/dpds-controller.service"

BASE_UTILS="bash sh env timeout cat sed grep awk date mkdir cp install rm rmdir dirname basename readlink tee tr
  head tail sort uniq cut chmod chown ln mv id uname wc sleep touch find xargs stat realpath mktemp ls
  true false cmp expr hostname whoami diff printf test tty"
# Any call to one of these is a tuning attempt (tripwires).
TUNING_TOOLS="ethtool setpci sysctl mlxconfig mlxfwreset cpupower tuned-adm grubby update-grub modprobe insmod"
# The tools every stub run provides.
STUB_TOOLS="sudo systemctl docker cargo git curl pkg-config clang ld.lld rustc"

PASS=0
FAIL=0
FAILED_NAMES=()
ROOT_TMP="$(mktemp -d)"
trap 'rm -rf "$ROOT_TMP"' EXIT

# ------------------------------------------------------------------ assertions
# Inside a test (subshell) a failed assertion records the failure and the test continues.
T_FAILED=0
fail() { echo "    FAIL: $*"; T_FAILED=1; }
assert_eq()  { [[ "$1" == "$2" ]] || fail "${3:-values differ}: expected '$2', got '$1'"; }
assert_rc()  { [[ "$RC" == "$1" ]] || fail "exit code: expected $1, got $RC (${2:-})"; }
assert_rc_nonzero() { [[ "$RC" != 0 ]] || fail "expected a non-zero exit code, got 0 (${1:-})"; }
assert_out_has()  { grep -qiF -- "$1" <<<"$OUT" || fail "stdout lacks '$1'"; }
assert_err_has()  { grep -qiF -- "$1" <<<"$ERR" || fail "stderr lacks '$1'"; }
assert_all_has()  { grep -qiF -- "$1" <<<"$OUT$ERR" || fail "output lacks '$1'"; }
assert_log_has()  { grep -Eq -- "$1" "$STUB_LOG" 2>/dev/null || fail "call log lacks /$1/"; }
assert_log_lacks() { ! grep -Eq -- "$1" "$STUB_LOG" 2>/dev/null || fail "call log unexpectedly has /$1/: $(grep -E -- "$1" "$STUB_LOG" | head -3 | tr '\n' '|')"; }
assert_log_empty() { [[ ! -s "$STUB_LOG" ]] || fail "expected no tool calls, got: $(tr '\n' '|' <"$STUB_LOG")"; }
assert_file()    { [[ -f "$1" ]] || fail "missing file $1"; }
assert_no_file() { [[ ! -e "$1" ]] || fail "unexpected $1"; }
assert_exec()    { [[ -x "$1" ]] || fail "not executable: $1"; }
assert_file_has() { grep -Eq -- "$2" "$1" 2>/dev/null || fail "$1 lacks /$2/"; }
# assert_before REGEX_A REGEX_B: first match of A is on an earlier line than the first match of B.
assert_before() {
  local a b
  a="$(grep -En -- "$1" "$STUB_LOG" 2>/dev/null | head -1 | cut -d: -f1)"
  b="$(grep -En -- "$2" "$STUB_LOG" 2>/dev/null | head -1 | cut -d: -f1)"
  if [[ -z "$a" || -z "$b" ]]; then
    fail "order check /$1/ before /$2/: missing call (a='${a}' b='${b}')"
  elif (( a >= b )); then
    fail "expected /$1/ (line $a) before /$2/ (line $b)"
  fi
}
# assert_env_has REGEX: a `docker compose` call logged with its DPDS_* environment matches.
assert_env_has() { grep -Eq -- "$1" "$STUB_LOG.env" 2>/dev/null || fail "compose env log lacks /$1/: $(tr '\n' '|' <"$STUB_LOG.env" 2>/dev/null)"; }
# assert_after_last A B: some match of B exists after the first match of A.
assert_dir_of() { # assert_dir_of REGEX EXPECTED_DIR : every .dirs line matching REGEX ran in EXPECTED_DIR
  local n=0 line d
  while IFS= read -r line; do
    n=$((n + 1)); d="${line##* @ }"
    [[ "$d" == "$2" ]] || fail "'${line%% @ *}' ran in '$d', expected '$2'"
  done < <(grep -E -- "$1" "$STUB_LOG.dirs" 2>/dev/null)
  (( n > 0 )) || fail "no call matching /$1/ recorded in dirs log"
}

# ------------------------------------------------------------------ sandbox
# Everything below is called inside the per-test subshell.
make_stub() { # make_stub NAME  (body on stdin; prelude defines $LOG)
  { printf '#!/bin/bash\nLOG="${STUB_LOG:?}"\n'; cat; } >"$T/stubs/$1"
  chmod +x "$T/stubs/$1"
}

write_controller_stub() { # write_controller_stub PATH  - behaves like dpds-controller
  cat >"$1" <<'EOF'
#!/bin/bash
echo "dpds-controller $*" >> "${STUB_LOG:?}"
echo "$0" >> "${STUB_LOG}.ctlpaths"
if [ "${1:-}" = check ]; then
  echo "CHECK-OUTPUT-MARKER"
  exit "${STUB_CONTROLLER_RC:-0}"
fi
exit 0
EOF
  chmod +x "$1"
}

setup_sandbox() {
  T="$(mktemp -d "$ROOT_TMP/t.XXXXXX")"
  mkdir -p "$T/stubs" "$T/base" "$T/cwd" "$T/home"
  local u p
  for u in $BASE_UTILS; do
    p="$(command -v "$u" 2>/dev/null)" && [[ "$p" == /* ]] && ln -sf "$p" "$T/base/$u"
  done
  export STUB_LOG="$T/calls.log"
  : >"$STUB_LOG"; : >"$STUB_LOG.dirs"; : >"$STUB_LOG.sudo"; : >"$STUB_LOG.env"
  unset STUB_ACTIVE STUB_NO_LIBDPDK STUB_NO_COMPOSE STUB_RUSTC_LLVM STUB_CLANG_VERSION STUB_LLD_VERSION \
        STUB_CONTROLLER_RC STUB_CARGO_FAIL STUB_SYSTEMCTL_RC STUB_SYSTEMCTL_FAIL_SUB STUB_DOCKER_FAIL_SUB \
        STUB_GIT_FAIL_SUB STUB_CURL_RC DPDS_BIND STUB_DOCKER_INFO_FAIL STUB_GIT_DIRTY STUB_GIT_BRANCH \
        DPDS_CONTROLLER DPDS_FRONTEND_BIND DPDS_FRONTEND_PORT
  export DPDS_PREFIX="$T/prefix" DPDS_ETC_DIR="$T/etc" DPDS_STATE_DIR="$T/state" DPDS_SYSTEMD_DIR="$T/systemd"
  export CARGO_TARGET_DIR="$T/target"
  export HOME="$T/home"
  mkdir -p "$DPDS_PREFIX/bin" "$DPDS_SYSTEMD_DIR"

  # sudo: transparent (strips -E/-n), recorded separately.
  make_stub sudo <<'EOF'
echo "sudo $*" >> "$LOG.sudo"
while [ "${1:-}" = -E ] || [ "${1:-}" = -n ]; do shift; done
exec "$@"
EOF
  make_stub systemctl <<'EOF'
echo "systemctl $*" >> "$LOG"; echo "systemctl $* @ $PWD" >> "$LOG.dirs"
sub=""; for a in "$@"; do case $a in -*) ;; *) sub=$a; break;; esac; done
case $sub in
  is-active) if [ "${STUB_ACTIVE:-0}" = 1 ]; then echo active; exit 0; else echo inactive; exit 3; fi;;
  status) echo "STUB-SYSTEMCTL-STATUS"; exit "${STUB_SYSTEMCTL_RC:-0}";;
esac
[ -n "${STUB_SYSTEMCTL_FAIL_SUB:-}" ] && [ "$sub" = "$STUB_SYSTEMCTL_FAIL_SUB" ] && { echo "Failed: stub" >&2; exit 5; }
exit "${STUB_SYSTEMCTL_RC:-0}"
EOF
  make_stub docker <<'EOF'
dir=$PWD
if [ "${1:-}" = compose ]; then
  shift
  while [ $# -gt 0 ]; do
    case $1 in
      -f|--file) dir=$(cd "$(dirname "$2")" 2>/dev/null && pwd -P); shift 2;;
      --project-directory) dir=$(cd "$2" 2>/dev/null && pwd -P); shift 2;;
      -p|--project-name|--profile|--env-file) shift 2;;
      *) break;;
    esac
  done
  echo "docker compose $*" >> "$LOG"; echo "docker compose $* @ $dir" >> "$LOG.dirs"
  echo "docker compose $* :: DPDS_CONTROLLER=${DPDS_CONTROLLER-<unset>} DPDS_FRONTEND_BIND=${DPDS_FRONTEND_BIND-<unset>} DPDS_FRONTEND_PORT=${DPDS_FRONTEND_PORT-<unset>}" >> "$LOG.env"
  case ${1:-} in
    version) if [ -n "${STUB_NO_COMPOSE:-}" ]; then echo "docker: 'compose' is not a docker command." >&2; exit 1; fi
             echo "Docker Compose version v2.30.0"; exit 0;;
    ps) echo "STUB-COMPOSE-PS";;
  esac
  [ -n "${STUB_DOCKER_FAIL_SUB:-}" ] && [ "${1:-}" = "$STUB_DOCKER_FAIL_SUB" ] && { echo "stub compose failure" >&2; exit 1; }
  exit 0
fi
echo "docker $*" >> "$LOG"; echo "docker $* @ $dir" >> "$LOG.dirs"
if [ "${1:-}" = info ] && [ -n "${STUB_DOCKER_INFO_FAIL:-}" ]; then
  echo "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?" >&2
  exit 1
fi
exit 0
EOF
  make_stub git <<'EOF'
dir=$PWD
if [ "${1:-}" = -C ]; then dir=$(cd "$2" 2>/dev/null && pwd -P); shift 2; fi
echo "git $*" >> "$LOG"; echo "git $* @ $dir" >> "$LOG.dirs"
# current branch: STUB_GIT_BRANCH (default nightly); "HEAD" means a detached HEAD
br="${STUB_GIT_BRANCH:-nightly}"
case "${1:-}" in
  rev-parse) echo "$br";;
  branch) [ "$br" = HEAD ] || echo "$br";;
  symbolic-ref) if [ "$br" = HEAD ]; then echo "fatal: ref HEAD is not a symbolic ref" >&2; exit 128; fi; echo "$br";;
  status) [ -n "${STUB_GIT_DIRTY:-}" ] && echo " M dpds.sh";;
  diff|diff-index) [ -n "${STUB_GIT_DIRTY:-}" ] && exit 1;;
esac
[ -n "${STUB_GIT_FAIL_SUB:-}" ] && [ "${1:-}" = "$STUB_GIT_FAIL_SUB" ] && { echo "fatal: stub git failure" >&2; exit 1; }
exit 0
EOF
  make_stub curl <<'EOF'
echo "curl $*" >> "$LOG"
echo '{"version":"stub-online"}'
exit "${STUB_CURL_RC:-0}"
EOF
  make_stub pkg-config <<'EOF'
echo "pkg-config $*" >> "$LOG"
if [ -n "${STUB_NO_LIBDPDK:-}" ]; then echo "Package libdpdk was not found" >&2; exit 1; fi
case "$*" in *--modversion*) echo "25.11.0";; *--exists*|*--cflags*|*--libs*) ;; esac
exit 0
EOF
  make_stub clang <<'EOF'
echo "clang $*" >> "$LOG"
echo "Debian clang version ${STUB_CLANG_VERSION:-23.1.0} (1~stub)"
echo "Target: aarch64-unknown-linux-gnu"
EOF
  make_stub ld.lld <<'EOF'
echo "ld.lld $*" >> "$LOG"
echo "Debian LLD ${STUB_LLD_VERSION:-23.1.0} (compatible with GNU linkers)"
EOF
  make_stub rustc <<'EOF'
echo "rustc $*" >> "$LOG"
case "$*" in
  *-vV*|*"--version --verbose"*|*"-V -v"*|*"--verbose --version"*)
    printf 'rustc 1.99.0-nightly (abcdef123 2026-10-01)\nbinary: rustc\ncommit-hash: abcdef123\nhost: aarch64-unknown-linux-gnu\nrelease: 1.99.0-nightly\nLLVM version: %s\n' "${STUB_RUSTC_LLVM:-23.1.0}";;
  *) echo "rustc 1.99.0-nightly (abcdef123 2026-10-01)";;
esac
EOF
  make_stub cargo <<'EOF'
dir=$PWD; prev=""
for a in "$@"; do [ "$prev" = --manifest-path ] && dir=$(cd "$(dirname "$a")" 2>/dev/null && pwd -P); prev=$a; done
echo "cargo $*" >> "$LOG"; echo "cargo $* @ $dir" >> "$LOG.dirs"
case "${1:-}" in
  --version|-V|-vV|--version=*) echo "cargo 1.99.0-nightly (stub)"; exit 0;;
  build) ;;
  *) exit 0;;
esac
if [ -n "${STUB_CARGO_FAIL:-}" ]; then echo "error: stub build failure" >&2; exit 101; fi
pkgs=(); prev=""
for a in "$@"; do case $prev in -p|--package|--bin) pkgs+=("$a");; esac; prev=$a; done
[ ${#pkgs[@]} -eq 0 ] && pkgs=(dpds dpds-controller)
out="${CARGO_TARGET_DIR:?}/release"; mkdir -p "$out"
for p in "${pkgs[@]}"; do
  case $p in
    dpds-controller)
      cat >"$out/dpds-controller" <<'CTL'
#!/bin/bash
echo "dpds-controller $*" >> "${STUB_LOG:?}"
echo "$0" >> "${STUB_LOG}.ctlpaths"
if [ "${1:-}" = check ]; then echo "CHECK-OUTPUT-MARKER"; exit "${STUB_CONTROLLER_RC:-0}"; fi
exit 0
CTL
      echo "# STUB-BINARY-dpds-controller" >>"$out/dpds-controller"
      chmod +x "$out/dpds-controller";;
    dpds)
      printf '#!/bin/bash\n# STUB-BINARY-dpds\nexit 0\n' >"$out/dpds"; chmod +x "$out/dpds";;
  esac
done
exit 0
EOF
  for u in $TUNING_TOOLS; do
    make_stub "$u" <<EOF
echo "$u \$*" >> "\$LOG"
EOF
  done
}

# Mimic a finished install (for uninstall/update/status tests).
fake_install() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  printf '#!/bin/bash\nexit 0\n' >"$DPDS_PREFIX/bin/dpds"; chmod +x "$DPDS_PREFIX/bin/dpds"
  mkdir -p "$DPDS_ETC_DIR" "$DPDS_STATE_DIR"
  printf 'DPDS_CONTROLLER_BIND=0.0.0.0:8100\n' >"$DPDS_ETC_DIR/controller.env"
  echo '{"links":[]}' >"$DPDS_STATE_DIR/links.json"
  printf '[Service]\nExecStart=%s/bin/dpds-controller serve\n' "$DPDS_PREFIX" >"$DPDS_SYSTEMD_DIR/dpds-controller.service"
}

remove_stub() { rm -f "$T/stubs/$1"; }

# run_dpds ARGS... : sets OUT (stdout), ERR (stderr), RC. Stdin from $STDIN (default empty).
run_dpds() {
  local errf="$T/stderr" outf="$T/stdout"
  ( cd "$T/cwd" && PATH="$T/stubs:$T/base" timeout 60 bash "$SCRIPT" "$@" <"${STDIN:-/dev/null}" >"$outf" 2>"$errf" )
  RC=$?
  OUT="$(cat "$outf")"; ERR="$(cat "$errf")"
}
feed() { STDIN="$T/stdin"; printf '%s' "$1" >"$STDIN"; }

installed_unit() { echo "$DPDS_SYSTEMD_DIR/dpds-controller.service"; }

# ------------------------------------------------------------------ tests: usage / channel
test_script_exists_and_is_executable_bash() {
  assert_file "$SCRIPT"
  head -1 "$SCRIPT" | grep -Eq '^#!.*bash' || fail "dpds.sh needs a bash shebang"
  bash -n "$SCRIPT" 2>/dev/null || fail "dpds.sh has a syntax error"
}

test_no_args_prints_usage_exit1() {
  run_dpds
  assert_rc 1
  assert_all_has "Usage"
  local c; for c in check install update start stop restart status uninstall; do assert_all_has "$c"; done
  assert_log_empty
}

test_unknown_command_exit1_with_usage() {
  run_dpds frobnicate
  assert_rc 1
  assert_all_has "Usage"
  assert_log_empty
}

test_unknown_option_exit1() {
  run_dpds --bogus install
  assert_rc 1
  assert_all_has "Usage"
  assert_log_empty
}

test_stable_prints_no_release_and_fails() {
  run_dpds --stable install
  assert_rc_nonzero
  assert_err_has "stable release"
  assert_log_empty
}

test_stable_with_every_command_runs_nothing() {
  local c
  for c in check install update start stop restart status uninstall; do
    : >"$STUB_LOG"
    run_dpds --stable --yes "$c"
    assert_rc_nonzero "--stable $c"
    assert_err_has "stable release"
    assert_log_empty
  done
}

test_nightly_and_stable_conflict() {
  run_dpds --nightly --stable check
  assert_rc_nonzero
  assert_log_empty
}

test_nightly_flag_accepted_same_as_default() {
  run_dpds --nightly check
  assert_rc 0
  assert_out_has "CHECK-OUTPUT-MARKER"
}

# ------------------------------------------------------------------ tests: check
test_check_all_present_runs_controller_check_and_passes_output() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  run_dpds check
  assert_rc 0 "$ERR"
  assert_out_has "CHECK-OUTPUT-MARKER"
  assert_log_has '^dpds-controller check'
  assert_log_lacks '^cargo build'
  assert_eq "$(cat "$STUB_LOG.ctlpaths")" "$DPDS_PREFIX/bin/dpds-controller" "controller binary used"
}

test_check_passes_through_nonzero_exit_code() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  export STUB_CONTROLLER_RC=2
  run_dpds check
  assert_rc 2
  assert_out_has "CHECK-OUTPUT-MARKER"
}

test_check_without_any_controller_builds_it_first() {
  run_dpds check
  assert_rc 0 "$ERR"
  assert_log_has '^cargo build .*--release'
  assert_log_has '^cargo build .*(-p|--package|--bin)[ =]dpds-controller( |$)'
  assert_before '^cargo build' '^dpds-controller check'
  assert_out_has "CHECK-OUTPUT-MARKER"
  assert_dir_of '^cargo build' "$REPO_ROOT"
}

test_check_never_enables_or_installs() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  run_dpds check
  assert_log_lacks '^systemctl'
  assert_log_lacks '^docker compose (up|build|down)'
  assert_no_file "$DPDS_ETC_DIR/controller.env"
  assert_no_file "$(installed_unit)"
}

test_check_missing_cargo() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  remove_stub cargo
  run_dpds check
  assert_rc_nonzero
  assert_err_has "cargo"
  assert_log_lacks '^dpds-controller check'
}

test_check_missing_rustc() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  remove_stub rustc
  run_dpds check
  assert_rc_nonzero
  assert_err_has "rustc"
  assert_log_lacks '^dpds-controller check'
}

test_check_missing_clang() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  remove_stub clang
  run_dpds check
  assert_rc_nonzero
  assert_err_has "clang"
  assert_log_lacks '^dpds-controller check'
}

test_check_missing_lld() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  remove_stub ld.lld
  run_dpds check
  assert_rc_nonzero
  assert_err_has "lld"
  assert_log_lacks '^dpds-controller check'
}

test_check_clang_llvm_major_mismatch() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  export STUB_CLANG_VERSION=22.1.0 STUB_RUSTC_LLVM=23.1.0
  run_dpds check
  assert_rc_nonzero
  assert_err_has "clang"
  assert_err_has "22"
  assert_err_has "23"
  assert_log_lacks '^dpds-controller check'
}

test_check_lld_llvm_major_mismatch() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  export STUB_LLD_VERSION=22.1.0 STUB_RUSTC_LLVM=23.1.0
  run_dpds check
  assert_rc_nonzero
  assert_err_has "lld"
  assert_err_has "22"
  assert_err_has "23"
  assert_log_lacks '^dpds-controller check'
}

test_check_only_the_llvm_major_has_to_match() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  export STUB_CLANG_VERSION=23.0.1 STUB_LLD_VERSION=23.9.9 STUB_RUSTC_LLVM=23.1.5
  run_dpds check
  assert_rc 0 "$ERR"
  assert_out_has "CHECK-OUTPUT-MARKER"
}

test_check_missing_pkg_config() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  remove_stub pkg-config
  run_dpds check
  assert_rc_nonzero
  assert_err_has "pkg-config"
  assert_log_lacks '^dpds-controller check'
}

test_check_missing_libdpdk() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  export STUB_NO_LIBDPDK=1
  run_dpds check
  assert_rc_nonzero
  assert_err_has "libdpdk"
  assert_log_lacks '^dpds-controller check'
}

test_check_works_without_docker() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  remove_stub docker
  run_dpds check
  assert_rc 0 "$ERR"
  assert_out_has "CHECK-OUTPUT-MARKER"
  assert_log_has '^dpds-controller check'
}

test_check_works_without_the_compose_plugin_and_without_a_daemon() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  export STUB_NO_COMPOSE=1 STUB_DOCKER_INFO_FAIL=1
  run_dpds check
  assert_rc 0 "$ERR"
  assert_out_has "CHECK-OUTPUT-MARKER"
  assert_log_lacks '^docker compose (build|up|down)'
}

test_check_building_the_controller_needs_no_docker() {
  remove_stub docker
  run_dpds check
  assert_rc 0 "$ERR"
  assert_log_has '^cargo build .*(-p|--package|--bin)[ =]dpds-controller( |$)'
  assert_out_has "CHECK-OUTPUT-MARKER"
}

test_install_missing_docker_builds_nothing() {
  remove_stub docker
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "docker"
  assert_log_lacks '^cargo build'
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
}

test_install_missing_compose_plugin_builds_nothing() {
  export STUB_NO_COMPOSE=1
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "compose"
  assert_log_lacks '^cargo build'
}

test_update_missing_docker_does_nothing() {
  fake_install
  remove_stub docker
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "docker"
  assert_log_lacks '^cargo build'
}

# ------------------------------------------------------------------ tests: install
test_install_builds_dpds_with_telemetry_and_controller() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^cargo build .*--release'
  assert_log_has '^cargo build .*(-p|--package|--bin)[ =]dpds( |$).*(--features|-F)[ =]telemetry|^cargo build .*(--features|-F)[ =]telemetry.*(-p|--package|--bin)[ =]dpds( |$)'
  assert_log_has '^cargo build .*(-p|--package|--bin)[ =]dpds-controller( |$)'
  # the telemetry invocation must not also select the controller (the feature belongs to dpds only)
  assert_log_lacks '^cargo build .*(-p|--package|--bin)[ =]dpds( |$).*dpds-controller'
  assert_log_lacks '^cargo build .*dpds-controller.*(-p|--package|--bin)[ =]dpds( |$)'
  assert_dir_of '^cargo build' "$REPO_ROOT"
}

test_install_runs_host_check_after_build_and_shows_output() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^dpds-controller check'
  assert_before '^cargo build' '^dpds-controller check'
  assert_out_has "CHECK-OUTPUT-MARKER"
}

test_install_host_check_failure_is_feedback_only_with_yes() {
  export STUB_CONTROLLER_RC=1
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file "$(installed_unit)"
}

test_install_installs_binaries_unit_env_state() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file_has "$DPDS_PREFIX/bin/dpds" 'STUB-BINARY-dpds'
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
  assert_file "$(installed_unit)"
  assert_file "$DPDS_ETC_DIR/controller.env"
  [[ -d "$DPDS_STATE_DIR" ]] || fail "state dir not created"
}

test_install_unit_paths_follow_prefix_and_etc() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_file_has "$(installed_unit)" "^ExecStart=$DPDS_PREFIX/bin/dpds-controller serve\$"
  assert_file_has "$(installed_unit)" "^EnvironmentFile=-$DPDS_ETC_DIR/controller.env\$"
  assert_file_has "$(installed_unit)" '^KillSignal=SIGTERM$'
  assert_file_has "$(installed_unit)" '^Restart=on-failure$'
}

test_install_env_file_default_bind_and_paths() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  local f="$DPDS_ETC_DIR/controller.env"
  assert_file_has "$f" '^DPDS_CONTROLLER_BIND="?0\.0\.0\.0:8100"?$'
  assert_file_has "$f" "^DPDS_CONTROLLER_DPDS_BIN=\"?$DPDS_PREFIX/bin/dpds\"?\$"
  assert_file_has "$f" "^DPDS_CONTROLLER_STATE_DIR=\"?$DPDS_STATE_DIR\"?\$"
}

test_install_env_file_bind_override() {
  export DPDS_BIND=127.0.0.1:9000
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_file_has "$DPDS_ETC_DIR/controller.env" '^DPDS_CONTROLLER_BIND="?127\.0\.0\.1:9000"?$'
}

test_install_does_not_overwrite_existing_env_file() {
  mkdir -p "$DPDS_ETC_DIR"
  printf 'DPDS_CONTROLLER_BIND=10.1.2.3:7777\n# my-custom-sentinel\n' >"$DPDS_ETC_DIR/controller.env"
  local before; before="$(cat "$DPDS_ETC_DIR/controller.env")"
  export DPDS_BIND=127.0.0.1:9000
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_eq "$(cat "$DPDS_ETC_DIR/controller.env")" "$before" "env file content"
}

test_install_reloads_systemd_and_builds_ui_image_in_repo_root() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl daemon-reload'
  assert_log_has '^docker compose build'
  assert_before '^dpds-controller check' '^systemctl daemon-reload'
  assert_before '^dpds-controller check' '^docker compose build'
  assert_dir_of '^docker compose build' "$REPO_ROOT"
}

test_install_does_not_enable_or_start() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_lacks '^systemctl (enable|start|restart|disable)'
  assert_log_lacks '^docker compose (up|down|start|restart)'
}

test_install_is_idempotent() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds"
  assert_file_has "$DPDS_ETC_DIR/controller.env" '^DPDS_CONTROLLER_BIND='
}

test_install_answer_n_aborts_without_installing() {
  feed $'n\n'
  run_dpds install
  assert_rc 1
  assert_log_has '^cargo build'
  assert_log_has '^dpds-controller check'
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
  assert_no_file "$DPDS_ETC_DIR/controller.env"
  assert_no_file "$DPDS_STATE_DIR"
  assert_log_lacks '^systemctl'
  assert_log_lacks '^docker compose (build|up)'
}

test_install_eof_on_prompt_aborts() {
  feed ''
  run_dpds install
  assert_rc 1
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
  assert_log_lacks '^systemctl'
}

test_install_answer_y_continues() {
  feed $'y\n'
  run_dpds install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file "$(installed_unit)"
  assert_log_has '^systemctl daemon-reload'
}

test_install_answer_yes_uppercase_continues() {
  feed $'YES\n'
  run_dpds install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

test_install_yes_flag_never_reads_stdin() {
  # stdin is /dev/null (EOF); with --yes it must still proceed
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

test_install_flag_order_is_free() {
  run_dpds --yes --nightly install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

test_install_missing_prerequisite_builds_and_installs_nothing() {
  export STUB_NO_LIBDPDK=1
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "libdpdk"
  assert_log_lacks '^cargo build'
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$(installed_unit)"
  assert_log_lacks '^systemctl'
}

test_install_clang_mismatch_builds_nothing() {
  export STUB_CLANG_VERSION=22.1.0
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "clang"
  assert_log_lacks '^cargo build'
}

test_install_build_failure_installs_nothing() {
  export STUB_CARGO_FAIL=1
  run_dpds --yes install
  assert_rc_nonzero
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$(installed_unit)"
  assert_log_lacks '^systemctl'
}

test_install_ui_image_build_failure_is_reported() {
  export STUB_DOCKER_FAIL_SUB=build
  run_dpds --yes install
  assert_rc_nonzero
}

# ------------------------------------------------------------------ tests: start/stop/restart/status
test_start_enables_service_then_brings_up_ui() {
  run_dpds start
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
  assert_log_has '^docker compose up -d'
  assert_before '^systemctl enable --now dpds-controller' '^docker compose up -d'
  assert_dir_of '^docker compose up' "$REPO_ROOT"
}

test_start_needs_no_build_tools() {
  local t
  for t in cargo rustc clang ld.lld pkg-config git curl; do remove_stub "$t"; done
  run_dpds start
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
}

test_start_systemctl_failure_propagates() {
  export STUB_SYSTEMCTL_RC=1
  run_dpds start
  assert_rc_nonzero
}

test_start_compose_failure_propagates() {
  export STUB_DOCKER_FAIL_SUB=up
  run_dpds start
  assert_rc_nonzero
}

test_stop_disables_service_and_brings_ui_down() {
  run_dpds stop
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl disable --now dpds-controller$'
  assert_log_has '^docker compose down'
  assert_dir_of '^docker compose down' "$REPO_ROOT"
}

test_stop_compose_down_runs_even_if_systemctl_fails() {
  export STUB_SYSTEMCTL_FAIL_SUB=disable
  run_dpds stop
  assert_log_has '^docker compose down'
}

test_stop_systemctl_failure_propagates() {
  export STUB_SYSTEMCTL_FAIL_SUB=disable
  run_dpds stop
  assert_rc_nonzero
}

test_restart_is_stop_then_start() {
  run_dpds restart
  assert_rc 0 "$ERR"
  assert_before '^systemctl disable --now dpds-controller' '^systemctl enable --now dpds-controller'
  assert_before '^docker compose down' '^docker compose up -d'
  assert_before '^systemctl disable --now dpds-controller' '^docker compose up -d'
}

test_status_runs_all_three_probes() {
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl .*status.* dpds-controller'
  assert_log_has '^docker compose ps'
  assert_log_has '^curl .*http://127\.0\.0\.1:8100/api/online'
  assert_out_has "STUB-SYSTEMCTL-STATUS"
  assert_out_has "STUB-COMPOSE-PS"
  assert_out_has "stub-online"
  assert_dir_of '^docker compose ps' "$REPO_ROOT"
}

test_status_uses_port_from_env_file() {
  mkdir -p "$DPDS_ETC_DIR"
  printf 'DPDS_CONTROLLER_BIND=0.0.0.0:9123\n' >"$DPDS_ETC_DIR/controller.env"
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^curl .*http://127\.0\.0\.1:9123/api/online'
}

test_status_exit_zero_when_everything_is_down() {
  export STUB_SYSTEMCTL_RC=3 STUB_CURL_RC=7
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl .*status'
  assert_log_has '^docker compose ps'
  assert_log_has '^curl '
}

# ------------------------------------------------------------------ tests: update
test_update_pulls_nightly_rebuilds_and_reinstalls() {
  fake_install
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^git fetch'
  assert_log_has '^git checkout nightly$'
  assert_log_has '^git pull --ff-only$'
  assert_log_lacks '^git pull$'
  assert_before '^git fetch' '^git checkout nightly'
  assert_before '^git checkout nightly' '^git pull --ff-only'
  assert_before '^git pull --ff-only' '^cargo build'
  assert_dir_of '^git (fetch|checkout|pull)' "$REPO_ROOT"
  assert_log_has '^cargo build .*(-p|--package|--bin)[ =]dpds-controller( |$)'
  assert_log_has '^systemctl daemon-reload'
  assert_log_has '^docker compose build'
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
  assert_file_has "$DPDS_PREFIX/bin/dpds" 'STUB-BINARY-dpds'
}

test_update_restarts_when_it_was_active() {
  fake_install
  export STUB_ACTIVE=1
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl is-active.* dpds-controller'
  assert_before '^cargo build' '^systemctl enable --now dpds-controller'
  assert_log_has '^systemctl disable --now dpds-controller$'
  assert_log_has '^docker compose up -d'
}

test_update_does_not_start_when_it_was_inactive() {
  fake_install
  export STUB_ACTIVE=0
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl is-active.* dpds-controller'
  assert_log_lacks '^systemctl (enable|start|restart)'
  assert_log_lacks '^docker compose up'
}

test_update_git_failure_aborts_before_build() {
  fake_install
  export STUB_GIT_FAIL_SUB=pull
  run_dpds --yes update
  assert_rc_nonzero
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|restart)'
}

test_update_prerequisites_are_checked() {
  fake_install
  remove_stub cargo
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "cargo"
  assert_log_lacks '^systemctl (enable|restart)'
}

# ------------------------------------------------------------------ tests: spec v7 P1 (frontend proxy target)
write_env_bind() { mkdir -p "$DPDS_ETC_DIR"; printf 'DPDS_CONTROLLER_BIND=%s\n' "$1" >"$DPDS_ETC_DIR/controller.env"; }

test_start_exports_the_controller_url_from_the_env_file_port() {
  write_env_bind 0.0.0.0:9123
  run_dpds start
  assert_rc 0 "$ERR"
  assert_env_has '^docker compose up .*DPDS_CONTROLLER=http://127\.0\.0\.1:9123( |$)'
}

test_start_controller_url_with_a_loopback_bind_and_quoted_value() {
  write_env_bind '"127.0.0.1:9456"'
  run_dpds start
  assert_rc 0 "$ERR"
  assert_env_has '^docker compose up .*DPDS_CONTROLLER=http://127\.0\.0\.1:9456( |$)'
}

test_start_controller_url_defaults_to_port_8100_without_an_env_file() {
  run_dpds start
  assert_rc 0 "$ERR"
  assert_env_has '^docker compose up .*DPDS_CONTROLLER=http://127\.0\.0\.1:8100( |$)'
}

test_restart_exports_the_controller_url_too() {
  write_env_bind 0.0.0.0:9123
  run_dpds restart
  assert_rc 0 "$ERR"
  assert_env_has '^docker compose up .*DPDS_CONTROLLER=http://127\.0\.0\.1:9123( |$)'
}

test_install_exports_the_controller_url_to_the_compose_build() {
  write_env_bind 0.0.0.0:9123
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_env_has '^docker compose build .*DPDS_CONTROLLER=http://127\.0\.0\.1:9123( |$)'
}

test_update_restart_exports_the_controller_url() {
  fake_install
  write_env_bind 0.0.0.0:9123
  export STUB_ACTIVE=1
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_env_has '^docker compose build .*DPDS_CONTROLLER=http://127\.0\.0\.1:9123( |$)'
  assert_env_has '^docker compose up .*DPDS_CONTROLLER=http://127\.0\.0\.1:9123( |$)'
}

test_frontend_bind_and_port_from_the_callers_environment_reach_compose() {
  export DPDS_FRONTEND_BIND=127.0.0.1 DPDS_FRONTEND_PORT=9090
  run_dpds start
  assert_rc 0 "$ERR"
  assert_env_has '^docker compose up .*DPDS_FRONTEND_BIND=127\.0\.0\.1 DPDS_FRONTEND_PORT=9090$'
}

test_status_probes_the_configured_bind_address() {
  write_env_bind 10.0.0.5:9123
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^curl .*http://10\.0\.0\.5:9123/api/online'
  assert_log_lacks '^curl .*127\.0\.0\.1'
}

test_status_probes_loopback_for_a_wildcard_bind() {
  write_env_bind 0.0.0.0:9123
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^curl .*http://127\.0\.0\.1:9123/api/online'
  assert_log_lacks '^curl .*0\.0\.0\.0'
}

test_status_probes_a_loopback_bind_as_is() {
  write_env_bind 127.0.0.1:9000
  run_dpds status
  assert_log_has '^curl .*http://127\.0\.0\.1:9000/api/online'
}

# ------------------------------------------------------------------ tests: spec v7 P2 (update safety, failed stop)
test_update_refuses_a_dirty_working_tree_before_anything_else() {
  fake_install
  export STUB_ACTIVE=1 STUB_GIT_DIRTY=1
  run_dpds --yes update
  assert_rc_nonzero
  grep -Eiq 'uncommitted|dirty|working tree|local changes' <<<"$ERR" || fail "stderr does not explain the dirty tree: $ERR"
  assert_log_lacks '^cargo build'
  assert_log_lacks '^git (checkout|pull)'
  assert_log_lacks '^systemctl (enable|disable)'
  assert_log_lacks '^docker compose (build|up|down)'
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'dpds-controller'
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" || fail "binary was replaced"
}

test_update_with_a_clean_tree_is_not_blocked() {
  fake_install
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^git pull --ff-only$'
}

test_update_warns_when_switching_from_another_branch() {
  fake_install
  export STUB_GIT_BRANCH=feature-x
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_all_has "feature-x"
  assert_all_has "nightly"
  grep -Ei 'warn' <<<"$OUT$ERR" | grep -qF 'feature-x' || fail "no WARN line names the branch feature-x"
  assert_log_has '^git checkout nightly$'
}

test_update_warns_about_a_detached_head() {
  fake_install
  export STUB_GIT_BRANCH=HEAD
  run_dpds --yes update
  assert_rc 0 "$ERR"
  grep -Ei 'warn' <<<"$OUT$ERR" | grep -qi 'detached' || fail "no WARN line mentions a detached HEAD"
  assert_log_has '^git checkout nightly$'
}

test_update_on_nightly_gives_no_branch_warning() {
  fake_install
  run_dpds --yes update
  assert_rc 0 "$ERR"
  ! grep -Ei 'warn' <<<"$OUT$ERR" | grep -Eqi 'detached|branch' || fail "unexpected branch warning: $(grep -Ei warn <<<"$OUT$ERR")"
}

test_update_pulls_fast_forward_only() {
  fake_install
  run_dpds --yes update
  assert_log_has '^git pull --ff-only$'
}

test_restart_still_starts_when_the_service_stop_fails() {
  export STUB_SYSTEMCTL_FAIL_SUB=disable
  run_dpds restart
  assert_rc_nonzero
  grep -Eqi 'stop' <<<"$ERR" || fail "the failed stop is not reported: $ERR"
  assert_log_has '^docker compose down'
  assert_log_has '^systemctl enable --now dpds-controller$'
  assert_log_has '^docker compose up -d'
  assert_before '^systemctl disable --now dpds-controller' '^systemctl enable --now dpds-controller'
}

test_restart_still_starts_when_the_compose_down_fails() {
  export STUB_DOCKER_FAIL_SUB=down
  run_dpds restart
  assert_rc_nonzero
  grep -Eqi 'stop|down' <<<"$ERR" || fail "the failed stop is not reported: $ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
  assert_log_has '^docker compose up -d'
}

test_update_restart_still_starts_when_the_stop_fails() {
  fake_install
  export STUB_ACTIVE=1 STUB_SYSTEMCTL_FAIL_SUB=disable
  run_dpds --yes update
  assert_rc_nonzero
  grep -Eqi 'stop' <<<"$ERR" || fail "the failed stop is not reported: $ERR"
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
  assert_log_has '^systemctl enable --now dpds-controller$'
  assert_log_has '^docker compose up -d'
}

# ------------------------------------------------------------------ tests: spec v7 P3 (prerequisites, host check)
test_install_with_an_unreachable_docker_daemon_builds_nothing() {
  export STUB_DOCKER_INFO_FAIL=1
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "docker"
  assert_err_has "daemon"
  assert_log_has '^docker info'
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl'
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
}

test_update_with_an_unreachable_docker_daemon_does_nothing() {
  fake_install
  export STUB_ACTIVE=1 STUB_DOCKER_INFO_FAIL=1
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "daemon"
  assert_log_lacks '^cargo build'
  assert_log_lacks '^git (checkout|pull)'
  assert_log_lacks '^systemctl (enable|disable)'
}

test_docker_info_is_checked_before_the_first_build() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_before '^docker info' '^cargo build'
}

test_start_and_stop_do_not_need_a_reachable_daemon_probe() {
  export STUB_DOCKER_INFO_FAIL=1
  run_dpds start
  assert_rc 0 "$ERR"
  assert_log_lacks '^docker info'
}

test_install_runs_the_host_check_through_the_privileged_runner() {
  if [[ "$(id -u)" == 0 ]]; then echo "    (skipped: running as root, sudo is not used)"; return; fi
  run_dpds --yes install
  assert_rc 0 "$ERR"
  grep -Eq 'dpds-controller check' "$STUB_LOG.sudo" || fail "host check did not go through sudo: $(tr '\n' '|' <"$STUB_LOG.sudo")"
  assert_out_has "CHECK-OUTPUT-MARKER"
}

test_the_standalone_check_command_keeps_running_as_the_caller() {
  # `check` is the quick read-only report; only the install-time check is privileged
  if [[ "$(id -u)" == 0 ]]; then echo "    (skipped: running as root)"; return; fi
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  run_dpds check
  assert_rc 0 "$ERR"
  ! grep -q 'dpds-controller check' "$STUB_LOG.sudo" || fail "check unexpectedly ran through sudo"
}

# ------------------------------------------------------------------ tests: uninstall
test_uninstall_removes_binaries_and_unit_keeps_state_and_env() {
  fake_install
  local env_before state_before
  env_before="$(cat "$DPDS_ETC_DIR/controller.env")"
  state_before="$(cat "$DPDS_STATE_DIR/links.json")"
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
  assert_eq "$(cat "$DPDS_ETC_DIR/controller.env")" "$env_before" "env file kept"
  assert_eq "$(cat "$DPDS_STATE_DIR/links.json")" "$state_before" "state kept"
}

test_uninstall_stops_service_and_reloads_systemd() {
  fake_install
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl disable --now dpds-controller$'
  assert_log_has '^docker compose down'
  assert_log_has '^systemctl daemon-reload'
  assert_before '^systemctl disable --now dpds-controller' '^systemctl daemon-reload'
}

test_uninstall_tolerates_service_not_installed() {
  fake_install
  export STUB_SYSTEMCTL_FAIL_SUB=disable
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
}

test_uninstall_when_nothing_installed_succeeds() {
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
}

test_install_then_uninstall_roundtrip_keeps_state_and_env() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  echo keep >"$DPDS_STATE_DIR/marker"
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$(installed_unit)"
  assert_file "$DPDS_ETC_DIR/controller.env"
  assert_file "$DPDS_STATE_DIR/marker"
}

# ------------------------------------------------------------------ tests: no tuning
test_no_tuning_tool_is_ever_called() {
  local c
  fake_install
  for c in check install update start stop restart status uninstall; do
    run_dpds --yes "$c"
  done
  for c in $TUNING_TOOLS; do assert_log_lacks "^$c( |\$)"; done
  ! grep -Eq '/proc/|/sys/|nr_hugepages|hugepages' "$STUB_LOG.sudo" || fail "privileged command touches /proc, /sys or hugepages: $(grep -E '/proc/|/sys/|hugepages' "$STUB_LOG.sudo" | head -3 | tr '\n' '|')"
}

test_script_source_contains_no_tuning_commands() {
  local pat
  for pat in 'ethtool' 'setpci' 'sysctl' 'mlxconfig' 'mlxfwreset' 'cpupower' 'nr_hugepages' 'hugepages-' 'isolcpus' 'tuned-adm' '/proc/sys' 'update-grub' 'grubby'; do
    # comments may talk about them (e.g. "never runs ethtool"); executable lines may not
    ! grep -v '^[[:space:]]*#' "$SCRIPT" | grep -Eq -- "$pat" || fail "dpds.sh has a non-comment line mentioning '$pat'"
  done
}

# ------------------------------------------------------------------ tests: unit file
unit_value() { grep -E "^$1=" "$UNIT" | tail -1 | cut -d= -f2-; }

test_unit_exists() { assert_file "$UNIT"; }

test_unit_exec_start_serves_installed_binary() {
  assert_eq "$(unit_value ExecStart)" "/usr/local/bin/dpds-controller serve" "ExecStart"
}

test_unit_environment_file_is_optional_etc_dpds() {
  assert_eq "$(unit_value EnvironmentFile)" "-/etc/dpds/controller.env" "EnvironmentFile"
}

test_unit_kill_signal_and_restart_policy() {
  assert_eq "$(unit_value KillSignal)" "SIGTERM" "KillSignal"
  assert_eq "$(unit_value Restart)" "on-failure" "Restart"
}

test_unit_runs_as_root() {
  local u; u="$(unit_value User)"
  [[ -z "$u" || "$u" == root ]] || fail "unit must run as root, User=$u"
}

test_unit_timeout_stop_at_least_120s() {
  local v secs; v="$(unit_value TimeoutStopSec)"
  [[ -n "$v" ]] || { fail "TimeoutStopSec not set"; return; }
  case "$v" in
    infinity) secs=999999;;
    *min) secs=$(( ${v%min} * 60 ));;
    *s)   secs=${v%s};;
    *[!0-9]*) fail "cannot parse TimeoutStopSec=$v"; return;;
    *)    secs=$v;;
  esac
  (( secs >= 120 )) || fail "TimeoutStopSec=$v is shorter than 120 s"
}

test_unit_install_section_wanted_by_multi_user() {
  assert_file_has "$UNIT" '^\[Install\]'
  assert_eq "$(unit_value WantedBy)" "multi-user.target" "WantedBy"
}

test_unit_has_no_tuning_commands() {
  ! grep -Eiq 'ethtool|setpci|sysctl|mlxconfig|hugepages|cpupower|ExecStartPre' "$UNIT" || fail "unit contains tuning/pre-start commands"
}

test_unit_passes_systemd_analyze_verify() {
  command -v systemd-analyze >/dev/null || { echo "    (skipped: systemd-analyze not installed)"; return; }
  # verify needs an existing, executable ExecStart target; point the copy at /bin/true.
  local copy="$T/dpds-controller.service" real_true; real_true="$(command -v true)"
  sed "s#^ExecStart=/usr/local/bin/dpds-controller#ExecStart=$real_true#" "$UNIT" >"$copy"
  local out; out="$(systemd-analyze verify "$copy" 2>&1)"
  if grep -Eq 'Failed to parse|Unknown (key|section)|Unknown lvalue|ignoring|Invalid|not executable|Failed to load' <<<"$out"; then
    fail "systemd-analyze verify complained: $(tr '\n' '|' <<<"$out")"
  fi
}

# ------------------------------------------------------------------ runner
run_one() {
  local name="$1"
  (
    set +e
    setup_sandbox
    "$name"
    exit "$T_FAILED"
  )
}

main() {
  local filters=("$@") names n f want
  mapfile -t names < <(declare -F | awk '{print $3}' | grep '^test_' | sort)
  for n in "${names[@]}"; do
    want=1
    if (( ${#filters[@]} )); then
      want=0
      for f in "${filters[@]}"; do [[ "$n" == *"$f"* ]] && want=1; done
    fi
    (( want )) || continue
    echo "RUN  $n"
    if run_one "$n"; then
      PASS=$((PASS + 1)); echo "ok   $n"
    else
      FAIL=$((FAIL + 1)); FAILED_NAMES+=("$n"); echo "FAIL $n"
    fi
  done
  echo
  echo "passed: $PASS  failed: $FAIL"
  if (( FAIL )); then printf 'failed tests:\n'; printf '  %s\n' "${FAILED_NAMES[@]}"; return 1; fi
  return 0
}

main "$@"
