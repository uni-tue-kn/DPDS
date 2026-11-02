//! DPDS data plane (library part): the per-direction RX -> DPDS -> TX
//! loop, its CLI, the telemetry machinery (counters, epochs, first-loss,
//! replies) and the setup validation of the `dpds` binary. The loop is
//! generic over small traits ([`io`]) so it can be tested without an EAL;
//! `main.rs` wires it to the real NICs and mempools.

pub mod cli;
pub mod consts;
pub mod counters;
pub mod first_loss;
pub mod forward;
pub mod io;
pub mod quit;
pub mod reply;
pub mod setup;
pub mod stats;
pub mod worker;
