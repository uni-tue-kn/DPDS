//! The `--links <file>` file (v2, directed links): parsing, validation and
//! the numbering of the directions.
//!
//! # Test-facing API (module `dpds::links`)
//!
//! A link is ONE direction: packets arriving on `ingress` leave on `egress`
//! with the link's one config. `ingress == egress` (a hairpin on one port) is
//! allowed. Across all links a port appears at most once as ingress and at
//! most once as egress. Direction index = link index (file order).
//!
//! ```text
//! pub const MAX_NAME_LEN: usize = 32;
//! pub struct LinkSpec { pub name: String, pub ingress: String, pub egress: String,
//!                       pub config: dpds_core::Config, pub lcore: Option<u32>,
//!                       pub histogram: dpds::delay_stats::HistogramSpec,
//!                       pub delay_stats: bool }
//! pub fn parse_links(json: &str) -> Result<Vec<LinkSpec>, LinksError>
//!     JSON `{ "links": [ { "name", "ingress", "egress", "config": {..},
//!                          "lcore": 3, "histogram": {"bins", "lo_ns", "hi_ns"},
//!                          "delay_stats": true }, .. ] }`; `lcore`, `histogram`
//!     (and each of its fields) and `delay_stats` are optional (defaults: None,
//!     `HistogramSpec::default()`, true). File-level validation:
//!       - invalid JSON / missing "links" / a bad Config / a missing required key
//!         (the error names the key; `config.delayer.initialization_rate_pps` is
//!         required too)                           -> Err
//!       - no link at all (`"links": []`)                      -> Err
//!       - an unknown key at any level (`#[serde(deny_unknown_fields)]`), in
//!         particular the v1 keys `ports`, `configs`, `lcores`, `histograms`
//!                                                              -> Err naming the key
//!       - name empty, longer than 32, or a char outside [A-Za-z0-9_.-] -> Err naming the link
//!       - duplicate name                                       -> Err naming the name
//!       - an empty ingress or egress string                    -> Err naming the link
//!       - a port that is the ingress of two links              -> Err naming the port and "ingress"
//!       - a port that is the egress of two links               -> Err naming the port and "egress"
//!         (the same port once as ingress and once as egress, in one link
//!         (hairpin) or in two (ring), is fine)
//!       - some links with `lcore` and some without              -> Err naming "lcore"
//!       - the same lcore on two links                           -> Err naming the lcore
//!       - `histogram`: `bins` not an integer in 1..=4096; `lo_ns`/`hi_ns`
//!         negative or not a number; both given and `lo_ns >= hi_ns` -> Err naming the link
//!       - the /dpds/ports dict limit: `3 + 5 * ports + 10 * links` keys must be
//!         <= 256 (`ports` = distinct ports named by the links); the error names "256"
//! pub fn load_links(path: &Path) -> Result<Vec<LinkSpec>, LinksError>   (read + parse_links;
//!     an unreadable file is an Err whose Display contains the path)
//! #[derive(Debug, Clone)]
//! pub struct LinkDirection { pub rx_port: u16, pub tx_port: u16,
//!                            pub config: dpds_core::Config,
//!                            pub histogram: dpds::delay_stats::HistogramSpec,
//!                            pub delay_stats: bool,
//!                            pub lcore: u32 }   (the worker lcore that runs the link; more
//!                            fields, e.g. `link`, may exist, these tests do not read them)
//! pub fn resolve_directions(links: &[LinkSpec], probed: &[(String, u16)],
//!                           main_lcore: u32, workers: &[u32])
//!     -> Result<Vec<LinkDirection>, LinksError>
//!     (`workers` = the EAL worker lcores in ascending order, main lcore excluded)
//!     `probed` = (DPDK device name, port id) of every probed ethdev (no reliance on
//!     port-id order). Returns exactly one direction per link, in file order:
//!     rx_port = id of `ingress`, tx_port = id of `egress`.
//!       - an ingress/egress that is not probed   -> Err naming the port
//!       - a probed port that no link uses (neither as ingress nor as egress)
//!                                                 -> Err naming the port (device name)
//!       - a probed port used only as egress (or only as ingress) is fine
//!       - (duplicate ingress/egress is an error of `parse_links` or of this
//!         function; the tests accept either place)
//! LinksError: Debug + Display + std::error::Error
//!
//! # Lcores
//! `lcore` is all-or-none across the links (`parse_links`). With explicit lcores
//! `LinkDirection.lcore` is exactly the listed one (no sorting; link 0 on 3,
//! link 1 on 1 is fine); `resolve_directions` errors: an lcore equal to
//! `main_lcore` (message contains "main"), an lcore not in `workers` (contains
//! the number and "worker"), a worker lcore that no link uses (contains its
//! number). Without `lcore` anywhere: link i runs on `workers[i]`; a different
//! number of workers than links is an error. The lcore rule at startup is
//! `1 + number of links` (`dpds::setup::validate_lcore_count(1 + links, links)`).
//! ```

use std::collections::BTreeSet;
use std::path::Path;

