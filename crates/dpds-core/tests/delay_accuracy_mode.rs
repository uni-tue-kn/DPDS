//! Spec 45 B1: `Buffer::try_new` rejects `reordering` combined with correlation and `reordering`
//! combined with an active Spacer limiter, with an error whose message names the combination;
//! every other combination builds. An inactive limiter never counts, whatever its mode.
//!
//! The messages are the ones of the spec; the `BuildError` variant names are not fixed, so the
//! tests look at the Display text only.

use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

const CORRELATION_MSG: &str = "reordering cannot be combined with correlation";
const SPACER_MSG: &str = "reordering works only with the TokenBucketFilter limiter, not Spacer";

/// `limiter`: `None` is an inactive limiter in the Spacer mode (what the DTO builds without a stage).
fn config(
    reordering: bool,
    correlation: bool,
    limiter: Option<LimiterMode>,
    inactive_mode: LimiterMode,
) -> Config {
    Config {
        buffer_capacity: 16,
        reordering,
        delayer: DelayerConfig {
            distribution: RVDistribution::NormalDistribution,
            mean_ns: 1000.0,
            std_ns: 100.0,
            activate_correlation: correlation,
            half_life_period_ns: 1000.0,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode: limiter.unwrap_or(inactive_mode),
            activate: limiter.is_some(),
            rate_bps: 1e9,
        },
        loss_model: LossModel::Random(0.0),
        seed: Some(1),
    }
}

fn err_of(c: Config) -> String {
    match Buffer::try_new(c) {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn reordering_with_correlation_is_rejected_naming_the_combination() {
    for limiter in [None, Some(LimiterMode::TokenBucketFilter)] {
        let e = err_of(config(true, true, limiter, LimiterMode::Spacer));
        assert!(e.contains(CORRELATION_MSG), "{limiter:?}: {e}");
    }
}

#[test]
fn reordering_with_an_active_spacer_is_rejected_naming_the_combination() {
    let e = err_of(config(
        true,
        false,
        Some(LimiterMode::Spacer),
        LimiterMode::Spacer,
    ));
    assert!(e.contains(SPACER_MSG), "{e}");
}

#[test]
fn reordering_with_correlation_and_an_active_spacer_is_rejected() {
    let e = err_of(config(
        true,
        true,
        Some(LimiterMode::Spacer),
        LimiterMode::Spacer,
    ));
    assert!(e.contains(CORRELATION_MSG) || e.contains(SPACER_MSG), "{e}");
}

#[test]
fn reordering_with_an_inactive_limiter_in_either_mode_is_accepted() {
    for mode in [LimiterMode::Spacer, LimiterMode::TokenBucketFilter] {
        assert!(
            Buffer::try_new(config(true, false, None, mode)).is_ok(),
            "{mode:?}"
        );
    }
}

#[test]
fn reordering_with_an_active_token_bucket_is_accepted() {
    assert!(Buffer::try_new(config(
        true,
        false,
        Some(LimiterMode::TokenBucketFilter),
        LimiterMode::Spacer
    ))
    .is_ok());
}

#[test]
fn without_reordering_every_other_combination_is_accepted() {
    for correlation in [false, true] {
        for limiter in [
            None,
            Some(LimiterMode::Spacer),
            Some(LimiterMode::TokenBucketFilter),
        ] {
            for inactive in [LimiterMode::Spacer, LimiterMode::TokenBucketFilter] {
                assert!(
                    Buffer::try_new(config(false, correlation, limiter, inactive)).is_ok(),
                    "correlation {correlation}, limiter {limiter:?}, inactive mode {inactive:?}"
                );
            }
        }
    }
}

#[test]
fn the_check_does_not_depend_on_the_capacity() {
    // the controller validates with a capacity-zero buffer
    let mut c = config(true, true, None, LimiterMode::Spacer);
    c.buffer_capacity = 0;
    assert!(err_of(c).contains(CORRELATION_MSG));
    let mut c = config(true, false, Some(LimiterMode::Spacer), LimiterMode::Spacer);
    c.buffer_capacity = 0;
    assert!(err_of(c).contains(SPACER_MSG));
}
