//! Delay statistics and the delay histogram of one direction (A3).
//!
//! # Test-facing API (new)
//!
//! Module `dpds::delay_stats`, available in every build (pure, no feature gate):
//!
//! ```text
//! pub const HIST_BINS: usize = 256;        (the default number of bins)
//! pub const MAX_BINS: usize = 4096;        (spec 30: read in pages of 500, see hist_pages.rs)
//! pub const MIN_WIDTH_NS: f64 = 1000.0;
//! #[derive(Debug, Clone, Copy, PartialEq)]
//! pub struct HistRange { pub lo_ns: f64, pub bin_ns: f64, pub bins: usize }
//!   HistRange::new(lo_ns, bin_ns) -> HistRange                 (256 bins)
//!   HistRange::with_bins(lo_ns, bin_ns, bins) -> Result<HistRange, HistogramError>
//!                                                    (Err for 0 bins; D4 of the fix round)
//!   HistRange::default() = new(0.0, MIN_WIDTH_NS / 256.0)      (the "mean 0, std 0" window)
//!   HistRange::from_config(&dpds_core::Config) -> HistRange    (the automatic rule, below; 256 bins)
//!   HistRange::from_spec(&Config, &HistogramSpec) -> Result<HistRange, HistogramError>
//!   HistRange::hi_ns(&self) -> f64 = lo_ns + bins * bin_ns      (exclusive)
//!   HistRange::bin(&self, delay_ns: f64) -> Bin
//! #[derive(Debug, Clone, Copy, PartialEq, Default)]
//! pub struct HistogramSpec { pub bins: Option<usize>, pub lo_ns: Option<f64>, pub hi_ns: Option<f64> }
//!   (the optional per-direction `"histograms"` entry of the links file, see below)
//! HistogramError: Debug + Display + std::error::Error
//! #[derive(Debug, Clone, Copy, PartialEq, Eq)]
//! pub enum Bin { Underflow, Index(usize), Overflow }
//! ```
//!
//! ## Bin placement (`HistRange::bin`)
//! Half-open `[lo_ns, lo_ns + bins * bin_ns)`: `d < lo_ns` -> Underflow;
//! `d >= hi_ns` -> Overflow; else `Index(floor((d - lo_ns) * (1 / bin_ns)))`
//! (one subtract, one multiply by the precomputed reciprocal, no division in
//! the loop). The tests use `bin_ns` of 4.0 / 1.0 / 0.5, whose reciprocal is
//! exact, so edges are exact.
//!
//! ## Range rule (`HistRange::from_config`), the "documented rule"
//! From `delayer.mean_ns`, `delayer.std_ns` (whatever the distribution) and,
//! when the limiter is active, the Spacer queueing allowance
//! `extra = limit / rate_bps * 1e9` ns with the DERIVED limit
//! `limit = buffer_capacity * 1542 * 8` bit (spec20; `limit_bit` no longer exists;
//! a Spacer packet can wait up to a full limit's worth of bits; the realized delay
//! includes that offset). `2 * TX_MAX_WAIT_NS` (20 us, `TX_MAX_WAIT_NS` in `dpds::consts`) is always added to
//! `hi`, so the window is at least that wide and the minimum-width rule (1000 ns) never
//! applies any more:
//!
//! ```text
//! lo = max(0, mean - 6 * std)
//! hi = mean + 6 * std + extra + 2 * TX_MAX_WAIT_NS     (spec 57 T9: twice the staging wait)
//! if hi - lo < 1000 ns:           (deterministic delay etc.: a window around the middle)
//!     mid = (lo + hi) / 2;  lo = max(0, mid - 500);  hi = lo + 1000
//! bin_ns = (hi - lo) / 256
//! ```
//!
//! ## Configurable histogram (`HistRange::from_spec`)
//! `HistogramSpec { bins, lo_ns, hi_ns }`, every field optional; a missing field
//! takes the automatic default (256 bins, `lo`/`hi` of the rule above for the
//! direction's config). `bin_ns = (hi - lo) / bins`. Explicit values are taken
//! as given (the 1 us minimum width applies to the automatic range only).
//! Errors (`HistogramError`, Display names the offending field): `bins` not in
//! `1..=4096`; `lo_ns` or `hi_ns` negative or not finite; the EFFECTIVE `lo >= hi`
//! (also when only one of them is given and it lies beyond the automatic other):
//! that message names both `lo_ns` and `hi_ns`.
//!
//! ## Stats (`/dpds/stats,<dir>` via `Stats::stats_reply`)
//! Spec 57: both are recorded at the TX enqueue, for the packets the NIC ACCEPTED
//! (`staged[..sent]` of the `tx_burst`; the freed tail and rejected packets are not
//! counted). `tx_ns` is the clock read immediately before `tx_burst` (delay-stats instance
//! only). The delay is the realized delay `max(0, tx_ns - arrival_ns)` (arrival = the `now`
//! of the push, `pop_due_with_time`'s third element); the lateness is
//! `max(0, tx_ns - send_time_ns as f64)` and includes the staging wait (up to
//! `TX_MAX_WAIT_NS`); `lateness_mean_ns` is the lateness sum over the number of recorded
//! packets (the same count as the delay). Consequently, with a monotone clock the realized
//! delay of a packet is at least its intended delay plus the wait of the batch. The
//! histogram range rules above include `2 * TX_MAX_WAIT_NS` in `hi` (they are about the configured
//! delay plus the staging wait, T9), so the histogram tests below compute the expected bin
//! with `HistRange::bin` where the place depends on the clock.
//! Keys (always present, zero before any packet):
//!   `delay_mean_ns`, `delay_std_ns`, `lateness_mean_ns`: Str, `format!("{:.2}")`
//!       (like `tx_pkts_per_call`; DPDK telemetry has no float setter);
//!       std is the POPULATION standard deviation (divide by n);
//!   `delay_min_ns`, `delay_max_ns`, `lateness_max_ns`: Uint (truncated).
//! Numerics: std must stay exact for a large offset with a tiny spread (the
//! test below uses 1e10 ns +- 3 us): shift by a reference value or Welford,
//! not naive sum / sum-of-squares in f64.
//!
//! ## Histogram (`/dpds/delay_hist,<dir>`)
//!   `Stats::with_range(HistRange) -> Stats` (`Stats::default()` uses
//!   `HistRange::default()`), `DirectionStats::with_ranges(Vec<HistRange>)`
//!   (one per direction; `DirectionStats::new(n)` = n default ranges),
//!   `Stats::delay_hist_reply(&self) -> Reply` and
//!   `DirectionStats::delay_hist_reply(&self, param: &str) -> Option<Reply>`
//!   (`None` for a bad direction param, as `stats_reply`).
//!   Spec 30: the reply is a PAGE (see hist_pages.rs). Keys, exactly these eight:
//!   `lo_ns`, `bin_ns` (numbers: Uint or a decimal Str, the tests accept both),
//!   `num_bins` (Uint, the actual number of bins of the whole histogram),
//!   `bins` (UintArray, at most 500 entries of the page), `underflow`, `overflow`
//!   (Uint), `page`, `first_bin` (Uint, the position of the page). Cumulative since start; every accepted packet is in
//!   exactly one of bins/underflow/overflow.
//!
//! The telemetry-dependent tests only run with `--features telemetry`.
//!
//! # Fix round (spec v6): D4 and D5
//!
//! D4: `HistRange::with_bins(lo_ns, bin_ns, bins) -> Result<HistRange, HistogramError>`;
//! zero bins is an `Err` whose Display names `bins` (the constructor returned a plain
//! `HistRange` before). `HistRange::new` stays infallible (256 bins).
//!
//! D5, the per-direction delay-statistics switch (`"delay_stats": [..]` of the links file):
//! new API, all builds (the switch is plain data, only the recording needs the feature):
//!
//! ```text
//! Stats::with_delay_stats(range: HistRange, enabled: bool) -> Stats
//!     `Stats::with_range(r)` == `Stats::with_delay_stats(r, true)`.
//! DirectionStats::with_delay_settings(Vec<(HistRange, bool)>) -> DirectionStats
//!     one (range, enabled) per direction.
//! ```
//!
//! Off means, for that direction:
//!   - `Stats::stats_reply` / `DirectionStats::stats_reply`: the six keys `delay_mean_ns`,
//!     `delay_std_ns`, `delay_min_ns`, `delay_max_ns`, `lateness_mean_ns`, `lateness_max_ns`
//!     are ABSENT (not zero, not null: a telemetry dict has no null); every other key is
//!     present and unchanged, in the same order (so the reply has 22 keys instead of 28);
//!   - `delay_hist_reply` is EMPTY: no bins (`bins` absent or an empty array, `num_bins`
//!     absent or 0, `underflow`/`overflow` absent or 0), but it is still `Some(..)` for
//!     `DirectionStats::delay_hist_reply` of an existing direction (a bad direction param
//!     stays `None`);
//!   - with `--features telemetry`: `forward::run` records no realized delay, lateness
//!     or histogram entry, while every counter and drop cause is counted as before.