use dpds::delay_stats::HistogramSpec;
use dpds::links::{
    load_links, parse_links, resolve_directions, LinkDirection, LinkSpec, LinksError, MAX_NAME_LEN,
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
            "half_life_period_ns": 1.0,
            "initialization_rate_pps": 15000000.0
        },
        "limiter": {
            "mode": "Spacer",
            "activate": false,
            "rate_bps": 1e9
        },
        "loss_model": { "Random": 0.0 }
    })
}

/// A v2 link: `ingress` -> `egress` with a config of buffer capacity `cap`.
fn link(name: &str, ingress: &str, egress: &str, cap: u64) -> Value {
    json!({ "name": name, "ingress": ingress, "egress": egress, "config": cfg(cap) })
}

fn link_l(name: &str, ingress: &str, egress: &str, cap: u64, lcore: Value) -> Value {
    let mut l = link(name, ingress, egress, cap);
    l["lcore"] = lcore;
    l
}

fn link_h(name: &str, ingress: &str, egress: &str, cap: u64, hist: Value) -> Value {
    let mut l = link(name, ingress, egress, cap);
    l["histogram"] = hist;
    l
}

fn file(links: Vec<Value>) -> String {
    json!({ "links": links }).to_string()
}

fn err_of(links: Vec<Value>) -> String {
    err_text(&file(links))
}

fn err_text(json: &str) -> String {
    match parse_links(json) {
        Ok(_) => panic!("expected an error for {json}"),
        Err(e) => e.to_string(),
    }
}

fn links_of(links: Vec<Value>) -> Vec<LinkSpec> {
    parse_links(&file(links)).unwrap()
}

/// The ring `r0: p0 -> p1, r1: p1 -> p2, .., r<n-1>: p<n-1> -> p0` (n ports,
/// n links; n = 1 is a hairpin).
fn ring(n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| {
            link(
                &format!("r{i}"),
                &format!("p{i}"),
                &format!("p{}", (i + 1) % n),
                1,
            )
        })
        .collect()
}

/// `n` one-way links `w<i>: a<i> -> b<i>` (2n ports, n links).
fn one_way(n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| link(&format!("w{i}"), &format!("a{i}"), &format!("b{i}"), 1))
        .collect()
}

/// The classic bidirectional pair as two links.
fn pair(a: &str, b: &str) -> Vec<Value> {
    vec![
        link(&format!("{a}-to-{b}"), a, b, 11),
        link(&format!("{b}-to-{a}"), b, a, 22),
    ]
}

/// The documented example file: a pair (P0 <-> P1) and a hairpin on P2.
fn spec_file() -> String {
    let mut links = pair("P0", "P1");
    links.push(link("loop", "P2", "P2", 33));
    file(links)
}

// ── parsing ─────────────────────────────────────────────────────────

#[test]
fn the_documented_file_parses_with_its_configs_in_order() {
    let links = parse_links(&spec_file()).unwrap();
    assert_eq!(links.len(), 3);
    assert_eq!(links[0].name, "P0-to-P1");
    assert_eq!(
        (links[0].ingress.as_str(), links[0].egress.as_str()),
        ("P0", "P1")
    );
    assert_eq!(links[0].config.buffer_capacity, 11);
    assert_eq!(links[0].config.delayer.mean_ns, 1000.0);
    assert_eq!(links[1].name, "P1-to-P0");
    assert_eq!(
        (links[1].ingress.as_str(), links[1].egress.as_str()),
        ("P1", "P0")
    );
    assert_eq!(links[1].config.buffer_capacity, 22);
    assert_eq!(links[2].name, "loop");
    assert_eq!(
        (links[2].ingress.as_str(), links[2].egress.as_str()),
        ("P2", "P2")
    );
    assert_eq!(links[2].config.buffer_capacity, 33);
}

#[test]
fn the_optional_keys_default_to_none_default_spec_and_delay_stats_on() {
    let links = parse_links(&spec_file()).unwrap();
    for l in &links {
        assert_eq!(l.lcore, None);
        assert_eq!(l.histogram, HistogramSpec::default());
        assert!(l.delay_stats);
    }
}

#[test]
fn a_config_may_use_every_dpds_core_field_incl_the_seed() {
    let mut c = cfg(5);
    c["seed"] = json!(42);
    c["loss_model"] = json!({ "GilbertElliot": [0.01, 0.3, 1.0, 0.5, true] });
    let l = json!({ "name": "x", "ingress": "p", "egress": "q", "config": c });
    let links = parse_links(&file(vec![l])).unwrap();
    assert_eq!(links[0].config.seed, Some(42));
}

