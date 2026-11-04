//! Spec 30: the delay histogram is read in pages (`/dpds/delay_hist,<dir>,<page>`), so a link can
//! have up to `MAX_BINS` = 4096 bins although one DPDK telemetry array holds at most 512 entries.
//!
//! # What is pinned (the spec's text first, the rest are the tester's choices)
//!
//! - A page carries at most 500 bins. Page `p` (0-based) holds bins `500 * p ..` up to the
//!   histogram's end, so a histogram of `n` bins has `ceil(n / 500)` pages (at least one: an
//!   empty page 0 for a direction with the statistics off).
//! - `DirectionStats::delay_hist_reply(param)` takes `"<dir>"` (= page 0, so the old query keeps
//!   working) or `"<dir>,<page>"`; both parts are plain decimal numbers. A page that does not
//!   exist (`>= number of pages`) is `None`, which the telemetry registration turns into an error
//!   reply, like a bad `<dir>`. Garbage after the comma is `None` too.
//! - Every page carries `lo_ns`, `bin_ns`, `num_bins` (of the whole histogram), `underflow`,
//!   `overflow` (cumulative, so the same numbers on every page unless packets arrive between two
//!   reads), `bins` and the page's position so the reader can check it.
//! - ASSUMPTION (the spec says only "the page's index/first bin"): the two position fields are
//!   `page` (the 0-based page index, Uint) and `first_bin` (the index of the first bin of the
//!   page, `500 * page`, Uint). A page reply has exactly these eight keys.
//! - ASSUMPTION: a direction with the statistics off keeps its empty page 0 (`Some`, no bins, as
//!   before), and has no page 1.
//! - `Stats::delay_hist_reply()` (no parameter) is not part of the paging API and is left to the
//!   implementer; the paged replies are read through `DirectionStats`.
//!
//! The shape tests run in every build (without the `telemetry` feature the counts are zero); the
//! test that checks the counts of the pages against what was recorded needs the feature.

use dpds::delay_stats::{HistRange, MAX_BINS};
use dpds::reply::{Reply, Value};
use dpds::stats::DirectionStats;

/// Bins per page (the spec: "at most 500 bins" per page; DPDK's array limit is 512).
const PAGE: usize = 500;

fn range(bins: usize) -> HistRange {
    HistRange::with_bins(10.0, 1.0, bins).unwrap()
}

/// One direction per entry of `bins`.
fn directions(bins: &[usize]) -> DirectionStats {
    DirectionStats::with_ranges(bins.iter().map(|&n| range(n)).collect())
}

fn pages_of(bins: usize) -> usize {
    bins.div_ceil(PAGE).max(1)
}

fn val<'a>(r: &'a Reply, key: &str) -> &'a Value {
    let hits: Vec<_> = r.iter().filter(|(k, _)| k == key).collect();
    assert_eq!(
        hits.len(),
        1,
        "key {key:?} must appear exactly once in {r:?}"
    );
    &hits[0].1
}

fn uint(r: &Reply, key: &str) -> u64 {
    match val(r, key) {
        Value::Uint(v) => *v,
        other => panic!("{key}: expected Uint, got {other:?}"),
    }
}

/// A number that may be a Uint or a decimal Str.
fn num(r: &Reply, key: &str) -> f64 {
    match val(r, key) {
        Value::Uint(v) => *v as f64,
        Value::Str(s) => s
            .parse()
            .unwrap_or_else(|_| panic!("{key}: not a number: {s:?}")),
        other => panic!("{key}: expected a number, got {other:?}"),
    }
}

fn bins(r: &Reply) -> Vec<u64> {
    match val(r, "bins") {
        Value::UintArray(v) => v.clone(),
        other => panic!("bins: expected UintArray, got {other:?}"),
    }
}

fn keys(r: &Reply) -> Vec<&str> {
    let mut k: Vec<&str> = r.iter().map(|(k, _)| k.as_str()).collect();
    k.sort_unstable();
    k
}

/// The reply of page `p` of direction `d` (`None` for an error reply).
fn page(ds: &DirectionStats, d: usize, p: usize) -> Option<Reply> {
    ds.delay_hist_reply(&format!("{d},{p}"))
}

