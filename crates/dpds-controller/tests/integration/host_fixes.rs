//! C5, host and API part: hugepage need maths, `nohz_full` `(null)`, offline isolated
//! cores, blocking file reads in handlers, concurrent starts and `--bind` without the
//! `serve` subcommand.

use std::ffi::CString;
use std::sync::mpsc;
use std::time::Duration;

use crate::common::*;
use dpds_controller::lcores::assign_lcores;
use serde_json::json;

// ---- hugepage need saturates -------------------------------------------------------

#[test]
fn an_absurd_buffer_capacity_makes_the_hugepage_need_saturate_not_wrap() {
    // u64::MAX buffers: (capacity + pool slack) * 1920 B overflows u64. A wrapped
    // (small) need would turn this "ok"; a saturated one cannot fit.
    let host = FakeHost::bare_metal(); // 2 x 2048 x 2 MiB free
    let mut cfg = cfg_json(1.0e6, 0.0);
    cfg["buffer_capacity"] = json!(u64::MAX);
    let mut links = vec![
        link(1, "a", &["net_null0"], None),
        link(2, "b", &["net_null1"], None),
    ];
    for l in &mut links {
        l.configs = vec![dto_from(cfg.clone())];
    }
    let hints = hints_json(&host, &state_with(links));
    assert_eq!(
        hint(&hints, "hugepages")["level"],
        "error",
        "{}",
        hint(&hints, "hugepages")
    );
}

// ---- nohz_full "(null)" --------------------------------------------------------------

fn isolation_level(h: &FakeHost) -> (String, String) {
    let hints = hints_json(h, &Default::default());
    let i = hint(&hints, "isolation");
    (
        i["level"].as_str().unwrap().to_string(),
        format!("{} {}", i["title"], i["detail"]),
    )
}

#[test]
fn a_nohz_full_sysfs_value_of_null_counts_as_unset() {
    let h = FakeHost::bare_metal();
    h.cmdline("BOOT_IMAGE=/vmlinuz ro isolcpus=2-3,6-7 rcu_nocbs=2-3,6-7");
    h.write("sys/devices/system/cpu/nohz_full", "(null)\n");
    let (level, text) = isolation_level(&h);
    assert_ne!(level, "ok", "{text}");
    assert!(text.contains("nohz_full"), "{text}");
}

#[test]
fn a_real_nohz_full_sysfs_value_still_counts_as_set() {
    let h = FakeHost::bare_metal();
    h.cmdline("BOOT_IMAGE=/vmlinuz ro isolcpus=2-3,6-7 rcu_nocbs=2-3,6-7");
    h.write("sys/devices/system/cpu/nohz_full", "2-3,6-7\n");
    assert_eq!(isolation_level(&h).0, "ok");
}

// ---- auto-assigned cores are online ----------------------------------------------------

#[test]
fn isolated_cores_that_are_offline_are_never_assigned() {
    // 4 CPUs online, but the isolated list names 6-7 too
    let h = FakeHost::bare_metal();
    h.cpus(4, Some("2-3,6-7"), &[(0, "0-3")]);
    let a = assign_lcores(
        &h.roots(),
        &state_with(vec![
            link(1, "a", &["net_null0"], None),
            link(2, "b", &["net_null1"], None),
        ]),
    )
    .unwrap();
    let mut lcores: Vec<u32> = a.directions.iter().map(|d| d.lcore).collect();
    lcores.sort_unstable();
    assert_eq!(lcores, [2, 3]);
    // a third direction has no online isolated core left
    let e = assign_lcores(
        &h.roots(),
        &state_with(vec![
            link(1, "a", &["net_null0"], None),
            link(2, "b", &["net_null1"], None),
            link(3, "c", &["net_null2"], None),
        ]),
    )
    .unwrap_err()
    .to_string();
    assert!(e.to_lowercase().contains("isolated"), "{e}");
}

