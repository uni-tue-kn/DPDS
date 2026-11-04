//! Spec 30b (fixes from the review of 35d7876): a histogram page problem never costs the rest of
//! the poll.
//!
//! # What the tests assume
//!
//! - Only an I/O error (timeout, closed or broken socket: anything that can desynchronise the
//!   matching of replies to requests by send order) fails a poll (`poll_once` is `Err`, the
//!   `Poller` reconnects), as before.
//! - A page that is null, empty (`{}`), lacks `bins`, has a `bins` that is not an array of u64, has
//!   the wrong `page`, `first_bin` or `num_bins`, or the wrong number of bins makes that
//!   direction's histogram absent for this poll: `delay_histogram(&snapshot, dir)` is `None` and
//!   the histogram endpoint answers 404 "no delay histogram for direction <dir>". Counters
//!   (`dirs`), NIC stats (`eth`), `ports` and the other direction's histogram of the same poll are
//!   published unchanged. The poller stays connected (no reconnect) and the cache is fresh (not
//!   `stale`).
//! - An older `dpds` whose page 0 has `bins` but no `page`/`first_bin` is just such a bad page.
//! - Page 0 with `num_bins` or `lo_ns`/`bin_ns` but no valid `bins` is "broken": no histogram for
//!   that poll, never a 200 with zero bins. (A log warning naming the direction is part of the spec
//!   too, but the controller logs with `eprintln!` to the process's stderr, which an in-process
//!   test cannot capture, so it is not pinned here.)
//! - Spec 30b item 4: the bins of a snapshot live only in `hist_bins`; a snapshot with `bins` only
//!   inside `hists[d]` has no histogram (no fallback). Test snapshots are built with the shared
//!   helpers `set_hist`/`set_hists` of `common`.
//!
//! What the tests do not pin: whether the controller still asks for the pages after a bad one, and
//! what `Snapshot::hists` holds for a bad direction (only that `delay_histogram` is `None`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::common::*;
use crate::fake_telemetry::{hist_page, short_tempdir, FakeTelemetryServer, Mode};
use crate::histogram_extras::{hist, snap};
use dpds_controller::api::{self, Deps};
use dpds_controller::stats::{delay_histogram, Snapshot};
use dpds_controller::supervisor::{State, SupervisorStatus};
use dpds_controller::telemetry::{poll_once, socket_path, Client, Poller, TelemetrySource};
use serde_json::{json, Value};

const LO: f64 = 1000.0;
const BIN: f64 = 2.5;
/// Three pages (500, 500, 200 bins).
const BINS: usize = 1200;
const LAST_PAGE: usize = 2;

/// Bin `i` of direction `d` holds `(3 + 2 d) i + 1`, so the two directions never look alike.
fn expected_bins(d: usize, n: usize) -> Vec<u64> {
    (0..n as u64).map(|i| (3 + 2 * d as u64) * i + 1).collect()
}

fn full_hist(d: usize, n: usize) -> Value {
    hist(LO, BIN, &expected_bins(d, n), 7 + d as u64, 9 + d as u64)
}

fn base_snapshot() -> Snapshot {
    snap(full_hist(0, BINS), full_hist(1, BINS))
}

/// Which `(dir, page)` a `/dpds/delay_hist` parameter (`<dir>` or `<dir>,<page>`) names.
fn dir_page(param: &str) -> (usize, usize) {
    match param.split_once(',') {
        Some((d, p)) => (d.parse().unwrap(), p.parse().unwrap()),
        None => (param.parse().unwrap(), 0),
    }
}

/// A fake `dpds` for `base`; `tamper(dir, page, honest_page)` may change any histogram page
/// (`None` = a null reply).
fn handler(
    base: Snapshot,
    tamper: impl Fn(usize, usize, Value) -> Option<Value> + Send + Sync + 'static,
) -> impl Fn(&str) -> Option<Value> + Send + Sync + 'static {
    let hists = full_hists(&base);
    move |cmd| {
        let (name, param) = cmd.split_once(',').unwrap_or((cmd, ""));
        let idx: usize = param.parse().unwrap_or(usize::MAX);
        match name {
            "/dpds/ports" => Some(base.ports.clone()),
            "/dpds/stats" => base.dirs.get(idx).cloned(),
            "/dpds/delay_hist" => {
                let honest = hist_page(&hists, param)?;
                let (d, p) = dir_page(param);
                tamper(d, p, honest)
            }
            "/ethdev/stats" => base.eth.get(idx).cloned(),
            _ => None,
        }
    }
}

