//! `count`, `percentiles` and `expected` on `GET /api/stats/{dir}/delay_histogram` (`window_s` is
//! in `histogram_window.rs`).
//!
//! - `count`: samples of the returned histogram including under- and overflow.
//! - `percentiles: {p50, p90, p99, p999}` in ns, linear interpolation inside the bin: with rank
//!   `r = q * count` over the order underflow, bin 0 .. bin n-1, overflow, the percentile lies in
//!   the first bin whose cumulative count reaches `r`, at
//!   `edge_lo + (r - cumulative_before) / bin_count * bin_ns`. A rank inside the underflow gives
//!   `lo_ns`, inside the overflow the upper edge `lo_ns + n * bin_ns`; no samples give `null`. The
//!   ranks used below never fall exactly on a bin boundary. The clamp flag of `pXX` is the sibling
//!   key `pXX_clamped` inside `percentiles` (`"underflow"`, `"overflow"`, absent or null).
//! - `expected: [f64; num_bins]`, `expected_underflow`, `expected_overflow`: expected counts for
//!   `count` samples under the direction's delay distribution. `dpds` clamps negative delays to 0,
//!   so mass below 0 sits in the bin containing 0 (with `lo_ns = 0`: bin 0, underflow 0) or, when
//!   `lo_ns > 0`, in the underflow. `expected_model` is `"exact"` or, with `activate_correlation`,
//!   `"normal_approximation"`. With an active limiter `expected_note == "excludes limiter queueing
//!   delay"`, otherwise absent or null.
//! - Direction `d` uses the config of link `d` of the start body: `a-to-b` is direction 0,
//!   `b-to-a` direction 1. A missing delayer is a Deterministic 0 ns delay.
//!
//! Every expected number below was computed with Python's `math` (erf for Normal and LogNormal,
//! the closed-form Erlang CDF for Gamma with integer shape, the linear CDF for Uniform), not with
//! the controller code.

use crate::common::*;
use dpds_controller::stats::Snapshot;
use serde_json::{json, Value};

pub const TOTAL: u64 = 10_000;

/// A controller whose emulation was started with the links `a-to-b` (direction 0, config `c0`)
/// and `b-to-a` (direction 1, config `c1`): the expected counts follow the configuration the
/// emulation was started with.
pub fn app_with(c0: Value, c1: Value) -> TestApp {
    let a = TestApp::new(FakeHost::bare_metal());
    let body = setup(vec![link_cfg(NIC0, NIC1, c0), link_cfg(NIC1, NIC0, c1)]).body();
    let router = a.router.clone();
    let r =
        block_on(async move { call(&router, "POST", "/api/emulation/start", Some(body)).await });
    assert_eq!(r.status, 202, "{}", r.text());
    a
}

/// A direction config with the given distribution, mean and std (limiter off).
pub fn cfg(dist: &str, mean_ns: f64, std_ns: f64) -> Value {
    let mut c = cfg_json(mean_ns, std_ns);
    c["delayer"]["distribution"] = json!(dist);
    c
}

/// A `/dpds/delay_hist` reply like the real one (numbers as decimal strings).
pub fn hist(lo: f64, bin: f64, bins: &[u64], under: u64, over: u64) -> Value {
    json!({"lo_ns": lo.to_string(), "bin_ns": bin.to_string(), "num_bins": bins.len(),
           "bins": bins, "underflow": under, "overflow": over})
}

/// `n` bins with 100 under- and 100 overflow samples, `total` samples altogether
/// (the remainder goes into bin 0).
pub fn hist_total(lo: f64, bin: f64, n: usize, total: u64) -> Value {
    let rest = total - 200;
    let mut bins = vec![rest / n as u64; n];
    bins[0] += rest - (rest / n as u64) * n as u64;
    hist(lo, bin, &bins, 100, 100)
}

fn zero_dir() -> DirSample {
    DirSample {
        uptime_ns: 1,
        rx_pkts: 0,
        tx_sent: 0,
        loss: 0,
        limiter: 0,
        full: 0,
        tx_drops: 0,
    }
}

