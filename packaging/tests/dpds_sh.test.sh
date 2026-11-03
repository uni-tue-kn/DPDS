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
#     (also for --help, exit 0) mentions no docker, compose, container or image: the frontend is
#     served by the controller (spec v13).
#   - --nightly is the default channel and is accepted explicitly.
#   - --stable: error on stderr saying that no Rust stable release exists yet ("stable release"),
#     exit non-zero, nothing is executed (no cargo/git/systemctl).
#   - --nightly together with --stable: exit non-zero, nothing is executed.
#   - Errors go to stderr. The script may be started from any working directory: it works on the
#     repository it lives in (cargo and git run in the repo root).
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
#   DPDS_FRONTEND_DIST       a frontend dist directory (with index.html) built by the user; install and
#                     update then download nothing and build the controller with it (see below)
#   DPDS_FRONTEND_ASSET_URL  base URL of the frontend release assets (default
#                     https://github.com/uni-tue-kn/DPDS/releases/download/frontend-builds; a trailing
#                     slash is fine)
#   Privileged operations go through sudo when EUID != 0 (like p4tg.sh's run_privileged). Tests
#   do not require that sudo is used, only that the effects happen.
#
# Prerequisites (checked by `check`, `install`, `update`; NOT by start/stop/restart/status/
# uninstall, which only need systemctl and, for status, curl). Each missing prerequisite is an error
# on stderr that names the tool, exit non-zero, and nothing is built or installed:
#   cargo, rustc                         on PATH
#   clang and ld.lld                     whose LLVM major equals the one reported by
#                                        `rustc -vV` ("LLVM version: N.x.y"); the error names the
#                                        tool and both majors. Minor/patch/vendor prefix ignored.
#   pkg-config with libdpdk              `pkg-config --exists libdpdk`; error names "libdpdk"
#   curl, sha256sum, tar                 install and update only (they fetch and unpack the frontend);
#                                        `check` needs none of them
#   No system packages are ever installed. Docker is not a prerequisite of anything.
#
# check
#   Prerequisites, then runs `dpds-controller check` and passes its stdout and exit code through.
#   Binary used: $DPDS_PREFIX/bin/dpds-controller if executable; otherwise the one in
#   $CARGO_TARGET_DIR/release if present; otherwise it first runs
#   `cargo build --release -p dpds-controller` (the build is in the repo root) and uses that. That
#   build downloads nothing (no curl, no `git rev-parse HEAD:frontend`).
#
# frontend asset (install and update; spec v13). The controller embeds the web frontend at compile
#   time; CI uploads the built dist as a release asset. Unless DPDS_FRONTEND_DIST is set, the script
#   - takes <tree> = `git rev-parse HEAD:frontend` (run in the repo root; for update after
#     checkout/pull, because the pulled tree is what must be built),
#   - downloads <base>/dpds-frontend-<tree>.tar.gz and <base>/dpds-frontend-<tree>.tar.gz.sha256 with
#     curl (<base> = $DPDS_FRONTEND_ASSET_URL or the default above) BEFORE the first cargo build,
#     treating HTTP errors as failures (curl -f),
#   - verifies the tarball against the .sha256 file, which has the format of `sha256sum`
#     ("<hash>  dpds-frontend-<tree>.tar.gz"),
#   - unpacks it into a fresh temporary directory (not under the repository's frontend/): the files of
#     dist/ are at the top level of the tarball (index.html, assets/...) - CI makes it with
#     `tar -C frontend/dist -czf <asset> .`,
#   - builds the controller (and only there is it needed) with DPDS_FRONTEND_DIST=<that directory>,
#   - removes the temporary directories when it exits, on success and on every failure.
#   Failures stop with a non-zero exit before anything is built, installed or restarted:
#   - checksum mismatch (also an empty or garbage .sha256 file): the message names the asset file and
#     says "checksum" or "sha256";
#   - asset missing (HTTP 404): the message names the asset file, says to push and wait for CI
#     (the word "push" and the word "CI"), and says to set DPDS_FRONTEND_DIST to a locally built
#     dist; any other download failure also stops;
#   With DPDS_FRONTEND_DIST set by the caller nothing is downloaded (no curl call) and that
#   directory is passed to the controller build as is and never removed. A caller-provided
#   DPDS_FRONTEND_DIST without index.html is an error that names the directory, before any cargo build.
#   Review fixes (spec v13b item 3):
#   - install warns (stderr, install still succeeds) when `git status --porcelain -- frontend` reports
#     changes: the asset of the committed frontend/ is used; the warning names DPDS_FRONTEND_DIST;
#   - the tarball downloads but its .sha256 is 404: the message says the checksum file is missing
#     (partial upload: re-run CI or wait), not that the asset "is not available";
#   - curl gets `--proto =https --proto-redir =https` when the base URL starts with https://, and
#     neither flag for an explicit http:// base;
#   - the tarball is listed (`tar -tvzf`) before it is extracted; one with an absolute path, a ".."
#     path component, a symlink or a hardlink is refused (message names the asset, nothing extracted,
#     built or installed); extraction uses `tar --no-same-owner`;
#   - the README states the trust model: the checksum only detects corruption.
#
# install  (order: prerequisites, frontend asset, build, host check, confirmation, installation)
#   1. fetches the frontend asset (above), then
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
# update   `systemctl is-active dpds-controller` is queried; a dirty working tree is refused
#          (error, non-zero, before any checkout/pull/build); a warning names the current branch
#          (or "detached") when it is not nightly; then `git fetch`, `git checkout nightly`,
#          `git pull --ff-only` (in the repo root; a failing git call aborts before building);
#          then the frontend asset for the pulled tree and the install flow (same prompt unless
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
#   Restart=on-failure, [Install] WantedBy=multi-user.target; parses without warnings under
#   `systemd-analyze verify`.
#
# Stub conventions used by this file (PATH contains only stubs + whitelisted base utilities):
#   every stub appends "<tool> <argv>" to $STUB_LOG ("sudo" is transparent and only logged to
#   $STUB_LOG.sudo); "<tool> <argv> @ <dir>" with the effective directory goes to $STUB_LOG.dirs.
#   `docker` is a tripwire: it fails and logs to $STUB_LOG.docker, and every run_dpds checks that
#   log, so no test can pass if a command calls docker. The cargo stub logs the frontend dist it was
#   given to $STUB_LOG.dist, the curl stub serves $STUB_ASSET_DIR (404 for anything else), the git
#   stub answers `rev-parse HEAD:frontend` with $STUB_FRONTEND_TREE; `git status` reports a dirty tree
#   when STUB_GIT_DIRTY is set, and a dirty frontend/ (only for a call whose arguments name `frontend`)
#   when STUB_GIT_DIRTY_FRONTEND is set. The tar stub logs its arguments and runs the real tar; the
#   sha256sum and gzip used by the script are the real ones.
# =====================================================================================

