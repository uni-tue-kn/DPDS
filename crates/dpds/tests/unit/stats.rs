//! `dpds::stats`: the worker's `Telemetry` handle (loss recording, epoch
//! closing), the reply builders behind `/dpds/stats`, `/dpds/epochs`,
//! `/dpds/ports`, `/nic/hw_first_loss` and `/nic/epochs`, and the main
//! lcore's `NicPoller`.
//!
//! The reply builders return `dpds::reply::Reply` (ordered `(key, Value)` data), so the exact
//! wire fields are asserted here without an EAL; the registration shims that copy a `Reply` into
//! a `dpdk::telemetry::Dict` are covered by the e2e test.
//!
//! Builders and tests are telemetry-feature only (they need real counters),
//! except what must exist in both builds: `Stats`, `DirectionStats`,
//! `Telemetry`, `NicHwStats`, `NicPoller`, `PortInfo`, `DirectionInfo` and the
//! `register_*` fns (no-ops without the
//! feature: they must return `Ok` and touch no DPDK state).

use dpds::stats::{DirectionStats, NicHwStats, NicPoller, Stats, Telemetry};

#[test]
fn registration_without_the_feature_registers_nothing_and_succeeds() {
    if cfg!(feature = "telemetry") {
        return; // with the feature this needs an EAL; the e2e test covers it
    }
    let dirs: &'static DirectionStats = Box::leak(Box::new(DirectionStats::new(2)));
    assert!(DirectionStats::register_telemetry(dirs, 0.0).is_ok());
    let hw: &'static NicHwStats = Box::leak(Box::new(NicHwStats::new(2)));
    assert!(NicHwStats::register_telemetry(hw).is_ok());
    assert!(dpds::stats::register_port_telemetry(vec![], vec![], 0).is_ok());
}

#[test]
fn nic_poller_is_usable_in_every_build_and_never_panics() {
    let hw = NicHwStats::new(2);
    let initial = dpdk::port::PortStats::default();
    let mut p = NicPoller::new(1, 0.0, &initial, 0);
    let curr = dpdk::port::PortStats {
        ipackets: 10,
        imissed: 3,
        ..Default::default()
    };
    p.poll(1.0, &curr, &hw, || None);
    p.poll(2.0e9, &curr, &hw, || Some(0));
    if !cfg!(feature = "telemetry") {
        assert!(hw.first_loss[1].current().is_none());
        assert_eq!(hw.epochs[1].epochs_closed(), 0);
    }
}

#[test]
fn telemetry_is_usable_in_every_build() {
    let stats: &'static Stats = Box::leak(Box::default());
    let mut t = Telemetry::new(stats, 5.0);
    t.record_loss(
        dpds::first_loss::FirstLossReason::DpdsReject,
        dpds::first_loss::StageLossDetail::Single {
            from: dpds::first_loss::Stage::Dpds,
        },
        10.0,
    );
    t.tick(1.0, 1);
    t.tick(1.0e12, 0);
    let _ = t.stats; // pub field: the shared Stats
}

#[cfg(feature = "telemetry")]
mod with_feature {
    use std::collections::BTreeSet;

    use dpds::counters::{mean_2dp, DpdsCounters, EPOCH_CAPACITY, EPOCH_IDLE_GAP_NS};
    use dpds::first_loss::{FirstLossReason, Stage, StageLossDetail};
    use dpds::reply::{Reply, Value};
    use dpds::stats::{ports_reply, DirectionInfo, PortInfo};

    use super::*;

