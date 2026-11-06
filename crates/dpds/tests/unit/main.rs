//! Unit tests for the `dpds` library crate (one worker lcore per direction). None of them needs
//! an EAL. They compile and pass with and without `--features telemetry`; tests that need real
//! counters are gated on `#[cfg(feature = "telemetry")]`.
//!
//! `unsafe fn` in the API = the caller promises valid, owned mbufs (tests call them in
//! `unsafe {}` with fake mbufs that mocks never dereference). Semantics are documented at the
//! top of each test module.

mod causes;
mod cli;
mod counters;
mod delay_stats;
mod first_loss;
mod forward;
mod hist_pages;
mod links;
mod quit;
mod readme;
mod setup;
mod spec51;
mod spec53;
mod stats;
mod support;
mod worker;
