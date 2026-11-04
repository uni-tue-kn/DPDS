//! Integration tests of `dpds-controller` (one test binary, one module per topic,
//! shared helpers in `common`; no lint suppressions anywhere).
//!
//! Everything runs without root and without a NIC: fake sysfs/procfs trees,
//! a fake supervisor and telemetry source for the REST tests, shell scripts
//! standing in for `dpds` for the supervisor tests. The `e2e` module's tests are
//! `#[ignore]`d (they need `target/release/dpds` built with `--features telemetry`):
//!
//! ```text
//! cargo test --release -p dpds-controller --test integration              # everything else
//! cargo build --release -p dpds --features telemetry
//! cargo test --release -p dpds-controller --test integration -- --ignored --test-threads=1
//! ```
//!
//! The frontend serving added in spec v13 is described in the header of [`frontend`].
//!
//! The test-facing Rust API, JSON shapes and HTTP contract that the tests assume
//! are written down in the header of [`common`].

mod api;
mod check_cli;
mod common;
mod delay_stats;
mod discovery;
mod drain_hint;
mod dto;
mod e2e;
mod eal;
mod fake_telemetry;
mod fixtures_sanity;
mod frontend;
mod hints;
mod histogram_bad_pages;
mod histogram_extras;
mod histogram_paging;
mod histogram_window;
mod host_fixes;
mod host_guard;
mod http_hardening;
mod input_validation;
mod launch_revalidation;
mod lcores;
mod management_guard;
mod optional_stages;
mod release;
mod start_body;
mod startup_errors;
mod stateless;
mod stats;
mod supervisor;
mod supervisor_fixes;
mod telemetry_freshness;
mod v12;
mod v7_lows;
