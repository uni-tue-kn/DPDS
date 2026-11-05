//! The limiter's limit is derived, not configured (spec20): for both modes it is
//! `buffer_capacity * MAX_FRAME_WIRE_BYTES * 8` bit with `MAX_FRAME_WIRE_BYTES = 1542`
//! (1518 B maximum Ethernet frame + 4 FEC bytes + 20 B L1 overhead, as the limiter counts a
//! packet): exactly `buffer_capacity` maximum frames fit. It is
//! the Spacer's fill-state limit and the Token Bucket Filter's bucket depth;
//! the TBF still starts full.
//!
//! Wire size of a pushed packet of `len` bytes: `(len + 4 + 20) * 8` bit, so
//! `len = 1518` is exactly `1542 * 8 = 12336` bit (one "slot" of the limit) and
//! `len = 1519` is 8 bit more. All tests use `rate_bps = 1e9` (1 bit/ns) at a
//! single instant `t = 0` unless noted: nothing drains or refills.
//!
//! To observe the limiter alone, the buffer is popped after every push, so the
//! buffer itself never holds more than one packet and `BufferFull` cannot occur.
//!
//! The constant itself is not imported (its path is not part of the spec);
//! these tests pin its value through the exact boundaries.

use std::ffi::c_void;

use dpds_core::buffer::{Buffer, PushOutcome};
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

/// `len` of a packet that is exactly one slot (12336 bit) on the wire.
const SLOT_LEN: u32 = 1518;
const SLOT_BITS: f64 = 12_336.0;

const MODES: [LimiterMode; 2] = [LimiterMode::Spacer, LimiterMode::TokenBucketFilter];

fn config(capacity: usize, mode: LimiterMode, rate_bps: f64, reordering: bool) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering,
        delayer: DelayerConfig {
            distribution: RVDistribution::DeterministicDistribution,
            mean_ns: 0.0,
            std_ns: 0.0,
            activate_correlation: false,
            half_life_period_ns: 1.0,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode,
            activate: true,
            rate_bps,
        },
        loss_model: LossModel::Random(0.0),
        seed: Some(1),
    }
}

/// Reordering works only with the token bucket (spec 45): the Spacer runs in FIFO mode only.
fn allowed(mode: LimiterMode, reordering: bool) -> bool {
    !(reordering && matches!(mode, LimiterMode::Spacer))
}

fn pkt(i: usize) -> *mut c_void {
    i as *mut c_void
}

fn is_accepted(o: PushOutcome) -> bool {
    matches!(o, PushOutcome::Accepted { .. })
}

/// How many `len`-byte packets pushed at t = 0 (each popped at once) the limiter
/// admits before its first `LimiterDrop`.
fn admitted_at_once(capacity: usize, mode: LimiterMode, reordering: bool, len: u32) -> usize {
    let mut b = Buffer::new(config(capacity, mode, 1e9, reordering));
    let mut n = 0;
    for i in 0..(capacity * 20 + 20) {
        match b.push_outcome(pkt(i + 1), len, 0.0) {
            PushOutcome::Accepted { .. } => {
                n += 1;
                assert!(b.pop().is_some());
            }
            PushOutcome::LimiterDrop => return n,
            other => panic!("only the limiter may reject here, got {other:?}"),
        }
    }
    panic!("the limiter never rejected");
}

#[test]
fn the_limit_is_capacity_times_1542_bytes_for_both_modes() {
    for mode in MODES {
        for reordering in [false, true] {
            if !allowed(mode, reordering) {
                continue;
            }
            for capacity in [1usize, 2, 3, 10, 64] {
                assert_eq!(
                    admitted_at_once(capacity, mode, reordering, SLOT_LEN),
                    capacity,
                    "{mode:?} reordering={reordering} capacity={capacity}: \
                     exactly `capacity` slots of 12336 bit fit"
                );
            }
        }
    }
}

