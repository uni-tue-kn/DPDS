//! The `--links <file>` file: parsing, validation and the numbering of the
//! directions.
//!
//! ```json
//! { "links": [
//!   { "name": "a-b", "ports": ["0000:65:00.0", "0000:65:00.1"],
//!     "configs": [ <Config for ports[0] -> ports[1]>, <Config for ports[1] -> ports[0]> ],
//!     "histograms": [ {"bins": 100, "lo_ns": 0, "hi_ns": 2e6}, {} ],   (optional)
//!     "lcores": [2, 1],                                                (optional)
//!     "delay_stats": [true, false] },                                  (optional)
//!   { "name": "loop", "ports": ["net_null0"], "configs": [ <Config> ] }
//! ] }
//! ```
//!
//! Ports are DPDK device names (PCI address or vdev name). A link has one
//! port (a loopback, one direction) or two (two directions), exactly one
//! config per port. Directions are numbered globally in file order.
//!
//! `delay_stats` has one flag per direction (default all `true`): `false`
//! switches the intended-delay / lateness statistics and the delay histogram
//! of that direction off (counters and drop causes stay). A key the format
//! does not know is an error naming it.
//!
//! `/dpds/ports` must fit DPDK's 256-key telemetry dict (`3 + 5 * ports + 10 *
//! directions` keys), which limits a run to 16 ports.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use dpds_core::Config;
use serde::Deserialize;

use crate::consts::MAX_TELEMETRY_DICT_ENTRIES;
use crate::delay_stats::HistogramSpec;

/// Longest link name.
pub const MAX_NAME_LEN: usize = 32;

/// One link of the file, validated by [`parse_links`].
#[derive(Debug, Clone)]
pub struct LinkSpec {
    pub name: String,
    /// DPDK device names, 1 or 2.
    pub ports: Vec<String>,
    /// One config per port: `configs[k]` is the direction that receives on
    /// `ports[k]` (and transmits on the other port, or on the same one).
    pub configs: Vec<Config>,
    /// Explicit worker lcore per direction, if the file gives any.
    pub lcores: Option<Vec<u32>>,
    /// One entry per direction, `HistogramSpec::default()` if the file has none.
    pub histograms: Vec<HistogramSpec>,
    /// One entry per direction: whether it records delay statistics
    /// (`true` if the file has none).
    pub delay_stats: Vec<bool>,
}

