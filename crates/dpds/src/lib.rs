//! DPDS data plane (library part): the stages, counters, CLI and layout
//! logic of the `dpds` binary. The stages are generic over small traits
//! ([`io`]) so they can be tested without an EAL; `main.rs` wires them to
//! the real NICs, mempool and rings.

pub mod cli;
pub mod combined;
pub mod consts;
pub mod io;
pub mod layout;
pub mod quit;
pub mod rx;
pub mod stats;
pub mod tx;
pub mod worker;
