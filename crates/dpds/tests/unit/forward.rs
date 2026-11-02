//! The fused single-worker loop `dpds::forward::run` (unsafe fn: the RX mock
//! hands out fake mbufs that are valid and owned by the test).
//!
//! One iteration, in this order: read `quit` FIRST; `polls += 1`; one
//! `Rx::step` unless quit was already read as requested; `Worker::step`
//! with that batch (also when empty); `Tx::step` with the popped packets;
//! `t.tick(now, received)`; stop if quit was read as requested AND the worker
//! has no backlog. After the loop `Tx::cleanup`.
//!
//! Consequences pinned below: no `rx_burst` after quit was observed (a live
//! source cannot keep the buffer full forever); what was received before is
//! never lost; delayed packets still leave at their send time during the
//! drain; the loop terminates; TX tables are flushed at the end.
//!
//! Tests run the loop with mocks and a fake clock. The clock panics when it
//! is read absurdly often, so a loop that never terminates fails instead of
//! hanging the suite.

use std::cell::Cell;
use std::rc::Rc;

use dpds::forward::run;
use dpds::io::TxBurst;
use dpds::quit::Quit;
use dpds::worker::Worker;
use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

use crate::support::*;

fn config(capacity: usize, delay_ns: f64, loss: f64) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering: false,
        delayer: DelayerConfig {
            distribution: RVDistribution::DeterministicDistribution,
            mean_ns: delay_ns,
            std_ns: 0.0,
            activate_correlation: false,
            half_life_period_ns: 1.0,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1e9,
            limit_bit: 1e9,
        },
        loss_model: LossModel::Random(loss),
    }
}

fn worker(cfg: Config, ports: u16) -> Worker {
    Worker::new(Buffer::new(cfg), ports)
}

/// Fake time in ns; every read advances it by `step`.
fn ticking_clock(time: &Rc<Cell<f64>>, step: f64) -> impl FnMut() -> f64 {
    let time = time.clone();
    let reads = Cell::new(0u64);
    move || {
        reads.set(reads.get() + 1);
        assert!(reads.get() < 200_000, "the loop does not terminate");
        let now = time.get();
        time.set(now + step);
        now
    }
}

/// TX mock that also records the fake time of every call.
struct TimedTx {
    time: Rc<Cell<f64>>,
    calls: Vec<(f64, u16, Vec<dpdk::RawMbuf>)>,
}

impl TxBurst for TimedTx {
    fn tx_burst(&mut self, port: u16, bufs: &mut [dpdk::RawMbuf]) -> u16 {
        self.calls.push((self.time.get(), port, bufs.to_vec()));
        bufs.len() as u16
    }
}

fn batches(p: &[dpdk::RawMbuf], sizes: &[usize]) -> Vec<Vec<dpdk::RawMbuf>> {
    let mut out = Vec::new();
    let mut at = 0;
    for &n in sizes {
        out.push(p[at..at + n].to_vec());
        at += n;
    }
    out
}

#[test]
fn quit_already_requested_and_nothing_buffered_returns_without_polling_rx() {
    let quit = Quit::new();
    quit.request();
    let mut rx = MockRx::default();
    let mut tx = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);

    // SAFETY: the mocks hand out no mbufs at all.
    unsafe {
        run(
            worker(config(100, 0.0, 0.0), 1),
            &mut rx,
            &mut tx,
            &mut free,
            &mut t,
            &quit,
            fixed_clock(0.0),
        )
    };

    assert!(rx.calls.is_empty(), "no rx_burst once quit was observed");
    assert!(tx.calls.is_empty());
    assert!(free.freed.is_empty());
}

#[test]
fn everything_received_before_quit_comes_out_in_order_and_rx_stops_after_quit() {
    let mut m = FakeMbufs::new(15, 0, 64);
    let p = m.ptrs();
    let quit = Quit::new();
    let mut rx = MockRx::with_script(batches(&p, &[5, 5, 5]));
    // quit is requested while the NIC serves the 2nd poll
    rx.on_call = Some(Box::new(|n| {
        if n == 2 {
            quit.request();
        }
    }));
    let mut tx = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);

    // SAFETY: fake but valid mbufs owned by the test; mocks never dereference them.
    unsafe {
        run(
            worker(config(100, 0.0, 0.0), 1),
            &mut rx,
            &mut tx,
            &mut free,
            &mut t,
            &quit,
            fixed_clock(10.0),
        )
    };

    assert_eq!(rx.calls.len(), 2, "no rx_burst after the quit was observed");
    assert_eq!(rx.script.len(), 1, "the third batch was never received");
    let sent: Vec<_> = tx.calls.iter().flat_map(|c| c.1.clone()).collect();
    assert_eq!(sent, p[..10].to_vec(), "all received packets, in order");
    assert!(tx.calls.iter().all(|c| c.0 == 0));
    assert!(free.freed.is_empty());
}

