//! Spec 30: the delay histogram is read from `dpds` in pages. `/dpds/delay_hist,<dir>,<page>`
//! holds at most 500 bins (and `page`, `first_bin`, `lo_ns`, `bin_ns`, `num_bins`, `underflow`,
//! `overflow`); a histogram of up to 4096 bins is `ceil(num_bins / 500)` pages.
//!
//! The controller (`telemetry::poll_once`) reads page 0, then the remaining pages, in order,
//! and concatenates the bins; the REST endpoint returns all of them.
//!
//! # What the tests assume
//!
//! - The request for page 0 is `/dpds/delay_hist,<dir>` or `/dpds/delay_hist,<dir>,0` (both are
//!   page 0 on the `dpds` side, the tests accept either), page `p > 0` is
//!   `/dpds/delay_hist,<dir>,<p>`. Every page is requested once, in order, per poll, and no page
//!   beyond the last one (the page count follows from `num_bins` of page 0). A one-page histogram
//!   costs one request.
//! - Replies carry no parameter in their key, so the controller matches them by send order; the
//!   page position fields (`page`, `first_bin`) are what lets it detect a mixed-up or cut page.
//!   A poll that gets a wrong, missing or short page must never publish a histogram that is
//!   stitched together from the wrong pieces: it either fails (`Err`) or leaves the direction
//!   without a histogram (`delay_histogram` is `None`). Which of the two is the implementer's
//!   choice (the old code gave `Null` for a histogram that could not be read).
//! - The snapshot's internal storage of the bins (spec: `Vec<u64>` rather than
//!   `serde_json::Value`) is deliberately not pinned: it is invisible through behaviour.
//!   Tests here read a polled snapshot only through `delay_histogram(&snapshot, dir)`.
//!
//! The fake socket is `fake_telemetry.rs`: `serving` answers pages from the full histograms of
//! a snapshot, [`hist_page`] is the slicing.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::common::*;
use crate::fake_telemetry::{hist_page, short_tempdir, FakeTelemetryServer, Mode, PAGE_BINS};
use crate::histogram_extras::{hist, snap};
use dpds_controller::api::{self, Deps};
use dpds_controller::stats::{delay_histogram, Snapshot};
use dpds_controller::supervisor::{State, SupervisorStatus};
use dpds_controller::telemetry::{poll_once, socket_path, Client, Poller, TelemetrySource};
use serde_json::{json, Value};

const LO: f64 = 1000.0;
const BIN: f64 = 2.5;
const UNDER: u64 = 7;
const OVER: u64 = 9;

/// A full histogram of `n` bins, every bin count different: bin `i` holds `3 i + 1`.
fn full_hist(n: usize) -> Value {
    let bins: Vec<u64> = (0..n as u64).map(|i| 3 * i + 1).collect();
    hist(LO, BIN, &bins, UNDER, OVER)
}

fn expected_bins(n: usize) -> Vec<u64> {
    (0..n as u64).map(|i| 3 * i + 1).collect()
}

/// A snapshot whose direction 0 has `n0` bins and direction 1 has `n1`.
fn snapshot_with(n0: usize, n1: usize) -> Snapshot {
    snap(full_hist(n0), full_hist(n1))
}

fn query_handler(
    snapshot: Snapshot,
    tamper: impl Fn(&str, Option<Value>) -> Option<Value> + Send + Sync + 'static,
) -> impl Fn(&str) -> Option<Value> + Send + Sync + 'static {
    move |cmd| {
        let (name, param) = cmd.split_once(',').unwrap_or((cmd, ""));
        let idx: usize = param.parse().unwrap_or(usize::MAX);
        let honest = match name {
            "/dpds/ports" => Some(snapshot.ports.clone()),
            "/dpds/stats" => snapshot.dirs.get(idx).cloned(),
            "/dpds/delay_hist" => hist_page(&snapshot.hists, param),
            "/ethdev/stats" => snapshot.eth.get(idx).cloned(),
            _ => None,
        };
        if name == "/dpds/delay_hist" {
            tamper(param, honest)
        } else {
            honest
        }
    }
}

/// A server on a fresh socket, and a client connected to it.
fn connect(
    handler: impl Fn(&str) -> Option<Value> + Send + Sync + 'static,
) -> (FakeTelemetryServer, Client, tempfile::TempDir) {
    let dir = short_tempdir();
    let path = socket_path(dir.path());
    let server = FakeTelemetryServer::start(&path, 4242, vec![Mode::Serve], handler);
    let (client, _greeting) = Client::connect(&path).expect("connect to the fake socket");
    (server, client, dir)
}

