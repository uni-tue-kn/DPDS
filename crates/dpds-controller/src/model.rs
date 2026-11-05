//! The configuration of a run: the links of a start body. The controller
//! keeps it only while the emulation runs (it reports it in `GET /api/emulation`).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::dto::ConfigDto;
use crate::validate;

/// Longest link name, in characters.
pub const MAX_NAME_CHARS: usize = 32;

/// One emulated link: one direction, packets received on `ingress` leave on `egress`
/// with `config` applied. `ingress == egress` (hairpin) is allowed. A port is the
/// ingress of at most one link and the egress of at most one link.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Link {
    /// Position in the start body, from 1.
    pub id: u32,
    pub name: String,
    /// DPDK device name the link receives on: a PCI address or a vdev name such as `net_null0`.
    pub ingress: String,
    /// DPDK device name the link sends on.
    pub egress: String,
    pub config: ConfigDto,
}

/// A link of a start body: a [`Link`] without an id.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartLink {
    pub name: String,
    pub ingress: String,
    pub egress: String,
    pub config: ConfigDto,
}

/// Body of `POST /api/emulation/start`, `/api/emulation/validate` and `/api/host/hints`:
/// the links.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    pub links: Vec<StartLink>,
}

/// What the planners (lcores, hints) read, and what `GET /api/emulation` reports as `config`.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct RunConfig {
    pub links: Vec<Link>,
}

impl StartRequest {
    /// The links with ids (their position in the body, from 1) and PCI addresses in
    /// lower case. A port name that is not valid stays as sent, for validation to refuse.
    pub fn into_run_config(self) -> RunConfig {
        let normal = |port: String| validate::port_name(&port).unwrap_or(port);
        let links = self
            .links
            .into_iter()
            .zip(1..)
            .map(|(l, id)| Link {
                id,
                name: l.name,
                ingress: normal(l.ingress),
                egress: normal(l.egress),
                config: l.config,
            })
            .collect();
        RunConfig { links }
    }
}
