//! Bad `serve` options are a clean startup error
//! (a message on stderr naming the option, a non-zero exit code that is neither a panic
//! (101) nor an abort (signal / 134)), never a running server.
//!
//! - `--cors-origin` rejects `*` and any value with a path or a trailing slash;
//! - `--runtime-dir` is rejected when the resulting telemetry socket path
//!   `<runtime-dir>/instance/dpdk/dpds-ctl/dpdk_telemetry.v2` (41 bytes more than the
//!   directory) does not fit a unix socket address (107 bytes) or DPDK's runtime-dir limit.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::common::*;

/// Runs `dpds-controller <args>` on a free loopback address and waits for it to exit.
/// A process that keeps running (it accepted the options) is killed and fails the test.
fn run_to_exit(args: &[&str], envs: &[(&str, &str)]) -> (Option<i32>, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dpds-controller"));
    cmd.args(args)
        .arg("--bind")
        .arg(free_addr())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("cannot spawn dpds-controller");
    let t0 = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break Some(st);
        }
        if t0.elapsed() > Duration::from_secs(10) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("dpds-controller {args:?} {envs:?} kept running: the option was accepted");
    };
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    (status.code(), stderr)
}

/// A clean refusal: exit code 1..=100 or 102..=133, 135.. (not a panic, not an abort),
/// a message that mentions `needle`, no panic report.
fn assert_clean_refusal(args: &[&str], envs: &[(&str, &str)], needle: &str) {
    let (code, stderr) = run_to_exit(args, envs);
    let what = format!("{args:?} {envs:?}: exit {code:?}, stderr {stderr:?}");
    match code {
        Some(c) => assert!(c != 0 && c != 101 && c != 134, "{what}"),
        None => panic!("killed by a signal (abort?): {what}"),
    }
    assert!(!stderr.contains("panicked"), "{what}");
    assert!(
        stderr.to_lowercase().contains(needle),
        "stderr must mention {needle:?}: {what}"
    );
}

// ---- S3: --cors-origin -----------------------------------------------------------

#[test]
fn a_wildcard_cors_origin_is_a_clean_startup_error() {
    assert_clean_refusal(&["serve", "--cors-origin", "*"], &[], "cors-origin");
    // serve is the default command
    assert_clean_refusal(&["--cors-origin", "*"], &[], "cors-origin");
    // one bad value among good ones
    assert_clean_refusal(
        &[
            "serve",
            "--cors-origin",
            "http://ui.example",
            "--cors-origin",
            "*",
        ],
        &[],
        "cors-origin",
    );
}

#[test]
fn a_cors_origin_with_a_path_or_trailing_slash_is_a_clean_startup_error() {
    for bad in [
        "http://x/",
        "http://x:8080/",
        "http://x/path",
        "https://ui.example/app/",
    ] {
        assert_clean_refusal(&["serve", "--cors-origin", bad], &[], "cors-origin");
    }
}

#[test]
fn the_cors_origin_environment_variable_is_checked_too() {
    assert_clean_refusal(&["serve"], &[("DPDS_CORS_ORIGIN", "*")], "cors-origin");
    assert_clean_refusal(
        &["serve"],
        &[("DPDS_CORS_ORIGIN", "http://ok.example,http://x/")],
        "cors-origin",
    );
}

#[test]
fn plain_origins_are_still_accepted() {
    // scheme://host[:port] without a path is what browsers send in `Origin`
    let c = ControllerProcess::start(
        &[
            "serve",
            "--bind",
            "{addr}",
            "--cors-origin",
            "http://ui.example:8080",
            "--cors-origin",
            "https://other.example",
        ],
        &[],
    );
    let (st, _, _) = get_with_origin(&c.addr, "/api/online", "https://other.example");
    assert_eq!(st, 200);
}

// ---- S5: --runtime-dir too long for DPDK -------------------------------------------

#[test]
fn a_runtime_dir_whose_telemetry_socket_path_is_too_long_is_a_clean_startup_error() {
    // 70 bytes: short in itself, but + 41 bytes of `/instance/dpdk/dpds-ctl/dpdk_telemetry.v2`
    // is 111 bytes, over the 107 of a unix socket path
    let medium = format!("/tmp/{}", "r".repeat(65));
    // 200 bytes: over any limit
    let long = format!("/tmp/{}", "r".repeat(195));
    for rt in [medium, long] {
        assert_clean_refusal(&["serve", "--runtime-dir", &rt], &[], "runtime");
        let (_, stderr) = run_to_exit(&["serve", "--runtime-dir", &rt], &[]);
        let s = stderr.to_lowercase();
        assert!(
            s.contains("107") || s.contains("108") || s.contains("limit") || s.contains("too long"),
            "the message names the limit: {stderr:?}"
        );
    }
}

#[test]
fn a_short_runtime_dir_starts() {
    let rt = format!("/tmp/dpds-rt-{}", std::process::id());
    let c = ControllerProcess::start(&["serve", "--bind", "{addr}", "--runtime-dir", &rt], &[]);
    let (st, _, _) = get_with_origin(&c.addr, "/api/online", "http://x.example");
    assert_eq!(st, 200);
}