fn zero_port() -> PortSample {
    PortSample {
        ibytes: 0,
        obytes: 0,
        rx_missed_errors: 0,
        rx_mbuf_allocation_errors: 0,
    }
}

/// Telemetry of the two links `a-to-b` and `b-to-a` with the given histograms.
pub fn snap(h0: Value, h1: Value) -> Snapshot {
    let mut s = snapshot([zero_dir(), zero_dir()], [zero_port(), zero_port()]);
    set_hists(&mut s, vec![h0, h1]);
    s
}

pub async fn get_hist(a: &TestApp, dir: u32) -> Value {
    let r = a.get(&format!("/api/stats/{dir}/delay_histogram")).await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json()
}

pub fn num_of(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

pub fn assert_near(actual: f64, want: f64, tol: f64, what: &str) {
    assert!(
        (actual - want).abs() <= tol,
        "{what}: expected {want}, got {actual} (tolerance {tol})"
    );
}

// ---- percentiles -----------------------------------------------------------------------------

async fn percentiles_of(h: Value) -> Value {
    let a = app_with(
        cfg("NormalDistribution", 1.0e6, 1.0e5),
        cfg_json(1.0e6, 1.0e5),
    );
    a.telemetry.set(None, snap(h, hist(0.0, 10.0, &[1], 0, 0)));
    get_hist(&a, 0).await
}

fn check_pcts(v: &Value, want: [f64; 4]) {
    for (key, w) in ["p50", "p90", "p99", "p999"].iter().zip(want) {
        assert_near(
            num_of(&v["percentiles"][*key]),
            w,
            1e-6 * w.abs().max(1.0),
            key,
        );
        let flag = &v["percentiles"][format!("{key}_clamped")];
        assert!(flag.is_null(), "{key} is not clamped but has {flag}");
    }
}

#[tokio::test]
async fn percentiles_interpolate_linearly_inside_the_bin() {
    // bins [10, 20, 30, 40] of width 10 from 0: ranks 50, 90, 99, 99.9
    let v = percentiles_of(hist(0.0, 10.0, &[10, 20, 30, 40], 0, 0)).await;
    assert_eq!(v["count"], 100);
    check_pcts(&v, [26.666_666_666_666_67, 37.5, 39.75, 39.975]);
}

#[tokio::test]
async fn the_range_start_and_bin_width_are_taken_into_account() {
    // same counts, lo = 1000 ns, bin 50 ns
    let v = percentiles_of(hist(1000.0, 50.0, &[10, 20, 30, 40], 0, 0)).await;
    check_pcts(
        &v,
        [
            1000.0 + 2.0 * 50.0 + 50.0 * 20.0 / 30.0,
            1000.0 + 3.0 * 50.0 + 50.0 * 0.75,
            1000.0 + 3.0 * 50.0 + 50.0 * 0.975,
            1000.0 + 3.0 * 50.0 + 50.0 * 0.9975,
        ],
    );
}

#[tokio::test]
async fn a_percentile_in_the_overflow_is_clamped_to_the_upper_edge_and_flagged() {
    // 40 samples in the bins, 60 above the range: every percentile of interest is in the overflow
    let v = percentiles_of(hist(0.0, 10.0, &[10, 10, 10, 10], 0, 60)).await;
    assert_eq!(v["count"], 100);
    for key in ["p50", "p90", "p99", "p999"] {
        assert_near(num_of(&v["percentiles"][key]), 40.0, 1e-9, key);
        assert_eq!(
            v["percentiles"][format!("{key}_clamped")],
            "overflow",
            "{key}: {v}"
        );
    }
}

#[tokio::test]
async fn a_percentile_in_the_underflow_is_clamped_to_the_lower_edge_and_flagged() {
    // lo = 100, bins [25, 25], underflow 150, count 200: p50 (rank 100) is below the range;
    // p90 (rank 180) is in bin 1: 110 + 10 * 5 / 25 = 112; p99: 119.2; p999: 119.92
    let v = percentiles_of(hist(100.0, 10.0, &[25, 25], 150, 0)).await;
    assert_eq!(v["count"], 200);
    assert_near(num_of(&v["percentiles"]["p50"]), 100.0, 1e-9, "p50");
    assert_eq!(v["percentiles"]["p50_clamped"], "underflow", "{v}");
    for (key, w) in [("p90", 112.0), ("p99", 119.2), ("p999", 119.92)] {
        assert_near(num_of(&v["percentiles"][key]), w, 1e-6, key);
        assert!(
            v["percentiles"][format!("{key}_clamped")].is_null(),
            "{key}: {v}"
        );
    }
}

#[tokio::test]
async fn underflow_counts_towards_the_rank() {
    // underflow 20 + bins [20; 4], count 100: p50 rank 50 -> bin 1 (10..20): 10 + 10 * 10 / 20 = 15
    let v = percentiles_of(hist(0.0, 10.0, &[20, 20, 20, 20], 20, 0)).await;
    check_pcts(&v, [15.0, 35.0, 39.5, 39.95]);
}

#[tokio::test]
async fn empty_bins_between_the_samples_are_skipped() {
    // [90, 0, 0, 10], count 100, bin 100 wide: p999 rank 99.9 -> bin 3: 300 + 100 * 9.9 / 10
    let v = percentiles_of(hist(0.0, 100.0, &[90, 0, 0, 10], 0, 0)).await;
    check_pcts(
        &v,
        [
            0.0 + 100.0 * 50.0 / 90.0,
            100.0 * 90.0 / 90.0,
            300.0 + 100.0 * 0.9,
            300.0 + 100.0 * 0.99,
        ],
    );
}

#[tokio::test]
async fn without_samples_the_percentiles_are_null_and_the_count_is_zero() {
    let v = percentiles_of(hist(0.0, 10.0, &[0, 0, 0, 0], 0, 0)).await;
    assert_eq!(v["count"], 0);
    for key in ["p50", "p90", "p99", "p999"] {
        assert!(v["percentiles"][key].is_null(), "{key}: {v}");
        assert!(
            v["percentiles"][format!("{key}_clamped")].is_null(),
            "{key}: {v}"
        );
    }
}

#[tokio::test]
async fn the_count_includes_underflow_and_overflow() {
    let v = percentiles_of(hist(0.0, 10.0, &[1, 2, 3, 4], 5, 6)).await;
    assert_eq!(v["count"], 21);
}

#[tokio::test]
async fn the_percentiles_object_has_exactly_the_agreed_keys() {
    // contract shared with the frontend: p50, p90, p99, p999 (ns or null) and, per percentile,
    // `<key>_clamped` = "underflow" | "overflow" | null (null or absent = not clamped)
    let allowed = [
        "p50",
        "p90",
        "p99",
        "p999",
        "p50_clamped",
        "p90_clamped",
        "p99_clamped",
        "p999_clamped",
    ];
    for h in [
        hist(0.0, 10.0, &[10, 20, 30, 40], 0, 0),
        hist(0.0, 10.0, &[10, 10, 10, 10], 0, 60),
        hist(100.0, 10.0, &[25, 25], 150, 0),
        hist(0.0, 10.0, &[0, 0], 0, 0),
    ] {
        let v = percentiles_of(h).await;
        let p = v["percentiles"]
            .as_object()
            .unwrap_or_else(|| panic!("{v}"));
        for k in ["p50", "p90", "p99", "p999"] {
            assert!(p.contains_key(k), "{k} missing: {v}");
        }
        for k in p.keys() {
            assert!(allowed.contains(&k.as_str()), "unexpected key {k}: {v}");
        }
        for k in ["p50", "p90", "p99", "p999"] {
            let c = &p
                .get(&format!("{k}_clamped"))
                .cloned()
                .unwrap_or(Value::Null);
            assert!(
                c.is_null() || c == "underflow" || c == "overflow",
                "{k}_clamped: {c}"
            );
            assert!(p[k].is_null() || p[k].is_number(), "{k}: {}", p[k]);
        }
    }
}

#[tokio::test]
async fn the_existing_histogram_fields_are_unchanged() {
    let v = percentiles_of(hist(100.0, 10.0, &[3, 4], 1, 2)).await;
    assert_eq!(v["dir"], 0);
    assert_eq!(v["bins"], json!([3, 4]));
    assert_eq!(v["underflow"], 1);
    assert_eq!(v["overflow"], 2);
    assert_eq!(v["edges_ns"], json!([100.0, 110.0, 120.0]));
    assert_close(f(&v, "lo_ns"), 100.0);
    assert_close(f(&v, "bin_ns"), 10.0);
    assert!(v.get("age_ms").is_some() && v.get("stale").is_some());
}

#[tokio::test]
async fn the_window_fields_without_the_parameter_mean_since_start() {
    let v = percentiles_of(hist(0.0, 10.0, &[1, 2], 0, 0)).await;
    assert_eq!(v["window_s"], 0, "{v}");
    assert!(v["span_ms"].is_number(), "{v}");
}

// ---- expected: one direction at a time -----------------------------------------------------------

/// Checks `expected`, `expected_underflow`, `expected_overflow` against `(bins, under, over)`
/// (expected counts for `TOTAL` samples) and that they add up to the sample count.
fn check_expected(v: &Value, bins: &[f64], under: f64, over: f64, model: &str) {
    let got = v["expected"]
        .as_array()
        .unwrap_or_else(|| panic!("no expected array: {v}"));
    assert_eq!(got.len(), bins.len(), "one expected value per bin: {v}");
    let tol = 0.05; // counts, out of 10000
    for (i, (g, w)) in got.iter().zip(bins).enumerate() {
        assert_near(num_of(g), *w, tol, &format!("expected[{i}]"));
    }
    assert_near(
        num_of(&v["expected_underflow"]),
        under,
        tol,
        "expected_underflow",
    );
    assert_near(
        num_of(&v["expected_overflow"]),
        over,
        tol,
        "expected_overflow",
    );
    let sum: f64 = got.iter().map(num_of).sum::<f64>()
        + num_of(&v["expected_underflow"])
        + num_of(&v["expected_overflow"]);
    assert_near(
        sum,
        TOTAL as f64,
        1e-3,
        "sum of the expected counts vs the sample count",
    );
    assert_eq!(v["count"], TOTAL);
    assert_eq!(v["expected_model"], model, "{v}");
    assert!(absent(v, "expected_note"), "no limiter: {v}");
}

async fn expected_for(c0: Value, h0: Value) -> Value {
    let a = app_with(c0, cfg_json(1.0e6, 1.0e5));
    a.telemetry.set(None, snap(h0, hist(0.0, 10.0, &[1], 0, 0)));
    get_hist(&a, 0).await
}

#[tokio::test]
async fn normal_with_the_negative_mass_clamped_into_the_bin_of_zero() {
    // Normal(1000, 1000), lo 0, 8 bins of 500: P(X<0) = Phi(-1) = 0.158655 sits in bin 0,
    // which therefore gets Phi(-0.5) = 0.308538; underflow is 0 (nothing is below 0 after clamping)
    let v = expected_for(
        cfg("NormalDistribution", 1000.0, 1000.0),
        hist_total(0.0, 500.0, 8, TOTAL),
    )
    .await;
    check_expected(
        &v,
        &[
            3_085.375_387_260,
            1_914.624_612_740,
            1_914.624_612_740,
            1_498.822_847_945,
            918.480_526_626,
            440.570_693_207,
            165.404_666_224,
            48.597_672_941,
        ],
        0.0,
        13.498_980_316,
        "exact",
    );
}

#[tokio::test]
async fn normal_symmetric_around_the_mean_with_two_sigma_tails() {
    // Normal(1000, 200) over 600..1400: both tails are Phi(-2) = 0.0227501319
    let v = expected_for(
        cfg("NormalDistribution", 1000.0, 200.0),
        hist_total(600.0, 100.0, 8, TOTAL),
    )
    .await;
    check_expected(
        &v,
        &[
            440.570_693_207,
            918.480_526_626,
            1_498.822_847_945,
            1_914.624_612_740,
            1_914.624_612_740,
            1_498.822_847_945,
            918.480_526_626,
            440.570_693_207,
        ],
        227.501_319_482,
        227.501_319_482,
        "exact",
    );
}

#[tokio::test]
async fn negative_mass_goes_to_the_underflow_when_the_range_starts_above_zero() {
    // Normal(100, 200), lo 50: underflow = P(X' < 50) = Phi(-0.25) = 0.401294 (includes X < 0)
    let v = expected_for(
        cfg("NormalDistribution", 100.0, 200.0),
        hist_total(50.0, 100.0, 4, TOTAL),
    )
    .await;
    check_expected(
        &v,
        &[
            1_974.126_513_658,
            1_746.663_219_402,
            1_209.775_787_100,
            655.906_168_030,
        ],
        4_012.936_743_171,
        400.591_568_638,
        "exact",
    );
}

#[tokio::test]
async fn lognormal_uses_sigma2_ln_1_plus_cv2_and_mu_ln_mean_minus_half_sigma2() {
    // mean 1000, std 500: cv 0.5, sigma^2 = ln 1.25 = 0.2231435513, mu = ln 1000 - sigma^2 / 2 = 6.7961835033
    let v = expected_for(
        cfg("LogNormalDistribution", 1000.0, 500.0),
        hist_total(200.0, 400.0, 6, TOTAL),
    )
    .await;
    check_expected(
        &v,
        &[
            1_982.411_052_812,
            3_943.565_463_536,
            2_352.000_867_874,
            1_020.721_152_221,
            410.000_274_664,
            164.269_207_718,
        ],
        7.598_699_687,
        119.433_281_488,
        "exact",
    );
}

#[tokio::test]
async fn gamma_uses_shape_mean_over_std_squared_and_scale_std_squared_over_mean() {
    // mean 1000, std 500: shape 4, scale 250 (Erlang): F(250) = 1 - e^-1 (1 + 1 + 1/2 + 1/6) = 0.018988
    let v = expected_for(
        cfg("GammaDistribution", 1000.0, 500.0),
        hist_total(250.0, 250.0, 10, TOTAL),
    )
    .await;
    check_expected(
        &v,
        &[
            1_238.883_826_253,
            2_098.915_717_163,
            2_137.617_684_155,
            1_684.442_050_693,
            1_138.220_325_207,
            694.384_665_319,
            393.853_042_530,
            211.536_256_888,
            108.904_356_270,
            54.201_834_100,
        ],
        189.881_568_762,
        49.158_672_659,
        "exact",
    );
}

#[tokio::test]
async fn gamma_from_zero_has_no_underflow() {
    let v = expected_for(
        cfg("GammaDistribution", 1000.0, 500.0),
        hist_total(0.0, 250.0, 12, TOTAL),
    )
    .await;
    check_expected(
        &v,
        &[
            189.881_568_762,
            1_238.883_826_253,
            2_098.915_717_163,
            2_137.617_684_155,
            1_684.442_050_693,
            1_138.220_325_207,
            694.384_665_319,
            393.853_042_530,
            211.536_256_888,
            108.904_356_270,
            54.201_834_100,
            26.240_760_581,
        ],
        0.0,
        22.917_912_078,
        "exact",
    );
}

#[tokio::test]
async fn uniform_spans_mean_plus_minus_sqrt3_std() {
    // mean 1000, std 100: [826.7949, 1173.2051], width 346.4102; bins 800..1200 of 50
    let v = expected_for(
        cfg("UniformDistribution", 1000.0, 100.0),
        hist_total(800.0, 50.0, 8, TOTAL),
    )
    .await;
    let mid = 1_443.375_672_974;
    check_expected(
        &v,
        &[
            669.872_981_078,
            mid,
            mid,
            mid,
            mid,
            mid,
            mid,
            669.872_981_078,
        ],
        0.0,
        0.0,
        "exact",
    );
}

#[tokio::test]
async fn uniform_cut_by_the_range_puts_the_outside_mass_in_under_and_overflow() {
    // range 900..1100 of the support 826.79..1173.21: 21.13 % on each side
    let v = expected_for(
        cfg("UniformDistribution", 1000.0, 100.0),
        hist_total(900.0, 25.0, 8, TOTAL),
    )
    .await;
    let b = 721.687_836_487;
    check_expected(&v, &[b; 8], 2_113.248_654_052, 2_113.248_654_052, "exact");
}

#[tokio::test]
async fn uniform_with_a_support_below_zero_clamps_into_the_bin_of_zero() {
    // mean 100, std 100: support [-73.2051, 273.2051]; 21.13 % of the mass is negative and
    // lands in bin 0 (0..50), which also holds 14.43 %: 35.5662 %
    let v = expected_for(
        cfg("UniformDistribution", 100.0, 100.0),
        hist_total(0.0, 50.0, 8, TOTAL),
    )
    .await;
    check_expected(
        &v,
        &[
            3_556.624_327_026,
            1_443.375_672_974,
            1_443.375_672_974,
            1_443.375_672_974,
            1_443.375_672_974,
            669.872_981_078,
            0.0,
            0.0,
        ],
        0.0,
        0.0,
        "exact",
    );
}

#[tokio::test]
async fn deterministic_puts_all_mass_into_the_bin_of_the_mean() {
    // lo 0, bins of 500: 2700 is in bin 5 (2500..3000)
    let v = expected_for(
        cfg("DeterministicDistribution", 2700.0, 0.0),
        hist_total(0.0, 500.0, 8, TOTAL),
    )
    .await;
    let mut want = [0.0; 8];
    want[5] = TOTAL as f64;
    check_expected(&v, &want, 0.0, 0.0, "exact");
}

#[tokio::test]
async fn deterministic_ignores_the_std() {
    let v = expected_for(
        cfg("DeterministicDistribution", 2700.0, 300.0),
        hist_total(0.0, 500.0, 8, TOTAL),
    )
    .await;
    let mut want = [0.0; 8];
    want[5] = TOTAL as f64;
    check_expected(&v, &want, 0.0, 0.0, "exact");
}

#[tokio::test]
async fn deterministic_above_the_range_is_all_overflow_below_it_all_underflow() {
    let v = expected_for(
        cfg("DeterministicDistribution", 5000.0, 0.0),
        hist_total(0.0, 500.0, 8, TOTAL),
    )
    .await;
    check_expected(&v, &[0.0; 8], 0.0, TOTAL as f64, "exact");
    let v = expected_for(
        cfg("DeterministicDistribution", 500.0, 0.0),
        hist_total(1000.0, 500.0, 8, TOTAL),
    )
    .await;
    check_expected(&v, &[0.0; 8], TOTAL as f64, 0.0, "exact");
}

#[tokio::test]
async fn deterministic_zero_delay_lands_in_bin_zero_of_a_range_starting_at_zero() {
    let v = expected_for(
        cfg("DeterministicDistribution", 0.0, 0.0),
        hist_total(0.0, 500.0, 8, TOTAL),
    )
    .await;
    let mut want = [0.0; 8];
    want[0] = TOTAL as f64;
    check_expected(&v, &want, 0.0, 0.0, "exact");
}

// ---- expected: generic properties ------------------------------------------------------------------

#[tokio::test]
async fn the_expected_counts_add_up_to_the_sample_count_for_every_distribution() {
    // a range that cuts every distribution on both sides
    for (dist, mean, std) in [
        ("DeterministicDistribution", 1500.0, 0.0),
        ("NormalDistribution", 1500.0, 700.0),
        ("LogNormalDistribution", 1500.0, 900.0),
        ("GammaDistribution", 1500.0, 750.0),
        ("UniformDistribution", 1500.0, 400.0),
    ] {
        for total in [1_234_u64, 10_000, 777_777] {
            let v = expected_for(cfg(dist, mean, std), hist_total(1000.0, 100.0, 7, total)).await;
            let mut sum = num_of(&v["expected_underflow"]) + num_of(&v["expected_overflow"]);
            for e in v["expected"].as_array().unwrap() {
                let e = num_of(e);
                assert!(e >= 0.0, "{dist}: negative expectation {e}");
                sum += e;
            }
            assert_near(
                sum,
                total as f64,
                1e-6 * total as f64,
                &format!("{dist} N={total}"),
            );
            assert_eq!(v["count"], total);
            assert!(
                num_of(&v["expected_underflow"]) >= 0.0 && num_of(&v["expected_overflow"]) >= 0.0
            );
        }
    }
}

#[tokio::test]
async fn the_expected_counts_scale_with_the_sample_count() {
    let small = expected_for(
        cfg("NormalDistribution", 1000.0, 200.0),
        hist_total(600.0, 100.0, 8, 2_000),
    )
    .await;
    let big = expected_for(
        cfg("NormalDistribution", 1000.0, 200.0),
        hist_total(600.0, 100.0, 8, 20_000),
    )
    .await;
    for i in 0..8 {
        assert_near(
            num_of(&big["expected"][i]),
            10.0 * num_of(&small["expected"][i]),
            1e-6 * 20_000.0,
            "scaling",
        );
    }
    // bin 4 is 1000..1100 = mean .. mean + 0.5 sigma: Phi(0.5) - Phi(0) = 0.19146246127
    assert_near(
        num_of(&small["expected"][4]),
        2_000.0 * 0.191_462_461_27,
        0.05,
        "bin 4",
    );
}

#[tokio::test]
async fn without_samples_every_expected_count_is_zero() {
    let v = expected_for(
        cfg("NormalDistribution", 1000.0, 200.0),
        hist(600.0, 100.0, &[0; 8], 0, 0),
    )
    .await;
    assert_eq!(v["count"], 0);
    assert_eq!(v["expected"].as_array().unwrap().len(), 8);
    for e in v["expected"].as_array().unwrap() {
        assert_eq!(num_of(e), 0.0);
    }
    assert_eq!(num_of(&v["expected_underflow"]), 0.0);
    assert_eq!(num_of(&v["expected_overflow"]), 0.0);
}

#[tokio::test]
async fn each_direction_uses_its_own_config_and_histogram() {
    let a = app_with(
        cfg("DeterministicDistribution", 2700.0, 0.0),
        cfg("UniformDistribution", 1000.0, 100.0),
    );
    a.telemetry.set(
        None,
        snap(
            hist_total(0.0, 500.0, 8, TOTAL),
            hist_total(800.0, 50.0, 8, TOTAL),
        ),
    );
    let d0 = get_hist(&a, 0).await;
    let mut want = [0.0; 8];
    want[5] = TOTAL as f64;
    check_expected(&d0, &want, 0.0, 0.0, "exact");
    let d1 = get_hist(&a, 1).await;
    let mid = 1_443.375_672_974;
    check_expected(
        &d1,
        &[
            669.872_981_078,
            mid,
            mid,
            mid,
            mid,
            mid,
            mid,
            669.872_981_078,
        ],
        0.0,
        0.0,
        "exact",
    );
}

#[tokio::test]
async fn the_expected_array_follows_the_bin_count_of_the_telemetry_not_of_the_config() {
    let a = app_with(
        cfg("NormalDistribution", 3000.0, 400.0),
        cfg_json(1.0e6, 1.0e5),
    );
    // the fake dpds default for direction 1 has 100 bins, direction 0 has 256
    let s = snapshot([zero_dir(), zero_dir()], [zero_port(), zero_port()]);
    a.telemetry.set(None, s);
    let v0 = get_hist(&a, 0).await;
    assert_eq!(v0["expected"].as_array().unwrap().len(), 256);
    let v1 = get_hist(&a, 1).await;
    assert_eq!(v1["expected"].as_array().unwrap().len(), 100);
}

// ---- model and note ----------------------------------------------------------------------------------

#[tokio::test]
async fn with_correlation_every_distribution_is_approximated_by_the_normal() {
    // Normal(1000, 1000) numbers of `normal_with_the_negative_mass...`
    let want = [
        3_085.375_387_260,
        1_914.624_612_740,
        1_914.624_612_740,
        1_498.822_847_945,
        918.480_526_626,
        440.570_693_207,
        165.404_666_224,
        48.597_672_941,
    ];
    for dist in [
        "NormalDistribution",
        "LogNormalDistribution",
        "GammaDistribution",
        "UniformDistribution",
    ] {
        let mut c = cfg(dist, 1000.0, 1000.0);
        c["delayer"]["activate_correlation"] = json!(true);
        c["delayer"]["half_life_period_ns"] = json!(1.0e6);
        let v = expected_for(c, hist_total(0.0, 500.0, 8, TOTAL)).await;
        check_expected(&v, &want, 0.0, 13.498_980_316, "normal_approximation");
    }
}

#[tokio::test]
async fn the_model_is_per_direction() {
    let mut corr = cfg("GammaDistribution", 1000.0, 500.0);
    corr["delayer"]["activate_correlation"] = json!(true);
    let a = app_with(corr, cfg("GammaDistribution", 1000.0, 500.0));
    a.telemetry.set(
        None,
        snap(
            hist_total(0.0, 250.0, 12, TOTAL),
            hist_total(0.0, 250.0, 12, TOTAL),
        ),
    );
    assert_eq!(
        get_hist(&a, 0).await["expected_model"],
        "normal_approximation"
    );
    assert_eq!(get_hist(&a, 1).await["expected_model"], "exact");
}

/// A limiter stage of `mode` (present = active).
fn limited(mode: &str) -> Value {
    let mut c = cfg("NormalDistribution", 1000.0, 200.0);
    c["limiter"] = json!({"mode": mode, "rate_bps": 1.0e9});
    c
}

#[tokio::test]
async fn an_active_limiter_adds_the_note() {
    for mode in ["Spacer", "TokenBucketFilter"] {
        let v = expected_for(limited(mode), hist_total(600.0, 100.0, 8, TOTAL)).await;
        assert_eq!(
            v["expected_note"], "excludes limiter queueing delay",
            "{mode}: {v}"
        );
        // the numbers are those of the plain delay distribution
        assert_near(num_of(&v["expected"][3]), 1_914.624_612_740, 0.05, "bin 3");
        assert_eq!(v["expected_model"], "exact");
    }
}

#[tokio::test]
async fn a_config_without_a_limiter_adds_no_note() {
    let v = expected_for(
        cfg("NormalDistribution", 1000.0, 200.0),
        hist_total(600.0, 100.0, 8, TOTAL),
    )
    .await;
    assert!(absent(&v, "expected_note"), "{v}");
    // an explicit null is the same as no stage
    let mut c = cfg("NormalDistribution", 1000.0, 200.0);
    c["limiter"] = json!(null);
    let v = expected_for(c, hist_total(600.0, 100.0, 8, TOTAL)).await;
    assert!(absent(&v, "expected_note"), "{v}");
}

#[tokio::test]
async fn a_config_without_a_delayer_expects_every_sample_at_zero_delay() {
    // the missing delayer is Deterministic 0 ns: all expected mass sits in the bin of 0
    for c in [cfg_bare(), {
        let mut c = cfg_bare();
        c["delayer"] = json!(null);
        c
    }] {
        let v = expected_for(c, hist_total(0.0, 500.0, 8, TOTAL)).await;
        let mut want = [0.0; 8];
        want[0] = TOTAL as f64;
        check_expected(&v, &want, 0.0, 0.0, "exact");
    }
    // above a range that starts at 500 ns it is all underflow
    let v = expected_for(cfg_bare(), hist_total(500.0, 500.0, 8, TOTAL)).await;
    check_expected(&v, &[0.0; 8], TOTAL as f64, 0.0, "exact");
}

#[tokio::test]
async fn the_note_and_the_model_can_come_together() {
    let mut c = limited("Spacer");
    c["delayer"]["activate_correlation"] = json!(true);
    let v = expected_for(c, hist_total(600.0, 100.0, 8, TOTAL)).await;
    assert_eq!(v["expected_note"], "excludes limiter queueing delay");
    assert_eq!(v["expected_model"], "normal_approximation");
}

#[tokio::test]
async fn the_note_is_per_direction() {
    let a = app_with(limited("Spacer"), cfg("NormalDistribution", 1000.0, 200.0));
    a.telemetry.set(
        None,
        snap(
            hist_total(600.0, 100.0, 8, TOTAL),
            hist_total(600.0, 100.0, 8, TOTAL),
        ),
    );
    assert_eq!(
        get_hist(&a, 0).await["expected_note"],
        "excludes limiter queueing delay"
    );
    assert!(absent(&get_hist(&a, 1).await, "expected_note"));
}