/// Checks every page of direction `d` (a histogram of `n` bins): the page count, the length of
/// every page's array against what the page says it carries, the position fields, and that the
/// page after the last one is an error.
fn check_all_pages(ds: &DirectionStats, d: usize, n: usize) {
    let pages = pages_of(n);
    let mut total = 0;
    for p in 0..pages {
        let r = page(ds, d, p).unwrap_or_else(|| panic!("n = {n}: page {p} of {pages} exists"));
        assert_eq!(uint(&r, "num_bins"), n as u64, "n = {n}, page {p}");
        assert_eq!(uint(&r, "page"), p as u64, "n = {n}");
        assert_eq!(
            uint(&r, "first_bin"),
            (p * PAGE) as u64,
            "n = {n}, page {p}"
        );
        let want = PAGE.min(n - p * PAGE);
        // the array carries exactly what the page says: no silent truncation at 512 entries
        assert_eq!(
            bins(&r).len(),
            want,
            "n = {n}, page {p}: {} bins",
            bins(&r).len()
        );
        assert!(bins(&r).len() <= PAGE, "n = {n}, page {p}");
        assert_eq!(num(&r, "lo_ns"), 10.0, "n = {n}, page {p}");
        assert_eq!(num(&r, "bin_ns"), 1.0, "n = {n}, page {p}");
        total += bins(&r).len();
    }
    assert_eq!(total, n, "the pages together hold every bin");
    assert!(
        page(ds, d, pages).is_none(),
        "n = {n}: page {pages} does not exist and is an error reply"
    );
    assert!(page(ds, d, pages + 7).is_none(), "n = {n}");
}

#[test]
fn a_page_has_exactly_the_documented_keys() {
    let ds = directions(&[4096]);
    for p in [0, 1, 8] {
        let r = page(&ds, 0, p).unwrap();
        assert_eq!(
            keys(&r),
            [
                "bin_ns",
                "bins",
                "first_bin",
                "lo_ns",
                "num_bins",
                "overflow",
                "page",
                "underflow"
            ],
            "page {p}"
        );
    }
}

#[test]
fn the_maximum_histogram_of_4096_bins_has_nine_pages() {
    assert_eq!(MAX_BINS, 4096);
    let ds = directions(&[MAX_BINS]);
    check_all_pages(&ds, 0, 4096);
    // eight full pages of 500 and a last one of 96
    assert_eq!(bins(&page(&ds, 0, 0).unwrap()).len(), 500);
    assert_eq!(bins(&page(&ds, 0, 7).unwrap()).len(), 500);
    let last = page(&ds, 0, 8).unwrap();
    assert_eq!(bins(&last).len(), 96);
    assert_eq!(uint(&last, "first_bin"), 4000);
    assert!(page(&ds, 0, 9).is_none());
}

#[test]
fn the_page_count_follows_the_bin_count_around_the_page_boundaries() {
    for n in [
        1, 2, 255, 256, 499, 500, 501, 512, 513, 999, 1000, 1001, 1500, 2048, 4000, 4095, 4096,
    ] {
        let ds = directions(&[n]);
        check_all_pages(&ds, 0, n);
    }
}

#[test]
fn one_page_is_enough_up_to_500_bins_and_501_bins_need_a_second() {
    let ds = directions(&[500, 501]);
    assert!(page(&ds, 0, 1).is_none(), "500 bins are one page");
    let second = page(&ds, 1, 1).expect("501 bins are two pages");
    assert_eq!(bins(&second).len(), 1);
    assert_eq!(uint(&second, "first_bin"), 500);
}

#[test]
fn a_direction_alone_is_page_zero() {
    for n in [256, 500, 501, 4096] {
        let ds = directions(&[n]);
        let alone = ds.delay_hist_reply("0").expect("`,<dir>` alone is page 0");
        assert_eq!(Some(alone.clone()), page(&ds, 0, 0), "n = {n}");
        assert_eq!(uint(&alone, "page"), 0);
        assert_eq!(uint(&alone, "first_bin"), 0);
        assert_eq!(bins(&alone).len(), n.min(PAGE), "n = {n}");
        assert_eq!(uint(&alone, "num_bins"), n as u64);
    }
}

#[test]
fn every_direction_has_its_own_pages() {
    let ds = directions(&[4096, 256, 1200]);
    check_all_pages(&ds, 0, 4096);
    check_all_pages(&ds, 1, 256);
    check_all_pages(&ds, 2, 1200);
    // a direction that does not exist has no pages
    assert!(page(&ds, 3, 0).is_none());
    assert!(ds.delay_hist_reply("3").is_none());
}

