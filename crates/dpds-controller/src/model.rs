//! The configuration of a run: the links of a start body. The controller
//! keeps it only while the emulation runs (it reports it in `GET /api/emulation`).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::dto::ConfigDto;
use crate::host::Network;
use crate::validate;

/// One emulated link: one direction, packets received on `ingress` leave on `egress`
/// with `config` applied. `ingress == egress` (hairpin) is allowed. A port is the
/// ingress of at most one link and the egress of at most one link.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Link {
    /// Position in the start body, from 1.
    pub id: u32,
    /// The port the link receives on: a PCI address or a vdev name such as `net_null0`, or in a
    /// virtual network an interface name such as `eth1`.
    pub ingress: String,
    /// The port the link sends on.
    pub egress: String,
    pub config: ConfigDto,
}

impl Link {
    /// The label used in messages: `link <ingress> -> <egress>`.
    pub fn label(&self) -> String {
        format!("link {} -> {}", self.ingress, self.egress)
    }
}

/// A link of a start body: a [`Link`] without an id.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartLink {
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
    pub fn into_run_config(self, network: Network) -> RunConfig {
        let normal = |port: String| validate::port_name(&port, network).unwrap_or(port);
        let links = self
            .links
            .into_iter()
            .zip(1..)
            .map(|(l, id)| Link {
                id,
                ingress: normal(l.ingress),
                egress: normal(l.egress),
                config: l.config,
            })
            .collect();
        RunConfig { links }
    }
}
