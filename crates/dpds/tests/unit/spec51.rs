//! The IAT histogram, the frame size counters and `out_of_order` of a link.
//!
//! # API under test
//!
//! ```text
//! HistRange::iat_from_config(&Config) -> HistRange
//!     [0, hi) in 256 bins, hi = 4 * 1e9 / delayer.initialization_rate_pps ns, whatever the
//!     delay distribution, mean, std and limiter of the config.
//! Stats::with_hists(delay: HistRange, iat: HistRange, delay_stats: bool) -> Stats
//! DirectionStats::with_hists(Vec<(HistRange, HistRange, bool)>) -> DirectionStats
//!     one (delay range, IAT range, delay statistics on) per direction. The IAT histogram is
//!     recorded exactly when the delay statistics are on (the `DELAY = true` loop instance).
//! Stats::iat_hist_reply(&self) -> Reply                      page 0, like delay_hist_reply
//! DirectionStats::iat_hist_reply(&self, param) -> Option<Reply>
//!     "<dir>" or "<dir>,<page>" like delay_hist_reply: the eight keys of a delay histogram page
//!     (`lo_ns`, `bin_ns`, `num_bins`, `bins`, `underflow`, `overflow`, `page`, `first_bin`);
//!     `None` for a bad dir or a page past the last; empty page 0 when the statistics are off.
//! ```
//!
//! `/dpds/iat_hist,<dir>[,<page>]` (the DPDK registration) needs an EAL and is not covered here.
//!
//! Stats reply (`Stats::stats_reply`, every build): `frame_sizes` is a `UintArray` of 8 (the
//! cumulative counts of the buckets [0,63], [64], [65,127], [128,255], [256,511],
//! [512,1023], [1024,1518], [1519,inf) of the `pkt_len` of every received frame, before any
//! drop) and `out_of_order` is a `Uint` (cumulative, `Buffer::out_of_order`).
//!
//! IAT tests use `initialization_rate_pps = 15_625_000`: hi = 256 ns, `bin_ns = 1.0`, so a gap of
//! `g` ns (a whole number) is bin `g` exactly. A Spacer at 1 bit/ns delays packet `i` of a burst
//! by `sum_{j<i} 8 * (len_j + 24)` ns more than packet 0 (see delay_stats.rs), so the gap between
//! consecutive send times is `8 * (len_{i-1} + 24)` ns.

use dpds::delay_stats::HistRange;
use dpds::reply::{Reply, Value};
use dpds::stats::{DirectionStats, Stats};
use serde_json::json;

use crate::support::*;

const RATE: f64 = 15_625_000.0;

fn val<'a>(r: &'a Reply, key: &str) -> &'a Value {
    let hits: Vec<_> = r.iter().filter(|(k, _)| k == key).collect();
    assert_eq!(hits.len(), 1, "key {key:?} must appear once in {r:?}");
    &hits[0].1
}
fn uint(r: &Reply, key: &str) -> u64 {
    match val(r, key) {
        Value::Uint(v) => *v,
        other => panic!("{key}: expected Uint, got {other:?}"),
    }
}
fn num(r: &Reply, key: &str) -> f64 {
    match val(r, key) {
        Value::Uint(v) => *v as f64,
        Value::Str(s) => s.parse().unwrap_or_else(|_| panic!("{key}: {s:?}")),
        other => panic!("{key}: expected a number, got {other:?}"),
    }
}
fn array(r: &Reply, key: &str) -> Vec<u64> {
    match val(r, key) {
        Value::UintArray(v) => v.clone(),
        other => panic!("{key}: expected UintArray, got {other:?}"),
    }
}
fn keys(r: &Reply) -> Vec<&str> {
    let mut k: Vec<&str> = r.iter().map(|(k, _)| k.as_str()).collect();
    k.sort_unstable();
    k
}

fn with_rate(mut c: dpds_core::Config, rate: f64) -> dpds_core::Config {
    c.delayer.initialization_rate_pps = rate;
    c
}

// ---- the IAT range, every build --------------------------------------------------------