#[test]
fn syntax_and_structure_errors_are_errors_not_panics() {
    for bad in [
        "",
        "not json",
        "{}",
        r#"{"links": 3}"#,
        r#"{"links": [{}]}"#,
        r#"{"links": [{"name": "a", "ingress": "p", "egress": "q"}]}"#,
        r#"{"links": [{"name": "a", "ingress": "p", "egress": "q", "config": {"buffer_capacity": 1}}]}"#,
        r#"{"links": [{"name": 1, "ingress": "p", "egress": "q", "config": {}}]}"#,
        r#"{"links": [{"name": "a", "ingress": 1, "egress": "q", "config": {}}]}"#,
        r#"{"links": [{"name": "a", "ingress": ["p"], "egress": "q", "config": {}}]}"#,
    ] {
        assert!(parse_links(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_missing_required_key_is_an_error_naming_it() {
    for missing in ["name", "ingress", "egress", "config"] {
        let mut l = link("x", "p", "q", 1);
        l.as_object_mut().unwrap().remove(missing);
        let e = err_of(vec![l]);
        assert!(e.contains(missing), "{missing}: {e}");
    }
}

#[test]
fn a_config_without_initialization_rate_pps_is_an_error_naming_the_field() {
    // Required in every link config (no default), also with correlation off.
    let mut l = link("x", "p", "q", 1);
    l["config"]["delayer"]
        .as_object_mut()
        .unwrap()
        .remove("initialization_rate_pps");
    let e = err_of(vec![l]);
    assert!(e.contains("initialization_rate_pps"), "{e}");
}

#[test]
fn the_initialization_rate_of_a_link_config_is_read_per_link() {
    let mut a = link("a", "p", "q", 1);
    a["config"]["delayer"]["initialization_rate_pps"] = json!(2.5e6);
    let mut b = link("b", "q", "p", 1);
    b["config"]["delayer"]["initialization_rate_pps"] = json!(40_000);
    let links = parse_links(&file(vec![a, b])).unwrap();
    assert_eq!(links[0].config.delayer.initialization_rate_pps, 2.5e6);
    assert_eq!(links[1].config.delayer.initialization_rate_pps, 40_000.0);
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
            parse_links(&file(vec![link(good, "p", "q", 1)])).is_ok(),
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
        let e = err_of(vec![link(bad, "p", "q", 1)]);
        assert!(!e.is_empty(), "{bad:?}");
    }
}

#[test]
fn a_duplicate_name_is_an_error_naming_it() {
    let e = err_of(vec![link("same", "p", "q", 1), link("same", "r", "s", 1)]);
    assert!(e.contains("same"), "{e}");
}

#[test]
fn an_empty_ingress_or_egress_is_an_error_naming_the_link() {
    let e = err_of(vec![link("blank-in", "", "q", 1)]);
    assert!(e.contains("blank-in"), "{e}");
    let e = err_of(vec![link("blank-out", "p", "", 1)]);
    assert!(e.contains("blank-out"), "{e}");
}

#[test]
fn a_ring_a_hairpin_and_a_bidirectional_pair_are_valid_files() {
    // the user's example: A -> B, B -> C, C -> A
    let links = links_of(ring(3));
    let ends: Vec<(&str, &str)> = links
        .iter()
        .map(|l| (l.ingress.as_str(), l.egress.as_str()))
        .collect();
    assert_eq!(ends, [("p0", "p1"), ("p1", "p2"), ("p2", "p0")]);
    // a hairpin on one port
    let links = links_of(ring(1));
    assert_eq!(
        (links[0].ingress.as_str(), links[0].egress.as_str()),
        ("p0", "p0")
    );
    // two links between the same two ports, one per direction
    assert_eq!(links_of(pair("a", "b")).len(), 2);
    // a port that is only ever an egress, and one that is only an ingress
    assert_eq!(links_of(one_way(3)).len(), 3);
    assert!(parse_links(&file(vec![
        link("fan", "a", "b", 1),
        link("fan2", "c", "b2", 1)
    ]))
    .is_ok());
}

#[test]
fn a_port_may_be_ingress_of_one_link_and_egress_of_another_with_different_partners() {
    // b: egress of a-to-b, ingress of b-to-c; a and c are different partners
    let links = links_of(vec![
        link("a-to-b", "a", "b", 1),
        link("b-to-c", "b", "c", 2),
    ]);
    assert_eq!(links.len(), 2);
}

/// What a port conflict is reported as: by `parse_links`, or else by
/// `resolve_directions` with every named port probed.
fn conflict_of(links: Vec<Value>) -> String {
    let text = file(links);
    let specs = match parse_links(&text) {
        Err(e) => return e.to_string(),
        Ok(specs) => specs,
    };
    let names: BTreeSet<&str> = specs
        .iter()
        .flat_map(|l| [l.ingress.as_str(), l.egress.as_str()])
        .collect();
    let ids: Vec<(String, u16)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.to_string(), i as u16))
        .collect();
    let workers: Vec<u32> = (1..=specs.len() as u32).collect();
    match resolve_directions(&specs, &ids, 0, &workers) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("a port conflict must be an error: {text}"),
    }
}

#[test]
fn a_port_used_twice_as_ingress_is_an_error_naming_it() {
    let e = conflict_of(vec![
        link("one", "0000:65:00.0", "x", 1),
        link("two", "0000:65:00.0", "y", 1),
    ]);
    assert!(e.contains("0000:65:00.0"), "{e}");
    assert!(e.contains("ingress"), "{e}");
    // a hairpin's ingress counts too
    let e = conflict_of(vec![
        link("hp", "net_null0", "net_null0", 1),
        link("two", "net_null0", "y", 1),
    ]);
    assert!(e.contains("net_null0") && e.contains("ingress"), "{e}");
}

