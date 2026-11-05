//! Per-cause drop counters of one direction, driven through `forward::run`
//! with mocks (hand-derived, like `limiter.rs`/`loss.rs` of dpds-core).
//!
//! # Test-facing API (new)
//!
//! ```text
//! DpdsCounters: pub loss_drops, limiter_drops, buffer_full_drops: Counter
//!   (single writer, same discipline as dpds_rejects; zero-sized no-ops without
//!   the feature). The invariant, checked below: dpds_rejects ==
//!   loss_drops + limiter_drops + buffer_full_drops.
//!   They are the LAST three entries of DpdsCounters::EPOCH_COUNTER_NAMES /
//!   cumulative_snapshot() (12 counters now), so /dpds/epochs has the arrays
//!   loss_drops, limiter_drops, buffer_full_drops and /dpds/stats the uints of
//!   the same names.
//! First loss: reason stays `DpdsReject`; the `first_loss_detail` string names
//!   the cause of the FIRST reject of the run: "from=dpds,cause=loss",
//!   "from=dpds,cause=limiter" or "from=dpds,cause=buffer_full"
//!   (dpds::first_loss::format_first_loss(..).detail). `StageLossDetail::Single`
//!   (-> "from=dpds") stays for other callers; how the cause is carried inside
//!   the detail type is up to the implementation, the tests only look at the
//!   formatted string.
//! forward::run uses `Buffer::push_outcome`; precedence (spec20) buffer full >
//!   loss > limiter: the capacity check comes first and a packet that finds the
//!   buffer full counts as buffer full whatever loss model and limiter would
//!   say (they are not consulted); a packet rejected by both loss and limiter
//!   counts as loss.
//! ```
//!
//! The limiter's limit is derived: `buffer_capacity * 1542 * 8` bit. A
//! 1519-byte packet is (1519 + 4 + 20) * 8 = 12344 bit on the wire, 8 bit more
//! than one 12336-bit "slot" of the limit, so a Spacer limit drop happens while
//! the buffer still has room.
//!
//! Scenario "mixed" (deterministic delay 1_000_000 ns, Spacer 1 bit/ns, buffer
//! capacity 4 = limit 49344 bit, all packets 1519 bytes = 12344 bit, loss model
//! GilbertElliot(p=1, r=0, k=0, h=1): exactly the FIRST packet is lost):
//!
//! Iteration 1 (now 0), packets P1..P5:
//!   P1 lost (but accounted by the limiter: fill 12344)            -> loss
//!   P2 fill 12344 + 12344 = 24688 <= 49344, send 1_012_344        -> accepted
//!   P3 fill 24688 + 12344 = 37032 <= 49344, send 1_024_688        -> accepted
//!   P4, P5: 37032 + 12344 = 49376 > 49344 (buffer holds 2 of 4)   -> limiter (x2)
//! Iteration 2 (now 50_000), packets P6..P8: the limiter's fill (37032, rejected
//!   packets are not added) has drained to 0 by t = 1_050_000.
//!   P6 fill 0 -> accepted (send 1_050_000), P7 fill 12344 -> accepted
//!   (send 1_062_344), the buffer now holds 4 = capacity
//!   P8: buffer full (untouched: no limiter accounting, no loss draw)
//! So loss 1, limiter 2, buffer_full 1, dpds_rejects 4, 4 packets sent
//! (P2, P3, P6, P7 in this order).

use dpds::forward::run;
use dpds::quit::Quit;
use dpds::stats::{Stats, Telemetry};
use dpds::worker::Worker;
use dpds_core::buffer::Buffer;
use dpds_core::Config;
use serde_json::json;

use crate::support::*;

const GE_FIRST_LOST: fn() -> serde_json::Value =
    || json!({ "GilbertElliot": [1.0, 0.0, 0.0, 1.0, true] });

struct Out {
    tx: MockTx,
    free: MockFree,
}

/// Runs one direction (port 0 -> port 0): batch `i` is delivered by `rx_burst`
/// call `i + 1`; quit is requested inside the last call. The clock returns
/// `clock[0]`, `clock[1]`, ... then the last value.
fn drive(
    cfg: Config,
    script: Vec<Vec<dpdk::RawMbuf>>,
    clock: &[f64],
    stats: &'static Stats,
) -> Out {
    let quit = Quit::new();
    let last = script.len();
    let mut rx = MockRx::with_script(script);
    {
        let q = &quit;
        rx.on_call = Some(Box::new(move |k| {
            if k == last {
                q.request();
            }
        }));
    }
    let mut tx = MockTx::default();
    let mut free = MockFree::default();
    let mut t = Telemetry::new(stats, 0.0);
    let (now, _) = scripted_clock(clock, None);
    // SAFETY: fake but valid mbufs owned by the test; the mocks never
    // dereference them.
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
    Out { tx, free }
}