set -u

HERE="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -P -- "$HERE/../.." && pwd)"
SCRIPT="$REPO_ROOT/dpds.sh"
UNIT="$REPO_ROOT/packaging/dpds-controller.service"

BASE_UTILS="bash sh env timeout cat sed grep awk date mkdir cp install rm rmdir dirname basename readlink tee tr
  head tail sort uniq cut chmod chown ln mv id uname wc sleep touch find xargs stat realpath mktemp ls
  true false cmp expr hostname whoami diff printf test tty tar gzip sha256sum"
DEFAULT_TREE=0123456789abcdef0123456789abcdef01234567
ASSET_BASE_URL=https://github.com/uni-tue-kn/DPDS/releases/download/frontend-builds
# Any call to one of these is a tuning attempt (tripwires).
TUNING_TOOLS="ethtool setpci sysctl mlxconfig mlxfwreset cpupower tuned-adm grubby update-grub modprobe insmod"
# The tools every stub run provides.
STUB_TOOLS="sudo systemctl docker cargo git curl pkg-config clang ld.lld rustc"

# The real tar the logging stub hands over to (found with the caller's PATH, before any sandbox).
REAL_TAR="$(command -v tar)"
export REAL_TAR

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
# assert_dist_has REGEX: a cargo call logged with the frontend dist it saw matches.
assert_dist_has() { grep -Eq -- "$1" "$STUB_LOG.dist" 2>/dev/null || fail "cargo dist log lacks /$1/: $(tr '\n' '|' <"$STUB_LOG.dist" 2>/dev/null)"; }
# assert_err_word NAME: stderr names NAME as a word (not as part of another word).
assert_err_word() { grep -Eiq -- "(^|[^[:alnum:]_.-])$1([^[:alnum:]_.-]|\$)" <<<"$ERR" || fail "stderr does not name '$1': $ERR"; }
# Temp dirs of the script (download, unpack) must be gone when it exits (TMPDIR is $T/tmp).
assert_no_temp_leftovers() { [[ -z "$(ls -A "$T/tmp" 2>/dev/null)" ]] || fail "temp files left behind: $(ls -A "$T/tmp" | tr '\n' ' ')"; }
# The frontend dist the controller build was given (empty if none).
dist_of_controller_build() { sed -n 's/^cargo build .*dpds-controller\( .*\)* :: DPDS_FRONTEND_DIST=\([^ ]*\) .*/\2/p' "$STUB_LOG.dist" | tail -n 1; }
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
        DPDS_FRONTEND_DIST DPDS_FRONTEND_ASSET_URL
  export STUB_FRONTEND_TREE="$DEFAULT_TREE" STUB_ASSET_DIR="$T/assets"
  mkdir -p "$T/tmp"
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
  # tar: logs its arguments, then is the real tar
  make_stub tar <<'EOF'
