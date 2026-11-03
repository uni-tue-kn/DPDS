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
#     "Usage" and every command name) on stdout or stderr, nothing else is executed. The usage text
#     (also for --help, exit 0) mentions no docker, compose, container or image, and (spec v15) no
#     download, CI-built frontend, release asset or DPDS_FRONTEND_DIST; it mentions Node or npm
#     (install and update build the frontend on the host).
#   - --nightly is the default channel and is accepted explicitly.
#   - --stable (spec v18): install and update use the latest release tag instead of the nightly
#     branch. After the prerequisites, `git fetch --tags`, then the highest tag matching v* by version
#     (`git tag --list 'v*' --sort=-v:refname`, first line; v1.10.0 beats v1.9.0, tags such as x2.0.0 or
#     nightly-build are not releases), then `git checkout <tag>` (a detached HEAD is fine), all in the
#     repository root and before the frontend build and any cargo build. No `git checkout nightly`, no
#     `git pull`, no "switching to nightly" warning. update keeps its other rules: a dirty tree is
#     refused before any git change, the service is restarted only if it was active. A failing
#     `git fetch` or `git checkout` aborts before any build. With no v* tag: error on stderr that says
#     "release" and "tag", exit non-zero, no checkout, no build, no npm, no systemctl change.
#     The other commands do not touch git with --stable. --nightly update/install are unchanged (install
#     makes no git call at all, update checks out nightly) even when v* tags exist.
#     The git stub answers `git tag` from STUB_GIT_TAGS (space separated); without --sort it lists
#     them alphabetically, with --sort=-v:refname by version, highest first.
#   - `status` (spec v18) prints the installed version taken from the /api/online answer ("version"),
#     on a line of its own besides the raw answer, also when the key is not the first one. Nothing
#     reads the version from jq (not on the PATH) or from the repository.
#   - --nightly together with --stable: exit non-zero, nothing is executed.
#   - Errors go to stderr. The script may be started from any working directory: it works on the
#     repository it lives in (cargo, npm and git run there; npm in its frontend/ directory).
#   - No command ever calls docker (the stub is a tripwire, see below), and the script has no
#     docker, compose or nginx code, no DPDS_FRONTEND_BIND/DPDS_FRONTEND_PORT and no DPDS_CONTROLLER
#     setting (DPDS_CONTROLLER_BIND of the env file stays).
#
# Environment overrides (so tests never touch the real system):
#   DPDS_PREFIX       install prefix; binaries go to $DPDS_PREFIX/bin     (default /usr/local)
#   DPDS_ETC_DIR      config dir; env file is $DPDS_ETC_DIR/controller.env (default /etc/dpds)
#   DPDS_STATE_DIR    controller state dir                                (default /var/lib/dpds-controller)
#   DPDS_SYSTEMD_DIR  where the unit is installed                         (default /etc/systemd/system)
#   DPDS_BIND         bind address written into a NEW env file            (default 0.0.0.0:8100)
#   CARGO_TARGET_DIR  standard cargo variable; built binaries are read from
#                     $CARGO_TARGET_DIR/release/{dpds,dpds-controller}    (default <repo>/target)
#   There is no DPDS_FRONTEND_ASSET_URL any more, and a DPDS_FRONTEND_DIST in the caller's
#   environment is ignored by install and update (spec v15; it stays a way to feed a plain
#   `cargo build` by hand, which is not dpds.sh's business).
#   Privileged operations go through sudo when EUID != 0 (like p4tg.sh's run_privileged). Tests
#   do not require that sudo is used, only that the effects happen.
#
# Prerequisites (spec v15; checked by `check`, `install`, `update`; NOT by start/stop/restart/
# status/uninstall, which only need systemctl and, for status, curl). The script never installs
# anything. Every unmet prerequisite is reported as an ERROR line on stderr that names the tool, says
# what to install ("install" in the line, for a missing tool) and, for a version, the required version;
# ALL unmet prerequisites are reported, not only the first (one run, several ERROR lines), then the
# command exits non-zero. Met ones are reported as OK lines (stdout or stderr).
#   cargo, rustc                         on PATH; rustc's version is part of its OK line, and a
#                                        rustc below the `rust-version` the workspace declares
#                                        (crates/dpds-core/Cargo.toml) is an error naming rustc
#   clang and ld.lld                     whose LLVM major equals the one reported by
#                                        `rustc -vV` ("LLVM version: N.x.y"); the error names the
#                                        tool and both majors. Minor/patch/vendor prefix ignored.
#   pkg-config with libdpdk              `pkg-config --exists libdpdk`; error names "libdpdk".
#                                        Version >= 26.07 (`pkg-config --atleast-version=26.07
#                                        libdpdk`); an older one is an error naming libdpdk, the
#                                        required 26.07 and the found version.
#   node, npm                            on PATH; node >= the minimum of frontend/package.json
#                                        "engines.node" (">=22.22" today; compared numerically
#                                        per component, "9" < "22" < "100"); an older node is an
#                                        error naming node and the required version (22.22).
#   curl, sha256sum, tar                 NOT prerequisites of anything but (curl) `status`.
#   No system packages are ever installed. Docker is not a prerequisite of anything.
#   install and update run all of this before any git call, cargo build, npm call or systemctl call.
#
# check
#   Prerequisites, then runs `dpds-controller check` and passes its stdout and exit code through (when
#   a prerequisite is unmet, `dpds-controller check` is not run).
#   Binary used: $DPDS_PREFIX/bin/dpds-controller if executable; otherwise the one in
#   $CARGO_TARGET_DIR/release if present; otherwise it first runs
#   `cargo build --release -p dpds-controller` (the build is in the repo root) and uses that. That
#   build runs no npm and downloads nothing (no curl, no `git rev-parse HEAD:frontend`).
#
# frontend (install and update; spec v15, replaces the CI release asset of spec v13). The controller
#   embeds the web frontend at compile time. install and update build it from the checked-out tree
#   (for update after checkout/pull, so the pulled sources are built, local changes included, no
#   warning about them), before ANY cargo build:
#   - `npm ci`, then `npm run build`, both with the working directory <repo>/frontend,
#   - `<repo>/frontend/dist/index.html` must exist afterwards, else an error naming index.html and
#     the dist directory,
#   - the controller (cargo build ... dpds-controller, its own invocation) is built with
#     DPDS_FRONTEND_DIST=<repo>/frontend/dist, whatever the caller's DPDS_FRONTEND_DIST is,
#   - a stale frontend/dist is never embedded: it is removed after `npm ci` succeeded, right before
#     `npm run build` (so a failing `npm ci` leaves the previous dist in place, while a build that
#     fails or succeeds without writing an index.html leaves none: error, nothing built or installed),
#   - a failing `npm ci`, a failing `npm run build` or a missing index.html stop with a non-zero exit
#     and an error on stderr before any cargo build, install step or systemctl call (service untouched,
#     installed binaries not replaced),
#   - no curl, no tar, no sha256sum, no download, and install makes no git call at all.
#
# install  (order: prerequisites, frontend build, cargo builds, host check, confirmation, installation)
#   1. builds the frontend (above), then
#      `cargo build --release ... dpds ... --features telemetry` (one invocation naming only dpds)
#      and `cargo build --release ... dpds-controller` (a separate invocation, with the frontend dist).
#   2. runs `dpds-controller check` on the freshly built controller, through the privileged runner
#      (sudo when EUID != 0) so root-only hints work; its output is shown. Its exit
#      code is feedback only and never aborts the install (hints are not errors for installing).
#   3. unless --yes: asks "continue?" on stdin. y/yes (any case) continues; anything else
#      including EOF aborts: exit 1, nothing installed, no daemon-reload.
#   4. installs dpds and dpds-controller (executable) into $DPDS_PREFIX/bin; the unit from
#      packaging/dpds-controller.service into $DPDS_SYSTEMD_DIR/dpds-controller.service with
#      `ExecStart=$DPDS_PREFIX/bin/dpds-controller serve` and
#      `EnvironmentFile=-$DPDS_ETC_DIR/controller.env` substituted for the defaults;
#      creates $DPDS_ETC_DIR/controller.env ONLY if absent (never overwritten) containing
#        DPDS_CONTROLLER_BIND=<DPDS_BIND or 0.0.0.0:8100>
#        DPDS_CONTROLLER_DPDS_BIN=$DPDS_PREFIX/bin/dpds
#        DPDS_CONTROLLER_STATE_DIR=$DPDS_STATE_DIR
#      (values may be double-quoted); creates $DPDS_STATE_DIR.
#   5. `systemctl daemon-reload`.
#   6. Does NOT enable or start anything (no `systemctl enable|start`).
#   7. Re-running install is fine (idempotent). Failure of a build step: exit non-zero.
#
# start    `systemctl enable --now dpds-controller`, nothing else.
# stop     `systemctl disable --now dpds-controller`, nothing else.
# restart  stop followed by start. When the stop fails, start is still attempted (a running
#          service must not be left stopped) and the failure is reported: exit non-zero.
#   A failing systemctl call makes the command exit non-zero.
# status   `systemctl status dpds-controller` and
#          `curl ... http://<host>:<port>/api/online` where host:port is the bind address in
#          $DPDS_ETC_DIR/controller.env (default 127.0.0.1:8100; 0.0.0.0 is probed as 127.0.0.1).
#          Always runs both, prints their output, exits 0 even if the service is inactive or the API
#          is unreachable.
# update   prerequisites first; `systemctl is-active dpds-controller` is queried; a dirty working tree is
#          refused (error, non-zero, before any checkout/pull/build); a warning names the current branch
#          (or "detached") when it is not nightly; then `git fetch`, `git checkout nightly`,
#          `git pull --ff-only` (in the repo root; a failing git call aborts before building);
#          then the frontend build for the pulled tree and the install flow (same prompt unless
#          --yes); then, only if the service was active, restart (= stop + start). If it was not
#          active nothing is started.
# uninstall (needs --yes semantic only for install-like prompts; none here)
#          `systemctl disable --now dpds-controller` (failure tolerated),
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
#   LimitMEMLOCK=infinity in [Service] (spec v17; dpds locks its memory), Restart=on-failure, [Install] WantedBy=multi-user.target; parses without warnings under
#   `systemd-analyze verify`.
#
# Sandbox conventions used by this file (PATH contains only stubs + whitelisted base utilities):
#   Every test runs a COPY of dpds.sh in a sandbox repository ($T/repo, with the unit file,
#   frontend/package.json, Cargo.toml and crates/dpds-core/Cargo.toml copied from the real tree), so
#   the stub npm can write frontend/dist without touching the real repository.
#   Every stub appends "<tool> <argv>" to $STUB_LOG ("sudo" is transparent and only logged to
#   $STUB_LOG.sudo); "<tool> <argv> @ <dir>" with the effective directory goes to $STUB_LOG.dirs.
#   `docker` is a tripwire: it fails and logs to $STUB_LOG.docker, and every run_dpds checks that
#   log, so no test can pass if a command calls docker. The cargo stub logs the frontend dist it was
#   given to $STUB_LOG.dist (with the content of its index.html).
#   node: `node --version` prints v$STUB_NODE_VERSION (default 22.22.1). npm: `npm --version`, and
#   `npm ci` / `npm run build`; STUB_NPM_FAIL_SUB=ci|run makes that subcommand fail (exit 1);
#   `npm run build` writes dist/index.html (<html>BUILT-FRONTEND</html>) and dist/assets/app.js in its
#   working directory (after emptying dist/, like vite), except with STUB_NPM_NO_INDEX=1 (dist/ without
#   index.html) and STUB_NPM_BUILD_NOOP=1 (the build succeeds and leaves dist/ exactly as it was). rustc: STUB_RUSTC_VERSION
#   (default 1.99.1). pkg-config: STUB_DPDK_VERSION (default 26.07.0) answers --modversion and
#   --atleast-version; STUB_NO_LIBDPDK makes libdpdk unavailable.
#   curl is only answered for `status` (/api/online); any other URL fails with exit 22. tar and
#   sha256sum are tripwires that log and fail: install and update must never call them.
# =====================================================================================

