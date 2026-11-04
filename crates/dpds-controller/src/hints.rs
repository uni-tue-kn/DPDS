//! Read-only host hints (B6), shared by `serve` and `check`. Nothing here
//! changes the host: every hint only reports and suggests.
//!
//! Thresholds come from the DPDS README ("Performance Tuning"): isolated
//! cores with `nohz_full` and `rcu_nocbs`, hugepages for the mbuf pools,
//! workers on the NIC's NUMA node, `performance` governor, interrupts off the
//! worker cores, MaxReadReq 4096 (1024 and 2048 are acceptable).

use std::fs;
use std::time::Duration;

use serde::Serialize;
use utoipa::ToSchema;

use crate::discovery::{discover, find, Interface};
use crate::eal::drain_timeout;
use crate::host::{fmt_cpus, read_trim, HostRoots};
use crate::lcores::{assign_lcores, Assignment, LcoreError};
use crate::model::RunConfig;

/// Mbuf size in hugepage memory (README: about 1.9 KB per mbuf).
const BYTES_PER_MBUF: u64 = 1920;
/// Descriptors per ring `dpds` asks for, plus the pool's cache and burst slack
/// (README pool formula: capacity + rx + tx + 768 + 64).
const POOL_EXTRA: u64 = 4096 + 4096 + 768 + 64;
/// `TimeoutStopSec` of `packaging/dpds-controller.service`; systemd SIGKILLs the
/// whole cgroup after it, in the middle of a drain.
const SYSTEMD_STOP_LIMIT_S: u64 = 600;
/// A drain timeout above this leaves less than 10 s of the limit for the rest of the stop.
const DRAIN_WARN_S: u64 = 590;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Level::Ok => "OK",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Hint {
    pub id: String,
    pub level: Level,
    pub title: String,
    pub detail: String,
    pub suggestion: String,
}

fn hint(id: &str, level: Level, title: &str, detail: String, suggestion: &str) -> Hint {
    Hint {
        id: id.into(),
        level,
        title: title.into(),
        detail,
        suggestion: suggestion.into(),
    }
}

struct Ctx<'a> {
    roots: &'a HostRoots,
    config: &'a RunConfig,
    interfaces: Vec<Interface>,
    assignment: Result<Assignment, LcoreError>,
}

impl Ctx<'_> {
    /// Discovered NICs used by the links.
    fn used_nics(&self) -> Vec<&Interface> {
        let mut used: Vec<&Interface> = Vec::new();
        for port in self
            .config
            .links
            .iter()
            .flat_map(|l| [&l.ingress, &l.egress])
        {
            if let Some(i) = find(&self.interfaces, port) {
                if !used.iter().any(|u| u.pci == i.pci) {
                    used.push(i);
                }
            }
        }
        used
    }
}

/// All hints, in a fixed order, one per check.
pub fn compute_hints(roots: &HostRoots, config: &RunConfig) -> Vec<Hint> {
    let ctx = Ctx {
        roots,
        config,
        interfaces: discover(roots),
        assignment: assign_lcores(roots, config),
    };
    let mut hints: Vec<Hint> = management_nic(&ctx).into_iter().collect();
    hints.extend([
        isolation(&ctx),
        isolated_cores(&ctx),
        hugepages(&ctx),
        numa(&ctx),
        governor(&ctx),
        irqbalance(&ctx),
        iommu(&ctx),
        mtu(&ctx),
        maxreadreq(&ctx),
    ]);
    hints.extend(drain(&ctx));
    hints
}

// ---- drain vs systemd --------------------------------------------------------

