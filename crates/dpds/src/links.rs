//! The `--links <file>` file: parsing, validation and direction numbering.
//!
//! ```json
//! { "links": [
//!   { "ingress": "0000:65:00.0", "egress": "0000:65:00.1",
//!     "config": { <dpds_core::Config> },
//!     "lcore": 3,                                                (optional)
//!     "histogram": {"bins": 100, "lo_ns": 0, "hi_ns": 2e6},      (optional)
//!     "iat_histogram": {"bins": 100, "lo_ns": 0, "hi_ns": 256},  (optional, like histogram)
//!     "delay_stats": true },                                     (optional)
//!   { "ingress": "net_null0", "egress": "net_null0", "config": { <Config> } }
//! ] }
//! ```
//!
//! A link is one direction: packets arriving on `ingress` leave on `egress`
//! with the link's config. Ports are DPDK device names (PCI address or vdev
//! name); `ingress == egress` (a hairpin) is allowed. A port is the ingress of
//! at most one link and the egress of at most one link, so every RX and TX
//! queue has exactly one worker. The direction index is the link's index in
//! the file.
//!
//! `lcore` is all-or-none across the links. `delay_stats: false` switches off
//! the intended-delay / lateness statistics and the delay histogram of that
//! link (counters and drop causes stay). An unknown key is an error naming it.
//!
//! `/dpds/ports` must fit DPDK's 256-key telemetry dict (`3 + 5 * ports + 9 *
//! links` keys, `ports` = distinct ports named by the links).

use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use dpds_core::Config;
use serde::Deserialize;

use crate::consts::MAX_TELEMETRY_DICT_ENTRIES;
use crate::delay_stats::HistogramSpec;

/// One validated link of the file.
#[derive(Debug, Clone)]
pub struct LinkSpec {
    /// Device name of the receiving port.
    pub ingress: String,
    /// Device name of the transmitting port.
    pub egress: String,
    pub config: Config,
    /// Explicit worker lcore, if the file gives one.
    pub lcore: Option<u32>,
    pub histogram: HistogramSpec,
    /// Inter-arrival-time histogram.
    pub iat_histogram: HistogramSpec,
    /// Whether the link records delay statistics (default `true`).
    pub delay_stats: bool,
}

impl LinkSpec {
    /// The label used in messages: `link <ingress> -> <egress>`.
    pub fn label(&self) -> String {
        format!("link {} -> {}", self.ingress, self.egress)
    }
}

/// A problem with the links file or how it fits the probed ports and lcores;
/// the message names the link, port or lcore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinksError(String);

impl fmt::Display for LinksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LinksError {}