set -u

HERE="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# SRC_ROOT is the real repository (read-only for the tests); REPO_ROOT is the sandbox copy that
# setup_sandbox makes and the script runs in.
SRC_ROOT="$(cd -P -- "$HERE/../.." && pwd)"
REPO_ROOT="$SRC_ROOT"
SCRIPT="$SRC_ROOT/dpds.sh"
UNIT="$SRC_ROOT/packaging/dpds-controller.service"

BASE_UTILS="bash sh env timeout cat sed grep awk date mkdir cp install rm rmdir dirname basename readlink tee tr
  head tail sort uniq cut chmod chown ln mv id uname wc sleep touch find xargs stat realpath mktemp ls
  true false cmp expr hostname whoami diff printf test tty"
# Any call to one of these is a tuning attempt (tripwires).
TUNING_TOOLS="ethtool setpci sysctl mlxconfig mlxfwreset cpupower tuned-adm grubby update-grub modprobe insmod"
# The tools every stub run provides.
STUB_TOOLS="sudo systemctl docker cargo git curl pkg-config clang ld.lld rustc node npm"

PASS=0
FAIL=0
FAILED_NAMES=()
ROOT_TMP="$(mktemp -d)"
trap 'rm -rf "$ROOT_TMP"' EXIT

# ------------------------------------------------------------------ assertions
# Inside a test (subshell) a failed assertion records the failure and the test continues.
T_FAILED=0
fail() { echo "    FAIL: $*${CTX:+ [$CTX]}"; T_FAILED=1; }
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
# assert_dist_has REGEX: a cargo call logged with the frontend dist it saw matches.
assert_dist_has() { grep -Eq -- "$1" "$STUB_LOG.dist" 2>/dev/null || fail "cargo dist log lacks /$1/: $(tr '\n' '|' <"$STUB_LOG.dist" 2>/dev/null)"; }
# assert_err_word NAME: stderr names NAME as a word (not as part of another word).
assert_err_word() { grep -Eiq -- "(^|[^[:alnum:]_.-])$1([^[:alnum:]_.-]|\$)" <<<"$ERR" || fail "stderr does not name '$1': $ERR"; }
# Temp dirs of the script (download, unpack) must be gone when it exits (TMPDIR is $T/tmp).
assert_no_temp_leftovers() { [[ -z "$(ls -A "$T/tmp" 2>/dev/null)" ]] || fail "temp files left behind: $(ls -A "$T/tmp" | tr '\n' ' ')"; }
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
  : >"$STUB_LOG"; : >"$STUB_LOG.dirs"; : >"$STUB_LOG.sudo"; : >"$STUB_LOG.dist"; : >"$STUB_LOG.docker"
  unset STUB_ACTIVE STUB_NO_LIBDPDK STUB_RUSTC_LLVM STUB_CLANG_VERSION STUB_LLD_VERSION \
        STUB_CONTROLLER_RC STUB_CARGO_FAIL STUB_SYSTEMCTL_RC STUB_SYSTEMCTL_FAIL_SUB \
        STUB_GIT_FAIL_SUB STUB_CURL_RC DPDS_BIND STUB_GIT_DIRTY STUB_GIT_DIRTY_FRONTEND STUB_GIT_BRANCH \
        STUB_GIT_TAGS STUB_ONLINE_JSON STUB_NODE_VERSION STUB_NPM_FAIL_SUB STUB_NPM_NO_INDEX STUB_NPM_BUILD_NOOP STUB_DPDK_VERSION STUB_RUSTC_VERSION \
        DPDS_FRONTEND_DIST DPDS_FRONTEND_ASSET_URL
  mkdir -p "$T/tmp"
  export DPDS_PREFIX="$T/prefix" DPDS_ETC_DIR="$T/etc" DPDS_STATE_DIR="$T/state" DPDS_SYSTEMD_DIR="$T/systemd"
  export CARGO_TARGET_DIR="$T/target"
  export HOME="$T/home"
  mkdir -p "$DPDS_PREFIX/bin" "$DPDS_SYSTEMD_DIR"

  # The sandbox repository: a copy of the script and of the few files it reads.
  mkdir -p "$T/repo/packaging" "$T/repo/frontend" "$T/repo/crates/dpds-core"
  cp "$SRC_ROOT/dpds.sh" "$T/repo/dpds.sh"
  cp "$SRC_ROOT/packaging/dpds-controller.service" "$T/repo/packaging/"
  cp "$SRC_ROOT/frontend/package.json" "$T/repo/frontend/"
  cp "$SRC_ROOT/Cargo.toml" "$T/repo/Cargo.toml"
  cp "$SRC_ROOT/crates/dpds-core/Cargo.toml" "$T/repo/crates/dpds-core/"
  REPO_ROOT="$(cd -P "$T/repo" && pwd)"
  SCRIPT="$REPO_ROOT/dpds.sh"

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
  # tar and sha256sum are tripwires: install and update neither download nor unpack anything.
  for u in tar sha256sum; do
    make_stub "$u" <<EOF
