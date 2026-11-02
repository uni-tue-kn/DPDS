//! Generic telemetry machinery, ported from preof's own tests
//! (`peof/src/counters.rs`' `tests`, `telemetry_tests` and
//! `nic_epoch_tracker_tests` modules) so the semantics match preof's:
//! `mean_2dp`, `Counter`/`MaxGauge` (incl. `observe_with`, `take`,
//! single-writer/concurrent-reader), `should_close_epoch`/`EpochDriver`
//! (boundary strictly > 500 ms idle, only after an arrival, contiguous
//! epochs), `EpochFirstLoss` (first call per epoch wins, `record` always
//! reaches the whole-process tracker, `take` resets), `EpochLog` (deltas,
//! capacity 32 oldest kept + `epochs_dropped`, gauge high-water survival
//! past capacity, first-loss slots, lock-free reader), `NicEpochTracker`.
//!
//! Without the `telemetry` feature only shape tests run (zero-sized no-op
//! types, constants, names); everything that needs real accumulation is
//! gated like preof's.
//!
//! Adaptations from preof: the DPDS counters struct is `DpdsCounters`
//! (names/order pinned below); loss events use DPDS reasons/stages.

mod shape {
    use dpds::counters::*;

    #[test]
    fn mean_2dp_zero_count_is_zero_point_zero() {
        assert_eq!(mean_2dp(0, 0), "0.00");
        assert_eq!(mean_2dp(5, 0), "0.00");
    }

    #[test]
    fn mean_2dp_seven_over_two_is_three_point_five_zero() {
        assert_eq!(mean_2dp(7, 2), "3.50");
    }

    #[test]
    fn mean_2dp_one_over_three_rounds_to_zero_point_three_three() {
        assert_eq!(mean_2dp(1, 3), "0.33");
    }

    #[test]
    fn mean_2dp_two_over_three_rounds_to_zero_point_six_seven() {
        // Pins the rounding direction: 2/3 = 0.6666... -> "0.67", not truncated.
        assert_eq!(mean_2dp(2, 3), "0.67");
    }

    #[test]
    fn mean_2dp_of_a_whole_batch_size_has_two_decimals() {
        assert_eq!(mean_2dp(32, 2), "16.00");
    }

    #[test]
    fn dpds_counters_is_64_byte_aligned() {
        assert_eq!(std::mem::align_of::<DpdsCounters>(), 64);
    }

    #[test]
    fn epoch_idle_gap_ns_is_five_hundred_milliseconds() {
        assert_eq!(EPOCH_IDLE_GAP_NS, 500_000_000.0);
    }

    #[test]
    fn epoch_capacity_is_thirty_two() {
        assert_eq!(EPOCH_CAPACITY, 32);
    }

    /// Exact order matters: `close()`'s counter array and
    /// `epochs_reply`'s names are matched up positionally.
    #[test]
    fn dpds_counters_epoch_counter_names_are_pinned_in_order() {
        assert_eq!(
            DpdsCounters::EPOCH_COUNTER_NAMES,
            [
                "polls",
                "rx_pkts",
                "rx_full_bursts",
                "dpds_rejects",
                "pops",
                "tx_calls",
                "tx_pkts",
                "tx_sent",
                "tx_short_calls",
                // the drop causes, appended
                "loss_drops",
                "limiter_drops",
                "buffer_full_drops",
            ]
        );
    }

    #[test]
    fn dpds_counters_epoch_gauge_names_are_pinned_in_order() {
        assert_eq!(
            DpdsCounters::EPOCH_GAUGE_NAMES,
            ["tx_batch_max", "buffer_len_max"]
        );
    }

    #[test]
    fn dpds_epoch_log_has_twelve_counters_and_two_gauges() {
        // Fails to compile if the alias width drifts from the names above.
        fn accepts(_log: EpochLog<12, 2>) {}
        accepts(DpdsEpochLog::default());
        assert_eq!(DpdsCounters::default().cumulative_snapshot().len(), 12);
    }

    #[test]
    fn nic_epoch_log_names_are_pinned() {
        assert_eq!(
            NicEpochLog::EPOCH_COUNTER_NAMES,
            ["rx_packets", "imissed", "rx_port_discards"]
        );
        assert!(NicEpochLog::EPOCH_GAUGE_NAMES.is_empty());
        fn accepts(_log: EpochLog<3, 0>) {}
        accepts(NicEpochLog::default());
    }

    /// `EpochBoundary`/`EpochRecord`/`NicEpochRecord` are real in both builds
    /// (call sites type-check either way).
    #[test]
    fn plain_data_types_derive_what_the_call_sites_need() {
        let b = EpochBoundary {
            start_uptime_ns: 1.0,
            end_uptime_ns: 2.0,
        };
        let b2 = b;
        assert_eq!(b, b2);
        let n = NicEpochRecord::default();
        let n2 = n;
        assert_eq!(n, n2);
        assert_eq!(format!("{n:?}"), format!("{n2:?}"));
        let r = EpochRecord::<1, 1> {
            start_uptime_ns: 0,
            end_uptime_ns: 1,
            counters: [2],
            gauges: [3],
        };
        assert_eq!(r, r);
    }
}

#[cfg(not(feature = "telemetry"))]
mod zero_cost {
    //! Without the feature everything is a zero-sized no-op (preof's
    //! "Everything is free when you don't ask for it").
    use dpds::counters::*;
    use dpds::first_loss::*;

    #[test]
    fn counter_gauge_log_driver_scratch_tracker_are_zero_sized() {
        assert_eq!(std::mem::size_of::<Counter>(), 0);
        assert_eq!(std::mem::size_of::<MaxGauge>(), 0);
        assert_eq!(std::mem::size_of::<EpochLog<3, 1>>(), 0);
        assert_eq!(std::mem::size_of::<DpdsEpochLog>(), 0);
        assert_eq!(std::mem::size_of::<EpochDriver>(), 0);
        assert_eq!(std::mem::size_of::<EpochFirstLoss>(), 0);
        assert_eq!(std::mem::size_of::<NicEpochTracker>(), 0);
        assert_eq!(std::mem::size_of::<StageLossTracker>(), 0);
        // the counters struct is all zero-sized fields (it keeps its 64-byte alignment)
        assert_eq!(std::mem::size_of::<DpdsCounters>(), 0);
    }

    #[test]
    fn writes_are_ignored_and_reads_are_zero() {
        let c = Counter;
        c.inc();
        c.add(5);
        assert_eq!(c.get(), 0);
        let g = MaxGauge;
        g.observe(9);
        assert_eq!(g.get(), 0);
        assert_eq!(g.take(), 0);
    }

    #[test]
    fn max_gauge_observe_with_never_invokes_the_closure() {
        use std::cell::Cell;
        let g = MaxGauge;
        let calls = Cell::new(0u32);
        g.observe_with(|| {
            calls.set(calls.get() + 1);
            42
        });
        assert_eq!(
            calls.get(),
            0,
            "the sampling work must disappear at the source level without the feature"
        );
    }

    #[test]
    fn should_close_epoch_and_epoch_driver_are_inert() {
        assert!(!should_close_epoch(1e12, 0.0, true));
        let mut d = EpochDriver::new(0.0);
        assert_eq!(d.tick(0.0, 1), None);
        assert_eq!(d.tick(1_000_000_000_000.0, 0), None);
    }

    #[test]
    fn epoch_log_never_records_anything() {
        let log: EpochLog<1, 0> = EpochLog;
        log.close(
            [10],
            [],
            0,
            100,
            Some(StageLossEvent {
                reason: FirstLossReason::DescriptorFull,
                detail: StageLossDetail::RingFull {
                    from: Stage::Dpds,
                    to: Stage::Nic,
                },
                relative_ns: 42.0,
            }),
        );
        assert_eq!(log.epochs_closed(), 0);
        assert_eq!(log.epochs_dropped(), 0);
        assert_eq!(log.record(0), None);
        assert_eq!(log.first_loss(0), None);
        assert_eq!(log.aggregate_gauge_max(0, 7), 7);
    }