#[test]
fn a_port_used_twice_as_egress_is_an_error_naming_it() {
    let e = conflict_of(vec![
        link("one", "x", "0000:65:00.1", 1),
        link("two", "y", "0000:65:00.1", 1),
    ]);
    assert!(e.contains("0000:65:00.1"), "{e}");
    assert!(e.contains("egress"), "{e}");
    // a hairpin's egress counts too
    let e = conflict_of(vec![
        link("hp", "net_null0", "net_null0", 1),
        link("two", "x", "net_null0", 1),
    ]);
    assert!(e.contains("net_null0") && e.contains("egress"), "{e}");
}

#[test]
fn a_port_used_twice_as_ingress_and_twice_as_egress_is_an_error_naming_it() {
    // p is ingress of both links and egress of both: two links p -> p
    let e = conflict_of(vec![link("a", "p", "p", 1), link("b", "p", "p", 1)]);
    assert!(e.contains('p'), "{e}");
    let e = conflict_of(vec![
        link("a", "net_null0", "net_null1", 1),
        link("b", "net_null0", "net_null1", 1),
    ]);
    assert!(e.contains("net_null0") || e.contains("net_null1"), "{e}");
}

#[test]
fn the_same_ingress_and_egress_pair_listed_twice_is_a_conflict() {
    let e = conflict_of(vec![link("a", "x", "y", 1), link("b", "x", "y", 1)]);
    assert!(e.contains('x') || e.contains('y'), "{e}");
}

#[test]
fn the_v1_keys_are_unknown_fields_naming_the_key() {
    // the old shape: ports / configs / lcores / histograms (no compatibility)
    let old = r#"{ "links": [ { "name": "ab", "ports": ["p0", "p1"], "configs": [] } ] }"#;
    let e = err_text(old);
    assert!(e.contains("ports"), "{e}");
    assert!(e.contains("unknown field"), "serde's wording: {e}");
    for key in ["ports", "configs", "lcores", "histograms"] {
        let mut l = link("ab", "p0", "p1", 1);
        l[key] = json!([]);
        let e = err_of(vec![l]);
        assert!(e.contains(key), "{key}: {e}");
        assert!(e.contains("unknown field"), "{key}: {e}");
    }
}

#[test]
fn load_links_reads_a_file_and_reports_a_missing_one_with_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("links.json");
    std::fs::write(&good, spec_file()).unwrap();
    assert_eq!(load_links(&good).unwrap().len(), 3);

    let missing = dir.path().join("nope.json");
    let e = load_links(&missing).unwrap_err().to_string();
    assert!(e.contains("nope.json"), "{e}");

    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "{ not json").unwrap();
    assert!(load_links(&bad).is_err());
    assert!(load_links(Path::new(dir.path())).is_err(), "a directory");

    // a v1 file is an error, naming the file as well as the key
    let old = dir.path().join("old.json");
    std::fs::write(
        &old,
        r#"{ "links": [ { "name": "a", "ports": ["p"], "configs": [] } ] }"#,
    )
    .unwrap();
    let e = load_links(&old).unwrap_err().to_string();
    assert!(e.contains("old.json") && e.contains("ports"), "{e}");
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

/// Main lcore 0 and the workers 1..=links (the `-l 0-n` layout).
fn resolve(links: &[LinkSpec], ids: &[(String, u16)]) -> Result<Vec<LinkDirection>, LinksError> {
    let workers: Vec<u32> = (1..=links.len() as u32).collect();
    resolve_directions(links, ids, 0, &workers)
}