fn mixed_config() -> Config {
    config_general(
        4,
        false,
        "DeterministicDistribution",
        1_000_000.0,
        0.0,
        Some(("Spacer", 1e9)),
        GE_FIRST_LOST(),
    )
}

/// Clock of the mixed scenario: iteration 1 at 0, iteration 2 at 50 us, the
/// last (draining) read at 5 ms.
const MIXED_CLOCK: [f64; 3] = [0.0, 50_000.0, 5_000_000.0];

/// All packets of the mixed scenario are 1519 bytes.
const MIXED_LEN: u32 = 1519;

/// 5 packets in the first burst, 3 in the second.
fn mixed_script(m: &mut FakeMbufs) -> Vec<Vec<dpdk::RawMbuf>> {
    let p = m.ptrs();
    assert_eq!(p.len(), 8);
    vec![p[..5].to_vec(), p[5..].to_vec()]
}

#[test]
fn rejected_packets_are_freed_and_the_four_accepted_ones_are_sent_in_every_build() {
    let mut m = FakeMbufs::new(8, 0, MIXED_LEN);
    let p = m.ptrs();
    let script = mixed_script(&mut m);
    let stats: &'static Stats = Box::leak(Box::default());
    let o = drive(mixed_config(), script, &MIXED_CLOCK, stats);
    assert_eq!(
        o.tx.offered(),
        vec![p[1], p[2], p[5], p[6]],
        "P2, P3, P6 and P7 leave, in order"
    );
    let mut freed = o.free.freed();
    freed.sort();
    let mut expect = vec![p[0], p[3], p[4], p[7]];
    expect.sort();
    assert_eq!(freed, expect, "P1, P4, P5, P8 are freed");
}

#[cfg(feature = "telemetry")]
mod with_feature {
    use dpds::counters::DpdsCounters;
    use dpds::first_loss::{format_first_loss, FirstLossReason};
    use dpds::reply::Value;

    use super::*;

    fn uint(r: &dpds::reply::Reply, key: &str) -> u64 {
        match r.iter().find(|(k, _)| k == key) {
            Some((_, Value::Uint(v))) => *v,
            other => panic!("{key}: expected Uint, got {other:?}"),
        }
    }

    fn detail(stats: &Stats) -> String {
        format_first_loss(stats.first_loss.current()).detail
    }

    #[test]
    fn mixed_scenario_counts_each_cause_and_the_sum_is_dpds_rejects() {
        let mut m = FakeMbufs::new(8, 0, MIXED_LEN);
        let script = mixed_script(&mut m);
        let stats: &'static Stats = Box::leak(Box::default());
        drive(mixed_config(), script, &MIXED_CLOCK, stats);
        let c = &stats.counters;
        assert_eq!(c.rx_pkts.get(), 8);
        assert_eq!(c.loss_drops.get(), 1);
        assert_eq!(c.limiter_drops.get(), 2);
        assert_eq!(c.buffer_full_drops.get(), 1);
        assert_eq!(c.dpds_rejects.get(), 4);
        assert_eq!(
            c.dpds_rejects.get(),
            c.loss_drops.get() + c.limiter_drops.get() + c.buffer_full_drops.get()
        );
        assert_eq!(c.tx_sent.get(), 4);

        let r = stats.stats_reply(0);
        assert_eq!(uint(&r, "loss_drops"), 1);
        assert_eq!(uint(&r, "limiter_drops"), 2);
        assert_eq!(uint(&r, "buffer_full_drops"), 1);
        assert_eq!(uint(&r, "dpds_rejects"), 4);
    }

    #[test]
    fn the_first_loss_detail_names_the_cause_of_the_first_reject() {
        // mixed: the very first reject is P1, the loss model
        let mut m = FakeMbufs::new(8, 0, MIXED_LEN);
        let script = mixed_script(&mut m);
        let stats: &'static Stats = Box::leak(Box::default());
        drive(mixed_config(), script, &MIXED_CLOCK, stats);
        assert_eq!(
            stats.first_loss.current().unwrap().reason,
            FirstLossReason::DpdsReject
        );
        assert_eq!(detail(stats), "from=dpds,cause=loss");
        let r = stats.stats_reply(0);
        assert!(r
            .iter()
            .any(|(k, v)| k == "first_loss_detail"
                && *v == Value::Str("from=dpds,cause=loss".into())));

        // limiter only: capacity 3 = limit 37008 bit; of three 12344-bit
        // packets the third does not fit (37032 > 37008) while the buffer
        // holds only two
        let cfg = config_full(3, false, 0.0, 0.0, Some(1e9));
        let mut m = FakeMbufs::new(3, 0, MIXED_LEN);
        let p = m.ptrs();
        let stats: &'static Stats = Box::leak(Box::default());
        drive(cfg, vec![p], &[0.0, 1_000_000.0], stats);
        let c = &stats.counters;
        assert_eq!(
            (
                c.loss_drops.get(),
                c.limiter_drops.get(),
                c.buffer_full_drops.get()
            ),
            (0, 1, 0)
        );
        assert_eq!(detail(stats), "from=dpds,cause=limiter");

        // buffer full only: capacity 2, a delay nothing is due for
        let cfg = config(2, 1_000_000.0, 0.0);
        let mut m = FakeMbufs::new(3, 0, 100);
        let p = m.ptrs();
        let stats: &'static Stats = Box::leak(Box::default());
        drive(cfg, vec![p], &[0.0, 5_000_000.0], stats);
        let c = &stats.counters;
        assert_eq!(
            (
                c.loss_drops.get(),
                c.limiter_drops.get(),
                c.buffer_full_drops.get()
            ),
            (0, 0, 1)
        );
        assert_eq!(detail(stats), "from=dpds,cause=buffer_full");
    }