    #[test]
    fn epoch_first_loss_and_tracker_are_no_ops() {
        let mut efl = EpochFirstLoss;
        let tracker = StageLossTracker;
        assert_eq!(efl.take(), None);
        efl.record(
            &tracker,
            FirstLossReason::DescriptorFull,
            StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic,
            },
            123.0,
        );
        assert_eq!(efl.take(), None);
        assert!(!tracker.try_record(
            FirstLossReason::DpdsReject,
            StageLossDetail::Single { from: Stage::Dpds },
            1.0
        ));
        assert_eq!(tracker.current(), None);
    }

    #[test]
    fn nic_epoch_tracker_never_closes_nor_reads_the_xstats() {
        let mut t = NicEpochTracker::new(0.0, 0, 0, 0);
        assert_eq!(
            t.poll(1.0, 100, 0, || panic!("must not read port discards")),
            None
        );
        assert_eq!(
            t.poll(10.0 * EPOCH_IDLE_GAP_NS, 100, 0, || panic!("must not read")),
            None
        );
    }
}

#[cfg(feature = "telemetry")]
mod telemetry_tests {
    use dpds::counters::*;
    use dpds::first_loss::{
        FirstLossReason, Stage, StageLossDetail, StageLossEvent, StageLossTracker,
    };

    // ---- extras specific to DpdsCounters -------------------------------

    #[test]
    fn cumulative_snapshot_reports_every_counter_at_its_named_index() {
        let c = DpdsCounters::default();
        c.polls.add(1);
        c.rx_pkts.add(2);
        c.rx_full_bursts.add(3);
        c.dpds_rejects.add(4);
        c.pops.add(5);
        c.tx_calls.add(6);
        c.tx_pkts.add(7);
        c.tx_sent.add(8);
        c.tx_short_calls.add(9);
        c.loss_drops.add(10);
        c.limiter_drops.add(11);
        c.buffer_full_drops.add(12);
        c.tx_batch_max.observe(100);
        c.buffer_len_max.observe(200);
        // the three drop causes are appended after the nine original counters
        assert_eq!(
            c.cumulative_snapshot(),
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
        );
        assert_eq!(DpdsCounters::EPOCH_COUNTER_NAMES.len(), 12);
        for (i, name) in DpdsCounters::EPOCH_COUNTER_NAMES.iter().enumerate() {
            let expected = match *name {
                "polls" => 1,
                "rx_pkts" => 2,
                "rx_full_bursts" => 3,
                "dpds_rejects" => 4,
                "pops" => 5,
                "tx_calls" => 6,
                "tx_pkts" => 7,
                "tx_sent" => 8,
                "tx_short_calls" => 9,
                "loss_drops" => 10,
                "limiter_drops" => 11,
                "buffer_full_drops" => 12,
                other => panic!("unexpected counter name {other}"),
            };
            assert_eq!(c.cumulative_snapshot()[i], expected, "{name}");
        }
    }

    #[test]
    fn cumulative_snapshot_does_not_reset_or_include_gauges() {
        let c = DpdsCounters::default();
        c.tx_batch_max.observe(16);
        assert_eq!(c.cumulative_snapshot(), [0; 12]);
        assert_eq!(c.tx_batch_max.get(), 16);
    }

    #[test]
    fn stage_loss_tracker_is_cache_line_aligned_and_the_log_too() {
        assert_eq!(std::mem::align_of::<StageLossTracker>(), 64);
        assert_eq!(std::mem::align_of::<DpdsEpochLog>(), 64);
    }

    // ---- ported from preof -------------------------------------------

    // ── should_close_epoch: pure boundary-detection decision ────────────
    //
    // Real behaviour only exists with the telemetry feature on -- without
    // it, `should_close_epoch` is a no-op that always returns `false` (see
    // its own `#[cfg]` split), so a test asserting a real `true` result
    // would fail outright, and the `false`-asserting ones below would only
    // pass by coincidence (the no-op returns `false` unconditionally,
    // proving nothing about the real boundary logic). Moved here,
    // unchanged, from `mod tests` for exactly that reason.

    #[test]
    fn should_close_epoch_true_when_gap_exceeds_threshold_and_epoch_saw_arrivals() {
        let now_ns = 10_000_000_000.0;
        let last_arrival_ns = now_ns - (EPOCH_IDLE_GAP_NS + 1.0);
        assert!(should_close_epoch(now_ns, last_arrival_ns, true));
    }

    #[test]
    fn should_close_epoch_false_when_gap_is_under_threshold() {
        let now_ns = 10_000_000_000.0;
        let last_arrival_ns = now_ns - (EPOCH_IDLE_GAP_NS - 1.0);
        assert!(!should_close_epoch(now_ns, last_arrival_ns, true));
    }

    #[test]
    fn should_close_epoch_false_at_exactly_the_threshold() {
        // Pins the boundary condition itself: a gap of EXACTLY
        // EPOCH_IDLE_GAP_NS must not close (the design says "strictly
        // greater than"), rather than leaving the >= vs > choice to
        // whatever the implementer happens to write.
        let now_ns = 10_000_000_000.0;
        let last_arrival_ns = now_ns - EPOCH_IDLE_GAP_NS;
        assert!(!should_close_epoch(now_ns, last_arrival_ns, true));
    }

    #[test]
    fn should_close_epoch_false_when_epoch_has_seen_no_arrivals_yet_even_with_huge_gap() {
        // A long pre-traffic startup idle (or two back-to-back silent
        // epochs) must never emit a run of empty epoch closes, no matter
        // how large the gap -- gated on saw_arrival_this_epoch, not on the
        // gap size alone.
        let now_ns = 10_000_000_000.0;
        let last_arrival_ns = 0.0;
        assert!(!should_close_epoch(now_ns, last_arrival_ns, false));
    }

    #[test]
    fn should_close_epoch_false_on_a_tick_that_itself_saw_arrivals() {
        // A tick that itself saw arrivals updates last_arrival_ns to
        // now_ns before (or instead of) calling this function, so the gap
        // collapses to 0 -- must never close regardless of
        // saw_arrival_this_epoch.
        let now_ns = 10_000_000_000.0;
        assert!(!should_close_epoch(now_ns, now_ns, true));
        assert!(!should_close_epoch(now_ns, now_ns, false));
    }

    // ── EpochDriver: the whole per-tick state machine ────────────────────
    //
    // Real behaviour only exists with the telemetry feature on -- without
    // it, `EpochDriver` is a genuinely fieldless no-op twin (see its own
    // `#[cfg]` split), so these tests -- which read `last_arrival_ns`/
    // `saw_arrival_this_epoch` directly -- don't just fail, they fail to
    // *compile* in that build. Moved here, unchanged, from `mod tests` for
    // exactly that reason. This is the seam that lets the stage-loop
    // wiring's own bugs (flag not reset after a close, last_arrival_ns not
    // updated, wrong next-epoch start) be caught here instead of only
    // being visible on hardware.

    #[test]
    fn epoch_driver_arrival_tick_updates_last_arrival_and_sets_flag() {
        let mut d = EpochDriver::new(0.0);
        let result = d.tick(1_000.0, 5);

        assert_eq!(
            result, None,
            "an arrival tick can never itself be a boundary"
        );
        assert_eq!(d.last_arrival_ns, 1_000.0);
        assert!(d.saw_arrival_this_epoch);
    }

    #[test]
    fn epoch_driver_sub_threshold_gap_returns_none() {
        let mut d = EpochDriver::new(0.0);
        d.tick(1_000.0, 5);

        let now_ns = 1_000.0 + (EPOCH_IDLE_GAP_NS - 1.0);
        let result = d.tick(now_ns, 0);

        assert_eq!(result, None);
        assert!(
            d.saw_arrival_this_epoch,
            "a sub-threshold idle tick must not reset the flag -- the epoch is still open"
        );
        assert_eq!(
            d.last_arrival_ns, 1_000.0,
            "an idle tick must never update last_arrival_ns, only an arrival tick does"
        );
    }

    #[test]
    fn epoch_driver_over_threshold_gap_after_arrivals_closes_and_resets_flag_so_subsequent_idle_ticks_return_none(
    ) {
        let mut d = EpochDriver::new(0.0);
        d.tick(0.0, 3);

        let close_now = EPOCH_IDLE_GAP_NS + 1.0;
        let boundary = d.tick(close_now, 0);

        assert_eq!(
            boundary,
            Some(EpochBoundary {
                start_uptime_ns: 0.0,
                end_uptime_ns: close_now
            })
        );
        assert!(
            !d.saw_arrival_this_epoch,
            "closing must reset saw_arrival_this_epoch -- otherwise every subsequent idle tick \
             would immediately close another (empty) epoch"
        );

        // The specific bug this type exists to prevent: several consecutive
        // idle ticks after the close, even ones with an even larger gap,
        // must all return None -- not a run of empty epoch closes.
        for i in 1..=5u32 {
            let t = close_now + (i as f64) * (EPOCH_IDLE_GAP_NS + 1.0);
            assert_eq!(
                d.tick(t, 0),
                None,
                "idle tick {i} after a close must not emit another empty epoch close"
            );
        }
    }

