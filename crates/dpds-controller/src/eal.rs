//! Pure functions behind the supervisor (B3): EAL command line, the links file
//! handed to `dpds --links`, and the drain timeout.

use std::path::PathBuf;
use std::time::Duration;

use dpds_core::Config;
use serde_json::{json, Value};

use crate::dto::{ConfigDto, DtoError};
use crate::host::is_vdev;
use crate::lcores::DirectionPlan;
use crate::model::Link;

/// Margin on top of the longest delay when waiting for `dpds` to drain.
const DRAIN_MARGIN: Duration = Duration::from_secs(5);

/// A DPDK device of the links and the kernel driver of a PCI port, if known.
#[derive(Debug, Clone)]
pub struct EalPort {
    pub name: String,
    pub driver: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EalInput {
    pub main_lcore: u32,
    pub worker_lcores: Vec<u32>,
    pub ports: Vec<EalPort>,
    pub mlx5_devargs: String,
    pub eal_extra_args: Vec<String>,
    pub links_file: PathBuf,
}

/// `dpds` arguments without the binary name. Vdev names (`net_*`) become
/// `--vdev`; mlx5 devargs go only to PCI ports bound to mlx5_core. `mprq_en` is
/// never generated.
pub fn eal_argv(i: &EalInput) -> Vec<String> {
    let lcores: Vec<String> = std::iter::once(i.main_lcore)
        .chain(i.worker_lcores.iter().copied())
        .map(|c| c.to_string())
        .collect();
    let mut argv = vec![
        "-l".to_string(),
        lcores.join(","),
        "--main-lcore".into(),
        i.main_lcore.to_string(),
        "--in-memory".into(),
        "--file-prefix".into(),
        "dpds-ctl".into(),
    ];
    for p in &i.ports {
        if is_vdev(&p.name) {
            argv.push("--vdev".into());
            argv.push(p.name.clone());
        } else {
            argv.push("-a".into());
            let mlx5 = p.driver.as_deref() == Some("mlx5_core") && !i.mlx5_devargs.is_empty();
            argv.push(if mlx5 {
                format!("{},{}", p.name, i.mlx5_devargs)
            } else {
                p.name.clone()
            });
        }
    }
    argv.extend(i.eal_extra_args.iter().cloned());
    argv.push("--".into());
    argv.push("--links".into());
    argv.push(i.links_file.to_string_lossy().into_owned());
    argv
}

/// The `--links` file (v2): one entry per link with its `dpds_core::Config` JSON and the
/// explicit lcore of its direction plan; `histogram` only when the link has histogram
/// settings and `delay_stats: false` only when delay statistics are off.
pub fn links_file_json(links: &[Link], directions: &[DirectionPlan]) -> Result<Value, DtoError> {
    let mut out = Vec::with_capacity(links.len());
    for l in links {
        let core = l.config.to_core()?;
        let config = serde_json::to_value(core).map_err(|e| DtoError::new(e.to_string()))?;
        let mut entry = json!({
            "name": l.name,
            "ingress": l.ingress,
            "egress": l.egress,
            "config": config,
        });
        if let Some(d) = directions.iter().find(|d| d.link == l.name) {
            entry["lcore"] = json!(d.lcore);
        }
        if !l.config.delay_stats {
            entry["delay_stats"] = json!(false);
        }
        if l.config.histogram.is_some() {
            entry["histogram"] = histogram_json(&l.config);
        }
        out.push(entry);
    }
    Ok(json!({ "links": out }))
}

fn histogram_json(c: &ConfigDto) -> Value {
    serde_json::to_value(c.histogram.clone().unwrap_or_default()).unwrap_or_else(|_| json!({}))
}

/// How long `dpds` may need to drain: the longest `mean + 6 std` (plus
/// `limit / rate` when the limiter is on) over all directions, plus a margin.
pub fn drain_timeout(configs: &[Config]) -> Duration {
    let longest_ns = configs
        .iter()
        .map(|c| {
            let limiter = if c.limiter.activate && c.limiter.rate_bps > 0.0 {
                c.limiter.limit_bit / c.limiter.rate_bps * 1e9
            } else {
                0.0
            };
            c.delayer.mean_ns + 6.0 * c.delayer.std_ns + limiter
        })
        .fold(0.0_f64, f64::max);
    // capped so absurd configs cannot overflow the Duration
    Duration::from_secs_f64((longest_ns / 1e9).clamp(0.0, 1e6)) + DRAIN_MARGIN
}
