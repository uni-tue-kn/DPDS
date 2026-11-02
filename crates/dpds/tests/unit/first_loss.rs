//! First-loss machinery, ported from preof's `peof/tests/first_loss.rs`,
//! `peof/tests/hardware_drop.rs` and the `first_loss.rs` unit tests, reduced
//! to the DPDS reasons/stages/shapes:
//!
//! - `FirstLossReason`: `DpdsReject`, `DescriptorFull`, and the NIC-level
//!   `RxMissed`/`RxNoMbuf`/`RxError`/`TxError` (wire spelling `IMissed`,
//!   `RX_NoMbuf`, `IErrors`, `OErrors`); `Stage`: `Dpds` ("dpds"), `Nic` ("nic").
//! - `StageLossDetail`: `Single{from}` ("from=dpds"), `RingFull{from,to}`
//!   ("from=dpds,to=nic", reused by DescriptorFull), `HardwareDrop{from,count}`
//!   ("from=dpds,count=N").
//! - `StageLossTracker`: first writer wins, no overwrite, exact round trip.
//! - `format_first_loss`, `detect_hardware_drop` (not gated: pure).

use dpds::first_loss::{
    detect_hardware_drop, format_first_loss, FirstLossFields, FirstLossReason, Stage,
    StageLossDetail, StageLossEvent,
};

// ── format_first_loss: pure, not gated ───────────────────────────────────

#[test]
fn format_first_loss_of_none_is_the_clean_default() {
    assert_eq!(
        format_first_loss(None),
        FirstLossFields {
            reason: "Clean".to_string(),
            detail: String::new(),
            relative_ns: 0
        }
    );
}

#[test]
fn format_first_loss_dpds_reject() {
    let f = format_first_loss(Some(StageLossEvent {
        reason: FirstLossReason::DpdsReject,
        detail: StageLossDetail::Single { from: Stage::Dpds },
        relative_ns: 4200.0,
    }));
    assert_eq!(f.reason, "DpdsReject");
    assert_eq!(f.detail, "from=dpds");
    assert_eq!(f.relative_ns, 4200);
}

#[test]
fn format_first_loss_descriptor_full_uses_the_ring_full_shape() {
    let f = format_first_loss(Some(StageLossEvent {
        reason: FirstLossReason::DescriptorFull,
        detail: StageLossDetail::RingFull {
            from: Stage::Dpds,
            to: Stage::Nic,
        },
        relative_ns: 55.5,
    }));
    assert_eq!(f.reason, "DescriptorFull");
    assert_eq!(f.detail, "from=dpds,to=nic");
    assert_eq!(
        f.relative_ns, 55,
        "relative_ns truncates (as u64), not rounds"
    );
}

#[test]
fn format_first_loss_hardware_reasons_use_the_dpdk_counter_spelling() {
    let cases = [
        (FirstLossReason::RxMissed, "IMissed"),
        (FirstLossReason::RxNoMbuf, "RX_NoMbuf"),
        (FirstLossReason::RxError, "IErrors"),
        (FirstLossReason::TxError, "OErrors"),
    ];
    for (reason, spelling) in cases {
        let f = format_first_loss(Some(StageLossEvent {
            reason,
            detail: StageLossDetail::HardwareDrop {
                from: Stage::Dpds,
                count: 42,
            },
            relative_ns: 1000.0,
        }));
        assert_eq!(f.reason, spelling);
        assert_eq!(f.detail, "from=dpds,count=42");
        assert_eq!(f.relative_ns, 1000);
        assert_eq!(reason.to_string(), spelling, "Display is the wire spelling");
    }
}

#[test]
fn format_first_loss_hardware_drop_count_zero_is_not_dropped_from_the_string() {
    let f = format_first_loss(Some(StageLossEvent {
        reason: FirstLossReason::RxError,
        detail: StageLossDetail::HardwareDrop {
            from: Stage::Dpds,
            count: 0,
        },
        relative_ns: 3.0,
    }));
    assert_eq!(f.detail, "from=dpds,count=0");
}