#[test]
fn an_isolated_core_in_a_hole_of_the_online_list_is_skipped() {
    let h = FakeHost::bare_metal();
    h.cpus(8, Some("1-3"), &[(0, "0-7")]);
    h.write("sys/devices/system/cpu/online", "0-1,3-7\n"); // cpu2 is offline
    let a = assign_lcores(
        &h.roots(),
        &state_with(vec![
            link(1, "a", &["net_null0"], None),
            link(2, "b", &["net_null1"], None),
        ]),
    )
    .unwrap();
    let mut lcores: Vec<u32> = a.directions.iter().map(|d| d.lcore).collect();
    lcores.sort_unstable();
    assert_eq!(lcores, [1, 3]);
}

// ---- blocking reads do not block the runtime -----------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handlers_that_read_sysfs_do_not_block_the_async_workers() {
    // `class` of a NIC is a FIFO nobody writes to: reading it blocks. Two such requests
    // would occupy both async workers if the read ran on them; with spawn_blocking
    // the workers stay free and /api/online answers.
    let host = FakeHost::bare_metal();
    let fifo = host.sysfs().join(format!("bus/pci/devices/{NIC0}/class"));
    std::fs::remove_file(&fifo).unwrap();
    let c = CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: plain libc call with a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    let a = TestApp::new(host);

    let blockers: Vec<_> = ["/api/interfaces", "/api/host/hints"]
        .into_iter()
        .map(|path| {
            let router = a.router.clone();
            tokio::spawn(async move { router_get(&router, path).await.status })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));

    let (tx, rx) = mpsc::channel();
    let router = a.router.clone();
    tokio::spawn(async move {
        let _ = tx.send(router_get(&router, "/api/online").await.status);
    });
    let got = rx.recv_timeout(Duration::from_secs(3));

    // release the readers whatever happened
    for _ in 0..2 {
        if let Ok(mut w) = std::fs::OpenOptions::new().write(true).open(&fifo) {
            use std::io::Write;
            let _ = w.write_all(b"0x020000\n");
        }
    }
    for b in blockers {
        let _ = b.await;
    }
    assert_eq!(
        got,
        Ok(200),
        "/api/online must answer while two handlers wait for a file"
    );
}

// ---- concurrent starts -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_starts_launch_exactly_once() {
    let a = TestApp::new(FakeHost::bare_metal());
    a.create_link("l", &["net_null0"]).await;
    // the supervisor needs 300 ms to take the plan: the window in which a second
    // start could pass the state check
    a.supervisor.set_start_delay(Duration::from_millis(300));
    let tasks: Vec<_> = (0..2)
        .map(|_| {
            let router = a.router.clone();
            tokio::spawn(async move {
                let req = axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/emulation/start")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{}"))
                    .unwrap();
                send(&router, req).await.status
            })
        })
        .collect();
    let mut statuses = Vec::new();
    for t in tasks {
        statuses.push(t.await.unwrap());
    }
    statuses.sort_unstable();
    assert_eq!(statuses, [202, 409], "one start wins, the other is refused");
    assert_eq!(
        a.supervisor.plans().len(),
        1,
        "the plan must not be overwritten"
    );
}

// ---- --bind without `serve` --------------------------------------------------------------------

#[test]
fn bind_without_the_serve_subcommand_is_honoured() {
    let c = ControllerProcess::start(&["--bind", "{addr}"], &[]);
    let (st, _) = http(&c.addr, "GET", "/api/online", None).unwrap();
    assert_eq!(st, 200);
}

#[test]
fn the_bind_env_without_the_serve_subcommand_is_honoured_too() {
    let addr = free_addr();
    let state = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_dpds-controller"))
        .env("DPDS_CONTROLLER_BIND", &addr)
        .arg("--state-dir")
        .arg(state.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let t0 = std::time::Instant::now();
    let ok = loop {
        if matches!(http(&addr, "GET", "/api/online", None), Ok((200, _))) {
            break true;
        }
        if t0.elapsed() > Duration::from_secs(10) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = child.kill();
    let _ = child.wait();
    assert!(ok, "no answer on {addr}");
}