fn connect(
    mode: Mode,
    h: impl Fn(&str) -> Option<Value> + Send + Sync + 'static,
) -> (FakeTelemetryServer, Client, tempfile::TempDir) {
    let dir = short_tempdir();
    let path = socket_path(dir.path());
    let server = FakeTelemetryServer::start(&path, 4242, vec![mode], h);
    let (client, _greeting) = Client::connect(&path).expect("connect to the fake socket");
    (server, client, dir)
}

// ---- the ways a page can be bad ------------------------------------------------------------------

type Edit = fn(usize, Value) -> Option<Value>;
type Page0Edit = fn(Value) -> Option<Value>;

fn with(mut v: Value, f: impl FnOnce(&mut Value)) -> Option<Value> {
    f(&mut v);
    Some(v)
}

fn without(v: Value, key: &str) -> Option<Value> {
    with(v, |v| {
        v.as_object_mut().unwrap().remove(key);
    })
}

/// The edits that make a page bad wherever it is in the histogram (`page` is its index).
fn bad_pages() -> Vec<(&'static str, Edit)> {
    vec![
        ("null reply", |_, _| None),
        ("empty reply", |_, _| Some(json!({}))),
        ("no bins", |_, v| without(v, "bins")),
        ("bins null", |_, v| with(v, |v| v["bins"] = Value::Null)),
        ("bins a string", |_, v| with(v, |v| v["bins"] = json!("x"))),
        ("bins an object", |_, v| {
            with(v, |v| v["bins"] = json!({"0": 1}))
        }),
        ("one bin short", |_, v| {
            with(v, |v| {
                v["bins"].as_array_mut().unwrap().pop();
            })
        }),
        ("one bin too many", |_, v| {
            with(v, |v| v["bins"].as_array_mut().unwrap().push(json!(1)))
        }),
        ("no bins at all but num_bins", |_, v| {
            with(v, |v| v["bins"] = json!([]))
        }),
        ("negative bin", |_, v| with(v, |v| v["bins"][3] = json!(-1))),
        ("fractional bin", |_, v| {
            with(v, |v| v["bins"][3] = json!(1.5))
        }),
        ("string bin", |_, v| with(v, |v| v["bins"][3] = json!("7"))),
        ("null bin", |_, v| with(v, |v| v["bins"][3] = Value::Null)),
        ("page of the next index", |p, v| {
            with(v, |v| v["page"] = json!(p + 1))
        }),
        ("page as a string", |p, v| {
            with(v, |v| v["page"] = json!(p.to_string()))
        }),
        ("no page", |_, v| without(v, "page")),
        ("no first_bin", |_, v| without(v, "first_bin")),
        ("neither page nor first_bin (old dpds)", |_, v| {
            let v = without(v, "page")?;
            without(v, "first_bin")
        }),
        ("first_bin off by one", |p, v| {
            with(v, |v| v["first_bin"] = json!(p * 500 + 1))
        }),
        ("first_bin of another page", |p, v| {
            with(v, |v| v["first_bin"] = json!((p + 1) * 500))
        }),
        ("num_bins one more", |_, v| {
            with(v, |v| v["num_bins"] = json!(BINS + 1))
        }),
        ("num_bins a string", |_, v| {
            with(v, |v| v["num_bins"] = json!(BINS.to_string()))
        }),
        ("no num_bins", |_, v| without(v, "num_bins")),
    ]
}

/// A poll of `base` whose histogram page `page` of `bad` goes through `edit`.
fn poll_with_bad_page(
    base: &Snapshot,
    bad: usize,
    page: usize,
    edit: Edit,
) -> std::io::Result<Snapshot> {
    let h = handler(base.clone(), move |d, p, honest| {
        if (d, p) == (bad, page) {
            edit(p, honest)
        } else {
            Some(honest)
        }
    });
    let (_server, mut client, _dir) = connect(Mode::Serve, h);
    poll_once(&mut client)
}

