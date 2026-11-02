//! Exact (hand-computed) tests of `Limiter`, the Spacer and the token bucket.
//!
//! All values below are derived from the formulas in `limiter.rs`, not from
//! running the code:
//!
//! - wire size of a packet of `len` bytes = `(len + 20) * 8` bit (20 = preamble
//!   7 + SFD 1 + IPG 12; `Limiter` itself does NOT add the 4 FEC bytes, `Buffer`
//!   does), so `len = 100` is 960 bit and `len = 1500` is 12160 bit;
//! - `rate_bpns = rate_bps / 1e9`; `rate_bps = 1e9` is exactly 1 bit/ns, so
//!   every quantity in the 1 Gb/s sequences is an integer and compared exactly;
//! - Spacer: `fill = max(0, fill - dt * rate)`, reject if `fill + size > limit`,
//!   else `offset = fill / rate + max(0, last_update - t)` and `fill += size`;
//! - TBF: `fill = min(limit, fill + dt * rate)`, accept (offset 0) iff
//!   `fill >= size`, then `fill -= size`;
//! - `dt = max(0, t - last_update)` and `last_update = max(last_update, t)`.

use dpds_core::limiter::{Limiter, LimiterMode};

const PKT_100: u32 = 100; // 960 bit on the wire
const BITS_100: f64 = 960.0;

#[test]
fn constructor_sets_initial_state_for_both_modes() {
    let spacer = Limiter::new(LimiterMode::Spacer, true, 100e9, 5000.0);
    assert!(matches!(spacer.mode, LimiterMode::Spacer));
    assert!(spacer.activate);
    assert_eq!(spacer.rate_bpns, 100.0, "100 Gb/s is 100 bit/ns");
    assert_eq!(spacer.limit_bit, 5000.0);
    assert_eq!(spacer.fill_state_bit, 0.0, "the Spacer starts empty");
    assert_eq!(spacer.last_update_ns, 0.0);

    let tbf = Limiter::new(LimiterMode::TokenBucketFilter, false, 1e9, 5000.0);
    assert!(matches!(tbf.mode, LimiterMode::TokenBucketFilter));
    assert!(!tbf.activate);
    assert_eq!(tbf.rate_bpns, 1.0);
    assert_eq!(
        tbf.fill_state_bit, 5000.0,
        "the TBF starts with a full bucket"
    );
    assert_eq!(tbf.last_update_ns, 0.0);
}

#[test]
fn inactive_limiter_passes_everything_and_leaves_its_state_alone() {
    for mode in [LimiterMode::Spacer, LimiterMode::TokenBucketFilter] {
        // limit 0 and a huge packet would be rejected by an active limiter.
        let mut l = Limiter::new(mode, false, 1e9, 0.0);
        let before = (l.fill_state_bit, l.last_update_ns);
        for (t, len) in [(0.0, 1), (123.5, 9000), (1e12, u32::MAX / 16)] {
            assert_eq!(l.limiter_calculate_offset_ns(t, len), Some(0.0));
        }
        assert_eq!((l.fill_state_bit, l.last_update_ns), before);
    }
}

#[test]
fn spacer_offsets_accumulate_the_serialisation_time_of_earlier_packets() {
    let mut l = Limiter::new(LimiterMode::Spacer, true, 1e9, 1e6);
    // Three packets arriving at the same instant leave 0, 960 and 1920 ns later.
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(0.0));
    assert_eq!(l.fill_state_bit, BITS_100);
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(960.0));
    assert_eq!(l.fill_state_bit, 1920.0);
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(1920.0));
    assert_eq!(l.fill_state_bit, 2880.0);
}

#[test]
fn spacer_offset_is_the_remaining_backlog_and_idle_time_drains_it() {
    let mut l = Limiter::new(LimiterMode::Spacer, true, 1e9, 1e6);
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(0.0)); // fill 960
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(960.0)); // fill 1920

    // 500 ns later 500 bit have drained: backlog 1420 ns. The packet therefore
    // leaves at 500 + 1420 = 1920 ns, exactly when the previous one ends.
    assert_eq!(l.limiter_calculate_offset_ns(500.0, PKT_100), Some(1420.0));
    assert_eq!(l.fill_state_bit, 1420.0 + BITS_100);
    assert_eq!(l.last_update_ns, 500.0);

    // A gap of 9500 ns is longer than the whole backlog (2380 bit): empty again.
    assert_eq!(l.limiter_calculate_offset_ns(10_000.0, PKT_100), Some(0.0));
    assert_eq!(l.fill_state_bit, BITS_100);
    assert_eq!(l.last_update_ns, 10_000.0);
}

#[test]
fn spacer_with_a_time_earlier_than_the_last_update_adds_the_lag_to_the_offset() {
    // Delay jitter can hand the limiter a send time earlier than one it has seen.
    let mut l = Limiter::new(LimiterMode::Spacer, true, 1e9, 1e6);
    assert_eq!(l.limiter_calculate_offset_ns(10_000.0, PKT_100), Some(0.0));
    // t = 9000 < last_update = 10000: nothing drains (dt = 0), last_update stays,
    // and the offset is the backlog (960) plus the 1000 ns by which the packet
    // is ahead of last_update: it leaves at 9000 + 1960 = 10960 = 10000 + 960.
    assert_eq!(
        l.limiter_calculate_offset_ns(9_000.0, PKT_100),
        Some(1_960.0)
    );
    assert_eq!(l.last_update_ns, 10_000.0);
    assert_eq!(l.fill_state_bit, 1920.0);
}