    #[test]
    fn epoch_driver_close_sets_next_epochs_start_to_the_boundary_time_so_epochs_are_contiguous() {
        let mut d = EpochDriver::new(0.0);
        d.tick(0.0, 3);

        let close1_now = EPOCH_IDLE_GAP_NS + 1.0;
        let b1 = d.tick(close1_now, 0).expect("must close");
        assert_eq!(b1.start_uptime_ns, 0.0);
        assert_eq!(b1.end_uptime_ns, close1_now);
        assert_eq!(
            d.epoch_start_uptime_ns, close1_now,
            "the next epoch must start exactly where the previous one ended"
        );

        // Arrivals resume, then close again -- the second boundary's start
        // must equal the first boundary's end exactly: no gap, no overlap.
        d.tick(close1_now + 10.0, 7);
        let close2_now = close1_now + 10.0 + EPOCH_IDLE_GAP_NS + 1.0;
        let b2 = d.tick(close2_now, 0).expect("must close again");
        assert_eq!(
            b2.start_uptime_ns, close1_now,
            "epoch 2 must start exactly at epoch 1's end"
        );
        assert_eq!(b2.end_uptime_ns, close2_now);
    }

    #[test]
    fn epoch_driver_arrivals_resuming_after_a_close_start_a_fresh_epoch_that_can_itself_close_later(
    ) {
        let mut d = EpochDriver::new(0.0);
        d.tick(0.0, 1);
        let close_now = EPOCH_IDLE_GAP_NS + 1.0;
        assert!(d.tick(close_now, 0).is_some());
        assert!(!d.saw_arrival_this_epoch);

        let resume_now = close_now + 1.0;
        d.tick(resume_now, 4);
        assert!(
            d.saw_arrival_this_epoch,
            "arrivals resuming after a close must set the flag again -- it must not be stuck \
             false forever"
        );
        assert_eq!(d.last_arrival_ns, resume_now);

        let close2_now = resume_now + EPOCH_IDLE_GAP_NS + 1.0;
        let boundary = d.tick(close2_now, 0);
        assert!(
            boundary.is_some(),
            "the fresh epoch must be able to close on its own later"
        );
    }

    #[test]
    fn epoch_driver_long_pretraffic_startup_idle_emits_nothing_at_all() {
        let mut d = EpochDriver::new(0.0);

        for i in 1..=5u32 {
            // Every gap here is a large multiple of EPOCH_IDLE_GAP_NS, but
            // no arrival has ever occurred -- must never emit a close.
            let t = (i as f64) * (EPOCH_IDLE_GAP_NS * 3.0);
            assert_eq!(
                d.tick(t, 0),
                None,
                "a pre-traffic startup idle tick must never emit a close, no matter how large \
                 the gap"
            );
        }
        assert!(!d.saw_arrival_this_epoch);
    }

    // ── Counter ──────────────────────────────────────────────────────────

    #[test]
    fn counter_fresh_is_zero() {
        let c = Counter::default();
        assert_eq!(c.get(), 0);
    }

    #[test]
    fn counter_inc_and_add_accumulate() {
        let c = Counter::default();
        c.inc();
        c.inc();
        c.add(5);
        assert_eq!(c.get(), 7, "2 inc()s + add(5) must total 7");
    }

    #[test]
    fn counter_add_zero_is_a_no_op() {
        let c = Counter::default();
        c.add(3);
        let before = c.get();
        c.add(0);
        assert_eq!(
            c.get(),
            before,
            "add(0) must not change the accumulated total"
        );
    }

    // ── MaxGauge ─────────────────────────────────────────────────────────

    #[test]
    fn max_gauge_fresh_is_zero() {
        let g = MaxGauge::default();
        assert_eq!(g.get(), 0);
    }

    #[test]
    fn max_gauge_observe_keeps_the_maximum() {
        let g = MaxGauge::default();
        g.observe(5);
        g.observe(9);
        g.observe(2);
        assert_eq!(g.get(), 9);
    }

    #[test]
    fn max_gauge_observe_never_decreases_on_a_smaller_value() {
        let g = MaxGauge::default();
        g.observe(100);
        g.observe(1);
        assert_eq!(
            g.get(),
            100,
            "a smaller observed value must never lower the high-water mark"
        );
    }

    // ── MaxGauge::observe_with ───────────────────────────────────────────
    //
    // With the telemetry feature on, `observe_with` must evaluate its
    // closure and behave identically to `observe` -- these pins exist
    // alongside (not instead of) the no-feature
    // `max_gauge_observe_with_never_invokes_closure_without_telemetry_feature`
    // pin in `mod tests`, which is the one that actually distinguishes the
    // lazy variant from a plain `observe(f())` call.

    #[test]
    fn max_gauge_observe_with_evaluates_closure_exactly_once_per_call() {
        use std::cell::Cell;

        let g = MaxGauge::default();
        let calls = Cell::new(0u32);
        g.observe_with(|| {
            calls.set(calls.get() + 1);
            5
        });
        assert_eq!(
            calls.get(),
            1,
            "observe_with must evaluate its closure exactly once per call"
        );

        g.observe_with(|| {
            calls.set(calls.get() + 1);
            3
        });
        assert_eq!(
            calls.get(),
            2,
            "a second call must evaluate its own closure exactly once"
        );
    }

    #[test]
    fn max_gauge_observe_with_keeps_the_maximum_and_never_decreases() {
        let g = MaxGauge::default();
        g.observe_with(|| 5);
        g.observe_with(|| 9);
        g.observe_with(|| 2);
        assert_eq!(
            g.get(),
            9,
            "observe_with must keep the running maximum, identically to observe"
        );
    }

    #[test]
    fn max_gauge_observe_with_and_observe_agree_on_resulting_value() {
        let via_observe = MaxGauge::default();
        let via_observe_with = MaxGauge::default();
        let samples = [5u64, 9, 2, 100, 1, 0, 50, 100, 3];

        for &v in &samples {
            via_observe.observe(v);
            via_observe_with.observe_with(|| v);
        }

        assert_eq!(
            via_observe_with.get(),
            via_observe.get(),
            "observe_with and observe must agree on the resulting high-water mark for the \
             same sequence of samples"
        );
    }

    // ── MaxGauge::take ───────────────────────────────────────────────────
    //
    // `take` is what lets the stage loop produce a genuinely per-epoch
    // gauge value (see this module's "MaxGauge semantics change" design
    // pin) -- these three tests are load-bearing together with
    // `epoch_log_gauge_aggregate_is_the_max_of_every_epoch_own_maximum_and_the_live_value`
    // below: that test proves the *aggregate* math is right given correct
    // per-epoch inputs, these prove `take` actually produces a correct,
    // freshly-reset per-epoch input in the first place -- without `take`
    // resetting for real, every "epoch's own maximum" would silently be
    // the whole-process maximum instead, and the two tests together are
    // what would catch that.

    #[test]
    fn max_gauge_take_on_a_fresh_gauge_returns_zero() {
        let g = MaxGauge::default();
        assert_eq!(g.take(), 0);
    }

    #[test]
    fn max_gauge_take_returns_the_current_max_and_resets_to_zero() {
        let g = MaxGauge::default();
        g.observe(5);
        g.observe(90);
        g.observe(3);

        assert_eq!(g.take(), 90, "take must return the maximum observed so far");
        assert_eq!(
            g.get(),
            0,
            "take must leave the gauge reset to 0, not still holding the pre-take maximum"
        );
    }

    #[test]
    fn max_gauge_observe_after_take_can_produce_a_new_maximum_lower_than_the_pre_take_one() {
        // Proves the reset is real, not cosmetic: if `take` merely read
        // the value without resetting, a subsequent smaller `observe`
        // could never become the new maximum (since `observe` never
        // decreases the mark).
        let g = MaxGauge::default();
        g.observe(90);
        assert_eq!(g.take(), 90);

        g.observe(7);
        assert_eq!(
            g.get(),
            7,
            "after take() resets the gauge to 0, observing 7 must make 7 the new maximum -- \
             impossible unless the reset was real"
        );
    }

    // ── Single-writer / concurrent-reader race safety ───────────────────
    //
    // Bounded by construction (no risk of hanging the suite on a broken
    // implementation): the writer thread runs a fixed loop of N ops then
    // sets a plain `AtomicBool` completion flag (not part of the design
    // under test -- just test-local synchronization); the reader thread
    // polls in a tight loop, recording every value it sees, and takes one
    // final sample after observing the flag before stopping.

