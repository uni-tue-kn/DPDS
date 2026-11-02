//! DPDS data plane (library part): the single-core RX -> emulator -> TX loop,
//! its CLI, the telemetry machinery (counters, epochs, first-loss, replies)
//! and the setup validation of the `dpds` binary. The stages are generic
//! over small traits ([`io`]) so they can be tested without an EAL;
//! `main.rs` wires them to the real NICs and mempool.

pub mod cli;
pub mod consts;
pub mod counters;
pub mod first_loss;
pub mod forward;
pub mod io;
pub mod quit;
pub mod reply;
pub mod rx;
pub mod setup;
pub mod stats;
pub mod tx;
pub mod worker;
