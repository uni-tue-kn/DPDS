//! The IAT histogram is configurable like the delay histogram.
//!
//!
//! ```text
//! HistRange::iat_from_spec(&Config, &HistogramSpec) -> Result<HistRange, HistogramError>
//!     like `from_spec` with the IAT defaults: lo_ns 0, hi_ns 4e9 / initialization_rate_pps,
//!     256 bins. A default spec equals `iat_from_config`.
//! LinkSpec.iat_histogram / LinkDirection.iat_histogram: HistogramSpec
//!     the link-level `"iat_histogram": {bins, lo_ns, hi_ns}` of the links file, beside
//!     `"histogram"`; absent = `HistogramSpec::default()`; validated like `histogram`, an error
//!     names the link (`link <ingress> -> <egress>`) and "IAT histogram".
//! ```
//!
//! IAT tests use `initialization_rate_pps = 15_625_000`: default hi = 256 ns.

use dpds::delay_stats::{HistRange, HistogramSpec};
use dpds::links::{parse_links, resolve_directions};
use serde_json::{json, Value};

use crate::support::*;

const RATE: f64 = 15_625_000.0;

fn spec(bins: Option<usize>, lo: Option<f64>, hi: Option<f64>) -> HistogramSpec {
    HistogramSpec {
        bins,
        lo_ns: lo,
        hi_ns: hi,
    }
}

fn base() -> dpds_core::Config {
    let mut c = config(100, 0.0, 0.0);
    c.delayer.initialization_rate_pps = RATE;
    c
}

fn iat(sp: HistogramSpec) -> Result<HistRange, String> {
    HistRange::iat_from_spec(&base(), &sp).map_err(|e| e.to_string())
}

fn close(r: &HistRange, lo: f64, hi: f64, bins: usize) {
    assert_eq!(r.bins, bins, "{r:?}");
    assert!((r.lo_ns - lo).abs() < 1e-9, "{r:?}");
    assert!((r.hi_ns() - hi).abs() < 1e-9, "{r:?}");
    assert!((r.bin_ns - (hi - lo) / bins as f64).abs() < 1e-9, "{r:?}");
}

// ---- the range ------------------------------------------------------------------------------

#[test]
fn a_default_spec_is_the_spec_51_iat_range() {
    for rate in [RATE, 1e6, 1e9, 15e6] {
        let mut c = base();
        c.delayer.initialization_rate_pps = rate;
        let got = HistRange::iat_from_spec(&c, &HistogramSpec::default()).unwrap();
        assert_eq!(got, HistRange::iat_from_config(&c), "rate {rate}");
    }
}

#[test]
fn omitted_fields_keep_their_iat_defaults() {
    close(&iat(spec(Some(64), None, None)).unwrap(), 0.0, 256.0, 64);
    close(
        &iat(spec(None, Some(10.0), None)).unwrap(),
        10.0,
        256.0,
        256,
    );
    close(
        &iat(spec(None, None, Some(512.0))).unwrap(),
        0.0,
        512.0,
        256,
    );
    close(
        &iat(spec(Some(32), Some(8.0), Some(40.0))).unwrap(),
        8.0,
        40.0,
        32,
    );
}

#[test]
fn the_iat_defaults_do_not_depend_on_the_delay_distribution() {
    let mut c = config_general(
        10,
        false,
        "NormalDistribution",
        5e6,
        1e5,
        Some(("Spacer", 1e9)),
        json!({ "Random": 0.1 }),
    );
    c.delayer.initialization_rate_pps = RATE;
    let got = HistRange::iat_from_spec(&c, &spec(Some(128), None, None)).unwrap();
    close(&got, 0.0, 256.0, 128);
}

#[test]
fn the_iat_range_follows_the_delay_histograms_limits() {
    for bad in [
        spec(Some(0), None, None),
        spec(Some(4097), None, None),
        spec(None, Some(-1.0), None),
        spec(None, None, Some(f64::NAN)),
        spec(None, Some(5.0), Some(5.0)),
        // lo above the IAT default hi (256 ns)
        spec(None, Some(300.0), None),
    ] {
        assert!(iat(bad).is_err(), "{bad:?}");
    }
    assert!(iat(spec(Some(4096), None, None)).is_ok());
}

