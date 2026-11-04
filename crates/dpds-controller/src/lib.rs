//! dpds-controller: supervises the `dpds` child process and
//! serves its telemetry over a REST API. It reads sysfs/procfs for interface discovery and
//! tuning hints, which suggest commands and never change the host.

pub mod api;
pub mod cdf;
pub mod discovery;
pub mod dto;
pub mod eal;
pub mod frontend;
pub mod hints;
pub mod host;
pub mod lcores;
pub mod model;
pub mod nic_hints;
pub mod stats;
pub mod supervisor;
pub mod telemetry;
pub mod validate;
