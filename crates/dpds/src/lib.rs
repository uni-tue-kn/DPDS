//! DPDS data plane (library part): the per-direction RX -> DPDS -> TX loop, the
//! CLI, the `--links` file ([`links`]), telemetry (counters, first-loss, delay
//! statistics, replies) and setup validation. The loop is generic over small
//! traits ([`io`]) so it runs without an EAL; `main.rs` wires it to the NICs.

pub mod cli;
pub mod consts;
pub mod counters;
pub mod delay_stats;
pub mod first_loss;
pub mod forward;
pub mod io;
pub mod links;
pub mod quit;
pub mod reply;
pub mod setup;
pub mod stats;
pub mod worker;