echo "$u \$*" >> "\$LOG"
echo "$u: tripwire - dpds.sh must not use $u (spec v15)" >&2
exit 1
EOF
  done
  make_stub docker <<'EOF'
echo "docker $*" >> "$LOG"; echo "docker $*" >> "$LOG.docker"
echo "docker: tripwire - dpds.sh must not call docker (spec v13)" >&2
exit 1
EOF
  make_stub git <<'EOF'
dir=$PWD
if [ "${1:-}" = -C ]; then dir=$(cd "$2" 2>/dev/null && pwd -P); shift 2; fi
echo "git $*" >> "$LOG"; echo "git $* @ $dir" >> "$LOG.dirs"
# current branch: STUB_GIT_BRANCH (default nightly); "HEAD" means a detached HEAD
br="${STUB_GIT_BRANCH:-nightly}"
[ -n "${STUB_GIT_FAIL_SUB:-}" ] && [ "${1:-}" = "$STUB_GIT_FAIL_SUB" ] && { echo "fatal: stub git failure" >&2; exit 1; }
case "${1:-}" in
  rev-parse)
    case "$*" in
      *) echo "$br";;
    esac;;
  branch) [ "$br" = HEAD ] || echo "$br";;
  tag)
    # STUB_GIT_TAGS: space separated. Without --sort git lists alphabetically; --sort=-v:refname is by version, highest first.
    sortkey=""; pat=""
    for a in "${@:2}"; do case "$a" in --sort=*) sortkey=${a#--sort=};; -*) ;; *) [ -z "$pat" ] && pat=$a;; esac; done
    for t in ${STUB_GIT_TAGS:-}; do
      # shellcheck disable=SC2053
      if [ -z "$pat" ] || [[ "$t" == $pat ]]; then echo "$t"; fi
    done | case "$sortkey" in
      -v:refname) sort -rV;;
      v:refname) sort -V;;
      *) LC_ALL=C sort;;
    esac;;
  symbolic-ref) if [ "$br" = HEAD ]; then echo "fatal: ref HEAD is not a symbolic ref" >&2; exit 128; fi; echo "$br";;
  status)
    if [ -n "${STUB_GIT_DIRTY:-}" ]; then echo " M dpds.sh"
    elif [ -n "${STUB_GIT_DIRTY_FRONTEND:-}" ]; then
      case " $* " in *" frontend "*) echo " M frontend/src/App.tsx";; esac
    fi;;
  diff|diff-index) [ -n "${STUB_GIT_DIRTY:-}" ] && exit 1;;
esac
exit 0
EOF
  # curl: only `status` uses it, for /api/online. STUB_CURL_RC makes every call fail with that code.
  make_stub curl <<'EOF'
echo "curl $*" >> "$LOG"
url=""
for a in "$@"; do case $a in http*|file*) url=$a;; esac; done
if [ "${STUB_CURL_RC:-0}" != 0 ]; then echo "curl: (${STUB_CURL_RC}) stub failure" >&2; exit "${STUB_CURL_RC}"; fi
case "$url" in
  */api/online)
    json="${STUB_ONLINE_JSON:-}"
    [ -n "$json" ] || json='{"version":"7.8.9","dpds_bin":"/usr/local/bin/dpds","telemetry":true,"root":true}'
    echo "$json"; exit 0;;
esac
echo "curl: (22) The requested URL returned error: 404" >&2
exit 22
EOF
  # pkg-config: libdpdk is $STUB_DPDK_VERSION (default 26.07.0); answers --exists, --modversion and
  # --atleast-version (also as --atleast-version V, or --exists "libdpdk >= V")
  make_stub pkg-config <<'EOF'
echo "pkg-config $*" >> "$LOG"
if [ -n "${STUB_NO_LIBDPDK:-}" ]; then echo "Package libdpdk was not found" >&2; exit 1; fi
ver="${STUB_DPDK_VERSION:-26.07.0}"
req=""; prev=""
for a in "$@"; do
  case $a in
    --atleast-version=*) req=${a#*=};;
    *">="*) req=$(sed 's/.*>= *//' <<<"$a");;
  esac
  case $prev in --atleast-version|">=") req=$a;; esac
  prev=$a
done
if [ -n "$req" ]; then
  [ "$(printf '%s\n%s\n' "$req" "$ver" | sort -V | head -n 1)" = "$req" ] && exit 0
  exit 1
fi
case "$*" in *--modversion*) echo "$ver";; esac
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
v="${STUB_RUSTC_VERSION:-1.99.1}"
case "$*" in
  *-vV*|*"--version --verbose"*|*"-V -v"*|*"--verbose --version"*)
    printf 'rustc %s (abcdef123 2026-10-01)\nbinary: rustc\ncommit-hash: abcdef123\nhost: aarch64-unknown-linux-gnu\nrelease: %s\nLLVM version: %s\n' "$v" "$v" "${STUB_RUSTC_LLVM:-23.1.0}";;
  *) echo "rustc $v (abcdef123 2026-10-01)";;
esac
EOF
  # node: only version queries matter
  make_stub node <<'EOF'
echo "node $*" >> "$LOG"
v="${STUB_NODE_VERSION:-22.22.1}"
case "$*" in
  *process.versions.node*) echo "$v";;
  *process.version*) echo "v$v";;
  *--version*|*" -v"*|"-v") echo "v$v";;
esac
exit 0
EOF
  # npm: logs argv and directory; `ci` makes node_modules, `run build` makes dist/ in its directory
  make_stub npm <<'EOF'
echo "npm $*" >> "$LOG"; echo "npm $* @ $PWD" >> "$LOG.dirs"
case "${1:-}" in --version|-v) echo "10.2.4"; exit 0;; esac
sub=""; for a in "$@"; do case $a in -*) ;; *) sub=$a; break;; esac; done
if [ -n "${STUB_NPM_FAIL_SUB:-}" ] && [ "$sub" = "$STUB_NPM_FAIL_SUB" ]; then
  echo "npm ERR! stub failure of npm $sub" >&2; exit 1
fi
case $sub in
  ci|install) mkdir -p node_modules;;
  run)
    script=""; seen=0
    for a in "$@"; do case $a in -*) ;; run) seen=1;; *) [ "$seen" = 1 ] && [ -z "$script" ] && script=$a;; esac; done
    if [ "$script" = build ] && [ -n "${STUB_NPM_BUILD_NOOP:-}" ]; then
      :  # a build that succeeds but writes nothing: whatever dist holds stays
    elif [ "$script" = build ]; then
      rm -rf dist; mkdir -p dist/assets
      echo "console.log('built')" >dist/assets/app.js
      [ -n "${STUB_NPM_NO_INDEX:-}" ] || echo "<html>BUILT-FRONTEND</html>" >dist/index.html
    fi;;
esac
exit 0
EOF
  make_stub cargo <<'EOF'
dir=$PWD; prev=""
for a in "$@"; do [ "$prev" = --manifest-path ] && dir=$(cd "$(dirname "$a")" 2>/dev/null && pwd -P); prev=$a; done
echo "cargo $*" >> "$LOG"; echo "cargo $* @ $dir" >> "$LOG.dirs"
case "${1:-}" in
  --version|-V|-vV|--version=*) echo "cargo ${STUB_RUSTC_VERSION:-1.99.1} (stub)"; exit 0;;
  build) ;;
  *) exit 0;;
