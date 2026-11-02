//! Startup validation and the constants from C `config.h`.
//!
//! `validate_lcore_count(lcores)` where `lcores` counts the main lcore: Ok
//! only for exactly 2 (main + one data-plane worker, like preof's
//! `baseline`). 0, 1, 3, 4, ... => `Err(SetupError::LcoreCount(n))` whose
//! Display names the count.
//!
//! `validate_port_count(n)`: Ok for 1 or 2, else `Err(SetupError::PortCount(n))`.

use dpds::consts::*;
use dpds::setup::{validate_lcore_count, validate_port_count, SetupError};

#[test]
fn exactly_two_lcores_are_valid() {
    assert!(validate_lcore_count(2).is_ok());
}

#[test]
fn every_other_lcore_count_is_an_error_naming_the_count() {
    for n in [0usize, 1, 3, 4, 5, 6, 64] {
        let err = validate_lcore_count(n).unwrap_err();
        assert!(
            matches!(err, SetupError::LcoreCount(c) if c == n),
            "{n}: {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains(&n.to_string()), "{n}: {msg}");
        assert!(msg.contains('2'), "the message says 2 are needed: {msg}");
    }
}

#[test]
fn one_or_two_ports_are_valid() {
    assert!(validate_port_count(1).is_ok());
    assert!(validate_port_count(2).is_ok());
}

#[test]
fn zero_or_three_plus_ports_are_errors() {
    for n in [0usize, 3, 4, 16] {
        let err = validate_port_count(n).unwrap_err();
        assert!(
            matches!(err, SetupError::PortCount(c) if c == n),
            "{n}: {err:?}"
        );
        assert!(err.to_string().contains(&n.to_string()), "{n}: {err}");
    }
}

#[test]
fn setup_error_is_a_std_error() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<SetupError>();
}

#[test]
fn constants_match_the_c_config() {
    assert_eq!(MAX_BURST_SIZE, 16);
    assert_eq!(NIC_DESCRIPTORS, 65535);
    assert_eq!(MBUF_CACHE_SIZE, 512);
    assert_eq!(MTU, 1500);
    assert_eq!(DEFAULT_NUM_MBUFS, 1024 * 1024 - 1);
}

#[test]
fn hardware_counters_are_polled_every_250_ms_like_preof() {
    assert_eq!(HW_STATS_POLL_INTERVAL_MS, 250);
}
