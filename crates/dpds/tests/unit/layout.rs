//! Core layout choice, port-count validation and the constants from C `config.h`.
//!
//! `Layout::from_lcore_count(lcores)` where `lcores` counts the main lcore:
//! - 2 => `Combined`       (1 worker lcore: RX+Worker+TX on it, 0 rings)
//! - 3 => `RxAndWorkerTx`  (2 worker lcores: RX | Worker+TX, 1 ring)
//! - 4 => `Separate`       (3 worker lcores: RX | Worker | TX, 2 rings)
//! - anything else (0, 1, 5, ...) => `Err(LayoutError::LcoreCount(n))`.
//!
//! `validate_port_count(n)`: Ok for 1 or 2, else `Err(LayoutError::PortCount(n))`.

use dpds::consts::*;
use dpds::layout::{validate_port_count, Layout, LayoutError};

#[test]
fn two_three_four_lcores_map_to_the_three_layouts() {
    assert_eq!(Layout::from_lcore_count(2).unwrap(), Layout::Combined);
    assert_eq!(Layout::from_lcore_count(3).unwrap(), Layout::RxAndWorkerTx);
    assert_eq!(Layout::from_lcore_count(4).unwrap(), Layout::Separate);
}

#[test]
fn worker_lcore_counts_are_lcores_minus_main() {
    assert_eq!(Layout::Combined.worker_lcores(), 1);
    assert_eq!(Layout::RxAndWorkerTx.worker_lcores(), 2);
    assert_eq!(Layout::Separate.worker_lcores(), 3);
}

#[test]
fn ring_counts_follow_the_layout() {
    assert_eq!(Layout::Combined.rings(), 0);
    assert_eq!(Layout::RxAndWorkerTx.rings(), 1);
    assert_eq!(Layout::Separate.rings(), 2);
}

#[test]
fn unsupported_lcore_counts_are_errors() {
    for n in [0usize, 1, 5, 6, 64] {
        let err = Layout::from_lcore_count(n).unwrap_err();
        assert!(
            matches!(err, LayoutError::LcoreCount(c) if c == n),
            "{n}: {err:?}"
        );
        assert!(err.to_string().contains(&n.to_string()), "{n}: {err}");
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
            matches!(err, LayoutError::PortCount(c) if c == n),
            "{n}: {err:?}"
        );
    }
}

#[test]
fn layout_error_is_a_std_error() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<LayoutError>();
}

#[test]
fn constants_match_the_c_config() {
    assert_eq!(MAX_BURST_SIZE, 16);
    assert_eq!(BURST_HIST_BUCKETS, 5);
    assert_eq!(
        BURST_HIST_BUCKETS,
        MAX_BURST_SIZE.trailing_zeros() as usize + 1
    );
    assert_eq!(NIC_DESCRIPTORS, 65535);
    assert_eq!(MBUF_CACHE_SIZE, 512);
    assert_eq!(MTU, 1500);
    assert_eq!(DEFAULT_NUM_MBUFS, 1024 * 1024 - 1);
}

#[test]
fn ring_size_is_a_power_of_two_as_required_by_dpdk_26_07() {
    assert_eq!(RING_SIZE, 65536);
    assert!(RING_SIZE.is_power_of_two());
}