echo "tar $*" >> "$LOG"
exec "${REAL_TAR:?}" "$@"
EOF
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
      *HEAD:frontend*) echo "${STUB_FRONTEND_TREE:-0123456789abcdef0123456789abcdef01234567}";;
      *) echo "$br";;
    esac;;
  branch) [ "$br" = HEAD ] || echo "$br";;
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
  # curl: serves the files of $STUB_ASSET_DIR by their base name (any URL prefix), 404 for the rest
  # like the real one: with -f/--fail exit 22 and no output file, without it the error page is
  # saved and the exit code is 0. Handles -o/--output (also in a cluster like -fsSLo) and -w
  # '%{http_code}'. STUB_CURL_RC makes every call fail with that code.
  make_stub curl <<'EOF'
echo "curl $*" >> "$LOG"
url=""; out=""; fail=0; wout=""; prev=""
for a in "$@"; do
  case $prev in
    -o|--output) out=$a; prev=""; continue;;
    -w|--write-out) wout=$a; prev=""; continue;;
  esac
  case $a in
    --output|--write-out) prev=$a;;
    --fail|--fail-with-body) fail=1;;
    --*) ;;
    -*) case $a in *f*) fail=1;; esac
        case $a in *o) prev=-o;; *w) prev=-w;; esac;;
    http*|file*) url=$a;;
  esac
done
case "$url" in
  */api/online) echo '{"version":"stub-online"}'; exit "${STUB_CURL_RC:-0}";;
esac
if [ "${STUB_CURL_RC:-0}" != 0 ]; then echo "curl: (${STUB_CURL_RC}) stub failure" >&2; exit "${STUB_CURL_RC}"; fi
src="${STUB_ASSET_DIR:-/nonexistent}/${url##*/}"
if [ -f "$src" ]; then code=200; else code=404; fi
if [ "$code" = 404 ] && [ "$fail" = 1 ]; then
  echo "curl: (22) The requested URL returned error: 404" >&2
  exit 22
fi
# (a binary body is only read into a variable when it goes to stdout: bash warns about NUL bytes)
if [ "$code" != 200 ]; then body="Not Found"; elif [ -z "$out" ]; then body=$(cat "$src"; echo x); body=${body%x}; else body=""; fi
if [ -n "$out" ]; then
  if [ "$code" = 200 ]; then cp "$src" "$out"; else printf '%s' "$body" > "$out"; fi