esac
# what the build sees of the frontend: the dist dir, its index.html (newlines removed) and a nested file
dist="${DPDS_FRONTEND_DIST:-}"; idx=MISSING; app=no
[ -f "$dist/index.html" ] && idx=$(tr -d '\n' <"$dist/index.html")
[ -f "$dist/assets/app.js" ] && app=yes
echo "cargo $* :: DPDS_FRONTEND_DIST=${DPDS_FRONTEND_DIST-<unset>} index=$idx app=$app" >> "$LOG.dist"
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

# A frontend dist built by the user (`npm run build`), named by DPDS_FRONTEND_DIST.
make_local_dist() {
  LOCAL_DIST="$T/mydist"
  mkdir -p "$LOCAL_DIST/assets"
  echo "<html>LOCAL-DIST</html>" >"$LOCAL_DIST/index.html"
  echo "console.log('local')" >"$LOCAL_DIST/assets/app.js"
  export DPDS_FRONTEND_DIST="$LOCAL_DIST"
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
remove_tool() { rm -f "$T/stubs/$1" "$T/base/$1"; }

# run_dpds ARGS... : sets OUT (stdout), ERR (stderr), RC. Stdin from $STDIN (default empty).
run_dpds() {
  local errf="$T/stderr" outf="$T/stdout"
  ( cd "$T/cwd" && PATH="$T/stubs:$T/base" TMPDIR="$T/tmp" timeout 60 bash "$SCRIPT" "$@" <"${STDIN:-/dev/null}" >"$outf" 2>"$errf" )
  RC=$?
  OUT="$(cat "$outf")"; ERR="$(cat "$errf")"
  # spec v13: no command ever calls docker
  [[ ! -s "$STUB_LOG.docker" ]] || fail "dpds.sh called docker: $(tr '\n' '|' <"$STUB_LOG.docker")"
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

# ------------------------------------------------------------------ tests: spec v18 (--stable = latest release tag)
RELEASE_TAGS="v1.9.0 v1.10.0 v1.2.0 v0.9.9 x2.0.0 nightly-build"

test_stable_install_builds_the_highest_release_tag() {
  export STUB_GIT_TAGS="$RELEASE_TAGS"
  run_dpds --stable --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^git fetch .*--tags|^git fetch --tags'
  assert_log_has '^git checkout (--detach )?v1\.10\.0$'
  assert_log_lacks '^git checkout .*(nightly|v1\.9\.0|v1\.2\.0|v0\.9\.9|x2\.0\.0)'
  assert_log_lacks '^git pull'
  assert_before '^(rustc|cargo|node|npm|clang|pkg-config) ' '^git fetch'
  assert_before '^git fetch' '^git checkout'
  assert_before '^git checkout' '^npm ci'
  assert_before '^git checkout' '^cargo build'
  assert_dir_of '^git (fetch|checkout)' "$REPO_ROOT"
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
  assert_file_has "$DPDS_PREFIX/bin/dpds" 'STUB-BINARY-dpds'
}

test_stable_update_checks_out_the_highest_release_tag_instead_of_nightly() {
  fake_install
  export STUB_GIT_TAGS="$RELEASE_TAGS"
  run_dpds --stable --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^git fetch .*--tags|^git fetch --tags'
  assert_log_has '^git checkout (--detach )?v1\.10\.0$'
  assert_log_lacks '^git checkout nightly'
  assert_log_lacks '^git pull'
  assert_before '^git fetch' '^git checkout'
  assert_before '^git checkout' '^npm ci'
  assert_before '^git checkout' '^cargo build'
  assert_dir_of '^git (fetch|checkout)' "$REPO_ROOT"
  ! grep -Ei 'warn' <<<"$OUT$ERR" | grep -qi 'nightly' || fail "a warning talks about switching to nightly: $(grep -Ei warn <<<"$OUT$ERR")"
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
}

test_stable_update_restarts_only_a_service_that_was_active() {
  fake_install
  export STUB_GIT_TAGS="v1.0.0" STUB_ACTIVE=1
  run_dpds --stable --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl disable --now dpds-controller$'
  assert_log_has '^systemctl enable --now dpds-controller$'
  : >"$STUB_LOG"
  STUB_ACTIVE=0 run_dpds --stable --yes update
  assert_rc 0 "$ERR"
  assert_log_lacks '^systemctl (enable|start|restart)'
}

test_stable_update_refuses_a_dirty_tree_before_any_git_change() {
  fake_install
  export STUB_GIT_TAGS="v1.0.0" STUB_GIT_DIRTY=1
  run_dpds --stable --yes update
  assert_rc_nonzero
  grep -Eiq 'uncommitted|dirty|working tree|local changes' <<<"$ERR" || fail "stderr does not explain the dirty tree: $ERR"
  assert_log_lacks '^git checkout'
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable)'
}

test_stable_update_from_a_detached_release_tag_is_not_blocked_by_a_warning_about_nightly() {
  fake_install
  export STUB_GIT_TAGS="v1.0.0 v1.1.0" STUB_GIT_BRANCH=HEAD
  run_dpds --stable --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^git checkout (--detach )?v1\.1\.0$'
  assert_log_lacks '^git checkout nightly'
}

test_stable_picks_the_highest_version_not_the_last_or_the_alphabetical_one() {
  local tags expected
  for pair in "v2.0.0 v10.0.0 v9.9.9=v10.0.0" "v1.0.0=v1.0.0" "v1.0.9 v1.0.10 v1.0.2=v1.0.10" "v0.0.9 v0.1.0 v0.0.10=v0.1.0"; do
    tags="${pair%=*}"; expected="${pair#*=}"
    : >"$STUB_LOG"
    STUB_GIT_TAGS="$tags" run_dpds --stable --yes install
    assert_rc 0 "$tags: $ERR"
    assert_log_has "^git checkout (--detach )?${expected//./\\.}\$"
  done
}

test_stable_ignores_tags_that_are_not_releases() {
  export STUB_GIT_TAGS="x2.0.0 nightly-build 1.99.0 zz v1.0.0"
  run_dpds --stable --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^git checkout (--detach )?v1\.0\.0$'
}

test_stable_without_a_release_tag_is_a_clear_error_for_install_and_update() {
  local tags
  for tags in "" "nightly-build x2.0.0 1.0.0"; do
    for c in install update; do
      fake_install
      : >"$STUB_LOG"
      STUB_GIT_TAGS="$tags" run_dpds --stable --yes "$c"
      CTX="$c, tags '$tags'"
      assert_rc_nonzero "$c"
      assert_err_has "release"
      assert_err_has "tag"
      assert_log_lacks '^git checkout'
      assert_log_lacks '^git pull'
      assert_log_lacks '^npm (ci|run|install)'
      assert_log_lacks '^cargo build'
      assert_log_lacks '^systemctl (enable|disable|restart|daemon-reload)'
      ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" 2>/dev/null || fail "binary replaced by $c"
    done
  done
  CTX=""
}

test_stable_aborts_before_any_build_when_fetching_tags_fails() {
  fake_install
  export STUB_GIT_TAGS="v1.0.0" STUB_GIT_FAIL_SUB=fetch
  run_dpds --stable --yes update
  assert_rc_nonzero
  assert_log_lacks '^git checkout'
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable|restart)'
}

test_stable_aborts_before_any_build_when_the_checkout_fails() {
  fake_install
  export STUB_GIT_TAGS="v1.0.0" STUB_GIT_FAIL_SUB=checkout
  run_dpds --stable --yes update
  assert_rc_nonzero
  assert_log_lacks '^npm (ci|run|install)'
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable|restart)'
}

test_stable_leaves_the_other_commands_alone() {
  local c
  export STUB_GIT_TAGS="v1.0.0"
  for c in start stop restart status uninstall; do
    : >"$STUB_LOG"
    run_dpds --stable --yes "$c"
    assert_log_lacks '^git '
    assert_log_lacks '^(cargo build|npm (ci|run|install))'
  done
}

