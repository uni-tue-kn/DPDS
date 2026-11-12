//! Port-name validation: nothing from a free-form string reaches the `dpds` command line.

use crate::host::Network;

/// A port of a link in the given network mode.
///
/// Physical: a DPDK device name, a PCI address (returned in lower case) or a vdev that takes no
/// arguments, `net_null<N>` or `net_ring<N>`. `net_pcap` and any `,key=value` devargs are
/// refused: they make EAL create files or open interfaces on behalf of the (root) controller.
///
/// Virtual: a kernel interface name, 1 to 15 bytes of `[A-Za-z0-9_.-]`, case kept.
pub fn port_name(name: &str, network: Network) -> Result<String, String> {
    match network {
        Network::Physical => {
            if is_pci(name) {
                return Ok(name.to_ascii_lowercase());
            }
            if is_vdev(name) {
                return Ok(name.to_string());
            }
            Err(format!(
                "port {name:?} is neither a PCI address (0000:65:00.0) nor a vdev (net_null0, net_ring0; no devargs, no net_pcap), as this installation runs in the physical network mode"
            ))
        }
        Network::Virtual => {
            if is_netdev_name(name) {
                return Ok(name.to_string());
            }
            Err(format!(
                "port {name:?} is not a network interface name (1 to 15 characters of A-Z a-z 0-9 _ . -, not lo, net_*, a PCI address, . or ..), as this installation runs in the virtual network mode"
            ))
        }
    }
}

/// A kernel interface name DPDK may open: not `lo`, not a DPDK device name, not a PCI address.
fn is_netdev_name(s: &str) -> bool {
    (1..=15).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        && !matches!(s, "lo" | "." | "..")
        && !s.starts_with("net_")
        && !is_pci(s)
}

/// `dddd:bb:dd.f` with hex digits and a function of 0 to 7.
pub fn is_pci(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 12
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b':',
            10 => *c == b'.',
            11 => (b'0'..=b'7').contains(c),
            _ => c.is_ascii_hexdigit(),
        })
}

/// `net_(null|ring)<n>`, nothing else.
fn is_vdev(s: &str) -> bool {
    ["null", "ring"].iter().any(|k| {
        s.strip_prefix("net_")
            .and_then(|r| r.strip_prefix(k))
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()))
    })
}
