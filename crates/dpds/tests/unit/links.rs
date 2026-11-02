//! The `--links <file>` file: parsing, validation and the numbering of the
//! directions (A1).
//!
//! # Test-facing API (new module `dpds::links`)
//!
//! ```text
//! pub const MAX_NAME_LEN: usize = 32;
//! pub struct LinkSpec { pub name: String, pub ports: Vec<String>, pub configs: Vec<dpds_core::Config>,
//!                       pub lcores: Option<Vec<u32>>,
//!                       pub histograms: Vec<dpds::delay_stats::HistogramSpec> }
//!     `histograms` comes from the optional `"histograms": [ {"bins": n, "lo_ns": x,
//!     "hi_ns": y}, .. ]` of the link (one entry per direction, same length as
//!     `configs`, every field of an entry optional, `{}` allowed). After parsing it
//!     ALWAYS has `configs.len()` entries: absent -> all `HistogramSpec::default()`.
//!     Static validation errors (naming the link): length differs from `configs`;
//!     `bins` not an integer in 1..=500; `lo_ns`/`hi_ns` negative or not a number;
//!     both given and `lo_ns >= hi_ns`. (The check against the automatic range
//!     when only one of lo/hi is given is `HistRange::from_spec`'s, at startup.)
//! pub fn parse_links(json: &str) -> Result<Vec<LinkSpec>, LinksError>
//!     JSON `{ "links": [ { "name", "ports": [..], "configs": [..] }, .. ] }`,
//!     `configs` are `dpds_core::Config` objects. File-level validation only:
//!       - invalid JSON / missing "links" / a bad Config      -> Err
//!       - no link at all (`"links": []`)                      -> Err
//!       - name empty, longer than 32, or a char outside [A-Za-z0-9_.-] -> Err naming the link
//!       - duplicate name                                       -> Err naming the name
//!       - ports.len() not 1 or 2 (or an empty port string)     -> Err naming the link
//!       - configs.len() != ports.len()                         -> Err naming the link
//!       - a port listed twice (in one link or in two links)    -> Err naming the port
//! pub fn load_links(path: &Path) -> Result<Vec<LinkSpec>, LinksError>   (read + parse_links;
//!     an unreadable file is an Err whose Display contains the path)
//! pub fn num_directions(links: &[LinkSpec]) -> usize                    (= total number of ports)
//! #[derive(Debug, Clone)]
//! pub struct LinkDirection { pub link: usize, pub rx_port: u16, pub tx_port: u16,
//!                            pub config: dpds_core::Config,
//!                            pub histogram: dpds::delay_stats::HistogramSpec,   (= histograms[k])
//!                            pub lcore: u32 }   (the worker lcore that runs the direction)
//! pub fn resolve_directions(links: &[LinkSpec], probed: &[(String, u16)],
//!                           main_lcore: u32, workers: &[u32])
//!     -> Result<Vec<LinkDirection>, LinksError>
//!     (`workers` = the EAL worker lcores in ascending order, main lcore excluded)
//!     `probed` = (DPDK device name, port id) of every probed ethdev, i.e. what
//!     `rte_eth_dev_get_port_by_name` resolves (no reliance on port-id order).
//!     Directions are numbered globally in file order: link 0 dir 0 (ports[0] ->
//!     ports[1], configs[0]), link 0 dir 1 (ports[1] -> ports[0], configs[1]),
//!     link 1 dir 0, ... A one-port link has one direction (rx = tx = that port).
//!       - a link port that is not probed        -> Err naming the port
//!       - a probed port in no link              -> Err naming the port (device name)
//! LinksError: Debug + Display + std::error::Error
//!
//! # Explicit lcores (Amendment 2)
//! A link may carry `"lcores": [..]`, one per direction (same length as
//! `configs`), `LinkSpec.lcores: Some(..)`; absent -> `None`.
//!   parse_links errors (naming the link / the lcore / the word "lcores"):
//!     length differs from `configs`; a negative or non-integer entry; an lcore
//!     listed twice (within a link or across links); some links with `lcores`
//!     and some without (mixing).
//!   resolve_directions, with explicit lcores: `LinkDirection.lcore` is exactly
//!     the listed lcore, in listing order (no sorting; `[2, 1]` is fine). Errors:
//!     an lcore equal to `main_lcore` (message contains "main"), an lcore not in
//!     `workers` (message contains the number and "worker"), a worker lcore that
//!     no direction uses (message contains its number).
//!   Without `lcores` anywhere: direction i runs on `workers[i]`; a different
//!   number of workers than directions is an error.
//! ```