test_nightly_install_makes_no_git_call_even_when_release_tags_exist() {
  export STUB_GIT_TAGS="$RELEASE_TAGS"
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_lacks '^git '
  : >"$STUB_LOG"
  run_dpds --nightly --yes install
  assert_rc 0 "$ERR"
  assert_log_lacks '^git '
}

test_nightly_update_keeps_following_the_branch_even_when_release_tags_exist() {
  fake_install
  export STUB_GIT_TAGS="$RELEASE_TAGS"
  run_dpds --nightly --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^git checkout nightly$'
  assert_log_has '^git pull --ff-only$'
  assert_log_lacks '^git checkout (--detach )?v[0-9]'
}

test_help_describes_stable_as_the_latest_release_tag() {
  run_dpds --help
  assert_rc 0
  grep -E -- '--stable' <<<"$OUT" | grep -Eqi 'release|tag' || fail "--help does not tie --stable to a release or tag: $OUT"
  ! grep -Eqi 'no rust stable release|only prints|not yet|no stable release' <<<"$OUT" || fail "--help still says there is no stable release: $OUT"
  grep -E -- '--nightly' <<<"$OUT" | grep -qi 'default' || fail "--help no longer says --nightly is the default: $OUT"
}

# ------------------------------------------------------------------ tests: spec v18 (status shows the installed version)
# a line of the output that is not the raw JSON answer
plain_line_with() { grep -v '[{"]' <<<"$OUT" | grep -Fq -- "$1"; }

test_status_prints_the_installed_version_on_its_own_line() {
  run_dpds status
  assert_rc 0 "$ERR"
  plain_line_with "7.8.9" || fail "no plain line shows the version 7.8.9: $OUT"
}

test_status_finds_the_version_whatever_the_key_order_and_spacing() {
  export STUB_ONLINE_JSON='{"dpds_bin":"/usr/local/bin/dpds","telemetry":true,"version": "3.4.5","root":true}'
  run_dpds status
  assert_rc 0 "$ERR"
  plain_line_with "3.4.5" || fail "no plain line shows the version 3.4.5: $OUT"
  export STUB_ONLINE_JSON='{
  "version": "11.12.13",
  "telemetry": true
}'
  run_dpds status
  assert_rc 0 "$ERR"
  plain_line_with "11.12.13" || fail "no plain line shows the version 11.12.13: $OUT"
}

test_status_shows_no_version_when_the_api_is_down_and_still_exits_zero() {
  export STUB_CURL_RC=7
  run_dpds status
  assert_rc 0 "$ERR"
  ! plain_line_with "7.8.9" || fail "a version is shown although the API is down: $OUT"
}

test_status_survives_an_answer_without_a_version() {
  export STUB_ONLINE_JSON='{"telemetry":true}'
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl .*status'
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

test_check_works_without_docker_curl_tar_and_sha256sum() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  local t
  for t in docker curl tar sha256sum; do remove_tool "$t"; done
  run_dpds check
  assert_rc 0 "$ERR"
  assert_out_has "CHECK-OUTPUT-MARKER"
  assert_log_has '^dpds-controller check'
}

test_check_building_the_controller_downloads_nothing() {
  local t
  for t in docker curl tar sha256sum; do remove_tool "$t"; done
  run_dpds check
  assert_rc 0 "$ERR"
  assert_log_has '^cargo build .*(-p|--package|--bin)[ =]dpds-controller( |$)'
  assert_log_lacks '^curl '
  assert_log_lacks '^git .*rev-parse'
  assert_log_lacks '^npm (ci|run)'
  assert_out_has "CHECK-OUTPUT-MARKER"
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

test_install_unit_keeps_the_memlock_limit() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_file_has "$(installed_unit)" '^LimitMEMLOCK=infinity$'
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

test_install_reloads_systemd_after_the_host_check() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl daemon-reload'
  assert_before '^dpds-controller check' '^systemctl daemon-reload'
}

test_install_does_not_enable_or_start() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_lacks '^systemctl (enable|start|restart|disable)'
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

# ------------------------------------------------------------------ tests: start/stop/restart/status
test_start_enables_and_starts_the_service_only() {
  run_dpds start
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
  assert_log_lacks '^(cargo|git|curl) '
}

test_start_needs_no_build_tools() {
  local t
  for t in cargo rustc clang ld.lld pkg-config git curl node npm; do remove_stub "$t"; done
  run_dpds start
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
}

test_start_systemctl_failure_propagates() {
  export STUB_SYSTEMCTL_RC=1
  run_dpds start
  assert_rc_nonzero
}

test_stop_disables_and_stops_the_service_only() {
  run_dpds stop
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl disable --now dpds-controller$'
  assert_log_lacks '^(cargo|git|curl) '
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
}

test_status_runs_both_probes() {
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl .*status.* dpds-controller'
  assert_log_has '^curl .*http://127\.0\.0\.1:8100/api/online'
  assert_out_has "STUB-SYSTEMCTL-STATUS"
  assert_out_has "7.8.9"
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
}

test_update_does_not_start_when_it_was_inactive() {
  fake_install
  export STUB_ACTIVE=0
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl is-active.* dpds-controller'
  assert_log_lacks '^systemctl (enable|start|restart)'
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

# ------------------------------------------------------------------ tests: status probes the configured bind address
write_env_bind() { mkdir -p "$DPDS_ETC_DIR"; printf 'DPDS_CONTROLLER_BIND=%s\n' "$1" >"$DPDS_ETC_DIR/controller.env"; }

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
  assert_log_has '^systemctl enable --now dpds-controller$'
  assert_before '^systemctl disable --now dpds-controller' '^systemctl enable --now dpds-controller'
}

test_update_restart_still_starts_when_the_stop_fails() {
  fake_install
  export STUB_ACTIVE=1 STUB_SYSTEMCTL_FAIL_SUB=disable
  run_dpds --yes update
  assert_rc_nonzero
  grep -Eqi 'stop' <<<"$ERR" || fail "the failed stop is not reported: $ERR"
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
  assert_log_has '^systemctl enable --now dpds-controller$'
}

# ------------------------------------------------------------------ tests: spec v7 P3 (prerequisites, host check)
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


# ------------------------------------------------------------------ tests: spec v15 (frontend built on the host, prerequisites)
# ---- helpers
PREREQ_TOOLS="cargo rustc clang ld.lld pkg-config libdpdk node npm"
# break_prereq TOOL: make one prerequisite unavailable (libdpdk: the package, not pkg-config)
break_prereq() { case "$1" in libdpdk) export STUB_NO_LIBDPDK=1;; *) remove_tool "$1";; esac; }
# the word an ERROR line must use for the tool
named_as() { case "$1" in ld.lld) echo lld;; *) echo "$1";; esac; }
# words that an ERROR line about the broken tool must not use (other tool groups)
unrelated_to() {
  case "$1" in
    node|npm) echo "cargo rustc clang lld libdpdk pkg-config";;
    clang|ld.lld) echo "node npm libdpdk pkg-config cargo";;
    pkg-config|libdpdk) echo "node npm clang lld";;
  esac
}
err_lines() { grep -E 'ERROR' <<<"$ERR" || true; }
# assert_error_line_has WORD...: one single ERROR line on stderr contains every WORD (case-insensitive)
assert_error_line_has() {
  local line w ok
  while IFS= read -r line; do
    ok=1
    for w in "$@"; do grep -qiF -- "$w" <<<"$line" || ok=0; done
    (( ok )) && return 0
  done < <(err_lines)
  fail "no single ERROR line has all of '$*'; stderr: $ERR"
}
assert_error_lines_lack() {
  local w
  for w in "$@"; do
    ! err_lines | grep -qiF -- "$w" || fail "an ERROR line names '$w' although it is unrelated: $(err_lines | tr '\n' '|')"
  done
}
# assert_ok_line_has WORD...: one OK line (stdout or stderr) contains every WORD
assert_ok_line_has() {
  local line w ok
  while IFS= read -r line; do
    grep -Eq '(^|[^[:alnum:]])OK([^[:alnum:]]|$)' <<<"$line" || continue
    ok=1
    for w in "$@"; do grep -qiF -- "$w" <<<"$line" || ok=0; done
    (( ok )) && return 0
  done <<<"$OUT"$'\n'"$ERR"
  fail "no OK line has all of '$*'; output: $OUT"
}