#[test]
fn software_reason_and_stage_display_spellings_are_pinned() {
    assert_eq!(FirstLossReason::DpdsReject.to_string(), "DpdsReject");
    assert_eq!(
        FirstLossReason::DescriptorFull.to_string(),
        "DescriptorFull"
    );
    assert_eq!(Stage::Dpds.to_string(), "dpds");
    assert_eq!(Stage::Nic.to_string(), "nic");
}

// ── detect_hardware_drop: ported verbatim from hardware_drop.rs ──────────

mod hardware_drop {
    use dpdk::port::PortStats;

    use super::*;

    fn stats(imissed: u64, rx_nombuf: u64, ierrors: u64, oerrors: u64) -> PortStats {
        PortStats {
            imissed,
            rx_nombuf,
            ierrors,
            oerrors,
            ..Default::default()
        }
    }

    #[test]
    fn identical_snapshots_report_no_drop() {
        assert_eq!(
            detect_hardware_drop(&stats(5, 3, 2, 1), &stats(5, 3, 2, 1)),
            None
        );
        assert_eq!(
            detect_hardware_drop(&stats(0, 0, 0, 0), &stats(0, 0, 0, 0)),
            None
        );
    }

    #[test]
    fn unrelated_counters_changing_does_not_trigger_a_drop() {
        let prev = PortStats {
            ipackets: 100,
            opackets: 100,
            ibytes: 1000,
            obytes: 1000,
            ..Default::default()
        };
        let curr = PortStats {
            ipackets: 200,
            opackets: 250,
            ibytes: 2000,
            obytes: 2500,
            ..Default::default()
        };
        assert_eq!(detect_hardware_drop(&prev, &curr), None);
    }

    #[test]
    fn each_counter_increase_reports_its_reason_with_the_delta() {
        assert_eq!(
            detect_hardware_drop(&stats(10, 0, 0, 0), &stats(25, 0, 0, 0)),
            Some((FirstLossReason::RxMissed, 15))
        );
        assert_eq!(
            detect_hardware_drop(&stats(0, 4, 0, 0), &stats(0, 9, 0, 0)),
            Some((FirstLossReason::RxNoMbuf, 5))
        );
        assert_eq!(
            detect_hardware_drop(&stats(0, 0, 100, 0), &stats(0, 0, 107, 0)),
            Some((FirstLossReason::RxError, 7))
        );
        assert_eq!(
            detect_hardware_drop(&stats(0, 0, 0, 1), &stats(0, 0, 0, 4)),
            Some((FirstLossReason::TxError, 3))
        );
    }

    #[test]
    fn a_single_count_increase_of_exactly_one_is_still_reported() {
        assert_eq!(
            detect_hardware_drop(&stats(0, 0, 0, 0), &stats(1, 0, 0, 0)),
            Some((FirstLossReason::RxMissed, 1))
        );
    }

    #[test]
    fn a_decreasing_counter_is_no_drop_and_does_not_panic() {
        assert_eq!(
            detect_hardware_drop(&stats(100, 0, 0, 0), &stats(40, 0, 0, 0)),
            None
        );
        assert_eq!(
            detect_hardware_drop(&stats(0, 50, 0, 0), &stats(0, 1, 0, 0)),
            None
        );
        assert_eq!(
            detect_hardware_drop(&stats(0, 0, 999, 0), &stats(0, 0, 0, 0)),
            None
        );
        assert_eq!(
            detect_hardware_drop(&stats(0, 0, 0, 8), &stats(0, 0, 0, 3)),
            None
        );
    }

    #[test]
    fn one_counter_decreasing_while_another_increases_only_reports_the_increase() {
        assert_eq!(
            detect_hardware_drop(&stats(100, 0, 0, 5), &stats(40, 0, 0, 9)),
            Some((FirstLossReason::TxError, 4))
        );
    }