use dpds::consts::TX_MAX_WAIT_NS;

/// Spec 57 T9: the automatic window ends this far above `mean + 6 std + extra`.
const WINDOW_MARGIN_NS: f64 = 2.0 * TX_MAX_WAIT_NS;
use dpds::delay_stats::{Bin, HistRange, HistogramSpec, HIST_BINS, MAX_BINS, MIN_WIDTH_NS};
use dpds::reply::{Reply, Value};
use dpds::stats::{DirectionStats, Stats};
use serde_json::json;

use crate::support::*;

/// Capacity 10 (derived limit 123_360 bit); `limiter` = the active Spacer's rate.
fn cfg(mean: f64, std: f64, limiter: Option<f64>) -> dpds_core::Config {
    cfg_cap(10, mean, std, limiter.map(|r| ("Spacer", r)))
}

fn cfg_cap(
    capacity: usize,
    mean: f64,
    std: f64,
    limiter: Option<(&str, f64)>,
) -> dpds_core::Config {
    config_general(
        capacity,
        false,
        "NormalDistribution",
        mean,
        std,
        limiter,
        json!({ "Random": 0.0 }),
    )
}

/// `HistRange::with_bins` for the valid bin counts the tests use.
fn wb(lo_ns: f64, bin_ns: f64, bins: usize) -> HistRange {
    HistRange::with_bins(lo_ns, bin_ns, bins).unwrap()
}

#[test]
fn constants_and_the_default_range() {
    assert_eq!(HIST_BINS, 256);
    assert_eq!(MAX_BINS, 4096);
    assert_eq!(MIN_WIDTH_NS, 1000.0);
    let d = HistRange::default();
    assert_eq!((d.lo_ns, d.bin_ns, d.bins), (0.0, 1000.0 / 256.0, 256));
    assert_eq!(HistRange::new(5.0, 2.0), wb(5.0, 2.0, 256));
    assert_eq!(d.hi_ns(), 1000.0);
}

// ── bin placement ───────────────────────────────────────────────────

#[test]
fn bin_edges_of_a_four_ns_wide_range() {
    let r = HistRange::new(1000.0, 4.0);
    assert_eq!(r.hi_ns(), 2024.0);
    for (d, expect) in [
        (999.999, Bin::Underflow),
        (-5.0, Bin::Underflow),
        (0.0, Bin::Underflow),
        (1000.0, Bin::Index(0)),
        (1003.999, Bin::Index(0)),
        (1004.0, Bin::Index(1)),
        (1500.0, Bin::Index(125)),
        (2019.999, Bin::Index(254)),
        (2020.0, Bin::Index(255)),
        (2023.999, Bin::Index(255)),
        (2024.0, Bin::Overflow),
        (1e12, Bin::Overflow),
        (f64::INFINITY, Bin::Overflow),
    ] {
        assert_eq!(r.bin(d), expect, "d = {d}");
    }
}

#[test]
fn bin_edges_of_a_half_ns_range_starting_at_zero() {
    let r = HistRange::new(0.0, 0.5);
    assert_eq!(r.hi_ns(), 128.0);
    for (d, expect) in [
        (-0.001, Bin::Underflow),
        (0.0, Bin::Index(0)),
        (0.49, Bin::Index(0)),
        (0.5, Bin::Index(1)),
        (127.5, Bin::Index(255)),
        (127.99, Bin::Index(255)),
        (128.0, Bin::Overflow),
    ] {
        assert_eq!(r.bin(d), expect, "d = {d}");
    }
}

#[test]
fn every_bin_index_is_reachable_and_in_range() {
    let r = HistRange::new(10.0, 1.0);
    for k in 0..HIST_BINS {
        assert_eq!(r.bin(10.0 + k as f64 + 0.5), Bin::Index(k));
    }
}

// ── range rule ──────────────────────────────────────────────────────

fn range(c: dpds_core::Config) -> (f64, f64) {
    let r = HistRange::from_config(&c);
    (r.lo_ns, r.bin_ns)
}

/// The window `[lo, hi)` as `(lo, bin_ns)` for the bin width `(hi - lo) / 256`.
fn window(lo: f64, hi: f64) -> (f64, f64) {
    (lo, (hi - lo) / 256.0)
}

#[test]
fn a_deterministic_delay_gets_the_staging_wait_as_its_window() {
    // mean 1 ms, std 0: [1e6, 1e6 + 2 * TX_MAX_WAIT_NS), width 20 us (>= 1 us: no recentring)
    let w = WINDOW_MARGIN_NS;
    assert_eq!(range(cfg(1_000_000.0, 0.0, None)), window(1e6, 1e6 + w));
    assert_eq!(range(cfg(1_000_000.0, 0.0, None)), (1e6, 78.125));
    let r = HistRange::from_config(&cfg(1_000_000.0, 0.0, None));
    assert_eq!(r.hi_ns(), 1e6 + w);
}

#[test]
fn the_window_starts_at_zero_or_at_the_lower_edge_and_ends_one_wait_above_the_mean() {
    let w = WINDOW_MARGIN_NS;
    assert_eq!(range(cfg(0.0, 0.0, None)), window(0.0, w));
    // no recentring any more: lo is the mean itself
    assert_eq!(range(cfg(200.0, 0.0, None)), window(200.0, 200.0 + w));
}

#[test]
fn a_small_spread_adds_to_the_staging_wait() {
    // 12 * std = 120 ns: [mean - 60, mean + 60 + 2 W)
    let w = WINDOW_MARGIN_NS;
    assert_eq!(
        range(cfg(100_000.0, 10.0, None)),
        window(100_000.0 - 60.0, 100_000.0 + 60.0 + w)
    );
}

