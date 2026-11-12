#!/usr/bin/env bash
# Tests for dpds.sh (repo root) and packaging/dpds-controller.service.
#
# Run:  bash packaging/tests/dpds_sh.test.sh [test_name_substring ...]
# Plain bash, no dependencies (no bats, no shellcheck). Exit 0 iff every test passes.
#
# =====================================================================================
# CONTRACT (the tests enforce it; dpds.sh must satisfy it)
# =====================================================================================
#
# Command line:   dpds.sh [--nightly|--stable] [--yes] COMMAND
#   COMMAND is one of: check install update start stop restart status uninstall.
#   Flags come before the command (any order between them).
#   - no arguments, unknown command or unknown option: exit 1, usage text (containing the word
#     "Usage" and every command name) on stdout or stderr, nothing else is executed. The usage text
#     (also for --help, exit 0) mentions no docker, compose, container or image, no download,
#     CI-built frontend, release asset or DPDS_FRONTEND_DIST; it mentions Node or npm.
#   - Release channel: DPDS_CHANNEL is `latest` (the default; also when empty or unset) or
#     `nightly`. `--nightly` selects nightly and `--stable` selects latest, both override the
#     variable (even an invalid one); both flags together: exit non-zero naming both, nothing
#     executed. Any other value (`beta`, `Nightly`, `main`, ...): exit non-zero, stderr "Invalid
#     DPDS channel '<value>' (expected 'latest', 'nightly' or a release tag like 'v1.0.0')", before
#     any git, cargo, npm or systemctl call, for check, install and update. The channel's branch is
#     `main` for latest and `nightly` for nightly.
#   - Release tag channel: DPDS_CHANNEL (the variable only; the flags still override it) also
#     accepts a release tag matching ^v[0-9]+\.[0-9]+\.[0-9]+$ (`v1.0.0`). update then:
#     prerequisites, dirty-tree refusal, `git fetch --tags`, the tag lookup
#     `git rev-parse -q --verify "refs/tags/<tag>^{commit}"` (a missing tag is an error naming it,
#     nothing changed), `git checkout --detach <tag>`, no `git pull`, then rebuild, reinstall and
#     restart. install stays read-only and warns (naming the tag and HEAD, telling to run `update`)
#     when HEAD is not the tag's commit. status prints `Channel: <tag> (tag <tag>), checkout: ...`.
#     The git stub answers the tag lookup with `commit-of-<tag>` for tags listed in STUB_GIT_TAGS
#     (else exit 1) and `rev-parse HEAD` with STUB_GIT_HEAD_COMMIT.
#   - update on a branch channel: after the prerequisites and the dirty-tree refusal, `git fetch`,
#     `git checkout <branch>`, `git pull --ff-only` (repo root, in this order, each failing step
#     aborts before the frontend, any build and any systemctl change), then the frontend and cargo
#     builds. A warning names the checked-out branch and the channel's branch when they differ
#     ("detached" for a detached HEAD) and update switches anyway. No tag lookup: no `git tag`, no
#     `git fetch --tags`, no checkout of a v* tag.
#   - install makes only read-only git calls (to find the checked-out branch): no fetch, checkout,
#     pull or tag. On another branch than the channel's it warns naming both branches and telling
#     to run `update`, and installs anyway. A tree without git (a tarball) is not an error. No
#     warning when the branch matches.
#   - The git stub's current branch is `main` unless STUB_GIT_BRANCH says otherwise (`HEAD` =
#     detached); STUB_GIT_NOT_A_REPO makes every git call fail like outside a repository;
#     STUB_GIT_TAGS lists tags (they must not matter on a branch channel).
#   - `status` prints the release channel and the branch of the checkout (a line with the word
#     "channel" and its value, and the branch name; exit 0 even without git), and the installed
#     version taken from the /api/online answer ("version") on a line of its own besides the raw
#     answer, also when the key is not the first one. Nothing reads the version from jq (not on
#     the PATH) or from the repository.
#   - Errors go to stderr. The script may be started from any working directory: it works on the
#     repository it lives in (cargo, npm and git run there; npm in its frontend/ directory).
#   - No command ever calls docker (the stub is a tripwire), and the script has no docker, compose
#     or nginx code, no DPDS_FRONTEND_BIND/DPDS_FRONTEND_PORT and no DPDS_CONTROLLER setting
#     (DPDS_CONTROLLER_BIND of the env file stays).
#   - Root: every command, with any flags, refuses to run when EUID is 0 AND SUDO_USER is set
#     (`sudo ./dpds.sh ...`), at once, before any tool or sudo call and before anything is written:
#     non-zero exit and a stderr message with "as your user", "not with sudo" and "PATH". Plain
#     root (EUID 0, no SUDO_USER: a container) is not refused and `run_privileged` runs the command
#     directly, without calling sudo. A non-root user is not refused, even with SUDO_USER left in
#     the environment. The tests become root through `sudo -n` (EUID cannot be faked) and print SKIP
#     without it. Run the suite itself as a normal user.
#   - Not installed: `start`, `restart` and `status` check that the unit file of `install`
#     ($DPDS_SYSTEMD_DIR/dpds-controller.service) exists. Without it they say "dpds-controller is
#     not installed: run ./dpds.sh install first" (start and restart on stderr with a non-zero
#     exit, status with exit 0) and show none of systemctl's own complaints ("does not exist",
#     "not found", "could not be found"). The stub systemctl answers like real systemd for a
#     missing unit file, so tests of start/restart/status call `seed_unit` (or fake_install)
#     first. `stop` and `uninstall` do not need the unit.
#
# Network mode (virtual network spec): `--network physical|virtual` (flag before the command, with a
#   separate value), accepted only with install and update (any other command, a missing or another value: exit 1,
#   error naming --network, nothing executed). The mode is stored in $DPDS_ETC_DIR/network, exactly one line
#   `physical` or `virtual`. install/update without the flag keep the stored mode, with it they switch; without a
#   stored mode (no file, or content other than physical/virtual) and without the flag they fail before any npm or
#   cargo build ("No network mode is stored ... --network"). The file is written by install_flow (also via update)
#   and only once the installation goes ahead: a declined confirmation leaves it alone. uninstall keeps it.
#   The controller's host check (`dpds-controller check`, also the one of install) judges the mode being installed
#   and gets `--etc-dir $DPDS_ETC_DIR` when DPDS_ETC_DIR is not /etc/dpds (and no --etc-dir otherwise). The stub
#   controller logs the mode it finds in that etc dir to $STUB_LOG.network.
#   `check` in a stored virtual mode additionally requires the af_packet driver of DPDK: a librte_net_af_packet.a or
#   librte_net_af_packet.so* below `pkg-config --variable=libdir libdpdk` or its dpdk/pmds-*/ subdirectory
#   (the stub answers STUB_DPDK_LIBDIR, a directory the sandbox creates), else non-zero with an error naming net_af_packet.
# No systemd: systemd is detected by the directory $DPDS_SYSTEMD_RUN_DIR (default /run/systemd/system; the sandbox points
#   it at an existing directory, the no-systemd tests remove it). Without it install does everything except installing the
#   unit and `systemctl daemon-reload`, update does not query systemctl, start/stop/restart/status call no systemctl, and
#   install, start, restart and status print how to start the controller by hand:
#   `DPDS_CONTROLLER_BIND=<bind> DPDS_CONTROLLER_DPDS_BIN=<bindir>/dpds <bindir>/dpds-controller serve`. start and
#   restart then require the installed binaries instead of the unit file.
#
# Environment overrides (so tests never touch the real system):
#   DPDS_PREFIX       install prefix; binaries go to $DPDS_PREFIX/bin     (default /usr/local)
#   DPDS_ETC_DIR      config dir; env file is $DPDS_ETC_DIR/controller.env (default /etc/dpds)
#   DPDS_SYSTEMD_DIR  where the unit is installed                         (default /etc/systemd/system)
#   DPDS_SYSTEMD_RUN_DIR  the directory whose existence means "systemd runs" (default /run/systemd/system)
#   DPDS_BIND         bind address written into a NEW env file            (default 0.0.0.0:8100)
#   CARGO_TARGET_DIR  standard cargo variable; built binaries are read from
#                     $CARGO_TARGET_DIR/release/{dpds,dpds-controller}    (default <repo>/target)
#   The controller stores nothing: a DPDS_STATE_DIR in the environment is ignored, whatever it
#   holds. A DPDS_FRONTEND_DIST in the caller's environment is ignored by install and update.
#   Privileged operations go through sudo when EUID != 0.
#
# Prerequisites (checked by `check`, `install`, `update`; NOT by start/stop/restart/status/
# uninstall, which only need systemctl and, for status, curl). The script never installs
# anything. Every unmet prerequisite is an ERROR line on stderr that names the tool, says what to
# install ("install" in the line, for a missing tool) and, for a version, the required version;
# ALL unmet prerequisites are reported (one run, several ERROR lines), then the command exits
# non-zero. Met ones are OK lines (stdout or stderr).
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
#                                        "engines.node" (">=26.0" today; compared numerically
#                                        per component, "9" < "26" < "100"); an older node is an
#                                        error naming node and the required version (26.0).
#   curl, sha256sum, tar                 NOT prerequisites of anything but (curl) `status`.
#   install and update run all of this before any git call, cargo build, npm call or systemctl call.
#
# check
#   Prerequisites, then runs `dpds-controller check` and passes its stdout and exit code through (when
#   a prerequisite is unmet, `dpds-controller check` is not run).
#   Binary used: $DPDS_PREFIX/bin/dpds-controller if executable; otherwise the one in
#   $CARGO_TARGET_DIR/release if present; otherwise it first runs
#   `cargo build --release -p dpds-controller` (in the repo root) and uses that. That build runs
#   no npm and downloads nothing.
#
# frontend (install and update). The controller embeds the web frontend at compile time. install and
#   update build it from the checked-out tree (for update after checkout/pull, local changes
#   included, no warning about them), before ANY cargo build:
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
#      (sudo when EUID != 0) so root-only hints work, with no state argument; its output is shown.
#      Any non-zero exit of it only gives a warning that names the exit code (hints, not blockers),
#      then the prompt (--yes skips it). The stub controller takes STUB_CONTROLLER_RC and
#      STUB_CONTROLLER_ERR (a message for stderr).
#   3. unless --yes: asks "continue?" on stdin. y/yes (any case) continues; anything else
#      including EOF aborts: exit 1, nothing installed, no daemon-reload.
#   4. installs dpds and dpds-controller (executable) into $DPDS_PREFIX/bin; the unit from
#      packaging/dpds-controller.service into $DPDS_SYSTEMD_DIR/dpds-controller.service with
#      `ExecStart=$DPDS_PREFIX/bin/dpds-controller serve` and
#      `EnvironmentFile=-$DPDS_ETC_DIR/controller.env` substituted for the defaults;
#      creates $DPDS_ETC_DIR/controller.env ONLY if absent (never overwritten) containing
#        DPDS_CONTROLLER_BIND=<DPDS_BIND or 0.0.0.0:8100>
#        DPDS_CONTROLLER_DPDS_BIN=$DPDS_PREFIX/bin/dpds
#      (values may be double-quoted). No state directory is created or named, no
#      DPDS_CONTROLLER_STATE_DIR is written, the unit has no StateDirectory= or state path.
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
#          refused (error, non-zero, before any checkout/pull/build); then the git steps (see above);
#          then the frontend build for the pulled tree and the install flow (same prompt unless
#          --yes); then, only if the service was active, restart (= stop + start). If it was not
#          active nothing is started.
# uninstall
#          `systemctl disable --now dpds-controller` (failure tolerated),
#          removes $DPDS_PREFIX/bin/{dpds,dpds-controller}, the unit and the stamp, `systemctl daemon-reload`.
#          KEEPS $DPDS_ETC_DIR/controller.env untouched. Nothing installed => still exit 0.
#
# Stamp file $DPDS_ETC_DIR/installed-commit (default /etc/dpds/installed-commit; its name has no "state"): one line, the full
#   `git rev-parse HEAD` of the installed tree. install_flow removes any existing stamp right before it copies the binaries
#   (after the host check and the confirmation), and writes the new one after everything else (binaries, unit, env file,
#   daemon-reload) succeeded, so a failed install never leaves a stamp claiming success. A tree without git writes no stamp and
#   is not an error; a declined confirmation or a failed build leaves the existing stamp alone.
#   update: when the stamp equals HEAD and $BIN_DIR/dpds, $BIN_DIR/dpds-controller and the unit file exist, it prints an info
#   line (naming the commit; saying it is already up to date and that `./dpds.sh install` rebuilds anyway) and exits 0 with
#   no npm, no cargo, no host check, no question, no install of binaries/unit/env and no systemctl call that changes anything
#   (the service stays as it was). The git steps still run. Otherwise it proceeds as usual and writes the stamp. install always
#   rebuilds and reinstalls and writes the stamp.
#
# Never tunes the host: no ethtool, dcb, setpci, sysctl, mlxconfig, mstconfig, cpupower, hugepage/proc/sys writes,
# neither in the script nor in the unit file. The controller's `check` prints the root
# commands an administrator would run, indented under each hint after a `# run as root:` line, and dpds.sh shows the
# controller's lines exactly as they are (the stub controller below answers STUB_CONTROLLER_HINT_TEXT, which may span
# several lines). dpds.sh never runs those commands and never calls the controller's routes.
#
# packaging/dpds-controller.service
#   runs as root (no User= or User=root), `EnvironmentFile=-/etc/dpds/controller.env`,
#   ExecStart=/usr/local/bin/dpds-controller serve, KillSignal=SIGTERM, TimeoutStopSec >= 120 s,
#   LimitMEMLOCK=infinity in [Service] (dpds locks its memory), Restart=on-failure,
#   [Install] WantedBy=multi-user.target; parses without warnings under `systemd-analyze verify`.
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
#   node: `node --version` prints v$STUB_NODE_VERSION (default 26.13.0). npm: `npm --version`, and
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
TUNING_TOOLS="ethtool dcb setpci sysctl mlxconfig mstconfig mlxfwreset mstfwreset cpupower tuned-adm grubby update-grub modprobe insmod"
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
# No state directory was created or named by a privileged call.
assert_no_state_dir_anywhere() {
  [[ -z "$(find "$T" -iname '*state*' -not -path "$T/stubs/*" 2>/dev/null)" ]] \
    || fail "a state directory or file was created: $(find "$T" -iname '*state*' -not -path "$T/stubs/*" | tr '\n' ' ')"
  ! grep -qiE 'state|var/lib' "$STUB_LOG.sudo" 2>/dev/null \
    || fail "a privileged call mentions a state directory: $(grep -iE 'state|var/lib' "$STUB_LOG.sudo" | tr '\n' '|')"
}
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
if [ "${1:-}" = check ] || [ "${1:-}" = serve ]; then
  d=""; p=""
  for a in "$@"; do case "$a" in --etc-dir=*) d=${a#*=};; esac; [ "$p" = --etc-dir ] && d=$a; p=$a; done
  if [ -z "$d" ]; then echo NOETCDIR >> "${STUB_LOG}.network"
  elif [ -f "$d/network" ]; then cat "$d/network" >> "${STUB_LOG}.network"
  else echo MISSING >> "${STUB_LOG}.network"; fi
fi
if [ "${1:-}" = check ]; then
  echo "CHECK-OUTPUT-MARKER"
  [ -n "${STUB_CONTROLLER_HINT_TEXT:-}" ] && echo "$STUB_CONTROLLER_HINT_TEXT"
  [ -n "${STUB_CONTROLLER_ERR:-}" ] && echo "$STUB_CONTROLLER_ERR" >&2
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
        STUB_CONTROLLER_RC STUB_CONTROLLER_ERR STUB_CONTROLLER_HINT_TEXT STUB_CARGO_FAIL STUB_SYSTEMCTL_RC STUB_SYSTEMCTL_FAIL_SUB \
        STUB_GIT_FAIL_SUB STUB_CURL_RC DPDS_BIND STUB_GIT_DIRTY STUB_GIT_DIRTY_FRONTEND STUB_GIT_BRANCH \
        STUB_GIT_TAGS STUB_GIT_HEAD_COMMIT STUB_GIT_NOT_A_REPO DPDS_CHANNEL STUB_ONLINE_JSON STUB_NODE_VERSION STUB_NPM_FAIL_SUB STUB_NPM_NO_INDEX STUB_NPM_BUILD_NOOP STUB_DPDK_VERSION STUB_RUSTC_VERSION \
        DPDS_FRONTEND_DIST DPDS_FRONTEND_ASSET_URL
  mkdir -p "$T/tmp"
  unset DPDS_STATE_DIR DPDS_CONTROLLER_STATE_DIR
  export DPDS_PREFIX="$T/prefix" DPDS_ETC_DIR="$T/etc" DPDS_SYSTEMD_DIR="$T/systemd"
  # systemd "runs" in the sandbox; the no-systemd tests remove this directory
  export DPDS_SYSTEMD_RUN_DIR="$T/run-systemd"
  mkdir -p "$DPDS_SYSTEMD_RUN_DIR"
  # the libdir `pkg-config --variable=libdir libdpdk` answers
  export STUB_DPDK_LIBDIR="$T/dpdk-lib"
  mkdir -p "$STUB_DPDK_LIBDIR"
  export CARGO_TARGET_DIR="$T/target"
  export HOME="$T/home"
  mkdir -p "$DPDS_PREFIX/bin" "$DPDS_SYSTEMD_DIR"
  # a mode is stored, as after an earlier install; tests of the first install remove it
  seed_network physical

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
# A unit that is not installed (no $DPDS_SYSTEMD_DIR/dpds-controller.service) answers like real systemd.
unit_file="${DPDS_SYSTEMD_DIR:-/nonexistent}/dpds-controller.service"
if [ ! -f "$unit_file" ]; then
  case $sub in
    enable) echo "Failed to enable unit: Unit file dpds-controller.service does not exist." >&2; exit 1;;
    start|restart|reload-or-restart|try-restart) echo "Failed to $sub dpds-controller.service: Unit dpds-controller.service not found." >&2; exit 5;;
    status) echo "Unit dpds-controller.service could not be found." >&2; exit 4;;
    cat) echo "No files found for dpds-controller.service." >&2; exit 1;;
    is-enabled) echo "Failed to get unit file state for dpds-controller.service: No such file or directory" >&2; exit 1;;
    show) case " $* " in *" --value "*) echo "not-found";; *) echo "LoadState=not-found";; esac; exit 0;;
    list-unit-files) printf 'UNIT FILE STATE VENDOR PRESET\n\n0 unit files listed.\n'; exit 0;;
  esac
fi
case $sub in
  is-active) if [ "${STUB_ACTIVE:-0}" = 1 ]; then echo active; exit 0; else echo inactive; exit 3; fi;;
  status) echo "STUB-SYSTEMCTL-STATUS"; exit "${STUB_SYSTEMCTL_RC:-0}";;
  show) case " $* " in *" --value "*) echo "loaded";; *) echo "LoadState=loaded";; esac; exit 0;;
  list-unit-files) printf 'UNIT FILE STATE VENDOR PRESET\ndpds-controller.service disabled enabled\n\n1 unit files listed.\n'; exit 0;;
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
[ -n "${STUB_GIT_NOT_A_REPO:-}" ] && { echo "fatal: not a git repository (or any of the parent directories): .git" >&2; exit 128; }
# current branch: STUB_GIT_BRANCH (default main, the branch of the default channel); "HEAD" means a detached HEAD
br="${STUB_GIT_BRANCH:-main}"
[ -n "${STUB_GIT_FAIL_SUB:-}" ] && [ "${1:-}" = "$STUB_GIT_FAIL_SUB" ] && { echo "fatal: stub git failure" >&2; exit 1; }
case "${1:-}" in
  rev-parse)
    # a tag lookup (`rev-parse -q --verify refs/tags/<tag>^{commit}`) answers `commit-of-<tag>` for a tag listed in
    # STUB_GIT_TAGS and fails silently (exit 1, like `-q`) for any other; HEAD (as a commit, not --abbrev-ref) answers
    # STUB_GIT_HEAD_COMMIT (default headcommit-0000). Everything else answers the current branch.
    want=""
    for a in "$@"; do case "$a" in *refs/tags/*) want=${a#*refs/tags/}; want=${want%%^*};; esac; done
    if [ -n "$want" ]; then
      for t in ${STUB_GIT_TAGS:-}; do [ "$t" = "$want" ] && { echo "commit-of-$t"; exit 0; }; done
      exit 1
    fi
    case "$*" in
      *--abbrev-ref*|*--symbolic*|*--show-*|*--is-*|*--git-dir*) echo "$br";;
      *HEAD*) echo "${STUB_GIT_HEAD_COMMIT:-headcommit-0000}";;
      *) echo "$br";;
    esac;;
  describe)
    # `git describe --tags --exact-match`: the tag whose commit is HEAD, else fail like git
    for t in ${STUB_GIT_TAGS:-}; do
      [ "${STUB_GIT_HEAD_COMMIT:-headcommit-0000}" = "commit-of-$t" ] && { echo "$t"; exit 0; }
    done
    echo "fatal: no tag exactly matches 'HEAD'" >&2; exit 128;;
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
for a in "$@"; do case $a in --variable=libdir) echo "${STUB_DPDK_LIBDIR:?}"; exit 0;; esac; done
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
v="${STUB_NODE_VERSION:-26.13.0}"
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
if [ "${1:-}" = check ] || [ "${1:-}" = serve ]; then
  d=""; p=""
  for a in "$@"; do case "$a" in --etc-dir=*) d=${a#*=};; esac; [ "$p" = --etc-dir ] && d=$a; p=$a; done
  if [ -z "$d" ]; then echo NOETCDIR >> "${STUB_LOG}.network"
  elif [ -f "$d/network" ]; then cat "$d/network" >> "${STUB_LOG}.network"
  else echo MISSING >> "${STUB_LOG}.network"; fi
fi
if [ "${1:-}" = check ]; then echo "CHECK-OUTPUT-MARKER"; [ -n "${STUB_CONTROLLER_HINT_TEXT:-}" ] && echo "$STUB_CONTROLLER_HINT_TEXT"; [ -n "${STUB_CONTROLLER_ERR:-}" ] && echo "$STUB_CONTROLLER_ERR" >&2; exit "${STUB_CONTROLLER_RC:-0}"; fi
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
  mkdir -p "$DPDS_ETC_DIR"
  printf 'DPDS_CONTROLLER_BIND=0.0.0.0:8100\n' >"$DPDS_ETC_DIR/controller.env"
  printf '[Service]\nExecStart=%s/bin/dpds-controller serve\n' "$DPDS_PREFIX" >"$DPDS_SYSTEMD_DIR/dpds-controller.service"
}

# The stored network mode of $DPDS_ETC_DIR/network (what the controller reads), and what is stored now.
seed_network() { mkdir -p "$DPDS_ETC_DIR"; printf '%s\n' "$1" >"$DPDS_ETC_DIR/network"; }
stored_network() { cat "$DPDS_ETC_DIR/network" 2>&1; }

# Only the unit file of a finished install: start, restart and status refuse to run without it.
seed_unit() {
  mkdir -p "$DPDS_SYSTEMD_DIR"
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
  # no command ever calls docker
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

# ------------------------------------------------------------------ tests: release channel (DPDS_CHANNEL, --nightly, --stable)
# latest = branch main (the default), nightly = branch nightly; --nightly / --stable override DPDS_CHANNEL. The git stub's
# current branch is `main` unless a test sets STUB_GIT_BRANCH, so with the default channel nothing warns about a switch.

# update on a branch: fetch, checkout BRANCH, pull --ff-only, in this order, before the frontend and cargo builds, in the repo root.
assert_update_follows() {
  local branch="$1" other
  assert_log_has '^git fetch'
  assert_log_has "^git checkout $branch\$"
  assert_log_has '^git pull --ff-only$'
  assert_log_lacks '^git pull$'
  for other in main nightly; do
    [[ "$other" == "$branch" ]] || assert_log_lacks "^git checkout $other\$"
  done
  assert_before '^git fetch' "^git checkout $branch\$"
  assert_before "^git checkout $branch\$" '^git pull --ff-only$'
  assert_before '^git pull --ff-only$' '^npm ci( |$)'
  assert_before '^git pull --ff-only$' '^cargo build'
  assert_dir_of '^git (fetch|checkout|pull)' "$REPO_ROOT"
  # no tag lookup on a branch channel
  assert_log_lacks '^git tag'
  assert_log_lacks '--tags'
  assert_log_lacks '^git checkout (--detach )?v[0-9]'
}

# --- which branch ---
test_update_without_any_channel_setting_follows_main() {
  fake_install
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_update_follows main
}

test_update_on_the_latest_channel_from_the_environment_follows_main() {
  fake_install
  DPDS_CHANNEL=latest run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_update_follows main
}

test_update_on_the_nightly_channel_from_the_environment_follows_nightly() {
  fake_install
  export STUB_GIT_BRANCH=nightly DPDS_CHANNEL=nightly
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_update_follows nightly
}

test_stable_flag_is_the_latest_channel_main() {
  fake_install
  run_dpds --stable --yes update
  assert_rc 0 "$ERR"
  assert_update_follows main
}

test_nightly_flag_follows_the_nightly_branch() {
  fake_install
  export STUB_GIT_BRANCH=nightly
  run_dpds --nightly --yes update
  assert_rc 0 "$ERR"
  assert_update_follows nightly
}

test_an_empty_channel_variable_means_the_default() {
  fake_install
  DPDS_CHANNEL="" run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_update_follows main
}

test_the_flags_override_the_environment_both_ways() {
  fake_install
  export STUB_GIT_BRANCH=main
  DPDS_CHANNEL=nightly run_dpds --stable --yes update
  assert_rc 0 "$ERR"
  CTX="DPDS_CHANNEL=nightly --stable" assert_update_follows main
  : >"$STUB_LOG"
  rm -f "$(stamp_file)"
  export STUB_GIT_BRANCH=nightly
  DPDS_CHANNEL=latest run_dpds --nightly --yes update
  assert_rc 0 "$ERR"
  CTX="DPDS_CHANNEL=latest --nightly" assert_update_follows nightly
  : >"$STUB_LOG"
  rm -f "$(stamp_file)"
  # the same value as the flag
  DPDS_CHANNEL=nightly run_dpds --nightly --yes update
  assert_rc 0 "$ERR"
  CTX="DPDS_CHANNEL=nightly --nightly" assert_update_follows nightly
}

test_a_flag_wins_even_over_an_invalid_environment_value() {
  fake_install
  export STUB_GIT_BRANCH=nightly
  DPDS_CHANNEL=beta run_dpds --nightly --yes update
  assert_rc 0 "$ERR"
  assert_update_follows nightly
}

test_both_flags_are_an_error_in_either_order_and_run_nothing() {
  local args
  for args in "--nightly --stable" "--stable --nightly"; do
    : >"$STUB_LOG"
    # shellcheck disable=SC2086
    DPDS_CHANNEL=nightly run_dpds $args update
    CTX="$args" assert_rc_nonzero
    CTX="$args" assert_err_has "--nightly"
    CTX="$args" assert_err_has "--stable"
    CTX="$args" assert_log_empty
  done
}

# --- invalid values ---
test_an_invalid_channel_is_an_error_naming_it_before_any_git_or_build_call() {
  local v c
  for v in beta Nightly LATEST main stable master; do
    for c in check install update; do
      : >"$STUB_LOG"
      DPDS_CHANNEL="$v" run_dpds --yes "$c"
      CTX="DPDS_CHANNEL=$v $c" assert_rc_nonzero
      CTX="DPDS_CHANNEL=$v $c" assert_err_has "Invalid DPDS channel '$v' (expected 'latest', 'nightly' or a release tag like 'v1.0.0')"
      CTX="DPDS_CHANNEL=$v $c" assert_log_lacks '^(git|cargo|npm|systemctl|dpds-controller) '
      CTX="DPDS_CHANNEL=$v $c" assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
    done
  done
}

# --- update ---
test_update_refuses_a_dirty_tree_before_any_git_change_on_every_channel() {
  fake_install
  export STUB_GIT_DIRTY=1 STUB_GIT_BRANCH=feature-x
  local args
  for args in "" "--stable" "--nightly"; do
    : >"$STUB_LOG"
    # shellcheck disable=SC2086
    run_dpds $args --yes update
    CTX="update $args" assert_rc_nonzero
    grep -Eiq 'uncommitted|dirty|working tree|local changes' <<<"$ERR" || fail "stderr does not explain the dirty tree: $ERR"
    CTX="update $args" assert_log_lacks '^git (fetch|checkout|pull)'
    CTX="update $args" assert_log_lacks '^(cargo build|npm ci)'
    CTX="update $args" assert_log_lacks '^systemctl (enable|disable)'
  done
}

test_a_failing_git_step_stops_update_before_any_build_on_every_channel() {
  fake_install
  export STUB_ACTIVE=1
  local args sub
  for args in "--stable" "--nightly"; do
    for sub in fetch checkout pull; do
      : >"$STUB_LOG"
      STUB_GIT_FAIL_SUB="$sub" run_dpds $args --yes update
      CTX="update $args, git $sub fails" assert_rc_nonzero
      CTX="update $args, git $sub fails" assert_log_lacks '^(cargo build|npm ci|npm run build)'
      CTX="update $args, git $sub fails" assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
    done
  done
  # a failing fetch stops before the checkout, a failing checkout before the pull
  : >"$STUB_LOG"
  STUB_GIT_FAIL_SUB=fetch run_dpds --yes update
  assert_log_lacks '^git (checkout|pull)'
  : >"$STUB_LOG"
  STUB_GIT_FAIL_SUB=checkout run_dpds --yes update
  assert_log_lacks '^git pull'
}

test_update_restarts_only_a_service_that_was_active_on_every_channel() {
  fake_install
  local args
  for args in "--stable" "--nightly"; do
    : >"$STUB_LOG"
    rm -f "$(stamp_file)"
    STUB_ACTIVE=1 run_dpds $args --yes update
    CTX="active, $args" assert_rc 0 "$ERR"
    CTX="active, $args" assert_log_has '^systemctl disable --now dpds-controller$'
    CTX="active, $args" assert_log_has '^systemctl enable --now dpds-controller$'
    : >"$STUB_LOG"
    rm -f "$(stamp_file)"
    STUB_ACTIVE=0 run_dpds $args --yes update
    CTX="inactive, $args" assert_rc 0 "$ERR"
    CTX="inactive, $args" assert_log_lacks '^systemctl (enable|start|restart)'
  done
}

test_update_checks_the_prerequisites_before_any_git_call_on_every_channel() {
  fake_install
  export STUB_NO_LIBDPDK=1
  local args
  for args in "--stable" "--nightly"; do
    : >"$STUB_LOG"
    run_dpds $args --yes update
    CTX="$args" assert_rc_nonzero
    CTX="$args" assert_log_lacks '^git (fetch|checkout|pull)'
    CTX="$args" assert_log_lacks '^cargo build'
  done
}

# a warning that names the branch the tree is on and the one the channel wants
assert_branch_warning() { # assert_branch_warning CURRENT TARGET
  local w; w="$(grep -Ei 'warn' <<<"$OUT$ERR")"
  grep -qF -- "$1" <<<"$w" || fail "no WARN line names the checked-out branch '$1': $w"
  grep -qF -- "$2" <<<"$w" || fail "no WARN line names the channel's branch '$2': $w"
  grep -qi 'branch' <<<"$w" || fail "the WARN line does not talk about a branch: $w"
}

test_update_warns_when_the_tree_is_on_another_branch_than_the_channel() {
  fake_install
  export STUB_GIT_BRANCH=nightly
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_branch_warning nightly main
  assert_log_has '^git checkout main$'
  : >"$STUB_LOG"
  export STUB_GIT_BRANCH=main
  run_dpds --nightly --yes update
  assert_rc 0 "$ERR"
  assert_branch_warning main nightly
  assert_log_has '^git checkout nightly$'
  : >"$STUB_LOG"
  export STUB_GIT_BRANCH=feature-x
  run_dpds --stable --yes update
  assert_rc 0 "$ERR"
  assert_branch_warning feature-x main
}

test_update_warns_about_a_detached_head_and_follows_the_channel() {
  fake_install
  export STUB_GIT_BRANCH=HEAD
  run_dpds --nightly --yes update
  assert_rc 0 "$ERR"
  grep -Ei 'warn' <<<"$OUT$ERR" | grep -qi 'detached' || fail "no WARN line mentions a detached HEAD"
  assert_log_has '^git checkout nightly$'
}

test_update_on_the_channel_branch_gives_no_branch_warning() {
  fake_install
  local args branch
  for args in "--stable:main" "--nightly:nightly"; do
    branch="${args#*:}"
    : >"$STUB_LOG"
    STUB_GIT_BRANCH="$branch" run_dpds "${args%:*}" --yes update
    CTX="$args" assert_rc 0 "$ERR"
    ! grep -Ei 'warn' <<<"$OUT$ERR" | grep -Eqi 'detached|branch' || fail "$args: unexpected branch warning: $(grep -Ei warn <<<"$OUT$ERR")"
  done
}

test_the_release_tags_are_not_consulted_any_more() {
  # CI tags every version that reaches main, so main's head is the latest release: no `git tag`, no `fetch --tags`,
  # no checkout of a v* tag, whatever tags exist and whatever the channel
  export STUB_GIT_TAGS="v1.9.0 v1.10.0 v0.9.9"
  fake_install
  local args c
  for args in "--stable" "--nightly" ""; do
    for c in install update; do
      : >"$STUB_LOG"
      # shellcheck disable=SC2086
      run_dpds $args --yes "$c"
      CTX="$args $c" assert_rc 0 "$ERR"
      CTX="$args $c" assert_log_lacks '^git tag'
      CTX="$args $c" assert_log_lacks '--tags'
      CTX="$args $c" assert_log_lacks '^git checkout (--detach )?v[0-9]'
    done
  done
}

test_stable_needs_no_release_tag_any_more() {
  # --stable follows main, tags or not
  fake_install
  export STUB_GIT_TAGS=""
  run_dpds --stable --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^git checkout main$'
  ! grep -Eqi 'release.*tag|tag.*release' <<<"$ERR" || fail "still complains about release tags: $ERR"
}

# --- install ---
test_install_never_moves_the_tree_on_any_channel() {
  local args
  for args in "" "--stable" "--nightly"; do
    : >"$STUB_LOG"
    # shellcheck disable=SC2086
    run_dpds $args --yes install
    CTX="install $args" assert_rc 0 "$ERR"
    CTX="install $args" assert_log_lacks '^git (fetch|checkout|pull|tag|merge|reset|switch|clean|stash)'
  done
  : >"$STUB_LOG"
  DPDS_CHANNEL=nightly run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_log_lacks '^git (fetch|checkout|pull|tag|merge|reset|switch|clean|stash)'
}

test_install_warns_when_the_tree_is_not_on_the_channels_branch_and_still_installs() {
  # warn only (naming both branches and `update`), do not switch and do not refuse
  local case_ cur target flag
  for case_ in "nightly|main|" "main|nightly|--nightly" "feature-x|main|--stable" "feature-x|nightly|--nightly"; do
    IFS='|' read -r cur target flag <<<"$case_"
    : >"$STUB_LOG"
    rm -rf "$DPDS_PREFIX/bin/dpds-controller" "$DPDS_ETC_DIR" "$(installed_unit)"; seed_network physical
    STUB_GIT_BRANCH="$cur" run_dpds $flag --yes install
    CTX="on $cur, channel branch $target" assert_rc 0 "$ERR"
    CTX="on $cur, channel branch $target" assert_branch_warning "$cur" "$target"
    CTX="on $cur, channel branch $target" assert_all_has "update"
    CTX="on $cur, channel branch $target" assert_exec "$DPDS_PREFIX/bin/dpds-controller"
    CTX="on $cur, channel branch $target" assert_file "$(installed_unit)"
  done
}

test_install_warns_about_a_detached_head_naming_the_channel_branch() {
  export STUB_GIT_BRANCH=HEAD
  run_dpds --yes install
  assert_rc 0 "$ERR"
  local w; w="$(grep -Ei 'warn' <<<"$OUT$ERR")"
  grep -qi 'detached' <<<"$w" || fail "no WARN line mentions the detached HEAD: $w"
  grep -qF 'main' <<<"$w" || fail "the WARN line does not name the channel's branch main: $w"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

test_install_on_the_channel_branch_gives_no_branch_warning() {
  local args branch
  for args in ":main" "--stable:main" "--nightly:nightly"; do
    branch="${args#*:}"
    : >"$STUB_LOG"
    # shellcheck disable=SC2086
    STUB_GIT_BRANCH="$branch" run_dpds ${args%:*} --yes install
    CTX="$args" assert_rc 0 "$ERR"
    ! grep -Ei 'warn' <<<"$OUT$ERR" | grep -Eqi 'detached|branch' || fail "$args: unexpected branch warning: $(grep -Ei warn <<<"$OUT$ERR")"
  done
  : >"$STUB_LOG"
  STUB_GIT_BRANCH=nightly DPDS_CHANNEL=nightly run_dpds --yes install
  assert_rc 0 "$ERR"
  ! grep -Ei 'warn' <<<"$OUT$ERR" | grep -Eqi 'detached|branch' || fail "env nightly: unexpected branch warning"
}

test_install_from_a_tree_without_git_still_works() {
  # a source tarball has no .git: the branch check must not turn into a failure
  export STUB_GIT_NOT_A_REPO=1
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

# --- status, usage, other commands ---
test_status_prints_the_channel_and_the_branch_of_the_checkout() {
  seed_unit
  export STUB_GIT_BRANCH=feature-x DPDS_CHANNEL=nightly
  run_dpds status
  assert_rc 0 "$ERR"
  grep -Eqi 'channel' <<<"$OUT" || fail "status does not print the channel: $OUT"
  grep -Eqi 'channel.*nightly|nightly.*channel' <<<"$OUT" || fail "status does not say the channel is nightly: $OUT"
  grep -qF 'feature-x' <<<"$OUT" || fail "status does not print the branch of the checkout: $OUT"
  plain_line_with "7.8.9" || fail "the installed version is gone from status: $OUT"
}

test_status_names_the_default_channel_latest() {
  seed_unit
  run_dpds status
  assert_rc 0 "$ERR"
  grep -Eqi 'channel.*latest|latest.*channel' <<<"$OUT" || fail "status does not say the channel is latest: $OUT"
  grep -Eq '(^|[^[:alnum:]_-])main([^[:alnum:]_-]|$)' <<<"$OUT" || fail "status does not print the branch main: $OUT"
}

test_status_still_exits_zero_in_a_tree_without_git() {
  seed_unit
  export STUB_GIT_NOT_A_REPO=1
  run_dpds status
  assert_rc 0 "$ERR"
  plain_line_with "7.8.9" || fail "the version is gone when git fails: $OUT"
}

test_help_documents_the_channel() {
  run_dpds --help
  assert_rc 0
  assert_out_has "DPDS_CHANNEL"
  grep -Eq -- '--stable' <<<"$OUT" || fail "--help lost --stable"
  grep -E -- '--stable' <<<"$OUT" | grep -Eqi 'latest|main' || fail "--help does not tie --stable to the latest channel / main: $OUT"
  grep -E -- '--nightly' <<<"$OUT" | grep -qi 'nightly' || fail "--help does not describe --nightly: $OUT"
  ! grep -E -- '--nightly' <<<"$OUT" | grep -qi 'default' || fail "--help still calls --nightly the default: $OUT"
  grep -Ei 'default' <<<"$OUT" | grep -qi 'latest' || fail "--help does not say that latest is the default: $OUT"
  grep -Eqi "latest.*main|main.*latest" <<<"$OUT" || fail "--help does not say that latest means main: $OUT"
  # the tag form is documented, but --stable does not select a tag
  ! grep -Eqi 'v\*|git tag|highest' <<<"$OUT" || fail "--help still talks about the old tag selection: $OUT"
}

test_the_channel_does_not_change_the_other_commands() {
  seed_unit
  fake_install
  local c
  for c in check start stop restart status uninstall; do
    : >"$STUB_LOG"
    DPDS_CHANNEL=nightly run_dpds --yes "$c"
    CTX="$c" assert_log_lacks '^git (fetch|checkout|pull|tag)'
  done
}

# ------------------------------------------------------------------ tests: release tag channel (DPDS_CHANNEL=<release tag>)
# DPDS_CHANNEL=v1.0.0 installs exactly that release: update detaches onto the tag, install only warns when HEAD is elsewhere.
# The git stub knows a tag when it is listed in STUB_GIT_TAGS; its commit is `commit-of-<tag>`, HEAD's is STUB_GIT_HEAD_COMMIT.

# update on a tag: fetch --tags, the tag lookup, checkout --detach TAG, in this order, never a pull, before the builds.
assert_update_detaches_onto() { # assert_update_detaches_onto TAG
  local tag="$1" lookup
  lookup="^git rev-parse .*refs/tags/$tag\\^\\{commit\\}"
  assert_log_has '^git fetch.*--tags'
  assert_log_has "$lookup"
  assert_log_has "^git checkout --detach $tag\$"
  assert_log_lacks '^git pull'
  assert_log_lacks '^git checkout (main|nightly)$'
  assert_before '^git fetch.*--tags' "$lookup"
  assert_before "$lookup" "^git checkout --detach $tag\$"
  assert_before "^git checkout --detach $tag\$" '^npm ci( |$)'
  assert_before "^git checkout --detach $tag\$" '^cargo build'
  assert_dir_of '^git (fetch|checkout)' "$REPO_ROOT"
  ! grep -qi 'unbound' <<<"$ERR" || fail "the script trips over an unset variable: $ERR"
}

# nothing was changed: no checkout, no pull, no build, no service change, and the installed controller is the old one
assert_nothing_changed_by_update() {
  assert_log_lacks '^git (checkout|pull)'
  assert_log_lacks '^(cargo build|npm ci|npm run build)'
  assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" 2>/dev/null || fail "the installed controller was replaced"
}

# a WARN line (stdout or stderr) that names the tag, and for which the mismatch of HEAD is explained
assert_tag_warning() { # assert_tag_warning TAG
  local w; w="$(grep -Ei 'warn' <<<"$OUT$ERR")"
  grep -qF -- "$1" <<<"$w" || fail "no WARN line names the tag '$1': $w"
  grep -qi 'update' <<<"$w" || fail "the WARN line does not tell to run update: $w"
}

# --- valid and invalid values ---
test_a_release_tag_is_a_valid_channel_for_check_install_and_update() {
  export STUB_GIT_TAGS="v1.0.0 v10.20.30"
  fake_install
  local v c
  for v in v1.0.0 v10.20.30; do
    for c in check install update; do
      : >"$STUB_LOG"
      DPDS_CHANNEL="$v" run_dpds --yes "$c"
      CTX="DPDS_CHANNEL=$v $c" assert_rc 0 "$ERR"
      CTX="DPDS_CHANNEL=$v $c" assert_no_invalid_channel
    done
  done
}
assert_no_invalid_channel() { ! grep -qi 'invalid dpds channel' <<<"$ERR" || fail "a valid tag was rejected: $ERR"; }

test_a_malformed_tag_is_an_invalid_channel_before_any_git_or_build_call() {
  local v c
  for v in v1 v1.0 1.0.0 v1.0.0-rc1 v1.0.0+build v1.0.0.0 vv1.0.0 V1.0.0 v1.0.x v1x0y0 v-1.0.0 'v1.0.0 ' ' v1.0.0' 'v1.0.0$(id)' 'v1.0.0;x' 'v1/0/0' refs/tags/v1.0.0; do
    for c in check install update; do
      : >"$STUB_LOG"
      STUB_GIT_TAGS="v1.0.0" DPDS_CHANNEL="$v" run_dpds --yes "$c"
      CTX="DPDS_CHANNEL='$v' $c" assert_rc_nonzero
      CTX="DPDS_CHANNEL='$v' $c" assert_err_has "Invalid DPDS channel '$v' (expected 'latest', 'nightly' or a release tag like 'v1.0.0')"
      CTX="DPDS_CHANNEL='$v' $c" assert_log_lacks '^(git|cargo|npm|systemctl|dpds-controller) '
      CTX="DPDS_CHANNEL='$v' $c" assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
    done
  done
  assert_no_file "$T/cwd/x"
  assert_no_file "$T/repo/x"
}

# --- update ---
test_update_on_a_tag_fetches_the_tags_and_detaches_onto_it_without_a_pull() {
  fake_install
  export STUB_GIT_TAGS="v0.9.0 v1.0.0 v1.1.0"
  DPDS_CHANNEL=v1.0.0 run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_update_detaches_onto v1.0.0
  assert_log_lacks '^git checkout (--detach )?v(0.9.0|1.1.0)'
  # the rebuild is the usual one: frontend, both cargo builds, the controller is installed
  assert_log_has '^npm run build'
  assert_log_has '^cargo build.*dpds-controller'
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
}

test_update_on_a_tag_from_a_detached_head_or_another_branch_works_and_never_pulls() {
  # a pull on a detached HEAD fails in real git: staying on a tag means every later update must not pull
  fake_install
  export STUB_GIT_TAGS="v1.0.0"
  local br
  for br in HEAD nightly main feature-x; do
    : >"$STUB_LOG"
    rm -f "$(stamp_file)"
    STUB_GIT_BRANCH="$br" DPDS_CHANNEL=v1.0.0 run_dpds --yes update
    CTX="on '$br'" assert_rc 0 "$ERR"
    CTX="on '$br'" assert_update_detaches_onto v1.0.0
  done
}

test_update_on_a_tag_restarts_only_a_service_that_was_active() {
  fake_install
  export STUB_GIT_TAGS="v1.0.0"
  STUB_ACTIVE=1 DPDS_CHANNEL=v1.0.0 run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl disable --now dpds-controller$'
  assert_log_has '^systemctl enable --now dpds-controller$'
  : >"$STUB_LOG"
  STUB_ACTIVE=0 DPDS_CHANNEL=v1.0.0 run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_lacks '^systemctl (enable|start|restart)'
}

test_update_on_a_tag_that_does_not_exist_names_it_and_changes_nothing() {
  fake_install
  export STUB_ACTIVE=1
  local tags
  for tags in "" "v0.9.0 v1.1.0 v1.0" "v1.0.1"; do
    : >"$STUB_LOG"
    STUB_GIT_TAGS="$tags" DPDS_CHANNEL=v1.0.0 run_dpds --yes update
    CTX="tags '$tags'" assert_rc_nonzero
    CTX="tags '$tags'" assert_err_has "v1.0.0"
    CTX="tags '$tags'" assert_nothing_changed_by_update
    # the lookup was tried (after fetching the tags), that is how the tag was found missing
    CTX="tags '$tags'" assert_log_has '^git fetch.*--tags'
    CTX="tags '$tags'" assert_log_has '^git rev-parse .*refs/tags/v1.0.0\^\{commit\}'
  done
}

test_update_on_a_tag_refuses_a_dirty_tree_before_any_git_change() {
  fake_install
  export STUB_GIT_DIRTY=1 STUB_GIT_TAGS="v1.0.0" STUB_GIT_BRANCH=feature-x
  DPDS_CHANNEL=v1.0.0 run_dpds --yes update
  assert_rc_nonzero
  grep -Eiq 'uncommitted|dirty|working tree|local changes' <<<"$ERR" || fail "stderr does not explain the dirty tree: $ERR"
  assert_log_lacks '^git (fetch|checkout|pull)'
  assert_nothing_changed_by_update
}

test_update_on_a_tag_checks_the_prerequisites_before_any_git_call() {
  fake_install
  export STUB_NO_LIBDPDK=1 STUB_GIT_TAGS="v1.0.0"
  DPDS_CHANNEL=v1.0.0 run_dpds --yes update
  assert_rc_nonzero
  assert_no_invalid_channel
  assert_err_has "libdpdk"
  assert_log_lacks '^git (fetch|checkout|pull)'
  assert_log_lacks '^cargo build'
}

test_a_failing_git_step_stops_a_tag_update_before_any_build() {
  fake_install
  export STUB_ACTIVE=1 STUB_GIT_TAGS="v1.0.0"
  local sub
  for sub in fetch checkout rev-parse; do
    : >"$STUB_LOG"
    STUB_GIT_FAIL_SUB="$sub" DPDS_CHANNEL=v1.0.0 run_dpds --yes update
    CTX="git $sub fails" assert_rc_nonzero
    CTX="git $sub fails" assert_no_invalid_channel
    # the tag path was really taken up to the failing step
    CTX="git $sub fails" assert_log_has '^git fetch.*--tags'
    CTX="git $sub fails" assert_log_lacks '^(cargo build|npm ci|npm run build)'
    CTX="git $sub fails" assert_log_lacks '^systemctl (enable|disable|daemon-reload)'
  done
  # a failing fetch stops before the checkout, a failing checkout (of the tag) was the detach
  : >"$STUB_LOG"
  STUB_GIT_FAIL_SUB=fetch DPDS_CHANNEL=v1.0.0 run_dpds --yes update
  assert_log_lacks '^git checkout'
  : >"$STUB_LOG"
  STUB_GIT_FAIL_SUB=checkout DPDS_CHANNEL=v1.0.0 run_dpds --yes update
  assert_log_has '^git checkout --detach v1.0.0$'
  assert_log_lacks '^git pull'
}

# --- flags and the other channels ---
test_the_flags_override_a_tag_channel_in_the_environment() {
  fake_install
  export STUB_GIT_TAGS="v1.0.0"
  DPDS_CHANNEL=v1.0.0 run_dpds --nightly --yes update
  assert_rc 0 "$ERR"
  CTX="v1.0.0 --nightly" assert_update_follows nightly
  : >"$STUB_LOG"
  rm -f "$(stamp_file)"
  DPDS_CHANNEL=v1.0.0 run_dpds --stable --yes update
  assert_rc 0 "$ERR"
  CTX="v1.0.0 --stable" assert_update_follows main
  # also over an invalid value, and with install and status the flag's channel is the one shown
  : >"$STUB_LOG"
  rm -f "$(stamp_file)"
  DPDS_CHANNEL=v1 run_dpds --nightly --yes update
  assert_rc 0 "$ERR"
  CTX="v1 --nightly" assert_update_follows nightly
}

test_both_flags_are_still_an_error_with_a_tag_channel() {
  export STUB_GIT_TAGS="v1.0.0"
  DPDS_CHANNEL=v1.0.0 run_dpds --nightly --stable update
  assert_rc_nonzero
  assert_err_has "--nightly"
  assert_err_has "--stable"
  assert_log_empty
}

test_a_branch_channel_after_a_tag_goes_back_to_the_branch() {
  # staying on a tag: the checkout is detached; the next update on a branch channel switches back and pulls as before
  fake_install
  export STUB_GIT_TAGS="v1.0.0" STUB_GIT_BRANCH=HEAD
  DPDS_CHANNEL=nightly run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_update_follows nightly
  : >"$STUB_LOG"
  rm -f "$(stamp_file)"  # the previous update stamped HEAD; this one must not be a no-op
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_update_follows main
}

# --- install ---
test_install_with_a_tag_channel_stays_read_only() {
  export STUB_GIT_TAGS="v1.0.0"
  local br
  for br in main HEAD feature-x; do
    : >"$STUB_LOG"
    STUB_GIT_BRANCH="$br" DPDS_CHANNEL=v1.0.0 run_dpds --yes install
    CTX="on '$br'" assert_rc 0 "$ERR"
    CTX="on '$br'" assert_log_lacks '^git (fetch|checkout|pull|merge|reset|switch|clean|stash)'
    CTX="on '$br'" assert_log_lacks '--tags'
    CTX="on '$br'" assert_exec "$DPDS_PREFIX/bin/dpds-controller"
    CTX="on '$br'" assert_file "$(installed_unit)"
  done
}

test_install_warns_when_head_is_not_the_tags_commit_and_still_installs() {
  export STUB_GIT_TAGS="v1.0.0 v1.1.0"
  local case_ cur head
  # on a branch, detached at another commit, detached at another tag's commit
  for case_ in "main|headcommit-0000" "HEAD|headcommit-0000" "HEAD|commit-of-v1.1.0" "feature-x|commit-of-v1.1.0"; do
    IFS='|' read -r cur head <<<"$case_"
    : >"$STUB_LOG"
    rm -rf "$DPDS_PREFIX/bin/dpds-controller" "$DPDS_ETC_DIR" "$(installed_unit)"; seed_network physical
    STUB_GIT_BRANCH="$cur" STUB_GIT_HEAD_COMMIT="$head" DPDS_CHANNEL=v1.0.0 run_dpds --yes install
    CTX="on '$cur' at $head" assert_rc 0 "$ERR"
    CTX="on '$cur' at $head" assert_tag_warning v1.0.0
    CTX="on '$cur' at $head" assert_exec "$DPDS_PREFIX/bin/dpds-controller"
    CTX="on '$cur' at $head" assert_file "$(installed_unit)"
    CTX="on '$cur' at $head" assert_log_lacks '^git (fetch|checkout|pull)'
    # it names HEAD's side as well: the branch, or "detached" for a detached checkout
    local w; w="$(grep -Ei 'warn' <<<"$OUT$ERR")"
    if [[ "$cur" == HEAD ]]; then
      CTX="on '$cur' at $head" grep -qi 'detached' <<<"$w" || fail "[$cur] the WARN line does not say HEAD is detached: $w"
    else
      grep -qF -- "$cur" <<<"$w" || fail "[$cur] the WARN line does not name the checked-out branch: $w"
    fi
  done
}

test_install_is_silent_when_head_is_the_tags_commit() {
  export STUB_GIT_TAGS="v1.0.0 v1.1.0"
  local br
  for br in HEAD main; do
    : >"$STUB_LOG"
    STUB_GIT_BRANCH="$br" STUB_GIT_HEAD_COMMIT="commit-of-v1.0.0" DPDS_CHANNEL=v1.0.0 run_dpds --yes install
    CTX="on '$br'" assert_rc 0 "$ERR"
    ! grep -Ei 'warn' <<<"$OUT$ERR" | grep -Eqi 'detached|branch|tag|v1\.0\.0|update|HEAD' \
      || fail "[$br] unexpected warning at the tag's commit: $(grep -Ei warn <<<"$OUT$ERR")"
    CTX="on '$br'" assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  done
}

test_install_with_a_tag_channel_works_when_the_tag_is_unknown_or_there_is_no_git() {
  # warn-only: never a failure of install
  STUB_GIT_TAGS="" DPDS_CHANNEL=v1.0.0 run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  rm -f "$DPDS_PREFIX/bin/dpds-controller"
  : >"$STUB_LOG"
  STUB_GIT_NOT_A_REPO=1 DPDS_CHANNEL=v1.0.0 run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
}

# --- status ---
test_status_shows_a_tag_channel_with_its_tag() {
  seed_unit
  export STUB_GIT_TAGS="v1.0.0" DPDS_CHANNEL=v1.0.0
  run_dpds status
  assert_rc 0 "$ERR"
  assert_out_has "Channel: v1.0.0 (tag v1.0.0), checkout: "
  plain_line_with "7.8.9" || fail "the installed version is gone from status: $OUT"
  # a branch checkout shows its branch, a detached one says so
  STUB_GIT_BRANCH=feature-x run_dpds status
  assert_rc 0 "$ERR"
  assert_out_has "Channel: v1.0.0 (tag v1.0.0), checkout: "
  grep -F 'Channel:' <<<"$OUT" | grep -qF 'feature-x' || fail "status does not print the branch of the checkout: $OUT"
  STUB_GIT_BRANCH=HEAD run_dpds status
  assert_rc 0 "$ERR"
  grep -F 'Channel:' <<<"$OUT" | grep -qi 'detached' || fail "status does not say the checkout is detached: $OUT"
  ! grep -qi 'unbound' <<<"$ERR" || fail "status trips over an unset variable: $ERR"
}

test_status_of_a_tag_channel_exits_zero_without_git() {
  seed_unit
  export STUB_GIT_NOT_A_REPO=1
  DPDS_CHANNEL=v1.0.0 run_dpds status
  assert_rc 0 "$ERR"
  assert_out_has "Channel: v1.0.0 (tag v1.0.0)"
  plain_line_with "7.8.9" || fail "the version is gone when git fails: $OUT"
}

test_status_of_the_branch_channels_is_unchanged() {
  seed_unit
  run_dpds status
  assert_rc 0 "$ERR"
  assert_out_has "Channel: latest (branch main), checkout: branch main"
  STUB_GIT_BRANCH=nightly DPDS_CHANNEL=nightly run_dpds status
  assert_out_has "Channel: nightly (branch nightly), checkout: branch nightly"
  ! grep -qF '(tag ' <<<"$OUT" || fail "a branch channel is shown as a tag: $OUT"
}

test_a_tag_channel_does_not_change_the_other_commands() {
  seed_unit
  fake_install
  export STUB_GIT_TAGS="v1.0.0"
  local c
  for c in check start stop restart status uninstall; do
    : >"$STUB_LOG"
    DPDS_CHANNEL=v1.0.0 run_dpds --yes "$c"
    CTX="$c" assert_log_lacks '^git (fetch|checkout|pull)'
    CTX="$c" assert_no_invalid_channel
  done
}

# --- documentation: usage text and header comment ---
test_help_documents_the_release_tag_form() {
  run_dpds --help
  assert_rc 0
  assert_out_has "DPDS_CHANNEL=v1.0.0"
  grep -Eqi 'release tag' <<<"$OUT" || fail "--help does not say what a release tag is: $OUT"
  grep -E 'v1\.0\.0' <<<"$OUT" | grep -Eqi 'tag|release' || fail "--help does not tie v1.0.0 to a release: $OUT"
  # the branch channels are still documented, latest is still the default
  assert_out_has "latest"
  assert_out_has "nightly"
  grep -Ei 'default' <<<"$OUT" | grep -qi 'latest' || fail "--help does not say that latest is the default: $OUT"
}

test_the_header_comment_of_the_script_documents_the_release_tag_form() {
  local header; header="$(grep -E '^#' "$SCRIPT" | head -60)"
  grep -qF 'v1.0.0' <<<"$header" || fail "the header comment never gives a tag example (v1.0.0)"
  grep -Eqi 'release tag|tag' <<<"$header" || fail "the header comment never mentions tags"
}

# ------------------------------------------------------------------ tests: status shows the installed version
# a line of the output that is not the raw JSON answer
plain_line_with() { grep -v '[{"]' <<<"$OUT" | grep -Fq -- "$1"; }

test_status_prints_the_installed_version_on_its_own_line() {
  seed_unit
  run_dpds status
  assert_rc 0 "$ERR"
  plain_line_with "7.8.9" || fail "no plain line shows the version 7.8.9: $OUT"
}

test_status_finds_the_version_whatever_the_key_order_and_spacing() {
  seed_unit
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
  seed_unit
  export STUB_CURL_RC=7
  run_dpds status
  assert_rc 0 "$ERR"
  ! plain_line_with "7.8.9" || fail "a version is shown although the API is down: $OUT"
}

test_status_survives_an_answer_without_a_version() {
  seed_unit
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

test_the_channel_flags_are_accepted_by_check() {
  run_dpds --nightly check
  assert_rc 0
  assert_out_has "CHECK-OUTPUT-MARKER"
  run_dpds --stable check
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

test_install_installs_binaries_unit_env() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file_has "$DPDS_PREFIX/bin/dpds" 'STUB-BINARY-dpds'
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
  assert_file "$(installed_unit)"
  assert_file "$DPDS_ETC_DIR/controller.env"
  assert_no_state_dir_anywhere
}

test_install_unit_paths_follow_prefix_and_etc() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  # a non-default etc dir reaches the service, so it reads the mode stored there
  assert_file_has "$(installed_unit)" "^ExecStart=$DPDS_PREFIX/bin/dpds-controller serve --etc-dir $DPDS_ETC_DIR\$"
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
  # nothing about a state directory
  ! grep -qi 'STATE' "$f" || fail "the env file mentions a state directory: $(cat "$f")"
  [[ "$(grep -c '^[A-Z_]*=' "$f")" == 2 ]] || fail "exactly BIND and DPDS_BIN are written: $(cat "$f")"
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
  assert_no_state_dir_anywhere
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
  seed_unit
  run_dpds start
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
  assert_log_lacks '^(cargo|git|curl) '
}

test_start_needs_no_build_tools() {
  seed_unit
  local t
  for t in cargo rustc clang ld.lld pkg-config git curl node npm; do remove_stub "$t"; done
  run_dpds start
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
}

test_start_systemctl_failure_propagates() {
  seed_unit
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
  seed_unit
  run_dpds restart
  assert_rc 0 "$ERR"
  assert_before '^systemctl disable --now dpds-controller' '^systemctl enable --now dpds-controller'
}

test_status_runs_both_probes() {
  seed_unit
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl .*status.* dpds-controller'
  assert_log_has '^curl .*http://127\.0\.0\.1:8100/api/online'
  assert_out_has "STUB-SYSTEMCTL-STATUS"
  assert_out_has "7.8.9"
}

test_status_uses_port_from_env_file() {
  seed_unit
  mkdir -p "$DPDS_ETC_DIR"
  printf 'DPDS_CONTROLLER_BIND=0.0.0.0:9123\n' >"$DPDS_ETC_DIR/controller.env"
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^curl .*http://127\.0\.0\.1:9123/api/online'
}

test_status_exit_zero_when_everything_is_down() {
  seed_unit
  export STUB_SYSTEMCTL_RC=3 STUB_CURL_RC=7
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl .*status'
  assert_log_has '^curl '
}

# ------------------------------------------------------------------ tests: update
test_update_pulls_main_by_default_rebuilds_and_reinstalls() {
  fake_install
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^git fetch'
  assert_log_has '^git checkout main$'
  assert_log_has '^git pull --ff-only$'
  assert_log_lacks '^git pull$'
  assert_before '^git fetch' '^git checkout main'
  assert_before '^git checkout main' '^git pull --ff-only'
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
  seed_unit
  write_env_bind 10.0.0.5:9123
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^curl .*http://10\.0\.0\.5:9123/api/online'
  assert_log_lacks '^curl .*127\.0\.0\.1'
}

test_status_probes_loopback_for_a_wildcard_bind() {
  seed_unit
  write_env_bind 0.0.0.0:9123
  run_dpds status
  assert_rc 0 "$ERR"
  assert_log_has '^curl .*http://127\.0\.0\.1:9123/api/online'
  assert_log_lacks '^curl .*0\.0\.0\.0'
}

test_status_probes_a_loopback_bind_as_is() {
  seed_unit
  write_env_bind 127.0.0.1:9000
  run_dpds status
  assert_log_has '^curl .*http://127\.0\.0\.1:9000/api/online'
}

# ------------------------------------------------------------------ tests: update safety, failed stop
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
  assert_all_has "main"
  grep -Ei 'warn' <<<"$OUT$ERR" | grep -qF 'feature-x' || fail "no WARN line names the branch feature-x"
  assert_log_has '^git checkout main$'
}

test_update_warns_about_a_detached_head() {
  fake_install
  export STUB_GIT_BRANCH=HEAD
  run_dpds --yes update
  assert_rc 0 "$ERR"
  grep -Ei 'warn' <<<"$OUT$ERR" | grep -qi 'detached' || fail "no WARN line mentions a detached HEAD"
  assert_log_has '^git checkout main$'
}

test_update_on_the_default_channels_branch_gives_no_branch_warning() {
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
  seed_unit
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

# ------------------------------------------------------------------ tests: prerequisites, host check
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


# ------------------------------------------------------------------ tests: frontend built on the host, prerequisites
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

# ---- frontend: nothing is downloaded, the caller's DPDS_FRONTEND_DIST is ignored
test_install_and_update_download_and_unpack_nothing() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_nothing_downloaded
  assert_log_lacks '^git (fetch|checkout|pull|tag|merge|reset|switch|clean|stash)'  # install only reads the branch
  fake_install
  : >"$STUB_LOG"
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_nothing_downloaded
  assert_log_lacks '^git (rev-parse .*frontend|status .*frontend)'  # the stamp needs plain `git rev-parse HEAD`
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
  assert_log_lacks '^git (fetch|checkout|pull|tag|merge|reset|switch|clean|stash)'  # install only reads the branch
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
test_uninstall_removes_binaries_and_unit_keeps_env() {
  fake_install
  local env_before
  env_before="$(cat "$DPDS_ETC_DIR/controller.env")"
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
  assert_eq "$(cat "$DPDS_ETC_DIR/controller.env")" "$env_before" "env file kept"
}

test_uninstall_touches_no_state_dir() {
  fake_install
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  ! grep -qi 'state' <<<"$OUT$ERR" || fail "talks about state: $OUT$ERR"
  assert_no_state_dir_anywhere
}

test_uninstall_ignores_a_state_dir_variable() {
  # a value (even a bad one) neither makes uninstall fail nor names a directory to remove
  fake_install
  mkdir -p "$T/mine"
  echo precious >"$T/mine/data"
  export DPDS_STATE_DIR="$T/mine"
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  assert_file "$T/mine/data"
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

test_install_then_uninstall_roundtrip_keeps_env_and_leaves_no_state() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$(installed_unit)"
  assert_file "$DPDS_ETC_DIR/controller.env"
  assert_no_state_dir_anywhere
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

# The controller's check output, with the commands it suggests under a hint, is shown as it is (indentation and
# order kept), once, and nothing is run.
CONTROLLER_HINT_BLOCK='INFO  vm.swappiness: vm.swappiness is 60, the target is 0 -> Set it to 0.
    # run as root:
    sysctl -w vm.swappiness=0
WARN  CPU governor: cores 2-3 use powersave -> Use performance.
    # run as root:
    echo performance > /sys/devices/system/cpu/cpu2/cpufreq/scaling_governor
    echo performance > /sys/devices/system/cpu/cpu3/cpufreq/scaling_governor'

assert_hint_block_shown_unchanged() {
  local line
  while IFS= read -r line; do
    assert_eq "$(grep -cxF -- "$line" <<<"$OUT")" "$(grep -cxF -- "$line" <<<"$CONTROLLER_HINT_BLOCK")" "line shown as often as the controller printed it: $line"
  done <<<"$CONTROLLER_HINT_BLOCK"
  assert_eq "$(grep -F -x -A2 -- "INFO  vm.swappiness: vm.swappiness is 60, the target is 0 -> Set it to 0." <<<"$OUT")" "$(head -n 3 <<<"$CONTROLLER_HINT_BLOCK")" "the commands stay right under their hint, indented"
  assert_eq "$(grep -F -x -A3 -- "WARN  CPU governor: cores 2-3 use powersave -> Use performance." <<<"$OUT")" "$(tail -n 4 <<<"$CONTROLLER_HINT_BLOCK")" "the second block stays whole"
}

test_check_prints_the_controllers_command_blocks_unchanged_and_runs_nothing() {
  local c
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  export STUB_CONTROLLER_HINT_TEXT="$CONTROLLER_HINT_BLOCK"
  run_dpds check
  assert_rc 0 "$ERR"
  assert_hint_block_shown_unchanged
  assert_log_has '^dpds-controller check'
  assert_log_lacks 'host/actions'
  assert_log_lacks '^curl '
  for c in $TUNING_TOOLS; do assert_log_lacks "^$c( |\$)"; done
}

test_install_shows_the_command_blocks_unchanged_and_runs_nothing() {
  local c
  export STUB_CONTROLLER_HINT_TEXT="$CONTROLLER_HINT_BLOCK"
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_hint_block_shown_unchanged
  assert_log_lacks 'host/actions'
  for c in $TUNING_TOOLS; do assert_log_lacks "^$c( |\$)"; done
}

test_script_source_contains_no_tuning_commands() {
  local pat
  for pat in 'ethtool' 'setpci' 'sysctl' 'mlxconfig' 'mlxfwreset' 'cpupower' 'nr_hugepages' 'hugepages-' 'isolcpus' 'tuned-adm' '/proc/sys' 'update-grub' 'grubby' \
             'mstconfig' 'mstfwreset' 'dcb buffer' 'smp_affinity' 'scaling_governor' 'swappiness' 'zone_reclaim' 'host/actions'; do
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

# dpds calls mlockall(MCL_CURRENT|MCL_FUTURE) before EAL init, so the unit sets the memlock limit
# explicitly instead of relying on systemd's default or on root's CAP_IPC_LOCK.
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

# ------------------------------------------------------------------ tests: refusal to run as root, not-installed messages
#
# dpds.sh refuses to run as root. EUID is a read-only bash variable that no test can fake, and the script may read
# it through $EUID or `id -u`, so these tests really become root: `sudo -n` (passwordless sudo, as on the CI runner and in
# the dev container). Without it the tests print a SKIP line and pass vacuously; the report says so. The sandbox is the
# same as for every test (stubs only, all paths below $T), root just runs the script; afterwards the files root created are
# handed back to the caller so the cleanup can remove them. `sudo` sets SUDO_USER (`sudo ./dpds.sh install`);
# the plain-root variant removes the SUDO_* variables again.
SUDO_PHRASES=("as your user" "not with sudo" "PATH")

can_become_root() { sudo -n true >/dev/null 2>&1; }

# run_dpds_as_root [--no-sudo-vars] ARGS... : like run_dpds, but the script runs with EUID 0.
run_dpds_as_root() {
  local unset_args=() v envs=()
  if [[ "${1:-}" == --no-sudo-vars ]]; then
    shift
    unset_args=(-u SUDO_USER -u SUDO_UID -u SUDO_GID -u SUDO_COMMAND)
  fi
  while IFS= read -r v; do envs+=("$v=${!v}"); done < <(compgen -e | grep -E '^(STUB_|DPDS_|CARGO_TARGET_DIR$|HOME$)')
  local errf="$T/stderr" outf="$T/stdout"
  ( cd "$T/cwd" && sudo -n env "${unset_args[@]}" "${envs[@]}" "PATH=$T/stubs:$T/base" "TMPDIR=$T/tmp" \
      timeout 60 bash "$SCRIPT" "$@" <"${STDIN:-/dev/null}" >"$outf" 2>"$errf" )
  RC=$?
  sudo -n chown -R "$(id -u):$(id -g)" "$T" >/dev/null 2>&1 || true
  OUT="$(cat "$outf")"; ERR="$(cat "$errf")"
  [[ ! -s "$STUB_LOG.docker" ]] || fail "dpds.sh called docker: $(tr '\n' '|' <"$STUB_LOG.docker")"
}

# The refusal: non-zero, the message on stderr, and NOTHING else happened (no tool call, no sudo call, nothing written).
assert_refused_as_root() {
  assert_rc_nonzero "$*"
  local p; for p in "${SUDO_PHRASES[@]}"; do assert_err_has "$p"; done
  assert_log_empty
  [[ ! -s "$STUB_LOG.sudo" ]] || fail "the script went on to call sudo: $(tr '\n' '|' <"$STUB_LOG.sudo")"
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$DPDS_ETC_DIR/controller.env"
  assert_no_file "$DPDS_ETC_DIR/installed-commit"
  assert_no_state_dir_anywhere
}

test_a_run_under_sudo_is_refused_for_every_command() {
  can_become_root || { echo "    SKIP: no passwordless sudo, cannot run the script as root"; return; }
  local c
  for c in check install update start stop restart status uninstall; do
    : >"$STUB_LOG"
    run_dpds_as_root "$c"
    CTX="sudo $c" assert_refused_as_root "$c"
  done
}

test_the_refusal_comes_before_every_flag_and_prompt() {
  can_become_root || { echo "    SKIP: no passwordless sudo, cannot run the script as root"; return; }
  local args
  for args in "--yes install" "--stable update" "--stable --yes install" "--nightly check"; do
    : >"$STUB_LOG"
    # shellcheck disable=SC2086
    run_dpds_as_root $args
    CTX="sudo $args" assert_refused_as_root "$args"
  done
}

test_plain_root_without_sudo_variables_is_not_refused_and_needs_no_sudo() {
  # a container: EUID 0, no SUDO_USER. The privileged steps run directly (no sudo call at all).
  can_become_root || { echo "    SKIP: no passwordless sudo, cannot run the script as root"; return; }
  run_dpds_as_root --no-sudo-vars --yes install
  assert_rc 0 "$ERR"
  ! grep -qi 'as your user' <<<"$OUT$ERR" || fail "refused plain root: $ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file "$(installed_unit)"
  [[ ! -s "$STUB_LOG.sudo" ]] || fail "plain root went through sudo: $(tr '\n' '|' <"$STUB_LOG.sudo")"
  : >"$STUB_LOG"
  run_dpds_as_root --no-sudo-vars start
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
  [[ ! -s "$STUB_LOG.sudo" ]] || fail "plain root went through sudo: $(tr '\n' '|' <"$STUB_LOG.sudo")"
}

test_root_is_refused_although_everything_is_installed_and_running() {
  can_become_root || { echo "    SKIP: no passwordless sudo, cannot run the script as root"; return; }
  fake_install
  export STUB_ACTIVE=1
  local c
  for c in start stop restart update uninstall; do
    : >"$STUB_LOG"
    run_dpds_as_root --yes "$c"
    assert_rc_nonzero "$c"
    assert_err_has "as your user"
    assert_log_empty
  done
  # nothing was removed or replaced either
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file "$(installed_unit)"
  assert_file "$DPDS_ETC_DIR/controller.env"
}

test_a_normal_user_with_a_leftover_sudo_user_variable_is_not_refused() {
  # the refusal is about running as root; SUDO_USER in the environment of a non-root user means nothing
  if [[ "$(id -u)" == 0 ]]; then echo "    SKIP: the suite itself runs as root"; return; fi
  seed_unit
  SUDO_USER=root SUDO_UID=0 run_dpds start
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
  ! grep -qi 'as your user' <<<"$OUT$ERR" || fail "refused a non-root user: $ERR"
}

test_the_usage_error_of_a_normal_user_is_unchanged() {
  # the refusal must not swallow the ordinary argument errors for the user the script is meant for
  run_dpds frobnicate
  assert_rc 1
  assert_all_has "Usage"
  ! grep -qi 'as your user' <<<"$OUT$ERR" || fail "a sudo message for a normal user: $ERR"
}

# start, restart and status say what to do when the unit is not installed. "Installed" = the unit file of
# `dpds.sh install` exists in $DPDS_SYSTEMD_DIR (the stub systemctl answers like real systemd for a missing unit: "does not
# exist", "not found", "could not be found"; none of that may reach the user).
SYSTEMCTL_COMPLAINTS='does not exist|not found|could not be found|Unit file'
assert_says_to_install_first() {
  assert_all_has "dpds-controller is not installed"
  assert_all_has "install first"
  grep -Eiq 'dpds\.sh install' <<<"$OUT$ERR" || fail "does not name the command to run (dpds.sh install): $OUT$ERR"
  ! grep -Eiq "$SYSTEMCTL_COMPLAINTS" <<<"$OUT$ERR" || fail "systemctl's own complaint leaked through: $(grep -Ei "$SYSTEMCTL_COMPLAINTS" <<<"$OUT$ERR" | tr '\n' '|')"
}

test_start_without_the_unit_says_to_install_first() {
  run_dpds start
  assert_rc_nonzero
  assert_err_has "dpds-controller is not installed"
  assert_says_to_install_first
}

test_restart_without_the_unit_says_to_install_first() {
  run_dpds restart
  assert_rc_nonzero
  assert_err_has "dpds-controller is not installed"
  assert_says_to_install_first
}

test_status_without_the_unit_says_to_install_first_and_still_exits_zero() {
  run_dpds status
  assert_rc 0 "$ERR"
  assert_says_to_install_first
}

test_the_install_first_message_is_not_shown_once_the_unit_is_there() {
  seed_unit
  local c
  for c in start restart status; do
    : >"$STUB_LOG"
    run_dpds "$c"
    assert_rc 0 "$c: $ERR"
    ! grep -qi 'not installed' <<<"$OUT$ERR" || fail "$c: claims it is not installed although the unit file exists: $OUT$ERR"
  done
}

test_start_works_right_after_install() {
  # the unit that install writes is what makes the service "installed"
  run_dpds --yes install
  assert_rc 0 "$ERR"
  : >"$STUB_LOG"
  run_dpds start
  assert_rc 0 "$ERR"
  assert_log_has '^systemctl enable --now dpds-controller$'
  ! grep -qi 'not installed' <<<"$OUT$ERR" || fail "start after install: $OUT$ERR"
}

test_start_after_uninstall_says_to_install_first_again() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  run_dpds uninstall
  assert_rc 0 "$ERR"
  : >"$STUB_LOG"
  run_dpds start
  assert_rc_nonzero
  assert_says_to_install_first
}

test_the_unit_is_looked_for_in_the_configured_systemd_dir() {
  # a unit in some other directory does not count, one in DPDS_SYSTEMD_DIR does
  mkdir -p "$T/elsewhere"
  printf '[Service]\n' >"$T/elsewhere/dpds-controller.service"
  run_dpds start
  assert_rc_nonzero
  assert_says_to_install_first
  seed_unit
  run_dpds start
  assert_rc 0 "$ERR"
}

# ------------------------------------------------------------------ tests: stateless controller, host check exit codes
# Exit codes of `dpds-controller check` are handled generically: 0 = fine, anything else = hints worth a warning and one
# question, never a blocker.

# Any non-zero host check: a warning naming the exit code, the controller's output passed on, asked once, not blocking.
assert_generic_host_check_warning() { # assert_generic_host_check_warning CODE
  assert_all_has "exit code $1"
  assert_all_has "CHECK-OUTPUT-MARKER"
  ! grep -Eiq 'will not start|not start|cannot start|from starting|state file' <<<"$OUT$ERR" \
    || fail "the host check result is reported as a start blocker or a state problem: $OUT$ERR"
  ! grep -q '^\[ERROR\]\|\[ERROR\]' <<<"$ERR" || fail "a host check result is reported as an [ERROR]: $ERR"
}

test_install_treats_exit_code_2_of_the_check_like_any_other_hint_exit() {
  export STUB_CONTROLLER_RC=2 STUB_CONTROLLER_ERR="dpds-controller: some message"
  feed $'y\n'
  run_dpds install
  assert_rc 0 "$ERR"
  assert_generic_host_check_warning 2
  assert_out_has "Continue with the installation"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file "$(installed_unit)"
  assert_log_has '^systemctl daemon-reload'
}

test_install_asks_once_after_exit_code_2_and_aborts_on_no() {
  export STUB_CONTROLLER_RC=2
  feed $'n\n'
  run_dpds install
  assert_rc 1
  assert_out_has "Continue with the installation"
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(installed_unit)"
  assert_log_lacks '^systemctl'
}

test_install_with_yes_goes_on_after_exit_code_2_without_asking() {
  export STUB_CONTROLLER_RC=2 STUB_CONTROLLER_ERR="dpds-controller: some message"
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_generic_host_check_warning 2
  ! grep -Eiq 'continue with the installation|\[y/N\]' <<<"$OUT$ERR" || fail "asked although --yes was given: $OUT"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file "$(installed_unit)"
}

test_every_nonzero_exit_of_the_check_is_handled_the_same_way() {
  local rc
  for rc in 1 2 3 7; do
    : >"$STUB_LOG"
    rm -rf "$DPDS_PREFIX/bin/dpds-controller" "$DPDS_ETC_DIR" "$(installed_unit)"; seed_network physical
    STUB_CONTROLLER_RC=$rc run_dpds --yes install
    CTX="check exit $rc" assert_rc 0 "$ERR"
    CTX="check exit $rc" assert_generic_host_check_warning "$rc"
    CTX="check exit $rc" assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  done
}

test_ordinary_host_hints_are_still_only_hints() {
  # exit 1 = an error-level hint (no hugepages, no isolation ...): feedback, asked once, never blocking
  export STUB_CONTROLLER_RC=1
  feed $'y\n'
  run_dpds install
  assert_rc 0 "$ERR"
  assert_all_has "not blockers"
  assert_out_has "Continue with the installation"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file "$(installed_unit)"
}

test_update_goes_on_after_exit_code_2_with_yes_and_restarts_an_active_service() {
  fake_install
  export STUB_ACTIVE=1 STUB_CONTROLLER_RC=2 STUB_CONTROLLER_ERR="dpds-controller: some message"
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_generic_host_check_warning 2
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
  assert_log_has '^systemctl enable --now dpds-controller$'
}

test_update_asks_after_exit_code_2_and_leaves_the_service_alone_on_no() {
  fake_install
  export STUB_ACTIVE=1 STUB_CONTROLLER_RC=2
  feed $'n\n'
  run_dpds update
  assert_rc_nonzero
  ! grep -q 'STUB-BINARY' "$DPDS_PREFIX/bin/dpds-controller" || fail "the installed controller was replaced"
  assert_log_lacks '^systemctl (enable|disable|start|stop|restart|daemon-reload)'
}

test_the_standalone_check_passes_the_exit_code_through_unchanged() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  export STUB_CONTROLLER_RC=2 STUB_CONTROLLER_ERR="dpds-controller: some message"
  run_dpds check
  assert_rc 2
  assert_all_has "some message"
}

test_the_host_check_is_called_with_no_state_argument() {
  run_dpds --yes install
  assert_rc 0 "$ERR"
  local call; call="$(grep '^dpds-controller check' "$STUB_LOG" | head -1)"
  [[ -n "$call" ]] || fail "the host check was not called"
  ! grep -Eiq 'state|--state-dir|var/lib' <<<"$call" || fail "the check got a state argument: $call"
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  : >"$STUB_LOG"
  run_dpds check
  call="$(grep '^dpds-controller check' "$STUB_LOG" | head -1)"
  [[ -n "$call" ]] || fail "the standalone check did not call the controller"
  ! grep -Eiq 'state|--state-dir|var/lib' <<<"$call" || fail "the standalone check got a state argument: $call"
}

test_the_script_has_no_state_dir_setting_or_state_step() {
  local code; code="$(grep -v '^[[:space:]]*#' "$SRC_ROOT/dpds.sh")"
  ! grep -Eq 'DPDS_STATE_DIR|DPDS_CONTROLLER_STATE_DIR|STATE_DIR' <<<"$code" || fail "dpds.sh still has a state dir setting: $(grep -En 'STATE_DIR' <<<"$code" | head -3)"
  ! grep -Eiq 'state file|not a valid state|rc == 2|exit code 2' <<<"$code" || fail "dpds.sh still has the unloadable-state-file branch"
  ! grep -q '/var/lib' <<<"$code" || fail "dpds.sh names a directory below /var/lib: $(grep -n '/var/lib' <<<"$code")"
}

test_a_state_dir_variable_in_the_environment_changes_nothing() {
  # not read, not validated, not created
  local bad
  for bad in "$T/ignored-state" "relative/state" "with space/state" ""; do
    : >"$STUB_LOG"
    rm -rf "$DPDS_PREFIX/bin/dpds-controller" "$DPDS_ETC_DIR" "$(installed_unit)"; seed_network physical
    DPDS_STATE_DIR="$bad" run_dpds --yes install
    CTX="DPDS_STATE_DIR='$bad'" assert_rc 0 "$ERR"
    CTX="DPDS_STATE_DIR='$bad'" assert_exec "$DPDS_PREFIX/bin/dpds-controller"
    ! grep -qi 'STATE' "$DPDS_ETC_DIR/controller.env" || fail "the env file mentions state with DPDS_STATE_DIR='$bad'"
  done
  assert_no_file "$T/ignored-state"
  assert_no_file "$T/relative"
  assert_no_state_dir_anywhere
}

test_the_unit_file_has_no_state_directory() {
  local u="$SRC_ROOT/packaging/dpds-controller.service"
  ! grep -Eiq 'StateDirectory|RuntimeDirectory=.*state|/var/lib|STATE_DIR|state dir' "$u" || fail "the unit still names a state directory: $(grep -Ein 'StateDirectory|/var/lib|STATE_DIR|state dir' "$u")"
  # the installed copy too
  run_dpds --yes install
  assert_rc 0 "$ERR"
  ! grep -Eiq 'StateDirectory|/var/lib|STATE_DIR|state dir|legacy' "$(installed_unit)" || fail "the installed unit names a state directory"
}

# ------------------------------------------------------------------ tests: update is a no-op when up to date
stamp_file() { echo "$DPDS_ETC_DIR/installed-commit"; }
write_stamp() { mkdir -p "$DPDS_ETC_DIR"; printf '%s\n' "$1" >"$(stamp_file)"; }
# A complete install (binaries, unit, env) whose stamp names COMMIT; the git stub's HEAD is COMMIT as well.
fake_stamped_install() { fake_install; write_stamp "$1"; export STUB_GIT_HEAD_COMMIT="$1"; }
# Snapshot of everything an install writes, to prove a no-op touched none of it.
installed_snapshot() {
  cat "$DPDS_PREFIX/bin/dpds" "$DPDS_PREFIX/bin/dpds-controller" "$(installed_unit)" "$DPDS_ETC_DIR/controller.env" "$(stamp_file)" 2>&1
  ls -l --time-style=full-iso "$DPDS_PREFIX/bin" "$DPDS_SYSTEMD_DIR" "$DPDS_ETC_DIR" 2>&1
}
assert_update_was_a_noop() {
  local commit="$1"
  assert_rc 0 "$ERR"
  assert_all_has "up to date"
  assert_all_has "${commit:0:7}"
  assert_all_has "install"
  assert_all_has "rebuild"
  # the git steps still ran; nothing was built, checked or installed; no systemctl call that changes anything
  assert_log_has '^git fetch'
  assert_log_has '^git checkout '
  assert_log_has '^git pull --ff-only$'
  assert_log_lacks '^npm (ci|run)'
  assert_log_lacks '^cargo build'
  assert_log_lacks '^dpds-controller check'
  assert_log_lacks '^systemctl (daemon-reload|enable|disable|start|stop|restart|reload)'
  assert_no_file "$REPO_ROOT/frontend/dist"
  assert_eq "$(installed_snapshot)" "$SNAPSHOT" "installed files, unit, env or stamp touched"
}
assert_update_was_a_full_update() {
  assert_rc 0 "$ERR"
  assert_log_has '^npm ci( |$)'
  assert_log_has '^cargo build .*(-p|--package|--bin)[ =]dpds-controller( |$)'
  assert_log_has '^systemctl daemon-reload'
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
  assert_file_has "$DPDS_PREFIX/bin/dpds" 'STUB-BINARY-dpds'
}

test_update_is_a_noop_when_the_installed_commit_is_the_head_and_the_service_is_stopped() {
  fake_stamped_install deadbeefcafe0123456789deadbeefcafe012345
  export STUB_ACTIVE=0
  SNAPSHOT="$(installed_snapshot)"
  run_dpds update   # no --yes: there must be no confirmation question either
  assert_update_was_a_noop "$STUB_GIT_HEAD_COMMIT"
}

test_update_is_a_noop_when_the_installed_commit_is_the_head_and_the_service_is_active() {
  fake_stamped_install deadbeefcafe0123456789deadbeefcafe012345
  export STUB_ACTIVE=1
  SNAPSHOT="$(installed_snapshot)"
  run_dpds --yes update
  assert_update_was_a_noop "$STUB_GIT_HEAD_COMMIT"
}

test_update_noop_works_on_the_nightly_channel_and_for_a_release_tag() {
  fake_stamped_install 1111111111111111111111111111111111111111
  export STUB_GIT_BRANCH=nightly
  SNAPSHOT="$(installed_snapshot)"
  run_dpds --nightly --yes update
  assert_update_was_a_noop "$STUB_GIT_HEAD_COMMIT"
  : >"$STUB_LOG"
  fake_stamped_install commit-of-v1.0.0
  export STUB_GIT_TAGS="v1.0.0"
  SNAPSHOT="$(installed_snapshot)"
  DPDS_CHANNEL=v1.0.0 run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_all_has "up to date"
  assert_log_has '^git checkout --detach v1\.0\.0$'
  assert_log_lacks '^(npm (ci|run)|cargo build)'
  assert_log_lacks '^systemctl (daemon-reload|enable|disable|start|stop|restart)'
  assert_eq "$(installed_snapshot)" "$SNAPSHOT" "installed files, unit, env or stamp touched"
}

test_update_with_a_different_installed_commit_does_the_full_update_and_stamps_the_new_head() {
  fake_stamped_install aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  export STUB_GIT_HEAD_COMMIT=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
  run_dpds --yes update
  assert_update_was_a_full_update
  assert_eq "$(cat "$(stamp_file)")" "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" "stamp"
  assert_eq "$(wc -l <"$(stamp_file)" | tr -d ' ')" 1 "stamp is one line"
}

test_update_without_a_stamp_does_the_full_update_and_writes_it() {
  fake_install
  export STUB_GIT_HEAD_COMMIT=cccccccccccccccccccccccccccccccccccccccc
  run_dpds --yes update
  assert_update_was_a_full_update
  assert_eq "$(cat "$(stamp_file)")" "cccccccccccccccccccccccccccccccccccccccc" "stamp"
}

test_update_with_a_stamp_but_missing_installed_files_does_the_full_update() {
  local missing
  for missing in "$DPDS_PREFIX/bin/dpds" "$DPDS_PREFIX/bin/dpds-controller" "$DPDS_SYSTEMD_DIR/dpds-controller.service"; do
    : >"$STUB_LOG"
    fake_stamped_install dddddddddddddddddddddddddddddddddddddddd
    rm -f "$missing"
    CTX="missing ${missing##*/}" run_dpds --yes update
    CTX="missing ${missing##*/}" assert_update_was_a_full_update
    CTX="missing ${missing##*/}" assert_file "$(installed_unit)"
    CTX="missing ${missing##*/}" assert_eq "$(cat "$(stamp_file)")" "dddddddddddddddddddddddddddddddddddddddd" "stamp"
  done
}

test_update_noop_still_needs_the_prerequisites_a_clean_tree_and_working_git() {
  fake_stamped_install eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee
  remove_stub cargo
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "cargo"
  assert_log_lacks '^git '
  setup_sandbox; fake_stamped_install eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee
  STUB_GIT_DIRTY=1 run_dpds --yes update
  assert_rc_nonzero
  assert_log_lacks '^git (checkout|pull)'
  setup_sandbox; fake_stamped_install eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee
  STUB_GIT_FAIL_SUB=pull run_dpds --yes update
  assert_rc_nonzero
  assert_log_lacks '^(npm (ci|run)|cargo build)'
}

test_install_writes_the_stamp_with_the_head_commit() {
  export STUB_GIT_HEAD_COMMIT=0123456789abcdef0123456789abcdef01234567
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_eq "$(cat "$(stamp_file)")" "0123456789abcdef0123456789abcdef01234567" "stamp"
  assert_eq "$(wc -l <"$(stamp_file)" | tr -d ' ')" 1 "stamp is one line"
  assert_no_state_dir_anywhere
}

test_install_always_rebuilds_even_when_the_stamp_matches_and_rewrites_it() {
  fake_stamped_install ffffffffffffffffffffffffffffffffffffffff
  run_dpds --yes install
  assert_update_was_a_full_update
  assert_eq "$(cat "$(stamp_file)")" "ffffffffffffffffffffffffffffffffffffffff" "stamp"
}

test_install_replaces_an_older_stamp() {
  fake_stamped_install oldoldoldoldoldoldoldoldoldoldoldoldold0
  export STUB_GIT_HEAD_COMMIT=newnewnewnewnewnewnewnewnewnewnewnewnew0
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_eq "$(cat "$(stamp_file)")" "newnewnewnewnewnewnewnewnewnewnewnewnew0" "stamp"
}

test_a_declined_or_failed_install_leaves_the_existing_stamp_alone() {
  fake_stamped_install oldoldoldoldoldoldoldoldoldoldoldoldold0
  feed $'n\n'
  run_dpds install
  assert_rc 1
  assert_eq "$(cat "$(stamp_file)" 2>&1)" "oldoldoldoldoldoldoldoldoldoldoldoldold0" "stamp after a declined install"
  local failure
  for failure in STUB_CARGO_FAIL=1 STUB_NPM_FAIL_SUB=ci STUB_NPM_FAIL_SUB=run; do
    setup_sandbox
    fake_stamped_install oldoldoldoldoldoldoldoldoldoldoldoldold0
    export "$failure"
    run_dpds --yes install
    CTX="$failure" assert_rc_nonzero
    CTX="$failure" assert_eq "$(cat "$(stamp_file)" 2>&1)" "oldoldoldoldoldoldoldoldoldoldoldoldold0" "stamp after a failed build"
  done
}

test_an_install_that_fails_halfway_leaves_no_stamp_so_the_next_update_rebuilds() {
  fake_stamped_install 9999999999999999999999999999999999999999
  STUB_SYSTEMCTL_FAIL_SUB=daemon-reload run_dpds --yes install
  assert_rc_nonzero
  assert_no_file "$(stamp_file)"
  # the stamp matched HEAD before: without the removal the next update would wrongly be a no-op
  : >"$STUB_LOG"
  run_dpds --yes update
  assert_update_was_a_full_update
  assert_eq "$(cat "$(stamp_file)")" "9999999999999999999999999999999999999999" "stamp"
}

test_a_failed_first_install_writes_no_stamp() {
  STUB_SYSTEMCTL_FAIL_SUB=daemon-reload run_dpds --yes install
  assert_rc_nonzero
  assert_no_file "$(stamp_file)"
}

test_install_from_a_tree_without_git_writes_no_stamp_and_drops_an_old_one() {
  export STUB_GIT_NOT_A_REPO=1
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_no_file "$(stamp_file)"
  # a previous stamp must not survive an install that cannot say which commit it installed
  write_stamp oldoldoldoldoldoldoldoldoldoldoldoldold0
  run_dpds --yes install
  assert_rc 0 "$ERR"
  assert_no_file "$(stamp_file)"
}

test_uninstall_removes_the_stamp_and_keeps_the_env_file() {
  fake_stamped_install 7777777777777777777777777777777777777777
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  assert_no_file "$(stamp_file)"
  assert_file "$DPDS_ETC_DIR/controller.env"
}

test_update_after_an_uninstall_reinstalls() {
  fake_stamped_install 6666666666666666666666666666666666666666
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  : >"$STUB_LOG"
  run_dpds --yes update
  assert_update_was_a_full_update
  assert_eq "$(cat "$(stamp_file)")" "6666666666666666666666666666666666666666" "stamp"
}

# ------------------------------------------------------------------ tests: network mode (physical | virtual)
# A first install has no stored mode: the tests that exercise it remove the file the sandbox seeds.
no_stored_network() { rm -f "$DPDS_ETC_DIR/network"; }
no_systemd() { rm -rf "$DPDS_SYSTEMD_RUN_DIR"; }
# What the stub controller found in its --etc-dir at each `check`/`serve` call.
controller_saw() { cat "$STUB_LOG.network" 2>/dev/null | tr '\n' ' '; }
assert_refused_before_building() {
  assert_log_lacks '^npm (ci|run)'
  assert_log_lacks '^cargo build'
  assert_no_file "$DPDS_PREFIX/bin/dpds"
  assert_no_file "$DPDS_PREFIX/bin/dpds-controller"
}

test_network_option_is_accepted_before_the_command_in_any_order() {
  local args
  for args in "--network virtual --yes install" "--yes --network virtual install" "--yes --nightly --network virtual install"; do
    setup_sandbox
    no_stored_network
    # shellcheck disable=SC2086
    run_dpds $args
    CTX="$args" assert_rc 0 "$ERR"
    CTX="$args" assert_eq "$(stored_network)" "virtual" "stored mode"
  done
}

test_network_option_only_belongs_to_install_and_update() {
  local c
  for c in check start stop restart status uninstall; do
    : >"$STUB_LOG"
    run_dpds --network virtual "$c"
    CTX="$c" assert_rc_nonzero
    CTX="$c" assert_err_has "--network"
    CTX="$c" assert_log_empty
    CTX="$c" assert_eq "$(stored_network)" "physical" "stored mode"
  done
}

test_network_option_needs_physical_or_virtual() {
  local args
  for args in "--network bogus --yes install" "--network Virtual --yes install" "--network= --yes install"; do
    : >"$STUB_LOG"
    # shellcheck disable=SC2086
    run_dpds $args
    CTX="$args" assert_rc_nonzero
    CTX="$args" assert_err_has "--network"
    CTX="$args" assert_err_has "virtual"
    CTX="$args" assert_log_empty
  done
  # a missing value must not swallow the command
  : >"$STUB_LOG"
  run_dpds --yes --network install
  assert_rc_nonzero
  assert_err_has "--network"
  assert_log_empty
}

test_a_first_install_without_the_option_fails_before_building() {
  no_stored_network
  run_dpds --yes install
  assert_rc_nonzero
  assert_err_has "--network"
  assert_err_has "physical"
  assert_err_has "virtual"
  assert_refused_before_building
  assert_no_file "$DPDS_ETC_DIR/network"
}

test_stored_content_other_than_physical_or_virtual_counts_as_no_mode() {
  local bad
  for bad in "banana" "" "physical virtual" "Virtual"; do
    : >"$STUB_LOG"
    seed_network "$bad"
    run_dpds --yes install
    CTX="stored '$bad'" assert_rc_nonzero
    CTX="stored '$bad'" assert_err_has "--network"
    CTX="stored '$bad'" assert_refused_before_building
  done
}

test_update_without_a_stored_mode_and_without_the_option_fails_before_building() {
  fake_install
  no_stored_network
  run_dpds --yes update
  assert_rc_nonzero
  assert_err_has "--network"
  assert_log_lacks '^npm (ci|run)'
  assert_log_lacks '^cargo build'
}

test_the_option_stores_the_mode_as_one_line() {
  no_stored_network
  run_dpds --yes --network physical install
  assert_rc 0 "$ERR"
  assert_eq "$(stored_network)" "physical" "stored mode"
  assert_eq "$(wc -l <"$DPDS_ETC_DIR/network" | tr -d ' ')" 1 "one line"
  : >"$STUB_LOG"
  run_dpds --yes --network virtual install
  assert_rc 0 "$ERR"
  assert_eq "$(cmp "$DPDS_ETC_DIR/network" <(printf 'virtual\n') && echo same)" "same" "exactly 'virtual' and a newline"
}

test_install_and_update_without_the_option_keep_the_stored_mode() {
  local m
  for m in virtual physical; do
    setup_sandbox
    seed_network "$m"
    run_dpds --yes install
    CTX="install, stored $m" assert_rc 0 "$ERR"
    CTX="install, stored $m" assert_eq "$(stored_network)" "$m" "stored mode"
    : >"$STUB_LOG"
    rm -f "$(stamp_file)" # the installed commit is HEAD: without this update would have nothing to do
    run_dpds --yes update
    CTX="update, stored $m" assert_update_was_a_full_update
    CTX="update, stored $m" assert_eq "$(stored_network)" "$m" "stored mode"
  done
}

test_the_option_switches_the_stored_mode_on_install_and_on_update() {
  seed_network physical
  run_dpds --yes --network virtual install
  assert_rc 0 "$ERR"
  assert_eq "$(stored_network)" "virtual" "stored mode after install"
  : >"$STUB_LOG"
  rm -f "$(stamp_file)"
  run_dpds --yes --network physical update
  assert_update_was_a_full_update
  assert_eq "$(stored_network)" "physical" "stored mode after update"
}

test_the_host_check_of_install_judges_the_mode_being_installed() {
  # the controller reads the mode from its etc dir, so it must see the new one at the host check
  no_stored_network
  run_dpds --yes --network virtual install
  assert_rc 0 "$ERR"
  assert_log_has '^dpds-controller check( .*)?--etc-dir[ =]'"$DPDS_ETC_DIR"
  assert_eq "$(controller_saw)" "virtual " "mode the host check saw"
  : >"$STUB_LOG.network"
  seed_network virtual
  run_dpds --yes --network physical install
  assert_rc 0 "$ERR"
  assert_eq "$(controller_saw)" "physical " "mode the host check saw after a switch"
}

test_a_declined_install_leaves_the_stored_mode_alone() {
  seed_network physical
  feed $'n\n'
  run_dpds --network virtual install
  assert_rc 1
  assert_eq "$(stored_network)" "physical" "stored mode"
  no_stored_network
  feed $'n\n'
  run_dpds --network virtual install
  assert_rc 1
  assert_no_file "$DPDS_ETC_DIR/network"
}

test_uninstall_keeps_the_stored_mode() {
  fake_install
  seed_network virtual
  run_dpds --yes uninstall
  assert_rc 0 "$ERR"
  assert_eq "$(stored_network)" "virtual" "stored mode"
}

test_the_controller_check_gets_the_etc_dir_only_when_it_is_not_the_default() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  run_dpds check
  assert_rc 0 "$ERR"
  assert_log_has '^dpds-controller check( .*)?--etc-dir[ =]'"$DPDS_ETC_DIR"'( |$)'
  : >"$STUB_LOG"
  DPDS_ETC_DIR=/etc/dpds run_dpds check
  assert_rc 0 "$ERR"
  assert_log_has '^dpds-controller check'
  assert_log_lacks '--etc-dir'
}