/// The `/dpds/delay_hist` requests of `server` with page 0 in its short form.
fn hist_queries(server: &FakeTelemetryServer) -> Vec<String> {
    server
        .queries()
        .into_iter()
        .filter(|q| q.starts_with("/dpds/delay_hist"))
        .map(|q| {
            let parts: Vec<&str> = q.split(',').collect();
            if parts.len() == 3 && parts[2] == "0" {
                format!("{},{}", parts[0], parts[1])
            } else {
                q
            }
        })
        .collect()
}

#[test]
fn the_fake_pages_like_the_real_dpds() {
    // the premise of the other tests: the slicing helper itself
    let hists = vec![full_hist(1200), full_hist(100)];
    let p0 = hist_page(&hists, "0").unwrap();
    assert_eq!(p0["bins"].as_array().unwrap().len(), PAGE_BINS);
    assert_eq!(
        (p0["page"].clone(), p0["first_bin"].clone()),
        (json!(0), json!(0))
    );
    assert_eq!(p0["num_bins"], json!(1200));
    let p2 = hist_page(&hists, "0,2").unwrap();
    assert_eq!(p2["bins"].as_array().unwrap().len(), 200);
    assert_eq!(p2["first_bin"], json!(1000));
    assert_eq!(p2["bins"][0], json!(3 * 1000 + 1));
    assert!(hist_page(&hists, "0,3").is_none());
    assert!(hist_page(&hists, "1,1").is_none());
    assert!(hist_page(&hists, "2").is_none());
}

#[test]
fn poll_once_reads_every_page_in_order_and_concatenates_the_bins() {
    let (server, mut client, _dir) = connect(query_handler(snapshot_with(4096, 100), |_, r| r));
    let snap = poll_once(&mut client).expect("a poll of paged histograms");

    let h = delay_histogram(&snap, 0).expect("direction 0 has a histogram");
    assert_eq!(h.bins.len(), 4096);
    assert_eq!(
        h.bins,
        expected_bins(4096),
        "the pages are stitched in order"
    );
    assert_eq!((h.lo_ns, h.bin_ns), (LO, BIN));
    assert_eq!((h.underflow, h.overflow), (UNDER, OVER));
    assert_eq!(h.edges_ns.len(), 4097);
    assert_eq!(h.edges_ns[4096], LO + 4096.0 * BIN);

    let h1 = delay_histogram(&snap, 1).expect("direction 1 has a histogram");
    assert_eq!(h1.bins, expected_bins(100));
    assert_eq!((h1.underflow, h1.overflow), (UNDER, OVER));

    // page 0, then 1 ..= 8 (4096 bins = 8 pages of 500 and one of 96), once each, in order; a
    // 100-bin histogram is one request
    let mut want: Vec<String> = vec!["/dpds/delay_hist,0".into()];
    want.extend((1..=8).map(|p| format!("/dpds/delay_hist,0,{p}")));
    want.push("/dpds/delay_hist,1".into());
    assert_eq!(hist_queries(&server), want);
}

#[test]
fn the_page_count_follows_num_bins_around_the_page_boundaries() {
    for (bins, pages) in [
        (1, 1),
        (256, 1),
        (500, 1),
        (501, 2),
        (1000, 2),
        (1001, 3),
        (4095, 9),
    ] {
        let (server, mut client, _dir) = connect(query_handler(snapshot_with(bins, 100), |_, r| r));
        let snap = poll_once(&mut client).unwrap_or_else(|e| panic!("{bins} bins: {e}"));
        let h = delay_histogram(&snap, 0).unwrap_or_else(|| panic!("{bins} bins: no histogram"));
        assert_eq!(h.bins, expected_bins(bins), "{bins} bins");
        let dir0: Vec<String> = hist_queries(&server)
            .into_iter()
            .filter(|q| q.starts_with("/dpds/delay_hist,0"))
            .collect();
        assert_eq!(
            dir0.len(),
            pages,
            "{bins} bins need {pages} page(s): {dir0:?}"
        );
    }
}

/// A poll against a server that misbehaves must not publish a stitched-together histogram.
fn assert_no_corrupt_histogram(what: &str, result: std::io::Result<Snapshot>) {
    match result {
        Err(_) => {}
        Ok(snap) => assert!(
            delay_histogram(&snap, 0).is_none(),
            "{what}: a histogram was published from wrong pages: {:?}",
            delay_histogram(&snap, 0).map(|h| h.bins.len())
        ),
    }
}

#[test]
fn a_page_that_is_replayed_is_not_stitched_in() {
    // page p > 0 answers with page 0 again (a reply matched to the wrong request)
    let honest_page0 = hist_page(&snapshot_with(2000, 100).hists, "0");
    let (_s, mut client, _d) = connect(query_handler(snapshot_with(2000, 100), move |param, r| {
        if param.starts_with("0,") && !param.ends_with(",0") {
            honest_page0.clone()
        } else {
            r
        }
    }));
    assert_no_corrupt_histogram("replayed page 0", poll_once(&mut client));
}