#[test]
fn a_wide_spread_is_mean_plus_minus_six_std_plus_the_wait_above() {
    let w = WINDOW_MARGIN_NS;
    // [4 ms, 16 ms + 2 W)
    assert_eq!(range(cfg(10e6, 1e6, None)), window(4e6, 16e6 + w));
    assert_eq!(
        range(cfg(1e6, 100.0, None)),
        window(1e6 - 600.0, 1e6 + 600.0 + w)
    );
}

#[test]
fn mean_minus_six_std_below_zero_is_clamped_at_zero() {
    // [0, 7 ms + 2 W): the clamped lower edge, upper edge mean + 6 std + 2 W
    assert_eq!(
        range(cfg(1e6, 1e6, None)),
        window(0.0, 7e6 + WINDOW_MARGIN_NS)
    );
}

/// The allowance is a floating-point division (123360 / 1e9 * 1e9 is not exact), so the
/// bin width is compared to 1e-9 relative; the lower edge stays exact.
fn assert_range(got: (f64, f64), want: (f64, f64)) {
    assert_eq!(got.0, want.0, "lower edge");
    assert!(
        (got.1 - want.1).abs() <= want.1 * 1e-9,
        "bin width {} vs {}",
        got.1,
        want.1
    );
}

#[test]
fn an_active_limiter_widens_the_range_by_its_queueing_allowance() {
    // capacity 10: derived limit 123_360 bit at 1e9 bit/s = 123_360 ns of possible Spacer
    // queueing: [0, 123_360 + 2 W]
    let w = WINDOW_MARGIN_NS;
    assert_range(range(cfg(0.0, 0.0, Some(1e9))), window(0.0, 123_360.0 + w));
    // [mean - 6 std, mean + 6 std + 123_360 + 2 W] = [4e5, 1_733_360]
    assert_range(
        range(cfg(1e6, 1e5, Some(1e9))),
        window(4e5, 1.6e6 + 123_360.0 + w),
    );
}

#[test]
fn the_queueing_allowance_follows_the_capacity_and_the_rate() {
    let w = WINDOW_MARGIN_NS;
    // twice the capacity: twice the derived limit: [0, 246_720 + 2 W]
    assert_range(
        range(cfg_cap(20, 0.0, 0.0, Some(("Spacer", 1e9)))),
        window(0.0, 246_720.0 + w),
    );
    // ten times the rate: a tenth of the allowance: [0, 12_336 + 2 W]
    assert_range(
        range(cfg_cap(10, 0.0, 0.0, Some(("Spacer", 1e10)))),
        window(0.0, 12_336.0 + w),
    );
    // an inactive limiter adds nothing whatever the capacity: only the wait
    assert_eq!(range(cfg_cap(10_000, 0.0, 0.0, None)), window(0.0, w));
}

#[test]
fn a_token_bucket_filter_adds_no_allowance_to_the_range() {
    // the TBF never delays a packet: same window as without a limiter
    assert_eq!(
        range(cfg_cap(10, 0.0, 0.0, Some(("TokenBucketFilter", 1e9)))),
        window(0.0, WINDOW_MARGIN_NS)
    );
}

#[test]
fn an_inactive_limiter_does_not_widen_the_range() {
    let c = config_general(
        10,
        false,
        "NormalDistribution",
        0.0,
        0.0,
        None, // activate = false
        json!({ "Random": 0.0 }),
    );
    assert_eq!(range(c), window(0.0, WINDOW_MARGIN_NS));
}

#[test]
fn the_rule_ignores_the_distribution_kind() {
    for dist in [
        "DeterministicDistribution",
        "UniformDistribution",
        "GammaDistribution",
    ] {
        let c = config_general(10, false, dist, 10e6, 0.0, None, json!({ "Random": 0.0 }));
        // std 0 is valid for the deterministic kind only; the others may
        // reject it, which is not what is tested here
        if dpds_core::buffer::Buffer::try_new(c).is_ok() {
            assert_eq!(range(c), window(10e6, 10e6 + WINDOW_MARGIN_NS), "{dist}");
        }
    }
}

// ── custom number of bins ───────────────────────────────────────────

#[test]
fn bin_placement_with_a_custom_number_of_bins() {
    let r = wb(1000.0, 4.0, 10);
    assert_eq!(r.hi_ns(), 1040.0);
    for (d, expect) in [
        (999.999, Bin::Underflow),
        (1000.0, Bin::Index(0)),
        (1004.0, Bin::Index(1)),
        (1039.999, Bin::Index(9)),
        (1040.0, Bin::Overflow),
        (1e9, Bin::Overflow),
    ] {
        assert_eq!(r.bin(d), expect, "d = {d}");
    }
    // one bin: everything in [lo, lo + bin) is bin 0
    let one = wb(10.0, 2.0, 1);
    assert_eq!(one.bin(9.999), Bin::Underflow);
    assert_eq!(one.bin(10.0), Bin::Index(0));
    assert_eq!(one.bin(11.999), Bin::Index(0));
    assert_eq!(one.bin(12.0), Bin::Overflow);
    // the maximum of 4096 bins
    let max = wb(0.0, 2.0, MAX_BINS);
    assert_eq!(max.hi_ns(), 8192.0);
    assert_eq!(max.bin(8191.999), Bin::Index(4095));
    assert_eq!(max.bin(8192.0), Bin::Overflow);
    assert_eq!(max.bin(1000.0), Bin::Index(500));
    assert_eq!(max.bin(0.0), Bin::Index(0));
}

fn spec(bins: Option<usize>, lo: Option<f64>, hi: Option<f64>) -> HistogramSpec {
    HistogramSpec {
        bins,
        lo_ns: lo,
        hi_ns: hi,
    }
}

fn from_spec(c: dpds_core::Config, sp: HistogramSpec) -> Result<HistRange, String> {
    HistRange::from_spec(&c, &sp).map_err(|e| e.to_string())
}

#[test]
fn the_default_spec_is_the_automatic_range() {
    assert_eq!(HistogramSpec::default(), spec(None, None, None));
    for c in [
        cfg(1e6, 0.0, None),
        cfg(10e6, 1e6, None),
        cfg(1e6, 1e5, Some(1e9)),
    ] {
        assert_eq!(
            from_spec(c, HistogramSpec::default()).unwrap(),
            HistRange::from_config(&c)
        );
    }
    assert_eq!(HistRange::from_config(&cfg(10e6, 1e6, None)).bins, 256);
}

#[test]
fn each_missing_field_falls_back_to_the_automatic_value() {
    // automatic [4 ms, 16 ms + 2 * TX_MAX_WAIT_NS), 256 bins
    let c = cfg(10e6, 1e6, None);
    let hi = 16e6 + WINDOW_MARGIN_NS;
    // only bins: the automatic range cut into 100 bins
    assert_eq!(
        from_spec(c, spec(Some(100), None, None)).unwrap(),
        wb(4e6, (hi - 4e6) / 100.0, 100)
    );
    // only lo: [5 ms, 16 ms + 2 W) in 256 bins
    assert_eq!(
        from_spec(c, spec(None, Some(5e6), None)).unwrap(),
        wb(5e6, (hi - 5e6) / 256.0, 256)
    );
    // only hi: [4 ms, 20 ms) in 256 bins (an explicit hi is taken as given, no wait added)
    assert_eq!(
        from_spec(c, spec(None, None, Some(20e6))).unwrap(),
        wb(4e6, 62_500.0, 256)
    );
    // lo and hi, default bins
    assert_eq!(
        from_spec(c, spec(None, Some(100.0), Some(612.0))).unwrap(),
        wb(100.0, 2.0, 256)
    );
}