#[test]
fn there_is_one_direction_per_link_in_file_order() {
    let mut v = pair("P0", "P1");
    v.push(link("loop", "P2", "P2", 33));
    v.extend([
        link("c-to-d", "P3", "P4", 44),
        link("d-to-c", "P4", "P3", 55),
    ]);
    let links = links_of(v);
    let ids = probed(&[("P0", 0), ("P1", 1), ("P2", 2), ("P3", 3), ("P4", 4)]);
    let d = resolve(&links, &ids).unwrap();
    assert_eq!(d.len(), links.len());
    let got: Vec<(u16, u16, usize)> = d
        .iter()
        .map(|x| (x.rx_port, x.tx_port, x.config.buffer_capacity))
        .collect();
    assert_eq!(
        got,
        [
            (0, 1, 11), // P0 -> P1
            (1, 0, 22), // P1 -> P0
            (2, 2, 33), // hairpin: rx = tx
            (3, 4, 44),
            (4, 3, 55),
        ]
    );
    // without explicit lcores link i runs on worker i (here 1..=5)
    assert_eq!(
        d.iter().map(|x| x.lcore).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    // the lcore rule: 1 + links
    assert!(dpds::setup::validate_lcore_count(1 + d.len(), d.len()).is_ok());
}

#[test]
fn a_ring_of_three_ports_maps_each_link_to_its_own_ingress_and_egress() {
    // A -> B, B -> C, C -> A
    let links = links_of(vec![
        link("a-to-b", "A", "B", 1),
        link("b-to-c", "B", "C", 2),
        link("c-to-a", "C", "A", 3),
    ]);
    let ids = probed(&[("A", 0), ("B", 1), ("C", 2)]);
    let d = resolve(&links, &ids).unwrap();
    let got: Vec<(u16, u16, usize)> = d
        .iter()
        .map(|x| (x.rx_port, x.tx_port, x.config.buffer_capacity))
        .collect();
    assert_eq!(got, [(0, 1, 1), (1, 2, 2), (2, 0, 3)]);
    // every RX queue and every TX queue has exactly one worker
    let rx: BTreeSet<u16> = d.iter().map(|x| x.rx_port).collect();
    let tx: BTreeSet<u16> = d.iter().map(|x| x.tx_port).collect();
    assert_eq!((rx.len(), tx.len()), (3, 3));
}

#[test]
fn ports_are_resolved_by_device_name_not_by_port_id_order() {
    // The links list the later-created / higher-addressed device first; the
    // port ids are not in listing order and not alphabetical either.
    let links = links_of(vec![
        link("x", "0000:83:00.0", "0000:17:00.0", 100),
        link("y", "0000:17:00.0", "0000:83:00.0", 200),
        link("z", "net_null9", "net_null9", 300),
    ]);
    let ids = probed(&[("0000:17:00.0", 0), ("0000:83:00.0", 1), ("net_null9", 2)]);
    let d = resolve(&links, &ids).unwrap();
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
fn an_ingress_or_egress_that_is_not_probed_is_an_error_naming_it() {
    let links = links_of(vec![link("ab", "P0", "P9", 1)]);
    let e = resolve(&links, &probed(&[("P0", 0)]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("P9"), "egress: {e}");
    let links = links_of(vec![link("ab", "P9", "P0", 1)]);
    let e = resolve(&links, &probed(&[("P0", 0)]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("P9"), "ingress: {e}");
}

#[test]
fn a_probed_port_that_no_link_uses_is_an_error_naming_it() {
    let links = links_of(vec![link("ab", "P0", "P1", 1)]);
    let e = resolve(&links, &probed(&[("P0", 0), ("P1", 1), ("P2", 2)]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("P2"), "{e}");
    // a ring of three with a fourth probed port
    let links = links_of(vec![
        link("a", "P0", "P1", 1),
        link("b", "P1", "P2", 1),
        link("c", "P2", "P0", 1),
    ]);
    let e = resolve(
        &links,
        &probed(&[("P0", 0), ("P1", 1), ("P2", 2), ("spare", 3)]),
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("spare"), "{e}");
}

#[test]
fn a_single_hairpin_link_works() {
    let links = links_of(vec![link("l", "net_null0", "net_null0", 7)]);
    let d = resolve(&links, &probed(&[("net_null0", 0)])).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].rx_port, d[0].tx_port), (0, 0));
    assert_eq!(d[0].config.buffer_capacity, 7);
}

#[test]
fn a_probed_port_used_only_as_egress_or_only_as_ingress_works() {
    // P1 is only an egress, P0 only an ingress
    let links = links_of(vec![link("one-way", "P0", "P1", 5)]);
    let d = resolve(&links, &probed(&[("P0", 0), ("P1", 1)])).unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].rx_port, d[0].tx_port), (0, 1));
    // fan-in to one egress from different ingress ports is NOT allowed, but a
    // chain where the middle port is both is
    let links = links_of(vec![link("a", "P0", "P1", 1), link("b", "P1", "P2", 2)]);
    let d = resolve(&links, &probed(&[("P0", 0), ("P1", 1), ("P2", 2)])).unwrap();
    assert_eq!(
        d.iter().map(|x| (x.rx_port, x.tx_port)).collect::<Vec<_>>(),
        [(0, 1), (1, 2)]
    );
}

// ── optional per-link histogram ─────────────────────────────────────

fn spec(bins: Option<usize>, lo: Option<f64>, hi: Option<f64>) -> HistogramSpec {
    HistogramSpec {
        bins,
        lo_ns: lo,
        hi_ns: hi,
    }
}

#[test]
fn a_histogram_is_parsed_per_link_with_every_field_optional() {
    let links = links_of(vec![
        link_h("a", "p0", "p1", 1, json!({ "bins": 100 })),
        link_h("b", "p1", "p2", 2, json!({ "lo_ns": 10.0, "hi_ns": 20.5 })),
        link_h("c", "p3", "p3", 3, json!({})),
        link_h(
            "d",
            "p4",
            "p0",
            4,
            json!({ "bins": 500, "lo_ns": 0, "hi_ns": 1e6 }),
        ),
    ]);
    assert_eq!(links[0].histogram, spec(Some(100), None, None));
    assert_eq!(links[1].histogram, spec(None, Some(10.0), Some(20.5)));
    assert_eq!(links[2].histogram, HistogramSpec::default());
    assert_eq!(links[3].histogram, spec(Some(500), Some(0.0), Some(1e6)));
}