/// A warning when a link's drain timeout (the controller's stop wait)
/// exceeds what the systemd unit allows.
fn drain(ctx: &Ctx) -> Option<Hint> {
    let slow: Vec<String> = ctx
        .config
        .links
        .iter()
        .filter_map(|l| {
            let configs: Vec<_> = l.config.to_core().ok().into_iter().collect();
            let t = drain_timeout(&configs);
            (t > Duration::from_secs(DRAIN_WARN_S))
                .then(|| format!("{} ({} s)", l.name, t.as_secs()))
        })
        .collect();
    (!slow.is_empty()).then(|| {
        hint(
            "drain_timeout",
            Level::Warn,
            "Drain timeout",
            format!(
                "stopping waits for the buffered packets to leave; link(s) {} need more than {DRAIN_WARN_S} s, but the systemd unit allows TimeoutStopSec={SYSTEMD_STOP_LIMIT_S}, after which systemd kills dpds mid-drain",
                slow.join(", ")
            ),
            "Shorten the delay, shrink the buffer capacity or raise the limiter rate (the drain takes capacity × 12336 bit / rate_bps) of those links, or raise TimeoutStopSec in dpds-controller.service.",
        )
    })
}

// ---- management NIC ----------------------------------------------------------

/// An error when a link uses a NIC that carries a kernel route.
fn management_nic(ctx: &Ctx) -> Option<Hint> {
    let routed: Vec<String> = ctx
        .used_nics()
        .into_iter()
        .filter(|i| i.kernel_route)
        .map(|i| {
            format!(
                "{}: {}",
                i.pci,
                i.reason.as_deref().unwrap_or("kernel route")
            )
        })
        .collect();
    (!routed.is_empty()).then(|| {
        hint(
            "management_nic",
            Level::Error,
            "Management NIC",
            format!("a link uses a NIC the host routes over: {}", routed.join("; ")),
            "Use NICs without a kernel route; binding the management NIC to DPDK cuts the host off the network. Start is refused.",
        )
    })
}

// ---- isolation -------------------------------------------------------------

fn isolation(ctx: &Ctx) -> Hint {
    let cmdline = ctx.roots.cmdline();
    let has = |key: &str| cmdline.split_whitespace().any(|w| w.starts_with(key));
    let isolcpus = has("isolcpus=") || !ctx.roots.isolated_cpus().is_empty();
    let nohz_full = has("nohz_full=")
        || read_trim(&ctx.roots.sys("devices/system/cpu/nohz_full"))
            .is_some_and(|s| !s.is_empty() && s != "(null)");
    let rcu_nocbs = has("rcu_nocbs=");
    let missing: Vec<&str> = [
        ("isolcpus", isolcpus),
        ("nohz_full", nohz_full),
        ("rcu_nocbs", rcu_nocbs),
    ]
    .iter()
    .filter(|(_, present)| !present)
    .map(|(name, _)| *name)
    .collect();
    let title = "Core isolation";
    if missing.is_empty() {
        hint(
            "isolation",
            Level::Ok,
            title,
            "isolcpus, nohz_full and rcu_nocbs are all set".into(),
            "",
        )
    } else if missing.len() == 3 {
        hint(
            "isolation",
            Level::Error,
            title,
            "none of isolcpus, nohz_full and rcu_nocbs is set; a busy-polling worker core can starve the kernel and make the host unreachable".into(),
            "Boot with isolcpus=<cores> nohz_full=<cores> rcu_nocbs=<cores> for the worker cores (see the README).",
        )
    } else {
        hint(
            "isolation",
            Level::Warn,
            title,
            format!("missing kernel parameter(s): {}", missing.join(", ")),
            "Set isolcpus, nohz_full and rcu_nocbs to the same worker cores; without nohz_full and rcu_nocbs a spinning worker can starve RCU.",
        )
    }
}

fn isolated_cores(ctx: &Ctx) -> Hint {
    let title = "Isolated cores";
    let isolated = ctx.roots.isolated_cpus();
    let dirs = ctx.config.links.len();
    if dirs == 0 {
        return if isolated.is_empty() {
            hint(
                "isolated_cores",
                Level::Info,
                title,
                "no cores are isolated and no links are configured yet".into(),
                "Isolate one core per link.",
            )
        } else {
            hint(
                "isolated_cores",
                Level::Ok,
                title,
                format!(
                    "{} isolated core(s): {}",
                    isolated.len(),
                    fmt_cpus(&isolated)
                ),
                "",
            )
        };
    }
    match &ctx.assignment {
        Err(e) => hint("isolated_cores", Level::Error, title, e.to_string(), "Isolate more cores (isolcpus), or reduce the number of links, or pin worker cores per link."),
        Ok(a) => {
            let outside: Vec<u32> = a
                .directions
                .iter()
                .map(|d| d.lcore)
                .filter(|c| !isolated.contains(c))
                .collect();
            if outside.is_empty() {
                hint("isolated_cores", Level::Ok, title, format!("{dirs} link(s) run on isolated cores"), "")
            } else {
                hint("isolated_cores", Level::Warn, title, format!("worker core(s) {} are not isolated", fmt_cpus(&outside)), "Pin only isolated cores as workers.")
            }
        }
    }
}

