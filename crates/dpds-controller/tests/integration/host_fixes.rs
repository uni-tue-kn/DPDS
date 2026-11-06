//! Host and API part: hugepage need maths, `nohz_full` `(null)`, offline isolated
//! cores, blocking file reads in handlers, concurrent starts and `--bind` without the
//! `serve` subcommand.

use std::ffi::CString;
use std::sync::mpsc;
use std::time::Duration;

use crate::common::*;
use serde_json::json;

// ---- hugepage need saturates -------------------------------------------------------

#[tokio::test]
async fn an_absurd_buffer_capacity_makes_the_hugepage_need_saturate_not_wrap() {
    // u64::MAX buffers: (capacity + pool slack) * 1920 B overflows u64. A wrapped
    // (small) need would turn this "ok"; a saturated one cannot fit. The hints of a body are
    // advice, not a start: either they answer 200 with an error-level hugepage hint, or the
    // body's capacity is refused (400 naming `buffer_capacity`, the cap is 2^31); a wrapped
    // "ok" is the one outcome that is wrong.
    let a = TestApp::new(FakeHost::bare_metal()); // 2 x 2048 x 2 MiB free
    let mut cfg = cfg_json(1.0e6, 0.0);
    cfg["buffer_capacity"] = json!(u64::MAX);
    let links = vec![
        link_cfg("a", "net_null0", "net_null0", cfg.clone()),
        link_cfg("b", "net_null1", "net_null1", cfg),
    ];
    let r = a.post("/api/host/hints", setup(links).body()).await;
    match r.status {
        200 => {
            let hints = r.json();
            assert_eq!(
                hint(&hints, "hugepages")["level"],
                "error",
                "{}",
                hint(&hints, "hugepages")
            );
        }
        400 => assert!(r.message().contains("buffer_capacity"), "{}", r.text()),
        other => panic!("unexpected status {other}: {}", r.text()),
    }
    // the largest valid capacity (2^31) on two links needs far more than the host has
    let mut cfg = cfg_json(1.0e6, 0.0);
    cfg["buffer_capacity"] = json!(2_147_483_648u64);
    let links = vec![
        link_cfg("a", "net_null0", "net_null0", cfg.clone()),
        link_cfg("b", "net_null1", "net_null1", cfg),
    ];
    let hints = hints_json(&a.host, &setup(links));
    assert_eq!(hint(&hints, "hugepages")["level"], "error");
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
    let a = plan_for(
        &h,
        &setup(vec![
            link("a", "net_null0", "net_null0"),
            link("b", "net_null1", "net_null1"),
        ]),
    )
    .unwrap();
    let mut lcores: Vec<u32> = a.directions.iter().map(|d| d.lcore).collect();
    lcores.sort_unstable();
    assert_eq!(lcores, [2, 3]);
    // a third direction has no online isolated core left
    let e = plan_for(
        &h,
        &setup(vec![
            link("a", "net_null0", "net_null0"),
            link("b", "net_null1", "net_null1"),
            link("c", "net_null2", "net_null2"),
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
    let a = plan_for(
        &h,
        &setup(vec![
            link("a", "net_null0", "net_null0"),
            link("b", "net_null1", "net_null1"),
        ]),
    )
    .unwrap();
    let mut lcores: Vec<u32> = a.directions.iter().map(|d| d.lcore).collect();
    lcores.sort_unstable();
    assert_eq!(lcores, [1, 3]);
}

// ---- blocking reads do not block the runtime -----------------------------------------------

/// Keeps a FIFO's readers moving until `stop` is set or `deadline` has passed: opens it
/// `O_RDWR | O_NONBLOCK` (never blocks on Linux, readers or not), writes a line, closes it again,
/// and repeats. A reader that is already blocked in `open(O_RDONLY)` is woken by the writer; one
/// that is not there yet gets the next round; the close gives `read_to_string` its EOF. Nothing
/// in here can wait for a peer, so the release itself cannot hang.
fn release_fifo(
    path: std::path::PathBuf,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    deadline: Duration,
) -> std::thread::JoinHandle<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::Ordering;

    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        while !stop.load(Ordering::Relaxed) && started.elapsed() < deadline {
            if let Ok(mut w) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
            {
                let _ = w.write_all(b"0x020000\n");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })
}

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
    let got = rx.recv_timeout(Duration::from_secs(10));

    // Release the readers, whatever `got` is. The releaser only ever opens the FIFO
    // O_RDWR | O_NONBLOCK, so it cannot block; it runs until every handler has returned (or
    // 60 s, so a bug cannot keep it alive for ever). Releasing later than the check above
    // matters: earlier, the readers would not be blocked while /api/online is asked.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let releaser = release_fifo(fifo, stop.clone(), Duration::from_secs(60));
    let mut unfinished = Vec::new();
    for (i, b) in blockers.into_iter().enumerate() {
        // Bounded wait: a handler that never returns fails the test instead of hanging it.
        if tokio::time::timeout(Duration::from_secs(30), b)
            .await
            .is_err()
        {
            unfinished.push(i);
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    releaser.join().unwrap();
    assert_eq!(
        got,
        Ok(200),
        "/api/online must answer while two handlers wait for a file"
    );
    assert!(
        unfinished.is_empty(),
        "handlers {unfinished:?} did not return within 30 s after their file was released"
    );
}

// ---- concurrent starts -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_starts_launch_exactly_once() {
    let a = TestApp::new(FakeHost::bare_metal());
    let body = setup(vec![link("l", "net_null0", "net_null0")]).body();
    // the supervisor needs 300 ms to take the plan: the window in which a second
    // start could pass the state check
    a.supervisor.set_start_delay(Duration::from_millis(300));
    let tasks: Vec<_> = (0..2)
        .map(|_| {
            let router = a.router.clone();
            let body = body.clone();
            tokio::spawn(async move {
                call(&router, "POST", "/api/emulation/start", Some(body))
                    .await
                    .status
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
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_dpds-controller"))
        .env("DPDS_CONTROLLER_BIND", &addr)
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