#[test]
fn a_batch_received_in_the_iteration_that_requested_quit_is_not_lost() {
    // quit is requested "during" the very first rx_burst: quit was read as
    // false at the top of that iteration, so its batch must still go all the
    // way through.
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let quit = Quit::new();
    let mut rx = MockRx::with_script(vec![p.clone()]);
    rx.on_call = Some(Box::new(|_| quit.request()));
    let mut tx = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);

    // SAFETY: fake but valid mbufs owned by the test.
    unsafe {
        run(
            worker(config(100, 0.0, 0.0), 1),
            &mut rx,
            &mut tx,
            &mut free,
            &mut t,
            &quit,
            fixed_clock(0.0),
        )
    };

    assert_eq!(rx.calls.len(), 1);
    let sent: Vec<_> = tx.calls.iter().flat_map(|c| c.1.clone()).collect();
    assert_eq!(sent, p);
}

#[test]
fn two_ports_are_polled_round_robin_and_the_port_is_flipped() {
    let mut m = FakeMbufs::new(5, 0, 64);
    for i in 3..5 {
        m.set_port(i, 1);
    }
    let p = m.ptrs();
    let quit = Quit::new();
    // poll 1 -> port 0 gets p0..3 (port 0), poll 2 -> port 1 gets p3..5 (port 1)
    let mut rx = MockRx::with_script(batches(&p, &[3, 2]));
    rx.on_call = Some(Box::new(|n| {
        if n == 4 {
            quit.request();
        }
    }));
    let mut tx = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);

    // SAFETY: fake but valid mbufs owned by the test.
    unsafe {
        run(
            worker(config(100, 0.0, 0.0), 2),
            &mut rx,
            &mut tx,
            &mut free,
            &mut t,
            &quit,
            fixed_clock(0.0),
        )
    };

    assert_eq!(
        rx.calls.iter().map(|c| c.0).collect::<Vec<_>>(),
        [0, 1, 0, 1]
    );
    let on = |port: u16| -> Vec<_> {
        tx.calls
            .iter()
            .filter(|c| c.0 == port)
            .flat_map(|c| c.1.clone())
            .collect()
    };
    assert_eq!(on(1), p[..3].to_vec(), "port 0 traffic leaves on port 1");
    assert_eq!(on(0), p[3..].to_vec(), "port 1 traffic leaves on port 0");
}

#[test]
fn delayed_packets_are_drained_after_quit_and_still_leave_at_their_send_time() {
    // 10 packets ingested during poll 1 (fake time 0), delay 1000 ns.
    // The clock advances 50 ns per read, quit comes during poll 2; the
    // packets are due at 1000 (+ingest time) and must be sent then, not
    // flushed at quit time.
    let mut m = FakeMbufs::new(15, 0, 64);
    let p = m.ptrs();
    let time = Rc::new(Cell::new(0.0));
    let quit = Quit::new();
    let mut rx = MockRx::with_script(batches(&p, &[10, 5]));
    rx.on_call = Some(Box::new(|n| {
        if n == 2 {
            quit.request();
        }
    }));
    let mut tx = TimedTx {
        time: time.clone(),
        calls: Vec::new(),
    };
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);

    // SAFETY: fake but valid mbufs owned by the test.
    unsafe {
        run(
            worker(config(100, 1000.0, 0.0), 1),
            &mut rx,
            &mut tx,
            &mut free,
            &mut t,
            &quit,
            ticking_clock(&time, 50.0),
        )
    };

    assert_eq!(rx.calls.len(), 2, "rx stops once quit is observed");
    let sent: Vec<_> = tx.calls.iter().flat_map(|c| c.2.clone()).collect();
    assert_eq!(
        sent,
        p.to_vec(),
        "every packet (both batches) is emitted, in order"
    );
    assert!(free.freed.is_empty());
    for (at, _, bufs) in &tx.calls {
        assert!(
            *at >= 1000.0,
            "a tx_burst of {} packets at fake time {at}: packets left before their send time",
            bufs.len()
        );
    }
}