    #[test]
    fn ties_are_broken_imissed_then_rx_nombuf_then_ierrors_then_oerrors() {
        assert_eq!(
            detect_hardware_drop(&stats(1, 1, 1, 1), &stats(2, 2, 2, 2)),
            Some((FirstLossReason::RxMissed, 1))
        );
        assert_eq!(
            detect_hardware_drop(&stats(5, 1, 1, 1), &stats(5, 2, 2, 2)),
            Some((FirstLossReason::RxNoMbuf, 1))
        );
        assert_eq!(
            detect_hardware_drop(&stats(5, 5, 1, 1), &stats(5, 5, 2, 2)),
            Some((FirstLossReason::RxError, 1))
        );
        assert_eq!(
            detect_hardware_drop(&stats(5, 5, 5, 1), &stats(5, 5, 5, 6)),
            Some((FirstLossReason::TxError, 5))
        );
    }

    #[test]
    fn the_tie_break_reports_the_winning_counters_own_delta() {
        // deltas: imissed=3, rx_nombuf=5, ierrors=9, oerrors=4
        assert_eq!(
            detect_hardware_drop(&stats(10, 20, 30, 40), &stats(13, 25, 39, 44)),
            Some((FirstLossReason::RxMissed, 3))
        );
    }
}

// ── StageLossTracker: only with the feature (a no-op ZST otherwise) ─────

#[cfg(feature = "telemetry")]
mod tracker {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use dpds::first_loss::StageLossTracker;

    use super::*;

    #[test]
    fn fresh_tracker_current_is_none() {
        assert_eq!(StageLossTracker::new().current(), None);
        assert_eq!(StageLossTracker::default().current(), None);
    }

    #[test]
    fn try_record_on_a_fresh_tracker_succeeds_and_current_reflects_it() {
        let tracker = StageLossTracker::new();
        assert!(tracker.try_record(
            FirstLossReason::DpdsReject,
            StageLossDetail::Single { from: Stage::Dpds },
            123.5,
        ));
        assert_eq!(
            tracker.current(),
            Some(StageLossEvent {
                reason: FirstLossReason::DpdsReject,
                detail: StageLossDetail::Single { from: Stage::Dpds },
                relative_ns: 123.5,
            })
        );
    }