    #[test]
    fn counter_single_writer_concurrent_reader_is_race_free_and_monotonic() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::thread;

        const N: u64 = 200_000;
        let counter: Arc<Counter> = Arc::new(Counter::default());
        let writer_done = Arc::new(AtomicBool::new(false));

        let writer_counter = Arc::clone(&counter);
        let writer_flag = Arc::clone(&writer_done);
        let writer = thread::spawn(move || {
            for i in 0..N {
                if i % 2 == 0 {
                    writer_counter.inc();
                } else {
                    writer_counter.add(1);
                }
            }
            writer_flag.store(true, Ordering::Relaxed);
        });

        let reader_counter = Arc::clone(&counter);
        let reader_flag = Arc::clone(&writer_done);
        let reader = thread::spawn(move || {
            let mut observed = Vec::new();
            loop {
                let done = reader_flag.load(Ordering::Relaxed);
                observed.push(reader_counter.get());
                if done {
                    // One more sample after the writer signalled
                    // completion, so the final value is included.
                    observed.push(reader_counter.get());
                    break;
                }
            }
            observed
        });

        writer.join().expect("writer thread panicked");
        let observed = reader.join().expect("reader thread panicked");

        assert_eq!(
            counter.get(),
            N,
            "single writer's N increments must all be visible with no lost updates"
        );
        assert!(
            observed.windows(2).all(|w| w[0] <= w[1]),
            "reader must never observe the counter's value go backwards: {observed:?}"
        );
    }

    #[test]
    fn max_gauge_single_writer_concurrent_reader_is_race_free_and_monotonic() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::thread;

        const N: u64 = 200_000;
        let gauge: Arc<MaxGauge> = Arc::new(MaxGauge::default());
        let writer_done = Arc::new(AtomicBool::new(false));

        let writer_gauge = Arc::clone(&gauge);
        let writer_flag = Arc::clone(&writer_done);
        let writer = thread::spawn(move || {
            for v in 0..N {
                writer_gauge.observe(v);
            }
            writer_flag.store(true, Ordering::Relaxed);
        });

        let reader_gauge = Arc::clone(&gauge);
        let reader_flag = Arc::clone(&writer_done);
        let reader = thread::spawn(move || {
            let mut observed = Vec::new();
            loop {
                let done = reader_flag.load(Ordering::Relaxed);
                observed.push(reader_gauge.get());
                if done {
                    observed.push(reader_gauge.get());
                    break;
                }
            }
            observed
        });

        writer.join().expect("writer thread panicked");
        let observed = reader.join().expect("reader thread panicked");

        assert_eq!(
            gauge.get(),
            N - 1,
            "high-water mark must land on the last (largest) observed value"
        );
        assert!(
            observed.windows(2).all(|w| w[0] <= w[1]),
            "reader must never observe the high-water mark go backwards: {observed:?}"
        );
    }

    // ── EpochLog ─────────────────────────────────────────────────────────
    //
    // Real-behaviour tests for the per-trial epoch bookkeeping introduced
    // alongside `Counter`/`MaxGauge` -- see the module doc comment above
    // `EPOCH_CAPACITY` for the full design. Without the `telemetry`
    // feature `EpochLog` is a zero-sized no-op (pinned structurally by
    // `mod tests`'s own `epoch_log_is_zero_sized_without_telemetry_feature`
    // -- there is no real accumulation to exercise in that build, same
    // reasoning as `Counter`/`MaxGauge` above).

    #[test]
    fn epoch_log_fresh_reports_no_closed_or_dropped_epochs_and_no_records() {
        let log: EpochLog<3, 1> = EpochLog::default();
        assert_eq!(log.epochs_closed(), 0);
        assert_eq!(log.epochs_dropped(), 0);
        assert_eq!(
            log.record(0),
            None,
            "a fresh log must have no record at index 0 -- nothing has been closed yet"
        );
    }

    #[test]
    fn epoch_log_close_records_a_delta_not_a_cumulative_total() {
        // Feeding cumulative snapshots 10, then 25, then 30 must record
        // per-epoch deltas 10, 15, 5 -- not the raw cumulative values
        // themselves (which would be 10, 25, 30).
        let log: EpochLog<1, 0> = EpochLog::default();

        log.close([10], [], 0, 100, None);
        log.close([25], [], 100, 200, None);
        log.close([30], [], 200, 300, None);

        assert_eq!(log.epochs_closed(), 3);
        assert_eq!(
            log.record(0).expect("epoch 0 must be recorded").counters,
            [10],
            "the first epoch has no prior cumulative snapshot, so its delta is the raw value \
             itself (a delta from an implicit zero baseline)"
        );
        assert_eq!(
            log.record(1).expect("epoch 1 must be recorded").counters,
            [15],
            "25 - 10 = 15, not the raw cumulative value 25"
        );
        assert_eq!(
            log.record(2).expect("epoch 2 must be recorded").counters,
            [5],
            "30 - 25 = 5, not the raw cumulative value 30"
        );
    }

    #[test]
    fn epoch_log_records_start_and_end_timestamps_monotonic_across_epochs() {
        let log: EpochLog<1, 0> = EpochLog::default();

        log.close([1], [], 1_000, 1_500, None);
        log.close([2], [], 1_500, 2_200, None);
        log.close([3], [], 2_200, 3_000, None);

        let r0 = log.record(0).unwrap();
        let r1 = log.record(1).unwrap();
        let r2 = log.record(2).unwrap();

        assert_eq!((r0.start_uptime_ns, r0.end_uptime_ns), (1_000, 1_500));
        assert_eq!((r1.start_uptime_ns, r1.end_uptime_ns), (1_500, 2_200));
        assert_eq!((r2.start_uptime_ns, r2.end_uptime_ns), (2_200, 3_000));

        // Monotonic: every epoch's own start < end, and one epoch's end
        // never exceeds the next epoch's start.
        for r in [r0, r1, r2] {
            assert!(r.start_uptime_ns < r.end_uptime_ns);
        }
        assert!(r0.end_uptime_ns <= r1.start_uptime_ns);
        assert!(r1.end_uptime_ns <= r2.start_uptime_ns);
    }

    #[test]
    fn epoch_log_overflow_keeps_the_oldest_capacity_epochs_and_counts_the_rest_dropped() {
        // Close EPOCH_CAPACITY + 3 epochs. The sweep's early trials are
        // the interesting ones, so the FIRST EPOCH_CAPACITY must be kept
        // intact (oldest kept, newest dropped), not overwritten by the
        // last 3.
        let log: EpochLog<1, 0> = EpochLog::default();
        let mut cumulative: u64 = 0;

        for i in 0..(EPOCH_CAPACITY + 3) {
            cumulative += 1; // each epoch's own delta is exactly 1
            let start = (i as u64) * 1_000;
            let end = start + 500;
            log.close([cumulative], [], start, end, None);
        }

        assert_eq!(
            log.epochs_closed(),
            EPOCH_CAPACITY as u32,
            "epochs_closed must saturate at EPOCH_CAPACITY, not grow past it"
        );
        assert_eq!(
            log.epochs_dropped(),
            3,
            "the 3 epoch-close attempts past EPOCH_CAPACITY must be counted as dropped"
        );

        // Every one of the first EPOCH_CAPACITY epochs' data must be
        // intact and in original (oldest-first) order.
        for i in 0..EPOCH_CAPACITY {
            let r = log
                .record(i)
                .unwrap_or_else(|| panic!("epoch {i} must still be recorded"));
            assert_eq!(
                r.counters,
                [1],
                "epoch {i}'s delta must be untouched by the overflow"
            );
            assert_eq!(
                r.start_uptime_ns,
                (i as u64) * 1_000,
                "epoch {i}'s start must be untouched"
            );
        }

        // The 3 epochs past the capacity must not have overwritten
        // anything -- there is no valid index EPOCH_CAPACITY..EPOCH_CAPACITY+3.
        assert_eq!(
            log.record(EPOCH_CAPACITY),
            None,
            "a dropped epoch must not be recorded at any index"
        );
    }

    #[test]
    fn epoch_log_gauge_aggregate_is_the_max_of_every_epoch_own_maximum_and_the_live_value() {
        // Each epoch's own gauge maximum is independent (MaxGauge-style
        // per-epoch reset -- see module doc's "MaxGauge semantics change"
        // pin), but the aggregate the existing whole-process wire field
        // depends on must still equal the max across all epochs' own
        // maxima plus the live in-progress value.
        let log: EpochLog<0, 1> = EpochLog::default();

        log.close([], [40], 0, 100, None); // epoch 0's own max: 40
        log.close([], [90], 100, 200, None); // epoch 1's own max: 90 (the overall max)
        log.close([], [10], 200, 300, None); // epoch 2's own max: 10

        assert_eq!(
            log.record(0).unwrap().gauges,
            [40],
            "each epoch keeps its OWN maximum, not a running max carried over from the \
             previous epoch"
        );
        assert_eq!(log.record(1).unwrap().gauges, [90]);
        assert_eq!(log.record(2).unwrap().gauges, [10]);

        // Aggregate: max over all closed epochs' maxima (40, 90, 10) and
        // a live value below all of them -- must equal 90.
        assert_eq!(log.aggregate_gauge_max(0, /* live_value = */ 5), 90);

        // Aggregate: a live value that's itself the largest must win.
        assert_eq!(log.aggregate_gauge_max(0, /* live_value = */ 500), 500);
    }

    // ── Overflow + gauges together ───────────────────────────────────────
    //
    // Defect 2: `MaxGauge::take()` unconditionally zeroes the gauge on
    // every epoch boundary, but `EpochLog::close`'s overflow branch (once
    // EPOCH_CAPACITY is reached) used to discard `epoch_gauges` outright
    // along with the counter deltas -- so a gauge peak that happened to
    // land in one of the epochs past EPOCH_CAPACITY vanished forever, and
    // `aggregate_gauge_max` could silently under-report the true
    // whole-process maximum once a sweep point ran more than
    // EPOCH_CAPACITY trials. The existing
    // `epoch_log_gauge_aggregate_is_the_max_of_every_epoch_own_maximum_and_the_live_value`
    // test above uses `EpochLog<0, 1>` but never overflows (only 3
    // epochs), so it could not have caught this. The four tests below
    // exercise `NG >= 1` together with an actual overflow.

    #[test]
    fn epoch_log_overflow_gauge_peak_in_a_dropped_epoch_still_reflected_in_aggregate() {
        let log: EpochLog<0, 1> = EpochLog::default();

        // The first EPOCH_CAPACITY epochs (all kept) have a small peak.
        for i in 0..EPOCH_CAPACITY {
            let start = (i as u64) * 1_000;
            log.close([], [10], start, start + 500, None);
        }
        assert_eq!(log.epochs_closed(), EPOCH_CAPACITY as u32);

        // The very next epoch (dropped -- capacity already reached)
        // carries the largest peak of the whole run. Two more dropped
        // epochs follow with smaller peaks, so this also exercises "more
        // than one epoch dropped" alongside the one holding the peak.
        let dropped_start = (EPOCH_CAPACITY as u64) * 1_000;
        log.close([], [999], dropped_start, dropped_start + 500, None);
        log.close([], [20], dropped_start + 500, dropped_start + 1_000, None);
        log.close([], [5], dropped_start + 1_000, dropped_start + 1_500, None);

        assert_eq!(
            log.epochs_closed(),
            EPOCH_CAPACITY as u32,
            "closed count must stay saturated"
        );
        assert_eq!(
            log.epochs_dropped(),
            3,
            "3 epoch-close attempts past capacity"
        );

        assert_eq!(
            log.aggregate_gauge_max(0, /* live_value = */ 1),
            999,
            "a gauge peak recorded in a DROPPED epoch must still be reflected in the \
             whole-process aggregate -- the exact scenario defect 2's fix (overflow_gauge_max) \
             exists for; pre-fix this returns 10 (only the kept epochs' peak), not 999"
        );
    }

    #[test]
    fn epoch_log_overflow_aggregate_still_reflects_a_larger_peak_in_a_kept_epoch() {
        let log: EpochLog<0, 1> = EpochLog::default();

        for i in 0..EPOCH_CAPACITY {
            let start = (i as u64) * 1_000;
            // One kept epoch holds the largest peak of the whole run.
            let peak = if i == 5 { 500 } else { 10 };
            log.close([], [peak], start, start + 500, None);
        }

        // Push the log into overflow with epochs whose own peaks are
        // smaller than the kept epoch's.
        let dropped_start = (EPOCH_CAPACITY as u64) * 1_000;
        log.close([], [50], dropped_start, dropped_start + 500, None);
        log.close([], [80], dropped_start + 500, dropped_start + 1_000, None);
        assert_eq!(log.epochs_dropped(), 2);

        assert_eq!(
            log.aggregate_gauge_max(0, /* live_value = */ 1),
            500,
            "a larger peak recorded in a KEPT epoch must still win over smaller dropped-epoch \
             peaks and a smaller live value"
        );
    }

    #[test]
    fn epoch_log_overflow_aggregate_still_reflects_a_larger_live_value() {
        let log: EpochLog<0, 1> = EpochLog::default();

        for i in 0..EPOCH_CAPACITY {
            let start = (i as u64) * 1_000;
            log.close([], [50], start, start + 500, None);
        }
        let dropped_start = (EPOCH_CAPACITY as u64) * 1_000;
        log.close([], [80], dropped_start, dropped_start + 500, None);
        assert_eq!(log.epochs_dropped(), 1);

        assert_eq!(
            log.aggregate_gauge_max(0, /* live_value = */ 600),
            600,
            "a live (in-progress, not-yet-closed) value larger than every closed epoch's own \
             peak -- kept or dropped -- must still win"
        );
    }

    #[test]
    fn epoch_log_overflow_gauge_peak_in_a_dropped_epoch_smaller_than_a_kept_epochs_peak_does_not_override_it(
    ) {
        // Proves the fix is a genuine max over kept + dropped + live, not
        // a shortcut that always prefers the overflow mark -- if it did,
        // this would incorrectly return the dropped epoch's smaller peak
        // (or otherwise fail to prefer the kept epoch's larger one).
        let log: EpochLog<0, 1> = EpochLog::default();

        for i in 0..EPOCH_CAPACITY {
            let start = (i as u64) * 1_000;
            let peak = if i == 10 { 200 } else { 1 };
            log.close([], [peak], start, start + 500, None);
        }
        let dropped_start = (EPOCH_CAPACITY as u64) * 1_000;
        log.close([], [20], dropped_start, dropped_start + 500, None); // dropped, smaller than 200
        assert_eq!(log.epochs_dropped(), 1);

        assert_eq!(
            log.aggregate_gauge_max(0, /* live_value = */ 5),
            200,
            "a smaller peak in a dropped epoch must not override a larger peak already \
             recorded in a kept epoch"
        );
    }

    #[test]
    fn epoch_log_single_writer_concurrent_reader_never_observes_a_partial_slot() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::thread;

        // Bounded by construction, same style as
        // counter_single_writer_concurrent_reader_is_race_free_and_monotonic
        // above: a fixed writer loop, a reader that polls in a tight loop
        // and takes one final sample after observing completion.
        const N: usize = 20;
        // Compile-time, not `assert!(N <= EPOCH_CAPACITY, ...)`: this pins
        // that the test exercises the non-overflow path regardless of
        // whether the test actually runs, and fails the build outright
        // (rather than clippy::assertions_on_constants on a runtime assert
        // over two compile-time constants) if EPOCH_CAPACITY ever shrinks
        // below N -- race safety here is orthogonal to overflow, so this
        // test must not silently start exercising the overflow path too.
        const _: () = assert!(N <= EPOCH_CAPACITY);

        let log: Arc<EpochLog<1, 0>> = Arc::new(EpochLog::default());
        let writer_done = Arc::new(AtomicBool::new(false));

        let writer_log = Arc::clone(&log);
        let writer_flag = Arc::clone(&writer_done);
        let writer = thread::spawn(move || {
            let mut cumulative: u64 = 0;
            for i in 0..N {
                // Every epoch's own delta is deterministically 100 -- a
                // reader observing anything else at a published index
                // would mean it read a torn/partial slot.
                cumulative += 100;
                let start = (i as u64) * 1_000_000;
                let end = start + 500_000;
                writer_log.close([cumulative], [], start, end, None);
            }
            writer_flag.store(true, Ordering::Relaxed);
        });

        let reader_log = Arc::clone(&log);
        let reader_flag = Arc::clone(&writer_done);
        let reader = thread::spawn(move || {
            let mut max_closed_seen = 0u32;
            loop {
                let done = reader_flag.load(Ordering::Relaxed);
                let closed = reader_log.epochs_closed();
                max_closed_seen = max_closed_seen.max(closed);

                for i in 0..closed {
                    let r = reader_log.record(i as usize).unwrap_or_else(|| {
                        panic!("index {i} below published epochs_closed={closed} must be readable")
                    });
                    assert_eq!(
                        r.counters,
                        [100],
                        "a published slot's delta must always be exactly 100 -- any other value \
                         (e.g. 0, or a torn mix of old/new bytes) means the reader observed a \
                         partially-written slot"
                    );
                    assert_eq!(
                        r.start_uptime_ns,
                        (i as u64) * 1_000_000,
                        "a published slot's start_uptime_ns must always match its index -- a \
                         mismatch means a torn read"
                    );
                    assert!(
                        r.end_uptime_ns > r.start_uptime_ns,
                        "a published slot must never have end <= start (a zeroed/partial write)"
                    );
                }

                if done {
                    break;
                }
            }
            max_closed_seen
        });

        writer.join().expect("writer thread panicked");
        reader.join().expect("reader thread panicked");

        assert_eq!(
            log.epochs_closed(),
            N as u32,
            "final state must have exactly N closed epochs"
        );
        assert_eq!(log.epochs_dropped(), 0);
        for i in 0..N {
            let r = log.record(i).unwrap();
            assert_eq!(r.counters, [100]);
        }
    }

    // ── EpochFirstLoss ───────────────────────────────────────────────────
    //
    // Per-epoch first-loss segmentation -- see this module's own
    // "Per-epoch first-loss segmentation" design section (right above
    // `EpochFirstLoss`'s definition) for the full contract these tests
    // pin.

    #[test]
    fn epoch_first_loss_fresh_take_is_none() {
        // Pin 1: a brand new EpochFirstLoss has recorded nothing yet.
        let mut efl = EpochFirstLoss::default();
        assert_eq!(efl.take(), None);
    }

    #[test]
    fn epoch_first_loss_record_within_one_epoch_second_call_does_not_overwrite_the_scratch() {
        // Pin 2: first record() within an epoch wins the scratch; a second
        // record() in the SAME epoch (before any take()) must not replace
        // it -- take() must still return the FIRST event, not the second.
        // Two distinct FirstLossReason variants make this unambiguous.
        let mut efl = EpochFirstLoss::default();
        let tracker = StageLossTracker::default();

        let first_detail = StageLossDetail::RingFull {
            from: Stage::Dpds,
            to: Stage::Nic,
        };
        efl.record(
            &tracker,
            FirstLossReason::DescriptorFull,
            first_detail,
            100.0,
        );

        let second_detail = StageLossDetail::HardwareDrop {
            from: Stage::Dpds,
            count: 7,
        };
        efl.record(&tracker, FirstLossReason::RxMissed, second_detail, 200.0);

        assert_eq!(
            efl.take(),
            Some(StageLossEvent {
                reason: FirstLossReason::DescriptorFull,
                detail: first_detail,
                relative_ns: 100.0
            }),
            "take() must return the FIRST event recorded this epoch, not the second"
        );
    }

    #[test]
    fn epoch_first_loss_record_always_reaches_the_underlying_tracker_independent_of_the_scratchs_own_state(
    ) {
        // Pin 3: record() must always call tracker.try_record too,
        // independent of whether the EpochFirstLoss scratch has already
        // claimed this epoch's slot -- the two latches (this epoch's
        // scratch, and the tracker's whole-process latch) are independent
        // of one another, not one gating the other. Demonstrated via the
        // tracker's OWN first-writer-wins rule: whichever of the two
        // record() calls' reasons is passed to try_record FIRST is what
        // tracker.current() must report, proving both calls actually
        // reached try_record (if the second record() call were somehow
        // gated by the scratch already being claimed, the tracker would
        // never see the second reason at all, but that's a different
        // question from which one *wins* -- so this test uses two
        // independent trackers, one per call, to prove each individual
        // record() call reaches its own tracker).
        let mut efl = EpochFirstLoss::default();
        let tracker_a = StageLossTracker::default();
        let tracker_b = StageLossTracker::default();

        efl.record(
            &tracker_a,
            FirstLossReason::DescriptorFull,
            StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic,
            },
            100.0,
        );
        // The scratch has already claimed this epoch's slot at this point
        // (pin 2 above) -- but the SECOND record() call below must still
        // reach ITS OWN tracker (tracker_b), independent of that.
        efl.record(
            &tracker_b,
            FirstLossReason::RxMissed,
            StageLossDetail::HardwareDrop {
                from: Stage::Dpds,
                count: 7,
            },
            200.0,
        );

        assert!(
            tracker_a.current().is_some(),
            "the first record() call must have reached tracker_a.try_record, regardless of the \
             scratch's own state at that point"
        );
        assert!(
            tracker_b.current().is_some(),
            "the second record() call must ALSO have reached tracker_b.try_record, even though \
             the EpochFirstLoss scratch had already claimed this epoch's slot from the first \
             call -- the scratch's own first-writer-wins latch must not gate calls into the \
             tracker at all"
        );
        assert_eq!(
            tracker_a.current().unwrap().reason,
            FirstLossReason::DescriptorFull,
            "tracker_a's own first (and only) record() call must win at the whole-process level, \
             per StageLossTracker's own first-writer-wins rule"
        );
        assert_eq!(
            tracker_b.current().unwrap().reason,
            FirstLossReason::RxMissed,
            "tracker_b's own first (and only) record() call must win at the whole-process level"
        );
    }

    #[test]
    fn epoch_first_loss_take_after_take_with_no_intervening_record_returns_none_the_second_time() {
        // Pin 4: take() resets the scratch -- a take() immediately
        // followed by another take() (no record() in between) must return
        // None the second time.
        let mut efl = EpochFirstLoss::default();
        let tracker = StageLossTracker::default();

        efl.record(
            &tracker,
            FirstLossReason::DescriptorFull,
            StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic,
            },
            100.0,
        );
        assert!(
            efl.take().is_some(),
            "the first take() must return the recorded event"
        );
        assert_eq!(
            efl.take(),
            None,
            "a second take() with no intervening record() must return None -- the first take() \
             must have genuinely reset the scratch, not merely peeked at it"
        );
    }

    #[test]
    fn epoch_first_loss_fresh_epoch_after_take_starts_clean_and_can_record_a_new_event() {
        // Pin 5: after a take(), a NEW record() call must be able to latch
        // its own event -- the old (already-taken) event must not still be
        // returned, and the scratch must not be stuck permanently claimed
        // from the first epoch.
        let mut efl = EpochFirstLoss::default();
        let tracker = StageLossTracker::default();

        efl.record(
            &tracker,
            FirstLossReason::DescriptorFull,
            StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic,
            },
            100.0,
        );
        efl.take();

        let new_detail = StageLossDetail::HardwareDrop {
            from: Stage::Dpds,
            count: 7,
        };
        efl.record(&tracker, FirstLossReason::RxError, new_detail, 300.0);

        assert_eq!(
            efl.take(),
            Some(StageLossEvent {
                reason: FirstLossReason::RxError,
                detail: new_detail,
                relative_ns: 300.0
            }),
            "a fresh epoch after take() must start clean and record its own new event, not \
             still hold (or be blocked by) the previous epoch's already-taken event"
        );
    }

    // ── EpochLog::close's new first-loss parameter / EpochLog::first_loss ──
    //
    // These pin the read-side CONTRACT of `EpochLog::first_loss` (see its
    // own doc comment for the exact outer/inner Option shape) together with
    // `close`'s new parameter.

    #[test]
    fn epoch_log_close_with_some_event_makes_it_retrievable_via_first_loss() {
        // Pin 7 (part 1): closing an epoch with Some(event) must make that
        // exact event retrievable as Some(Some(event)) via first_loss().
        let log: EpochLog<1, 0> = EpochLog::default();
        let event = StageLossEvent {
            reason: FirstLossReason::DescriptorFull,
            detail: StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic,
            },
            relative_ns: 555.0,
        };

        log.close([10], [], 0, 100, Some(event));

        assert_eq!(
            log.first_loss(0),
            Some(Some(event)),
            "closing with Some(event) must make it retrievable via first_loss(0) as \
             Some(Some(event)) -- outer Some because index 0 is published, inner Some(event) \
             because a loss WAS recorded this epoch"
        );
    }

    #[test]
    fn epoch_log_close_with_none_records_clean_distinguishable_from_not_yet_published() {
        // Pin 7 (part 2): closing with None must record "Clean"
        // (Some(None)), which must be distinguishable from an index that
        // hasn't been published at all (plain None) -- the exact ambiguity
        // the outer/inner Option design resolves.
        let log: EpochLog<1, 0> = EpochLog::default();
        log.close([10], [], 0, 100, None);

        assert_eq!(
            log.first_loss(0),
            Some(None),
            "closing with None must record Clean (Some(None)) for a published index -- distinct \
             from an unpublished index's plain None"
        );
        assert_eq!(
            log.first_loss(1),
            None,
            "index 1 has never been closed -- must be plain None (not yet published), not \
             confused with a published-but-Clean epoch"
        );
    }

    #[test]
    fn epoch_log_multiple_epochs_each_get_their_own_independent_first_loss_record() {
        // Pin 8: close 3 epochs with different (or absent) first-loss
        // events; each must be retrieved correctly and independently, with
        // no bleed-through from one epoch's record into another's.
        let log: EpochLog<1, 0> = EpochLog::default();

        let event0 = StageLossEvent {
            reason: FirstLossReason::DescriptorFull,
            detail: StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic,
            },
            relative_ns: 10.0,
        };
        let event2 = StageLossEvent {
            reason: FirstLossReason::RxNoMbuf,
            detail: StageLossDetail::HardwareDrop {
                from: Stage::Dpds,
                count: 7,
            },
            relative_ns: 30.0,
        };

        log.close([1], [], 0, 100, Some(event0));
        log.close([2], [], 100, 200, None); // epoch 1: Clean
        log.close([3], [], 200, 300, Some(event2));

        assert_eq!(
            log.first_loss(0),
            Some(Some(event0)),
            "epoch 0 must keep its own event"
        );
        assert_eq!(
            log.first_loss(1),
            Some(None),
            "epoch 1 must be Clean, not bleed epoch 0's event"
        );
        assert_eq!(
            log.first_loss(2),
            Some(Some(event2)),
            "epoch 2 must keep its own (different) event, not epoch 0's"
        );
    }

    #[test]
    fn epoch_log_overflow_drops_the_first_loss_event_of_a_dropped_epoch_too() {
        // Pin 9: a dropped (overflowed) epoch's first-loss data is simply
        // dropped along with the rest of that epoch's data -- see the
        // "Per-epoch first-loss segmentation" design section's own doc
        // comment for the reasoning (no meaningful aggregate exists for a
        // qualitative reason/detail the way a numeric gauge max exists).
        let log: EpochLog<1, 0> = EpochLog::default();

        for i in 0..EPOCH_CAPACITY {
            let start = (i as u64) * 1_000;
            log.close([1], [], start, start + 500, None);
        }
        assert_eq!(log.epochs_closed(), EPOCH_CAPACITY as u32);

        // This epoch is dropped (capacity already reached) -- even though
        // it carries a real event, that event must not survive anywhere.
        let dropped_start = (EPOCH_CAPACITY as u64) * 1_000;
        let dropped_event = StageLossEvent {
            reason: FirstLossReason::DpdsReject,
            detail: StageLossDetail::Single { from: Stage::Dpds },
            relative_ns: 999.0,
        };
        log.close(
            [1],
            [],
            dropped_start,
            dropped_start + 500,
            Some(dropped_event),
        );

        assert_eq!(log.epochs_dropped(), 1);
        assert_eq!(
            log.first_loss(EPOCH_CAPACITY),
            None,
            "a dropped epoch's first-loss event must not be retrievable at any index -- there is \
             no overflow survivor for first-loss data, unlike overflow_gauge_max for gauges"
        );
    }

    #[test]
    fn epoch_log_single_writer_concurrent_reader_never_observes_a_partial_first_loss_slot() {
        // Pin 10: same style as
        // epoch_log_single_writer_concurrent_reader_never_observes_a_partial_slot
        // above, but for the new first-loss data specifically -- a writer
        // closes epochs with REAL, distinguishable first-loss events (not
        // just None, so a torn read would actually be detectable), while a
        // reader repeatedly reads; no partial/inconsistent state may ever
        // be observed, and the final state must be exact.
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::thread;

        const N: usize = 20;
        const _: () = assert!(N <= EPOCH_CAPACITY);

        let log: Arc<EpochLog<1, 0>> = Arc::new(EpochLog::default());
        let writer_done = Arc::new(AtomicBool::new(false));

        let writer_log = Arc::clone(&log);
        let writer_flag = Arc::clone(&writer_done);
        let writer = thread::spawn(move || {
            for i in 0..N {
                let start = (i as u64) * 1_000_000;
                let end = start + 500_000;
                // Every epoch's own event carries relative_ns == its own
                // index * 1000.0 exactly -- a reader observing any other
                // value at a published index means it read a torn/partial
                // first-loss slot.
                let event = StageLossEvent {
                    reason: FirstLossReason::DescriptorFull,
                    detail: StageLossDetail::RingFull {
                        from: Stage::Dpds,
                        to: Stage::Nic,
                    },
                    relative_ns: (i as f64) * 1000.0,
                };
                writer_log.close([1], [], start, end, Some(event));
            }
            writer_flag.store(true, Ordering::Relaxed);
        });

        let reader_log = Arc::clone(&log);
        let reader_flag = Arc::clone(&writer_done);
        let reader = thread::spawn(move || loop {
            let done = reader_flag.load(Ordering::Relaxed);
            let closed = reader_log.epochs_closed();

            for i in 0..closed {
                let fl = reader_log.first_loss(i as usize).unwrap_or_else(|| {
                    panic!("index {i} below published epochs_closed={closed} must be readable")
                });
                let event = fl.unwrap_or_else(|| {
                    panic!("index {i} must have a recorded event, never Clean, in this test")
                });
                assert_eq!(
                    event.relative_ns,
                    (i as f64) * 1000.0,
                    "a published first-loss slot's relative_ns must always match its index \
                         exactly -- any other value means a torn/partial read"
                );
                assert_eq!(event.reason, FirstLossReason::DescriptorFull);
            }

            if done {
                break;
            }
        });

        writer.join().expect("writer thread panicked");
        reader.join().expect("reader thread panicked");

        assert_eq!(
            log.epochs_closed(),
            N as u32,
            "final state must have exactly N closed epochs"
        );
        for i in 0..N {
            let fl = log.first_loss(i).unwrap().unwrap();
            assert_eq!(fl.relative_ns, (i as f64) * 1000.0);
        }
    }
}

