//! Spec 30: `Container::add_uint` must not ignore DPDK's array limit.
//!
//! One DPDK telemetry array holds at most `RTE_TEL_MAX_ARRAY_ENTRIES` = 512 entries;
//! `rte_tel_data_add_array_uint` returns `-ENOSPC` from entry 513 on and drops the value. The
//! old `add_uint` discarded that return code, so a reply of more than 512 bins was cut silently
//! (the paged histogram makes sure it never asks for more than 500, and this test makes sure a
//! mistake there cannot go unnoticed).
//!
//! The spec allows either way of not ignoring it: propagate the error (return a `Result`) or
//! assert/log (panic). The test therefore accepts both: an entry is REJECTED when the call
//! panics or when its return value (formatted with `{:?}`) is an `Err(..)`; entries 1..=512 must
//! be ACCEPTED (neither), entry 513 and every later one REJECTED. A plain `()`-returning
//! `add_uint` that only logs cannot be distinguished from the old behaviour and fails the test.
//!
//! No EAL is needed (see the `telemetry::tests` module of the crate: array containers work
//! without `rte_eal_init`).

use std::panic::{catch_unwind, AssertUnwindSafe};

use dpdk::telemetry::Container;

/// DPDK's `RTE_TEL_MAX_ARRAY_ENTRIES`.
const LIMIT: usize = 512;

/// Whether adding entry `i` was rejected (panic or `Err`).
fn rejected(c: &mut Container, i: usize) -> bool {
    match catch_unwind(AssertUnwindSafe(|| format!("{:?}", c.add_uint(i as u64)))) {
        Ok(text) => text.starts_with("Err"),
        Err(_) => true,
    }
}

#[test]
fn the_premise_dpdk_refuses_entry_513_with_enospc() {
    // control: the raw call, so a failing test below is about `add_uint`, not about DPDK
    unsafe {
        let raw = dpdk_sys::rte_tel_data_alloc();
        assert!(!raw.is_null());
        dpdk_sys::rte_tel_data_start_array(raw, dpdk_sys::rte_tel_value_type_RTE_TEL_UINT_VAL);
        for i in 0..LIMIT {
            assert_eq!(
                dpdk_sys::rte_tel_data_add_array_uint(raw, i as u64),
                0,
                "entry {}",
                i + 1
            );
        }
        assert_eq!(
            dpdk_sys::rte_tel_data_add_array_uint(raw, 0),
            -28,
            "entry 513 is -ENOSPC"
        );
        dpdk_sys::rte_tel_data_free(raw);
    }
}

#[test]
fn add_uint_accepts_512_entries() {
    let mut c = Container::new_uint_array();
    for i in 0..LIMIT {
        assert!(
            !rejected(&mut c, i),
            "entry {} of {LIMIT} must be accepted",
            i + 1
        );
    }
}

#[test]
fn add_uint_does_not_ignore_enospc_beyond_512_entries() {
    let mut c = Container::new_uint_array();
    for i in 0..LIMIT {
        assert!(!rejected(&mut c, i), "entry {} must be accepted", i + 1);
    }
    for i in LIMIT..LIMIT + 3 {
        assert!(
            rejected(&mut c, i),
            "entry {} does not fit DPDK's 512-entry array: add_uint must propagate -ENOSPC or \
             assert, not drop the value silently",
            i + 1
        );
    }
}
