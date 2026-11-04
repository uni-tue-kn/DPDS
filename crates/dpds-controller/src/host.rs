//! Access to the host files the controller reads (sysfs and procfs), rooted at
//! configurable directories so tests can point at fake trees.

use std::ffi::CStr;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

/// Where the IP addresses of the host's interfaces come from.
#[derive(Debug, Clone)]
pub enum IfAddrs {
    /// `getifaddrs(3)`.
    System,
    /// A fixed `(netdev, address)` list in `getifaddrs` order (tests).
    Fixed(Vec<(String, IpAddr)>),
}

impl IfAddrs {
    /// `(netdev, address)` pairs that make a device a host uplink: every IPv4
    /// address and every IPv6 address except link-local (`fe80::/10`); loopback
    /// devices never count. IPv4 addresses come first, each family in the order
    /// the source returned them.
    pub fn uplink_addresses(&self) -> Vec<(String, IpAddr)> {
        let all = match self {
            IfAddrs::System => system_addresses(),
            IfAddrs::Fixed(list) => list.clone(),
        };
        let mut out: Vec<(String, IpAddr)> = all
            .into_iter()
            .filter(|(dev, addr)| {
                dev != "lo"
                    && match addr {
                        IpAddr::V4(_) => true,
                        IpAddr::V6(a) => (a.segments()[0] & 0xffc0) != 0xfe80,
                    }
            })
            .collect();
        out.sort_by_key(|(_, a)| a.is_ipv6());
        out
    }
}

/// The IPv4 and IPv6 addresses `getifaddrs` lists, without loopback devices.
fn system_addresses() -> Vec<(String, IpAddr)> {
    let mut out = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` fills `list` with a linked list that stays valid until
    // `freeifaddrs` below. Every pointer read from it is checked for null, the
    // address family decides which sockaddr struct the pointer really is, and
    // nothing borrowed from the list outlives the free.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return out;
        }
        let mut cur = list;
        while !cur.is_null() {
            let e = &*cur;
            cur = e.ifa_next;
            if e.ifa_addr.is_null()
                || e.ifa_name.is_null()
                || e.ifa_flags & (libc::IFF_LOOPBACK as u32) != 0
            {
                continue;
            }
            let addr = match i32::from((*e.ifa_addr).sa_family) {
                libc::AF_INET => {
                    let sa = &*(e.ifa_addr as *const libc::sockaddr_in);
                    IpAddr::V4(Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr)))
                }
                libc::AF_INET6 => {
                    let sa = &*(e.ifa_addr as *const libc::sockaddr_in6);
                    IpAddr::V6(Ipv6Addr::from(sa.sin6_addr.s6_addr))
                }
                _ => continue,
            };
            out.push((
                CStr::from_ptr(e.ifa_name).to_string_lossy().into_owned(),
                addr,
            ));
        }
        libc::freeifaddrs(list);
    }
    out
}

#[derive(Debug, Clone)]
pub struct HostRoots {
    pub sysfs: PathBuf,
    pub procfs: PathBuf,
    pub ifaddrs: IfAddrs,
}

impl HostRoots {
    pub fn system() -> HostRoots {
        HostRoots {
            sysfs: PathBuf::from("/sys"),
            procfs: PathBuf::from("/proc"),
            ifaddrs: IfAddrs::System,
        }
    }

    pub fn sys(&self, rel: &str) -> PathBuf {
        self.sysfs.join(rel)
    }

    pub fn proc(&self, rel: &str) -> PathBuf {
        self.procfs.join(rel)
    }

    pub fn pci_dir(&self, pci: &str) -> PathBuf {
        self.sys(&format!("bus/pci/devices/{pci}"))
    }

    /// `/proc/cmdline` (empty when unreadable).
    pub fn cmdline(&self) -> String {
        read_trim(&self.proc("cmdline")).unwrap_or_default()
    }

    /// Value of `key=` on the kernel command line (last occurrence).
    pub fn cmdline_value(&self, key: &str) -> Option<String> {
        let prefix = format!("{key}=");
        self.cmdline()
            .split_whitespace()
            .filter_map(|w| w.strip_prefix(&prefix).map(str::to_string))
            .next_back()
    }

    /// Online CPUs (falls back to `0..available_parallelism`).
    pub fn online_cpus(&self) -> Vec<u32> {
        match read_trim(&self.sys("devices/system/cpu/online")).map(|s| parse_cpulist(&s)) {
            Some(v) if !v.is_empty() => v,
            _ => {
                let n = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
                (0..n).collect()
            }
        }
    }

    /// Isolated CPUs: the sysfs file if present (empty file = none), else the
    /// `isolcpus=` kernel parameter.
    pub fn isolated_cpus(&self) -> Vec<u32> {
        match read_trim(&self.sys("devices/system/cpu/isolated")) {
            Some(s) => parse_cpulist(&s),
            None => self
                .cmdline_value("isolcpus")
                .map(|v| parse_cpulist(&v))
                .unwrap_or_default(),
        }
    }

    /// NUMA node of a CPU from `node<N>/cpulist`; `None` without NUMA information.
    pub fn node_of_cpu(&self, cpu: u32) -> Option<u32> {
        let nodes = fs::read_dir(self.sys("devices/system/node")).ok()?;
        for e in nodes.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(id) = name
                .strip_prefix("node")
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(list) = read_trim(&e.path().join("cpulist")) {
                if parse_cpulist(&list).contains(&cpu) {
                    return Some(id);
                }
            }
        }
        None
    }

    /// NUMA node of a PCI device (`None` for -1, absent or non-PCI names).
    pub fn pci_numa_node(&self, pci: &str) -> Option<u32> {
        read_trim(&self.pci_dir(pci).join("numa_node"))?
            .parse::<i64>()
            .ok()
            .and_then(|n| u32::try_from(n).ok())
    }
}

/// Reads a file and trims it; `None` if it cannot be read.
pub fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Parses a kernel CPU list like `2-3,6,8-9`. Non-numeric parts (such as the
/// `domain,managed_irq` flags of `isolcpus=`) are skipped.
pub fn parse_cpulist(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                    out.extend(a..=b);
                }
            }
            None => {
                if let Ok(n) = part.parse::<u32>() {
                    out.push(n);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether a DPDK device name is a virtual device (`net_null0`, `net_pcap1`, ...)
/// rather than a PCI address.
pub fn is_vdev(name: &str) -> bool {
    name.starts_with("net_")
}

/// Formats CPU ids as a comma separated list.
pub fn fmt_cpus(cpus: &[u32]) -> String {
    cpus.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpulist_parsing() {
        assert_eq!(parse_cpulist("2-3,6,8-9"), [2, 3, 6, 8, 9]);
        assert_eq!(parse_cpulist(""), Vec::<u32>::new());
        assert_eq!(parse_cpulist("domain,managed_irq,4-5"), [4, 5]);
        assert_eq!(parse_cpulist("3,3,1"), [1, 3]);
    }
}
