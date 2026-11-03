//! Stored state (B1): links and settings, persisted by [`crate::store`].

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::dto::ConfigDto;

/// One emulated link: one port (loopback, one direction) or two ports (two
/// directions: ports[0] -> ports[1] and back), one config per direction.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Link {
    pub id: u32,
    pub name: String,
    /// DPDK device names: PCI addresses or vdev names such as `net_null0`.
    pub ports: Vec<String>,
    pub configs: Vec<ConfigDto>,
    /// Optional worker lcore per direction; automatic when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<Vec<u32>>,
}

/// A named per-direction config, stored in the controller (K1). Not part of a run.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Preset {
    pub id: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub config: ConfigDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Settings {
    /// Main lcore; the lowest non-isolated online core when absent.
    pub main_lcore: Option<u32>,
    /// mlx5 devargs appended to `-a <pci>` of ports bound to mlx5_core. `mprq_en` is refused.
    pub mlx5_devargs: String,
    /// Extra EAL arguments, passed before the `--` separator.
    pub eal_extra_args: Vec<String>,
    /// Telemetry poll interval.
    pub poll_interval_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            main_lcore: None,
            mlx5_devargs: "txq_mpw_en=1".into(),
            eal_extra_args: Vec::new(),
            poll_interval_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredState {
    pub links: Vec<Link>,
    pub settings: Settings,
    /// Next link id; ids are never reused.
    pub next_id: u32,
    /// Absent in state files written before presets existed.
    #[serde(default)]
    pub presets: Vec<Preset>,
    /// Next preset id; ids are never reused.
    #[serde(default = "first_id")]
    pub next_preset_id: u32,
}

fn first_id() -> u32 {
    1
}

impl Default for StoredState {
    fn default() -> Self {
        StoredState {
            links: Vec::new(),
            settings: Settings::default(),
            next_id: 1,
            presets: Vec::new(),
            next_preset_id: 1,
        }
    }
}