#[cfg(feature = "telemetry")]
mod nic_epoch_tracker_tests {
    use dpds::counters::*;
    use std::cell::Cell;

    // ── Derives ──────────────────────────────────────────────────────────

    #[test]
    fn nic_epoch_record_derives_debug_clone_copy_partial_eq_default() {
        // Pins the required derive list -- a struct missing any
        // of these would fail to compile this test.
        let a = NicEpochRecord::default();
        let b = a; // Copy, not a move -- `a` must still be usable below.
        assert_eq!(a, b);
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    // ── No close without arrivals ────────────────────────────────────────

    #[test]
    fn no_close_without_arrivals_even_after_a_long_idle() {
        let mut t = NicEpochTracker::new(0.0, 0, 0, 0);
        for i in 1..=5u32 {
            // Every gap here is a large multiple of EPOCH_IDLE_GAP_NS, but
            // the rx_packets counter never advances -- an epoch that has
            // seen zero arrivals must never close, no matter how long the
            // idle gap (mirrors EpochDriver's own pre-traffic-startup
            // behaviour).
            let now_ns = (i as f64) * (EPOCH_IDLE_GAP_NS * 3.0);
            let result = t.poll(now_ns, 0, 0, || {
                panic!("read_port_discards must not be called -- no epoch ever closes here")
            });
            assert_eq!(
                result, None,
                "poll {i}: an epoch with zero arrivals must never close"
            );
        }
    }

    // ── No close before the idle gap is exceeded ────────────────────────

    #[test]
    fn no_close_before_idle_gap_exceeded() {
        let mut t = NicEpochTracker::new(0.0, 0, 0, 0);
        assert_eq!(
            t.poll(1_000.0, 100, 0, || panic!(
                "must not be called on an arrival poll"
            )),
            None
        );

        let under_threshold_now = 1_000.0 + (EPOCH_IDLE_GAP_NS - 1.0);
        let result = t.poll(under_threshold_now, 100, 0, || {
            panic!("must not be called on a non-closing idle poll")
        });
        assert_eq!(result, None);
    }

    // ── Closes after the idle gap, with the correct window and deltas ──

    #[test]
    fn closes_after_idle_gap_with_correct_window_and_deltas() {
        let read_calls = Cell::new(0u32);
        let mut t = NicEpochTracker::new(0.0, 1_000, 2, 50);

        // Arrival poll: 500 new rx packets, 1 new imissed -- must not
        // close, and must not read discards.
        assert_eq!(
            t.poll(10.0, 1_500, 3, || {
                read_calls.set(read_calls.get() + 1);
                panic!("must not be called on a non-closing poll")
            }),
            None
        );

        let close_now = 10.0 + EPOCH_IDLE_GAP_NS + 1.0;
        let record = t.poll(close_now, 1_500, 3, || {
            read_calls.set(read_calls.get() + 1);
            Some(60)
        });

        assert_eq!(
            record,
            Some(NicEpochRecord {
                start_uptime_ns: 0.0,
                end_uptime_ns: close_now,
                rx_packets: 500,
                imissed: 1,
                rx_port_discards: 10,
            }),
            "start must be the tracker's own stage_start for the first epoch; rx_packets/imissed \
             deltas are relative to new()'s own baseline (1_000/2); rx_port_discards is the read \
             value (60) minus new()'s own baseline (50)"
        );
        assert_eq!(
            read_calls.get(),
            1,
            "read_port_discards must be called exactly once, only on the closing poll"
        );
    }

    // ── Second epoch starts where the first ended; deltas are relative to
    //    the first close, not to new() ──────────────────────────────────

    #[test]
    fn second_epoch_starts_where_first_ended_and_deltas_are_relative_to_first_close() {
        let mut t = NicEpochTracker::new(0.0, 0, 0, 0);
        assert_eq!(t.poll(10.0, 100, 1, || None), None);

        let close1_now = 10.0 + EPOCH_IDLE_GAP_NS + 1.0;
        let r1 = t
            .poll(close1_now, 100, 1, || Some(5))
            .expect("first epoch must close");
        assert_eq!(r1.start_uptime_ns, 0.0);
        assert_eq!(r1.end_uptime_ns, close1_now);
        assert_eq!(r1.rx_packets, 100);
        assert_eq!(r1.imissed, 1);
        assert_eq!(r1.rx_port_discards, 5);

        let resume_now = close1_now + 1.0;
        assert_eq!(
            t.poll(resume_now, 250, 1, || panic!(
                "must not be called: not closing"
            )),
            None
        );

        let close2_now = resume_now + EPOCH_IDLE_GAP_NS + 1.0;
        let r2 = t
            .poll(close2_now, 250, 4, || Some(9))
            .expect("second epoch must close");
        assert_eq!(
            r2.start_uptime_ns, close1_now,
            "the second epoch must start exactly where the first one ended"
        );
        assert_eq!(r2.end_uptime_ns, close2_now);
        assert_eq!(
            r2.rx_packets, 150,
            "delta must be relative to the first close's own baseline (250-100=150), not \
             cumulative since new() (250-0)"
        );
        assert_eq!(
            r2.imissed, 3,
            "4 - 1 = 3, relative to the first close's baseline"
        );
        assert_eq!(
            r2.rx_port_discards, 4,
            "9 - 5 = 4, relative to the first close's own baseline, not new()'s"
        );
    }

    // ── read_port_discards call-count discipline ────────────────────────

    #[test]
    fn read_port_discards_not_invoked_on_any_non_closing_poll() {
        let calls = Cell::new(0u32);
        let mut t = NicEpochTracker::new(0.0, 0, 0, 0);
        for i in 0..5u32 {
            let now_ns = (i as f64) * 10.0;
            let rx_packets_cum = (i as u64) * 20;
            let result = t.poll(now_ns, rx_packets_cum, 0, || {
                calls.set(calls.get() + 1);
                Some(0)
            });
            assert_eq!(
                result, None,
                "poll {i} must not close (tight polling, no idle gap)"
            );
        }
        assert_eq!(
            calls.get(),
            0,
            "read_port_discards must never be invoked when no epoch closes"
        );
    }

    #[test]
    fn read_port_discards_invoked_exactly_once_per_close_across_two_closes() {
        let calls = Cell::new(0u32);
        let mut t = NicEpochTracker::new(0.0, 0, 0, 0);

        t.poll(10.0, 100, 0, || {
            calls.set(calls.get() + 1);
            Some(1)
        });
        let close1 = 10.0 + EPOCH_IDLE_GAP_NS + 1.0;
        t.poll(close1, 100, 0, || {
            calls.set(calls.get() + 1);
            Some(1)
        });
        assert_eq!(calls.get(), 1, "exactly one read for the first close");

        let resume = close1 + 1.0;
        t.poll(resume, 200, 0, || {
            calls.set(calls.get() + 1);
            Some(2)
        });
        let close2 = resume + EPOCH_IDLE_GAP_NS + 1.0;
        t.poll(close2, 200, 0, || {
            calls.set(calls.get() + 1);
            Some(2)
        });
        assert_eq!(calls.get(), 2, "exactly one more read for the second close");
    }

    // ── A failed read reports a zero delta, and does NOT advance the
    //    discard baseline -- the next successful read catches up ────────

    #[test]
    fn none_read_yields_zero_delta_and_next_successful_close_catches_up() {
        let mut t = NicEpochTracker::new(0.0, 0, 0, 100);

        t.poll(10.0, 50, 0, || unreachable!("not a closing poll"));
        let close1 = 10.0 + EPOCH_IDLE_GAP_NS + 1.0;
        let r1 = t.poll(close1, 50, 0, || None).expect("must close");
        assert_eq!(
            r1.rx_port_discards, 0,
            "a failed read must report a 0 delta for this epoch, not panic or propagate None"
        );

        let resume = close1 + 1.0;
        t.poll(resume, 80, 0, || unreachable!("not a closing poll"));
        let close2 = resume + EPOCH_IDLE_GAP_NS + 1.0;
        let r2 = t.poll(close2, 80, 0, || Some(130)).expect("must close");
        assert_eq!(
            r2.rx_port_discards, 30,
            "the discard baseline must NOT have advanced on the failed read, so the next \
             successful read (130) catches up against the ORIGINAL baseline (100), giving 30 -- \
             not 130 (relative to a wrongly-advanced baseline of 0) and not some other value"
        );
    }

    // ── Counters going backwards must saturate to 0, never underflow ───

    #[test]
    fn counters_going_backwards_saturate_to_zero_instead_of_underflowing() {
        let mut t = NicEpochTracker::new(0.0, 1_000, 500, 200);

        // A later poll reports SMALLER cumulative rx_packets/imissed than
        // the tracker's own baseline (e.g. a NIC counter reset) -- must
        // saturate the arrivals computation to 0, not panic, and must not
        // close (0 arrivals, nothing seen yet this epoch).
        let result = t.poll(10.0, 900, 400, || panic!("must not be called: no close"));
        assert_eq!(result, None);

        // A real arrival afterwards still works normally and can close
        // later.
        assert_eq!(
            t.poll(20.0, 950, 400, || panic!("must not be called: not closing")),
            None
        );
        let close_now = 20.0 + EPOCH_IDLE_GAP_NS + 1.0;
        let record = t
            .poll(close_now, 950, 400, || Some(150))
            .expect("must close: a real arrival was seen at t=20.0");

        // Every delta is relative to new()'s own baseline (1_000/500/200),
        // all of which are LARGER than the values ever reported afterward
        // (950/400/150) -- every one of these must saturate to 0, not
        // underflow/wrap.
        assert_eq!(
            record.rx_packets, 0,
            "950 - 1_000 (baseline) must saturate to 0"
        );
        assert_eq!(record.imissed, 0, "400 - 500 (baseline) must saturate to 0");
        assert_eq!(
            record.rx_port_discards, 0,
            "150 - 200 (baseline) must saturate to 0"
        );
    }

    // ── Arrival-only polls never close, however many in a row ──────────

    #[test]
    fn a_long_run_of_arrival_only_polls_never_closes() {
        let mut t = NicEpochTracker::new(0.0, 0, 0, 0);
        let mut cum = 0u64;
        for i in 1..=50u32 {
            cum += 10;
            let now_ns = i as f64; // tight spacing, well under the idle gap
            let result = t.poll(now_ns, cum, 0, || {
                panic!("an arrival poll must never read port discards")
            });
            assert_eq!(
                result, None,
                "poll {i} saw arrivals; an arrival tick can never itself be a boundary"
            );
        }
    }
}