#[test]
fn exactly_capacity_maximum_frames_fit_and_the_next_one_finds_the_buffer_full() {
    // The limit is what the buffer can hold: `capacity` frames of the maximum
    // size (1518 B + 4 FEC + 20 L1 = 1542 B = one slot) all pass the limiter
    // without anything being popped, and the next one is refused by the buffer
    // (checked first), not by the limiter.
    for mode in MODES {
        for reordering in [false, true] {
            if !allowed(mode, reordering) {
                continue;
            }
            for capacity in [1usize, 2, 5, 16] {
                let mut b = Buffer::new(config(capacity, mode, 1e9, reordering));
                for i in 0..capacity {
                    assert!(
                        is_accepted(b.push_outcome(pkt(i + 1), SLOT_LEN, 0.0)),
                        "{mode:?} reordering={reordering} capacity {capacity} frame {i}"
                    );
                }
                assert_eq!(b.len(), capacity);
                assert_eq!(
                    b.push_outcome(pkt(capacity + 1), SLOT_LEN, 0.0),
                    PushOutcome::BufferFull,
                    "{mode:?} capacity {capacity}"
                );
            }
        }
    }
}

#[test]
fn a_capacity_one_buffer_accepts_a_maximum_frame() {
    for mode in MODES {
        let mut b = Buffer::new(config(1, mode, 1e9, false));
        assert_eq!(
            b.push_outcome(pkt(1), 1518, 0.0),
            PushOutcome::Accepted { send_time_ns: 0 }
        );
    }
}

#[test]
fn the_limit_scales_with_the_capacity_in_bits() {
    // 992-bit packets (len 100): floor(capacity * 12336 / 992) of them fit
    // (Spacer: fill + packet <= limit; TBF: bucket >= packet), for both modes.
    for mode in MODES {
        for (capacity, expected) in [(1usize, 12), (2, 24), (3, 37), (4, 49), (10, 124)] {
            assert_eq!(
                admitted_at_once(capacity, mode, false, 100),
                expected,
                "{mode:?} capacity {capacity}: {} bit / 992 bit",
                capacity as f64 * SLOT_BITS
            );
        }
    }
}

#[test]
fn a_single_packet_is_judged_against_the_whole_derived_limit() {
    // capacity c: the largest admitted packet is len + 24 = 1542 * c bytes
    // (c * 12336 bit), one byte more is rejected, in both modes.
    for mode in MODES {
        for reordering in [false, true] {
            if !allowed(mode, reordering) {
                continue;
            }
            for capacity in [1usize, 2, 3, 10] {
                let largest = (1542 * capacity - 24) as u32;
                let mut b = Buffer::new(config(capacity, mode, 1e9, reordering));
                assert!(
                    is_accepted(b.push_outcome(pkt(1), largest, 0.0)),
                    "{mode:?} capacity {capacity} len {largest} fits exactly"
                );
                let mut b = Buffer::new(config(capacity, mode, 1e9, reordering));
                assert_eq!(
                    b.push_outcome(pkt(1), largest + 1, 0.0),
                    PushOutcome::LimiterDrop,
                    "{mode:?} capacity {capacity} len {} is 8 bit over",
                    largest + 1
                );
                assert!(b.is_empty());
            }
        }
    }
}

#[test]
fn the_limit_does_not_depend_on_the_rate() {
    for mode in MODES {
        for rate in [1e3, 1e6, 1e9, 100e9] {
            let mut b = Buffer::new(config(2, mode, rate, false));
            assert!(
                is_accepted(b.push_outcome(pkt(1), 3060, 0.0)),
                "{mode:?} {rate}"
            );
            let mut b = Buffer::new(config(2, mode, rate, false));
            assert_eq!(
                b.push_outcome(pkt(1), 3061, 0.0),
                PushOutcome::LimiterDrop,
                "{mode:?} {rate}"
            );
        }
    }
}

#[test]
fn the_limit_does_not_depend_on_the_current_buffer_content() {
    // Derived from the capacity, not from the free room: with other packets
    // queued the Spacer still admits what the fill state allows.
    for mode in MODES {
        let mut b = Buffer::new(config(4, mode, 1e9, false)); // limit 49344
                                                              // two queued packets (24672 bit) stay in the buffer ...
        assert!(is_accepted(b.push_outcome(pkt(1), SLOT_LEN, 0.0)));
        assert!(is_accepted(b.push_outcome(pkt(2), SLOT_LEN, 0.0)));
        assert_eq!(b.len(), 2);
        // ... and two more slots still fit, the fifth does not
        assert!(is_accepted(b.push_outcome(pkt(3), SLOT_LEN, 0.0)));
        assert!(is_accepted(b.push_outcome(pkt(4), SLOT_LEN, 0.0)));
        assert_eq!(b.len(), 4);
        b.pop();
        assert_eq!(
            b.push_outcome(pkt(5), SLOT_LEN, 0.0),
            PushOutcome::LimiterDrop,
            "{mode:?}: fill/bucket is spent although the buffer has room"
        );
    }
}