#[test]
fn explicit_values_are_used_as_given() {
    let c = cfg(10e6, 1e6, None);
    assert_eq!(
        from_spec(c, spec(Some(64), Some(100.0), Some(612.0))).unwrap(),
        wb(100.0, 8.0, 64)
    );
    // lo = 0 is allowed; a window narrower than the automatic 1 us minimum too
    assert_eq!(
        from_spec(c, spec(Some(10), Some(0.0), Some(10.0))).unwrap(),
        wb(0.0, 1.0, 10)
    );
    // the bins limits are inclusive
    assert_eq!(
        from_spec(c, spec(Some(1), Some(0.0), Some(8.0)))
            .unwrap()
            .bin_ns,
        8.0
    );
    assert_eq!(
        from_spec(c, spec(Some(500), Some(0.0), Some(1000.0)))
            .unwrap()
            .bin_ns,
        2.0
    );
    // spec 30: 4096 is the new maximum
    assert_eq!(
        from_spec(c, spec(Some(4096), Some(0.0), Some(8192.0)))
            .unwrap()
            .bins,
        4096
    );
    assert_eq!(
        from_spec(c, spec(Some(4096), None, None)).unwrap().bins,
        4096
    );
}

#[test]
fn bins_above_4096_are_refused_and_the_message_names_the_limit() {
    let c = cfg(10e6, 1e6, None);
    assert!(spec(Some(4096), None, None).validate().is_ok());
    assert!(
        spec(Some(501), None, None).validate().is_ok(),
        "501 was the old limit + 1"
    );
    for n in [4097, 5000, 100_000] {
        let e = spec(Some(n), None, None)
            .validate()
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("4096"),
            "bins = {n}: the message names the maximum: {e}"
        );
        assert!(e.contains("bins"), "{e}");
        let e = from_spec(c, spec(Some(n), None, None)).unwrap_err();
        assert!(e.contains("4096"), "bins = {n}: {e}");
    }
}

#[test]
fn a_bad_histogram_spec_is_an_error_naming_the_field() {
    let c = cfg(10e6, 1e6, None);
    for (sp, field) in [
        (spec(Some(0), None, None), "bins"),
        (spec(Some(MAX_BINS + 1), None, None), "bins"),
        (spec(Some(100_000), None, None), "bins"),
        (spec(None, Some(-1.0), None), "lo_ns"),
        (spec(None, None, Some(-1.0)), "hi_ns"),
        (spec(None, Some(f64::NAN), None), "lo_ns"),
        (spec(None, None, Some(f64::INFINITY)), "hi_ns"),
        (spec(None, Some(5.0), Some(5.0)), "lo_ns,hi_ns"), // lo == hi
        (spec(None, Some(9.0), Some(5.0)), "lo_ns,hi_ns"), // lo > hi
        (spec(None, Some(20e6), None), "lo_ns,hi_ns"),     // beyond the automatic hi (16 ms + 2 W)
        (spec(None, None, Some(1e6)), "lo_ns,hi_ns"),      // below the automatic lo (4 ms)
    ] {
        let e = from_spec(c, sp).expect_err(&format!("{sp:?}"));
        for f in field.split(',') {
            assert!(e.contains(f), "{sp:?}: the message names {f}: {e}");
        }
    }
    // the same lo alone is fine once it lies inside the automatic range
    assert!(from_spec(c, spec(None, Some(15e6), None)).is_ok());
}

#[test]
fn histogram_error_is_a_std_error() {
    fn assert_error<E: std::error::Error + std::fmt::Debug>(_: &E) {}
    let e = HistRange::from_spec(&cfg(1e6, 0.0, None), &spec(Some(0), None, None)).unwrap_err();
    assert_error(&e);
}

// ── stats and histogram through forward::run ────────────────────────

#[cfg(feature = "telemetry")]
mod with_feature {
    use dpds::delay_stats::HistRange;
    use dpds::forward::run;
    use dpds::quit::Quit;
    use dpds::reply::{Reply, Value};
    use dpds::stats::{DirectionStats, Stats, Telemetry};
    use dpds::worker::Worker;
    use dpds_core::buffer::Buffer;
    use dpds_core::Config;