fn err<T>(msg: impl Into<String>) -> Result<T, LinksError> {
    Err(LinksError(msg.into()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    links: Vec<RawLink>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLink {
    ingress: String,
    egress: String,
    config: Config,
    lcore: Option<u32>,
    histogram: Option<RawHistogram>,
    iat_histogram: Option<RawHistogram>,
    delay_stats: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHistogram {
    bins: Option<usize>,
    lo_ns: Option<f64>,
    hi_ns: Option<f64>,
}

/// Parses and validates the text of a links file (needs no EAL).
pub fn parse_links(json: &str) -> Result<Vec<LinkSpec>, LinksError> {
    let raw: RawFile =
        serde_json::from_str(json).map_err(|e| LinksError(format!("invalid links file: {e}")))?;
    if raw.links.is_empty() {
        return err("the links file has no link");
    }

    let mut ingresses = HashSet::new();
    let mut egresses = HashSet::new();
    let mut lcores = HashSet::new();
    let mut links = Vec::new();
    for raw in raw.links {
        let label = format!("link {} -> {}", raw.ingress, raw.egress);
        for (role, port, seen) in [
            ("ingress", &raw.ingress, &mut ingresses),
            ("egress", &raw.egress, &mut egresses),
        ] {
            if port.is_empty() {
                return err(format!("{label}: the {role} port name is empty"));
            }
            if !seen.insert(port.clone()) {
                return err(format!(
                    "port '{port}' is the {role} of more than one link ({label}): a port \
                     is the ingress of at most one link and the egress of at most one link"
                ));
            }
        }

        let spec_of = |h: Option<RawHistogram>| {
            h.map_or_else(HistogramSpec::default, |h| HistogramSpec {
                bins: h.bins,
                lo_ns: h.lo_ns,
                hi_ns: h.hi_ns,
            })
        };
        let histogram = spec_of(raw.histogram);
        histogram
            .validate()
            .map_err(|e| LinksError(format!("{label}: {e}")))?;
        let iat_histogram = spec_of(raw.iat_histogram);
        iat_histogram
            .validate()
            .map_err(|e| LinksError(format!("{label}: IAT histogram: {e}")))?;

        if let Some(lcore) = raw.lcore {
            if !lcores.insert(lcore) {
                return err(format!(
                    "lcore {lcore} is listed more than once ({label}): every link needs \
                     its own lcore"
                ));
            }
        }

        links.push(LinkSpec {
            ingress: raw.ingress,
            egress: raw.egress,
            config: raw.config,
            lcore: raw.lcore,
            histogram,
            iat_histogram,
            delay_stats: raw.delay_stats.unwrap_or(true),
        });
    }

    let ports = distinct_ports(&links).len();
    let keys = 3 + 5 * ports + 9 * links.len();
    if keys > MAX_TELEMETRY_DICT_ENTRIES {
        return err(format!(
            "{ports} ports and {} links are too many: the /dpds/ports telemetry reply needs 3 + \
             5 * ports + 9 * links = {keys} keys, DPDK's dict holds at most \
             {MAX_TELEMETRY_DICT_ENTRIES}",
            links.len()
        ));
    }

    if links.iter().any(|l| l.lcore.is_some()) && links.iter().any(|l| l.lcore.is_none()) {
        let missing: Vec<String> = links
            .iter()
            .filter(|l| l.lcore.is_none())
            .map(LinkSpec::label)
            .collect();
        return err(format!(
            "either every link has \"lcore\" or none: missing for link(s) {missing:?}"
        ));
    }
    Ok(links)
}

/// Reads and parses the links file at `path`.
pub fn load_links(path: &Path) -> Result<Vec<LinkSpec>, LinksError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| LinksError(format!("failed to read links file {}: {e}", path.display())))?;
    parse_links(&text).map_err(|e| LinksError(format!("{}: {e}", path.display())))
}

/// The distinct ports named by `links`.
fn distinct_ports(links: &[LinkSpec]) -> HashSet<&str> {
    links
        .iter()
        .flat_map(|l| [l.ingress.as_str(), l.egress.as_str()])
        .collect()
}

/// One link (= direction) resolved against the probed ports.
#[derive(Debug, Clone)]
pub struct LinkDirection {
    /// Index of the link in the file.
    pub link: usize,
    pub rx_port: u16,
    pub tx_port: u16,
    pub config: Config,
    pub histogram: HistogramSpec,
    pub iat_histogram: HistogramSpec,
    pub delay_stats: bool,
    /// The worker lcore that runs the link.
    pub lcore: u32,
}

/// Maps device names to port ids via `probed` (`(device name, port id)` of every
/// probed ethdev) and returns one direction per link in file order. Worker
/// lcores are the listed ones, or the i-th of `workers` (ascending, main lcore
/// excluded) for link i.
pub fn resolve_directions(
    links: &[LinkSpec],
    probed: &[(String, u16)],
    main_lcore: u32,
    workers: &[u32],
) -> Result<Vec<LinkDirection>, LinksError> {
    let id_of = |link: &LinkSpec, role: &str, port: &str| {
        probed
            .iter()
            .find(|(n, _)| n == port)
            .map(|(_, id)| *id)
            .ok_or_else(|| {
                let names: Vec<&str> = probed.iter().map(|(n, _)| n.as_str()).collect();
                LinksError(format!(
                    "{}: {role} port '{port}' is not a probed DPDK device (probed: \
                     {names:?}); allow it with -a / --vdev",
                    link.label()
                ))
            })
    };

    let mut directions = Vec::new();
    for (index, link) in links.iter().enumerate() {
        directions.push(LinkDirection {
            link: index,
            rx_port: id_of(link, "ingress", &link.ingress)?,
            tx_port: id_of(link, "egress", &link.egress)?,
            config: link.config,
            histogram: link.histogram,
            iat_histogram: link.iat_histogram,
            delay_stats: link.delay_stats,
            lcore: 0,
        });
    }
    let used = distinct_ports(links);
    for (name, _) in probed {
        if !used.contains(name.as_str()) {
            return err(format!(
                "port '{name}' is probed but no link uses it as ingress or egress; every probed \
                 port must be used (or not be allowed with -a / --vdev)"
            ));
        }
    }

    if links.iter().any(|l| l.lcore.is_some()) {
        if links.iter().any(|l| l.lcore.is_none()) {
            return err("either every link has \"lcore\" or none");
        }
        for (dir, link) in directions.iter_mut().zip(links) {
            let lcore = link.lcore.unwrap_or_default();
            if lcore == main_lcore {
                return err(format!(
                    "{}: lcore {lcore} is the main lcore, it cannot run a link",
                    link.label()
                ));
            }
            if !workers.contains(&lcore) {
                return err(format!(
                    "{}: lcore {lcore} is not an EAL worker lcore (workers: \
                     {workers:?}, main lcore {main_lcore})",
                    link.label()
                ));
            }
            dir.lcore = lcore;
        }
        if let Some(unused) = workers
            .iter()
            .find(|w| !directions.iter().any(|d| d.lcore == **w))
        {
            return err(format!(
                "worker lcore {unused} is not used by any link: \"lcore\" must name every EAL \
                 worker lcore ({workers:?}), one per link"
            ));
        }
    } else {
        if workers.len() != directions.len() {
            return err(format!(
                "{} links need {} worker lcores, found {workers:?}",
                directions.len(),
                directions.len()
            ));
        }
        for (d, &lcore) in directions.iter_mut().zip(workers) {
            d.lcore = lcore;
        }
    }
    Ok(directions)
}