else
  if [ "$code" = 200 ]; then cat "$src"; else printf '%s' "$body"; fi
fi
[ -n "$wout" ] && printf '%s' "${wout//%\{http_code\}/$code}"
exit 0
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
  make_frontend_asset
}

# The release asset of CI for TREE (default $STUB_FRONTEND_TREE) in $STUB_ASSET_DIR: the files of
# the dist directory at the top level of dpds-frontend-<tree>.tar.gz, plus <asset>.sha256 in the
# format of `sha256sum` ("<hash>  <asset>"). Runs with the real tools, outside run_dpds.
make_frontend_asset() {
  local tree="${1:-$STUB_FRONTEND_TREE}" asset src
  asset="dpds-frontend-$tree.tar.gz"
  src="$T/dist-src-$tree"
  mkdir -p "$STUB_ASSET_DIR" "$src/assets"
  echo "<html><!-- STUB-FRONTEND-INDEX $tree --></html>" >"$src/index.html"
  echo "console.log('stub')" >"$src/assets/app.js"
  tar -C "$src" -czf "$STUB_ASSET_DIR/$asset" .
  (cd "$STUB_ASSET_DIR" && sha256sum "$asset" >"$asset.sha256")
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
  assert_out_has "stub-online"
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

# ------------------------------------------------------------------ tests: spec v13 (frontend asset, no docker)
ASSET="dpds-frontend-$DEFAULT_TREE.tar.gz"

test_install_asks_git_for_the_frontend_tree_in_the_repo_root() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^git rev-parse( --verify)? HEAD:frontend$'
  assert_dir_of '^git rev-parse' "$REPO_ROOT"
}

test_install_downloads_the_asset_of_the_tree_and_its_checksum() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has "^curl .*$ASSET_BASE_URL/$ASSET( |\$)"
  assert_log_has "^curl .*$ASSET_BASE_URL/$ASSET\.sha256( |\$)"
}

test_the_asset_name_follows_the_tree_hash_of_git() {
  export STUB_FRONTEND_TREE=fedcba9876543210fedcba9876543210fedcba98
  make_frontend_asset
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has "^curl .*/dpds-frontend-fedcba9876543210fedcba9876543210fedcba98\.tar\.gz( |\$)"
  assert_log_has "^curl .*/dpds-frontend-fedcba9876543210fedcba9876543210fedcba98\.tar\.gz\.sha256( |\$)"
  assert_log_lacks "$DEFAULT_TREE"
}

test_the_asset_url_base_can_be_overridden() {
  export DPDS_FRONTEND_ASSET_URL=https://mirror.example/dl
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has "^curl .*https://mirror\.example/dl/$ASSET( |\$)"
  assert_log_has "^curl .*https://mirror\.example/dl/$ASSET\.sha256( |\$)"
  assert_log_lacks 'github\.com'
}

test_a_trailing_slash_on_the_asset_url_base_is_fine() {
  export DPDS_FRONTEND_ASSET_URL=https://mirror.example/dl/
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has "^curl .*https://mirror\.example/dl/$ASSET( |\$)"
  assert_log_lacks 'dl//'
}

test_install_fetches_the_frontend_before_building_anything() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_before '^git rev-parse.*HEAD:frontend' "^curl .*$ASSET"
  assert_before "^curl .*$ASSET" '^cargo build'
}

test_the_controller_is_built_with_the_unpacked_dist() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_dist_has "^cargo build .*(-p|--package|--bin)[ =]dpds-controller( .*)? :: DPDS_FRONTEND_DIST=/[^ ]+ index=<html><!-- STUB-FRONTEND-INDEX $DEFAULT_TREE --></html> app=yes\$"
}

test_the_unpacked_dist_is_not_inside_the_repository_frontend_dir() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  local d; d="$(dist_of_controller_build)"
  [[ -n "$d" && "$d" != "<unset>" ]] || { fail "the controller build got no DPDS_FRONTEND_DIST"; return; }
  [[ "$d" != "$REPO_ROOT/frontend"* ]] || fail "dist unpacked into the repository: $d"
}