test_check_in_virtual_mode_needs_the_af_packet_driver_in_dpdk() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  seed_network virtual
  run_dpds check
  assert_rc_nonzero
  assert_err_has "net_af_packet"
  # a static library, a shared library, a PMD in the plugin directory
  touch "$STUB_DPDK_LIBDIR/librte_net_af_packet.a"
  run_dpds check
  assert_rc 0 "$ERR"
  rm "$STUB_DPDK_LIBDIR/librte_net_af_packet.a"
  touch "$STUB_DPDK_LIBDIR/librte_net_af_packet.so.26.0"
  run_dpds check
  assert_rc 0 "$ERR"
  rm "$STUB_DPDK_LIBDIR/librte_net_af_packet.so.26.0"
  run_dpds check
  assert_rc_nonzero
  mkdir -p "$STUB_DPDK_LIBDIR/dpdk/pmds-26.0"
  touch "$STUB_DPDK_LIBDIR/dpdk/pmds-26.0/librte_net_af_packet.so.26.0"
  run_dpds check
  assert_rc 0 "$ERR"
}

test_check_in_physical_mode_does_not_look_for_the_af_packet_driver() {
  write_controller_stub "$DPDS_PREFIX/bin/dpds-controller"
  seed_network physical
  run_dpds check
  assert_rc 0 "$ERR"
}