// ---- the links file -------------------------------------------------------------------------

fn cfg() -> Value {
    json!({
        "buffer_capacity": 10, "reordering": false,
        "delayer": { "distribution": "DeterministicDistribution", "mean_ns": 1000.0, "std_ns": 0.0,
                     "activate_correlation": false, "half_life_period_ns": 1.0,
                     "initialization_rate_pps": 15000000.0 },
        "limiter": { "mode": "Spacer", "activate": false, "rate_bps": 1e9 },
        "loss_model": { "Random": 0.0 }
    })
}

fn link(ingress: &str, egress: &str, extra: Value) -> Value {
    let mut l = json!({ "ingress": ingress, "egress": egress, "config": cfg() });
    for (k, v) in extra.as_object().unwrap() {
        l[k] = v.clone();
    }
    l
}

fn parse(links: Vec<Value>) -> Result<Vec<dpds::links::LinkSpec>, String> {
    parse_links(&json!({ "links": links }).to_string()).map_err(|e| e.to_string())
}

#[test]
fn an_iat_histogram_is_parsed_per_link_and_independent_of_the_delay_one() {
    let links = parse(vec![
        link("p0", "p1", json!({ "iat_histogram": { "bins": 100 } })),
        link(
            "p1",
            "p2",
            json!({ "iat_histogram": { "lo_ns": 10.0, "hi_ns": 20.5 },
                    "histogram": { "bins": 7 } }),
        ),
        link("p3", "p3", json!({ "iat_histogram": {} })),
        link("p4", "p0", json!({})),
    ])
    .unwrap();
    assert_eq!(links[0].iat_histogram, spec(Some(100), None, None));
    assert_eq!(links[0].histogram, HistogramSpec::default());
    assert_eq!(links[1].iat_histogram, spec(None, Some(10.0), Some(20.5)));
    assert_eq!(links[1].histogram, spec(Some(7), None, None));
    assert_eq!(links[2].iat_histogram, HistogramSpec::default());
    assert_eq!(links[3].iat_histogram, HistogramSpec::default());
}

#[test]
fn a_bad_iat_histogram_is_an_error_naming_the_link_and_the_iat_histogram() {
    for bad in [
        json!({ "bins": 0 }),
        json!({ "bins": 4097 }),
        json!({ "lo_ns": -1.0 }),
        json!({ "hi_ns": -1.0 }),
        json!({ "lo_ns": 5.0, "hi_ns": 5.0 }),
        json!({ "lo_ns": 9.0, "hi_ns": 5.0 }),
    ] {
        let e = parse(vec![link("p", "q", json!({ "iat_histogram": bad }))]).unwrap_err();
        assert!(e.contains("link p -> q"), "{bad}: {e}");
        assert!(e.contains("IAT histogram"), "{bad}: {e}");
    }
    // a bad delay histogram does not talk about the IAT one
    let e = parse(vec![link("p", "q", json!({ "histogram": { "bins": 0 } }))]).unwrap_err();
    assert!(!e.contains("IAT"), "{e}");
}

#[test]
fn wrong_types_and_unknown_keys_of_the_iat_histogram_are_errors() {
    for bad in [
        json!({ "bins": -1 }),
        json!({ "bins": "100" }),
        json!({ "lo_ns": "x" }),
        json!({ "bin_ns": 10 }),
        json!([]),
        json!(3),
    ] {
        assert!(
            parse(vec![link("p", "q", json!({ "iat_histogram": bad }))]).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn directions_carry_the_iat_histogram_spec_of_their_link() {
    let links = parse(vec![
        link("P0", "P1", json!({ "iat_histogram": { "bins": 10 } })),
        link("P1", "P2", json!({})),
    ])
    .unwrap();
    let ids = vec![("P0".to_string(), 0u16), ("P1".into(), 1), ("P2".into(), 2)];
    let d = resolve_directions(&links, &ids, 0, &[1, 2]).unwrap();
    assert_eq!(d[0].iat_histogram, spec(Some(10), None, None));
    assert_eq!(d[1].iat_histogram, HistogramSpec::default());
}