test_install_removes_its_download_and_unpack_dirs() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_no_temp_leftovers
}

test_install_answer_n_still_cleans_up() {
  feed $'n\n'
  run_dpds install
  assert_rc 1
  assert_no_temp_leftovers
}

test_update_looks_up_the_frontend_tree_after_pulling() {
  fake_install
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_before '^git pull --ff-only$' '^git rev-parse.*HEAD:frontend'
  assert_before '^git pull --ff-only$' "^curl .*$ASSET"
  assert_before "^curl .*$ASSET" '^cargo build'
  assert_dist_has "^cargo build .*dpds-controller( .*)? :: DPDS_FRONTEND_DIST=/[^ ]+ index=.*STUB-FRONTEND-INDEX $DEFAULT_TREE.* app=yes\$"
  assert_no_temp_leftovers
}

# ---- checksum

test_a_tampered_tarball_aborts_naming_the_asset() {
  printf 'tampered' >>"$STUB_ASSET_DIR/$ASSET"
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "$ASSET"
  grep -Eiq 'checksum|sha256' <<<"$ERR" || fail "stderr does not mention the checksum: $ERR"
  assert_log_lacks '^cargo build'
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
  assert_log_lacks '^systemctl'
  assert_no_temp_leftovers
}

test_a_wrong_garbage_or_empty_checksum_file_aborts() {
  local content
  for content in "$(printf '0%.0s' {1..64})  $ASSET" "Not Found" ""; do
    : >"$STUB_LOG"
    printf '%s\n' "$content" >"$STUB_ASSET_DIR/$ASSET.sha256"
    run_dpds --yes install
    assert_rc_nonzero "checksum file '$content'"
    assert_err_has "$ASSET"
    assert_log_lacks '^cargo build'
    assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
    assert_no_temp_leftovers
  done
}

test_update_aborts_on_a_checksum_mismatch_before_touching_the_service() {
  fake_install
  export STUB_ACTIVE=1
  printf 'tampered' >>"$STUB_ASSET_DIR/$ASSET"
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "$ASSET"
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable)'
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" || fail "binary was replaced"
}

# ---- missing asset and download failures