#[test]
fn a_refused_packet_is_freed_exactly_once_and_never_sent() {
    let mut m = FakeMbufs::new(9, 0, 64);
    let p = m.ptrs();
    let quit = Quit::new();
    let mut rx = MockRx::with_script(batches(&p, &[9]));
    rx.on_call = Some(Box::new(|_| quit.request()));
    let mut tx = MockTx::default();
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);

    // SAFETY: fake but valid mbufs owned by the test.
    unsafe {
        run(
            worker(config(100, 0.0, 1.0), 1), // 100 % loss
            &mut rx,
            &mut tx,
            &mut free,
            &mut t,
            &quit,
            fixed_clock(0.0),
        )
    };

    assert!(tx.calls.is_empty());
    assert_eq!(free.freed, p);
}

#[test]
fn every_mbuf_is_either_offered_to_the_nic_or_freed_exactly_once() {
    // 200 mbufs on both ports, awkward batch sizes, a tiny buffer so some
    // are refused, a NIC that sometimes refuses part of a burst, a delay so
    // the drain phase matters.
    let n = 200;
    let mut m = FakeMbufs::new(n, 0, 64);
    for i in 0..n {
        m.set_port(i, (i % 2) as u16);
    }
    let p = m.ptrs();
    let sizes = [7, 16, 1, 9, 16, 16, 3, 16, 16, 16, 16, 16, 16, 16, 15, 5];
    assert_eq!(sizes.iter().sum::<usize>(), n);
    let time = Rc::new(Cell::new(0.0));
    let quit = Quit::new();
    let mut rx = MockRx::with_script(batches(&p, &sizes));
    rx.on_call = Some(Box::new(|k| {
        if k == 40 {
            quit.request();
        }
    }));
    let mut tx = MockTx {
        accept_limit: 5,
        ..Default::default()
    };
    let mut free = MockFree::default();
    let (_stats, mut t) = telemetry(0.0);

    // SAFETY: fake but valid mbufs owned by the test.
    unsafe {
        run(
            worker(config(20, 300.0, 0.0), 2),
            &mut rx,
            &mut tx,
            &mut free,
            &mut t,
            &quit,
            ticking_clock(&time, 10.0),
        )
    };

    assert_eq!(rx.calls.len(), 40);
    assert!(rx.script.is_empty(), "the whole script was received");
    let offered: Vec<_> = tx.calls.iter().flat_map(|c| c.1.clone()).collect();
    let mut offered_sorted = offered.clone();
    offered_sorted.sort();
    offered_sorted.dedup();
    assert_eq!(offered_sorted.len(), offered.len(), "no mbuf offered twice");
    let mut freed_sorted = free.freed.clone();
    freed_sorted.sort();
    freed_sorted.dedup();
    assert_eq!(freed_sorted.len(), free.freed.len(), "no mbuf freed twice");
    // The tail of a partially accepted burst is offered AND freed; every
    // other mbuf is exactly one of the two.
    let mut all: Vec<_> = offered.iter().chain(free.freed.iter()).copied().collect();
    all.sort();
    all.dedup();
    let mut expected = p.clone();
    expected.sort();
    assert_eq!(all, expected, "an mbuf was neither sent nor freed");
    assert!(
        !free.freed.is_empty(),
        "the scenario must exercise the free paths"
    );
}

#[cfg(feature = "telemetry")]
mod counters {
    use dpds::consts::MAX_BURST_SIZE;
    use dpds::counters::{DpdsCounters, EPOCH_IDLE_GAP_NS};
    use dpds::first_loss::FirstLossReason;

    use super::*;

    /// A clock that only moves when the test moves it.
    fn manual_clock(time: &Rc<Cell<f64>>) -> impl FnMut() -> f64 {
        let time = time.clone();
        let reads = Cell::new(0u64);
        move || {
            reads.set(reads.get() + 1);
            assert!(reads.get() < 200_000, "the loop does not terminate");
            time.get()
        }
    }