// ---- hugepages -------------------------------------------------------------

/// (total, free) hugepage memory in bytes over all page sizes.
fn hugepage_bytes(roots: &HostRoots) -> (u64, u64) {
    let (mut total, mut free) = (0u64, 0u64);
    let mut found = false;
    if let Ok(entries) = fs::read_dir(roots.sys("kernel/mm/hugepages")) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(kb) = name
                .strip_prefix("hugepages-")
                .and_then(|s| s.strip_suffix("kB"))
                .and_then(|s| s.parse::<u64>().ok())
            else {
                continue;
            };
            let read = |f: &str| read_trim(&e.path().join(f)).and_then(|s| s.parse::<u64>().ok());
            if let (Some(t), Some(f)) = (read("nr_hugepages"), read("free_hugepages")) {
                found = true;
                total = total.saturating_add(t.saturating_mul(kb.saturating_mul(1024)));
                free = free.saturating_add(f.saturating_mul(kb.saturating_mul(1024)));
            }
        }
    }
    if !found {
        let meminfo = read_trim(&roots.proc("meminfo")).unwrap_or_default();
        let field = |key: &str| {
            meminfo
                .lines()
                .find_map(|l| l.strip_prefix(key))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        };
        let page = field("Hugepagesize:").saturating_mul(1024);
        total = field("HugePages_Total:").saturating_mul(page);
        free = field("HugePages_Free:").saturating_mul(page);
    }
    (total, free)
}

fn mib(bytes: u64) -> String {
    format!("{} MiB", bytes / (1024 * 1024))
}

fn hugepages(ctx: &Ctx) -> Hint {
    let title = "Hugepages";
    let need: u64 = ctx
        .config
        .links
        .iter()
        .map(|l| &l.config)
        .map(|c| {
            (c.buffer_capacity as u64)
                .saturating_add(POOL_EXTRA)
                .saturating_mul(BYTES_PER_MBUF)
        })
        .fold(0u64, u64::saturating_add);
    let (total, free) = hugepage_bytes(ctx.roots);
    let no_huge = ctx
        .config
        .settings
        .eal_extra_args
        .iter()
        .any(|a| a == "--no-huge");
    if no_huge {
        return hint(
            "hugepages",
            Level::Info,
            title,
            "the settings pass --no-huge, so no hugepages are used".into(),
            "Use hugepages for real traffic.",
        );
    }
    let fix = "Reserve hugepages at boot (hugepagesz=2M hugepages=<n> or 1G pages) or via sysctl vm.nr_hugepages.";
    if total == 0 {
        let level = if need > 0 { Level::Error } else { Level::Warn };
        return hint(
            "hugepages",
            level,
            title,
            "no hugepages are configured; dpds cannot create its mbuf pools".into(),
            fix,
        );
    }
    let detail = format!(
        "{} free of {}, the links need about {}",
        mib(free),
        mib(total),
        mib(need)
    );
    if (free as f64) < need as f64 * 1.1 {
        hint(
            "hugepages",
            Level::Error,
            title,
            format!("{detail}; starting would fail with a pool creation error"),
            fix,
        )
    } else if (free as f64) < need as f64 * 1.5 {
        hint(
            "hugepages",
            Level::Warn,
            title,
            format!("{detail}; little headroom"),
            fix,
        )
    } else {
        hint("hugepages", Level::Ok, title, detail, "")
    }
}

// ---- NUMA ------------------------------------------------------------------