test_a_missing_asset_stops_with_a_hint_to_push_and_to_set_the_dist_dir() {
  rm -f "$STUB_ASSET_DIR"/*
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "$ASSET"
  grep -Eiq 'push' <<<"$ERR" || fail "stderr does not say to push: $ERR"
  grep -Eq '\bCI\b' <<<"$ERR" || fail "stderr does not mention CI: $ERR"
  assert_err_has "DPDS_FRONTEND_DIST"
  assert_log_lacks '^cargo build'
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
  assert_log_lacks '^systemctl'
  assert_no_temp_leftovers
}

test_a_missing_checksum_asset_stops_too() {
  rm -f "$STUB_ASSET_DIR/$ASSET.sha256"
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "$ASSET"
  assert_log_lacks '^cargo build'
  assert_no_temp_leftovers
}

test_update_with_a_missing_asset_stops_before_building_and_leaves_the_service_alone() {
  fake_install
  export STUB_ACTIVE=1
  rm -f "$STUB_ASSET_DIR"/*
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "DPDS_FRONTEND_DIST"
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable)'
}

test_a_failing_download_stops_the_install() {
  export STUB_CURL_RC=7
  run_dpds --yes install
  assert_rc_nonzero
  assert_log_lacks '^cargo build'
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_temp_leftovers
}

test_a_failing_tree_lookup_stops_the_install() {
  export STUB_GIT_FAIL_SUB=rev-parse
  run_dpds --yes install
  assert_rc_nonzero
  assert_log_lacks '^cargo build'
  assert_log_lacks '^curl '
}

# ---- DPDS_FRONTEND_DIST

test_a_dist_dir_from_the_user_skips_the_download_and_is_used_as_is() {
  make_local_dist
  rm -f "$STUB_ASSET_DIR"/*
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_lacks '^curl '
  assert_dist_has "^cargo build .*(-p|--package|--bin)[ =]dpds-controller( .*)? :: DPDS_FRONTEND_DIST=$LOCAL_DIST index=<html>LOCAL-DIST</html> app=yes\$"
  assert_file "$LOCAL_DIST/index.html"
  assert_file "$LOCAL_DIST/assets/app.js"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

test_update_with_a_dist_dir_from_the_user_skips_the_download() {
  fake_install
  make_local_dist
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_lacks '^curl '
  assert_dist_has "^cargo build .*dpds-controller( .*)? :: DPDS_FRONTEND_DIST=$LOCAL_DIST index=<html>LOCAL-DIST</html> app=yes\$"
  assert_file "$LOCAL_DIST/index.html"
}

# ------------------------------------------------------------------ tests: spec v13b item 3 (review fixes)
# The asset-fetching curl calls of the log (not the status probe).
asset_curl_calls() { grep -E '^curl .*dpds-frontend-' "$STUB_LOG" 2>/dev/null; }
TAR_LIST_RE='^tar (.* )?(-[a-zA-Z]*t[a-zA-Z]*|--list)( |$)'
TAR_EXTRACT_RE='^tar (.* )?(-[a-zA-Z]*x[a-zA-Z]*|--extract)( |$)'

# make_unsafe_asset KIND: the asset of the default tree with an index.html and assets/app.js plus one
# bad member (absolute | dotdot | symlink | hardlink), or a harmless one (dots-in-name) for the
# negative case. Nothing extracts it here; GNU tar strips or refuses the bad names when it does.
make_unsafe_asset() {
  local kind="$1" asset="dpds-frontend-$STUB_FRONTEND_TREE.tar.gz" src="$T/unsafe-src"
  rm -rf "${src:?}"; mkdir -p "$src/assets"
  echo "<html><!-- STUB-FRONTEND-INDEX --></html>" >"$src/index.html"
  echo "console.log('stub')" >"$src/assets/app.js"
  case "$kind" in
    absolute)
      echo evil >"$src/evil.js"
      "$REAL_TAR" -C "$src" -P --transform "s,^\./evil\.js\$,$T/abs-escape/evil.js," -czf "$STUB_ASSET_DIR/$asset" . ;;
    dotdot)
      echo evil >"$src/evil.js"
      "$REAL_TAR" -C "$src" --transform 's,^\./evil\.js$,../evil.js,' -czf "$STUB_ASSET_DIR/$asset" . ;;
    symlink)
      ln -s /etc/passwd "$src/assets/evil-link"
      "$REAL_TAR" -C "$src" -czf "$STUB_ASSET_DIR/$asset" . ;;
    hardlink)
      ln "$src/index.html" "$src/assets/hard.js"
      "$REAL_TAR" -C "$src" -czf "$STUB_ASSET_DIR/$asset" . ;;
    dots-in-name)
      echo ok >"$src/assets/app..min.js"
      "$REAL_TAR" -C "$src" -czf "$STUB_ASSET_DIR/$asset" . ;;
  esac
  (cd "$STUB_ASSET_DIR" && sha256sum "$asset" >"$asset.sha256")
}

test_a_dist_dir_from_the_user_without_index_html_fails_before_any_cargo_build() {
  local d
  for d in "$T/emptydist" "$T/not-there"; do
    : >"$STUB_LOG"
    [[ "$d" == "$T/emptydist" ]] && mkdir -p "$d"
    DPDS_FRONTEND_DIST="$d" run_dpds --yes install
    assert_rc_nonzero "DPDS_FRONTEND_DIST=$d"
    assert_err_has "$d"
    assert_err_has "index.html"
    assert_log_lacks '^cargo build'
    assert_log_lacks '^curl '
    assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
    assert_no_file "$(installed_unit)"
    assert_log_lacks '^systemctl'
  done
}

test_update_with_a_dist_dir_without_index_html_builds_nothing_and_leaves_the_service() {
  fake_install
  export STUB_ACTIVE=1
  mkdir -p "$T/emptydist"
  DPDS_FRONTEND_DIST="$T/emptydist" run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "$T/emptydist"
  assert_log_lacks '^cargo build'
  assert_log_lacks '^systemctl (enable|disable)'
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" || fail "binary was replaced"
}

test_install_with_local_changes_in_frontend_warns_and_names_the_dist_variable() {
  export STUB_GIT_DIRTY_FRONTEND=1
  run_dpds --yes install
  assert_rc 0 "a dirty frontend/ is a warning only: $ERR"
  assert_log_has '^git status .*--porcelain.* -- frontend$'
  grep -Eq '^\[WARN' <<<"$ERR" || fail "no warning on stderr: $ERR"
  grep -E '^\[WARN' <<<"$ERR" | grep -q 'DPDS_FRONTEND_DIST' || fail "no warning names DPDS_FRONTEND_DIST: $ERR"
  grep -E '^\[WARN' <<<"$ERR" | grep -Eiq 'uncommitted|local change|modified|dirty' || fail "the warning does not say what is wrong: $ERR"
  assert_dist_has "^cargo build .*dpds-controller( .*)? :: DPDS_FRONTEND_DIST=/[^ ]+ index=.*STUB-FRONTEND-INDEX $DEFAULT_TREE.* app=yes\$"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

test_install_with_a_clean_frontend_gives_no_dist_warning() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has '^git status .*--porcelain.* -- frontend$'
  ! grep -q 'DPDS_FRONTEND_DIST' <<<"$ERR" || fail "unexpected warning on a clean tree: $ERR"
}

test_a_missing_checksum_file_is_reported_as_a_partial_upload_not_as_a_missing_asset() {
  rm -f "${STUB_ASSET_DIR:?}/${ASSET:?}.sha256"
  assert_file "$STUB_ASSET_DIR/$ASSET"
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "$ASSET"
  grep -Eiq '(checksum|\.sha256)[^.]*(missing|not (found|there|uploaded))|missing[^.]*(checksum|\.sha256)' <<<"$ERR" \
    || fail "stderr does not say that the checksum file is missing: $ERR"
  grep -Eq '\bCI\b' <<<"$ERR" || fail "stderr does not mention CI (re-run it or wait): $ERR"
  ! grep -qi 'not available' <<<"$ERR" || fail "a missing checksum file is not 'not available': $ERR"
  assert_log_lacks '^cargo build'
  assert_no_temp_leftovers
}

test_a_missing_tarball_still_says_the_asset_is_not_available() {
  rm -f "${STUB_ASSET_DIR:?}"/*
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "not available"
}

test_https_asset_urls_pin_curl_to_https_including_redirects() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  local n=0 line
  while IFS= read -r line; do
    n=$((n + 1))
    grep -Eq -- "--proto[ =]=https( |\$)" <<<"$line" || fail "no --proto =https: $line"
    grep -Eq -- "--proto-redir[ =]=https( |\$)" <<<"$line" || fail "no --proto-redir =https: $line"
  done < <(asset_curl_calls)
  (( n >= 2 )) || fail "expected a curl call for the tarball and one for the checksum, saw $n"
}

test_an_overridden_https_base_is_pinned_too() {
  export DPDS_FRONTEND_ASSET_URL=https://mirror.example/dl/
  run_dpds --yes install
  assert_rc 0 "$ERR"
  local n=0 line
  while IFS= read -r line; do
    n=$((n + 1))
    grep -Eq -- "--proto[ =]=https( |\$)" <<<"$line" || fail "no --proto =https: $line"
    grep -Eq -- "--proto-redir[ =]=https( |\$)" <<<"$line" || fail "no --proto-redir =https: $line"
  done < <(asset_curl_calls)
  (( n >= 2 )) || fail "expected two asset curl calls, saw $n"
}

test_an_explicit_http_base_gets_no_proto_flags() {
  export DPDS_FRONTEND_ASSET_URL=http://127.0.0.1:9/dl
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has "^curl .*http://127\.0\.0\.1:9/dl/$ASSET( |\$)"
  assert_log_lacks '^curl .*--proto'
}

test_the_tarball_is_extracted_with_no_same_owner() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  local n=0 line
  while IFS= read -r line; do
    n=$((n + 1))
    grep -Eq -- ' --no-same-owner( |$)' <<<"$line" || fail "tar extraction without --no-same-owner: $line"
  done < <(grep -E -- "$TAR_EXTRACT_RE" "$STUB_LOG")
  (( n >= 1 )) || fail "no tar extraction logged"
}

test_the_tarball_is_listed_before_it_is_extracted() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has "$TAR_LIST_RE"
  assert_before "$TAR_LIST_RE" "$TAR_EXTRACT_RE"
}

# One test per kind of bad member: refused, named, nothing extracted, built or installed.
assert_unsafe_asset_refused() {
  assert_rc_nonzero
  assert_err_has "$ASSET"
  grep -Eiq 'absolute|\.\.|link|unsafe|refus|not allowed|suspicious' <<<"$ERR" || fail "stderr does not say why the asset is refused: $ERR"
  assert_log_has "$TAR_LIST_RE"
  assert_log_lacks "$TAR_EXTRACT_RE"
  assert_log_lacks '^cargo build'
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" 2>/dev/null || fail "a binary was installed"
  assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
  assert_no_file "$T/abs-escape/evil.js"
  assert_no_temp_leftovers
}

test_a_tarball_with_an_absolute_path_is_refused() {
  make_unsafe_asset absolute
  run_dpds --yes install
  assert_unsafe_asset_refused
}

test_a_tarball_with_a_dotdot_component_is_refused() {
  make_unsafe_asset dotdot
  run_dpds --yes install
  assert_unsafe_asset_refused
}

test_a_tarball_with_a_symlink_is_refused() {
  make_unsafe_asset symlink
  run_dpds --yes install
  assert_unsafe_asset_refused
}

test_a_tarball_with_a_hardlink_is_refused() {
  make_unsafe_asset hardlink
  run_dpds --yes install
  assert_unsafe_asset_refused
}

test_update_refuses_an_unsafe_tarball_before_touching_the_service() {
  fake_install
  export STUB_ACTIVE=1
  make_unsafe_asset symlink
  run_dpds --yes update
  assert_unsafe_asset_refused
  assert_log_lacks '^systemctl (enable|disable)'
}

test_a_file_name_that_merely_contains_two_dots_is_not_a_dotdot_component() {
  make_unsafe_asset dots-in-name
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_has "$TAR_EXTRACT_RE"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

test_the_readme_states_the_trust_model_checksum_detects_corruption_only() {
  # one paragraph says that the checksum only detects corruption and what authenticity rests on
  awk -v RS= '{ t = tolower($0) } t ~ /checksum/ && t ~ /corruption/ && t ~ /https/ && t ~ /github/ { f = 1 } END { exit !f }' "$REPO_ROOT/README.md" \
    || fail "README.md has no paragraph on the trust model (checksum only detects corruption; GitHub and HTTPS carry the authenticity)"
}

# ---- required commands

test_install_needs_curl_sha256sum_and_tar() {
  local t
  for t in curl sha256sum tar; do
    setup_sandbox
    remove_tool "$t"
    run_dpds --yes install
    assert_rc_nonzero "without $t"
    assert_err_word "$t"
    assert_log_lacks '^cargo build'
    assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
    assert_no_file "$(installed_unit)"
  done
}

test_update_needs_curl_sha256sum_and_tar() {
  local t
  for t in curl sha256sum tar; do
    setup_sandbox
    fake_install
    remove_tool "$t"
    run_dpds --yes update
    assert_rc_nonzero "without $t"
    assert_err_word "$t"
    assert_log_lacks '^cargo build'
    assert_log_lacks '^git (checkout|pull)'
  done
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
