//! Startup validation: lcore and port counts, the egress port of a
//! direction and the mempool size of a link.

use std::fmt;

use crate::consts::{BURST_SIZE, MBUF_CACHE_SIZE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupError {
    /// `found` lcores (the main lcore included) where `required` are needed.
    LcoreCount {
        found: usize,
        required: usize,
    },
    PortCount(usize),
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            SetupError::LcoreCount { found, required } => {
                let ports = required.saturating_sub(1);
                let (noun, verb) = if ports == 1 {
                    ("port", "needs")
                } else {
                    ("ports", "need")
                };
                write!(
                    f,
                    "{ports} {noun} {verb} {required} lcores, e.g. -l 0-{} (the main lcore and \
                     one worker per link); got {found}",
                    required.saturating_sub(1)
                )
            }
            SetupError::PortCount(n) => {
                write!(f, "DPDS needs 1 or 2 ports, found {n}")
            }
        }
    }
}

impl std::error::Error for SetupError {}

/// Ok only for `1 + num_links` lcores: the main lcore plus one worker per
/// link. The message counts the links as "ports" for the `--config*` shorthand
/// (one link per port).
pub fn validate_lcore_count(lcores: usize, num_ports: usize) -> Result<(), SetupError> {
    let required = 1 + num_ports;
    if lcores == required {
        Ok(())
    } else {
        Err(SetupError::LcoreCount {
            found: lcores,
            required,
        })
    }
}

/// Ok for one port (a hairpin link) or two ports (the `--config*` shorthand).
pub fn validate_port_count(ports: usize) -> Result<(), SetupError> {
    if matches!(ports, 1 | 2) {
        Ok(())
    } else {
        Err(SetupError::PortCount(ports))
    }
}

/// The port direction `direction` (traffic received on that port) leaves
/// through: the other port, or port 0 with a single port.
pub fn egress_port(direction: u16, num_ports: usize) -> u16 {
    if num_ports == 1 {
        0
    } else {
        direction ^ 1
    }
}

/// Mbufs of one link's pool, derived from its config and NIC rings (no
/// rounding). It must hold: the delay buffer, the RX ring it
/// fills and the TX ring it feeds (applied descriptor counts), 1.5 mempool
/// caches of slack and two bursts in flight.
pub fn required_mbufs(buffer_capacity: usize, rx_desc: u16, tx_desc: u16) -> u64 {
    (buffer_capacity as u64)
        .saturating_add(rx_desc as u64)
        .saturating_add(tx_desc as u64)
        .saturating_add((MBUF_CACHE_SIZE * 3 / 2) as u64)
        .saturating_add(2 * BURST_SIZE as u64)
}