# ------------------------------------------------------------------ tests: no systemd (a container)
controller_command() {
  echo "DPDS_CONTROLLER_BIND=${1:-0.0.0.0:8100} DPDS_CONTROLLER_DPDS_BIN=$DPDS_PREFIX/bin/dpds $DPDS_PREFIX/bin/dpds-controller serve"
}

test_install_without_systemd_installs_everything_but_the_unit_and_says_how_to_start() {
  no_systemd
  DPDS_BIND=127.0.0.1:9000 run_dpds --yes --network virtual install
  assert_rc 0 "$ERR"
  assert_exec "$DPDS_PREFIX/bin/dpds"
  assert_exec "$DPDS_PREFIX/bin/dpds-controller"
  assert_file_has "$DPDS_ETC_DIR/controller.env" '^DPDS_CONTROLLER_BIND="?127\.0\.0\.1:9000"?$'
  assert_eq "$(stored_network)" "virtual" "stored mode"
  assert_no_file "$(installed_unit)"
  assert_log_lacks '^systemctl'
  assert_all_has "$(controller_command 127.0.0.1:9000)"
}

test_install_with_systemd_still_installs_the_unit_and_reloads() {
  run_dpds --yes --network virtual install
  assert_rc 0 "$ERR"
  assert_file "$(installed_unit)"
  assert_log_has '^systemctl daemon-reload'
}