    use super::*;

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
    fn string<'a>(r: &'a Reply, key: &str) -> &'a str {
        match val(r, key) {
            Value::Str(v) => v,
            other => panic!("{key}: expected Str, got {other:?}"),
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

    /// One direction (port 0 -> 0), one burst `lens` (the packet length of packet i). Clock
    /// reads, in order: `clock[0]` = the burst's arrival (iteration 1; quit is requested in
    /// that rx call), `clock[1]` = `now` of iteration 2 (the pops and the send decision; it must
    /// be at least `TX_MAX_WAIT_NS` past the oldest send time), `clock[2]` = `tx_ns`, the extra
    /// read right before `tx_burst`; the last value repeats. The mbufs' `hash` starts as a
    /// sentinel: nothing may depend on it.
    fn drive(cfg: Config, lens: &[u32], clock: &[f64], stats: &'static Stats) {
        drive_bursts(cfg, &[lens], clock, stats, usize::MAX);
    }

    /// As [`drive`] with one burst per rx call (quit is requested in the last one) and a NIC
    /// that takes at most `accept` packets per `tx_burst`. The clock is `arrival_1 .. arrival_n,
    /// now, tx_ns`, which only holds while nothing is sent before the last iteration (the
    /// callers use a delay nothing is due for, or a zero delay with all arrivals within
    /// `TX_MAX_WAIT_NS`).
    fn drive_bursts(
        cfg: Config,
        bursts: &[&[u32]],
        clock: &[f64],
        stats: &'static Stats,
        accept: usize,
    ) {
        let total: usize = bursts.iter().map(|b| b.len()).sum();
        let mut m = FakeMbufs::new(total, 0, 64);
        let mut script = Vec::new();
        let mut next = 0;
        for b in bursts {
            let mut ptrs = Vec::new();
            for &l in *b {
                m.set_pkt_len(next, l);
                m.set_hash(next, HASH_SENTINEL);
                ptrs.push(m.ptr(next));
                next += 1;
            }
            script.push(ptrs);
        }
        let quit = Quit::new();
        let last = bursts.len();
        let mut rx = MockRx::with_script(script);
        {
            let q = &quit;
            rx.on_call = Some(Box::new(move |k| {
                if k == last {
                    q.request()
                }
            }));
        }
        let mut tx = MockTx {
            accept_limit: accept,
            ..Default::default()
        };
        let mut free = MockFree::default();
        let mut t = Telemetry::new(stats, 0.0);
        let (now, _) = scripted_clock(clock, None);
        // SAFETY: fake but valid mbufs owned by the test.
        unsafe {
            run(
                Worker::new(Buffer::new(cfg), 0, 0),
                &mut rx,
                &mut tx,
                &mut free,
                &mut t,
                &quit,
                now,
            )
        };
        // every mbuf was offered to the NIC or freed (an offered one the NIC refused is freed too)
        let seen: std::collections::HashSet<_> =
            tx.offered().into_iter().chain(free.freed()).collect();
        assert_eq!(seen.len(), total);
    }

    /// Asserts that `n` packets of realized delay `delay_ns` are in the one histogram place
    /// `range.bin(delay_ns)` names and nowhere else.
    fn assert_all_in_bin(h: &Reply, range: HistRange, delay_ns: f64, n: u64) {
        let (mut under, mut over, mut at) = (0, 0, None);
        match range.bin(delay_ns) {
            Bin::Underflow => under = n,
            Bin::Overflow => over = n,
            Bin::Index(i) => at = Some(i),
        }
        assert_eq!(uint(h, "underflow"), under, "{h:?}");
        assert_eq!(uint(h, "overflow"), over, "{h:?}");
        let b = bins(h);
        for (i, c) in b.iter().enumerate() {
            assert_eq!(*c, if Some(i) == at { n } else { 0 }, "bin {i}");
        }
    }

    /// Spacer 1 bit/ns (capacity 100: derived limit 1_217_600 bit, far above
    /// what the tests below queue), deterministic base delay `base`: all
    /// packets arrive at now = 0, so packet i is delayed by
    /// `base + sum_{j<i} 8 * (len_j + 24)` ns (the Spacer's backlog).
    fn spacer_cfg(base: f64) -> Config {
        config_general(
            100,
            false,
            "DeterministicDistribution",
            base,
            0.0,
            Some(("Spacer", 1e9)),
            json!({ "Random": 0.0 }),
        )
    }

    /// Capacity 4 (derived limit 49344 bit), Spacer 1 bit/ns, delay 1e6, loss
    /// model that drops exactly the first packet. Used with 1519-byte packets.
    fn mixed_limiter_cfg() -> Config {
        config_general(
            4,
            false,
            "DeterministicDistribution",
            1_000_000.0,
            0.0,
            Some(("Spacer", 1e9)),
            json!({ "GilbertElliot": [1.0, 0.0, 0.0, 1.0, true] }),
        )
    }

    #[test]
    fn a_fresh_stats_has_zero_delay_stats_and_an_empty_histogram() {
        let s = Stats::default();
        let r = s.stats_reply(0);
        for k in ["delay_mean_ns", "delay_std_ns", "lateness_mean_ns"] {
            assert_eq!(string(&r, k), "0.00", "{k}");
        }
        for k in ["delay_min_ns", "delay_max_ns", "lateness_max_ns"] {
            assert_eq!(uint(&r, k), 0, "{k}");
        }
        let h = s.delay_hist_reply();
        assert_eq!(h.len(), 8, "the six fields and the page position: {h:?}");
        assert_eq!(uint(&h, "num_bins"), 256);
        assert_eq!((uint(&h, "page"), uint(&h, "first_bin")), (0, 0));
        assert_eq!(bins(&h), vec![0u64; 256]);
        assert_eq!((uint(&h, "underflow"), uint(&h, "overflow")), (0, 0));
        assert_eq!(num(&h, "lo_ns"), 0.0);
        assert_eq!(num(&h, "bin_ns"), 3.90625);
    }

    #[test]
    fn delay_is_tx_minus_arrival_and_lateness_is_tx_minus_send_time() {
        // Spacer 1 bit/ns, base 800, lens 1, 0, 5, 0, 0, 0 all arriving at 0: send times 800,
        // 1000, 1192, 1424, 1616, 1808. Iteration 2 pops at 20_000 (oldest send time 19_200 old:
        // tx), the tx read is 30_000. Delay: 30_000 for every packet, std 0.
        // Lateness 30_000 - send: 29_200, 29_000, 28_808, 28_576, 28_384, 28_192 (sum 172_160).
        let stats: &'static Stats = Box::leak(Box::default());
        drive(
            spacer_cfg(800.0),
            &[1, 0, 5, 0, 0, 0],
            &[0.0, 20_000.0, 30_000.0],
            stats,
        );
        let r = stats.stats_reply(0);
        assert_eq!(string(&r, "delay_mean_ns"), "30000.00");
        assert_eq!(string(&r, "delay_std_ns"), "0.00");
        assert_eq!(uint(&r, "delay_min_ns"), 30_000);
        assert_eq!(uint(&r, "delay_max_ns"), 30_000);
        assert_eq!(string(&r, "lateness_mean_ns"), "28693.33");
        assert_eq!(uint(&r, "lateness_max_ns"), 29_200);
        assert_eq!(uint(&r, "tx_sent"), 6);
    }

    #[test]
    fn a_constant_delay_is_not_the_realized_delay_the_staging_wait_is_included() {
        // delay 5000, arrival 100, send time 5100. Popped at 20_000, tx at 25_000: realized
        // delay 24_900 (not the intended 5000), lateness 19_900.
        let stats: &'static Stats = Box::leak(Box::default());
        drive(
            config(100, 5_000.0, 0.0),
            &[64; 10],
            &[100.0, 20_000.0, 25_000.0],
            stats,
        );
        let r = stats.stats_reply(0);
        assert_eq!(string(&r, "delay_mean_ns"), "24900.00");
        assert_eq!(string(&r, "delay_std_ns"), "0.00");
        assert_eq!(
            (uint(&r, "delay_min_ns"), uint(&r, "delay_max_ns")),
            (24_900, 24_900)
        );
        assert_eq!(string(&r, "lateness_mean_ns"), "19900.00");
        assert_eq!(uint(&r, "lateness_max_ns"), 19_900);
    }

    #[test]
    fn every_packet_carries_its_own_arrival_to_the_tx() {
        // four bursts of one packet arriving at 0, 992, 1984, 2976 (delay 5e9: nothing due before
        // the last iteration), now 6e9, tx 1e10 + 2976: realized delays 1e10 + {2976, 1984, 992,
        // 0}, population std 1109.09 around 1e10 + 1488 (exact for a huge mean, tiny spread).
        let stats: &'static Stats = Box::leak(Box::default());
        drive_bursts(
            config(100, 5e9, 0.0),
            &[&[100], &[100], &[100], &[100]],
            &[0.0, 992.0, 1984.0, 2976.0, 6e9, 1e10 + 2976.0],
            stats,
            usize::MAX,
        );
        let r = stats.stats_reply(0);
        assert_eq!(string(&r, "delay_mean_ns"), "10000001488.00");
        assert_eq!(string(&r, "delay_std_ns"), "1109.09");
        assert_eq!(uint(&r, "delay_min_ns"), 10_000_000_000);
        assert_eq!(uint(&r, "delay_max_ns"), 10_000_002_976);
        // lateness: send times 5e9 + arrival; tx - send = 5e9 + 2976 - arrival
        assert_eq!(string(&r, "lateness_mean_ns"), "5000001488.00");
        assert_eq!(uint(&r, "lateness_max_ns"), 5_000_002_976);
    }

    #[test]
    fn only_accepted_packets_enter_the_delay_statistics() {
        // the "mixed" scenario of causes.rs, reduced: one burst at now = 0 of 5 packets of 1519
        // bytes (12344 bit each), capacity 4: P1 lost (accounted), P2 (send time 1_012_344) and
        // P3 (1_024_688) accepted, P4, P5 limiter drops. Popped at 5e6, tx 6e6: both packets have
        // the delay 6e6; lateness 4_987_656 and 4_975_312 (mean 4_981_484 over the 2 packets).
        let cfg = mixed_limiter_cfg();
        let stats: &'static Stats = Box::leak(Box::default());
        drive(cfg, &[1519; 5], &[0.0, 5_000_000.0, 6_000_000.0], stats);
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "dpds_rejects"), 3);
        assert_eq!(string(&r, "delay_mean_ns"), "6000000.00");
        assert_eq!(string(&r, "delay_std_ns"), "0.00");
        assert_eq!(uint(&r, "delay_min_ns"), 6_000_000);
        assert_eq!(uint(&r, "delay_max_ns"), 6_000_000);
        assert_eq!(string(&r, "lateness_mean_ns"), "4981484.00");
        assert_eq!(uint(&r, "lateness_max_ns"), 4_987_656);
        let h = stats.delay_hist_reply();
        let total: u64 =
            bins(&h).iter().sum::<u64>() + uint(&h, "underflow") + uint(&h, "overflow");
        assert_eq!(total, 2, "the histogram counts accepted packets only");
    }

    #[test]
    fn only_the_packets_the_nic_accepted_are_recorded_and_the_mean_uses_that_count() {
        // the first scenario, the NIC takes 4 of the 6 staged packets: delay and lateness of
        // the first 4 only: lateness 29_200, 29_000, 28_808, 28_576 -> mean 28_896 (a mean over
        // the 6 popped packets would be different), max 29_200.
        let stats: &'static Stats = Box::leak(Box::default());
        drive_bursts(
            spacer_cfg(800.0),
            &[&[1, 0, 5, 0, 0, 0]],
            &[0.0, 20_000.0, 30_000.0],
            stats,
            4,
        );
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "tx_sent"), 4);
        assert_eq!(uint(&r, "tx_drops"), 2);
        assert_eq!(string(&r, "delay_mean_ns"), "30000.00");
        assert_eq!(string(&r, "lateness_mean_ns"), "28896.00");
        assert_eq!(uint(&r, "lateness_max_ns"), 29_200);
        let h = stats.delay_hist_reply();
        let total: u64 =
            bins(&h).iter().sum::<u64>() + uint(&h, "underflow") + uint(&h, "overflow");
        assert_eq!(total, 4, "the freed tail is not in the histogram");
    }

    #[test]
    fn a_tx_that_takes_nothing_records_nothing() {
        let stats: &'static Stats = Box::leak(Box::default());
        drive_bursts(
            config(100, 0.0, 0.0),
            &[&[64; 5]],
            &[0.0, 20_000.0, 30_000.0],
            stats,
            0,
        );
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "tx_sent"), 0);
        for k in ["delay_mean_ns", "delay_std_ns", "lateness_mean_ns"] {
            assert_eq!(string(&r, k), "0.00", "{k}");
        }
        for k in ["delay_min_ns", "delay_max_ns", "lateness_max_ns"] {
            assert_eq!(uint(&r, k), 0, "{k}");
        }
        let h = stats.delay_hist_reply();
        assert_eq!(
            bins(&h).iter().sum::<u64>() + uint(&h, "underflow") + uint(&h, "overflow"),
            0
        );
    }

    #[test]
    fn delay_and_lateness_are_clamped_at_zero() {
        // arrival 1000, send time 1000, but the tx read is 500 (a clock that went back):
        // both would be -500, both are recorded as 0 (in the histogram too).
        let stats: &'static Stats = Box::leak(Box::default());
        drive(
            config(100, 0.0, 0.0),
            &[64; 3],
            &[1000.0, 20_000.0, 500.0],
            stats,
        );
        let r = stats.stats_reply(0);
        assert_eq!(string(&r, "delay_mean_ns"), "0.00");
        assert_eq!((uint(&r, "delay_min_ns"), uint(&r, "delay_max_ns")), (0, 0));
        assert_eq!(string(&r, "lateness_mean_ns"), "0.00");
        assert_eq!(uint(&r, "lateness_max_ns"), 0);
        assert_eq!(bins(&stats.delay_hist_reply())[0], 3, "delay 0 is bin 0");
    }

    #[test]
    fn the_iat_histogram_is_still_recorded_at_pop_for_packets_the_nic_refused() {
        // 6 packets, the NIC takes none: the 5 gaps between the model send times are still in
        // the IAT histogram (recorded at pop, not at the send)
        let cfg = spacer_cfg(800.0);
        let stats: &'static Stats = Box::leak(Box::new(Stats::with_hists(
            HistRange::default(),
            HistRange::iat_from_config(&cfg),
            true,
        )));
        drive_bursts(
            cfg,
            &[&[1, 0, 5, 0, 0, 0]],
            &[0.0, 20_000.0, 30_000.0],
            stats,
            0,
        );
        let h = stats.iat_hist_reply();
        let total: u64 =
            bins(&h).iter().sum::<u64>() + uint(&h, "underflow") + uint(&h, "overflow");
        assert_eq!(total, 5);
    }

    #[test]
    fn a_run_without_accepted_packets_keeps_the_stats_at_zero() {
        let stats: &'static Stats = Box::leak(Box::default());
        drive(config(100, 1000.0, 1.0), &[64; 10], &[0.0, 1.0], stats);
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "loss_drops"), 10);
        for k in ["delay_mean_ns", "delay_std_ns", "lateness_mean_ns"] {
            assert_eq!(string(&r, k), "0.00", "{k}");
        }
        for k in ["delay_min_ns", "delay_max_ns", "lateness_max_ns"] {
            assert_eq!(uint(&r, k), 0, "{k}");
        }
    }

    #[test]
    fn histogram_places_the_realized_delay_incl_underflow_and_overflow() {
        // range [20_000, 20_256) in 1 ns bins; six single-packet bursts arriving at 20_000,
        // 20_192, 20_384, 20_616, 20_808, 21_008 (zero delay, all within one wait of each
        // other), now = tx = 40_808: realized delays 20_808, 20_616, 20_424, 20_192, 20_000,
        // 19_800: underflow 1, bin 0, bin 192, overflow 3.
        let stats: &'static Stats =
            Box::leak(Box::new(Stats::with_range(HistRange::new(20_000.0, 1.0))));
        drive_bursts(
            config(100, 0.0, 0.0),
            &[&[64], &[64], &[64], &[64], &[64], &[64]],
            &[
                20_000.0, 20_192.0, 20_384.0, 20_616.0, 20_808.0, 21_008.0, 40_808.0,
            ],
            stats,
            usize::MAX,
        );
        let h = stats.delay_hist_reply();
        let b = bins(&h);
        assert_eq!(b.len(), 256);
        assert_eq!(uint(&h, "underflow"), 1);
        assert_eq!(uint(&h, "overflow"), 3);
        assert_eq!(b[0], 1, "delay 20_000 is the lower edge of bin 0");
        assert_eq!(b[192], 1, "delay 20_192");
        assert_eq!(b.iter().sum::<u64>(), 2);
        assert_eq!(num(&h, "lo_ns"), 20_000.0);
        assert_eq!(num(&h, "bin_ns"), 1.0);
    }

    #[test]
    fn a_custom_number_of_bins_changes_the_reply_length_and_the_placement() {
        // as above in [20_000, 20_200): 50 bins of 4 ns: underflow 1, bin 0, bin 48, overflow 3
        let stats: &'static Stats = Box::leak(Box::new(Stats::with_range(wb(20_000.0, 4.0, 50))));
        drive_bursts(
            config(100, 0.0, 0.0),
            &[&[64], &[64], &[64], &[64], &[64], &[64]],
            &[
                20_000.0, 20_192.0, 20_384.0, 20_616.0, 20_808.0, 21_008.0, 40_808.0,
            ],
            stats,
            usize::MAX,
        );
        let h = stats.delay_hist_reply();
        let b = bins(&h);
        assert_eq!(b.len(), 50);
        assert_eq!(uint(&h, "num_bins"), 50);
        assert_eq!((uint(&h, "underflow"), uint(&h, "overflow")), (1, 3));
        assert_eq!((b[0], b[48]), (1, 1));
        assert_eq!(b.iter().sum::<u64>(), 2);
        assert_eq!(num(&h, "lo_ns"), 20_000.0);
        assert_eq!(num(&h, "bin_ns"), 4.0);
    }

    #[test]
    fn a_fresh_custom_histogram_is_an_all_zero_array_of_its_own_length() {
        for n in [1usize, 100, 500] {
            let s = Stats::with_range(wb(0.0, 1.0, n));
            let h = s.delay_hist_reply();
            assert_eq!(bins(&h), vec![0u64; n]);
            assert_eq!(uint(&h, "num_bins"), n as u64);
        }
    }

    #[test]
    fn the_config_derived_range_contains_the_earliest_realized_delay() {
        // delay 2 ms: the range (spec 57 T9) is [2e6, 2e6 + 2 * TX_MAX_WAIT_NS), 20 us wide. The
        // earliest realized delay is one wait above the intended one (arrival 100, send time
        // 2_000_100, now = tx = 2_010_100): delay 2_010_000, in the middle of the window.
        let cfg = config(100, 2_000_000.0, 0.0);
        let range = HistRange::from_config(&cfg);
        let stats: &'static Stats = Box::leak(Box::new(Stats::with_range(range)));
        drive(cfg, &[64; 7], &[100.0, 2_010_100.0, 2_010_100.0], stats);
        let h = stats.delay_hist_reply();
        assert_eq!(num(&h, "lo_ns"), 2_000_000.0);
        assert_eq!(range.hi_ns(), 2_000_000.0 + 2.0 * TX_MAX_WAIT_NS);
        // exactly the window's middle: bin 127 or 128 depending on how the reciprocal rounds
        assert!(matches!(range.bin(2_010_000.0), Bin::Index(127 | 128)));
        assert_eq!((uint(&h, "underflow"), uint(&h, "overflow")), (0, 0));
        assert_all_in_bin(&h, range, 2_010_000.0, 7);
    }

    #[test]
    fn the_limiter_shifted_packets_are_binned_by_the_realized_delay() {
        // the mixed scenario: P2 (send 1_012_344) and P3 (1_024_688) arrive at 0; the range is
        // [1e6, 1e6 + 49_344 + 2 * TX_MAX_WAIT_NS); now 1_030_000 (>= W after the oldest send
        // time), tx 1_030_500: both have the delay 1_030_500, inside the window
        let cfg = mixed_limiter_cfg();
        let range = HistRange::from_config(&cfg);
        let stats: &'static Stats = Box::leak(Box::new(Stats::with_range(range)));
        drive(cfg, &[1519; 5], &[0.0, 1_030_000.0, 1_030_500.0], stats);
        let h = stats.delay_hist_reply();
        assert_eq!(num(&h, "lo_ns"), 1_000_000.0);
        assert_eq!(range.hi_ns(), 1_000_000.0 + 49_344.0 + 2.0 * TX_MAX_WAIT_NS);
        assert!(matches!(range.bin(1_030_500.0), Bin::Index(_)));
        assert_all_in_bin(&h, range, 1_030_500.0, 2);
    }

    #[test]
    fn the_histogram_reply_has_exactly_the_documented_keys() {
        let h = Stats::default().delay_hist_reply();
        let keys: Vec<&str> = h.iter().map(|(k, _)| k.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            [
                "bin_ns",
                "bins",
                "first_bin",
                "lo_ns",
                "num_bins",
                "overflow",
                "page",
                "underflow"
            ]
        );
    }

    #[test]
    fn direction_hist_replies_are_per_direction_and_reject_bad_params() {
        let r0 = HistRange::new(0.0, 1.0);
        let r1 = wb(5000.0, 2.0, 100);
        let ds = DirectionStats::with_ranges(vec![r0, r1]);
        assert_eq!(num(&ds.delay_hist_reply("0").unwrap(), "bin_ns"), 1.0);
        assert_eq!(num(&ds.delay_hist_reply("1").unwrap(), "lo_ns"), 5000.0);
        assert_eq!(bins(&ds.delay_hist_reply("0").unwrap()).len(), 256);
        assert_eq!(bins(&ds.delay_hist_reply("1").unwrap()).len(), 100);
        for bad in ["2", "", "x", "-1", "+1", "0,1", " 0", "0x1"] {
            assert!(ds.delay_hist_reply(bad).is_none(), "{bad:?}");
        }
        // new(n) uses the default range
        let ds = DirectionStats::new(1);
        assert_eq!(num(&ds.delay_hist_reply("0").unwrap(), "bin_ns"), 3.90625);
        assert!(ds.delay_hist_reply("1").is_none());
    }

    // ── D5: forward::run with the switch off ─────────────────────────

    fn keys_present(r: &Reply, keys: &[&str]) -> Vec<bool> {
        keys.iter()
            .map(|k| r.iter().any(|(key, _)| key == k))
            .collect()
    }

    #[test]
    fn a_direction_with_delay_stats_off_records_nothing_but_counts_everything() {
        // capacity 3 and delay 1 ms: of 5 packets 3 are accepted, 2 are refused (buffer
        // full); all 3 are popped at 1.015 ms (15 us past their send time) and sent in that
        // iteration (the switch-on instance reads the clock once more: still 1.015 ms, the
        // last value repeats: realized delay 1_015_000, inside the automatic window)
        let cfg = || config(3, 1_000_000.0, 0.0);
        let range = HistRange::from_config(&cfg());
        let off: &'static Stats = Box::leak(Box::new(Stats::with_delay_stats(range, false)));
        let on: &'static Stats = Box::leak(Box::new(Stats::with_range(range)));
        for stats in [off, on] {
            drive(cfg(), &[100; 5], &[0.0, 1_015_000.0], stats);
        }

        let (r_off, r_on) = (off.stats_reply(0), on.stats_reply(0));
        // control: the same traffic with the switch on has delay statistics
        assert_eq!(string(&r_on, "delay_mean_ns"), "1015000.00");
        assert_eq!(uint(&r_on, "lateness_max_ns"), 15_000);
        let h_on = on.delay_hist_reply();
        assert_eq!(bins(&h_on).iter().sum::<u64>(), 3);

        // off: no delay key, no histogram entry ...
        assert_eq!(keys_present(&r_off, &DELAY_KEYS), [false; 6], "{r_off:?}");
        assert_empty_hist(&off.delay_hist_reply());
        // ... and every counter and drop cause as with the switch on
        for k in [
            "rx_pkts",
            "dpds_rejects",
            "loss_drops",
            "limiter_drops",
            "buffer_full_drops",
            "pops",
            "tx_pkts",
            "tx_sent",
            "tx_drops",
            "buffer_len_max",
        ] {
            assert_eq!(uint(&r_off, k), uint(&r_on, k), "{k}");
        }
        assert_eq!(uint(&r_off, "rx_pkts"), 5);
        assert_eq!(uint(&r_off, "buffer_full_drops"), 2);
        assert_eq!(uint(&r_off, "tx_sent"), 3);
        assert_eq!(string(&r_off, "first_loss_reason"), "DpdsReject");
    }

    #[test]
    fn switching_off_changes_no_other_reply_value() {
        // the same traffic on and off: the two replies differ by the six keys only
        let cfg = || config(100, 1_000.0, 0.0);
        let range = HistRange::from_config(&cfg());
        let off: &'static Stats = Box::leak(Box::new(Stats::with_delay_stats(range, false)));
        let on: &'static Stats = Box::leak(Box::new(Stats::with_range(range)));
        for stats in [off, on] {
            drive(cfg(), &[64; 9], &[0.0, 20_000.0], stats);
        }
        let strip = |r: Reply| -> Reply {
            r.into_iter()
                .filter(|(k, _)| !DELAY_KEYS.contains(&k.as_str()))
                .collect()
        };
        assert_eq!(strip(off.stats_reply(5)), strip(on.stats_reply(5)));
        assert_eq!(off.epochs_reply(), on.epochs_reply());
    }
}