# The frontend minimum of node, from the real frontend/package.json (engines.node ">=X.Y").
node_min() { sed -n 's/.*"node"[[:space:]]*:[[:space:]]*">=\([0-9][0-9.]*\)".*/\1/p' "$SRC_ROOT/frontend/package.json" | head -n 1; }
# split_min VERSION: sets V_MAJ and V_MIN (the first two components)
split_min() { local rest; V_MAJ="${1%%.*}"; rest="${1#*.}"; V_MIN="${rest%%.*}"; }
# version_below VERSION / version_at VERSION: "X.Y.9" just under and "X.Y.0" exactly at X.Y
version_below() { split_min "$1"; if (( V_MIN > 0 )); then echo "$V_MAJ.$((V_MIN - 1)).9"; else echo "$((V_MAJ - 1)).99.9"; fi; }
version_at() { split_min "$1"; echo "$V_MAJ.$V_MIN.0"; }
version_above() { split_min "$1"; echo "$V_MAJ.$V_MIN.1 $((V_MAJ + 1)).0.0 100.0.0"; }
rust_min() { sed -n 's/^rust-version *= *"\([0-9][0-9.]*\)".*/\1/p' "$SRC_ROOT/crates/dpds-core/Cargo.toml" | head -n 1; }

assert_nothing_downloaded() { assert_log_lacks '^(curl|tar|sha256sum) '; }
assert_nothing_built_or_installed() {
  assert_log_lacks '^cargo build'
  assert_log_lacks '^(systemctl|dpds-controller check)'
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
  assert_no_file "$DPDS_ETC_DIR/controller.env"
}
# every controller build got the repository's freshly built dist (and there is at least one)
assert_controller_built_with_repo_dist() {
  local n=0 line
  while IFS= read -r line; do
    n=$((n + 1))
    [[ "$line" == *":: DPDS_FRONTEND_DIST=$REPO_ROOT/frontend/dist index=<html>BUILT-FRONTEND</html> app=yes" ]] \
      || fail "controller build without the repo's built dist: $line"
  done < <(grep -E '^cargo build .*(-p|--package|--bin)[ =]dpds-controller( |$)' "$STUB_LOG.dist")
  (( n >= 1 )) || fail "no controller build logged"
}

# ---- prerequisites: every tool is named
test_check_names_each_missing_tool_and_says_what_to_install() {
  local c
  for c in $PREREQ_TOOLS; do
    setup_sandbox; CTX="without $c"
    write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
    break_prereq "$c"
    run_dpds check
    assert_rc_nonzero
    assert_error_line_has "$(named_as "$c")" install
    # shellcheck disable=SC2046
    assert_error_lines_lack $(unrelated_to "$c")
    assert_log_lacks '^dpds-controller check'
  done
}

test_install_names_each_missing_tool_and_does_nothing() {
  local c
  for c in $PREREQ_TOOLS; do
    setup_sandbox; CTX="without $c"
    break_prereq "$c"
    run_dpds --yes install
    assert_rc_nonzero
    assert_error_line_has "$(named_as "$c")"
    assert_log_lacks '^(npm (ci|run)|git )'
    assert_nothing_built_or_installed
  done
}

test_update_names_each_missing_tool_before_any_git_cargo_or_npm_call() {
  local c
  for c in $PREREQ_TOOLS; do
    setup_sandbox; CTX="without $c"
    fake_install
    export STUB_ACTIVE=1
    break_prereq "$c"
    run_dpds --yes update
    assert_rc_nonzero
    assert_error_line_has "$(named_as "$c")"
    assert_log_lacks '^git '
    assert_log_lacks '^npm (ci|run)'
    assert_log_lacks '^cargo build'
    assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
    ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" || fail "binary was replaced"
  done
}

# ---- prerequisites: all failures at once
# Missing cargo and node, no libdpdk and a clang of another LLVM major: four ERROR lines in one run.
break_several() {
  remove_tool cargo; remove_tool node
  export STUB_NO_LIBDPDK=1 STUB_CLANG_VERSION=22.1.0
}
assert_all_four_named() {
  assert_rc_nonzero
  assert_error_line_has cargo
  assert_error_line_has node
  assert_error_line_has libdpdk
  assert_error_line_has clang 22
}
test_check_lists_every_failure_not_just_the_first() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  break_several
  run_dpds check
  assert_all_four_named
  assert_log_lacks '^dpds-controller check'
}

test_install_lists_every_failure_not_just_the_first() {
  break_several
  run_dpds --yes install
  assert_all_four_named
  assert_nothing_built_or_installed
  assert_log_lacks '^(npm (ci|run)|git )'
}

test_update_lists_every_failure_not_just_the_first() {
  fake_install
  break_several
  run_dpds --yes update
  assert_all_four_named
  assert_log_lacks '^(git |npm (ci|run)|cargo build)'
}

test_check_names_all_seven_tools_when_none_is_installed() {
  local t
  for t in cargo rustc clang ld.lld pkg-config node npm; do remove_tool "$t"; done
  run_dpds check
  assert_rc_nonzero
  for t in cargo rustc clang lld pkg-config node npm; do CTX="$t"; assert_error_line_has "$t"; done
}

test_install_names_all_seven_tools_when_none_is_installed() {
  local t
  for t in cargo rustc clang ld.lld pkg-config node npm; do remove_tool "$t"; done
  run_dpds --yes install
  assert_rc_nonzero
  for t in cargo rustc clang lld pkg-config node npm; do CTX="$t"; assert_error_line_has "$t"; done
  assert_nothing_built_or_installed
}

test_check_reports_met_prerequisites_as_ok_lines_with_the_rust_version() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  run_dpds check
  assert_rc 0 "$ERR"
  local t
  for t in cargo rustc clang lld libdpdk node npm; do CTX="$t"; assert_ok_line_has "$t"; done
  CTX="rust version"; assert_ok_line_has rustc 1.99.1
  assert_out_has "CHECK-OUTPUT-MARKER"
}

# ---- prerequisites: node version
test_the_node_minimum_comes_from_the_frontend_package_json() {
  local min; min="$(node_min)"
  [[ "$min" =~ ^[0-9]+\.[0-9]+ ]] || { fail "cannot read engines.node (>=X.Y) from frontend/package.json: '$min'"; return; }
  # either dpds.sh reads package.json, or it hard-codes the same number
  grep -q 'package\.json' "$SRC_ROOT/dpds.sh" || grep -qF -- "$min" "$SRC_ROOT/dpds.sh" \
    || fail "dpds.sh neither reads frontend/package.json nor names the minimum $min"
}

test_check_accepts_node_at_or_above_the_minimum_compared_numerically() {
  local min v; min="$(node_min)"
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  for v in "$(version_at "$min")" $(version_above "$min"); do
    : >"$STUB_LOG"; CTX="node $v (minimum $min)"
    STUB_NODE_VERSION="$v" run_dpds check
    assert_rc 0 "$ERR"
    assert_out_has "CHECK-OUTPUT-MARKER"
  done
}

test_check_rejects_node_below_the_minimum_and_names_the_required_version() {
  local min v; min="$(node_min)"
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  for v in "$(version_below "$min")" 9.11.2 0.12.18; do
    : >"$STUB_LOG"; CTX="node $v (minimum $min)"
    STUB_NODE_VERSION="$v" run_dpds check
    assert_rc_nonzero
    assert_error_line_has node "$min"
    assert_log_lacks '^dpds-controller check'
  done
}

test_install_rejects_an_old_node_before_building_anything() {
  local min v; min="$(node_min)"; v="$(version_below "$min")"
  STUB_NODE_VERSION="$v" run_dpds --yes install
  assert_rc_nonzero
  assert_error_line_has node "$min"
  assert_log_lacks '^(npm (ci|run)|git )'
  assert_nothing_built_or_installed
}