#[test]
fn a_bad_page_is_an_error_reply() {
    let ds = directions(&[1000]);
    assert!(ds.delay_hist_reply("0,0").is_some());
    assert!(ds.delay_hist_reply("0,1").is_some());
    for bad in [
        "0,2",
        "0,3",
        "0,99999",
        "0,18446744073709551615",
        "0,99999999999999999999999999",
        "0,",
        "0,x",
        "0,-1",
        "0,+1",
        "0, 1",
        "0 ,1",
        "0,1 ",
        "0,0x1",
        "0,1.0",
        "0,1,2",
        "0,0,0",
        "0,,1",
        ",1",
        ",",
        "",
        ",0,1",
        "x,0",
        "1,0",
        "-1,0",
    ] {
        assert!(ds.delay_hist_reply(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn a_switched_off_direction_keeps_an_empty_page_zero_and_has_no_page_one() {
    let ds = DirectionStats::with_delay_settings(vec![(range(4096), false), (range(4096), true)]);
    let off = ds.delay_hist_reply("0").expect("an existing direction");
    for (k, v) in &off {
        match (k.as_str(), v) {
            ("bins", Value::UintArray(b)) => assert!(b.is_empty(), "{off:?}"),
            ("num_bins" | "underflow" | "overflow", Value::Uint(n)) => assert_eq!(*n, 0, "{k}"),
            _ => {}
        }
    }
    assert_eq!(ds.delay_hist_reply("0,0"), Some(off));
    assert!(ds.delay_hist_reply("0,1").is_none());
    // the other direction is unaffected
    check_all_pages(&ds, 1, 4096);
}

#[cfg(feature = "telemetry")]
mod with_feature {
    use super::*;
    use dpds::delay_stats::DelayBatch;

    /// Records `count` packets of intended delay `delay_ns` (at now = 0).
    fn record(ds: &DirectionStats, d: usize, delay_ns: u64, count: u64) {
        let mut batch = DelayBatch::new();
        for _ in 0..count {
            batch.begin();
            batch.accepted(delay_ns);
            ds.stats[d].delay.record_accepts(&batch, 0.0);
        }
    }

    #[test]
    fn the_pages_concatenate_to_the_recorded_histogram() {
        // range [10, 4106) with 1 ns bins: a delay of 10 + i ns is bin i
        let ds = directions(&[4096, 100]);
        let counts: [(usize, u64); 11] = [
            (0, 1),
            (1, 2),
            (499, 3),
            (500, 4),
            (501, 5),
            (999, 6),
            (1000, 7),
            (3999, 8),
            (4000, 9),
            (4094, 10),
            (4095, 11),
        ];
        for (bin, n) in counts {
            record(&ds, 0, 10 + bin as u64, n);
        }
        record(&ds, 0, 5, 2); // below lo: underflow
        record(&ds, 0, 4106, 3); // lo + 4096 bins: overflow
        record(&ds, 0, 99_999, 1);
        record(&ds, 1, 10 + 99, 4); // the second direction is a single page

        let mut all = Vec::new();
        for p in 0..9 {
            let r = page(&ds, 0, p).unwrap();
            // the counters outside the bins are on every page
            assert_eq!(uint(&r, "underflow"), 2, "page {p}");
            assert_eq!(uint(&r, "overflow"), 4, "page {p}");
            assert_eq!(uint(&r, "num_bins"), 4096, "page {p}");
            all.extend(bins(&r));
        }
        assert_eq!(all.len(), 4096);
        let mut want = vec![0u64; 4096];
        for (bin, n) in counts {
            want[bin] = n;
        }
        assert_eq!(
            all, want,
            "bin i of the histogram is bin i % 500 of page i / 500"
        );
        assert_eq!(
            all.iter().sum::<u64>(),
            1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9 + 10 + 11
        );

        // `,<dir>` alone is page 0 with the same counts
        assert_eq!(
            bins(&ds.delay_hist_reply("0").unwrap()),
            want[..500].to_vec()
        );
        // the other direction is not mixed up
        let r1 = ds.delay_hist_reply("1").unwrap();
        assert_eq!(bins(&r1).len(), 100);
        assert_eq!(bins(&r1)[99], 4);
        assert_eq!(bins(&r1).iter().sum::<u64>(), 4);
    }

    #[test]
    fn a_page_read_later_sees_the_later_counts() {
        // cumulative: nothing is cached between two page reads
        let ds = directions(&[1000]);
        record(&ds, 0, 10 + 700, 1);
        assert_eq!(bins(&page(&ds, 0, 1).unwrap())[200], 1);
        record(&ds, 0, 10 + 700, 2);
        assert_eq!(bins(&page(&ds, 0, 1).unwrap())[200], 3);
        assert_eq!(bins(&page(&ds, 0, 0).unwrap()).iter().sum::<u64>(), 0);
    }
}
