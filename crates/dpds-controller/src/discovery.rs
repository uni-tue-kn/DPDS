//! Network interface discovery from sysfs (B5): PCI class 0x02 devices.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::Serialize;
use utoipa::ToSchema;

use crate::host::{read_trim, HostRoots};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PcieLink {
    pub speed: Option<String>,
    pub width: Option<u32>,
    pub max_speed: Option<String>,
    pub max_width: Option<u32>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Interface {
    pub pci: String,
    pub vendor_id: Option<String>,
    pub device_id: Option<String>,
    pub vendor: Option<String>,
    pub driver: Option<String>,
    pub netdev: Option<String>,
    pub mac: Option<String>,
    pub operstate: Option<String>,
    pub speed_mbps: Option<u32>,
    pub mtu: Option<u32>,
    pub numa_node: Option<u32>,
    pub pcie_link: Option<PcieLink>,
    /// The netdev, or a bond/VLAN/bridge above it, carries an IPv4 or IPv6 route
    /// (`/proc/net/route`, `/proc/net/ipv6_route`): the management interface guard.
    pub kernel_route: bool,
    pub dpds_usable: bool,
    pub reason: Option<String>,
}

fn vendor_name(id: &str) -> String {
    match id {
        "0x15b3" => "Mellanox (NVIDIA)",
        "0x8086" => "Intel",
        "0x14e4" => "Broadcom",
        "0x10ec" => "Realtek",
        "0x1af4" => "Red Hat (virtio)",
        "0x1077" => "QLogic",
        "0x1137" => "Cisco",
        "0x1924" => "Solarflare",
        "0x1d6a" => "Aquantia",
        "0x177d" => "Cavium",
        _ => return id.to_string(),
    }
    .to_string()
}

/// Interfaces that carry an IPv4 route (`/proc/net/route`) or an IPv6 route
/// (`/proc/net/ipv6_route`). IPv6 loopback and link-local routes (`fe80::/10`,
/// the multicast route `ff00::/8`, anything on `lo`) are on every interface and
/// do not count.
fn routed_netdevs(roots: &HostRoots) -> HashSet<String> {
    let mut routed = HashSet::new();
    if let Some(table) = read_trim(&roots.proc("net/route")) {
        routed.extend(
            table
                .lines()
                .skip(1)
                .filter_map(|l| l.split_whitespace().next().map(str::to_string)),
        );
    }
    if let Some(table) = read_trim(&roots.proc("net/ipv6_route")) {
        for l in table.lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            let (Some(dest), Some(iface)) = (f.first(), f.last()) else {
                continue;
            };
            let dest = dest.to_ascii_lowercase();
            let link_local = ["fe8", "fe9", "fea", "feb"]
                .iter()
                .any(|p| dest.starts_with(p));
            if f.len() >= 10 && *iface != "lo" && !link_local && !dest.starts_with("ff") {
                routed.insert((*iface).to_string());
            }
        }
    }
    routed
}

/// The device that carries the route of `netdev`: itself, or a bond, VLAN or
/// bridge above it (sysfs `upper_*` and `master`, followed transitively).
fn route_holder(roots: &HostRoots, netdev: &str, routed: &HashSet<String>) -> Option<String> {
    let mut seen = vec![netdev.to_string()];
    let mut i = 0;
    while i < seen.len() {
        let dev = seen[i].clone();
        if routed.contains(&dev) {
            return Some(dev);
        }
        let dir = roots.sys(&format!("class/net/{dev}"));
        let mut uppers: Vec<String> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .strip_prefix("upper_")
                    .map(str::to_string)
            })
            .collect();
        if let Some(m) = fs::read_link(dir.join("master"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        {
            uppers.push(m);
        }
        for u in uppers {
            if !seen.contains(&u) {
                seen.push(u);
            }
        }
        i += 1;
    }
    None
}

fn read_u32(path: &Path) -> Option<u32> {
    read_trim(path)?.parse().ok()
}

fn first_dir_entry(path: &Path) -> Option<String> {
    let mut names: Vec<String> = fs::read_dir(path)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.into_iter().next()
}

fn route_reason(netdev: &str, holder: &str) -> String {
    if netdev == holder {
        format!("{netdev} carries a kernel route (management interface); DPDK would take it away from the host")
    } else {
        format!("{holder} above {netdev} carries a kernel route (management interface); DPDK would take {netdev} away from the host")
    }
}

/// All PCI network devices, sorted by PCI address.
pub fn discover(roots: &HostRoots) -> Vec<Interface> {
    let Ok(entries) = fs::read_dir(roots.sys("bus/pci/devices")) else {
        return Vec::new();
    };
    let routed = routed_netdevs(roots);
    let mut out = Vec::new();
    for e in entries.flatten() {
        let pci = e.file_name().to_string_lossy().into_owned();
        let dir = roots.pci_dir(&pci);
        if !read_trim(&dir.join("class")).is_some_and(|c| c.starts_with("0x02")) {
            continue;
        }
        let vendor_id = read_trim(&dir.join("vendor"));
        let driver = fs::read_link(dir.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let netdev = first_dir_entry(&dir.join("net"));
        let net = |file: &str| {
            netdev
                .as_ref()
                .and_then(|n| read_trim(&roots.sys(&format!("class/net/{n}/{file}"))))
        };
        let pcie = PcieLink {
            speed: read_trim(&dir.join("current_link_speed")),
            width: read_u32(&dir.join("current_link_width")),
            max_speed: read_trim(&dir.join("max_link_speed")),
            max_width: read_u32(&dir.join("max_link_width")),
        };
        let has_pcie = pcie.speed.is_some()
            || pcie.width.is_some()
            || pcie.max_speed.is_some()
            || pcie.max_width.is_some();
        let holder = netdev
            .as_deref()
            .and_then(|n| route_holder(roots, n, &routed).map(|h| (n.to_string(), h)));
        let (dpds_usable, reason) = match driver.as_deref() {
            _ if holder.is_some() => (false, holder.as_ref().map(|(n, h)| route_reason(n, h))),
            Some("mlx5_core") => (true, None),
            Some(d @ ("vfio-pci" | "uio_pci_generic")) => (true, Some(format!("bound to {d}"))),
            Some(d) => (
                false,
                Some(format!(
                    "bound to {d}; bind to vfio-pci to use it with DPDK"
                )),
            ),
            None => (
                false,
                Some("no driver bound; bind to vfio-pci to use it with DPDK".to_string()),
            ),
        };
        out.push(Interface {
            vendor: vendor_id.as_deref().map(vendor_name),
            vendor_id,
            device_id: read_trim(&dir.join("device")),
            driver,
            kernel_route: holder.is_some(),
            mac: net("address"),
            operstate: net("operstate"),
            speed_mbps: net("speed")
                .and_then(|s| s.parse::<i64>().ok())
                .and_then(|s| u32::try_from(s).ok())
                .filter(|s| *s > 0),
            mtu: net("mtu").and_then(|s| s.parse().ok()),
            numa_node: roots.pci_numa_node(&pci),
            pcie_link: has_pcie.then_some(pcie),
            netdev,
            dpds_usable,
            reason,
            pci,
        });
    }
    out.sort_by(|a, b| a.pci.cmp(&b.pci));
    out
}

/// The discovered interface of a DPDK device name, if it is a PCI address of a
/// network device.
pub fn find<'a>(interfaces: &'a [Interface], port: &str) -> Option<&'a Interface> {
    interfaces.iter().find(|i| i.pci.eq_ignore_ascii_case(port))
}