/// What a poll with one bad histogram of direction `bad` must still be: Ok, everything else of the
/// base snapshot, the other direction's complete histogram, and no histogram for `bad`.
fn check_poll(base: &Snapshot, bad: usize, got: std::io::Result<Snapshot>) -> Result<(), String> {
    let got = got.map_err(|e| format!("the poll failed: {e}"))?;
    if got.dirs != base.dirs || got.eth != base.eth || got.ports != base.ports {
        return Err("the counters, NIC stats or ports are not those of the server".into());
    }
    if let Some(h) = delay_histogram(&got, bad) {
        return Err(format!(
            "direction {bad} still has a histogram of {} bins",
            h.bins.len()
        ));
    }
    let good = 1 - bad;
    match delay_histogram(&got, good) {
        None => Err(format!("direction {good} lost its histogram too")),
        Some(h) if h.bins != expected_bins(good, BINS) => {
            Err(format!("direction {good}'s bins are wrong"))
        }
        Some(h) if (h.underflow, h.overflow) != (7 + good as u64, 9 + good as u64) => {
            Err(format!("direction {good}'s under/overflow are wrong"))
        }
        Some(_) => Ok(()),
    }
}

fn assert_every_bad_page_is_survived(page: usize) {
    let base = base_snapshot();
    let mut failures = Vec::new();
    for bad in [0, 1] {
        for (what, edit) in bad_pages() {
            let got = poll_with_bad_page(&base, bad, page, edit);
            if let Err(why) = check_poll(&base, bad, got) {
                failures.push(format!("direction {bad}, page {page}, {what}: {why}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} case(s) fail:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn a_bad_page_0_leaves_the_rest_of_the_poll_alone() {
    assert_every_bad_page_is_survived(0);
}

#[test]
fn a_bad_middle_page_leaves_the_rest_of_the_poll_alone() {
    assert_every_bad_page_is_survived(1);
}

#[test]
fn a_bad_last_page_leaves_the_rest_of_the_poll_alone() {
    assert_every_bad_page_is_survived(LAST_PAGE);
}

#[test]
fn a_replayed_page_leaves_the_rest_of_the_poll_alone() {
    // page p > 0 answers with page 0 again, page 0 with page 1 (replies matched to the wrong request)
    let base = base_snapshot();
    let hists = full_hists(&base);
    let mut failures = Vec::new();
    for bad in [0, 1] {
        for page in 0..=LAST_PAGE {
            let other = if page == 0 { 1 } else { 0 };
            let replay = hist_page(&hists, &format!("{bad},{other}"));
            let h = handler(base.clone(), move |d, p, honest| {
                if (d, p) == (bad, page) {
                    replay.clone()
                } else {
                    Some(honest)
                }
            });
            let (_server, mut client, _dir) = connect(Mode::Serve, h);
            if let Err(why) = check_poll(&base, bad, poll_once(&mut client)) {
                failures.push(format!("direction {bad}, page {page} replayed: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn both_directions_bad_still_publish_counters_and_nic_stats() {
    let base = base_snapshot();
    let h = handler(base.clone(), |d, p, honest| match (d, p) {
        (0, 1) => Some(json!({})),
        (1, 0) => without(honest, "bins"),
        _ => Some(honest),
    });
    let (_server, mut client, _dir) = connect(Mode::Serve, h);
    let got = poll_once(&mut client).expect("bad pages do not fail the poll");
    assert_eq!(got.dirs, base.dirs);
    assert_eq!(got.eth, base.eth);
    assert_eq!(got.ports, base.ports);
    assert!(delay_histogram(&got, 0).is_none());
    assert!(delay_histogram(&got, 1).is_none());
}

#[test]
fn a_one_page_histogram_from_an_old_dpds_has_no_histogram() {
    // page 0 has `bins` but neither `page` nor `first_bin`; the histogram is a single page
    let base = snap(full_hist(0, 100), full_hist(1, 100));
    for bad in [0, 1] {
        let h = handler(base.clone(), move |d, _, honest| {
            if d == bad {
                let v = without(honest, "page")?;
                without(v, "first_bin")
            } else {
                Some(honest)
            }
        });
        let (_server, mut client, _dir) = connect(Mode::Serve, h);
        let got = poll_once(&mut client).expect("an old-style page 0 does not fail the poll");
        assert_eq!(got.dirs, base.dirs);
        assert!(
            delay_histogram(&got, bad).is_none(),
            "direction {bad}: no compatibility parsing"
        );
        let good = delay_histogram(&got, 1 - bad).expect("the other direction is fine");
        assert_eq!(good.bins, expected_bins(1 - bad, 100));
    }
}

// ---- absent (silent) versus broken page 0 --------------------------------------------------------

#[test]
fn a_page_0_that_is_empty_or_null_is_no_histogram_and_not_a_failure() {
    // delay stats off, or a dpds without the feature
    let base = base_snapshot();
    for reply in [Some(json!({})), None] {
        let h = handler(base.clone(), move |d, _, honest| {
            if d == 0 {
                reply.clone()
            } else {
                Some(honest)
            }
        });
        let (server, mut client, _dir) = connect(Mode::Serve, h);
        let got = poll_once(&mut client).expect("an absent histogram is not a failure");
        assert_eq!(got.dirs, base.dirs);
        assert!(delay_histogram(&got, 0).is_none());
        assert_eq!(
            delay_histogram(&got, 1).unwrap().bins,
            expected_bins(1, BINS)
        );
        let asked: Vec<String> = server
            .queries()
            .into_iter()
            .filter(|q| q.starts_with("/dpds/delay_hist,0"))
            .collect();
        assert_eq!(asked.len(), 1, "no page after an absent page 0: {asked:?}");
    }
}

#[test]
fn a_page_0_with_metadata_but_no_valid_bins_never_gives_a_histogram() {
    // item 2: "broken", not "absent": never a histogram with zero bins
    let metadata_only: [(&str, Page0Edit); 7] = [
        ("num_bins, no bins", |v| without(v, "bins")),
        ("num_bins, bins null", |v| {
            with(v, |v| v["bins"] = Value::Null)
        }),
        ("num_bins, bins a string", |v| {
            with(v, |v| v["bins"] = json!("x"))
        }),
        ("num_bins and empty bins", |v| {
            with(v, |v| v["bins"] = json!([]))
        }),
        ("only num_bins", |_| Some(json!({"num_bins": BINS}))),
        ("only lo_ns and bin_ns", |_| {
            Some(json!({"lo_ns": "1000", "bin_ns": "2.5"}))
        }),
        ("everything but bins and page", |v| {
            let v = without(v, "bins")?;
            let v = without(v, "page")?;
            without(v, "first_bin")
        }),
    ];
    let base = base_snapshot();
    let mut failures = Vec::new();
    for (what, edit) in metadata_only {
        let h = handler(base.clone(), move |d, p, honest| {
            if (d, p) == (0, 0) {
                edit(honest)
            } else {
                Some(honest)
            }
        });
        let (_server, mut client, _dir) = connect(Mode::Serve, h);
        if let Err(why) = check_poll(&base, 0, poll_once(&mut client)) {
            failures.push(format!("{what}: {why}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_page_0_with_too_many_bins_is_no_histogram_and_not_a_failure() {
    let base = base_snapshot();
    let h = handler(base.clone(), |d, p, honest| {
        if (d, p) == (0, 0) {
            with(honest, |v| v["num_bins"] = json!(5000))
        } else {
            Some(honest)
        }
    });
    let (_server, mut client, _dir) = connect(Mode::Serve, h);
    let got = poll_once(&mut client).expect("num_bins above the cap does not fail the poll");
    assert_eq!(got.dirs, base.dirs);
    assert!(delay_histogram(&got, 0).is_none());
    assert!(delay_histogram(&got, 1).is_some());
}

// ---- I/O errors still fail the poll --------------------------------------------------------------

#[test]
fn a_connection_that_breaks_in_the_middle_of_a_histogram_still_fails_the_poll() {
    let (_server, mut client, _dir) = connect(
        Mode::CloseOnLaterPage,
        handler(base_snapshot(), |_, _, honest| Some(honest)),
    );
    assert!(
        poll_once(&mut client).is_err(),
        "a transport error cannot be told from a bad page: the poll fails"
    );
}

// ---- one representation: bins only in hist_bins --------------------------------------------------

#[test]
fn a_polled_snapshot_keeps_page_0_without_bins_and_the_bins_in_hist_bins() {
    let base = base_snapshot();
    let (_server, mut client, _dir) = connect(
        Mode::Serve,
        handler(base.clone(), |_, _, honest| Some(honest)),
    );
    let got = poll_once(&mut client).unwrap();
    for d in 0..2 {
        assert!(
            got.hists[d].get("bins").is_none(),
            "hists[{d}] is page 0 without its bins: {}",
            got.hists[d]
        );
        assert_eq!(got.hists[d]["num_bins"], json!(BINS));
        assert_eq!(got.hist_bins[d], expected_bins(d, BINS));
    }
}

#[test]
fn the_snapshot_helpers_split_and_join_a_json_histogram() {
    let mut s = Snapshot::default();
    set_hist(&mut s, 1, full_hist(1, 10));
    set_hist(&mut s, 0, full_hist(0, 4));
    assert!(s.hists[0].get("bins").is_none() && s.hists[1].get("bins").is_none());
    assert_eq!(s.hist_bins[0], expected_bins(0, 4));
    assert_eq!(s.hist_bins[1], expected_bins(1, 10));
    assert_eq!(s.hists[1]["num_bins"], json!(10));
    assert_eq!(full_hists(&s), vec![full_hist(0, 4), full_hist(1, 10)]);
    set_hists(&mut s, vec![full_hist(1, 3)]);
    assert_eq!((s.hists.len(), s.hist_bins.len()), (1, 1));
    assert_eq!(s.hist_bins[0], expected_bins(1, 3));
}

#[test]
fn bins_only_in_hists_are_no_histogram() {
    // the fallback to `hists[d].bins` is gone: bins live in `hist_bins` and nowhere else
    let s = Snapshot {
        hists: vec![hist_json(0.0, 10.0, 16)],
        ..Snapshot::default()
    };
    assert!(s.hists[0]["bins"].is_array(), "the premise: bins in hists");
    assert!(s.hist_bins.is_empty());
    assert!(
        delay_histogram(&s, 0).is_none(),
        "a histogram was built from hists[0].bins"
    );
}

#[tokio::test]
async fn bins_only_in_hists_are_a_404_on_the_endpoint() {
    let a = TestApp::new(FakeHost::bare_metal());
    let mut s = base_snapshot();
    s.hist_bins.clear();
    s.hists = vec![hist_json(0.0, 10.0, 16), hist_json(0.0, 10.0, 16)];
    a.telemetry.set(None, s);
    for d in [0, 1] {
        let r = a.get(&format!("/api/stats/{d}/delay_histogram")).await;
        assert_eq!(r.status, 404, "direction {d}: {}", r.text());
    }
}

// ---- the whole way: Poller -> REST ---------------------------------------------------------------

struct Rig {
    router: axum::Router,
    poller: Arc<Poller>,
    server: FakeTelemetryServer,
    _dir: tempfile::TempDir,
}

fn rig(mode: Mode, h: impl Fn(&str) -> Option<Value> + Send + Sync + 'static) -> Rig {
    let rt = short_tempdir();
    let supervisor = FakeSupervisor::new();
    supervisor.force_status(SupervisorStatus {
        pid: Some(4242),
        started_at: Some(1_700_000_000),
        ..status(State::Running)
    });
    let runtime_dir: PathBuf = rt.path().to_path_buf();
    let server = FakeTelemetryServer::start(&socket_path(&runtime_dir), 4242, vec![mode], h);
    let poller = Poller::spawn(runtime_dir.clone(), supervisor.clone());
    poller.set_poll_interval(50);
    let router = api::router(Deps {
        roots: FakeHost::bare_metal().roots(),
        supervisor,
        telemetry: poller.clone(),
        dpds_bin: PathBuf::from("/usr/local/bin/dpds"),
        runtime_dir,
        dpds_has_telemetry: true,
        is_root: false,
    });
    Rig {
        router,
        poller,
        server,
        _dir: rt,
    }
}

impl Rig {
    fn polls(&self) -> usize {
        self.server
            .queries()
            .iter()
            .filter(|q| *q == "/dpds/ports")
            .count()
    }

    async fn until(&self, what: &str, mut ok: impl AsyncFnMut(&Rig) -> bool) {
        let t0 = Instant::now();
        while !ok(self).await {
            assert!(t0.elapsed() < Duration::from_secs(8), "timeout: {what}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn status_of_histogram(&self, dir: usize) -> Reply {
        router_get(&self.router, &format!("/api/stats/{dir}/delay_histogram")).await
    }
}

/// A run that is healthy at first; then (the flag) direction 0's page `page` goes through `edit`.
/// The poller must publish the new poll with direction 0's histogram gone and everything else
/// fresh, without reconnecting.
async fn a_bad_page_in_a_running_poller(page: usize, edit: Edit) {
    let base = base_snapshot();
    let bad = Arc::new(AtomicBool::new(false));
    let flag = bad.clone();
    let r = rig(
        Mode::Serve,
        handler(base.clone(), move |d, p, honest| {
            if flag.load(Ordering::SeqCst) && (d, p) == (0, page) {
                edit(p, honest)
            } else {
                Some(honest)
            }
        }),
    );
    r.until("a first healthy poll", async |r| {
        r.status_of_histogram(0).await.status == 200
    })
    .await;
    assert_eq!(r.server.connections(), 1);

    bad.store(true, Ordering::SeqCst);
    r.until("the bad page costs direction 0 its histogram", async |r| {
        r.status_of_histogram(0).await.status == 404
    })
    .await;
    let after = r.polls();
    r.until("two more polls", async |r| r.polls() >= after + 2)
        .await;

    // direction 0: answered like a missing histogram
    let missing = r.status_of_histogram(0).await;
    assert_eq!(missing.status, 404, "{}", missing.text());
    assert!(
        missing
            .message()
            .contains("no delay histogram for direction 0"),
        "{}",
        missing.message()
    );
    // direction 1: complete
    let ok = r.status_of_histogram(1).await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    let bins: Vec<u64> = serde_json::from_value(ok.json()["bins"].clone()).unwrap();
    assert_eq!(bins, expected_bins(1, BINS));
    // counters and NIC stats of the poll are published
    let stats = router_get(&r.router, "/api/stats").await;
    assert_eq!(stats.status, 200, "{}", stats.text());
    let v = stats.json();
    assert_eq!(v["stale"], json!(false), "{v}");
    assert_eq!(v["directions"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(
        v["directions"][0]["counters"]["rx_pkts"], base.dirs[0]["rx_pkts"],
        "{v}"
    );
    assert_eq!(v["ports"].as_array().unwrap().len(), 2, "{v}");
    // no failure counted, no reconnect
    let latest = r.poller.latest().expect("a published poll");
    assert!(!latest.stale, "a bad page counted as a failed poll");
    assert!(latest.age_ms < 2000, "{}", latest.age_ms);
    assert_eq!(r.server.connections(), 1, "the poller reconnected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_last_page_keeps_the_poller_connected_and_the_rest_published() {
    a_bad_page_in_a_running_poller(LAST_PAGE, |_, _| Some(json!({}))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_0_without_bins_is_a_404_not_a_200_with_zero_bins() {
    a_bad_page_in_a_running_poller(0, |_, v| without(v, "bins")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_style_page_0_keeps_the_poller_connected_and_the_rest_published() {
    a_bad_page_in_a_running_poller(0, |_, v| {
        let v = without(v, "page")?;
        without(v, "first_bin")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_null_middle_page_keeps_the_poller_connected_and_the_rest_published() {
    a_bad_page_in_a_running_poller(1, |_, _| None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_poller_whose_connection_breaks_in_a_histogram_reconnects_and_publishes_nothing() {
    // the contrast: an I/O error is a failed poll, as before
    let r = rig(
        Mode::CloseOnLaterPage,
        handler(base_snapshot(), |_, _, honest| Some(honest)),
    );
    r.until("a reconnect after the broken connection", async |r| {
        r.server.connections() >= 3
    })
    .await;
    assert!(
        r.poller.latest().is_none(),
        "a poll that broke in the middle must publish nothing"
    );
}