test_start_stop_restart_status_without_systemd_call_no_systemctl() {
  fake_install
  rm -f "$(installed_unit)"
  no_systemd
  local c
  for c in start restart status; do
    : >"$STUB_LOG"
    run_dpds "$c"
    CTX="$c" assert_rc 0 "$ERR"
    CTX="$c" assert_log_lacks '^systemctl'
    CTX="$c" assert_all_has "$(controller_command)"
  done
  : >"$STUB_LOG"
  run_dpds stop
  assert_rc 0 "$ERR"
  assert_log_lacks '^systemctl'
}

test_start_and_restart_without_systemd_need_the_installed_binaries_not_the_unit() {
  no_systemd
  local c
  for c in start restart; do
    : >"$STUB_LOG"
    run_dpds "$c"
    CTX="$c" assert_rc_nonzero
    CTX="$c" assert_all_has "not installed"
    CTX="$c" assert_log_lacks '^systemctl'
  done
}

test_update_without_systemd_does_not_ask_systemctl_and_does_not_restart() {
  fake_install
  rm -f "$(installed_unit)"
  no_systemd
  export STUB_ACTIVE=1
  run_dpds --yes update
  assert_rc 0 "$ERR"
  assert_log_has '^npm ci( |$)'
  assert_log_has '^cargo build .*(-p|--package|--bin)[ =]dpds-controller( |$)'
  assert_log_lacks '^systemctl'
  assert_file_has "$DPDS_PREFIX/bin/dpds-controller" 'STUB-BINARY-dpds-controller'
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