    #[test]
    fn second_try_record_after_a_winner_fails_and_leaves_every_field_untouched() {
        let tracker = StageLossTracker::new();
        let winner = StageLossDetail::HardwareDrop {
            from: Stage::Dpds,
            count: 188_617,
        };
        assert!(tracker.try_record(FirstLossReason::RxMissed, winner, 111.0));

        // the loser differs in every position: reason, shape, fields, time
        let recorded = tracker.try_record(
            FirstLossReason::DescriptorFull,
            StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic,
            },
            222.0,
        );
        assert!(!recorded);
        assert_eq!(
            tracker.current(),
            Some(StageLossEvent {
                reason: FirstLossReason::RxMissed,
                detail: winner,
                relative_ns: 111.0,
            })
        );
    }

    #[test]
    fn a_software_winner_is_not_overwritten_by_a_later_hardware_loss_and_vice_versa() {
        let tracker = StageLossTracker::new();
        let sw = StageLossDetail::RingFull {
            from: Stage::Dpds,
            to: Stage::Nic,
        };
        assert!(tracker.try_record(FirstLossReason::DescriptorFull, sw, 10.0));
        assert!(!tracker.try_record(
            FirstLossReason::RxMissed,
            StageLossDetail::HardwareDrop {
                from: Stage::Dpds,
                count: 999
            },
            20.0
        ));
        assert_eq!(
            tracker.current().unwrap().reason,
            FirstLossReason::DescriptorFull
        );

        let tracker = StageLossTracker::new();
        assert!(tracker.try_record(
            FirstLossReason::RxMissed,
            StageLossDetail::HardwareDrop {
                from: Stage::Dpds,
                count: 1
            },
            1.0
        ));
        assert!(!tracker.try_record(FirstLossReason::DescriptorFull, sw, 2.0));
        assert_eq!(tracker.current().unwrap().reason, FirstLossReason::RxMissed);
    }

    #[test]
    fn every_detail_shape_and_reason_round_trips_exactly() {
        let cases = [
            (
                FirstLossReason::DpdsReject,
                StageLossDetail::Single { from: Stage::Dpds },
            ),
            (
                FirstLossReason::DescriptorFull,
                StageLossDetail::RingFull {
                    from: Stage::Dpds,
                    to: Stage::Nic,
                },
            ),
            (
                FirstLossReason::RxMissed,
                StageLossDetail::HardwareDrop {
                    from: Stage::Dpds,
                    count: 42,
                },
            ),
            (
                FirstLossReason::RxNoMbuf,
                StageLossDetail::HardwareDrop {
                    from: Stage::Dpds,
                    count: u32::MAX,
                },
            ),
            (
                FirstLossReason::RxError,
                StageLossDetail::HardwareDrop {
                    from: Stage::Dpds,
                    count: 0, // a genuine zero is not "unset"
                },
            ),
            (
                FirstLossReason::TxError,
                StageLossDetail::HardwareDrop {
                    from: Stage::Dpds,
                    count: 7,
                },
            ),
        ];
        for (reason, detail) in cases {
            let tracker = StageLossTracker::new();
            assert!(tracker.try_record(reason, detail, 4200.0));
            let cur = tracker.current().expect("a recorded event must be Some");
            assert_eq!(cur.reason, reason);
            assert_eq!(cur.detail, detail);
            assert_eq!(cur.relative_ns, 4200.0);
        }
    }

    #[test]
    fn relative_ns_round_trips_bit_exactly() {
        for v in [0.0_f64, 21_500_000_000.0, 987_654_321.123_456_7] {
            let tracker = StageLossTracker::new();
            assert!(tracker.try_record(
                FirstLossReason::DpdsReject,
                StageLossDetail::Single { from: Stage::Dpds },
                v
            ));
            let got = tracker.current().unwrap().relative_ns;
            assert_eq!(got.to_bits(), v.to_bits());
        }
    }

    #[test]
    fn concurrent_try_record_has_exactly_one_winner_and_current_matches_it() {
        const N_THREADS: usize = 32;
        let tracker = Arc::new(StageLossTracker::new());
        let barrier = Arc::new(Barrier::new(N_THREADS));
        let win_count = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..N_THREADS)
            .map(|i| {
                let tracker = Arc::clone(&tracker);
                let barrier = Arc::clone(&barrier);
                let win_count = Arc::clone(&win_count);
                thread::spawn(move || {
                    barrier.wait();
                    let reason = match i % 3 {
                        0 => FirstLossReason::RxMissed,
                        1 => FirstLossReason::RxError,
                        _ => FirstLossReason::TxError,
                    };
                    // the thread index is encoded in count and relative_ns so a
                    // torn read (fields of two threads) is detectable
                    let detail = StageLossDetail::HardwareDrop {
                        from: Stage::Dpds,
                        count: i as u32 * 1000 + 7,
                    };
                    let won = tracker.try_record(reason, detail, i as f64);
                    if won {
                        win_count.fetch_add(1, Ordering::SeqCst);
                    }
                    won.then_some((i, reason, detail))
                })
            })
            .collect();
        let (winner, reason, detail) = handles
            .into_iter()
            .filter_map(|h| h.join().expect("recorder thread panicked"))
            .next()
            .expect("exactly one thread must report a win");
        assert_eq!(win_count.load(Ordering::SeqCst), 1);
        let cur = tracker.current().unwrap();
        assert_eq!(cur.reason, reason);
        assert_eq!(cur.detail, detail);
        assert_eq!(cur.relative_ns, winner as f64);
    }
}