#[test]
fn the_range_types_exist_in_every_build_and_stats_can_be_built_with_one() {
    // Without the feature nothing is recorded, but the API stays callable.
    let s = dpds::stats::Stats::with_range(HistRange::new(0.0, 1.0));
    let _ = s.delay_hist_reply();
    let ds = dpds::stats::DirectionStats::with_ranges(vec![HistRange::default(); 2]);
    assert_eq!(ds.stats.len(), 2);
}

// ── D4: zero bins ───────────────────────────────────────────────────

#[test]
fn zero_bins_are_rejected_by_with_bins() {
    let e = HistRange::with_bins(0.0, 1.0, 0).unwrap_err();
    assert!(
        e.to_string().contains("bins"),
        "the message names the field: {e}"
    );
    fn assert_error<E: std::error::Error + std::fmt::Debug>(_: &E) {}
    assert_error(&e);
    // every other count in 1..=MAX_BINS is fine and keeps its fields
    for n in [1usize, 2, HIST_BINS, MAX_BINS] {
        let r = HistRange::with_bins(10.0, 2.0, n).unwrap();
        assert_eq!((r.lo_ns, r.bin_ns, r.bins), (10.0, 2.0, n));
    }
}

#[test]
fn a_range_from_a_zero_bins_spec_is_still_an_error() {
    // the spec route (links file) already refused it; it must stay that way
    let e = HistRange::from_spec(&cfg(1e6, 0.0, None), &spec(Some(0), None, None)).unwrap_err();
    assert!(e.to_string().contains("bins"), "{e}");
}

