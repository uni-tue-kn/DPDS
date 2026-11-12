//! Lcore assignment: one main lcore, one worker lcore per link.

use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::host::{fmt_cpus, is_vdev, HostRoots, Network};
use crate::model::RunConfig;

/// A link as planned: traffic received on `rx` (the ingress), sent on `tx` (the
/// egress), handled by `lcore`. `dir` is the link's index in start-body order.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DirectionPlan {
    pub dir: u32,
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

/// Plans lcores for the links: the main lcore is the lowest online core that is not
/// isolated; each link gets its own free isolated core, the lowest id first, preferring the NUMA
/// node of the ingress NIC.
///
/// In a virtual network there is no isolation and no NUMA preference: the main lcore is the lowest
/// online core and the links take the next online cores in id order.
pub fn assign_lcores(roots: &HostRoots, config: &RunConfig) -> Result<Assignment, LcoreError> {
    let online = roots.online_cpus();
    if roots.network == Network::Virtual {
        let needed = config.links.len();
        if online.len() < needed + 1 {
            return err(format!(
                "{needed} link(s) need {} online cores but only {} are online",
                needed + 1,
                online.len()
            ));
        }
        return Ok(Assignment {
            main_lcore: online[0],
            directions: config
                .links
                .iter()
                .zip(&online[1..])
                .enumerate()
                .map(|(i, (l, core))| DirectionPlan {
                    dir: i as u32,
                    rx: l.ingress.clone(),
                    tx: l.egress.clone(),
                    lcore: *core,
                })
                .collect(),
        });
    }
    let isolated = roots.isolated_cpus();

    let Some(main_lcore) = online.iter().copied().find(|c| !isolated.contains(c)) else {
        return err(
            "every online CPU is isolated, so there is no core left for the main lcore".into(),
        );
    };

    let mut free: Vec<u32> = isolated
        .iter()
        .copied()
        .filter(|c| online.contains(c))
        .collect();
    let needed = config.links.len();
    if needed > free.len() {
        return err(format!(
            "{needed} link(s) need an isolated core but only {} are available (isolated cores: {}; set isolcpus)",
            free.len(),
            if isolated.is_empty() { "none".to_string() } else { fmt_cpus(&isolated) },
        ));
    }

    let mut plans = Vec::with_capacity(needed);
    for (i, l) in config.links.iter().enumerate() {
        let node = if is_vdev(&l.ingress) {
            None
        } else {
            roots.pci_numa_node(&l.ingress)
        };
        let local = node.and_then(|n| free.iter().position(|c| roots.node_of_cpu(*c) == Some(n)));
        plans.push(DirectionPlan {
            dir: i as u32,
            rx: l.ingress.clone(),
            tx: l.egress.clone(),
            lcore: free.remove(local.unwrap_or(0)),
        });
    }
    Ok(Assignment {
        main_lcore,
        directions: plans,
    })
}