#[test]
fn spacer_rejects_what_would_exceed_the_limit_and_keeps_its_fill_unchanged() {
    let mut l = Limiter::new(LimiterMode::Spacer, true, 1e9, 2000.0);
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(0.0)); // fill 960
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(960.0)); // fill 1920
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), None); // 1920 + 960 > 2000
    assert_eq!(l.fill_state_bit, 1920.0, "a rejected packet is not added");
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), None);

    // 960 ns later 960 bit have drained, so one packet fits again, offset 960.
    assert_eq!(l.limiter_calculate_offset_ns(960.0, PKT_100), Some(960.0));
    assert_eq!(l.fill_state_bit, 1920.0);
}

#[test]
fn spacer_limit_boundary_fill_plus_packet_equal_to_limit_is_accepted() {
    let mut l = Limiter::new(LimiterMode::Spacer, true, 1e9, 1920.0);
    assert!(l.limiter_calculate_offset_ns(0.0, PKT_100).is_some());
    assert!(l.limiter_calculate_offset_ns(0.0, PKT_100).is_some()); // 960 + 960 == 1920
    assert!(l.limiter_calculate_offset_ns(0.0, PKT_100).is_none());

    // One bit less than a single packet: even the first one is rejected.
    let mut tiny = Limiter::new(LimiterMode::Spacer, true, 1e9, 959.0);
    assert_eq!(tiny.limiter_calculate_offset_ns(0.0, PKT_100), None);
}

#[test]
fn spacer_offset_scales_inversely_with_the_rate() {
    // 10 Gb/s = 10 bit/ns: 960 bit take exactly 96 ns.
    let mut l = Limiter::new(LimiterMode::Spacer, true, 10e9, 1e6);
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(0.0));
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(96.0));
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(192.0));

    // 100 Gb/s with 1500-byte packets: (1500 + 20) * 8 = 12160 bit = 121.6 ns.
    let mut l = Limiter::new(LimiterMode::Spacer, true, 100e9, 1e9);
    for (i, expected) in [0.0, 121.6, 243.2, 364.8].into_iter().enumerate() {
        let got = l.limiter_calculate_offset_ns(0.0, 1500).unwrap();
        assert!(
            (got - expected).abs() < 1e-9,
            "packet {i}: {got} vs {expected}"
        );
    }
}

#[test]
fn spacer_never_goes_below_zero_fill() {
    let mut l = Limiter::new(LimiterMode::Spacer, true, 1e9, 1e6);
    // First call far in the future of last_update = 0 with an empty bucket.
    assert_eq!(l.limiter_calculate_offset_ns(1e9, PKT_100), Some(0.0));
    assert_eq!(l.fill_state_bit, BITS_100);
}

#[test]
fn tbf_budget_refill_and_reject_sequence() {
    // 1 bit/ns, bucket 3000 bit (starts full), 960 bit per packet.
    let mut l = Limiter::new(LimiterMode::TokenBucketFilter, true, 1e9, 3000.0);

    // Three packets at t = 0 spend 2880 of the 3000 initial bit.
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(0.0));
    assert_eq!(l.fill_state_bit, 2040.0);
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(0.0));
    assert_eq!(l.fill_state_bit, 1080.0);
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), Some(0.0));
    assert_eq!(l.fill_state_bit, 120.0);

    // 120 < 960: rejected, tokens untouched.
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), None);
    assert_eq!(l.fill_state_bit, 120.0);

    // 840 ns refill 840 bit: exactly 960 available -> accepted (>=), bucket 0.
    assert_eq!(l.limiter_calculate_offset_ns(840.0, PKT_100), Some(0.0));
    assert_eq!(l.fill_state_bit, 0.0);

    // 959 ns later: 959 < 960 rejected, but the refill is kept (last_update moves).
    assert_eq!(l.limiter_calculate_offset_ns(1799.0, PKT_100), None);
    assert_eq!(l.fill_state_bit, 959.0);
    assert_eq!(l.last_update_ns, 1799.0);
    // 1 ns later the missing bit has arrived.
    assert_eq!(l.limiter_calculate_offset_ns(1800.0, PKT_100), Some(0.0));
    assert_eq!(l.fill_state_bit, 0.0);
}

#[test]
fn tbf_refill_is_capped_at_the_bucket_size() {
    let mut l = Limiter::new(LimiterMode::TokenBucketFilter, true, 1e9, 3000.0);
    assert_eq!(l.limiter_calculate_offset_ns(1e9, PKT_100), Some(0.0));
    assert_eq!(
        l.fill_state_bit,
        3000.0 - BITS_100,
        "capped at 3000 before the packet"
    );
}

#[test]
fn tbf_with_a_bucket_smaller_than_a_packet_never_admits_it() {
    let mut l = Limiter::new(LimiterMode::TokenBucketFilter, true, 1e9, 959.0);
    assert_eq!(l.limiter_calculate_offset_ns(0.0, PKT_100), None);
    assert_eq!(
        l.limiter_calculate_offset_ns(1e9, PKT_100),
        None,
        "cap keeps it below 960"
    );
}

#[test]
fn tbf_does_not_refill_for_times_earlier_than_the_last_update() {
    let mut l = Limiter::new(LimiterMode::TokenBucketFilter, true, 1e9, 3000.0);
    assert_eq!(l.limiter_calculate_offset_ns(1000.0, PKT_100), Some(0.0)); // 3000 -> 2040
    assert_eq!(l.limiter_calculate_offset_ns(500.0, PKT_100), Some(0.0)); // dt = 0 -> 1080
    assert_eq!(l.fill_state_bit, 1080.0);
    assert_eq!(l.last_update_ns, 1000.0);
}

#[test]
fn tbf_never_adds_an_offset() {
    let mut l = Limiter::new(LimiterMode::TokenBucketFilter, true, 1e9, 1e9);
    for i in 0..100 {
        assert_eq!(
            l.limiter_calculate_offset_ns(i as f64 * 10.0, 1500),
            Some(0.0)
        );
    }
}