// ── D5: delay statistics off ────────────────────────────────────────

const DELAY_KEYS: [&str; 6] = [
    "delay_mean_ns",
    "delay_std_ns",
    "delay_min_ns",
    "delay_max_ns",
    "lateness_mean_ns",
    "lateness_max_ns",
];

fn key_list(r: &Reply) -> Vec<&str> {
    r.iter().map(|(k, _)| k.as_str()).collect()
}

fn has_key(r: &Reply, key: &str) -> bool {
    r.iter().any(|(k, _)| k == key)
}

/// No bins, whatever the exact shape of "empty" (see the header).
fn assert_empty_hist(r: &Reply) {
    for (k, v) in r {
        match (k.as_str(), v) {
            ("bins", Value::UintArray(b)) => assert!(b.is_empty(), "bins must be empty: {r:?}"),
            ("bins", other) => panic!("bins: expected an array, got {other:?}"),
            ("num_bins" | "underflow" | "overflow", Value::Uint(n)) => {
                assert_eq!(*n, 0, "{k} must be 0 or absent: {r:?}")
            }
            _ => {}
        }
    }
}

#[test]
fn delay_stats_off_drops_exactly_the_six_delay_keys_from_the_stats_reply() {
    let range = HistRange::default();
    let on = Stats::with_range(range).stats_reply(123);
    let off = Stats::with_delay_stats(range, false).stats_reply(123);
    assert_eq!(on.len(), 28, "the documented stats reply");
    for k in DELAY_KEYS {
        assert!(has_key(&on, k), "{k} present when on");
        assert!(!has_key(&off, k), "{k} must be absent when off: {off:?}");
    }
    assert_eq!(off.len(), 22);
    let expected: Vec<&str> = key_list(&on)
        .into_iter()
        .filter(|k| !DELAY_KEYS.contains(k))
        .collect();
    assert_eq!(key_list(&off), expected, "same keys, same order, minus six");
}