/// A problem with the links file or with how it fits the probed ports and
/// lcores. The message names the link, port or lcore concerned.
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
    name: String,
    ports: Vec<String>,
    configs: Vec<Config>,
    histograms: Option<Vec<RawHistogram>>,
    lcores: Option<Vec<u32>>,
    delay_stats: Option<Vec<bool>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHistogram {
    bins: Option<usize>,
    lo_ns: Option<f64>,
    hi_ns: Option<f64>,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Parses and validates the text of a links file (everything that needs no
/// EAL): see the module docs for the format.
pub fn parse_links(json: &str) -> Result<Vec<LinkSpec>, LinksError> {
    let raw: RawFile =
        serde_json::from_str(json).map_err(|e| LinksError(format!("invalid links file: {e}")))?;
    if raw.links.is_empty() {
        return err("the links file has no link");
    }

    let mut names = HashSet::new();
    let mut ports = HashSet::new();
    let mut lcores = HashSet::new();
    let mut links = Vec::new();
    for raw in raw.links {
        let name = raw.name;
        if !valid_name(&name) {
            return err(format!(
                "invalid link name '{name}': 1 to {MAX_NAME_LEN} characters of [A-Za-z0-9_.-]"
            ));
        }
        if !names.insert(name.clone()) {
            return err(format!("duplicate link name '{name}'"));
        }
        if !matches!(raw.ports.len(), 1 | 2) {
            return err(format!(
                "link '{name}': a link has 1 or 2 ports, found {}",
                raw.ports.len()
            ));
        }
        if raw.configs.len() != raw.ports.len() {
            return err(format!(
                "link '{name}': {} ports need {} configs (one per direction), found {}",
                raw.ports.len(),
                raw.ports.len(),
                raw.configs.len()
            ));
        }
        for port in &raw.ports {
            if port.is_empty() {
                return err(format!("link '{name}': a port name is empty"));
            }
            if !ports.insert(port.clone()) {
                return err(format!(
                    "port '{port}' is listed more than once (link '{name}'); a port belongs to \
                     at most one link and appears once"
                ));
            }
        }

        let directions = raw.configs.len();
        let histograms = match raw.histograms {
            None => vec![HistogramSpec::default(); directions],
            Some(h) if h.len() != directions => {
                return err(format!(
                    "link '{name}': histograms has {} entries, the link has {directions} \
                     directions",
                    h.len()
                ))
            }
            Some(h) => {
                let mut specs = Vec::new();
                for (k, h) in h.into_iter().enumerate() {
                    let spec = HistogramSpec {
                        bins: h.bins,
                        lo_ns: h.lo_ns,
                        hi_ns: h.hi_ns,
                    };
                    spec.validate()
                        .map_err(|e| LinksError(format!("link '{name}', direction {k}: {e}")))?;
                    specs.push(spec);
                }
                specs
            }
        };

        let delay_stats = match raw.delay_stats {
            None => vec![true; directions],
            Some(flags) if flags.len() != directions => {
                return err(format!(
                    "link '{name}': delay_stats has {} entries, the link has {directions} \
                     directions",
                    flags.len()
                ))
            }
            Some(flags) => flags,
        };

        if let Some(l) = &raw.lcores {
            if l.len() != directions {
                return err(format!(
                    "link '{name}': lcores has {} entries, the link has {directions} directions",
                    l.len()
                ));
            }
            for &lcore in l {
                if !lcores.insert(lcore) {
                    return err(format!(
                        "lcore {lcore} is listed more than once (link '{name}'): every \
                         direction needs its own lcore"
                    ));
                }
            }
        }

        links.push(LinkSpec {
            name,
            ports: raw.ports,
            configs: raw.configs,
            lcores: raw.lcores,
            histograms,
            delay_stats,
        });
    }

    // `/dpds/ports` has 3 + 5 keys per port + 10 per direction (a link has as
    // many directions as ports).
    let ports = num_directions(&links);
    let keys = 3 + 5 * ports + 10 * ports;
    if keys > MAX_TELEMETRY_DICT_ENTRIES {
        return err(format!(
            "{ports} ports are too many: the /dpds/ports telemetry reply needs 3 + 5 * ports + \
             10 * directions = {keys} keys, DPDK's dict holds at most \
             {MAX_TELEMETRY_DICT_ENTRIES} (at most 16 ports)"
        ));
    }

    if links.iter().any(|l| l.lcores.is_some()) && links.iter().any(|l| l.lcores.is_none()) {
        let missing: Vec<&str> = links
            .iter()
            .filter(|l| l.lcores.is_none())
            .map(|l| l.name.as_str())
            .collect();
        return err(format!(
            "either every link has \"lcores\" or none: missing for link(s) {missing:?}"
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

/// The number of directions (= worker lcores) of `links`: one per port.
pub fn num_directions(links: &[LinkSpec]) -> usize {
    links.iter().map(|l| l.ports.len()).sum()
}

/// One direction, resolved against the probed ports.
#[derive(Debug, Clone)]
pub struct LinkDirection {
    /// Index of the link in the file.
    pub link: usize,
    pub rx_port: u16,
    pub tx_port: u16,
    pub config: Config,
    pub histogram: HistogramSpec,
    /// Whether the direction records delay statistics.
    pub delay_stats: bool,
    /// The worker lcore that runs the direction.
    pub lcore: u32,
}

/// Numbers the directions in file order (link by link: `ports[0] -> ports[1]`
/// with `configs[0]`, then `ports[1] -> ports[0]` with `configs[1]`; a
/// one-port link is a single loopback direction), maps the device names to
/// port ids with `probed` (`(device name, port id)` of every probed ethdev) and
/// assigns the worker lcores: the listed ones, or the i-th of `workers`
/// (ascending, main lcore excluded) to direction i.
pub fn resolve_directions(
    links: &[LinkSpec],
    probed: &[(String, u16)],
    main_lcore: u32,
    workers: &[u32],
) -> Result<Vec<LinkDirection>, LinksError> {
    let id_of = |name: &str| probed.iter().find(|(n, _)| n == name).map(|(_, id)| *id);

    let mut directions = Vec::new();
    for (index, link) in links.iter().enumerate() {
        let mut ids = Vec::new();
        for port in &link.ports {
            let Some(id) = id_of(port) else {
                let names: Vec<&str> = probed.iter().map(|(n, _)| n.as_str()).collect();
                return err(format!(
                    "link '{}': port '{port}' is not a probed DPDK device (probed: {names:?}); \
                     allow it with -a / --vdev",
                    link.name
                ));
            };
            ids.push(id);
        }
        for (k, config) in link.configs.iter().enumerate() {
            directions.push(LinkDirection {
                link: index,
                rx_port: ids[k],
                tx_port: ids[(k + 1) % ids.len()],
                config: *config,
                histogram: link.histograms.get(k).copied().unwrap_or_default(),
                delay_stats: link.delay_stats.get(k).copied().unwrap_or(true),
                lcore: 0,
            });
        }
    }
    for (name, _) in probed {
        if !links.iter().any(|l| l.ports.contains(name)) {
            return err(format!(
                "port '{name}' is probed but belongs to no link; every probed port must be in a \
                 link (or not be allowed with -a / --vdev)"
            ));
        }
    }

    if links.iter().any(|l| l.lcores.is_some()) {
        if links.iter().any(|l| l.lcores.is_none()) {
            return err("either every link has \"lcores\" or none");
        }
        let listed = links.iter().flat_map(|l| {
            l.lcores
                .iter()
                .flatten()
                .map(move |&lcore| (l.name.as_str(), lcore))
        });
        for (d, (link, lcore)) in listed.enumerate() {
            if lcore == main_lcore {
                return err(format!(
                    "link '{link}': lcore {lcore} is the main lcore, it cannot run a direction"
                ));
            }
            if !workers.contains(&lcore) {
                return err(format!(
                    "link '{link}': lcore {lcore} is not an EAL worker lcore (workers: \
                     {workers:?}, main lcore {main_lcore})"
                ));
            }
            directions[d].lcore = lcore;
        }
        if let Some(unused) = workers
            .iter()
            .find(|w| !directions.iter().any(|d| d.lcore == **w))
        {
            return err(format!(
                "worker lcore {unused} is not used by any direction: \"lcores\" must name every \
                 EAL worker lcore ({workers:?}), one per direction"
            ));
        }
    } else {
        if workers.len() != directions.len() {
            return err(format!(
                "{} directions need {} worker lcores, found {workers:?}",
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