    fn get<'a>(r: &'a Reply, key: &str) -> &'a Value {
        let hits: Vec<_> = r.iter().filter(|(k, _)| k == key).collect();
        assert_eq!(
            hits.len(),
            1,
            "key {key:?} must appear exactly once in {r:?}"
        );
        &hits[0].1
    }
    fn uint(r: &Reply, key: &str) -> u64 {
        match get(r, key) {
            Value::Uint(v) => *v,
            other => panic!("{key}: expected Uint, got {other:?}"),
        }
    }
    fn string<'a>(r: &'a Reply, key: &str) -> &'a str {
        match get(r, key) {
            Value::Str(v) => v,
            other => panic!("{key}: expected Str, got {other:?}"),
        }
    }
    fn uints(r: &Reply, key: &str) -> Vec<u64> {
        match get(r, key) {
            Value::UintArray(v) => v.clone(),
            other => panic!("{key}: expected UintArray, got {other:?}"),
        }
    }
    fn strings(r: &Reply, key: &str) -> Vec<String> {
        match get(r, key) {
            Value::StrArray(v) => v.clone(),
            other => panic!("{key}: expected StrArray, got {other:?}"),
        }
    }
    fn keys(r: &Reply) -> BTreeSet<&str> {
        r.iter().map(|(k, _)| k.as_str()).collect()
    }
    fn set(names: &[&'static str]) -> BTreeSet<&'static str> {
        names.iter().copied().collect()
    }

    const GAP: f64 = EPOCH_IDLE_GAP_NS;

    // ── Telemetry: loss recording ────────────────────────────────────

    #[test]
    fn record_loss_stamps_the_time_relative_to_the_start_and_latches_the_first() {
        let stats: &'static Stats = Box::leak(Box::default());
        let mut t = Telemetry::new(stats, 1_000.0);
        t.record_loss(
            FirstLossReason::DpdsReject,
            StageLossDetail::Single { from: Stage::Dpds },
            5_000.0,
        );
        t.record_loss(
            FirstLossReason::DescriptorFull,
            StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic,
            },
            9_000.0,
        );
        let ev = stats.first_loss.current().unwrap();
        assert_eq!(ev.reason, FirstLossReason::DpdsReject, "first one wins");
        assert_eq!(ev.relative_ns, 4_000.0);
    }

    // ── Telemetry::tick: epoch closing ───────────────────────────────

    #[test]
    fn tick_with_arrivals_never_closes_and_a_short_idle_gap_does_not_either() {
        let stats: &'static Stats = Box::leak(Box::default());
        let mut t = Telemetry::new(stats, 0.0);
        t.tick(100.0, 5);
        t.tick(100.0 + GAP - 1.0, 0);
        t.tick(100.0 + GAP, 0); // exactly the gap: strictly greater is required
        assert_eq!(stats.epochs.epochs_closed(), 0);
    }

    #[test]
    fn a_gap_past_500ms_closes_an_epoch_with_deltas_gauges_and_the_epochs_first_loss() {
        let stats: &'static Stats = Box::leak(Box::default());
        let mut t = Telemetry::new(stats, 1_000.0);
        let c = &stats.counters;

        c.polls.add(10);
        c.rx_pkts.add(7);
        c.tx_batch_max.observe(5);
        c.buffer_len_max.observe(9);
        t.record_loss(
            FirstLossReason::DpdsReject,
            StageLossDetail::Single { from: Stage::Dpds },
            2_000.0,
        );
        t.tick(3_000.0, 7);
        let close_at = 3_000.0 + GAP + 1.0;
        t.tick(close_at, 0);

        assert_eq!(stats.epochs.epochs_closed(), 1);
        let r = stats.epochs.record(0).unwrap();
        // epoch timestamps are uptime: ns since the telemetry start (like
        // `uptime_ns` of /dpds/stats), NOT the raw now_ns() clock
        assert_eq!(r.start_uptime_ns, 0, "the first epoch starts at the start");
        assert_eq!(r.end_uptime_ns, (close_at - 1_000.0) as u64);
        assert_eq!(r.counters, [10, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(r.gauges, [5, 9], "tx_batch_max, buffer_len_max");
        let loss = stats
            .epochs
            .first_loss(0)
            .unwrap()
            .expect("epoch had a loss");
        assert_eq!(loss.reason, FirstLossReason::DpdsReject);
        assert_eq!(loss.relative_ns, 1_000.0);

        // gauges were taken (reset); the whole-process latch is untouched
        assert_eq!(c.tx_batch_max.get(), 0);
        assert_eq!(c.buffer_len_max.get(), 0);
        assert!(stats.first_loss.current().is_some());
    }

    #[test]
    fn the_next_epoch_is_contiguous_has_delta_counters_and_a_fresh_first_loss() {
        let stats: &'static Stats = Box::leak(Box::default());
        let mut t = Telemetry::new(stats, 0.0);
        let c = &stats.counters;

        c.rx_pkts.add(4);
        t.record_loss(
            FirstLossReason::DpdsReject,
            StageLossDetail::Single { from: Stage::Dpds },
            50.0,
        );
        t.tick(100.0, 4);
        let close1 = 100.0 + GAP + 1.0;
        t.tick(close1, 0);

        // second trial: clean
        c.rx_pkts.add(6);
        t.tick(close1 + 10.0, 6);
        let close2 = close1 + 10.0 + GAP + 1.0;
        t.tick(close2, 0);

        assert_eq!(stats.epochs.epochs_closed(), 2);
        let (r0, r1) = (
            stats.epochs.record(0).unwrap(),
            stats.epochs.record(1).unwrap(),
        );
        assert_eq!(r1.start_uptime_ns, r0.end_uptime_ns, "contiguous");
        assert_eq!(r0.counters[1], 4);
        assert_eq!(r1.counters[1], 6, "a delta, not the cumulative 10");
        assert!(stats.epochs.first_loss(0).unwrap().is_some());
        assert_eq!(
            stats.epochs.first_loss(1),
            Some(None),
            "the scratch was reset: epoch 2 is Clean"
        );
    }

    #[test]
    fn idle_ticks_before_any_traffic_and_after_a_close_emit_nothing() {
        let stats: &'static Stats = Box::leak(Box::default());
        let mut t = Telemetry::new(stats, 0.0);
        for i in 1..=5 {
            t.tick(i as f64 * 3.0 * GAP, 0);
        }
        assert_eq!(stats.epochs.epochs_closed(), 0);
        t.tick(20.0 * GAP, 1);
        for i in 1..=5 {
            t.tick(20.0 * GAP + i as f64 * 3.0 * GAP, 0);
        }
        assert_eq!(
            stats.epochs.epochs_closed(),
            1,
            "one close, not a run of empty epochs"
        );
    }

    // ── /dpds/stats ──────────────────────────────────────────────────

    const STATS_KEYS: [&str; 28] = [
        "uptime_ns",
        "polls",
        "rx_pkts",
        "rx_full_bursts",
        "dpds_rejects",
        "loss_drops",
        "limiter_drops",
        "buffer_full_drops",
        "pops",
        "tx_calls",
        "tx_pkts",
        "tx_sent",
        "tx_drops",
        "tx_short_calls",
        "tx_batch_max",
        "tx_pkts_per_call",
        "buffer_len_max",
        "frame_sizes",
        "out_of_order",
        "first_loss_reason",
        "first_loss_detail",
        "first_loss_relative_ns",
        "delay_mean_ns",
        "delay_std_ns",
        "delay_min_ns",
        "delay_max_ns",
        "lateness_mean_ns",
        "lateness_max_ns",
    ];

    #[test]
    fn stats_reply_has_exactly_the_documented_fields_and_value_types() {
        let stats = Stats::default();
        let r = stats.stats_reply(123);
        assert_eq!(keys(&r), set(&STATS_KEYS));
        assert_eq!(r.len(), STATS_KEYS.len(), "no duplicate keys");
        for k in STATS_KEYS {
            match k {
                "tx_pkts_per_call" | "first_loss_reason" | "first_loss_detail"
                | "delay_mean_ns" | "delay_std_ns" | "lateness_mean_ns" => {
                    assert!(matches!(get(&r, k), Value::Str(_)), "{k} is a string")
                }
                "frame_sizes" => assert!(
                    matches!(get(&r, k), Value::UintArray(a) if a.len() == 8),
                    "{k} is an array of 8"
                ),
                _ => assert!(matches!(get(&r, k), Value::Uint(_)), "{k} is a uint"),
            }
        }
        assert_eq!(uint(&r, "uptime_ns"), 123);
    }

    #[test]
    fn a_fresh_stats_reply_is_all_zero_and_clean() {
        let r = Stats::default().stats_reply(0);
        for k in STATS_KEYS {
            match k {
                "tx_pkts_per_call" | "delay_mean_ns" | "delay_std_ns" | "lateness_mean_ns" => {
                    assert_eq!(string(&r, k), "0.00", "{k}")
                }
                "first_loss_reason" => assert_eq!(string(&r, k), "Clean"),
                "first_loss_detail" => assert_eq!(string(&r, k), ""),
                "frame_sizes" => assert_eq!(get(&r, k), &Value::UintArray(vec![0; 8])),
                _ => assert_eq!(uint(&r, k), 0, "{k}"),
            }
        }
    }

    #[test]
    fn stats_reply_reports_the_cumulative_counters() {
        let stats = Stats::default();
        let c = &stats.counters;
        c.polls.add(1_000);
        c.rx_pkts.add(500);
        c.rx_full_bursts.add(11);
        c.dpds_rejects.add(12);
        c.pops.add(488);
        c.tx_calls.add(4);
        c.tx_pkts.add(10);
        c.tx_sent.add(8);
        c.tx_short_calls.add(2);
        let r = stats.stats_reply(99);
        assert_eq!(uint(&r, "polls"), 1_000);
        assert_eq!(uint(&r, "rx_pkts"), 500);
        assert_eq!(uint(&r, "rx_full_bursts"), 11);
        assert_eq!(uint(&r, "dpds_rejects"), 12);
        assert_eq!(uint(&r, "pops"), 488);
        assert_eq!(uint(&r, "tx_calls"), 4);
        assert_eq!(uint(&r, "tx_pkts"), 10);
        assert_eq!(uint(&r, "tx_sent"), 8);
        assert_eq!(uint(&r, "tx_short_calls"), 2);
    }

    #[test]
    fn tx_drops_is_derived_from_tx_pkts_and_tx_sent_and_saturates() {
        let stats = Stats::default();
        stats.counters.tx_pkts.add(10);
        stats.counters.tx_sent.add(7);
        assert_eq!(uint(&stats.stats_reply(0), "tx_drops"), 3);

        // the telemetry thread may read tx_sent a moment after tx_pkts
        let stats = Stats::default();
        stats.counters.tx_pkts.add(3);
        stats.counters.tx_sent.add(5);
        assert_eq!(uint(&stats.stats_reply(0), "tx_drops"), 0);
    }

    #[test]
    fn tx_pkts_per_call_is_the_two_decimal_mean_and_zero_without_calls() {
        let stats = Stats::default();
        stats.counters.tx_pkts.add(10);
        stats.counters.tx_calls.add(4);
        assert_eq!(string(&stats.stats_reply(0), "tx_pkts_per_call"), "2.50");
        assert_eq!(mean_2dp(10, 4), "2.50");

        let stats = Stats::default();
        stats.counters.tx_pkts.add(5); // no call counted (cannot happen, but no div by zero)
        assert_eq!(string(&stats.stats_reply(0), "tx_pkts_per_call"), "0.00");
    }

    #[test]
    fn stats_reply_gauges_are_the_max_over_closed_epochs_and_the_live_value() {
        let stats = Stats::default();
        stats.epochs.close([0; 12], [16, 40], 0, 100, None);
        stats.epochs.close([0; 12], [3, 5], 100, 200, None);
        stats.counters.tx_batch_max.observe(8);
        stats.counters.buffer_len_max.observe(2);
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "tx_batch_max"), 16);
        assert_eq!(uint(&r, "buffer_len_max"), 40);

        stats.counters.tx_batch_max.observe(64);
        assert_eq!(
            uint(&stats.stats_reply(0), "tx_batch_max"),
            64,
            "a larger live value wins"
        );
    }

    #[test]
    fn stats_reply_gauges_survive_epochs_dropped_past_the_capacity() {
        let stats = Stats::default();
        for i in 0..EPOCH_CAPACITY as u64 {
            stats
                .epochs
                .close([0; 12], [1, 1], i * 10, i * 10 + 5, None);
        }
        // the 33rd epoch is dropped, but its peak must still show up
        stats.epochs.close([0; 12], [128, 999], 1_000, 1_005, None);
        assert_eq!(stats.epochs.epochs_dropped(), 1);
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "tx_batch_max"), 128);
        assert_eq!(uint(&r, "buffer_len_max"), 999);
    }

    #[test]
    fn stats_reply_first_loss_is_the_whole_process_first_one() {
        let stats = Stats::default();
        stats.first_loss.try_record(
            FirstLossReason::DescriptorFull,
            StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic,
            },
            12_345.9,
        );
        // later events and a closed clean epoch do not change it
        stats.first_loss.try_record(
            FirstLossReason::DpdsReject,
            StageLossDetail::Single { from: Stage::Dpds },
            1.0,
        );
        stats.epochs.close([0; 12], [0, 0], 0, 1, None);
        let r = stats.stats_reply(0);
        assert_eq!(string(&r, "first_loss_reason"), "DescriptorFull");
        assert_eq!(string(&r, "first_loss_detail"), "from=dpds,to=nic");
        assert_eq!(uint(&r, "first_loss_relative_ns"), 12_345);
    }

    #[test]
    fn stats_reply_of_a_dpds_reject() {
        let stats = Stats::default();
        stats.first_loss.try_record(
            FirstLossReason::DpdsReject,
            StageLossDetail::Single { from: Stage::Dpds },
            7.0,
        );
        let r = stats.stats_reply(0);
        assert_eq!(string(&r, "first_loss_reason"), "DpdsReject");
        assert_eq!(string(&r, "first_loss_detail"), "from=dpds");
    }

    // ── /dpds/epochs ─────────────────────────────────────────────────

    const EPOCH_KEYS: [&str; 21] = [
        "epochs_closed",
        "epochs_dropped",
        "epoch_start_uptime_ns",
        "epoch_end_uptime_ns",
        "first_loss_reason",
        "first_loss_detail",
        "first_loss_relative_ns",
        // counters
        "polls",
        "rx_pkts",
        "rx_full_bursts",
        "dpds_rejects",
        "pops",
        "tx_calls",
        "tx_pkts",
        "tx_sent",
        "tx_short_calls",
        "loss_drops",
        "limiter_drops",
        "buffer_full_drops",
        // gauges
        "tx_batch_max",
        // + buffer_len_max below
        "buffer_len_max",
    ];

    #[test]
    fn epochs_reply_of_a_fresh_stats_has_the_wire_shape_with_empty_arrays() {
        let r = Stats::default().epochs_reply();
        assert_eq!(keys(&r), set(&EPOCH_KEYS));
        assert_eq!(uint(&r, "epochs_closed"), 0);
        assert_eq!(uint(&r, "epochs_dropped"), 0);
        for k in [
            "epoch_start_uptime_ns",
            "epoch_end_uptime_ns",
            "first_loss_relative_ns",
        ] {
            assert_eq!(uints(&r, k), Vec::<u64>::new(), "{k}");
        }
        for k in ["first_loss_reason", "first_loss_detail"] {
            assert_eq!(strings(&r, k), Vec::<String>::new(), "{k}");
        }
        for k in DpdsCounters::EPOCH_COUNTER_NAMES
            .iter()
            .chain(DpdsCounters::EPOCH_GAUGE_NAMES.iter())
        {
            assert_eq!(uints(&r, k), Vec::<u64>::new(), "{k}");
        }
    }

    #[test]
    fn epochs_reply_is_columnar_one_entry_per_closed_epoch_in_every_array() {
        use dpds::first_loss::StageLossEvent;
        let stats = Stats::default();
        stats.epochs.close(
            [10, 4, 0, 4, 0, 0, 0, 0, 0, 3, 1, 0],
            [0, 7],
            1_000,
            2_000,
            Some(StageLossEvent {
                reason: FirstLossReason::DpdsReject,
                detail: StageLossDetail::Single { from: Stage::Dpds },
                relative_ns: 1_500.7,
            }),
        );
        stats.epochs.close(
            [25, 9, 0, 6, 0, 5, 0, 5, 0, 3, 1, 2],
            [3, 2],
            2_000,
            3_500,
            None,
        );
        let r = stats.epochs_reply();

        assert_eq!(uint(&r, "epochs_closed"), 2);
        assert_eq!(uint(&r, "epochs_dropped"), 0);
        assert_eq!(uints(&r, "epoch_start_uptime_ns"), [1_000, 2_000]);
        assert_eq!(uints(&r, "epoch_end_uptime_ns"), [2_000, 3_500]);
        assert_eq!(strings(&r, "first_loss_reason"), ["DpdsReject", "Clean"]);
        assert_eq!(strings(&r, "first_loss_detail"), ["from=dpds", ""]);
        assert_eq!(uints(&r, "first_loss_relative_ns"), [1_500, 0]);
        // counters are per-epoch DELTAS, named by EPOCH_COUNTER_NAMES order
        assert_eq!(uints(&r, "polls"), [10, 15]);
        assert_eq!(uints(&r, "rx_pkts"), [4, 5]);
        assert_eq!(uints(&r, "rx_full_bursts"), [0, 0]);
        assert_eq!(uints(&r, "dpds_rejects"), [4, 2]);
        assert_eq!(uints(&r, "pops"), [0, 0]);
        assert_eq!(uints(&r, "tx_calls"), [0, 5]);
        assert_eq!(uints(&r, "tx_pkts"), [0, 0]);
        assert_eq!(uints(&r, "tx_sent"), [0, 5]);
        assert_eq!(uints(&r, "tx_short_calls"), [0, 0]);
        // the drop causes are per-epoch deltas too (epoch 1: 4 rejects = 3 + 1 + 0, epoch 2: 2 = 0 + 0 + 2)
        assert_eq!(uints(&r, "loss_drops"), [3, 0]);
        assert_eq!(uints(&r, "limiter_drops"), [1, 0]);
        assert_eq!(uints(&r, "buffer_full_drops"), [0, 2]);
        // gauges are per-epoch maxima
        assert_eq!(uints(&r, "tx_batch_max"), [0, 3]);
        assert_eq!(uints(&r, "buffer_len_max"), [7, 2]);
    }

    #[test]
    fn epochs_reply_keeps_the_oldest_32_and_counts_the_dropped() {
        let stats = Stats::default();
        for i in 0..(EPOCH_CAPACITY as u64 + 3) {
            stats.epochs.close(
                [i + 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0],
                i * 10,
                i * 10 + 5,
                None,
            );
        }
        let r = stats.epochs_reply();
        assert_eq!(uint(&r, "epochs_closed"), EPOCH_CAPACITY as u64);
        assert_eq!(uint(&r, "epochs_dropped"), 3);
        for k in ["epoch_start_uptime_ns", "polls", "tx_batch_max"] {
            assert_eq!(uints(&r, k).len(), EPOCH_CAPACITY, "{k}");
        }
        assert_eq!(strings(&r, "first_loss_reason").len(), EPOCH_CAPACITY);
        assert_eq!(
            uints(&r, "epoch_start_uptime_ns")[0],
            0,
            "the oldest is kept"
        );
    }

    // ── /dpds/stats,<dir> and /dpds/epochs,<dir> ────────────────────

    #[test]
    fn direction_stats_has_one_independent_stats_per_direction() {
        let ds = DirectionStats::new(2);
        assert_eq!(ds.stats.len(), 2);
        assert_eq!(DirectionStats::new(1).stats.len(), 1);
        ds.stats[1].counters.rx_pkts.add(7);
        assert_eq!(ds.stats[0].counters.rx_pkts.get(), 0);
        assert_eq!(ds.stats[1].counters.rx_pkts.get(), 7);
    }

    #[test]
    fn the_stats_reply_of_a_direction_is_that_directions_stats_reply() {
        let ds = DirectionStats::new(2);
        ds.stats[0].counters.rx_pkts.add(3);
        ds.stats[1].counters.rx_pkts.add(11);
        ds.stats[1].counters.tx_sent.add(5);
        for (param, rx, sent) in [("0", 3, 0), ("1", 11, 5)] {
            let r = ds.stats_reply(param, 99).unwrap();
            assert_eq!(uint(&r, "rx_pkts"), rx, "dir {param}");
            assert_eq!(uint(&r, "tx_sent"), sent, "dir {param}");
            assert_eq!(uint(&r, "uptime_ns"), 99);
            // same 26 keys as the per-direction Stats builder
            let direct = ds.stats[param.parse::<usize>().unwrap()].stats_reply(99);
            assert_eq!(r, direct);
            assert_eq!(r.len(), 28);
        }
    }

    #[test]
    fn the_epochs_reply_of_a_direction_is_that_directions_epochs_reply() {
        let ds = DirectionStats::new(2);
        ds.stats[1].epochs.close([1; 12], [4, 5], 10, 20, None);
        let r1 = ds.epochs_reply("1").unwrap();
        assert_eq!(r1, ds.stats[1].epochs_reply());
        assert_eq!(uint(&r1, "epochs_closed"), 1);
        let r0 = ds.epochs_reply("0").unwrap();
        assert_eq!(uint(&r0, "epochs_closed"), 0, "directions are independent");
    }

    #[test]
    fn direction_replies_reject_a_missing_or_invalid_direction() {
        let ds = DirectionStats::new(2);
        for bad in [
            "2",
            "",
            "x",
            "-1",
            "+1",
            "0,1",
            " 0",
            "1 ",
            "0x1",
            "99999999999999999999",
        ] {
            assert!(ds.stats_reply(bad, 0).is_none(), "{bad:?}");
            assert!(ds.epochs_reply(bad).is_none(), "{bad:?}");
        }
        let one = DirectionStats::new(1);
        assert!(one.stats_reply("0", 0).is_some());
        assert!(one.epochs_reply("0").is_some());
        assert!(
            one.stats_reply("1", 0).is_none(),
            "only one direction exists"
        );
        assert!(one.epochs_reply("1").is_none());
    }

    // ── /dpds/ports ──────────────────────────────────────────────────

    fn port(name: &str, rx: u16, tx: u16, rxr: u16, txr: u16) -> PortInfo {
        PortInfo {
            name: name.to_string(),
            rx_ring: rx,
            tx_ring: tx,
            rx_ring_requested: rxr,
            tx_ring_requested: txr,
        }
    }

    fn dir(rx: u16, tx: u16, lcore: u32, config: &str, pool: u32, raw: bool) -> DirectionInfo {
        DirectionInfo {
            rx_port: rx,
            tx_port: tx,
            lcore,
            config: config.to_string(),
            pool_size: pool,
            raw_free: raw,
            rx_name: format!("rx{rx}"),
            tx_name: format!("tx{tx}"),
            delay_stats: true,
        }
    }

    #[test]
    fn ports_reply_for_two_ports_and_two_directions() {
        let r = ports_reply(
            &[
                port("0000:65:00.1", 8192, 4096, 4096, 32768),
                port("net_pcap1", 1024, 512, 4096, 4095),
            ],
            &[
                dir(0, 1, 1, "/etc/d0.json", 1_048_575, true),
                dir(1, 0, 2, "d1.json", 2_000, false),
            ],
            2,
        );
        assert_eq!(uint(&r, "num_ports"), 2);
        assert_eq!(string(&r, "port0_name"), "0000:65:00.1");
        assert_eq!(uint(&r, "port0_rx_ring"), 8192);
        assert_eq!(uint(&r, "port0_tx_ring"), 4096);
        assert_eq!(uint(&r, "port0_rx_ring_requested"), 4096);
        assert_eq!(uint(&r, "port0_tx_ring_requested"), 32768);
        assert_eq!(string(&r, "port1_name"), "net_pcap1");
        assert_eq!(uint(&r, "port1_rx_ring"), 1024);
        assert_eq!(uint(&r, "port1_tx_ring"), 512);
        assert_eq!(uint(&r, "port1_rx_ring_requested"), 4096);
        assert_eq!(uint(&r, "port1_tx_ring_requested"), 4095);

        // a link is one direction: `num_links` equals `num_directions`
        assert_eq!(uint(&r, "num_directions"), 2);
        assert_eq!(uint(&r, "num_links"), 2);
        assert_eq!(string(&r, "dir0_rx_name"), "rx0");
        assert_eq!(string(&r, "dir0_tx_name"), "tx1");
        assert_eq!(string(&r, "dir1_rx_name"), "rx1");
        assert_eq!(string(&r, "dir1_tx_name"), "tx0");
        assert_eq!(uint(&r, "dir0_rx_port"), 0);
        assert_eq!(uint(&r, "dir0_tx_port"), 1);
        assert_eq!(uint(&r, "dir0_lcore"), 1);
        assert_eq!(string(&r, "dir0_config"), "/etc/d0.json");
        assert_eq!(uint(&r, "dir0_pool_size"), 1_048_575);
        assert_eq!(uint(&r, "dir0_raw_free"), 1);
        assert_eq!(uint(&r, "dir1_rx_port"), 1);
        assert_eq!(uint(&r, "dir1_tx_port"), 0);
        assert_eq!(uint(&r, "dir1_lcore"), 2);
        assert_eq!(string(&r, "dir1_config"), "d1.json");
        assert_eq!(uint(&r, "dir1_pool_size"), 2_000);
        assert_eq!(uint(&r, "dir1_raw_free"), 0);
        assert_eq!(
            r.len(),
            1 + 2 * 5 + 2 + 2 * 9,
            "num_ports + five fields per port + num_directions + num_links + nine per direction"
        );
    }

    #[test]
    fn ports_reply_keys_come_in_the_documented_order() {
        let r = ports_reply(
            &[port("a", 1, 1, 1, 1)],
            &[dir(0, 0, 1, "c.json", 9, false)],
            1,
        );
        let order: Vec<&str> = r.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            order,
            [
                "num_ports",
                "port0_name",
                "port0_rx_ring",
                "port0_tx_ring",
                "port0_rx_ring_requested",
                "port0_tx_ring_requested",
                "num_directions",
                "num_links",
                "dir0_rx_port",
                "dir0_tx_port",
                "dir0_lcore",
                "dir0_config",
                "dir0_pool_size",
                "dir0_raw_free",
                "dir0_rx_name",
                "dir0_tx_name",
                "dir0_delay_stats",
            ]
        );
    }

    #[test]
    fn ports_reply_for_one_port_has_no_port1_or_dir1_fields() {
        let r = ports_reply(
            &[port("net_null0", 256, 256, 4096, 4096)],
            &[dir(0, 0, 1, "config.json", 64, false)],
            1,
        );
        assert_eq!(uint(&r, "num_ports"), 1);
        assert_eq!(string(&r, "port0_name"), "net_null0");
        assert_eq!(uint(&r, "num_directions"), 1);
        assert_eq!(uint(&r, "dir0_rx_port"), 0);
        assert_eq!(uint(&r, "dir0_tx_port"), 0, "one port: loopback");
        assert!(keys(&r)
            .iter()
            .all(|k| !k.starts_with("port1_") && !k.starts_with("dir1_")));
        assert_eq!(r.len(), 1 + 5 + 2 + 9);
    }

    #[test]
    fn ports_reply_keys_are_short_enough_for_dpdk() {
        // rte_tel_data dict keys are limited to RTE_TEL_MAX_STRING_LEN (64)
        let r = ports_reply(
            &[port("x", 1, 1, 1, 1), port("y", 1, 1, 1, 1)],
            &[dir(0, 1, 1, "a", 1, true), dir(1, 0, 2, "b", 1, true)],
            2,
        );
        assert!(r.iter().all(|(k, _)| k.len() < 64));
    }

    #[test]
    fn ports_reply_of_a_ring_names_the_devices_per_link() {
        // ring A -> B, B -> C, C -> A over port ids 2, 0, 1 (ids are not in link
        // order); every link is one direction with its own ingress/egress.
        let mk = |rx, tx, rxn: &str, txn: &str| DirectionInfo {
            rx_name: rxn.into(),
            tx_name: txn.into(),
            ..dir(rx, tx, 1 + rx as u32, "links.json", 7, false)
        };
        let r = ports_reply(
            &[
                port("net_pcap0", 1, 1, 1, 1),
                port("net_pcap1", 1, 1, 1, 1),
                port("net_null2", 1, 1, 1, 1),
            ],
            &[
                mk(2, 0, "net_null2", "net_pcap0"),
                mk(0, 1, "net_pcap0", "net_pcap1"),
                mk(1, 2, "net_pcap1", "net_null2"),
            ],
            3,
        );
        assert_eq!(uint(&r, "num_ports"), 3);
        assert_eq!(uint(&r, "num_directions"), 3);
        assert_eq!(uint(&r, "num_links"), 3, "one direction per link");
        for (d, rxn, txn) in [
            (0, "net_null2", "net_pcap0"),
            (1, "net_pcap0", "net_pcap1"),
            (2, "net_pcap1", "net_null2"),
        ] {
            assert_eq!(string(&r, &format!("dir{d}_rx_name")), rxn);
            assert_eq!(string(&r, &format!("dir{d}_tx_name")), txn);
        }
        assert_eq!(
            uint(&r, "dir0_rx_port"),
            2,
            "the link's ingress, not the port-id order"
        );
        assert_eq!(uint(&r, "dir0_tx_port"), 0);
        assert_eq!(uint(&r, "dir2_tx_port"), 2);
        assert_eq!(r.len(), 3 + 3 * 5 + 3 * 9);
        assert!(r.iter().all(|(k, _)| k.len() < 64));
    }

    #[test]
    fn ports_reply_of_one_way_links_has_more_ports_than_links() {
        // two links a -> b and c -> d over four ports: 3 + 5 * 4 + 9 * 2 keys
        let r = ports_reply(
            &[
                port("a", 1, 1, 1, 1),
                port("b", 1, 1, 1, 1),
                port("c", 1, 1, 1, 1),
                port("d", 1, 1, 1, 1),
            ],
            &[
                dir(0, 1, 1, "links.json", 1, false),
                dir(2, 3, 2, "links.json", 1, false),
            ],
            2,
        );
        assert_eq!(uint(&r, "num_ports"), 4);
        assert_eq!(uint(&r, "num_directions"), 2);
        assert_eq!(uint(&r, "num_links"), 2);
        assert_eq!(r.len(), 3 + 5 * 4 + 9 * 2);
    }

    #[test]
    fn a_long_config_path_stays_a_single_string_value() {
        // RTE_TEL_MAX_STRING_LEN is 128 (rte_telemetry.h): what the dict does
        // with a longer string is not specified here; the reply builder
        // itself passes the path through unchanged.
        let path = "/some/long/directory/name/for/the/dpds/configs/direction-zero.json";
        let r = ports_reply(&[port("x", 1, 1, 1, 1)], &[dir(0, 0, 1, path, 1, false)], 1);
        assert_eq!(string(&r, "dir0_config"), path);
    }

    // ── dir<d>_delay_stats and the dict limit ──────────────

    #[test]
    fn ports_reply_reports_the_delay_stats_switch_per_direction() {
        let off = DirectionInfo {
            delay_stats: false,
            ..dir(1, 0, 2, "c.json", 1, false)
        };
        let r = ports_reply(
            &[port("a", 1, 1, 1, 1), port("b", 1, 1, 1, 1)],
            &[dir(0, 1, 1, "c.json", 1, false), off],
            2,
        );
        assert_eq!(uint(&r, "dir0_delay_stats"), 1);
        assert_eq!(uint(&r, "dir1_delay_stats"), 0);
        assert!(r.iter().all(|(k, _)| k.len() < 64));
    }

    #[test]
    fn ports_reply_has_three_plus_five_per_port_plus_nine_per_direction_keys() {
        // RTE_TEL_MAX_DICT_ENTRIES is 256: this is the formula `--links`
        // validation (tests/unit/links.rs, "dict limit") must use: 3 + 5 per
        // distinct port + 9 per link. In a ring (one link per port) 18 ports
        // fit (255 keys) and 19 do not (269).
        for (n_ports, fits) in [(1usize, true), (2, true), (18, true), (19, false)] {
            let ports: Vec<PortInfo> = (0..n_ports)
                .map(|i| port(&format!("p{i}"), 1, 1, 1, 1))
                .collect();
            let dirs: Vec<DirectionInfo> = (0..n_ports as u16)
                .map(|i| dir(i, i, 1 + i as u32, "links.json", 1, false))
                .collect();
            let r = ports_reply(&ports, &dirs, n_ports);
            assert_eq!(r.len(), 3 + 5 * n_ports + 9 * n_ports, "{n_ports} ports");
            assert_eq!(r.len() <= 256, fits, "{n_ports} ports: {} keys", r.len());
        }
    }

    // ── /nic/hw_first_loss and /nic/epochs ──────────────────────────

    #[test]
    fn nic_hw_first_loss_reply_is_clean_until_the_main_lcore_records_one() {
        let hw = NicHwStats::new(2);
        for p in ["0", "1"] {
            let r = hw.hw_first_loss_reply(p).unwrap();
            assert_eq!(
                keys(&r),
                set(&[
                    "hw_first_loss_reason",
                    "hw_first_loss_detail",
                    "hw_first_loss_relative_ns"
                ])
            );
            assert_eq!(string(&r, "hw_first_loss_reason"), "Clean");
            assert_eq!(string(&r, "hw_first_loss_detail"), "");
            assert_eq!(uint(&r, "hw_first_loss_relative_ns"), 0);
        }
        hw.first_loss[1].try_record(
            FirstLossReason::RxMissed,
            StageLossDetail::HardwareDrop {
                from: Stage::Dpds,
                count: 42,
            },
            250_000_000.0,
        );
        let r1 = hw.hw_first_loss_reply("1").unwrap();
        assert_eq!(string(&r1, "hw_first_loss_reason"), "IMissed");
        assert_eq!(string(&r1, "hw_first_loss_detail"), "from=dpds,count=42");
        assert_eq!(uint(&r1, "hw_first_loss_relative_ns"), 250_000_000);
        let r0 = hw.hw_first_loss_reply("0").unwrap();
        assert_eq!(
            string(&r0, "hw_first_loss_reason"),
            "Clean",
            "ports are independent"
        );
    }

    #[test]
    fn nic_replies_reject_bad_ports() {
        let hw = NicHwStats::new(2);
        for bad in ["2", "", "x", "-1", "0,1", " 0"] {
            assert!(hw.hw_first_loss_reply(bad).is_none(), "{bad:?}");
            assert!(hw.epochs_reply(bad).is_none(), "{bad:?}");
        }
        let one = NicHwStats::new(1);
        assert!(one.hw_first_loss_reply("0").is_some());
        assert!(
            one.hw_first_loss_reply("1").is_none(),
            "only one port is polled"
        );
        assert!(one.epochs_reply("1").is_none());
    }

    #[test]
    fn nic_epochs_reply_has_the_epoch_wire_shape_with_the_nic_columns() {
        let hw = NicHwStats::new(2);
        hw.epochs[0].close([1_000, 3, 20], [], 10, 20, None);
        hw.epochs[0].close([1_500, 3, 25], [], 20, 30, None);
        let r = hw.epochs_reply("0").unwrap();
        assert_eq!(
            keys(&r),
            set(&[
                "epochs_closed",
                "epochs_dropped",
                "epoch_start_uptime_ns",
                "epoch_end_uptime_ns",
                "first_loss_reason",
                "first_loss_detail",
                "first_loss_relative_ns",
                "rx_packets",
                "imissed",
                "rx_port_discards",
            ])
        );
        assert_eq!(uint(&r, "epochs_closed"), 2);
        assert_eq!(uints(&r, "rx_packets"), [1_000, 500]);
        assert_eq!(uints(&r, "imissed"), [3, 0]);
        assert_eq!(uints(&r, "rx_port_discards"), [20, 5]);
        assert_eq!(
            strings(&r, "first_loss_reason"),
            ["Clean", "Clean"],
            "a NIC port has no software tracker"
        );
        assert_eq!(uint(&hw.epochs_reply("1").unwrap(), "epochs_closed"), 0);
    }

    // ── NicPoller: the main lcore's 250 ms poll ──────────────────────

    use dpdk::port::PortStats;

    fn ps(ipackets: u64, imissed: u64, rx_nombuf: u64) -> PortStats {
        PortStats {
            ipackets,
            imissed,
            rx_nombuf,
            ..Default::default()
        }
    }

    #[test]
    fn a_rise_in_a_hardware_counter_is_latched_per_port_with_time_relative_to_start() {
        let hw = NicHwStats::new(2);
        let mut p0 = NicPoller::new(0, 1_000.0, &ps(0, 0, 0), 0);
        let mut p1 = NicPoller::new(1, 1_000.0, &ps(0, 0, 0), 0);

        p0.poll(1_250.0, &ps(0, 0, 0), &hw, || None);
        assert!(hw.first_loss[0].current().is_none(), "no rise, no loss");

        p0.poll(1_500.0, &ps(0, 5, 0), &hw, || None);
        let ev = hw.first_loss[0].current().expect("imissed rose");
        assert_eq!(ev.reason, FirstLossReason::RxMissed);
        assert_eq!(
            ev.detail,
            StageLossDetail::HardwareDrop {
                from: Stage::Dpds,
                count: 5
            }
        );
        assert_eq!(ev.relative_ns, 500.0, "now (1500) - start (1000)");

        // a later, different rise does not overwrite the first
        p0.poll(1_750.0, &ps(0, 5, 9), &hw, || None);
        assert_eq!(
            hw.first_loss[0].current().unwrap().reason,
            FirstLossReason::RxMissed
        );

        // the other port is untouched
        p1.poll(1_750.0, &ps(0, 0, 0), &hw, || None);
        assert!(hw.first_loss[1].current().is_none());
    }

    #[test]
    fn the_rise_is_measured_against_the_previous_poll_not_the_first() {
        let hw = NicHwStats::new(1);
        let mut p = NicPoller::new(0, 0.0, &ps(0, 100, 0), 0);
        p.poll(250.0, &ps(0, 100, 0), &hw, || None);
        assert!(
            hw.first_loss[0].current().is_none(),
            "initial imissed 100 is the baseline"
        );
        p.poll(500.0, &ps(0, 130, 0), &hw, || None);
        assert_eq!(
            hw.first_loss[0].current().unwrap().detail,
            StageLossDetail::HardwareDrop {
                from: Stage::Dpds,
                count: 30
            }
        );
    }

    #[test]
    fn an_idle_gap_after_rx_traffic_closes_a_nic_epoch_with_deltas_since_the_baseline() {
        let hw = NicHwStats::new(1);
        let reads = std::cell::Cell::new(0u32);
        // baselines: 1000 packets, 2 imissed, 50 discards
        let mut p = NicPoller::new(0, 0.0, &ps(1_000, 2, 0), 50);

        p.poll(10.0, &ps(1_500, 3, 0), &hw, || {
            reads.set(reads.get() + 1);
            Some(0)
        });
        assert_eq!(
            reads.get(),
            0,
            "port discards are only read when an epoch closes"
        );
        assert_eq!(hw.epochs[0].epochs_closed(), 0);

        let close = 10.0 + GAP + 1.0;
        p.poll(close, &ps(1_500, 3, 0), &hw, || {
            reads.set(reads.get() + 1);
            Some(60)
        });
        assert_eq!(reads.get(), 1);
        assert_eq!(hw.epochs[0].epochs_closed(), 1);
        let r = hw.epochs[0].record(0).unwrap();
        assert_eq!(r.start_uptime_ns, 0);
        assert_eq!(r.end_uptime_ns, close as u64);
        assert_eq!(
            r.counters,
            [500, 1, 10],
            "rx_packets, imissed, rx_port_discards"
        );
        assert_eq!(hw.epochs[0].first_loss(0), Some(None));
    }

    #[test]
    fn nic_epoch_timestamps_are_uptime_relative_to_the_poller_start() {
        let hw = NicHwStats::new(1);
        let mut p = NicPoller::new(0, 5_000.0, &ps(0, 0, 0), 0);
        p.poll(5_010.0, &ps(10, 0, 0), &hw, || None);
        let close = 5_010.0 + GAP + 1.0;
        p.poll(close, &ps(10, 0, 0), &hw, || Some(0));
        let r = hw.epochs[0].record(0).unwrap();
        assert_eq!(r.start_uptime_ns, 0);
        assert_eq!(r.end_uptime_ns, (close - 5_000.0) as u64);
    }

    #[test]
    fn the_second_nic_epoch_holds_deltas_since_the_first_close() {
        let hw = NicHwStats::new(1);
        let mut p = NicPoller::new(0, 0.0, &ps(0, 0, 0), 0);
        p.poll(10.0, &ps(100, 1, 0), &hw, || None);
        let close1 = 10.0 + GAP + 1.0;
        p.poll(close1, &ps(100, 1, 0), &hw, || Some(5));
        p.poll(close1 + 1.0, &ps(250, 1, 0), &hw, || None);
        let close2 = close1 + 1.0 + GAP + 1.0;
        p.poll(close2, &ps(250, 4, 0), &hw, || Some(9));

        assert_eq!(hw.epochs[0].epochs_closed(), 2);
        assert_eq!(hw.epochs[0].record(0).unwrap().counters, [100, 1, 5]);
        let r1 = hw.epochs[0].record(1).unwrap();
        assert_eq!(r1.start_uptime_ns, close1 as u64, "contiguous");
        assert_eq!(r1.counters, [150, 3, 4], "deltas, not cumulative sums");
    }

    #[test]
    fn a_failed_discard_read_gives_zero_and_the_next_one_catches_up() {
        let hw = NicHwStats::new(1);
        let mut p = NicPoller::new(0, 0.0, &ps(0, 0, 0), 100);
        p.poll(10.0, &ps(50, 0, 0), &hw, || unreachable!());
        let close1 = 10.0 + GAP + 1.0;
        p.poll(close1, &ps(50, 0, 0), &hw, || None);
        assert_eq!(hw.epochs[0].record(0).unwrap().counters[2], 0);

        p.poll(close1 + 1.0, &ps(80, 0, 0), &hw, || unreachable!());
        p.poll(close1 + 1.0 + GAP + 1.0, &ps(80, 0, 0), &hw, || Some(130));
        assert_eq!(hw.epochs[0].record(1).unwrap().counters[2], 30);
    }

    #[test]
    fn nic_epochs_are_independent_per_port() {
        let hw = NicHwStats::new(2);
        let mut p0 = NicPoller::new(0, 0.0, &ps(0, 0, 0), 0);
        let mut p1 = NicPoller::new(1, 0.0, &ps(0, 0, 0), 0);
        p0.poll(10.0, &ps(10, 0, 0), &hw, || None);
        p0.poll(10.0 + GAP + 1.0, &ps(10, 0, 0), &hw, || Some(0));
        p1.poll(10.0, &ps(0, 0, 0), &hw, || None); // port 1 never saw traffic
        p1.poll(10.0 + GAP + 1.0, &ps(0, 0, 0), &hw, || None);
        assert_eq!(hw.epochs[0].epochs_closed(), 1);
        assert_eq!(hw.epochs[1].epochs_closed(), 0);
    }
}
