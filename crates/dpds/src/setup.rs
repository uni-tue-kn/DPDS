//! Startup validation: exactly one worker lcore (2 lcores with main) and one
//! or two ports.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupError {
    /// Lcore count, including the main lcore.
    LcoreCount(usize),
    PortCount(usize),
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetupError::LcoreCount(n) => write!(
                f,
                "DPDS runs on exactly 2 lcores (the main lcore and one worker, e.g. -l 0-1); \
                 got {n}"
            ),
            SetupError::PortCount(n) => {
                write!(f, "DPDS needs 1 or 2 ports, found {n}")
            }
        }
    }
}

impl std::error::Error for SetupError {}

/// Ok only for 2 lcores: the main lcore plus exactly one data-plane worker.
pub fn validate_lcore_count(lcores: usize) -> Result<(), SetupError> {
    if lcores == 2 {
        Ok(())
    } else {
        Err(SetupError::LcoreCount(lcores))
    }
}

/// Ok for one port (loopback) or two ports.
pub fn validate_port_count(ports: usize) -> Result<(), SetupError> {
    if matches!(ports, 1 | 2) {
        Ok(())
    } else {
        Err(SetupError::PortCount(ports))
    }
}