/// FIFO only: reordering does not work with the Spacer (spec 45).
#[test]
fn spacer_offsets_grow_by_one_slot_up_to_the_derived_limit() {
    let mut b = Buffer::new(config(3, LimiterMode::Spacer, 1e9, false));
    for k in 0..3u64 {
        assert_eq!(
            b.push_outcome(pkt(k as usize + 1), SLOT_LEN, 0.0),
            PushOutcome::Accepted {
                send_time_ns: k * 12_336
            }
        );
        b.pop();
    }
    assert_eq!(
        b.push_outcome(pkt(4), SLOT_LEN, 0.0),
        PushOutcome::LimiterDrop
    );
    // after one slot of time (12336 ns) exactly one slot is free again
    assert!(is_accepted(b.push_outcome(pkt(5), SLOT_LEN, 12_336.0)));
    assert_eq!(
        b.push_outcome(pkt(6), SLOT_LEN, 12_336.0),
        PushOutcome::LimiterDrop
    );
}

#[test]
fn the_token_bucket_starts_full_at_the_derived_depth_and_refills_up_to_it() {
    for reordering in [false, true] {
        let mut b = Buffer::new(config(2, LimiterMode::TokenBucketFilter, 1e9, reordering));
        // starts full: two slots at t = 0 (and no send-time offset)
        for i in 0..2 {
            assert_eq!(
                b.push_outcome(pkt(i + 1), SLOT_LEN, 0.0),
                PushOutcome::Accepted { send_time_ns: 0 }
            );
            b.pop();
        }
        assert_eq!(
            b.push_outcome(pkt(3), SLOT_LEN, 0.0),
            PushOutcome::LimiterDrop
        );
        // a long idle time refills only up to the depth (two slots), not more
        let later = 1e12;
        for i in 0..2 {
            assert!(is_accepted(b.push_outcome(pkt(10 + i), SLOT_LEN, later)));
            b.pop();
        }
        assert_eq!(
            b.push_outcome(pkt(20), SLOT_LEN, later),
            PushOutcome::LimiterDrop,
            "reordering={reordering}"
        );
    }
}

#[test]
fn a_burst_larger_than_the_derived_depth_is_dropped_by_the_token_bucket() {
    // Capacity 3: depth 37008 bit = three slots. A burst of five slots at one
    // instant: three pass, the rest is dropped (not delayed).
    let mut b = Buffer::new(config(3, LimiterMode::TokenBucketFilter, 1e9, false));
    let outcomes: Vec<PushOutcome> = (0..5)
        .map(|i| {
            let o = b.push_outcome(pkt(i + 1), SLOT_LEN, 0.0);
            b.pop();
            o
        })
        .collect();
    assert_eq!(
        outcomes,
        vec![
            PushOutcome::Accepted { send_time_ns: 0 },
            PushOutcome::Accepted { send_time_ns: 0 },
            PushOutcome::Accepted { send_time_ns: 0 },
            PushOutcome::LimiterDrop,
            PushOutcome::LimiterDrop,
        ]
    );
}

#[test]
fn the_limit_is_computed_without_overflow_for_large_capacities() {
    // 400_000 * 12336 = 4.87e9 bit does not fit a u32 (and 1e6, a capacity real
    // configs use, gives 1.2e10): a u32 computation panics in a debug build (as
    // the tests are) and must not wrap.
    for mode in MODES {
        for capacity in [400_000usize, 1_000_000] {
            let mut b = Buffer::new(config(capacity, mode, 1e9, false));
            for i in 0..100 {
                assert!(
                    is_accepted(b.push_outcome(pkt(i + 1), 9_000, 0.0)),
                    "{mode:?} capacity {capacity} packet {i}"
                );
            }
        }
    }
}

#[test]
fn an_inactive_limiter_ignores_the_derived_limit() {
    for mode in MODES {
        let mut cfg = config(1, mode, 1e9, false);
        cfg.limiter.activate = false;
        let mut b = Buffer::new(cfg);
        assert!(is_accepted(b.push_outcome(pkt(1), 60_000, 0.0)));
    }
}
