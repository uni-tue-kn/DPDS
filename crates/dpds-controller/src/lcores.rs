//! Lcore assignment (B7): one main lcore, one worker lcore per link.

use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::host::{fmt_cpus, is_vdev, HostRoots};
use crate::model::RunConfig;

/// A link as planned: traffic received on `rx` (the ingress), sent on `tx` (the
/// egress), handled by `lcore`. `dir` is the link's index in start-body order.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DirectionPlan {
    pub dir: u32,
    pub link: String,
    pub rx: String,
    pub tx: String,
    pub lcore: u32,
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub main_lcore: u32,
    pub directions: Vec<DirectionPlan>,
}

#[derive(Debug, Clone)]
pub struct LcoreError(String);

impl fmt::Display for LcoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LcoreError {}

fn err<T>(m: String) -> Result<T, LcoreError> {
    Err(LcoreError(m))
}

/// Directions numbered in link order: the link is the direction.
fn directions(config: &RunConfig) -> Vec<Pending> {
    config
        .links
        .iter()
        .map(|l| Pending {
            link: l.name.clone(),
            rx: l.ingress.clone(),
            tx: l.egress.clone(),
            pinned: l.worker,
        })
        .collect()
}

/// One link before its lcore is known.
struct Pending {
    link: String,
    rx: String,
    tx: String,
    pinned: Option<u32>,
}

/// Plans lcores for the links. Per-link `worker` overrides are used
/// literally (any online core except the main lcore); the other links get free
/// isolated cores, the lowest id first, preferring the NUMA node of the ingress NIC.
pub fn assign_lcores(roots: &HostRoots, config: &RunConfig) -> Result<Assignment, LcoreError> {
    let online = roots.online_cpus();
    let isolated = roots.isolated_cpus();

    let main_lcore = match config.settings.main_lcore {
        Some(m) if online.contains(&m) => m,
        Some(m) => return err(format!("main lcore {m} is not an online CPU")),
        None => match online.iter().find(|c| !isolated.contains(c)) {
            Some(m) => *m,
            None => return err("every online CPU is isolated, so there is no core for the main lcore (set settings.main_lcore)".into()),
        },
    };

    let mut dirs = directions(config);

    let mut used = vec![main_lcore];
    for d in &dirs {
        if let Some(c) = d.pinned {
            if !online.contains(&c) {
                return err(format!(
                    "link {}: worker core {c} is not an online CPU",
                    d.link
                ));
            }
            if c == main_lcore {
                return err(format!(
                    "link {}: worker core {c} is the main lcore",
                    d.link
                ));
            }
            if used.contains(&c) {
                return err(format!(
                    "link {}: worker core {c} is used more than once",
                    d.link
                ));
            }
            used.push(c);
        }
    }

    let free: Vec<u32> = isolated
        .iter()
        .copied()
        .filter(|c| online.contains(c) && !used.contains(c))
        .collect();
    let needed = dirs.iter().filter(|d| d.pinned.is_none()).count();
    if needed > free.len() {
        return err(format!(
            "{needed} link(s) need an isolated core but only {} are available (isolated cores: {}; set isolcpus, or pin cores with the worker override)",
            free.len(),
            if isolated.is_empty() { "none".to_string() } else { fmt_cpus(&isolated) },
        ));
    }

    let mut free = free;
    let mut plans = Vec::with_capacity(dirs.len());
    for (i, d) in dirs.drain(..).enumerate() {
        let lcore = match d.pinned {
            Some(c) => c,
            None => {
                let node = if is_vdev(&d.rx) {
                    None
                } else {
                    roots.pci_numa_node(&d.rx)
                };
                let local =
                    node.and_then(|n| free.iter().position(|c| roots.node_of_cpu(*c) == Some(n)));
                free.remove(local.unwrap_or(0))
            }
        };
        plans.push(DirectionPlan {
            dir: i as u32,
            link: d.link,
            rx: d.rx,
            tx: d.tx,
            lcore,
        });
    }
    Ok(Assignment {
        main_lcore,
        directions: plans,
    })
}
