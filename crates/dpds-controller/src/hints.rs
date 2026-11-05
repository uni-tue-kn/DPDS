//! Host hints (B6), shared by `serve` and `check`. Computing them changes nothing: a hint that
//! can be fixed lists the exact shell commands an administrator would run as root
//! (`Hint::commands`, display strings only, never executed), computed from what the controller
//! read. The only tools it runs are read-only queries (see `nic_hints`). The kernel command line
//! checks name the parameters to add and have no commands.
//!
//! Thresholds come from the DPDS README ("Performance Tuning"): isolated
//! cores with `nohz_full` and `rcu_nocbs`, hugepages for the mbuf pools,
//! workers on the NIC's NUMA node, `performance` governor, interrupts off the
//! isolated cores, MaxReadReq 4096 (1024 and 2048 are acceptable).

use std::fs;
use std::time::{Duration, Instant};

use serde::Serialize;
use utoipa::ToSchema;

use crate::discovery::{discover, find, Interface};
use crate::eal::drain_timeout;
use crate::host::{fmt_cpus, parse_cpulist, read_trim, HostRoots};
use crate::lcores::{assign_lcores, Assignment, LcoreError};
use crate::model::RunConfig;
use crate::nic_hints::{blocked, nic_hints, FirmwareCache};

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
    /// The shell commands an administrator runs as root to enable this tuning on this host, one
    /// per element. DPDS never runs them. Absent when the hint suggests none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

impl Hint {
    pub fn with_commands(mut self, commands: Vec<String>) -> Hint {
        self.commands = commands;
        self
    }
}

pub(crate) fn hint(id: &str, level: Level, title: &str, detail: String, suggestion: &str) -> Hint {
    Hint {
        id: id.into(),
        level,
        title: title.into(),
        detail,
        suggestion: suggestion.into(),
        commands: Vec::new(),
    }
}

pub(crate) struct Ctx<'a> {
    pub roots: &'a HostRoots,
    /// Whether tools may run: not while an emulation is starting, running or stopping.
    pub run_tools: bool,
    /// When this request began its tool time budget, read from `roots.exec.now()`.
    pub started: Instant,
    /// Where the firmware query results are kept between requests; `None` keeps nothing.
    pub cache: Option<&'a FirmwareCache>,
    /// Drop the cached firmware results of the used NICs first.
    pub recheck: bool,
    config: &'a RunConfig,
    interfaces: Vec<Interface>,
    assignment: Result<Assignment, LcoreError>,
}