#[test]
fn the_iat_range_is_four_mean_gaps_in_256_bins_from_zero() {
    for (rate, hi) in [
        (RATE, 256.0),
        (1e6, 4000.0),
        (1e7, 400.0),
        (1e9, 4.0),
        (15e6, 4e9 / 15e6),
    ] {
        let r = HistRange::iat_from_config(&with_rate(config(100, 0.0, 0.0), rate));
        assert_eq!(r.lo_ns, 0.0, "rate {rate}");
        assert_eq!(r.bins, 256, "rate {rate}");
        assert!(
            (r.hi_ns() - hi).abs() <= hi * 1e-12,
            "rate {rate}: {}",
            r.hi_ns()
        );
        assert!((r.bin_ns - hi / 256.0).abs() <= hi * 1e-12, "rate {rate}");
    }
}

#[test]
fn the_iat_range_ignores_the_delay_distribution_and_the_limiter() {
    let plain = HistRange::iat_from_config(&with_rate(config(100, 0.0, 0.0), 1e6));
    let normal = config_general(
        10,
        false,
        "NormalDistribution",
        5e6,
        1e5,
        Some(("Spacer", 1e9)),
        json!({ "Random": 0.1 }),
    );
    assert_eq!(HistRange::iat_from_config(&with_rate(normal, 1e6)), plain);
}

// ---- the IAT reply shape, every build ------------------------------------------------------------

fn iat_dirs(on: bool) -> DirectionStats {
    let iat = HistRange::iat_from_config(&with_rate(config(100, 0.0, 0.0), RATE));
    DirectionStats::with_hists(vec![(HistRange::default(), iat, on); 2])
}

#[test]
fn the_iat_reply_has_the_eight_keys_of_a_histogram_page() {
    let ds = iat_dirs(true);
    let r = ds.iat_hist_reply("0").expect("page 0");
    assert_eq!(
        keys(&r),
        [
            "bin_ns",
            "bins",
            "first_bin",
            "lo_ns",
            "num_bins",
            "overflow",
            "page",
            "underflow"
        ]
    );
    assert_eq!(num(&r, "lo_ns"), 0.0);
    assert_eq!(num(&r, "bin_ns"), 1.0);
    assert_eq!(uint(&r, "num_bins"), 256);
    assert_eq!(array(&r, "bins").len(), 256);
    assert_eq!((uint(&r, "page"), uint(&r, "first_bin")), (0, 0));
    assert_eq!(ds.iat_hist_reply("0,0"), Some(r.clone()));
    assert_eq!(ds.iat_hist_reply("1"), Some(r));
}