test_update_rejects_an_old_node_before_any_git_call() {
  local min v; min="$(node_min)"; v="$(version_below "$min")"
  fake_install
  export STUB_ACTIVE=1
  STUB_NODE_VERSION="$v" run_dpds --yes update
  assert_rc_nonzero
  assert_error_line_has node "$min"
  assert_log_lacks '^(git |npm (ci|run)|cargo build)'
  assert_log_lacks '^systemctl (enable|disable)'
}

# ---- prerequisites: DPDK version (user, 2026-11-03: at least 26.07)
test_check_accepts_dpdk_26_07_and_newer() {
  local v
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  for v in 26.07.0 26.07.1 26.11.0 27.03.0; do
    : >"$STUB_LOG"; CTX="DPDK $v"
    STUB_DPDK_VERSION="$v" run_dpds check
    assert_rc 0 "$ERR"
    assert_out_has "CHECK-OUTPUT-MARKER"
  done
}

test_check_rejects_older_dpdk_naming_the_required_and_the_found_version() {
  local v
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  for v in 25.11.0 25.12.0 26.03.0 24.11.2; do
    : >"$STUB_LOG"; CTX="DPDK $v"
    STUB_DPDK_VERSION="$v" run_dpds check
    assert_rc_nonzero
    assert_error_line_has libdpdk 26.07 "${v%.*}"
    assert_log_lacks '^dpds-controller check'
  done
}

test_install_rejects_dpdk_25_11_before_building_anything() {
  STUB_DPDK_VERSION=25.11.0 run_dpds --yes install
  assert_rc_nonzero
  assert_error_line_has libdpdk 26.07 25.11
  assert_log_lacks '^(npm (ci|run)|git )'
  assert_nothing_built_or_installed
}

test_update_rejects_dpdk_25_11_before_any_git_call() {
  fake_install
  export STUB_ACTIVE=1
  STUB_DPDK_VERSION=25.11.0 run_dpds --yes update
  assert_rc_nonzero
  assert_error_line_has libdpdk 26.07 25.11
  assert_log_lacks '^(git |npm (ci|run)|cargo build)'
  assert_log_lacks '^systemctl (enable|disable)'
}

test_the_dpdk_version_is_asked_of_pkg_config() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  run_dpds check
  assert_rc 0 "$ERR"
  assert_log_has '^pkg-config .*(--atleast-version[ =]26\.07|--modversion|>= *26\.07)'
}

test_dpdk_sys_requires_the_same_minimum() {
  assert_file_has "$SRC_ROOT/crates/dpdk-sys/build.rs" 'atleast_version\("26\.07"\)'
}

# ---- prerequisites: rustc against the workspace's rust-version
test_rustc_below_the_workspace_rust_version_is_rejected() {
  local rv v; rv="$(rust_min)"
  [[ "$rv" =~ ^[0-9]+\.[0-9]+$ ]] || { fail "cannot read rust-version (X.Y) from crates/dpds-core/Cargo.toml: '$rv'"; return; }
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  for v in "$(version_below "$rv")" 1.50.0 1.9.9; do
    : >"$STUB_LOG"; CTX="rustc $v (rust-version $rv)"
    STUB_RUSTC_VERSION="$v" run_dpds check
    assert_rc_nonzero
    assert_error_line_has rustc
    assert_log_lacks '^dpds-controller check'
  done
}

test_rustc_at_or_above_the_workspace_rust_version_is_accepted() {
  local rv v; rv="$(rust_min)"
  [[ "$rv" =~ ^[0-9]+\.[0-9]+$ ]] || { fail "cannot read rust-version: '$rv'"; return; }
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  for v in "$rv.0" "$rv.0-nightly" "$rv.1" 1.100.0 2.0.0; do
    : >"$STUB_LOG"; CTX="rustc $v (rust-version $rv)"
    STUB_RUSTC_VERSION="$v" run_dpds check
    assert_rc 0 "$ERR"
    assert_out_has "CHECK-OUTPUT-MARKER"
  done
}

# ---- frontend: built on the host
test_install_builds_the_frontend_with_npm_ci_then_npm_run_build_in_frontend() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^npm ci( |$)'
  assert_log_has '^npm run build( |$)'
  assert_before '^npm ci( |$)' '^npm run build( |$)'
  assert_dir_of '^npm (ci|run build)( |$)' "$REPO_ROOT/frontend"
  assert_file "$REPO_ROOT/frontend/dist/index.html"
}

test_install_builds_the_frontend_before_any_cargo_build() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_before '^npm ci( |$)' '^cargo build'
  assert_before '^npm run build( |$)' '^cargo build'
  assert_before '^npm run build( |$)' '^cargo build .*(-p|--package|--bin)[ =]dpds-controller( |$)'
}

test_the_controller_is_built_with_the_repo_frontend_dist() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_controller_built_with_repo_dist
  assert_dir_of '^cargo build' "$REPO_ROOT"
}

test_update_builds_the_frontend_after_the_pull_and_before_cargo() {
  fake_install
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_before '^git pull --ff-only$' '^npm ci( |$)'
  assert_before '^npm ci( |$)' '^npm run build( |$)'
  assert_before '^npm run build( |$)' '^cargo build'
  assert_dir_of '^npm (ci|run build)( |$)' "$REPO_ROOT/frontend"
  assert_controller_built_with_repo_dist
  assert_dir_of '^cargo build' "$REPO_ROOT"
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
}

test_the_directory_change_into_frontend_does_not_leak_into_later_steps() {
  # the cd into frontend/ must not leak into the later steps (install paths, git, cargo)
  fake_install
  export STUB_ACTIVE=1
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_dir_of '^(git (fetch|checkout|pull)|cargo build)' "$REPO_ROOT"
}

# ---- frontend: failures stop everything
test_a_failing_npm_ci_stops_install_before_the_build_and_installs_nothing() {
  export STUB_NPM_FAIL_SUB=ci
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "npm"
  assert_log_has '^npm ci( |$)'
  assert_log_lacks '^npm run build'
  assert_nothing_built_or_installed
}

test_a_failing_npm_run_build_stops_install_before_cargo_and_installs_nothing() {
  export STUB_NPM_FAIL_SUB=run
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "npm"
  assert_before '^npm ci( |$)' '^npm run build( |$)'
  assert_nothing_built_or_installed
}

test_a_missing_index_html_after_the_build_stops_install() {
  export STUB_NPM_NO_INDEX=1
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "index.html"
  assert_err_has "frontend/dist"
  assert_log_has '^npm run build( |$)'
  assert_nothing_built_or_installed
}

# ---- frontend: a stale dist is never embedded
STALE_MARKER="STALE-FRONTEND"
# seed_stale_dist: a dist of an earlier build in the sandbox repository
seed_stale_dist() {
  mkdir -p "$REPO_ROOT/frontend/dist/assets"
  echo "<html>$STALE_MARKER</html>" >"$REPO_ROOT/frontend/dist/index.html"
  echo "console.log('stale')" >"$REPO_ROOT/frontend/dist/assets/app.js"
}
assert_stale_dist_not_embedded() {
  ! grep -qF -- "$STALE_MARKER" "$STUB_LOG.dist" || fail "the stale dist was embedded: $(tr '\n' '|' <"$STUB_LOG.dist")"
}

test_a_stale_dist_is_not_embedded_when_the_build_writes_nothing() {
  seed_stale_dist
  export STUB_NPM_BUILD_NOOP=1
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "index.html"
  assert_log_has '^npm run build( |$)'
  assert_stale_dist_not_embedded
  assert_nothing_built_or_installed
  assert_no_file "$REPO_ROOT/frontend/dist/index.html"
}

test_update_does_not_embed_a_stale_dist_when_the_build_writes_nothing() {
  fake_install
  export STUB_ACTIVE=1 STUB_NPM_BUILD_NOOP=1
  seed_stale_dist
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "index.html"
  assert_stale_dist_not_embedded
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" || fail "binary was replaced"
}

test_a_successful_build_replaces_a_stale_dist() {
  seed_stale_dist
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_file_has "$REPO_ROOT/frontend/dist/index.html" 'BUILT-FRONTEND'
  assert_stale_dist_not_embedded
  assert_controller_built_with_repo_dist
}