#[test]
fn delay_stats_on_is_what_with_range_builds() {
    let range = HistRange::default();
    let a = Stats::with_delay_stats(range, true);
    let b = Stats::with_range(range);
    assert_eq!(a.stats_reply(7), b.stats_reply(7));
    assert_eq!(a.delay_hist_reply(), b.delay_hist_reply());
    assert_eq!(Stats::default().stats_reply(7), b.stats_reply(7));
}

#[test]
fn delay_stats_off_has_an_empty_histogram_reply() {
    let off = Stats::with_delay_stats(HistRange::default(), false);
    assert_empty_hist(&off.delay_hist_reply());
    // while the switched-on twin still has its 256 zero bins
    let on = Stats::with_range(HistRange::default()).delay_hist_reply();
    assert!(on
        .iter()
        .any(|(k, v)| k == "bins" && *v == Value::UintArray(vec![0; 256])));
}

#[test]
fn the_switch_is_per_direction() {
    let r = HistRange::new(0.0, 1.0);
    let ds = DirectionStats::with_delay_settings(vec![(r, true), (r, false), (r, true)]);
    assert_eq!(ds.stats.len(), 3);
    for (d, enabled) in [(0, true), (1, false), (2, true)] {
        let param = d.to_string();
        let stats = ds.stats_reply(&param, 0).unwrap();
        for k in DELAY_KEYS {
            assert_eq!(has_key(&stats, k), enabled, "dir {d} {k}");
        }
        let hist = ds.delay_hist_reply(&param).expect("an existing direction");
        if enabled {
            assert!(hist.iter().any(|(k, _)| k == "bins"));
            assert!(hist
                .iter()
                .any(|(k, v)| k == "bins" && *v == Value::UintArray(vec![0; 256])));
        } else {
            assert_empty_hist(&hist);
        }
    }
    // counters and the epochs reply do not care
    assert!(ds.epochs_reply("1").is_some());
    for bad in ["3", "", "x", "-1"] {
        assert!(ds.delay_hist_reply(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn the_ranges_of_with_delay_settings_are_kept_for_the_enabled_directions() {
    let ds = DirectionStats::with_delay_settings(vec![
        (wb(5000.0, 2.0, 100), true),
        (wb(7.0, 1.0, 10), false),
    ]);
    let h = ds.delay_hist_reply("0").unwrap();
    assert!(h
        .iter()
        .any(|(k, v)| k == "bins" && *v == Value::UintArray(vec![0; 100])));
    assert_eq!(ds.stats[0].delay.range(), wb(5000.0, 2.0, 100));
}
