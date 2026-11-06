//! Startup validation: lcore and port counts and the mempool size of a link.

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
                let links = required.saturating_sub(1);
                let (noun, verb) = if links == 1 {
                    ("link", "needs")
                } else {
                    ("links", "need")
                };
                write!(
                    f,
                    "{links} {noun} {verb} {required} lcores (main + one worker per link), \
                     e.g. -l 0-{links}; got {found}"
                )
            }
            SetupError::PortCount(n) => {
                write!(f, "DPDS needs 1 or 2 ports, found {n}")
            }
        }
    }
}

impl std::error::Error for SetupError {}

/// Ok only for `1 + num_links` lcores: main plus one worker per link.
pub fn validate_lcore_count(lcores: usize, num_links: usize) -> Result<(), SetupError> {
    let required = 1 + num_links;
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

/// Mbufs of one link's pool (unrounded): the delay buffer, the RX ring it fills
/// and the TX ring it feeds (applied descriptor counts), 1.5 mempool caches of
/// slack and two bursts in flight.
pub fn required_mbufs(buffer_capacity: usize, rx_desc: u16, tx_desc: u16) -> u64 {
    (buffer_capacity as u64)
        .saturating_add(rx_desc as u64)
        .saturating_add(tx_desc as u64)
        .saturating_add((MBUF_CACHE_SIZE * 3 / 2) as u64)
        .saturating_add(2 * BURST_SIZE as u64)
}