fn numa(ctx: &Ctx) -> Hint {
    let title = "NUMA locality";
    let a = match &ctx.assignment {
        Ok(a) if !a.directions.is_empty() => a,
        Ok(_) => {
            return hint(
                "numa",
                Level::Ok,
                title,
                "no links are configured".into(),
                "",
            )
        }
        Err(_) => {
            return hint(
                "numa",
                Level::Info,
                title,
                "not evaluated: the lcore assignment fails (see the isolated cores hint)".into(),
                "",
            )
        }
    };
    let mut remote = Vec::new();
    let mut known = false;
    for d in &a.directions {
        let nic = ctx.roots.pci_numa_node(&d.rx);
        let cpu = ctx.roots.node_of_cpu(d.lcore);
        if let (Some(nic), Some(cpu)) = (nic, cpu) {
            known = true;
            if nic != cpu {
                remote.push(format!(
                    "link {} on core {} (node {cpu}) receives on {} (node {nic})",
                    d.link, d.lcore, d.rx
                ));
            }
        }
    }
    if !remote.is_empty() {
        hint(
            "numa",
            Level::Warn,
            title,
            remote.join("; "),
            "Use worker cores on the NUMA node of the receiving NIC.",
        )
    } else if known {
        hint(
            "numa",
            Level::Ok,
            title,
            "workers run on the NUMA node of their NIC".into(),
            "",
        )
    } else {
        hint(
            "numa",
            Level::Ok,
            title,
            "no NUMA information for the used NICs (virtual devices or a VM)".into(),
            "",
        )
    }
}

// ---- CPU governor, irqbalance ------------------------------------------------

fn governor(ctx: &Ctx) -> Hint {
    let title = "CPU governor";
    let cpus: Vec<u32> = match &ctx.assignment {
        Ok(a) if !a.directions.is_empty() => a.directions.iter().map(|d| d.lcore).collect(),
        _ => {
            let iso = ctx.roots.isolated_cpus();
            if iso.is_empty() {
                ctx.roots.online_cpus()
            } else {
                iso
            }
        }
    };
    let not_performance: Vec<String> = cpus
        .iter()
        .filter_map(|c| {
            let g = read_trim(&ctx.roots.sys(&format!(
                "devices/system/cpu/cpu{c}/cpufreq/scaling_governor"
            )))?;
            (g != "performance").then(|| format!("cpu{c}: {g}"))
        })
        .collect();
    let known = cpus.iter().any(|c| {
        ctx.roots
            .sys(&format!(
                "devices/system/cpu/cpu{c}/cpufreq/scaling_governor"
            ))
            .exists()
    });
    if !not_performance.is_empty() {
        hint(
            "governor",
            Level::Warn,
            title,
            not_performance.join(", "),
            "Set the governor of the worker cores to performance.",
        )
    } else if known {
        hint(
            "governor",
            Level::Ok,
            title,
            "worker cores use the performance governor".into(),
            "",
        )
    } else {
        hint(
            "governor",
            Level::Info,
            title,
            "cpufreq is not available (virtualised host?)".into(),
            "",
        )
    }
}

fn irqbalance(ctx: &Ctx) -> Hint {
    let title = "irqbalance";
    let running = fs::read_dir(&ctx.roots.procfs).is_ok_and(|d| {
        d.flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
                && read_trim(&e.path().join("comm")).as_deref() == Some("irqbalance")
        })
    });
    if running {
        hint("irqbalance", Level::Warn, title, "the irqbalance daemon is running and may move interrupts onto the worker cores".into(), "Stop irqbalance or ban the worker cores (IRQBALANCE_BANNED_CPULIST) and keep NIC interrupts off them.")
    } else {
        hint(
            "irqbalance",
            Level::Ok,
            title,
            "irqbalance is not running".into(),
            "",
        )
    }
}

// ---- IOMMU, MTU, PCIe --------------------------------------------------------