#[test]
fn the_iat_reply_has_no_second_page_and_refuses_bad_params() {
    let ds = iat_dirs(true);
    for bad in ["0,1", "2", "x", "", "0,x", "0,0,0", "-1"] {
        assert!(ds.iat_hist_reply(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn a_link_without_delay_statistics_has_an_empty_iat_page_zero() {
    let ds = iat_dirs(false);
    let r = ds.iat_hist_reply("0").expect("empty page 0, not an error");
    assert!(
        !r.iter()
            .any(|(k, v)| (k == "bins" && *v != Value::UintArray(vec![]))
                || (k == "num_bins" && *v != Value::Uint(0))),
        "{r:?}"
    );
    assert!(ds.iat_hist_reply("0,1").is_none());
}

#[test]
fn the_stats_reply_carries_frame_sizes_and_out_of_order_in_every_build() {
    let r = Stats::default().stats_reply(1);
    assert_eq!(array(&r, "frame_sizes"), vec![0u64; 8]);
    assert_eq!(uint(&r, "out_of_order"), 0);
    // also when the delay statistics are off: the counters are not part of them
    let r = Stats::with_delay_stats(HistRange::default(), false).stats_reply(1);
    assert_eq!(array(&r, "frame_sizes"), vec![0u64; 8]);
    assert_eq!(uint(&r, "out_of_order"), 0);
}

// ---- through forward::run (needs the counters) ---------------------------------------------------

#[cfg(feature = "telemetry")]
mod with_feature {
    use dpds::forward::run;
    use dpds::quit::Quit;
    use dpds::stats::Telemetry;
    use dpds::worker::Worker;
    use dpds_core::buffer::Buffer;
    use dpds_core::Config;

    use super::*;

    /// One direction (port 0 -> 0). Burst `k` is what the k-th `rx_burst` returns (`bursts[k][i]`
    /// = the `pkt_len` of its i-th packet); quit is requested inside the last scripted call, so
    /// the next iteration drains. The k-th iteration reads `clock[k]`, the last value repeats.
    fn drive(cfg: Config, bursts: &[&[u32]], clock: &[f64], stats: &'static Stats) {
        let total: usize = bursts.iter().map(|b| b.len()).sum();
        let mut m = FakeMbufs::new(total, 0, 64);
        let mut script = Vec::new();
        let mut next = 0;
        for b in bursts {
            let mut ptrs = Vec::new();
            for &len in *b {
                m.set_pkt_len(next, len);
                ptrs.push(m.ptr(next));
                next += 1;
            }
            script.push(ptrs);
        }
        let quit = Quit::new();
        let mut rx = MockRx::with_script(script);
        let last = bursts.len();
        {
            let q = &quit;
            rx.on_call = Some(Box::new(move |n| {
                if n == last {
                    q.request()
                }
            }));
        }
        let (mut tx, mut free) = (MockTx::default(), MockFree::default());
        let mut t = Telemetry::new(stats, 0.0);
        let (now, _) = scripted_clock(clock, None);
        // SAFETY: fake but valid mbufs owned by the test.
        unsafe {
            run(
                Worker::new(Buffer::new(cfg), 0, 0),
                &mut rx,
                &mut tx,
                &mut free,
                &mut t,
                &quit,
                now,
            )
        };
        assert_eq!(tx.offered().len() + free.freed().len(), total);
    }

    fn leak(s: Stats) -> &'static Stats {
        Box::leak(Box::new(s))
    }

    fn iat_stats(cfg: &Config) -> &'static Stats {
        leak(Stats::with_hists(
            HistRange::default(),
            HistRange::iat_from_config(cfg),
            true,
        ))
    }

    // ---- frame sizes -----------------------------------------------------------------

    fn sizes(len: u32) -> Vec<u64> {
        let stats = leak(Stats::default());
        drive(config(10, 0.0, 0.0), &[&[len]], &[0.0, 1e6], stats);
        array(&stats.stats_reply(0), "frame_sizes")
    }

    #[test]
    fn every_bucket_boundary_lands_in_the_right_bucket() {
        let cases: [(u32, usize); 22] = [
            (0, 0),
            (1, 0),
            (63, 0),
            (64, 1),
            (65, 2),
            (127, 2),
            (128, 3),
            (255, 3),
            (256, 4),
            (511, 4),
            (512, 5),
            (1023, 5),
            (1024, 6),
            (1500, 6),
            (1518, 6),
            (1519, 7),
            (1520, 7),
            (9000, 7),
            (65_535, 7),
            (100_000, 7),
            (1_000_000, 7),
            (u32::MAX, 7),
        ];
        for (len, bucket) in cases {
            let mut want = vec![0u64; 8];
            want[bucket] = 1;
            assert_eq!(sizes(len), want, "pkt_len {len} belongs to bucket {bucket}");
        }
    }

    #[test]
    fn the_counters_are_cumulative_over_bursts_and_sum_to_rx_pkts() {
        let stats = leak(Stats::default());
        drive(
            config(100, 0.0, 0.0),
            &[
                &[64, 64, 1518],
                &[1519, 0],
                &[],
                &[65, 128, 256, 512, 1024, 63],
            ],
            &[0.0, 10.0, 20.0, 30.0, 1e6],
            stats,
        );
        let r = stats.stats_reply(0);
        assert_eq!(array(&r, "frame_sizes"), vec![2, 2, 1, 1, 1, 1, 2, 1]);
        assert_eq!(
            array(&r, "frame_sizes").iter().sum::<u64>(),
            uint(&r, "rx_pkts")
        );
    }

    #[test]
    fn frames_that_are_rejected_are_counted_too() {
        // capacity 2, 5 frames in one burst: 3 are refused (buffer full) but were received
        let stats = leak(Stats::default());
        drive(
            config(2, 1e6, 0.0),
            &[&[64, 64, 1500, 1500, 9000]],
            &[0.0, 1e9],
            stats,
        );
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "buffer_full_drops"), 3);
        assert_eq!(array(&r, "frame_sizes"), vec![0, 2, 0, 0, 0, 0, 2, 1]);
    }

    #[test]
    fn frames_dropped_by_the_loss_model_are_counted_too() {
        let stats = leak(Stats::default());
        drive(
            config(100, 0.0, 1.0),
            &[&[64, 100, 200, 300, 600, 1200, 1519]],
            &[0.0, 1e6],
            stats,
        );
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "tx_sent"), 0, "all lost");
        assert_eq!(array(&r, "frame_sizes"), vec![0, 1, 1, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn a_link_without_delay_statistics_counts_frame_sizes_too() {
        let stats = leak(Stats::with_delay_stats(HistRange::default(), false));
        drive(
            config(100, 0.0, 0.0),
            &[&[63, 64, 65, 1518, 1519]],
            &[0.0, 1e6],
            stats,
        );
        assert_eq!(
            array(&stats.stats_reply(0), "frame_sizes"),
            vec![1, 1, 1, 0, 0, 0, 1, 1]
        );
    }

    // ---- the IAT histogram ---------------------------------------------------------------

    fn spacer(base: f64) -> Config {
        with_rate(
            config_general(
                100,
                false,
                "DeterministicDistribution",
                base,
                0.0,
                Some(("Spacer", 1e9)),
                json!({ "Random": 0.0 }),
            ),
            RATE,
        )
    }

    /// A deterministic delay of 100 ns, no limiter.
    fn plain(reordering: bool) -> Config {
        with_rate(
            config_general(
                100,
                reordering,
                "DeterministicDistribution",
                100.0,
                0.0,
                None,
                json!({ "Random": 0.0 }),
            ),
            RATE,
        )
    }

    /// The histogram of `iat_hist_reply` as `(bin index, count)` of the non-empty bins.
    fn filled(r: &Reply) -> Vec<(usize, u64)> {
        array(r, "bins")
            .into_iter()
            .enumerate()
            .filter(|&(_, c)| c > 0)
            .collect()
    }

    #[test]
    fn the_gaps_between_the_send_times_of_one_burst_fill_the_bins() {
        // lens 1, 0, 5, 0, 0, 0: gaps 200, 192, 232, 192, 192 (5 gaps for 6 packets)
        let cfg = spacer(800.0);
        let stats = iat_stats(&cfg);
        drive(cfg, &[&[1, 0, 5, 0, 0, 0]], &[0.0, 1e5], stats);
        let r = stats.iat_hist_reply();
        assert_eq!(filled(&r), vec![(192, 3), (200, 1), (232, 1)]);
        assert_eq!((uint(&r, "underflow"), uint(&r, "overflow")), (0, 0));
    }

    #[test]
    fn a_single_packet_has_no_gap() {
        let cfg = spacer(800.0);
        let stats = iat_stats(&cfg);
        drive(cfg, &[&[100]], &[0.0, 1e5], stats);
        let r = stats.iat_hist_reply();
        assert!(filled(&r).is_empty(), "{r:?}");
        assert_eq!((uint(&r, "underflow"), uint(&r, "overflow")), (0, 0));
    }

    #[test]
    fn the_previous_send_time_is_carried_across_bursts_and_loop_passes() {
        // delay 100, no limiter. it1 (now 0): A, B pushed, send 100, nothing due.
        // it2 (now 150): A, B popped -> gap 0; C pushed (send 250).
        // it3 (now 300, quit): C popped -> gap 250 - 100 = 150 from B, in another pass.
        // The staged packets leave in it4 (10 us after the oldest send time).
        let cfg = plain(false);
        let stats = iat_stats(&cfg);
        // it4 (1e6) sends what it2 and it3 staged
        drive(cfg, &[&[64, 64], &[64]], &[0.0, 150.0, 300.0, 1e6], stats);
        let r = stats.iat_hist_reply();
        assert_eq!(filled(&r), vec![(0, 1), (150, 1)]);
        assert_eq!((uint(&r, "underflow"), uint(&r, "overflow")), (0, 0));
    }

    #[test]
    fn a_negative_gap_counts_as_zero() {
        // reordering. it1 (now 1000): A pushed, send 1100. it2 (now 1100): A popped (the first of
        // the run, no gap). it3 (now 0): C pushed, send 100, not due. it4 (now 1e6): C popped, its
        // send time is 1000 ns before A's: gap -1000 -> 0 (bin 0), not a wrapped huge number.
        let cfg = plain(true);
        let stats = iat_stats(&cfg);
        drive(
            cfg,
            &[&[64], &[], &[64]],
            &[1000.0, 1100.0, 0.0, 1e6],
            stats,
        );
        let r = stats.iat_hist_reply();
        assert_eq!(filled(&r), vec![(0, 1)]);
        assert_eq!((uint(&r, "underflow"), uint(&r, "overflow")), (0, 0));
    }

    #[test]
    fn a_gap_of_hi_or_more_is_an_overflow_and_the_gap_just_below_is_the_last_bin() {
        // lens 7, 8, 0, 100: the three gaps are 8 * (7 + 24) = 248, 8 * (8 + 24) = 256 (= hi,
        // half open) and 8 * (0 + 24) = 192.
        let cfg = spacer(800.0);
        let stats = iat_stats(&cfg);
        drive(cfg, &[&[7, 8, 0, 100]], &[0.0, 1e5], stats);
        let r = stats.iat_hist_reply();
        assert_eq!(filled(&r), vec![(192, 1), (248, 1)]);
        assert_eq!(uint(&r, "overflow"), 1, "the gap of exactly hi");
        assert_eq!(uint(&r, "underflow"), 0);
    }

    #[test]
    fn rejected_packets_have_no_send_time_and_no_gap() {
        // capacity 3, 5 packets of len 0: 3 accepted (gaps 192, 192), 2 refused
        let cfg = {
            let mut c = spacer(800.0);
            c.buffer_capacity = 3;
            c
        };
        let stats = iat_stats(&cfg);
        drive(cfg, &[&[0, 0, 0, 0, 0]], &[0.0, 1e5], stats);
        let r = stats.iat_hist_reply();
        assert_eq!(filled(&r), vec![(192, 2)]);
        assert_eq!((uint(&r, "underflow"), uint(&r, "overflow")), (0, 0));
    }

    #[test]
    fn a_link_without_delay_statistics_records_no_iat() {
        let cfg = spacer(800.0);
        let iat = HistRange::iat_from_config(&cfg);
        let stats = leak(Stats::with_hists(HistRange::default(), iat, false));
        drive(cfg, &[&[1, 0, 5, 0, 0, 0]], &[0.0, 1e5], stats);
        let r = stats.iat_hist_reply();
        assert!(
            array_or_empty(&r).iter().all(|&c| c == 0),
            "nothing recorded: {r:?}"
        );
        assert_eq!(uint(&stats.stats_reply(0), "tx_sent"), 6);
    }

    fn array_or_empty(r: &Reply) -> Vec<u64> {
        r.iter()
            .find_map(|(k, v)| match (k.as_str(), v) {
                ("bins", Value::UintArray(a)) => Some(a.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    // ---- out_of_order through the loop ---------------------------------------------------

    #[test]
    fn a_reordering_link_reports_the_out_of_order_departures() {
        // pushed at now = 0, 10, 20, 5 (send 100, 110, 120, 105), all popped in the last pass in
        // the order of the arrival sequence numbers 0, 3, 1, 2: seq 1 and seq 2 are below the
        // highest popped (3): 2.
        let stats = leak(Stats::default());
        drive(
            plain(true),
            &[&[64], &[64], &[64], &[64]],
            &[0.0, 10.0, 20.0, 5.0, 1e6],
            stats,
        );
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "tx_sent"), 4);
        assert_eq!(uint(&r, "out_of_order"), 2);
    }

    #[test]
    fn a_fifo_link_reports_zero_out_of_order_whatever_the_send_times() {
        let stats = leak(Stats::default());
        drive(
            plain(false),
            &[&[64], &[64], &[64], &[64]],
            &[0.0, 10.0, 20.0, 5.0, 1e6],
            stats,
        );
        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "tx_sent"), 4);
        assert_eq!(uint(&r, "out_of_order"), 0);
    }

    #[test]
    fn out_of_order_is_counted_without_the_delay_statistics_too() {
        let stats = leak(Stats::with_delay_stats(HistRange::default(), false));
        drive(
            plain(true),
            &[&[64], &[64], &[64], &[64]],
            &[0.0, 10.0, 20.0, 5.0, 1e6],
            stats,
        );
        assert_eq!(uint(&stats.stats_reply(0), "out_of_order"), 2);
    }
}
