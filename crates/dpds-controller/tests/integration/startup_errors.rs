//! Bad `serve` options are a clean startup error
//! (a message on stderr naming the option, a non-zero exit code that is neither a panic
//! (101) nor an abort (signal / 134)), never a running server.
//!
//! - `--cors-origin` rejects `*` and any value with a path or a trailing slash;
//! - without a stored network mode (`<etc-dir>/network` missing or not `physical`/`virtual`) both `serve`
//!   and `check` exit 2 and say how to store one (`dpds.sh install --network ...`);
//! - `--runtime-dir` is rejected when the resulting telemetry socket path
//!   `<runtime-dir>/instance/dpdk/dpds-ctl/dpdk_telemetry.v2` (41 bytes more than the
//!   directory) does not fit a unix socket address (107 bytes) or DPDK's runtime-dir limit.

use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::common::*;
use dpds_controller::host::Network;

/// Runs `dpds-controller <args>` on a free loopback address and waits for it to exit.
/// A process that keeps running (it accepted the options) is killed and fails the test.
fn run_to_exit(args: &[&str], envs: &[(&str, &str)]) -> (Option<i32>, String) {
    let mut cmd = controller_cmd();
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

// ---- the stored network mode --------------------------------------------------------------------

/// An etc dir whose `network` file holds `content` (no file for None).
fn etc_with(content: Option<&str>) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    if let Some(c) = content {
        std::fs::write(d.path().join("network"), c).unwrap();
    }
    d
}

#[test]
fn the_stored_mode_is_physical_or_virtual_and_nothing_else() {
    assert_eq!(
        Network::read(etc_with(Some("physical\n")).path()),
        Ok(Network::Physical)
    );
    assert_eq!(
        Network::read(etc_with(Some("virtual\n")).path()),
        Ok(Network::Virtual)
    );
    let e = Network::read(etc_with(None).path()).unwrap_err();
    assert!(e.contains("install --network"), "{e}");
    for bad in ["", "\n", "Physical\n", "banana\n", "physical\nvirtual\n"] {
        let e = Network::read(etc_with(Some(bad)).path()).unwrap_err();
        assert!(e.contains("install --network"), "{bad:?}: {e}");
    }
    let e = Network::read(etc_with(Some("banana\n")).path()).unwrap_err();
    assert!(e.contains("banana"), "the content is named: {e}");
    assert_eq!(serde_json::to_value(Network::Virtual).unwrap(), "virtual");
    assert_eq!(serde_json::to_value(Network::Physical).unwrap(), "physical");
}

#[test]
fn without_a_stored_mode_serve_and_check_exit_2_and_say_how_to_store_one() {
    for content in [None, Some(""), Some("banana\n")] {
        let d = etc_with(content);
        let etc = d.path().to_str().unwrap();
        for envs in [&[][..], &[("DPDS_CONTROLLER_ETC_DIR", etc)][..]] {
            // the option and the environment variable name the same directory
            let args: &[&str] = if envs.is_empty() {
                &["serve", "--etc-dir", etc]
            } else {
                &["serve"]
            };
            let (code, stderr) = run_to_exit(args, envs);
            assert_eq!(code, Some(2), "{content:?} {args:?}: {stderr}");
            assert!(
                stderr.contains("install --network"),
                "{content:?}: {stderr}"
            );
        }
        let o = controller_cmd()
            .args(["check", "--etc-dir", etc])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{content:?}");
        assert!(String::from_utf8_lossy(&o.stderr).contains("install --network"));
    }
}

#[test]
fn the_stored_mode_is_what_the_running_controller_reports() {
    for mode in ["physical", "virtual"] {
        let d = etc_with(Some(&format!("{mode}\n")));
        let etc = d.path().to_str().unwrap();
        let by_option =
            ControllerProcess::start(&["serve", "--bind", "{addr}", "--etc-dir", etc], &[]);
        let (st, v) = http(&by_option.addr, "GET", "/api/online", None).unwrap();
        assert_eq!((st, v["network"].as_str()), (200, Some(mode)));
        let by_env = ControllerProcess::start(
            &["serve", "--bind", "{addr}"],
            &[("DPDS_CONTROLLER_ETC_DIR", etc)],
        );
        let (_, v) = http(&by_env.addr, "GET", "/api/online", None).unwrap();
        assert_eq!(v["network"], mode);
    }
}