#[test]
fn the_documented_limits_are_inclusive() {
    for h in [
        json!({ "bins": 1 }),
        json!({ "bins": 500 }),
        json!({ "bins": 501 }),
        json!({ "bins": 4096 }),
        json!({ "lo_ns": 0.0 }),
        json!({ "lo_ns": 0.0, "hi_ns": 0.001 }),
    ] {
        assert!(
            parse_links(&file(vec![link_h("x", "p", "q", 1, h.clone())])).is_ok(),
            "{h}"
        );
    }
}

#[test]
fn bad_histograms_are_errors_naming_the_link() {
    for bad in [
        json!({ "bins": 0 }),
        json!({ "bins": 4097 }),
        json!({ "bins": 100000 }),
        json!({ "lo_ns": -1.0 }),
        json!({ "hi_ns": -1.0 }),
        json!({ "lo_ns": 5.0, "hi_ns": 5.0 }),
        json!({ "lo_ns": 9.0, "hi_ns": 5.0 }),
    ] {
        let e = err_of(vec![link_h("hist-link", "p", "q", 1, bad.clone())]);
        assert!(e.contains("hist-link"), "{bad}: {e}");
    }
    // spec 30: a bins value above the maximum names the maximum
    for bins in [4097, 100_000] {
        let e = err_of(vec![link_h(
            "hist-link",
            "p",
            "q",
            1,
            json!({ "bins": bins }),
        )]);
        assert!(e.contains("4096"), "bins {bins}: {e}");
    }
    // values of the wrong JSON type are errors too (the message need not name the link)
    for bad in [
        json!({ "bins": -1 }),
        json!({ "bins": 2.5 }),
        json!({ "bins": "100" }),
        json!({ "lo_ns": "x" }),
    ] {
        assert!(
            parse_links(&file(vec![link_h("x", "p", "q", 1, bad.clone())])).is_err(),
            "{bad}"
        );
    }
    // not an object (the v1 array form is gone)
    for bad in [json!([]), json!([{}]), json!(3), json!("x")] {
        assert!(
            parse_links(&file(vec![link_h("x", "p", "q", 1, bad.clone())])).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn directions_carry_the_histogram_spec_of_their_link() {
    let links = links_of(vec![
        link_h("a", "P0", "P1", 11, json!({ "bins": 10 })),
        link_h("b", "P1", "P2", 22, json!({ "lo_ns": 1.0, "hi_ns": 2.0 })),
        link("c", "P2", "P0", 33),
    ]);
    let ids = probed(&[("P0", 0), ("P1", 1), ("P2", 2)]);
    let d = resolve(&links, &ids).unwrap();
    assert_eq!(d[0].histogram, spec(Some(10), None, None));
    assert_eq!(d[1].histogram, spec(None, Some(1.0), Some(2.0)));
    assert_eq!(d[2].histogram, HistogramSpec::default());
}

// ── explicit lcores (all-or-none) ───────────────────────────────────

#[test]
fn lcore_is_optional_and_parsed_per_link() {
    let links = links_of(vec![
        link_l("a", "p0", "p1", 1, json!(7)),
        link_l("b", "p1", "p0", 2, json!(2)),
        link_l("c", "p2", "p2", 3, json!(3)),
    ]);
    assert_eq!(
        links[0].lcore,
        Some(7),
        "no sorting, the listed value is kept"
    );
    assert_eq!(links[1].lcore, Some(2));
    assert_eq!(links[2].lcore, Some(3));
}

#[test]
fn a_repeated_lcore_is_an_error_naming_it() {
    let e = err_of(vec![
        link_l("x", "a", "b", 1, json!(4)),
        link_l("y", "b", "a", 2, json!(4)),
    ]);
    assert!(e.contains('4') && e.contains("lcore"), "{e}");
}

#[test]
fn mixing_links_with_and_without_lcore_is_an_error() {
    for links in [
        vec![
            link_l("with", "a", "b", 1, json!(1)),
            link("without", "b", "a", 2),
        ],
        vec![
            link("without", "a", "b", 1),
            link_l("with", "b", "a", 2, json!(1)),
        ],
    ] {
        let e = err_of(links);
        assert!(e.contains("lcore"), "{e}");
    }
}

#[test]
fn lcore_of_the_wrong_json_type_is_an_error() {
    for bad in [
        json!(-1),
        json!(1.5),
        json!("1"),
        json!([1]),
        json!([]),
        json!(true),
        json!({}),
    ] {
        assert!(
            parse_links(&file(vec![link_l("x", "a", "b", 1, bad.clone())])).is_err(),
            "{bad}"
        );
    }
}

fn ring_links(lcores: Option<[u32; 3]>) -> Vec<LinkSpec> {
    let mut v = vec![
        link("a-to-b", "P0", "P1", 11),
        link("b-to-c", "P1", "P2", 22),
        link("c-to-a", "P2", "P0", 33),
    ];
    if let Some(l) = lcores {
        for (link, lcore) in v.iter_mut().zip(l) {
            link["lcore"] = json!(lcore);
        }
    }
    links_of(v)
}

const IDS: [(&str, u16); 3] = [("P0", 0), ("P1", 1), ("P2", 2)];

#[test]
fn without_lcores_link_i_runs_on_the_i_th_worker_in_ascending_order() {
    let links = ring_links(None);
    let d = resolve_directions(&links, &probed(&IDS), 1, &[3, 5, 9]).unwrap();
    assert_eq!(d.iter().map(|x| x.lcore).collect::<Vec<_>>(), [3, 5, 9]);
    // a different number of workers than links is an error
    assert!(resolve_directions(&links, &probed(&IDS), 0, &[1, 2]).is_err());
    assert!(resolve_directions(&links, &probed(&IDS), 0, &[1, 2, 3, 4]).is_err());
}

#[test]
fn explicit_lcores_are_honoured_literally_even_in_descending_order() {
    // workers 1, 2, 3 (main lcore 0); the links run on 3, 1, 2
    let links = ring_links(Some([3, 1, 2]));
    let d = resolve_directions(&links, &probed(&IDS), 0, &[1, 2, 3]).unwrap();
    assert_eq!(d.iter().map(|x| x.lcore).collect::<Vec<_>>(), [3, 1, 2]);
    // the lcore belongs to the link, not to the port ids
    assert_eq!(
        d.iter().map(|x| (x.rx_port, x.tx_port)).collect::<Vec<_>>(),
        [(0, 1), (1, 2), (2, 0)]
    );
    // a per-link override that is neither ascending nor contiguous: workers 2, 3, 7
    let links = ring_links(Some([7, 2, 3]));
    let d = resolve_directions(&links, &probed(&IDS), 0, &[2, 3, 7]).unwrap();
    assert_eq!(d.iter().map(|x| x.lcore).collect::<Vec<_>>(), [7, 2, 3]);
    // the classic pair, `-l 0-2` with link 0 on 2 and link 1 on 1
    let links = links_of(vec![
        link_l("ab", "P0", "P1", 1, json!(2)),
        link_l("ba", "P1", "P0", 2, json!(1)),
    ]);
    let d = resolve_directions(&links, &probed(&IDS[..2]), 0, &[1, 2]).unwrap();
    assert_eq!((d[0].lcore, d[1].lcore), (2, 1));
}

#[test]
fn the_main_lcore_cannot_run_a_link() {
    let links = links_of(vec![
        link_l("ab", "P0", "P1", 1, json!(0)),
        link_l("ba", "P1", "P0", 2, json!(1)),
    ]);
    let e = resolve_directions(&links, &probed(&IDS[..2]), 0, &[1, 2])
        .unwrap_err()
        .to_string();
    assert!(e.contains("main") && e.contains('0'), "{e}");
}

#[test]
fn an_lcore_that_is_not_an_eal_worker_is_an_error_naming_it() {
    let links = links_of(vec![
        link_l("ab", "P0", "P1", 1, json!(1)),
        link_l("ba", "P1", "P0", 2, json!(9)),
    ]);
    let e = resolve_directions(&links, &probed(&IDS[..2]), 0, &[1, 2])
        .unwrap_err()
        .to_string();
    assert!(e.contains('9') && e.contains("worker"), "{e}");
}

#[test]
fn every_worker_lcore_must_be_used() {
    // one hairpin link on lcore 1, but the EAL has workers 1 and 2
    let links = links_of(vec![link_l("l", "P0", "P0", 1, json!(1))]);
    let e = resolve_directions(&links, &probed(&IDS[..1]), 0, &[1, 2])
        .unwrap_err()
        .to_string();
    assert!(e.contains('2'), "the unused worker is named: {e}");
}

// ── unknown keys ────────────────────────────────────────────────────

#[test]
fn a_typo_of_an_optional_link_key_is_an_error_naming_it() {
    for typo in [
        "histograms",
        "histogram_",
        "delay_stat",
        "delaystats",
        "lcores",
        "Name",
        "port",
        "configs",
        "Config",
        "ingres",
        "egres",
    ] {
        let mut l = link("ab", "p0", "p1", 1);
        l[typo] = json!(1);
        let e = err_of(vec![l]);
        assert!(e.contains(typo), "{typo}: {e}");
        assert!(e.contains("unknown field"), "serde's wording: {typo}: {e}");
    }
}

#[test]
fn an_unknown_top_level_key_is_an_error_naming_it() {
    let text = format!(r#"{{ "links": [{}], "lnks": [] }}"#, link("a", "p", "q", 1));
    let e = err_text(&text);
    assert!(e.contains("lnks"), "{e}");
}

#[test]
fn an_unknown_key_in_the_histogram_is_an_error_naming_it() {
    let l = link_h("ab", "p0", "p1", 1, json!({ "bins": 10, "lo": 5.0 }));
    let e = err_text(&file(vec![l]));
    assert!(e.contains("lo"), "{e}");
    let l = link_h("ab", "p0", "p1", 1, json!({ "bin_ns": 10 }));
    assert!(err_text(&file(vec![l])).contains("bin_ns"));
}

#[test]
fn every_documented_key_is_accepted() {
    let mut l = link("ab", "p0", "p1", 1);
    l["histogram"] = json!({ "bins": 10, "lo_ns": 0, "hi_ns": 100.0 });
    l["lcore"] = json!(2);
    l["delay_stats"] = json!(false);
    let links = parse_links(&file(vec![l])).unwrap();
    assert_eq!(links[0].lcore, Some(2));
    assert!(!links[0].delay_stats);
    assert_eq!(links[0].histogram, spec(Some(10), Some(0.0), Some(100.0)));
}

// ── delay_stats ─────────────────────────────────────────────────────

#[test]
fn delay_stats_is_a_bool_per_link_default_true() {
    let mut a = link("a", "p0", "p1", 1);
    a["delay_stats"] = json!(false);
    let mut b = link("b", "p1", "p2", 2);
    b["delay_stats"] = json!(true);
    let c = link("c", "p2", "p0", 3);
    let links = links_of(vec![a, b, c]);
    assert!(!links[0].delay_stats);
    assert!(links[1].delay_stats);
    assert!(links[2].delay_stats, "default on");
}

#[test]
fn delay_stats_of_the_wrong_type_is_an_error() {
    for bad in [
        json!([true]),
        json!([true, false]),
        json!(1),
        json!(0),
        json!("true"),
    ] {
        let mut l = link("typelink", "p0", "p1", 1);
        l["delay_stats"] = bad.clone();
        assert!(parse_links(&file(vec![l])).is_err(), "{bad}");
    }
}

#[test]
fn delay_stats_may_be_mixed_with_and_without_lcore() {
    // unlike `lcore`, `delay_stats` is per link and optional everywhere
    let mut a = link_l("a", "p0", "p1", 1, json!(1));
    a["delay_stats"] = json!(false);
    let b = link_l("b", "p1", "p0", 1, json!(2));
    let links = links_of(vec![a, b]);
    assert!(!links[0].delay_stats);
    assert!(links[1].delay_stats);
}

#[test]
fn directions_carry_the_delay_stats_switch_of_their_link() {
    // a: on, b: off, c: off, d: on
    let mut a = link("a", "P0", "P1", 1);
    a["delay_stats"] = json!(true);
    let mut b = link("b", "P1", "P2", 2);
    b["delay_stats"] = json!(false);
    let mut c = link("c", "P4", "P4", 3);
    c["delay_stats"] = json!(false);
    let d = link("d", "P3", "P0", 4);
    let links = links_of(vec![a, b, c, d]);
    let ids = probed(&[("P0", 0), ("P1", 1), ("P2", 2), ("P3", 3), ("P4", 4)]);
    let d = resolve_directions(&links, &ids, 0, &[1, 2, 3, 4]).unwrap();
    assert_eq!(
        d.iter().map(|x| x.delay_stats).collect::<Vec<_>>(),
        [true, false, false, true]
    );
}

// ── /dpds/ports dict limit ──────────────────────────────────────────
//
// `/dpds/ports` has `3 + 5 * ports + 10 * links` keys (ports = distinct
// ports of the links) and DPDK's dict holds 256: a ring of 16 fits (243), a
// ring of 17 does not (258). With ports != links the formula decides: 12
// one-way links over 24 ports are 3 + 120 + 120 = 243 (fits), 13 are 263.

fn assert_fits(links: Vec<Value>, what: &str) {
    let n = links.len();
    assert_eq!(parse_links(&file(links)).unwrap().len(), n, "{what}");
}

fn assert_too_many(links: Vec<Value>, what: &str) {
    let e = err_of(links);
    assert!(e.contains("256"), "{what}: the maximum is named: {e}");
}

#[test]
fn a_ring_of_sixteen_ports_fits_and_seventeen_do_not() {
    assert_fits(ring(16), "16-ring: 243 keys");
    assert_too_many(ring(17), "17-ring: 258 keys");
    assert_too_many(ring(40), "40-ring");
}

#[test]
fn the_limit_counts_distinct_ports_and_links_with_the_new_formula() {
    // 2 links per pair over 2 ports: 8 pairs = 16 links, 16 ports = 243
    let pairs = |n: usize| -> Vec<Value> {
        (0..n)
            .flat_map(|i| pair(&format!("a{i}"), &format!("b{i}")))
            .collect()
    };
    assert_fits(pairs(8), "8 bidirectional pairs: 16 ports, 16 links, 243");
    assert_too_many(pairs(9), "9 pairs: 18 ports, 18 links, 273");
    // one-way links: twice as many ports as links
    assert_fits(one_way(12), "12 one-way links: 24 ports, 12 links, 243");
    assert_too_many(one_way(13), "13 one-way links: 26 ports, 13 links, 263");
    // hairpins: one port per link
    let hairpins = |n: usize| -> Vec<Value> {
        (0..n)
            .map(|i| link(&format!("h{i}"), &format!("p{i}"), &format!("p{i}"), 1))
            .collect()
    };
    assert_fits(hairpins(16), "16 hairpins: 243");
    assert_too_many(hairpins(17), "17 hairpins: 258");
}