#[test]
fn a_missing_page_is_not_replaced_by_a_shorter_histogram() {
    // page 3 gets a null reply (an error on the dpds side)
    let (_s, mut client, _d) = connect(query_handler(snapshot_with(4096, 100), |param, r| {
        if param == "0,3" {
            None
        } else {
            r
        }
    }));
    assert_no_corrupt_histogram("null page 3", poll_once(&mut client));
}

#[test]
fn a_page_without_bins_is_not_replaced_by_a_shorter_histogram() {
    // page 2 is a dict with no `bins` (what `dpds` answers to a bad parameter)
    let (_s, mut client, _d) = connect(query_handler(snapshot_with(1500, 100), |param, r| {
        if param == "0,2" {
            Some(json!({}))
        } else {
            r
        }
    }));
    assert_no_corrupt_histogram("empty page 2", poll_once(&mut client));
}

#[test]
fn a_short_page_is_detected() {
    // page 5 carries 499 bins although it says it starts at bin 2500 of 4096: the reply was cut
    // (what an ignored -ENOSPC or a reply over the size limit looks like), so the total would
    // no longer be num_bins
    let (_s, mut client, _d) = connect(query_handler(snapshot_with(4096, 100), |param, r| {
        if param == "0,5" {
            r.map(|mut v| {
                v["bins"].as_array_mut().unwrap().pop();
                v
            })
        } else {
            r
        }
    }));
    assert_no_corrupt_histogram("short page 5", poll_once(&mut client));
}

#[test]
fn a_histogram_with_no_bins_is_one_empty_page() {
    // a direction with `delay_stats` off: page 0 is empty and there is no page 1
    let snapshot = snap(full_hist(300), crate::common::empty_hist_json());
    let (server, mut client, _dir) = connect(query_handler(snapshot, |_, r| r));
    let snap = poll_once(&mut client).expect("an empty histogram is not a failure");
    assert_eq!(delay_histogram(&snap, 0).unwrap().bins, expected_bins(300));
    let off: Vec<String> = hist_queries(&server)
        .into_iter()
        .filter(|q| q.starts_with("/dpds/delay_hist,1"))
        .collect();
    assert_eq!(off.len(), 1, "no page after the empty one: {off:?}");
}

// ---- the whole way: Poller -> ring -> REST -------------------------------------------------------

struct Rig {
    router: axum::Router,
    poller: Arc<Poller>,
    _server: FakeTelemetryServer,
    _dir: tempfile::TempDir,
}

fn rig(snapshot: Snapshot) -> Rig {
    let rt = short_tempdir();
    let supervisor = FakeSupervisor::new();
    supervisor.force_status(SupervisorStatus {
        pid: Some(4242),
        started_at: Some(1_700_000_000),
        ..status(State::Running)
    });
    let runtime_dir: PathBuf = rt.path().to_path_buf();
    let server = FakeTelemetryServer::serving(&socket_path(&runtime_dir), 4242, snapshot);
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
        _server: server,
        _dir: rt,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_histogram_endpoint_returns_all_4096_bins_of_a_polled_run() {
    let r = rig(snapshot_with(4096, 100));
    let t0 = Instant::now();
    let v = loop {
        let reply = router_get(&r.router, "/api/stats/0/delay_histogram").await;
        if reply.status == 200 {
            break reply.json();
        }
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "no histogram after 10 s: {} {}",
            reply.status,
            reply.text()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let bins: Vec<u64> = serde_json::from_value(v["bins"].clone()).unwrap();
    assert_eq!(bins, expected_bins(4096));
    assert_eq!(v["edges_ns"].as_array().unwrap().len(), 4097);
    assert_eq!(
        (v["underflow"].clone(), v["overflow"].clone()),
        (json!(UNDER), json!(OVER))
    );
    let total: u64 = bins.iter().sum::<u64>() + UNDER + OVER;
    assert_eq!(v["count"], json!(total), "{}", v["count"]);

    // the ring keeps whole histograms too
    let history = r.poller.history();
    let (_, newest) = history.last().expect("a poll in the ring");
    assert_eq!(
        delay_histogram(newest, 0).unwrap().bins,
        expected_bins(4096)
    );
    assert_eq!(delay_histogram(newest, 1).unwrap().bins, expected_bins(100));

    // the other direction is a single page
    let reply = router_get(&r.router, "/api/stats/1/delay_histogram").await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.json()["bins"].as_array().unwrap().len(), 100);
}