use std::path::Path;

use dpds::delay_stats::HistogramSpec;
use dpds::links::{
    load_links, num_directions, parse_links, resolve_directions, LinkDirection, LinkSpec,
    LinksError, MAX_NAME_LEN,
};
use serde_json::{json, Value};

fn cfg(capacity: u64) -> Value {
    json!({
        "buffer_capacity": capacity,
        "reordering": false,
        "delayer": {
            "distribution": "DeterministicDistribution",
            "mean_ns": 1000.0,
            "std_ns": 0.0,
            "activate_correlation": false,
            "half_life_period_ns": 1.0
        },
        "limiter": {
            "mode": "Spacer",
            "activate": false,
            "rate_bps": 1e9,
            "limit_bit": 1e6
        },
        "loss_model": { "Random": 0.0 }
    })
}

fn link(name: &str, ports: &[&str], caps: &[u64]) -> Value {
    json!({ "name": name, "ports": ports, "configs": caps.iter().map(|c| cfg(*c)).collect::<Vec<_>>() })
}

fn file(links: Vec<Value>) -> String {
    json!({ "links": links }).to_string()
}

fn err_of(links: Vec<Value>) -> String {
    match parse_links(&file(links)) {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.to_string(),
    }
}

fn spec_file() -> String {
    file(vec![
        link("a-b", &["0000:65:00.0", "0000:65:00.1"], &[11, 22]),
        link("loop", &["net_null0"], &[33]),
    ])
}

// ── parsing ─────────────────────────────────────────────────────────

#[test]
fn the_documented_two_link_file_parses_with_its_configs_in_order() {
    let links = parse_links(&spec_file()).unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].name, "a-b");
    assert_eq!(links[0].ports, ["0000:65:00.0", "0000:65:00.1"]);
    assert_eq!(
        links[0]
            .configs
            .iter()
            .map(|c| c.buffer_capacity)
            .collect::<Vec<_>>(),
        [11, 22]
    );
    assert_eq!(links[1].name, "loop");
    assert_eq!(links[1].ports, ["net_null0"]);
    assert_eq!(links[1].configs.len(), 1);
    assert_eq!(links[1].configs[0].buffer_capacity, 33);
    assert_eq!(links[0].configs[0].delayer.mean_ns, 1000.0);
    assert_eq!(num_directions(&links), 3);
    assert_eq!(num_directions(&links[1..]), 1);
}

#[test]
fn a_config_may_use_every_dpds_core_field_incl_the_seed() {
    let mut c = cfg(5);
    c["seed"] = json!(42);
    c["loss_model"] = json!({ "GilbertElliot": [0.01, 0.3, 1.0, 0.5, true] });
    let l = json!({ "name": "x", "ports": ["p"], "configs": [c] });
    let links = parse_links(&file(vec![l])).unwrap();
    assert_eq!(links[0].configs[0].seed, Some(42));
}