test_a_failing_npm_ci_leaves_the_previous_dist_in_place() {
  seed_stale_dist
  export STUB_NPM_FAIL_SUB=ci
  run_dpds --yes install
  assert_rc_nonzero
  assert_log_lacks '^npm run build'
  assert_file "$REPO_ROOT/frontend/dist/index.html"
  assert_file_has "$REPO_ROOT/frontend/dist/index.html" "$STALE_MARKER"
  assert_file "$REPO_ROOT/frontend/dist/assets/app.js"
  assert_nothing_built_or_installed
}

test_update_with_a_failing_npm_ci_leaves_the_previous_dist_in_place() {
  fake_install
  export STUB_ACTIVE=1 STUB_NPM_FAIL_SUB=ci
  seed_stale_dist
  run_dpds --yes update
  assert_rc_nonzero
  assert_file_has "$REPO_ROOT/frontend/dist/index.html" "$STALE_MARKER"
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
}

test_a_failing_npm_run_build_after_the_dist_was_removed_installs_nothing_and_embeds_nothing() {
  seed_stale_dist
  export STUB_NPM_FAIL_SUB=run
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "npm"
  assert_before '^npm ci( |$)' '^npm run build( |$)'
  assert_stale_dist_not_embedded
  assert_nothing_built_or_installed
  # the stale dist went away between npm ci and npm run build
  assert_no_file "$REPO_ROOT/frontend/dist/index.html"
}

test_a_failing_npm_ci_stops_update_and_leaves_the_service_alone() {
  fake_install
  export STUB_ACTIVE=1 STUB_NPM_FAIL_SUB=ci
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "npm"
  assert_log_lacks '^npm run build'
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" || fail "binary was replaced"
}

test_a_failing_npm_run_build_stops_update_and_leaves_the_service_alone() {
  fake_install
  export STUB_ACTIVE=1 STUB_NPM_FAIL_SUB=run
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "npm"
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" || fail "binary was replaced"
}

test_a_missing_index_html_stops_update_and_leaves_the_service_alone() {
  fake_install
  export STUB_ACTIVE=1 STUB_NPM_NO_INDEX=1
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "index.html"
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" || fail "binary was replaced"
}

test_install_after_a_failed_install_still_works() {
  export STUB_NPM_FAIL_SUB=run
  run_dpds --yes install
  assert_rc_nonzero
  unset STUB_NPM_FAIL_SUB
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

# ---- frontend: nothing is downloaded, the caller's DPDS_FRONTEND_DIST is not a thing any more
test_install_and_update_download_and_unpack_nothing() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_nothing_downloaded
  assert_log_lacks '^git '
  fake_install
  : >"$STUB_LOG"
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_nothing_downloaded
  assert_log_lacks '^git (rev-parse|status .*frontend)'
  assert_no_temp_leftovers
}

test_install_and_update_need_no_curl_sha256sum_or_tar() {
  local t
  for t in curl sha256sum tar; do remove_tool "$t"; done
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_controller_built_with_repo_dist
  fake_install
  run_dpds --yes update
  assert_rc 0 "$ERR"
}

test_a_dist_directory_in_the_callers_environment_is_ignored() {
  make_local_dist
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^npm run build( |$)'
  assert_controller_built_with_repo_dist
  ! grep -E '(-p|--package|--bin)[ =]dpds-controller( |$)' "$STUB_LOG.dist" | grep -qF -- "$LOCAL_DIST" \
    || fail "the controller was built with the caller's dist: $(tr '\n' '|' <"$STUB_LOG.dist")"
  assert_file "$LOCAL_DIST/index.html"
  assert_nothing_downloaded
}

test_update_ignores_a_dist_directory_in_the_callers_environment() {
  fake_install
  make_local_dist
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^npm run build( |$)'
  assert_controller_built_with_repo_dist
}

test_a_nonexistent_dist_variable_does_not_matter_any_more() {
  export DPDS_FRONTEND_DIST="$T/does-not-exist"
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_controller_built_with_repo_dist
  ! grep -qF -- "does-not-exist" <<<"$OUT$ERR" || fail "the variable is still mentioned: $OUT$ERR"
}

test_the_old_asset_url_variable_does_nothing() {
  export DPDS_FRONTEND_ASSET_URL=https://mirror.example/dl
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_lacks 'mirror\.example'
  assert_nothing_downloaded
}

test_a_dirty_frontend_is_built_as_it_is_without_a_warning() {
  export STUB_GIT_DIRTY_FRONTEND=1
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_lacks '^git '
  ! grep -Ei 'warn' <<<"$OUT$ERR" | grep -Eqi 'frontend|uncommitted|DPDS_FRONTEND_DIST' || fail "unexpected warning: $(grep -Ei warn <<<"$OUT$ERR")"
  assert_log_has '^npm run build( |$)'
  assert_controller_built_with_repo_dist
}

# ---- the script text: no leftovers of the release asset path
test_script_has_no_release_asset_leftovers() {
  local pat
  for pat in 'ASSET_URL' 'frontend-builds' 'sha256' '\.tar\.gz' 'frontend-asset' 'release asset' '--proto'; do
    ! grep -Eiq -- "$pat" "$SRC_ROOT/dpds.sh" || fail "dpds.sh still mentions /$pat/ (code or comment)"
  done
  local code; code="$(grep -v '^[[:space:]]*#' "$SRC_ROOT/dpds.sh")"
  ! grep -Eq '\btar\b|sha256sum|WORK_DIR' <<<"$code" || fail "dpds.sh code still uses tar, sha256sum or a work directory"
  ! grep -Eq '\$\{?DPDS_FRONTEND_DIST' <<<"$code" || fail "dpds.sh still reads the caller's DPDS_FRONTEND_DIST"
}

test_usage_text_describes_the_host_build_not_a_download() {
  run_dpds --help
  assert_rc 0
  grep -Eiq 'node|npm' <<<"$OUT" || fail "--help does not mention Node or npm: $OUT"
  ! grep -Eiq 'download|CI-built|release asset|DPDS_FRONTEND_DIST|asset' <<<"$OUT$ERR" || fail "--help still talks about the asset path: $OUT"
}

# ---- no docker anywhere

test_no_command_needs_docker() {
  local c
  remove_stub docker
  fake_install
  for c in check install update start stop restart status uninstall; do
    run_dpds --yes "$c"
    assert_rc 0 "$c: $ERR"
  done
}

test_usage_text_has_no_docker_compose_container_or_image() {
  run_dpds
  ! grep -Eiq 'docker|compose|container|image' <<<"$OUT$ERR" || fail "usage mentions docker: $OUT$ERR"
  run_dpds --help
  assert_rc 0
  assert_out_has "Usage"
  ! grep -Eiq 'docker|compose|container|image' <<<"$OUT$ERR" || fail "--help mentions docker: $OUT$ERR"
}

test_script_source_has_no_docker_nginx_or_frontend_container_settings() {
  local code; code="$(grep -v '^[[:space:]]*#' "$SCRIPT")"
  ! grep -Eiq 'docker|compose|nginx' <<<"$code" || fail "dpds.sh has a non-comment line with docker/compose/nginx"
  ! grep -Eq 'DPDS_FRONTEND_(BIND|PORT)|DPDS_CONTROLLER([^_A-Za-z]|$)' <<<"$code" || fail "dpds.sh still has the frontend container settings"
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

# Spec v17: dpds calls mlockall(MCL_CURRENT|MCL_FUTURE) before EAL init, and its parent is this
# unit's controller, so the unit sets the memlock limit of the service to unlimited explicitly
# instead of relying on systemd's default or on root's CAP_IPC_LOCK.
test_unit_lifts_the_memlock_limit_in_the_service_section() {
  local n; n="$(grep -Ec '^LimitMEMLOCK=' "$UNIT")"
  assert_eq "$n" "1" "number of LimitMEMLOCK= lines"
  assert_eq "$(unit_value LimitMEMLOCK)" "infinity" "LimitMEMLOCK"
  local section
  section="$(awk '/^\[/ {s=$0} /^LimitMEMLOCK=/ {print s}' "$UNIT")"
  assert_eq "$section" "[Service]" "section of LimitMEMLOCK"
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