    fn count_names() -> &'static [&'static str; 9] {
        &DpdsCounters::EPOCH_COUNTER_NAMES
    }
    fn idx(name: &str) -> usize {
        count_names().iter().position(|n| *n == name).unwrap()
    }

    #[test]
    fn the_loop_counts_polls_per_iteration_and_the_steps_count_the_rest() {
        let mut m = FakeMbufs::new(5, 0, 64);
        let p = m.ptrs();
        let quit = Quit::new();
        let mut rx = MockRx::with_script(batches(&p, &[5]));
        rx.on_call = Some(Box::new(|n| {
            if n == 3 {
                quit.request();
            }
        }));
        let mut tx = MockTx::default();
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);

        // SAFETY: fake but valid mbufs owned by the test.
        unsafe {
            run(
                worker(config(100, 0.0, 0.0), 1),
                &mut rx,
                &mut tx,
                &mut free,
                &mut t,
                &quit,
                fixed_clock(0.0),
            )
        };

        let c = &stats.counters;
        // 3 iterations that polled the NIC + the final one that observed quit
        assert_eq!(c.polls.get(), 4);
        assert_eq!(c.rx_pkts.get(), 5);
        assert_eq!(c.rx_full_bursts.get(), 0);
        assert_eq!(c.emulator_rejects.get(), 0);
        assert_eq!(c.pops.get(), 5);
        assert_eq!(c.tx_calls.get(), 1);
        assert_eq!(c.tx_pkts.get(), 5);
        assert_eq!(c.tx_sent.get(), 5);
        assert_eq!(c.tx_short_calls.get(), 0);
        assert_eq!(c.tx_batch_max.get(), 5);
        // Zero delay: every packet leaves in the step it arrived, and the
        // gauge is sampled after the pop.
        assert_eq!(c.buffer_len_max.get(), 0);
        assert!(stats.first_loss.current().is_none());
    }

    #[test]
    fn full_loss_counts_rejects_and_latches_emulator_reject() {
        let mut m = FakeMbufs::new(MAX_BURST_SIZE, 0, 64);
        let p = m.ptrs();
        let quit = Quit::new();
        let mut rx = MockRx::with_script(batches(&p, &[MAX_BURST_SIZE]));
        rx.on_call = Some(Box::new(|_| quit.request()));
        let mut tx = MockTx::default();
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);

        // SAFETY: fake but valid mbufs owned by the test.
        unsafe {
            run(
                worker(config(100, 0.0, 1.0), 1),
                &mut rx,
                &mut tx,
                &mut free,
                &mut t,
                &quit,
                fixed_clock(0.0),
            )
        };

        let c = &stats.counters;
        assert_eq!(c.rx_pkts.get(), 16);
        assert_eq!(c.rx_full_bursts.get(), 1);
        assert_eq!(c.emulator_rejects.get(), 16);
        assert_eq!(c.pops.get(), 0);
        assert_eq!(c.tx_calls.get(), 0);
        assert_eq!(
            stats.first_loss.current().unwrap().reason,
            FirstLossReason::EmulatorReject
        );
    }

    #[test]
    fn an_idle_gap_longer_than_500_ms_after_traffic_closes_exactly_one_epoch() {
        // The NIC serves 3 packets on poll 1; every poll moves the fake clock
        // by 200 ms (polls 1..6 -> 200..1200 ms); quit on poll 6.
        // Arrival at 200 ms; the gap exceeds 500 ms first at poll 4 (800 ms).
        let mut m = FakeMbufs::new(3, 0, 64);
        let p = m.ptrs();
        let time = Rc::new(Cell::new(0.0));
        let quit = Quit::new();
        let mut rx = MockRx::with_script(batches(&p, &[3]));
        {
            let time = time.clone();
            let quit = &quit;
            rx.on_call = Some(Box::new(move |n| {
                time.set(n as f64 * 200_000_000.0);
                if n == 6 {
                    quit.request();
                }
            }));
        }
        let mut tx = MockTx::default();
        let mut free = MockFree::default();
        let (stats, mut t) = telemetry(0.0);

        // SAFETY: fake but valid mbufs owned by the test.
        unsafe {
            run(
                worker(config(100, 0.0, 0.0), 1),
                &mut rx,
                &mut tx,
                &mut free,
                &mut t,
                &quit,
                manual_clock(&time),
            )
        };

        assert_eq!(
            stats.epochs.epochs_closed(),
            1,
            "one epoch, not a run of empty ones"
        );
        let r = stats.epochs.record(0).unwrap();
        assert_eq!(r.start_uptime_ns, 0);
        assert_eq!(
            r.end_uptime_ns, 800_000_000,
            "closed by the first poll past the gap"
        );
        assert!(r.end_uptime_ns as f64 - 200_000_000.0 > EPOCH_IDLE_GAP_NS);
        assert_eq!(r.counters[idx("rx_pkts")], 3);
        assert_eq!(r.counters[idx("pops")], 3);
        assert_eq!(r.counters[idx("tx_sent")], 3);
        assert_eq!(
            r.gauges[1], 0,
            "buffer_len_max of the epoch (zero delay, sampled after the pop)"
        );
        assert_eq!(stats.epochs.first_loss(0), Some(None), "a clean epoch");
    }
}
