//! dpds-controller: stores DPDS links, supervises the `dpds` child process and
//! serves its telemetry over a REST API. Read-only towards the host: it only
//! reads sysfs/procfs for interface discovery and tuning hints.

pub mod api;
pub mod cdf;
pub mod discovery;
pub mod dto;
pub mod eal;
pub mod hints;
pub mod host;
pub mod lcores;
pub mod model;
pub mod stats;
pub mod store;
pub mod supervisor;
pub mod telemetry;
pub mod validate;