#[test]
fn syntax_and_structure_errors_are_errors_not_panics() {
    for bad in [
        "",
        "not json",
        "{}",
        r#"{"links": 3}"#,
        r#"{"links": [{}]}"#,
        r#"{"links": [{"name": "a", "ports": ["p"]}]}"#,
        r#"{"links": [{"name": "a", "ports": ["p"], "configs": [{"buffer_capacity": 1}]}]}"#,
        r#"{"links": [{"name": 1, "ports": ["p"], "configs": []}]}"#,
    ] {
        assert!(parse_links(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn no_link_at_all_is_an_error() {
    assert!(parse_links(r#"{"links": []}"#).is_err());
}

#[test]
fn names_follow_the_documented_rule() {
    let ok32 = "a".repeat(MAX_NAME_LEN);
    assert_eq!(MAX_NAME_LEN, 32);
    for good in ["a", "A-b", "x_1.2-Z", "0", ok32.as_str()] {
        assert!(
            parse_links(&file(vec![link(good, &["p"], &[1])])).is_ok(),
            "{good:?}"
        );
    }
    let long = "a".repeat(MAX_NAME_LEN + 1);
    for bad in [
        "",
        "a b",
        "a/b",
        "ä",
        "a:b",
        "a,b",
        "a\"b",
        "a\nb",
        long.as_str(),
    ] {
        let e = err_of(vec![link(bad, &["p"], &[1])]);
        assert!(!e.is_empty(), "{bad:?}");
    }
}

#[test]
fn a_duplicate_name_is_an_error_naming_it() {
    let e = err_of(vec![link("same", &["p"], &[1]), link("same", &["q"], &[1])]);
    assert!(e.contains("same"), "{e}");
}

#[test]
fn a_link_has_one_or_two_ports() {
    let e = err_of(vec![link("zero", &[], &[])]);
    assert!(e.contains("zero"), "{e}");
    let e = err_of(vec![link("three", &["a", "b", "c"], &[1, 1, 1])]);
    assert!(e.contains("three"), "{e}");
    assert!(parse_links(&file(vec![link("one", &["a"], &[1])])).is_ok());
    assert!(parse_links(&file(vec![link("two", &["a", "b"], &[1, 1])])).is_ok());
    assert!(parse_links(&file(vec![link("blank", &[""], &[1])])).is_err());
}

#[test]
fn the_number_of_configs_must_equal_the_number_of_ports() {
    for (ports, caps) in [
        (vec!["a", "b"], vec![1u64]),
        (vec!["a"], vec![1, 2]),
        (vec!["a", "b"], vec![1, 2, 3]),
        (vec!["a"], vec![]),
    ] {
        let e = err_of(vec![link("mism", &ports, &caps)]);
        assert!(e.contains("mism"), "{ports:?}/{caps:?}: {e}");
    }
}

#[test]
fn a_port_may_belong_to_at_most_one_link_and_appear_once() {
    let e = err_of(vec![
        link("one", &["0000:65:00.0", "0000:65:00.1"], &[1, 1]),
        link("two", &["0000:65:00.1"], &[1]),
    ]);
    assert!(e.contains("0000:65:00.1"), "{e}");
    let e = err_of(vec![link("dup", &["net_null0", "net_null0"], &[1, 1])]);
    assert!(e.contains("net_null0"), "{e}");
}

#[test]
fn load_links_reads_a_file_and_reports_a_missing_one_with_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("links.json");
    std::fs::write(&good, spec_file()).unwrap();
    assert_eq!(load_links(&good).unwrap().len(), 2);

    let missing = dir.path().join("nope.json");
    let e = load_links(&missing).unwrap_err().to_string();
    assert!(e.contains("nope.json"), "{e}");

    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "{ not json").unwrap();
    assert!(load_links(&bad).is_err());
    assert!(load_links(Path::new(dir.path())).is_err(), "a directory");
}

#[test]
fn links_error_is_a_std_error() {
    fn assert_error<E: std::error::Error + std::fmt::Debug>(_: &E) {}
    let e = parse_links("x").unwrap_err();
    assert_error(&e);
    assert!(!e.to_string().is_empty());
}

// ── direction numbering and resolution by device name ───────────────

fn probed(pairs: &[(&str, u16)]) -> Vec<(String, u16)> {
    pairs.iter().map(|(n, i)| (n.to_string(), *i)).collect()
}

/// Main lcore 0 and the workers 1..=directions (the `-l 0-n` layout).
fn resolve(links: &[LinkSpec], ids: &[(String, u16)]) -> Result<Vec<LinkDirection>, LinksError> {
    let workers: Vec<u32> = (1..=num_directions(links) as u32).collect();
    resolve_directions(links, ids, 0, &workers)
}

fn links_of(links: Vec<Value>) -> Vec<LinkSpec> {
    parse_links(&file(links)).unwrap()
}

#[test]
fn directions_are_numbered_in_file_order_link_by_link() {
    let links = links_of(vec![
        link("ab", &["P0", "P1"], &[11, 22]),
        link("loop", &["P2"], &[33]),
        link("cd", &["P3", "P4"], &[44, 55]),
    ]);
    let ids = probed(&[("P0", 0), ("P1", 1), ("P2", 2), ("P3", 3), ("P4", 4)]);
    let d = resolve(&links, &ids).unwrap();
    let got: Vec<(usize, u16, u16, usize)> = d
        .iter()
        .map(|x| (x.link, x.rx_port, x.tx_port, x.config.buffer_capacity))
        .collect();
    assert_eq!(
        got,
        [
            (0, 0, 1, 11), // link ab dir 0: ports[0] -> ports[1], configs[0]
            (0, 1, 0, 22), // link ab dir 1: ports[1] -> ports[0], configs[1]
            (1, 2, 2, 33), // loopback: rx = tx
            (2, 3, 4, 44),
            (2, 4, 3, 55),
        ]
    );
    assert_eq!(d.len(), num_directions(&links));
    // without explicit lcores direction i runs on worker i (here 1..=5)
    assert_eq!(
        d.iter().map(|x| x.lcore).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    // the lcore rule: 1 + directions
    assert!(dpds::setup::validate_lcore_count(1 + d.len(), d.len()).is_ok());
}

#[test]
fn ports_are_resolved_by_device_name_not_by_port_id_order() {
    // The link lists the later-created / higher-addressed device first; the
    // port ids are not in listing order and not alphabetical either.
    let links = links_of(vec![
        link("x", &["0000:83:00.0", "0000:17:00.0"], &[100, 200]),
        link("y", &["net_null9"], &[300]),
    ]);
    let ids = probed(&[("0000:17:00.0", 0), ("0000:83:00.0", 1), ("net_null9", 2)]);
    let d = resolve(&links, &ids).unwrap();
    // dir 0 = ports[0] -> ports[1] = 0000:83 (id 1) -> 0000:17 (id 0) with configs[0]
    assert_eq!(
        (d[0].rx_port, d[0].tx_port, d[0].config.buffer_capacity),
        (1, 0, 100)
    );
    assert_eq!(
        (d[1].rx_port, d[1].tx_port, d[1].config.buffer_capacity),
        (0, 1, 200)
    );
    assert_eq!(
        (d[2].rx_port, d[2].tx_port, d[2].config.buffer_capacity),
        (2, 2, 300)
    );
    // a different id assignment gives different ids for the same names
    let ids = probed(&[("net_null9", 0), ("0000:17:00.0", 1), ("0000:83:00.0", 2)]);
    let d = resolve(&links, &ids).unwrap();
    assert_eq!((d[0].rx_port, d[0].tx_port), (2, 1));
    assert_eq!((d[1].rx_port, d[1].tx_port), (1, 2));
    assert_eq!((d[2].rx_port, d[2].tx_port), (0, 0));
}

#[test]
fn a_link_port_that_is_not_probed_is_an_error_naming_it() {
    let links = links_of(vec![link("ab", &["P0", "P9"], &[1, 1])]);
    let e = resolve(&links, &probed(&[("P0", 0)]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("P9"), "{e}");
}

#[test]
fn a_probed_port_that_belongs_to_no_link_is_an_error_naming_it() {
    let links = links_of(vec![link("ab", &["P0", "P1"], &[1, 1])]);
    let e = resolve(&links, &probed(&[("P0", 0), ("P1", 1), ("P2", 2)]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("P2"), "{e}");
}

#[test]
fn exactly_the_probed_ports_in_the_links_is_fine_and_a_single_loopback_works() {
    let links = links_of(vec![link("l", &["net_null0"], &[7])]);
    let d = resolve(&links, &probed(&[("net_null0", 0)])).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].link, d[0].rx_port, d[0].tx_port), (0, 0, 0));
}

// ── optional per-direction histograms ───────────────────────────────

fn link_h(name: &str, ports: &[&str], caps: &[u64], hist: Value) -> Value {
    let mut l = link(name, ports, caps);
    l["histograms"] = hist;
    l
}

fn spec(bins: Option<usize>, lo: Option<f64>, hi: Option<f64>) -> HistogramSpec {
    HistogramSpec {
        bins,
        lo_ns: lo,
        hi_ns: hi,
    }
}

#[test]
fn without_histograms_every_direction_gets_the_default_spec() {
    let links = parse_links(&spec_file()).unwrap();
    for l in &links {
        assert_eq!(l.histograms.len(), l.configs.len());
        assert!(l.histograms.iter().all(|h| *h == HistogramSpec::default()));
    }
}

#[test]
fn histograms_are_parsed_per_direction_with_every_field_optional() {
    let links = parse_links(&file(vec![
        link_h(
            "ab",
            &["p0", "p1"],
            &[1, 2],
            json!([{ "bins": 100 }, { "lo_ns": 10.0, "hi_ns": 20.5 }]),
        ),
        link_h("l", &["p2"], &[3], json!([{}])),
        link_h(
            "full",
            &["p3"],
            &[4],
            json!([{ "bins": 500, "lo_ns": 0, "hi_ns": 1e6 }]),
        ),
    ]))
    .unwrap();
    assert_eq!(links[0].histograms[0], spec(Some(100), None, None));
    assert_eq!(links[0].histograms[1], spec(None, Some(10.0), Some(20.5)));
    assert_eq!(links[1].histograms, [HistogramSpec::default()]);
    assert_eq!(links[2].histograms, [spec(Some(500), Some(0.0), Some(1e6))]);
}

#[test]
fn the_documented_limits_are_inclusive() {
    for h in [
        json!({ "bins": 1 }),
        json!({ "bins": 500 }),
        json!({ "lo_ns": 0.0 }),
        json!({ "lo_ns": 0.0, "hi_ns": 0.001 }),
    ] {
        assert!(
            parse_links(&file(vec![link_h("x", &["p"], &[1], json!([h.clone()]))])).is_ok(),
            "{h}"
        );
    }
}

#[test]
fn bad_histograms_are_errors_naming_the_link() {
    for bad in [
        json!({ "bins": 0 }),
        json!({ "bins": 501 }),
        json!({ "bins": 100000 }),
        json!({ "lo_ns": -1.0 }),
        json!({ "hi_ns": -1.0 }),
        json!({ "lo_ns": 5.0, "hi_ns": 5.0 }),
        json!({ "lo_ns": 9.0, "hi_ns": 5.0 }),
    ] {
        let e = err_of(vec![link_h(
            "hist-link",
            &["p"],
            &[1],
            json!([bad.clone()]),
        )]);
        assert!(e.contains("hist-link"), "{bad}: {e}");
    }
    // values of the wrong JSON type are errors too (the message need not name the link)
    for bad in [
        json!({ "bins": -1 }),
        json!({ "bins": 2.5 }),
        json!({ "bins": "100" }),
        json!({ "lo_ns": "x" }),
    ] {
        assert!(
            parse_links(&file(vec![link_h("x", &["p"], &[1], json!([bad.clone()]))])).is_err(),
            "{bad}"
        );
    }
    // not an array, or an entry that is not an object
    assert!(parse_links(&file(vec![link_h("x", &["p"], &[1], json!({}))])).is_err());
    assert!(parse_links(&file(vec![link_h("x", &["p"], &[1], json!([3]))])).is_err());
}

#[test]
fn the_histograms_array_must_be_as_long_as_the_configs() {
    for (ports, caps, hist) in [
        (vec!["a", "b"], vec![1u64, 2], json!([{ "bins": 10 }])),
        (vec!["a"], vec![1], json!([{}, {}])),
        (vec!["a"], vec![1], json!([])),
        (vec!["a", "b"], vec![1, 2], json!([{}, {}, {}])),
    ] {
        let e = err_of(vec![link_h("lenmis", &ports, &caps, hist)]);
        assert!(e.contains("lenmis"), "{e}");
    }
}

#[test]
fn directions_carry_the_histogram_spec_of_their_config_slot() {
    let links = links_of(vec![
        link_h(
            "ab",
            &["P0", "P1"],
            &[11, 22],
            json!([{ "bins": 10 }, { "lo_ns": 1.0, "hi_ns": 2.0 }]),
        ),
        link("loop", &["P2"], &[33]),
    ]);
    let ids = probed(&[("P0", 0), ("P1", 1), ("P2", 2)]);
    let d = resolve(&links, &ids).unwrap();
    assert_eq!(d[0].histogram, spec(Some(10), None, None));
    assert_eq!(d[1].histogram, spec(None, Some(1.0), Some(2.0)));
    assert_eq!(d[2].histogram, HistogramSpec::default());
}

// ── explicit lcores ─────────────────────────────────────────────────

fn link_l(name: &str, ports: &[&str], caps: &[u64], lcores: Value) -> Value {
    let mut l = link(name, ports, caps);
    l["lcores"] = lcores;
    l
}

#[test]
fn lcores_are_optional_and_parsed_per_link() {
    let links = parse_links(&spec_file()).unwrap();
    assert!(links.iter().all(|l| l.lcores.is_none()));
    let links = parse_links(&file(vec![
        link_l("ab", &["p0", "p1"], &[1, 2], json!([7, 2])),
        link_l("l", &["p2"], &[3], json!([3])),
    ]))
    .unwrap();
    assert_eq!(links[0].lcores, Some(vec![7, 2]), "listing order is kept");
    assert_eq!(links[1].lcores, Some(vec![3]));
}

#[test]
fn lcores_of_the_wrong_length_are_an_error_naming_the_link() {
    for (ports, caps, lcores) in [
        (vec!["a", "b"], vec![1u64, 2], json!([1])),
        (vec!["a"], vec![1], json!([1, 2])),
        (vec!["a"], vec![1], json!([])),
    ] {
        let e = err_of(vec![link_l("lcmis", &ports, &caps, lcores)]);
        assert!(e.contains("lcmis"), "{e}");
    }
}

#[test]
fn a_repeated_lcore_is_an_error_naming_it() {
    // within a link
    let e = err_of(vec![link_l("x", &["a", "b"], &[1, 2], json!([4, 4]))]);
    assert!(e.contains('4') && e.contains("lcore"), "{e}");
    // across links
    let e = err_of(vec![
        link_l("x", &["a", "b"], &[1, 2], json!([1, 2])),
        link_l("y", &["c"], &[3], json!([2])),
    ]);
    assert!(e.contains('2') && e.contains("lcore"), "{e}");
}

#[test]
fn mixing_links_with_and_without_lcores_is_an_error() {
    for links in [
        vec![
            link_l("with", &["a"], &[1], json!([1])),
            link("without", &["b"], &[2]),
        ],
        vec![
            link("without", &["a"], &[1]),
            link_l("with", &["b"], &[2], json!([1])),
        ],
    ] {
        let e = err_of(links);
        assert!(e.contains("lcores"), "{e}");
    }
}

#[test]
fn lcores_of_the_wrong_json_type_are_errors() {
    for bad in [
        json!([-1]),
        json!([1.5]),
        json!(["1"]),
        json!(3),
        json!([null]),
    ] {
        assert!(
            parse_links(&file(vec![link_l("x", &["a"], &[1], bad.clone())])).is_err(),
            "{bad}"
        );
    }
}

fn two_links(lcores: Option<(Value, Value)>) -> Vec<LinkSpec> {
    let (mut ab, mut l) = (
        link("ab", &["P0", "P1"], &[11, 22]),
        link("loop", &["P2"], &[33]),
    );
    if let Some((a, b)) = lcores {
        ab["lcores"] = a;
        l["lcores"] = b;
    }
    links_of(vec![ab, l])
}

const IDS: [(&str, u16); 3] = [("P0", 0), ("P1", 1), ("P2", 2)];

#[test]
fn without_lcores_direction_i_runs_on_the_i_th_worker_in_ascending_order() {
    let links = two_links(None);
    let d = resolve_directions(&links, &probed(&IDS), 1, &[3, 5, 9]).unwrap();
    assert_eq!(d.iter().map(|x| x.lcore).collect::<Vec<_>>(), [3, 5, 9]);
    // a different number of workers than directions is an error
    assert!(resolve_directions(&links, &probed(&IDS), 0, &[1, 2]).is_err());
    assert!(resolve_directions(&links, &probed(&IDS), 0, &[1, 2, 3, 4]).is_err());
}

#[test]
fn explicit_lcores_are_honoured_literally_in_listing_order() {
    // descending within a link, and a per-link override that is neither
    // ascending nor contiguous: workers 2, 3, 7 (main lcore 0)
    let links = two_links(Some((json!([7, 2]), json!([3]))));
    let d = resolve_directions(&links, &probed(&IDS), 0, &[2, 3, 7]).unwrap();
    assert_eq!(d.iter().map(|x| x.lcore).collect::<Vec<_>>(), [7, 2, 3]);
    // the lcore belongs to the direction, not to the port ids or the ports
    assert_eq!(
        d.iter().map(|x| (x.rx_port, x.tx_port)).collect::<Vec<_>>(),
        [(0, 1), (1, 0), (2, 2)]
    );
    // the 2-port example of the spec: -l 0-2 with [2, 1]
    let links = links_of(vec![link_l("ab", &["P0", "P1"], &[1, 2], json!([2, 1]))]);
    let d = resolve_directions(&links, &probed(&IDS[..2]), 0, &[1, 2]).unwrap();
    assert_eq!((d[0].lcore, d[1].lcore), (2, 1));
}

#[test]
fn the_main_lcore_cannot_run_a_direction() {
    let links = links_of(vec![link_l("ab", &["P0", "P1"], &[1, 2], json!([0, 1]))]);
    let e = resolve_directions(&links, &probed(&IDS[..2]), 0, &[1, 2])
        .unwrap_err()
        .to_string();
    assert!(e.contains("main") && e.contains('0'), "{e}");
}

#[test]
fn an_lcore_that_is_not_an_eal_worker_is_an_error_naming_it() {
    let links = links_of(vec![link_l("ab", &["P0", "P1"], &[1, 2], json!([1, 9]))]);
    let e = resolve_directions(&links, &probed(&IDS[..2]), 0, &[1, 2])
        .unwrap_err()
        .to_string();
    assert!(e.contains('9') && e.contains("worker"), "{e}");
}

#[test]
fn every_worker_lcore_must_be_used() {
    // one loopback direction on lcore 1, but the EAL has workers 1 and 2
    let links = links_of(vec![link_l("l", &["P0"], &[1], json!([1]))]);
    let e = resolve_directions(&links, &probed(&IDS[..1]), 0, &[1, 2])
        .unwrap_err()
        .to_string();
    assert!(e.contains('2'), "the unused worker is named: {e}");
}

// ── fix round (spec v6): D1 unknown keys, D2 dict limit, D5 delay_stats ──
//
// D1: a key the format does not know is an error naming it, at the top level,
//     in a link and in a `histograms` entry (`#[serde(deny_unknown_fields)]`);
//     every documented key still parses.
// D2: `/dpds/ports` must fit DPDK's `RTE_TEL_MAX_DICT_ENTRIES` (256) keys:
//     `3 + 5 * ports + 10 * directions` (see unit/stats.rs, "ports_reply_has_three_plus
//     ..."). A links file that exceeds it is an error of `parse_links` naming the
//     maximum "256" (so it fails before any port is probed); a link has as many
//     directions as ports, so 16 ports fit and 17 do not.
// D5: `"delay_stats": [bool, ..]` per link, one entry per direction, default all true:
//     `LinkSpec.delay_stats: Vec<bool>` (ALWAYS `configs.len()` entries) and
//     `LinkDirection.delay_stats: bool`.

fn err_text(json: &str) -> String {
    match parse_links(json) {
        Ok(_) => panic!("expected an error for {json}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn an_unknown_key_in_a_link_is_an_error_naming_it() {
    // `lcore` instead of `lcores`: silently ignored before, which would start
    // the run on the wrong lcores
    let mut l = link("ab", &["p0", "p1"], &[1, 2]);
    l["lcore"] = json!([1, 2]);
    let e = err_text(&file(vec![l]));
    assert!(e.contains("lcore"), "{e}");
    assert!(e.contains("unknown field"), "serde's wording: {e}");
}

#[test]
fn the_typos_of_every_optional_link_key_are_caught() {
    for typo in [
        "histogram",
        "delay_stat",
        "delaystats",
        "lcore",
        "Name",
        "port",
        "config",
    ] {
        let mut l = link("ab", &["p0"], &[1]);
        l[typo] = json!([]);
        let e = err_text(&file(vec![l]));
        assert!(e.contains(typo), "{typo}: {e}");
    }
}

#[test]
fn an_unknown_top_level_key_is_an_error_naming_it() {
    let text = format!(
        r#"{{ "links": [{}], "lnks": [] }}"#,
        link("a", &["p"], &[1])
    );
    let e = err_text(&text);
    assert!(e.contains("lnks"), "{e}");
}

#[test]
fn an_unknown_key_in_a_histogram_entry_is_an_error_naming_it() {
    let l = link_h("ab", &["p0"], &[1], json!([{ "bins": 10, "lo": 5.0 }]));
    let e = err_text(&file(vec![l]));
    assert!(e.contains("lo"), "{e}");
    let l = link_h("ab", &["p0"], &[1], json!([{ "bin_ns": 10 }]));
    assert!(err_text(&file(vec![l])).contains("bin_ns"));
}

#[test]
fn every_documented_key_is_still_accepted() {
    let mut l = link("ab", &["p0", "p1"], &[1, 2]);
    l["histograms"] = json!([{ "bins": 10, "lo_ns": 0, "hi_ns": 100.0 }, {}]);
    l["lcores"] = json!([2, 1]);
    l["delay_stats"] = json!([true, false]);
    let links = parse_links(&file(vec![l])).unwrap();
    assert_eq!(links[0].lcores, Some(vec![2, 1]));
    assert_eq!(links[0].delay_stats, [true, false]);
}

#[test]
fn delay_stats_default_to_on_for_every_direction() {
    let links = parse_links(&spec_file()).unwrap();
    assert_eq!(links[0].delay_stats, [true, true]);
    assert_eq!(links[1].delay_stats, [true]);
}

#[test]
fn delay_stats_are_parsed_per_direction() {
    let mut a = link("ab", &["p0", "p1"], &[1, 2]);
    a["delay_stats"] = json!([false, true]);
    let mut b = link("l", &["p2"], &[3]);
    b["delay_stats"] = json!([false]);
    let c = link("c", &["p3", "p4"], &[1, 1]);
    let links = parse_links(&file(vec![a, b, c])).unwrap();
    assert_eq!(links[0].delay_stats, [false, true]);
    assert_eq!(links[1].delay_stats, [false]);
    assert_eq!(links[2].delay_stats, [true, true]);
}

#[test]
fn delay_stats_of_the_wrong_length_or_type_are_errors_naming_the_link() {
    for (bad, what) in [
        (json!([true]), "one entry for two directions"),
        (
            json!([true, true, true]),
            "three entries for two directions",
        ),
        (json!([]), "empty"),
    ] {
        let mut l = link("lenlink", &["p0", "p1"], &[1, 2]);
        l["delay_stats"] = bad;
        let e = err_of(vec![l]);
        assert!(e.contains("lenlink"), "{what}: {e}");
        assert!(e.contains("delay_stats"), "{what}: {e}");
    }
    for bad in [json!([1, 0]), json!(["true", "false"]), json!(true)] {
        let mut l = link("typelink", &["p0", "p1"], &[1, 2]);
        l["delay_stats"] = bad.clone();
        assert!(parse_links(&file(vec![l])).is_err(), "{bad}");
    }
}

#[test]
fn delay_stats_may_be_mixed_with_and_without_lcores() {
    // unlike `lcores`, `delay_stats` is per link and optional everywhere
    let mut a = link_l("a", &["p0"], &[1], json!([1]));
    a["delay_stats"] = json!([false]);
    let b = link_l("b", &["p1"], &[1], json!([2]));
    let links = parse_links(&file(vec![a, b])).unwrap();
    assert_eq!(links[0].delay_stats, [false]);
    assert_eq!(links[1].delay_stats, [true]);
}

#[test]
fn directions_carry_the_delay_stats_switch_of_their_config_slot() {
    // ab: [on, off], l: [off], c: [on]
    let mut a = link("ab", &["P0", "P1"], &[1, 2]);
    a["delay_stats"] = json!([true, false]);
    let mut b = link("l", &["P2"], &[3]);
    b["delay_stats"] = json!([false]);
    let c = link("c", &["P3"], &[4]);
    let links = links_of(vec![a, b, c]);
    let ids = probed(&[("P0", 0), ("P1", 1), ("P2", 2), ("P3", 3)]);
    let d = resolve_directions(&links, &ids, 0, &[1, 2, 3, 4]).unwrap();
    assert_eq!(
        d.iter().map(|x| x.delay_stats).collect::<Vec<_>>(),
        [true, false, false, true]
    );
}

/// `n` loopback links `l0 .. l<n-1>` on ports `p0 .. p<n-1>` (n ports, n directions).
fn loopbacks(n: usize) -> String {
    file(
        (0..n)
            .map(|i| link(&format!("l{i}"), &[&format!("p{i}")], &[1]))
            .collect(),
    )
}

/// `n` two-port links (2n ports, 2n directions).
fn pairs(n: usize) -> String {
    file(
        (0..n)
            .map(|i| {
                link(
                    &format!("l{i}"),
                    &[&format!("a{i}"), &format!("b{i}")],
                    &[1, 1],
                )
            })
            .collect(),
    )
}

#[test]
fn sixteen_ports_fit_the_dpdk_dict_limit() {
    // 3 + 5 * 16 + 10 * 16 = 243 <= 256
    assert_eq!(parse_links(&loopbacks(16)).unwrap().len(), 16);
    assert_eq!(parse_links(&pairs(8)).unwrap().len(), 8);
}

#[test]
fn seventeen_ports_exceed_the_limit_and_the_error_names_the_maximum() {
    // 3 + 5 * 17 + 10 * 17 = 258 > 256
    let e = err_text(&loopbacks(17));
    assert!(e.contains("256"), "the maximum is named: {e}");
    let e = err_text(&pairs(9)); // 18 ports
    assert!(e.contains("256"), "{e}");
    let e = err_text(&loopbacks(40));
    assert!(e.contains("256"), "{e}");
}