fn iommu(ctx: &Ctx) -> Hint {
    let title = "IOMMU";
    let vfio: Vec<&str> = ctx
        .used_nics()
        .into_iter()
        .filter(|i| i.driver.as_deref() == Some("vfio-pci"))
        .map(|i| i.pci.as_str())
        .collect();
    if vfio.is_empty() {
        return hint(
            "iommu",
            Level::Ok,
            title,
            "no vfio-pci NIC is used, so no IOMMU is needed".into(),
            "",
        );
    }
    let enabled = fs::read_dir(ctx.roots.sys("class/iommu")).is_ok_and(|mut d| d.next().is_some());
    if !enabled {
        return hint("iommu", Level::Warn, title, format!("{} use vfio-pci but no IOMMU is active", vfio.join(", ")), "Boot with intel_iommu=on iommu=pt (or amd_iommu=on iommu=pt), and enable VT-d/AMD-Vi in the firmware.");
    }
    if ctx
        .roots
        .cmdline()
        .split_whitespace()
        .any(|w| w == "iommu=pt")
    {
        hint(
            "iommu",
            Level::Ok,
            title,
            "the IOMMU is active in passthrough mode".into(),
            "",
        )
    } else {
        hint(
            "iommu",
            Level::Info,
            title,
            "the IOMMU is active but iommu=pt is not set".into(),
            "Add iommu=pt to the kernel command line to avoid IOMMU translation overhead.",
        )
    }
}

fn mtu(ctx: &Ctx) -> Hint {
    let title = "MTU";
    let small: Vec<String> = ctx
        .used_nics()
        .into_iter()
        .filter_map(|i| {
            let m = i.mtu?;
            (m < 1500).then(|| format!("{} has {m}", i.netdev.as_deref().unwrap_or(&i.pci)))
        })
        .collect();
    if small.is_empty() {
        hint(
            "mtu",
            Level::Ok,
            title,
            "used NICs have an MTU of at least 1500".into(),
            "",
        )
    } else {
        hint(
            "mtu",
            Level::Warn,
            title,
            format!(
                "{}; dpds runs at 1500 and sizes its buffers for it",
                small.join(", ")
            ),
            "Set the interface MTU to at least 1500.",
        )
    }
}

/// MaxReadReq in bytes from the PCIe capability in the PCI config space.
fn max_read_req(config: &[u8]) -> Option<u32> {
    let mut ptr = usize::from(*config.get(0x34)? & 0xfc);
    for _ in 0..48 {
        if ptr == 0 {
            return None;
        }
        let (id, next) = (*config.get(ptr)?, *config.get(ptr + 1)?);
        if id == 0x10 {
            let ctl = u16::from_le_bytes([*config.get(ptr + 8)?, *config.get(ptr + 9)?]);
            return match (ctl >> 12) & 7 {
                code @ 0..=5 => Some(128 << code),
                _ => None,
            };
        }
        ptr = usize::from(next & 0xfc);
    }
    None
}

fn maxreadreq(ctx: &Ctx) -> Hint {
    let title = "PCIe MaxReadReq";
    let nics = ctx.used_nics();
    if nics.is_empty() {
        return hint(
            "pcie_maxreadreq",
            Level::Ok,
            title,
            "no PCI NIC is used".into(),
            "",
        );
    }
    let (mut level, mut details) = (Level::Ok, Vec::new());
    let mut unreadable = Vec::new();
    for n in nics {
        match fs::read(ctx.roots.pci_dir(&n.pci).join("config"))
            .ok()
            .as_deref()
            .and_then(max_read_req)
        {
            None => unreadable.push(n.pci.clone()),
            Some(b) => {
                let l = match b {
                    0..=1023 => Level::Warn,
                    1024..=4095 => Level::Info,
                    _ => Level::Ok,
                };
                level = level.max(l);
                details.push(format!("{}: {b} B", n.pci));
            }
        }
    }
    if !unreadable.is_empty() {
        level = level.max(Level::Info);
        details.push(format!(
            "{}: not readable (needs root and a PCIe device)",
            unreadable.join(", ")
        ));
    }
    let suggestion = if level == Level::Ok {
        ""
    } else {
        "Raise the PCIe maximum read request size to 4096 B (the README shows how); 1024 B and above is acceptable."
    };
    hint(
        "pcie_maxreadreq",
        level,
        title,
        details.join("; "),
        suggestion,
    )
}