impl Ctx<'_> {
    /// Discovered NICs used by the links.
    pub(crate) fn used_nics(&self) -> Vec<&Interface> {
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

/// The text with every control character spelled `\xNN`, so a name read from the host cannot
/// move the cursor or recolour a terminal.
pub fn escape_control(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() {
                format!("\\x{:02x}", u32::from(c))
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// How a NIC is named in a hint: its netdev, else its PCI address, control characters escaped.
pub(crate) fn nic_label(nic: &Interface) -> String {
    escape_control(nic.netdev.as_deref().unwrap_or(&nic.pci))
}

/// All hints, in a fixed order, one per check.
pub fn compute_hints(roots: &HostRoots, config: &RunConfig, run_tools: bool) -> Vec<Hint> {
    compute(roots, config, run_tools, None, false)
}

/// Like `compute_hints`, but the firmware query results are kept in `cache` and read from it; with
/// `recheck` the results of the NICs of `config` are dropped first (only when tools may run).
pub fn compute_hints_cached(
    roots: &HostRoots,
    config: &RunConfig,
    run_tools: bool,
    cache: &FirmwareCache,
    recheck: bool,
) -> Vec<Hint> {
    compute(roots, config, run_tools, Some(cache), recheck)
}

fn compute(
    roots: &HostRoots,
    config: &RunConfig,
    run_tools: bool,
    cache: Option<&FirmwareCache>,
    recheck: bool,
) -> Vec<Hint> {
    let ctx = Ctx {
        roots,
        run_tools,
        started: roots.exec.now(),
        cache,
        recheck,
        config,
        interfaces: discover(roots),
        assignment: assign_lcores(roots, config),
    };
    let mut hints: Vec<Hint> = uplink(&ctx).into_iter().collect();
    hints.extend([
        isolation(&ctx),
        isolated_cores(&ctx),
        hugepages(&ctx),
        numa(&ctx),
        governor(&ctx),
        irq_affinity(&ctx),
        iommu(&ctx),
        mtu(&ctx),
        swappiness(&ctx),
        zone_reclaim_mode(&ctx),
        rcu_nocb_poll(&ctx),
        boot_hugepages(&ctx),
    ]);
    hints.extend(nic_hints(&ctx));
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

// ---- host uplink -------------------------------------------------------------

/// An error when a link uses a NIC the host itself uses.
fn uplink(ctx: &Ctx) -> Option<Hint> {
    let uplinks: Vec<String> = ctx
        .used_nics()
        .into_iter()
        .filter(|i| i.uplink)
        .map(|i| {
            format!(
                "{}: {}",
                i.pci,
                i.reason.as_deref().unwrap_or("host uplink")
            )
        })
        .collect();
    (!uplinks.is_empty()).then(|| {
        hint(
            "uplink",
            Level::Error,
            "Host uplink",
            format!("a link uses a NIC the host uses: {}", uplinks.join("; ")),
            "Use NICs the host does not use (no IP address, no route, not in Open vSwitch); binding the host's uplink to DPDK cuts the host off the network. Start is refused.",
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
        .map(link_need)
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
    let fix = "Reserve hugepages at boot, which is the reliable way (default_hugepagesz=1G hugepagesz=1G hugepages=<n>, or hugepagesz=2M hugepages=<n>), or at run time via sysctl vm.nr_hugepages.";
    // Commands need the links (the need depends on them) and free < need
    let with_commands = |mut h: Hint| -> Hint {
        if (free as f64) >= need as f64 || need == 0 {
            return h;
        }
        if let Some((commands, note)) = hugepage_commands(ctx) {
            h.detail.push_str(&note);
            h = h.with_commands(commands);
        }
        h
    };
    if total == 0 {
        let level = if need > 0 { Level::Error } else { Level::Warn };
        return with_commands(hint(
            "hugepages",
            level,
            title,
            "no hugepages are configured; dpds cannot create its mbuf pools".into(),
            fix,
        ));
    }
    let detail = format!(
        "{} free of {}, the links need about {}",
        mib(free),
        mib(total),
        mib(need)
    );
    if (free as f64) < need as f64 * 1.1 {
        with_commands(hint(
            "hugepages",
            Level::Error,
            title,
            format!("{detail}; starting would fail with a pool creation error"),
            fix,
        ))
    } else if (free as f64) < need as f64 * 1.5 {
        with_commands(hint(
            "hugepages",
            Level::Warn,
            title,
            format!("{detail}; little headroom"),
            fix,
        ))
    } else {
        hint("hugepages", Level::Ok, title, detail, "")
    }
}

/// Memory the mbuf pools of one link need.
fn link_need(c: &crate::dto::ConfigDto) -> u64 {
    (c.buffer_capacity as u64)
        .saturating_add(POOL_EXTRA)
        .saturating_mul(BYTES_PER_MBUF)
}

/// `MemAvailable` of `/proc/meminfo` in bytes.
fn mem_available(roots: &HostRoots) -> Option<u64> {
    read_trim(&roots.proc("meminfo"))?
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

/// The commands that raise `nr_hugepages` of the default hugepage size until the links' pools fit
/// with 50 % headroom (the level of the "little headroom" warning), and a note for the detail.
/// Per NUMA node of the links' ingress NICs when every one is known, else for the whole host.
/// Together they never add more than half of `MemAvailable`; the note names `MemAvailable` when
/// that keeps the commands from the full target. `None` when the counts cannot be read.
fn hugepage_commands(ctx: &Ctx) -> Option<(Vec<String>, String)> {
    let roots = ctx.roots;
    let kb: u64 = read_trim(&roots.proc("meminfo"))?
        .lines()
        .find_map(|l| l.strip_prefix("Hugepagesize:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let page = kb.checked_mul(1024).filter(|p| *p > 0)?;
    let size = format!("hugepages-{kb}kB");
    let wanted = |need: u64| need.saturating_mul(3) / 2;
    let count = |dir: &std::path::Path, file: &str| {
        read_trim(&dir.join(file)).and_then(|s| s.parse::<u64>().ok())
    };
    // (node, need) per link, by the NUMA node of the ingress NIC
    let per_link: Vec<(Option<u32>, u64)> = ctx
        .config
        .links
        .iter()
        .map(|l| (roots.pci_numa_node(&l.ingress), link_need(&l.config)))
        .collect();
    let node_dir = |n: u32| roots.sys(&format!("devices/system/node/node{n}/hugepages/{size}"));
    let mut nodes: Vec<(Option<u32>, u64)> = Vec::new();
    let all_known = per_link
        .iter()
        .all(|(n, _)| n.is_some_and(|n| node_dir(n).is_dir()));
    for (node, need) in per_link {
        let key = if all_known { node } else { None };
        match nodes.iter_mut().find(|(n, _)| *n == key) {
            Some((_, sum)) => *sum = sum.saturating_add(need),
            None => nodes.push((key, need)),
        }
    }
    let Some(available) = mem_available(roots) else {
        return Some((
            Vec::new(),
            "; no command is suggested because MemAvailable in /proc/meminfo is not readable"
                .into(),
        ));
    };
    let mut budget = available / 2 / page;
    let mut capped = false;
    let mut commands = Vec::new();
    for (node, need) in nodes {
        // the display path of the file an administrator writes
        let dir = match node {
            Some(n) => format!("/sys/devices/system/node/node{n}/hugepages/{size}"),
            None => format!("/sys/kernel/mm/hugepages/{size}"),
        };
        let files = match node {
            Some(n) => node_dir(n),
            None => roots.sys(&format!("kernel/mm/hugepages/{size}")),
        };
        let (nr, free) = (
            count(&files, "nr_hugepages")?,
            count(&files, "free_hugepages")?,
        );
        let missing = wanted(need)
            .saturating_sub(free.saturating_mul(page))
            .div_ceil(page);
        if missing == 0 {
            continue;
        }
        let add = missing.min(budget);
        capped |= add < missing;
        budget -= add;
        if add > 0 {
            commands.push(format!(
                "echo {} > {dir}/nr_hugepages",
                nr.saturating_add(add)
            ));
        }
    }
    let note = if capped {
        format!(
            "; MemAvailable is {}, and at most half of it ({} pages of {} MiB) is suggested, so the commands stay below the full target",
            mib(available),
            available / 2 / page,
            page / (1024 * 1024)
        )
    } else {
        String::new()
    };
    Some((commands, note))
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

// ---- CPU governor, NIC interrupts ----------------------------------------------

fn governor(ctx: &Ctx) -> Hint {
    let title = "CPU governor";
    let workers: Vec<u32> = match &ctx.assignment {
        Ok(a) if !a.directions.is_empty() => a.directions.iter().map(|d| d.lcore).collect(),
        _ => Vec::new(),
    };
    let isolated = ctx.roots.isolated_cpus();
    let cpus: Vec<u32> = if !workers.is_empty() {
        workers.clone()
    } else if isolated.is_empty() {
        ctx.roots.online_cpus()
    } else {
        isolated.clone()
    };
    let cpufreq = |c: u32, file: &str| {
        ctx.roots
            .sys(&format!("devices/system/cpu/cpu{c}/cpufreq/{file}"))
    };
    let not_performance: Vec<String> = cpus
        .iter()
        .filter_map(|c| {
            let g = read_trim(&cpufreq(*c, "scaling_governor"))?;
            (g != "performance").then(|| format!("cpu{c}: {g}"))
        })
        .collect();
    let known = cpus
        .iter()
        .any(|c| cpufreq(*c, "scaling_governor").exists());
    if !not_performance.is_empty() {
        // the worker cores and the isolated cores that can switch to performance
        let mut targets: Vec<u32> = workers.iter().chain(&isolated).copied().collect();
        targets.sort_unstable();
        targets.dedup();
        let commands: Vec<String> = targets
            .into_iter()
            .filter(|c| {
                read_trim(&cpufreq(*c, "scaling_available_governors"))
                    .is_some_and(|l| l.split_whitespace().any(|g| g == "performance"))
            })
            .map(|c| {
                format!(
                    "echo performance > /sys/devices/system/cpu/cpu{c}/cpufreq/scaling_governor"
                )
            })
            .collect();
        hint(
            "governor",
            Level::Warn,
            title,
            not_performance.join(", "),
            "Set the governor of the worker cores to performance.",
        )
        .with_commands(commands)
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

/// The PCI interrupts of a device: the numbers in `msi_irqs/`.
fn msi_irqs(roots: &HostRoots, pci: &str) -> Vec<u32> {
    let mut irqs: Vec<u32> = fs::read_dir(roots.pci_dir(pci).join("msi_irqs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().parse().ok())
        .collect();
    irqs.sort_unstable();
    irqs
}

fn irqbalance_running(roots: &HostRoots) -> bool {
    fs::read_dir(&roots.procfs).is_ok_and(|d| {
        d.flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
                && read_trim(&e.path().join("comm")).as_deref() == Some("irqbalance")
        })
    })
}

/// NIC interrupts of the used NICs that run on isolated cores.
fn irq_affinity(ctx: &Ctx) -> Hint {
    let (id, title) = ("irq_affinity", "NIC interrupts");
    let nics = ctx.used_nics();
    if nics.is_empty() {
        return hint(id, Level::Ok, title, "no PCI NIC is used".into(), "");
    }
    // a NIC the host uses or DPDS cannot use gets no command, so its interrupts are not looked at
    let (usable, skipped): (Vec<&Interface>, Vec<&Interface>) =
        nics.into_iter().partition(|n| blocked(n).is_none());
    let skipped = if skipped.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = skipped
            .iter()
            .map(|n| {
                format!(
                    "{} ({}: {})",
                    nic_label(n),
                    n.pci,
                    blocked(n).unwrap_or_default()
                )
            })
            .collect();
        format!(
            "; skipped, no command is suggested for: {}",
            list.join(", ")
        )
    };
    if usable.is_empty() {
        return hint(
            id,
            Level::Info,
            title,
            format!("the interrupts of the used NICs are not checked{skipped}"),
            "",
        );
    }
    let mut isolated = ctx.roots.isolated_cpus();
    isolated.extend(ctx.roots.nohz_full_cpus());
    isolated.sort_unstable();
    isolated.dedup();
    if isolated.is_empty() {
        return hint(
            id,
            Level::Ok,
            title,
            format!("no cores are isolated, so no interrupt runs on one (see the core isolation hint){skipped}"),
            "",
        );
    }
    let housekeeping: Vec<u32> = ctx
        .roots
        .online_cpus()
        .into_iter()
        .filter(|c| !isolated.contains(c))
        .collect();
    // (irq, NIC, cores it runs on, cores to move it to)
    let mut on_isolated: Vec<(u32, String, Vec<u32>, Vec<u32>)> = Vec::new();
    for nic in usable {
        let label = nic_label(nic);
        // the housekeeping cores of the NIC's node, else all of them
        let local: Vec<u32> = ctx
            .roots
            .pci_numa_node(&nic.pci)
            .map(|n| ctx.roots.node_cpus(n))
            .unwrap_or_default()
            .into_iter()
            .filter(|c| housekeeping.contains(c))
            .collect();
        let target = if local.is_empty() {
            &housekeeping
        } else {
            &local
        };
        for irq in msi_irqs(ctx.roots, &nic.pci) {
            let dir = ctx.roots.proc(&format!("irq/{irq}"));
            let cpus = read_trim(&dir.join("effective_affinity_list"))
                .or_else(|| read_trim(&dir.join("smp_affinity_list")))
                .map(|s| parse_cpulist(&s))
                .unwrap_or_default();
            if cpus.iter().any(|c| isolated.contains(c)) {
                on_isolated.push((irq, label.clone(), cpus, target.clone()));
            }
        }
    }
    let balance = if irqbalance_running(ctx.roots) {
        "; the irqbalance daemon is running and may move interrupts back onto the isolated cores unless it bans them (IRQBALANCE_BANNED_CPULIST)"
    } else {
        ""
    };
    if on_isolated.is_empty() {
        return hint(
            id,
            Level::Ok,
            title,
            format!("NIC interrupts are off the isolated cores{balance}{skipped}"),
            "",
        );
    }
    let list = on_isolated
        .iter()
        .map(|(irq, nic, cpus, _)| format!("IRQ {irq} of {nic} on core(s) {}", fmt_cpus(cpus)))
        .collect::<Vec<_>>()
        .join(", ");
    let mut targets: Vec<String> = Vec::new();
    for (_, nic, _, target) in &on_isolated {
        let t = format!("{nic}: {}", fmt_cpus(target));
        if !targets.contains(&t) {
            targets.push(t);
        }
    }
    let h = hint(
        id,
        Level::Warn,
        title,
        format!(
            "{list}; the isolated cores are {}{balance}{skipped}. The commands move them to the non-isolated online cores of the NIC's NUMA node, or to all of them when the node has none ({}); the kernel refuses to move managed interrupts",
            fmt_cpus(&isolated),
            targets.join("; ")
        ),
        "Move the NIC interrupts to the housekeeping cores (smp_affinity_list) and keep irqbalance off the isolated cores.",
    );
    if housekeeping.is_empty() {
        return h;
    }
    on_isolated.sort_by_key(|(irq, ..)| *irq);
    h.with_commands(
        on_isolated
            .iter()
            .map(|(irq, _, _, target)| {
                format!(
                    "echo {} > /proc/irq/{irq}/smp_affinity_list",
                    fmt_cpus(target)
                )
            })
            .collect(),
    )
}

// ---- VM tunables ---------------------------------------------------------------

/// A `vm.*` sysctl that should be 0: `None` when the file is absent or unreadable.
fn vm_zero(ctx: &Ctx, id: &str, title: &str, name: &str, why: &str, missing: Level) -> Hint {
    let path = ctx.roots.proc(&format!("sys/vm/{name}"));
    let Some(value) = read_trim(&path) else {
        return hint(
            id,
            missing,
            title,
            format!("vm.{name} is not readable on this host"),
            "",
        );
    };
    match value.parse::<u64>() {
        Ok(0) => hint(id, Level::Ok, title, format!("vm.{name} is 0"), ""),
        Ok(v) => hint(
            id,
            Level::Info,
            title,
            format!("vm.{name} is {v}, the target is 0; {why}"),
            &format!("Set vm.{name} to 0 (sysctl -w vm.{name}=0)."),
        )
        .with_commands(vec![format!("sysctl -w vm.{name}=0")]),
        Err(_) => hint(
            id,
            missing,
            title,
            format!("vm.{name} holds {value:?}, not a number"),
            "",
        ),
    }
}

fn swappiness(ctx: &Ctx) -> Hint {
    vm_zero(
        ctx,
        "swappiness",
        "vm.swappiness",
        "swappiness",
        "swapping out a worker's memory causes latency spikes",
        Level::Ok,
    )
}

fn zone_reclaim_mode(ctx: &Ctx) -> Hint {
    // a host without NUMA has no such file, which is fine
    vm_zero(
        ctx,
        "zone_reclaim_mode",
        "vm.zone_reclaim_mode",
        "zone_reclaim_mode",
        "reclaiming local memory instead of using another node stalls allocations",
        Level::Ok,
    )
}

// ---- kernel command line (hint-only) -------------------------------------------

fn rcu_nocb_poll(ctx: &Ctx) -> Hint {
    let title = "RCU callback polling";
    if ctx
        .roots
        .cmdline()
        .split_whitespace()
        .any(|w| w == "rcu_nocb_poll")
    {
        hint(
            "rcu_nocb_poll",
            Level::Ok,
            title,
            "rcu_nocb_poll is set".into(),
            "",
        )
    } else {
        hint(
            "rcu_nocb_poll",
            Level::Info,
            title,
            "rcu_nocb_poll is not on the kernel command line".into(),
            "Add rcu_nocb_poll to the kernel command line so the RCU offload threads poll instead of waking the isolated cores.",
        )
    }
}

fn boot_hugepages(ctx: &Ctx) -> Hint {
    let title = "Boot-time 1 GiB hugepages";
    let cmdline = ctx.roots.cmdline();
    let has = |w: &str| {
        cmdline
            .split_whitespace()
            .any(|x| x.eq_ignore_ascii_case(w))
    };
    let count = cmdline
        .split_whitespace()
        .any(|x| x.strip_prefix("hugepages=").is_some_and(|n| !n.is_empty()));
    if has("default_hugepagesz=1G") && has("hugepagesz=1G") && count {
        hint(
            "boot_hugepages",
            Level::Ok,
            title,
            "1 GiB hugepages are reserved on the kernel command line".into(),
            "",
        )
    } else {
        hint(
            "boot_hugepages",
            Level::Info,
            title,
            "no complete boot-time 1 GiB hugepage reservation (default_hugepagesz=1G, hugepagesz=1G and hugepages=<n>) is on the kernel command line".into(),
            "Reserve hugepages at boot with default_hugepagesz=1G hugepagesz=1G hugepages=<n>; it is the only reliable way to get contiguous 1 GiB pages.",
        )
    }
}

// ---- IOMMU, MTU --------------------------------------------------------

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
            (m < 1500).then(|| format!("{} has {m}", nic_label(i)))
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
