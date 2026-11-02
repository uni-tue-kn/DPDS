//! Mapping of lcore count to pipeline layout, and port-count validation.

use std::fmt;

/// How the three stages (RX, Worker, TX) are spread over worker lcores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// RX, Worker and TX in one loop on one lcore; no rings.
    Combined,
    /// RX on one lcore, Worker and TX on another; one ring.
    RxAndWorkerTx,
    /// RX, Worker and TX on their own lcores; two rings.
    Separate,
}

/// Why the lcore or port count is unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    /// Lcore count, including the main lcore.
    LcoreCount(usize),
    PortCount(usize),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutError::LcoreCount(n) => write!(
                f,
                "this application needs 2 to 4 lcores (1 main + 1 to 3 workers), got {n}"
            ),
            LayoutError::PortCount(n) => write!(
                f,
                "this application requires one or two enabled ports, got {n}; \
                 pass one or two PCI addresses via -a or --vdev"
            ),
        }
    }
}

impl std::error::Error for LayoutError {}

impl Layout {
    /// Layout for `lcores` enabled lcores, the main lcore included (it only
    /// prints the dashboard).
    pub fn from_lcore_count(lcores: usize) -> Result<Layout, LayoutError> {
        match lcores {
            2 => Ok(Layout::Combined),
            3 => Ok(Layout::RxAndWorkerTx),
            4 => Ok(Layout::Separate),
            n => Err(LayoutError::LcoreCount(n)),
        }
    }

    /// Number of worker lcores (all lcores except main) the layout uses.
    pub fn worker_lcores(self) -> usize {
        match self {
            Layout::Combined => 1,
            Layout::RxAndWorkerTx => 2,
            Layout::Separate => 3,
        }
    }

    /// Number of inter-stage rings the layout needs.
    pub fn rings(self) -> usize {
        self.worker_lcores() - 1
    }
}

/// One port (loopback through a single NIC) or two ports (separate RX/TX NICs).
pub fn validate_port_count(ports: usize) -> Result<(), LayoutError> {
    match ports {
        1 | 2 => Ok(()),
        n => Err(LayoutError::PortCount(n)),
    }
}
