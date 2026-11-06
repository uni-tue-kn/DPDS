//! `Clock::new()` without an initialised EAL must panic with a clear message
//! instead of computing `1e9 / 0 = inf` (and caching it in the shared statics).
//!
//! Own test binary: no test here calls `Eal::new`, so `rte_get_tsc_hz()` stays 0
//! (only `rte_eal_init` sets it).

use std::panic::catch_unwind;

use dpdk::clock::Clock;

fn panic_message(e: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        String::new()
    }
}

#[test]
fn clock_new_before_the_eal_panics_with_a_clear_message() {
    let err = catch_unwind(Clock::new).expect_err("Clock::new() before the EAL must panic");
    let msg = panic_message(err);
    assert!(
        msg.contains("Clock::new() called before the EAL is initialised"),
        "unexpected panic message: {msg:?}"
    );
}

#[test]
fn a_failed_clock_new_does_not_poison_the_shared_caches() {
    // The first call must not cache a 0 Hz / inf scale that the second returns.
    for attempt in 1..=2 {
        let err = catch_unwind(Clock::new)
            .expect_err(&format!("attempt {attempt}: Clock::new() must panic again"));
        assert!(
            panic_message(err).contains("before the EAL is initialised"),
            "attempt {attempt}"
        );
    }
}

#[test]
fn default_clock_panics_like_new() {
    assert!(catch_unwind(Clock::default).is_err());
}