    #[test]
    fn precedence_in_the_loop_buffer_full_before_loss_before_limiter() {
        // 20000-byte packets exceed the derived limit of a capacity-10 buffer
        // (123_360 bit): the limiter would reject every one. Capacity 0 is
        // always full.
        let with = |capacity: usize, loss: f64, limiter: Option<f64>| {
            config_full(capacity, false, 0.0, loss, limiter)
        };
        for (cfg, expect) in [
            (with(10, 1.0, Some(1e9)), (10, 0, 0)), // loss + limiter -> loss
            (with(10, 0.0, Some(1e9)), (0, 10, 0)), // limiter only
            (with(0, 1.0, Some(1e9)), (0, 0, 10)),  // full + loss + limiter -> full
            (with(0, 0.0, Some(1e9)), (0, 0, 10)),  // full + limiter -> full
            (with(0, 1.0, None), (0, 0, 10)),       // full + loss -> full
            (with(0, 0.0, None), (0, 0, 10)),       // only full
        ] {
            let mut m = FakeMbufs::new(10, 0, 20_000);
            let p = m.ptrs();
            let stats: &'static Stats = Box::leak(Box::default());
            drive(cfg, vec![p], &[0.0, 1.0], stats);
            let c = &stats.counters;
            assert_eq!(
                (
                    c.loss_drops.get(),
                    c.limiter_drops.get(),
                    c.buffer_full_drops.get()
                ),
                expect
            );
            assert_eq!(c.dpds_rejects.get(), 10);
        }
    }

    #[test]
    fn a_pass_through_has_no_drops_of_any_cause() {
        let mut m = FakeMbufs::new(8, 0, 100);
        let p = m.ptrs();
        let stats: &'static Stats = Box::leak(Box::default());
        // the staged packets need 10 us to be sent: the last read is far past that
        drive(config(100, 0.0, 0.0), vec![p], &[0.0, 1e6], stats);
        let c = &stats.counters;
        assert_eq!(
            (
                c.dpds_rejects.get(),
                c.loss_drops.get(),
                c.limiter_drops.get(),
                c.buffer_full_drops.get()
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(detail(stats), "");
    }

    #[test]
    fn the_causes_reach_the_closed_epoch() {
        // The third clock read (1e9 ns) is more than 500 ms after the last
        // arrival (iteration 2, now 50_000): the epoch closes in the draining
        // iteration, after the pops.
        let mut m = FakeMbufs::new(8, 0, MIXED_LEN);
        let script = mixed_script(&mut m);
        let stats: &'static Stats = Box::leak(Box::default());
        drive(mixed_config(), script, &[0.0, 50_000.0, 1.0e9], stats);
        assert_eq!(stats.epochs.epochs_closed(), 1);
        let rec = stats.epochs.record(0).unwrap();
        let at = |name: &str| {
            let i = DpdsCounters::EPOCH_COUNTER_NAMES
                .iter()
                .position(|n| *n == name)
                .unwrap_or_else(|| panic!("{name} is not an epoch counter"));
            rec.counters[i]
        };
        assert_eq!(at("loss_drops"), 1);
        assert_eq!(at("limiter_drops"), 2);
        assert_eq!(at("buffer_full_drops"), 1);
        assert_eq!(at("dpds_rejects"), 4);
        let names = DpdsCounters::EPOCH_COUNTER_NAMES;
        assert_eq!(
            &names[names.len() - 3..],
            ["loss_drops", "limiter_drops", "buffer_full_drops"],
            "appended after the original nine"
        );
    }

    #[test]
    fn the_first_loss_time_is_the_iterations_now() {
        let mut m = FakeMbufs::new(8, 0, MIXED_LEN);
        let script = mixed_script(&mut m);
        let stats: &'static Stats = Box::leak(Box::default());
        drive(
            mixed_config(),
            script,
            &[7_000.0, 57_000.0, 5_000_000.0],
            stats,
        );
        assert_eq!(stats.first_loss.current().unwrap().relative_ns, 7_000.0);
    }
}
