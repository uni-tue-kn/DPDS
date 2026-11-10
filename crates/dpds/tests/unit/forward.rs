//! The per-direction loop `dpds::forward::run` (unsafe fn: the RX mock hands
//! out fake mbufs that are valid and owned by the test). One call = ONE
//! direction: it receives on `worker.rx_port()` and transmits on
//! `worker.tx_port()`; the egress port is fixed, the mbuf `port` field is
//! never written.
//!
//! One iteration, in observable order:
//!  1. `quit` is read first;
//!  2. ONE clock read `now` (also in the iteration that sees quit). The only other read
//!     is `tx_ns` of the delay-statistics instance (telemetry build, delay stats on),
//!     immediately before a `tx_burst` and only in iterations that call it; not on the
//!     reject path, not for first-loss times, not for anything else;
//!  3. unless quit: ONE `rx_burst(rx_port, BURST_SIZE slots)`;
//!  4. every received mbuf is pushed with `now`; rejects are collected and
//!     freed with ONE `free_bulk` call (all rejects, in push order) after the
//!     RX loop; no call when there are none;
//!  5. due packets are popped with the same `now` into the free slots of the staging
//!     array that lives across iterations: at most `TX_BURST_SIZE - staged`;
//!  6. (prefetch: not observable);
//!  7. if `staged > 0` and (`staged == TX_BURST_SIZE` or the oldest staged packet,
//!     `staged[0]`, is `TX_MAX_WAIT_NS` old: `now - send_time >= TX_MAX_WAIT_NS`): ONE `tx_burst(tx_port, staged[..n])`, also in an iteration
//!     that popped nothing; the unsent tail is freed with ONE `free_bulk`; the staging
//!     array is empty afterwards;
//!  8. telemetry `buffer_len_max` after the pops, `tick(now, received)`;
//!  9. stop if quit was read as requested, the buffer is empty AND nothing is staged
//!     (the last partial batch leaves when its wait ran out, no flush on quit).
//!
//! `TX_BURST_SIZE` (64) is twice `BURST_SIZE` (32): RX delivers at most 32 per iteration,
//! but a backlog of due packets fills the whole staging array in one iteration.
//!
//! Consequences: once quit was observed nothing is received any more, what was received
//! is emitted at its send time plus at most the staging wait (drain) or freed, and the
//! loop terminates. A loop whose clock never gets `TX_MAX_WAIT_NS` past the send time of
//! a staged packet never terminates (the scripted clocks panic after 200 000 reads), so
//! the scenarios below choose their clocks accordingly: the default clock of [`Play`]
//! advances by `TX_MAX_WAIT_NS` per read, i.e. a zero-delay packet pushed in iteration
//! k leaves in iteration k + 1 (exactly on the `>=` boundary).
//!
//! The tests record the interactions in an ordered log (`support::Ev`) and
//! compare whole sequences. `Ev::Quit` is pushed by the test's hook inside
//! the `rx_burst` call that requests quit (after the `Rx` event).
//!
//! Without a delay every packet is due in the iteration it arrives
//! (`pop_due(now)` with send time == now). Tests use `DeterministicDistribution`
//! delays, so a packet pushed at `t` is due at exactly `(t + delay) as u64`.

use std::collections::HashSet;

use dpds::consts::{BURST_SIZE, TX_BURST_SIZE, TX_MAX_WAIT_NS};
use dpds::delay_stats::HistRange;
use dpds::forward::run;
use dpds::io::{MbufFree, RxBurst, TxBurst};
use dpds::quit::Quit;
use dpds::stats::{Stats, Telemetry};
use dpds::worker::Worker;
use dpds_core::buffer::Buffer;
use dpds_core::Config;

use crate::support::Ev::{Clock as C, Quit as Q};
use crate::support::*;

type Mbuf = dpdk::RawMbuf;

fn drive(
    w: Worker,
    rx: &mut impl RxBurst,
    tx: &mut impl TxBurst,
    free: &mut impl MbufFree,
    t: &mut Telemetry<'_>,
    quit: &Quit,
    now: impl FnMut() -> f64,
) {
    // SAFETY: fake but valid mbufs owned by the test; the mocks never
    // dereference them.
    unsafe { run(w, rx, tx, free, t, quit, now) };
}

fn rx_ev(port: u16) -> Ev {
    Ev::Rx {
        port,
        slots: BURST_SIZE,
    }
}

fn tx_ev(port: u16, n: usize) -> Ev {
    Ev::Tx { port, n }
}

fn free_ev(n: usize) -> Ev {
    Ev::Free { n }
}

fn batches(p: &[Mbuf], sizes: &[usize]) -> Vec<Vec<Mbuf>> {
    let mut out = Vec::new();
    let mut at = 0;
    for &n in sizes {
        out.push(p[at..at + n].to_vec());
        at += n;
    }
    assert_eq!(at, p.len(), "the sizes must use every mbuf");
    out
}

/// Which clock a scenario uses.
enum Clk {
    Fixed(f64),
    Script(Vec<f64>),
    /// `start`, `step`: read n returns `start + n * step`.
    Tick(f64, f64),
}

/// One run of the loop with every mock wired to one log.
struct Play {
    cfg: Config,
    rx_port: u16,
    tx_port: u16,
    script: Vec<Vec<Mbuf>>,
    /// Quit is requested inside the n-th `rx_burst` call (1-based); 0 means
    /// before the loop starts.
    quit_at: usize,
    accept: usize,
    clk: Clk,
    stats: &'static Stats,
    start_ns: f64,
}

struct Outcome {
    log: Log,
    rx_calls: Vec<(u16, usize)>,
    rx_batches_left: usize,
    tx: MockTx,
    free: MockFree,
    #[cfg(feature = "telemetry")]
    stats: &'static Stats,
}

impl Play {
    fn new(cfg: Config, script: Vec<Vec<Mbuf>>) -> Play {
        Play {
            cfg,
            rx_port: 0,
            tx_port: 0,
            script,
            quit_at: 1,
            accept: usize::MAX,
            clk: Clk::Tick(0.0, TX_MAX_WAIT_NS),
            // delay statistics off: the loop of the instance without them reads the
            // clock once per iteration in every build, so the logs are build-independent
            stats: Box::leak(Box::new(Stats::with_delay_stats(
                HistRange::default(),
                false,
            ))),
            start_ns: 0.0,
        }
    }

    fn ports(mut self, rx_port: u16, tx_port: u16) -> Play {
        self.rx_port = rx_port;
        self.tx_port = tx_port;
        self
    }

    fn quit_at(mut self, n: usize) -> Play {
        self.quit_at = n;
        self
    }

    fn accept(mut self, n: usize) -> Play {
        self.accept = n;
        self
    }

    fn clock(mut self, clk: Clk) -> Play {
        self.clk = clk;
        self
    }

    /// The direction's delay statistics on (telemetry builds): the loop reads the
    /// clock once more before every `tx_burst`.
    #[cfg(feature = "telemetry")]
    fn delay_stats(mut self) -> Play {
        self.stats = Box::leak(Box::new(Stats::with_range(HistRange::default())));
        self
    }

    fn run(self) -> Outcome {
        let log = new_log();
        let quit = Quit::new();
        let mut rx = MockRx::with_script(self.script);
        rx.log = Some(log.clone());
        if self.quit_at == 0 {
            quit.request();
        } else {
            let n = self.quit_at;
            let q = &quit;
            let l = log.clone();
            rx.on_call = Some(Box::new(move |k| {
                if k == n {
                    l.borrow_mut().push(Q);
                    q.request();
                }
            }));
        }
        let mut tx = MockTx {
            accept_limit: self.accept,
            log: Some(log.clone()),
            ..Default::default()
        };
        let mut free = MockFree {
            log: Some(log.clone()),
            ..Default::default()
        };
        let mut t = Telemetry::new(self.stats, self.start_ns);
        let clock: Box<dyn FnMut() -> f64> = match &self.clk {
            Clk::Fixed(v) => Box::new(scripted_clock(&[*v], Some(&log)).0),
            Clk::Script(v) => Box::new(scripted_clock(v, Some(&log)).0),
            Clk::Tick(s, d) => Box::new(ticking_clock(*s, *d, Some(&log)).0),
        };
        let w = Worker::new(Buffer::new(self.cfg), self.rx_port, self.tx_port);
        drive(w, &mut rx, &mut tx, &mut free, &mut t, &quit, clock);
        Outcome {
            log,
            rx_calls: rx.calls.clone(),
            rx_batches_left: rx.script.len(),
            tx,
            free,
            #[cfg(feature = "telemetry")]
            stats: self.stats,
        }
    }
}

impl Outcome {
    fn events(&self) -> Vec<Ev> {
        self.log.borrow().clone()
    }

    fn clock_reads(&self) -> usize {
        clock_reads(&self.log)
    }

    /// Fake time of every `tx_burst` call for a `Tick(start, step)` clock:
    /// the time of the iteration's single clock read.
    fn tx_times(&self, start: f64, step: f64) -> Vec<(f64, usize)> {
        let mut reads = 0usize;
        let mut out = Vec::new();
        for e in self.log.borrow().iter() {
            match e {
                C => reads += 1,
                Ev::Tx { n, .. } => out.push((start + (reads - 1) as f64 * step, *n)),
                _ => {}
            }
        }
        out
    }
}

// ── structure of an iteration ────────────────────────────────────────

#[test]
fn quit_already_requested_and_nothing_buffered_reads_the_clock_once_and_never_polls_rx() {
    let o = Play::new(config(100, 0.0, 0.0), vec![]).quit_at(0).run();
    assert_eq!(o.events(), [C], "one clock read, no rx_burst, no tx");
    assert!(o.rx_calls.is_empty());
    assert!(o.tx.calls.is_empty());
    assert!(o.free.bulk_calls.is_empty());
}

#[test]
fn an_idle_iteration_is_one_clock_read_and_one_rx_burst_only() {
    let o = Play::new(config(100, 0.0, 0.0), vec![]).quit_at(5).run();
    let mut expect = Vec::new();
    for _ in 0..5 {
        expect.extend([C, rx_ev(0)]);
    }
    expect.extend([Q, C]);
    assert_eq!(
        o.events(),
        expect,
        "5 idle polls, then the iteration that sees quit"
    );
    assert_eq!(o.clock_reads(), 6);
    assert!(o.tx.calls.is_empty() && o.free.bulk_calls.is_empty());
}

#[test]
fn a_partial_batch_is_staged_and_leaves_in_a_later_iteration_that_pops_nothing() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()]).run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), Q, C, tx_ev(0, 5)],
        "iteration 1: clock, rx, popped and staged, no tx; iteration 2 (clock +10 us, sees quit) \
         pops nothing and sends the staged batch, then the loop ends"
    );
    assert_eq!(o.tx.calls[0].1, p, "in order");
    assert!(o.free.bulk_calls.is_empty());
}

#[test]
fn rx_asks_for_exactly_burst_size_slots_every_time() {
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let o = Play::new(
        config(100, 0.0, 0.0),
        vec![p[..1].to_vec(), vec![], p[1..].to_vec()],
    )
    .quit_at(6)
    .run();
    assert_eq!(o.rx_calls.len(), 6);
    assert!(
        o.rx_calls.iter().all(|c| c.1 == BURST_SIZE),
        "{:?}",
        o.rx_calls
    );
    assert_eq!(BURST_SIZE, 32);
}

#[test]
fn a_burst_of_32_is_not_a_full_batch_it_waits_and_goes_out_in_one_tx_burst() {
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()]).run();
    assert_eq!(o.events(), [C, rx_ev(0), Q, C, tx_ev(0, 32)]);
    assert_eq!(o.tx.calls[0].1, p);
}

#[test]
fn the_staging_constants_are_the_documented_ones() {
    let _: usize = TX_BURST_SIZE;
    let _: f64 = TX_MAX_WAIT_NS;
    assert_eq!(TX_BURST_SIZE, 64);
    assert_eq!(BURST_SIZE, 32, "the RX burst size is unchanged");
    assert_eq!(TX_MAX_WAIT_NS, 10_000.0);
}

// ── staging ─────────────────────────────────────

#[test]
fn a_full_staging_array_of_64_is_sent_at_once_whatever_the_age() {
    // 32 + 32 zero-delay packets in two iterations, 1 ns apart: the second pop fills the staging
    // array (64 == TX_BURST_SIZE), so the tx happens in that iteration although the oldest staged
    // packet is 1 ns old.
    let mut m = FakeMbufs::new(TX_BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), batches(&p, &[32, 32]))
        .quit_at(2)
        .clock(Clk::Script(vec![0.0, 1.0, 2.0]))
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), C, rx_ev(0), Q, tx_ev(0, TX_BURST_SIZE), C]
    );
    assert_eq!(o.tx.calls[0].1, p);
    assert!(o.free.bulk_calls.is_empty());
}

#[test]
fn nothing_is_sent_until_the_oldest_staged_send_time_is_10_us_old() {
    // zero delay: packets pushed at 0 (3) and 1000 (2) have send times 0 and 1000. The clock
    // reads 0, 1000, W - 1 (age 9999: no tx), W (age exactly W: tx of all 5 in one burst), W.
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let w = TX_MAX_WAIT_NS;
    let o = Play::new(
        config(100, 0.0, 0.0),
        vec![p[..3].to_vec(), p[3..].to_vec()],
    )
    .quit_at(4)
    .clock(Clk::Script(vec![0.0, 1000.0, w - 1.0, w, w]))
    .run();
    assert_eq!(
        o.events(),
        [
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            Q,
            tx_ev(0, 5),
            C
        ],
        "no tx at ages 0, 1000 (oldest staged) and W - 1; one tx at exactly W"
    );
    assert_eq!(o.tx.calls[0].1, p, "send-time order");
}

#[test]
fn the_wait_is_measured_from_the_send_time_not_from_the_arrival_or_the_pop() {
    // delay 5000: pushed at 0, send time 5000; popped at 8000 (late). Age: 3000 at the pop,
    // 9999 at W + 4999, exactly W at W + 5000 = 15000. Measured from the arrival it would be
    // old enough already at 9999 + ..., measured from the pop only at 18000.
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 5000.0, 0.0), vec![p.clone()])
        .quit_at(4)
        .clock(Clk::Script(vec![0.0, 8000.0, 14_999.0, 15_000.0]))
        .run();
    assert_eq!(
        o.events(),
        [
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            Q,
            tx_ev(0, 3),
            C
        ]
    );
}

#[test]
fn the_pop_budget_is_limited_by_the_free_staging_slots() {
    // zero delay, cap 100. it1 (t=0): 32 pushed and popped (staged 32). it2 (t=1): 20 more
    // (staged 52). it3 (t=2): 30 more, 30 due, but only 64 - 52 = 12 slots are free: 12 are
    // popped, the staging array is full and goes out (the first 64 packets, in order). it4 (t=3,
    // quit seen): the other 18 are popped and wait; they leave once 10 us old (t = W + 2).
    let mut m = FakeMbufs::new(82, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), batches(&p, &[32, 20, 30]))
        .quit_at(3)
        .clock(Clk::Script(vec![0.0, 1.0, 2.0, 3.0, TX_MAX_WAIT_NS + 2.0]))
        .run();
    assert_eq!(
        o.events(),
        [
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            Q,
            tx_ev(0, 64),
            C,
            C,
            tx_ev(0, 18)
        ]
    );
    assert_eq!(o.tx.calls[0].1, p[..64].to_vec());
    assert_eq!(o.tx.calls[1].1, p[64..].to_vec());
}

#[test]
fn after_a_send_the_staging_is_empty_and_the_next_batch_is_a_separate_tx_burst() {
    // 5 packets, tx (the NIC takes 3, the 2 left are freed and not retried), then 5 more
    let mut m = FakeMbufs::new(10, 0, 64);
    let p = m.ptrs();
    let o = Play::new(
        config(100, 0.0, 0.0),
        vec![p[..5].to_vec(), vec![], p[5..].to_vec()],
    )
    .quit_at(3)
    .accept(3)
    .run();
    assert_eq!(
        o.events(),
        [
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            tx_ev(0, 5),
            free_ev(2),
            C,
            rx_ev(0),
            Q,
            C,
            tx_ev(0, 5),
            free_ev(2)
        ]
    );
    assert_eq!(o.tx.calls[0].1, p[..5].to_vec());
    assert_eq!(o.tx.calls[1].1, p[5..].to_vec(), "no leftover of batch 1");
    assert_eq!(o.free.bulk_calls[0], p[3..5].to_vec());
    assert_eq!(o.free.bulk_calls[1], p[8..].to_vec());
}

#[test]
fn quit_does_not_flush_the_staging_and_the_loop_ends_only_when_nothing_is_staged() {
    // quit during the first rx: the buffer is empty after the first pop, but 3 packets are
    // staged. Iterations 2-4 (ages 100, 200, 300) send nothing and read the clock once each;
    // iteration 5 (age W) sends and the loop ends right after it.
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()])
        .clock(Clk::Script(vec![0.0, 100.0, 200.0, 300.0, TX_MAX_WAIT_NS]))
        .run();
    assert_eq!(o.events(), [C, rx_ev(0), Q, C, C, C, C, tx_ev(0, 3)]);
    assert_eq!(o.clock_reads(), 5);
    assert_eq!(o.rx_calls.len(), 1);
}

// ── the mbuf's hash field ───────────────────────────────

#[test]
fn the_instance_without_delay_statistics_never_touches_the_mbuf_hash() {
    // accepted (and sent), rejected (cap 3) and unsent (NIC takes 1) packets alike
    let mut m = FakeMbufs::new(8, 0, 64);
    for i in 0..8 {
        m.set_hash(i, HASH_SENTINEL + i as u64);
    }
    let p = m.ptrs();
    let o = Play::new(config(3, 0.0, 0.0), vec![p]).accept(1).run();
    assert_eq!(o.tx.offered().len(), 3);
    assert_eq!(o.free.freed().len(), 5 + 2);
    for i in 0..8 {
        assert_eq!(m.hash(i), HASH_SENTINEL + i as u64, "mbuf {i}");
    }
}

// ── ports ────────────────────────────────────────────────────────────

#[test]
fn rx_uses_the_rx_port_and_tx_the_egress_port_of_the_direction() {
    for (rx_port, tx_port) in [(0u16, 0u16), (0, 1), (1, 0)] {
        let mut m = FakeMbufs::new(4, 0, 64);
        let p = m.ptrs();
        let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()])
            .ports(rx_port, tx_port)
            .run();
        assert_eq!(
            o.events(),
            [C, rx_ev(rx_port), Q, C, tx_ev(tx_port, 4)],
            "rx {rx_port} tx {tx_port}"
        );
        assert!(o.rx_calls.iter().all(|c| c.0 == rx_port));
        assert!(o.tx.calls.iter().all(|c| c.0 == tx_port));
    }
}

#[test]
fn the_mbuf_port_field_is_never_written() {
    // Whatever value the NIC left there, in every layout and on every path
    // (sent, rejected, unsent).
    for (rx_port, tx_port) in [(0u16, 0u16), (0, 1), (1, 0)] {
        let mut m = FakeMbufs::new(8, 0, 64);
        let before: Vec<u16> = (0..8).map(|i| (i * 3 + 1) as u16).collect();
        for (i, v) in before.iter().enumerate() {
            m.set_port(i, *v);
        }
        let p = m.ptrs();
        // cap 3: 5 rejects, 3 popped, the NIC takes 1 -> 2 unsent
        let o = Play::new(config(3, 0.0, 0.0), vec![p.clone()])
            .ports(rx_port, tx_port)
            .accept(1)
            .run();
        assert_eq!(o.tx.calls.len(), 1);
        assert_eq!(o.free.freed().len(), 5 + 2, "all three paths were taken");
        let after: Vec<u16> = (0..8).map(|i| m.port(i)).collect();
        assert_eq!(
            after, before,
            "rx {rx_port} tx {tx_port}: port field touched"
        );
    }
}

// ── quit / drain ─────────────────────────────────────────────────────

#[test]
fn everything_received_before_quit_comes_out_in_order_and_rx_stops_after_quit() {
    let mut m = FakeMbufs::new(15, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), batches(&p, &[5, 5, 5]))
        .quit_at(2)
        .ports(1, 0)
        .run();
    assert_eq!(
        o.rx_calls.len(),
        2,
        "no rx_burst after the quit was observed"
    );
    assert_eq!(o.rx_batches_left, 1, "the third batch was never received");
    assert_eq!(
        o.tx.offered(),
        p[..10].to_vec(),
        "all received packets, in order"
    );
    assert!(o.tx.calls.iter().all(|c| c.0 == 0));
    assert!(o.free.bulk_calls.is_empty());
    let ev = o.events();
    let quit_pos = ev.iter().position(|e| *e == Q).unwrap();
    assert!(
        ev[quit_pos..].iter().all(|e| !matches!(e, Ev::Rx { .. })),
        "{ev:?}"
    );
}

#[test]
fn a_batch_received_in_the_iteration_that_requested_quit_is_not_lost() {
    // quit is requested "during" the very first rx_burst: quit was read as
    // false at the top of that iteration, so its batch goes all the way through.
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()]).run();
    assert_eq!(o.rx_calls.len(), 1);
    assert_eq!(o.tx.offered(), p);
}

#[test]
fn delayed_packets_are_drained_after_quit_and_leave_one_wait_after_their_send_time() {
    // 10 packets ingested in iteration 1 (t = 0), 5 in iteration 2 (t = 50, quit is requested
    // during that rx), delay 1000 ns, clock +50 ns per read: iteration i reads (i - 1) * 50.
    // Due: 1000 (iteration 21) and 1050 (iteration 22), both staged. The oldest staged send
    // time (1000) is 10 us old at t = 11000, iteration 221: all 15 leave there in one burst.
    // A second clock read in any iteration would shift these times and the read count.
    let mut m = FakeMbufs::new(15, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 1000.0, 0.0), batches(&p, &[10, 5]))
        .quit_at(2)
        .clock(Clk::Tick(0.0, 50.0))
        .run();
    assert_eq!(o.rx_calls.len(), 2, "rx stops once quit is observed");
    assert_eq!(
        o.tx_times(0.0, 50.0),
        [(11_000.0, 15)],
        "one burst, when the oldest send time is TX_MAX_WAIT_NS old; no flush at quit time"
    );
    assert_eq!(o.tx.offered(), p, "every packet, in order");
    assert!(o.free.bulk_calls.is_empty());
    assert_eq!(
        o.clock_reads(),
        221,
        "exactly one clock read per iteration, drain included"
    );
}

#[test]
fn a_packet_is_due_at_its_send_time_exactly_and_not_before() {
    // delay 500, ingested at 1000 -> due 1500. Reads: 1000, 1499, 1500, ...
    let mut m = FakeMbufs::new(3, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 500.0, 0.0), vec![p.clone()])
        .quit_at(3)
        .clock(Clk::Script(vec![
            1000.0,
            1499.0,
            1500.0,
            1500.0 + TX_MAX_WAIT_NS,
        ]))
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), C, rx_ev(0), C, rx_ev(0), Q, C, tx_ev(0, 3)],
        "nothing at 1499, all three popped at 1500 (equal counts as due), sent 10 us later"
    );
    assert_eq!(o.tx.offered(), p);
}

#[test]
fn push_and_pop_use_the_same_now_so_a_zero_delay_packet_is_staged_in_its_own_iteration() {
    // Clock jumps by a lot on every read: the packet is old enough in the next iteration.
    let mut m = FakeMbufs::new(2, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()])
        .clock(Clk::Tick(1e9, 1e9))
        .run();
    assert_eq!(o.events(), [C, rx_ev(0), Q, C, tx_ev(0, 2)]);
}

// ── pop budget ───────────────────────────────────────────────────────

#[test]
fn due_packets_are_popped_until_the_staging_array_is_full_in_one_iteration() {
    // 70 packets ingested at t = 0 over three iterations (delay 1000), then everything is due
    // (t = 2000): it4 pops 64 (staging array full: tx at once), it5 pops the other 6 (age 1000:
    // no tx), it6 (t = 11000, age 10000) sends the 6.
    let mut m = FakeMbufs::new(70, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 1000.0, 0.0), batches(&p, &[32, 32, 6]))
        .quit_at(3)
        .clock(Clk::Script(vec![0.0, 0.0, 0.0, 2000.0, 2000.0, 11_000.0]))
        .run();
    assert_eq!(
        o.events(),
        [
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            C,
            rx_ev(0),
            Q,
            C,
            tx_ev(0, 64),
            C,
            C,
            tx_ev(0, 6),
        ]
    );
    assert_eq!(o.tx.offered(), p, "FIFO order across the splits");
}

#[test]
fn a_backlog_drains_while_rx_keeps_delivering_full_bursts() {
    // delay 1000, 32 packets per iteration. it1-4 (t = 0) build a backlog of 128 (due at 1000).
    // From it5 on the clock advances by 1000 per iteration, so each burst is due one iteration
    // later. Every iteration pops 64 while 64 are due: it5 (128 due), it6 (96 due), it7 (64 due)
    // each send 64. it8 has only 32 due (staged, no tx); it9 (quit seen, t = 6000) pops the last
    // 32 and fills the array. The old 32-per-iteration budget kept the backlog at 128 instead.
    let mut m = FakeMbufs::new(8 * BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(1000, 1000.0, 0.0), batches(&p, &[32; 8]))
        .quit_at(8)
        .clock(Clk::Script(vec![
            0.0, 0.0, 0.0, 0.0, 2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 1e9,
        ]))
        .run();
    let mut expect = vec![C, rx_ev(0), C, rx_ev(0), C, rx_ev(0), C, rx_ev(0)];
    for _ in 0..3 {
        expect.extend([C, rx_ev(0), tx_ev(0, TX_BURST_SIZE)]);
    }
    expect.extend([C, rx_ev(0), Q, C, tx_ev(0, TX_BURST_SIZE)]);
    assert_eq!(o.events(), expect);
    assert_eq!(o.tx.offered(), p, "FIFO order");
}

#[test]
fn the_backlog_goes_first_and_the_budget_covers_old_and_new_packets_together() {
    // iteration 1: 32 packets (delay 100, not due at t = 0)
    // iteration 2 (t = 1000): 32 more arrive, the 32 old ones are due -> they are popped and
    //   staged (not due: the new ones, ingested at 1000, due 1100)
    // iteration 3 (t = 1100): the new ones are popped, the staging array is full: one tx of 64,
    //   the old ones first.
    let mut m = FakeMbufs::new(64, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 100.0, 0.0), batches(&p, &[32, 32]))
        .quit_at(2)
        .clock(Clk::Script(vec![0.0, 1000.0, 1100.0]))
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), C, rx_ev(0), Q, C, tx_ev(0, TX_BURST_SIZE)]
    );
    assert_eq!(o.tx.calls[0].1, p);
}

// ── rejects: one bulk free after the RX loop ─────────────────────────

#[test]
fn rejects_are_freed_by_one_bulk_call_with_all_of_them_in_push_order() {
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    // 100 % loss: nothing is staged, so even a frozen clock ends the loop
    let o = Play::new(config(100, 0.0, 1.0), vec![p.clone()])
        .clock(Clk::Fixed(0.0))
        .run();
    assert_eq!(o.events(), [C, rx_ev(0), Q, free_ev(32), C]);
    assert_eq!(o.free.bulk_calls, [p], "one call, all 32 rejects, in order");
    assert!(o.tx.calls.is_empty(), "nothing popped: no tx_burst");
}

#[test]
fn rejects_are_freed_in_their_own_iteration_before_the_later_tx() {
    // cap 10, zero delay: all 32 are pushed before the pop, so the first 10
    // fit and the other 22 are rejected.
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(10, 0.0, 0.0), vec![p.clone()])
        .ports(0, 1)
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), Q, free_ev(22), C, tx_ev(1, 10)],
        "rejects freed after the rx loop; the 10 accepted ones are staged and leave in iteration 2"
    );
    assert_eq!(o.free.bulk_calls, [p[10..].to_vec()]);
    assert_eq!(o.tx.calls[0].1, p[..10].to_vec());
}

#[test]
fn each_iteration_frees_its_own_rejects_with_its_own_single_call() {
    // cap 10, delay 1e6: 10 stay, the rest of every batch is rejected.
    let mut m = FakeMbufs::new(64, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(10, 1e6, 0.0), batches(&p, &[32, 32]))
        .quit_at(2)
        .clock(Clk::Script(vec![0.0, 0.0, 2e6]))
        .run();
    assert_eq!(
        o.events(),
        [
            C,
            rx_ev(0),
            free_ev(22),
            C,
            rx_ev(0),
            Q,
            free_ev(32),
            C,
            tx_ev(0, 10)
        ]
    );
    assert_eq!(o.free.bulk_calls[0], p[10..32].to_vec());
    assert_eq!(o.free.bulk_calls[1], p[32..].to_vec());
    assert_eq!(o.tx.calls[0].1, p[..10].to_vec());
}

#[test]
fn a_limiter_refusal_uses_the_packet_length_and_is_freed_in_bulk() {
    // Spacer, capacity 2: the derived limit is 2 * 1542 * 8 = 24672 bit. A
    // 1519-byte packet is (1519 + 4 + 20) * 8 = 12344 bit: one fits, the second
    // (24688 bit) does not, although the buffer has room for it.
    let cfg = config_full(2, false, 0.0, 0.0, Some(1e9));
    let mut m = FakeMbufs::new(2, 0, 1519);
    let p = m.ptrs();
    let o = Play::new(cfg, vec![p.clone()])
        .clock(Clk::Script(vec![0.0, 1e6]))
        .run();
    assert_eq!(
        o.events()[..4],
        [C, rx_ev(0), Q, free_ev(1)],
        "the second packet is rejected, freed right after the rx loop"
    );
    assert_eq!(o.free.freed(), vec![p[1]]);
    assert_eq!(o.tx.offered(), vec![p[0]]);
}

// ── unsent: one bulk free after the TX ───────────────────────────────

#[test]
fn the_unsent_tail_is_freed_by_one_bulk_call_in_order() {
    let mut m = FakeMbufs::new(10, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()])
        .ports(0, 1)
        .accept(3)
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), Q, C, tx_ev(1, 10), free_ev(7)],
        "one clock read although the TX was short"
    );
    assert_eq!(o.free.bulk_calls, [p[3..].to_vec()]);
    assert_eq!(o.clock_reads(), 2);
}

#[test]
fn a_tx_that_takes_nothing_frees_the_whole_burst_once() {
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()])
        .accept(0)
        .run();
    assert_eq!(o.events(), [C, rx_ev(0), Q, C, tx_ev(0, 32), free_ev(32)]);
    assert_eq!(o.free.bulk_calls, [p]);
}

#[test]
fn a_tx_that_takes_everything_frees_nothing() {
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p])
        .accept(BURST_SIZE)
        .run();
    assert!(o.free.bulk_calls.is_empty());
    assert!(!o.events().iter().any(|e| matches!(e, Ev::Free { .. })));
}

#[test]
fn rejects_then_tx_then_unsent_make_two_separate_bulk_calls_in_that_order() {
    // cap 10, 32 received, the NIC takes 4 of the 10 popped.
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(10, 0.0, 0.0), vec![p.clone()])
        .accept(4)
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), Q, free_ev(22), C, tx_ev(0, 10), free_ev(6)]
    );
    assert_eq!(o.free.bulk_calls[0], p[10..].to_vec(), "the rejects");
    assert_eq!(o.free.bulk_calls[1], p[4..10].to_vec(), "the unsent tail");
}

// ── clock reads ──────────────────────────────────────────────────────

#[test]
fn the_clock_is_read_exactly_once_per_iteration_on_every_path() {
    // idle, reject, pop+send, short TX, drain: 8 rx calls (quit at 8) and
    // one more iteration that sees quit.
    let mut m = FakeMbufs::new(40, 0, 64);
    let p = m.ptrs();
    let script = vec![
        vec![],
        p[..32].to_vec(),
        vec![],
        p[32..36].to_vec(),
        p[36..40].to_vec(),
    ];
    let o = Play::new(config(6, 0.0, 0.0), script)
        .quit_at(8)
        .accept(2)
        .run();
    let ev = o.events();
    // Every rx_burst is directly preceded by a clock read and every clock
    // read before the quit is directly followed by an rx_burst.
    let quit_pos = ev.iter().position(|e| *e == Q).unwrap();
    for (i, e) in ev[..quit_pos].iter().enumerate() {
        if matches!(e, Ev::Rx { .. }) {
            assert_eq!(ev[i - 1], C, "{ev:?}");
        }
        if *e == C {
            assert!(matches!(ev[i + 1], Ev::Rx { .. }), "{ev:?}");
        }
    }
    assert_eq!(o.rx_calls.len(), 8);
    assert_eq!(
        o.clock_reads(),
        9,
        "8 iterations that polled rx + the iteration that saw quit: {ev:?}"
    );
    assert!(!o.tx.calls.is_empty() && !o.free.bulk_calls.is_empty());
}

#[test]
fn the_tx_error_path_and_the_reject_path_do_not_read_the_clock_again() {
    // First loss is recorded on both paths when telemetry is on; either
    // way it must reuse the iteration's `now`.
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(8, 0.0, 0.0), vec![p])
        .accept(0)
        .clock(Clk::Script(vec![123.0, 123.0 + TX_MAX_WAIT_NS]))
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), Q, free_ev(24), C, tx_ev(0, 8), free_ev(8)]
    );
    assert_eq!(o.clock_reads(), 2);
}

// ── ordering ────────────────────────────────────────────────────

#[test]
fn reordering_mode_with_constant_delay_keeps_the_entry_order_across_bursts_at_one_instant() {
    // Both batches enter at the same instant (t = 0): equal send times must
    // come out in entry order.
    let mut m = FakeMbufs::new(64, 0, 64);
    let p = m.ptrs();
    let o = Play::new(
        config_full(100, true, 1000.0, 0.0, None),
        batches(&p, &[32, 32]),
    )
    .quit_at(2)
    .clock(Clk::Script(vec![0.0, 0.0, 5000.0]))
    .run();
    assert_eq!(o.tx.offered(), p);
    assert_eq!(
        o.tx.calls.len(),
        1,
        "32 + 32 popped in two iterations fill the staging array"
    );
}

#[test]
fn reordering_mode_with_constant_delay_keeps_the_entry_order_across_instants() {
    let mut m = FakeMbufs::new(96, 0, 64);
    let p = m.ptrs();
    let o = Play::new(
        config_full(200, true, 1000.0, 0.0, None),
        batches(&p, &[32, 32, 32]),
    )
    .quit_at(3)
    .clock(Clk::Tick(0.0, 10.0))
    .run();
    assert_eq!(o.tx.offered(), p);
}

// ── conservation ─────────────────────────────────────────────────────

#[test]
fn every_mbuf_is_either_accepted_by_the_nic_or_freed_exactly_once() {
    // 200 mbufs, awkward batch sizes (<= 32), a tiny buffer so some are
    // rejected, a NIC that takes only 5 per burst, a delay so the drain phase
    // matters.
    let n = 200;
    let mut m = FakeMbufs::new(n, 0, 64);
    for i in 0..n {
        m.set_port(i, (i % 2) as u16);
    }
    let p = m.ptrs();
    let sizes = [7, 32, 1, 9, 32, 32, 3, 32, 32, 20];
    let o = Play::new(config(20, 300.0, 0.0), batches(&p, &sizes))
        .ports(1, 0)
        .quit_at(40)
        .accept(5)
        .clock(Clk::Tick(0.0, 10.0))
        .run();

    assert_eq!(o.rx_calls.len(), 40);
    assert_eq!(o.rx_batches_left, 0, "the whole script was received");

    let freed = o.free.freed();
    let freed_set: HashSet<_> = freed.iter().copied().collect();
    assert_eq!(freed_set.len(), freed.len(), "no mbuf freed twice");

    let mut accepted = HashSet::new();
    let mut offered = HashSet::new();
    for (_, bufs) in &o.tx.calls {
        assert!(bufs.len() <= TX_BURST_SIZE);
        for (i, m) in bufs.iter().enumerate() {
            assert!(offered.insert(*m), "mbuf offered to the NIC twice");
            if i < 5 {
                accepted.insert(*m);
            }
        }
    }
    assert!(
        accepted.is_disjoint(&freed_set),
        "an mbuf the NIC took was freed as well"
    );
    let all: HashSet<_> = accepted.union(&freed_set).copied().collect();
    let expected: HashSet<_> = p.iter().copied().collect();
    assert_eq!(all, expected, "an mbuf was neither sent nor freed");
    assert!(!freed.is_empty() && !accepted.is_empty());
    assert!(
        o.free.bulk_calls.iter().all(|c| !c.is_empty()),
        "no free_bulk call with an empty slice"
    );
}

// ── telemetry per direction ──────────────────────────────────────────

#[cfg(feature = "telemetry")]
mod counters {
    use dpds::counters::{DpdsCounters, EPOCH_IDLE_GAP_NS};
    use dpds::first_loss::{FirstLossReason, Stage, StageLossDetail};
    use dpds::stats::DirectionStats;

    use super::*;

    fn idx(name: &str) -> usize {
        DpdsCounters::EPOCH_COUNTER_NAMES
            .iter()
            .position(|n| *n == name)
            .unwrap()
    }

    #[test]
    fn the_loop_counts_polls_per_iteration_and_the_stages_count_the_rest() {
        let mut m = FakeMbufs::new(5, 0, 64);
        let p = m.ptrs();
        let o = Play::new(config(100, 0.0, 0.0), vec![p]).quit_at(3).run();
        let c = &o.stats.counters;
        // 3 iterations that polled the NIC + the final one that observed quit
        assert_eq!(c.polls.get(), 4);
        assert_eq!(c.rx_pkts.get(), 5);
        assert_eq!(c.rx_full_bursts.get(), 0);
        assert_eq!(c.dpds_rejects.get(), 0);
        assert_eq!(c.pops.get(), 5);
        assert_eq!(
            c.tx_calls.get(),
            1,
            "no tx_burst for iterations that popped nothing"
        );
        assert_eq!(c.tx_pkts.get(), 5);
        assert_eq!(c.tx_sent.get(), 5);
        assert_eq!(c.tx_short_calls.get(), 0);
        assert_eq!(c.tx_batch_max.get(), 5);
        // Zero delay: every packet leaves in the iteration it arrived, and the
        // gauge is sampled after the pop.
        assert_eq!(c.buffer_len_max.get(), 0);
        assert!(o.stats.first_loss.current().is_none());
    }

    #[test]
    fn rx_full_bursts_counts_only_bursts_of_exactly_32() {
        let mut m = FakeMbufs::new(32 + 31 + 32, 0, 64);
        let p = m.ptrs();
        let o = Play::new(config(200, 0.0, 0.0), batches(&p, &[32, 31, 32]))
            .quit_at(3)
            .run();
        let c = &o.stats.counters;
        assert_eq!(c.rx_pkts.get(), 95);
        assert_eq!(c.rx_full_bursts.get(), 2);
        // it1 stages 32 (t = 0); it2 (t = W) pops 31 -> 63 staged, the oldest is exactly W old:
        // tx of 63; it3 stages 32 (t = 2W); it4 (t = 3W) sends them.
        assert_eq!(c.tx_batch_max.get(), 63);
        assert_eq!(c.tx_calls.get(), 2);
        assert_eq!(c.tx_pkts.get(), 95);
    }

    #[test]
    fn full_loss_counts_rejects_and_latches_dpds_reject_with_the_iterations_now() {
        let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
        let p = m.ptrs();
        let mut play = Play::new(config(100, 0.0, 1.0), vec![p]).clock(Clk::Fixed(5_000.0));
        play.start_ns = 1_000.0;
        let o = play.run();
        let c = &o.stats.counters;
        assert_eq!(c.rx_pkts.get(), 32);
        assert_eq!(c.rx_full_bursts.get(), 1);
        assert_eq!(c.dpds_rejects.get(), 32);
        assert_eq!(c.pops.get(), 0);
        assert_eq!(c.tx_calls.get(), 0);
        let ev = o.stats.first_loss.current().unwrap();
        assert_eq!(ev.reason, FirstLossReason::DpdsReject);
        // the detail names the cause: here the loss model
        assert_eq!(
            dpds::first_loss::format_first_loss(Some(ev)).detail,
            "from=dpds,cause=loss"
        );
        assert_eq!(c.loss_drops.get(), 32);
        assert_eq!((c.limiter_drops.get(), c.buffer_full_drops.get()), (0, 0));
        assert_eq!(ev.relative_ns, 4_000.0, "now - start");
    }

    #[test]
    fn buffer_full_rejects_are_counted_too() {
        let mut m = FakeMbufs::new(10, 0, 64);
        let p = m.ptrs();
        let o = Play::new(config(4, 1e6, 0.0), vec![p])
            .clock(Clk::Script(vec![0.0, 2e6]))
            .run();
        let c = &o.stats.counters;
        assert_eq!(c.dpds_rejects.get(), 6);
        assert_eq!(c.buffer_full_drops.get(), 6, "capacity 4 of 10 packets");
        assert_eq!((c.loss_drops.get(), c.limiter_drops.get()), (0, 0));
        assert_eq!(
            dpds::first_loss::format_first_loss(o.stats.first_loss.current()).detail,
            "from=dpds,cause=buffer_full"
        );
        assert_eq!(
            c.buffer_len_max.get(),
            4,
            "the gauge never exceeds the capacity"
        );
        assert_eq!(c.pops.get(), 4);
    }

    #[test]
    fn a_short_tx_counts_drops_and_latches_descriptor_full_with_the_iterations_now() {
        let mut m = FakeMbufs::new(10, 0, 64);
        let p = m.ptrs();
        let mut play = Play::new(config(100, 0.0, 0.0), vec![p])
            .ports(0, 1)
            .accept(3)
            .clock(Clk::Script(vec![5_000.0, 5_000.0 + TX_MAX_WAIT_NS]));
        play.start_ns = 1_000.0;
        let o = play.run();
        let c = &o.stats.counters;
        assert_eq!(c.tx_calls.get(), 1);
        assert_eq!(c.tx_pkts.get(), 10);
        assert_eq!(c.tx_sent.get(), 3);
        assert_eq!(c.tx_short_calls.get(), 1);
        assert_eq!(c.tx_batch_max.get(), 10);
        assert_eq!(c.pops.get(), 10);
        let ev = o.stats.first_loss.current().unwrap();
        assert_eq!(ev.reason, FirstLossReason::DescriptorFull);
        assert_eq!(
            ev.detail,
            StageLossDetail::RingFull {
                from: Stage::Dpds,
                to: Stage::Nic
            }
        );
        assert_eq!(
            ev.relative_ns, 14_000.0,
            "the now of the iteration that sent (5000 + W - 1000), no extra clock read"
        );
        let r = o.stats.stats_reply(0);
        let drops = r.iter().find(|(k, _)| k == "tx_drops").unwrap();
        assert_eq!(drops.1, dpds::reply::Value::Uint(7));
    }

    #[test]
    fn the_first_loss_of_an_iteration_is_the_reject_that_precedes_the_tx_error() {
        let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
        let p = m.ptrs();
        let o = Play::new(config(10, 0.0, 0.0), vec![p]).accept(4).run();
        let ev = o.stats.first_loss.current().unwrap();
        assert_eq!(ev.reason, FirstLossReason::DpdsReject);
    }

    #[test]
    fn no_loss_no_first_loss() {
        let mut m = FakeMbufs::new(3, 0, 64);
        let p = m.ptrs();
        let o = Play::new(config(100, 0.0, 0.0), vec![p]).run();
        assert!(o.stats.first_loss.current().is_none());
    }

    #[test]
    fn tx_batch_max_is_the_largest_burst_over_iterations() {
        let mut m = FakeMbufs::new(10, 0, 64);
        let p = m.ptrs();
        let o = Play::new(
            config(100, 0.0, 0.0),
            vec![p[..7].to_vec(), vec![], p[7..].to_vec()],
        )
        .quit_at(3)
        .run();
        let c = &o.stats.counters;
        assert_eq!(c.tx_batch_max.get(), 7);
        assert_eq!(c.tx_calls.get(), 2);
        assert_eq!(c.tx_pkts.get(), 10);
    }

    #[test]
    fn buffer_len_max_is_a_high_water_mark_sampled_on_every_iteration_even_idle_ones() {
        let mut m = FakeMbufs::new(5, 0, 64);
        let p = m.ptrs();
        let o = Play::new(
            config(100, 1000.0, 0.0),
            vec![p[..2].to_vec(), vec![], p[2..].to_vec()],
        )
        .quit_at(3)
        .clock(Clk::Script(vec![0.0, 0.0, 0.0, 5000.0, 11_000.0]))
        .run();
        let c = &o.stats.counters;
        assert_eq!(
            c.buffer_len_max.get(),
            5,
            "2 + 3 waiting, mark stays after the drain"
        );
        assert_eq!(c.pops.get(), 5);
    }

    #[test]
    fn buffer_len_max_is_sampled_after_the_pop_so_a_pass_through_reports_zero() {
        let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
        let p = m.ptrs();
        let o = Play::new(config(100, 0.0, 0.0), vec![p]).run();
        assert_eq!(o.stats.counters.buffer_len_max.get(), 0);
    }

    #[test]
    fn directions_have_independent_counters_and_first_loss() {
        let ds: &'static DirectionStats = Box::leak(Box::new(DirectionStats::new(2)));
        // direction 0: 100 % loss, direction 1: lossless
        let mut m0 = FakeMbufs::new(6, 0, 64);
        let p0 = m0.ptrs();
        let mut play0 = Play::new(config(100, 0.0, 1.0), vec![p0.clone()]).ports(0, 1);
        play0.stats = &ds.stats[0];
        let o0 = play0.run();

        let mut m1 = FakeMbufs::new(9, 1, 64);
        let p1 = m1.ptrs();
        let mut play1 = Play::new(config(100, 0.0, 0.0), vec![p1.clone()]).ports(1, 0);
        play1.stats = &ds.stats[1];
        let o1 = play1.run();

        let (c0, c1) = (&ds.stats[0].counters, &ds.stats[1].counters);
        assert_eq!(c0.rx_pkts.get(), 6);
        assert_eq!(c0.dpds_rejects.get(), 6);
        assert_eq!(c0.tx_sent.get(), 0);
        assert_eq!(c1.rx_pkts.get(), 9);
        assert_eq!(c1.dpds_rejects.get(), 0);
        assert_eq!(c1.tx_sent.get(), 9);
        assert!(ds.stats[0].first_loss.current().is_some());
        assert!(ds.stats[1].first_loss.current().is_none());
        assert_eq!(o0.free.freed(), p0);
        assert_eq!(o1.tx.calls[0].0, 0, "direction 1 leaves on port 0");
    }

    #[test]
    fn an_idle_gap_longer_than_500_ms_after_traffic_closes_exactly_one_epoch() {
        // Iteration i reads t = i * 200 ms. 3 packets arrive in iteration 1
        // (200 ms); the gap exceeds 500 ms first in iteration 4 (800 ms);
        // quit during the 6th rx -> the last iteration is the 7th.
        let mut m = FakeMbufs::new(3, 0, 64);
        let p = m.ptrs();
        let times: Vec<f64> = (1..=7).map(|i| i as f64 * 200_000_000.0).collect();
        let o = Play::new(config(100, 0.0, 0.0), vec![p])
            .quit_at(6)
            .clock(Clk::Script(times))
            .run();
        let stats = o.stats;
        assert_eq!(
            stats.epochs.epochs_closed(),
            1,
            "one epoch, not a run of empty ones"
        );
        let r = stats.epochs.record(0).unwrap();
        assert_eq!(r.start_uptime_ns, 0);
        assert_eq!(
            r.end_uptime_ns, 800_000_000,
            "closed by the first iteration past the gap"
        );
        assert!(r.end_uptime_ns as f64 - 200_000_000.0 > EPOCH_IDLE_GAP_NS);
        assert_eq!(r.counters[idx("rx_pkts")], 3);
        assert_eq!(r.counters[idx("pops")], 3);
        assert_eq!(r.counters[idx("tx_sent")], 3);
        assert_eq!(r.gauges[1], 0, "buffer_len_max of the epoch");
        assert_eq!(stats.epochs.first_loss(0), Some(None), "a clean epoch");
    }

    #[test]
    fn pops_are_counted_at_pop_time_and_the_tx_counters_at_send_time() {
        // it1 pops 3 packets (staged, no tx); at the start of the next rx call (it2) the pops are
        // counted and no tx counter has moved. After the send (it2) the tx counters are there.
        let mut m = FakeMbufs::new(3, 0, 64);
        let p = m.ptrs();
        let stats: &'static Stats = Box::leak(Box::new(Stats::with_delay_stats(
            HistRange::default(),
            false,
        )));
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let quit = Quit::new();
        let mut rx = MockRx::with_script(vec![p.clone()]);
        {
            let (q, seen) = (&quit, seen.clone());
            rx.on_call = Some(Box::new(move |k| {
                let c = &stats.counters;
                seen.borrow_mut().push((
                    c.pops.get(),
                    c.tx_calls.get(),
                    c.tx_pkts.get(),
                    c.tx_sent.get(),
                    c.tx_batch_max.get(),
                ));
                if k == 2 {
                    q.request();
                }
            }));
        }
        let (mut tx, mut free) = (MockTx::default(), MockFree::default());
        let mut t = Telemetry::new(stats, 0.0);
        let (clock, _) = ticking_clock(0.0, TX_MAX_WAIT_NS, None);
        let w = Worker::new(Buffer::new(config(100, 0.0, 0.0)), 0, 0);
        drive(w, &mut rx, &mut tx, &mut free, &mut t, &quit, clock);
        assert_eq!(
            *seen.borrow(),
            [(0, 0, 0, 0, 0), (3, 0, 0, 0, 0)],
            "(pops, tx_calls, tx_pkts, tx_sent, tx_batch_max) at the start of rx calls 1 and 2"
        );
        let c = &stats.counters;
        assert_eq!(c.pops.get(), 3);
        assert_eq!(
            (c.tx_calls.get(), c.tx_pkts.get(), c.tx_sent.get()),
            (1, 3, 3)
        );
        assert_eq!(c.tx_batch_max.get(), 3);
    }

    #[test]
    fn tx_counters_are_per_tx_burst_call_including_short_ones() {
        // two batches of 5, the NIC takes 3 of each
        let mut m = FakeMbufs::new(10, 0, 64);
        let p = m.ptrs();
        let o = Play::new(
            config(100, 0.0, 0.0),
            vec![p[..5].to_vec(), vec![], p[5..].to_vec()],
        )
        .quit_at(3)
        .accept(3)
        .run();
        let c = &o.stats.counters;
        assert_eq!(c.pops.get(), 10);
        assert_eq!(c.tx_calls.get(), 2);
        assert_eq!(c.tx_pkts.get(), 10);
        assert_eq!(c.tx_sent.get(), 6);
        assert_eq!(c.tx_short_calls.get(), 2);
        assert_eq!(c.tx_batch_max.get(), 5);
    }

    #[test]
    fn with_delay_stats_the_loop_reads_the_clock_once_more_right_before_each_tx_burst() {
        // The scenario of `nothing_is_sent_until_the_oldest_staged_send_time_is_10_us_old`:
        // only the iteration that sends has a second read, placed directly before the tx.
        let mut m = FakeMbufs::new(5, 0, 64);
        let p = m.ptrs();
        let w = TX_MAX_WAIT_NS;
        let o = Play::new(
            config(100, 0.0, 0.0),
            vec![p[..3].to_vec(), p[3..].to_vec()],
        )
        .quit_at(4)
        .clock(Clk::Script(vec![
            0.0,
            1000.0,
            w - 1.0,
            w,
            w + 500.0,
            w + 500.0,
        ]))
        .delay_stats()
        .run();
        assert_eq!(
            o.events(),
            [
                C,
                rx_ev(0),
                C,
                rx_ev(0),
                C,
                rx_ev(0),
                C,
                rx_ev(0),
                Q,
                C,
                tx_ev(0, 5),
                C
            ]
        );
        assert_eq!(o.clock_reads(), 6);
    }

    #[test]
    fn with_delay_stats_the_extra_read_follows_the_reject_free_and_precedes_the_tx() {
        // cap 10 of 32: the rejects are freed, then clock, then tx; the short tx frees its tail
        let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
        let p = m.ptrs();
        let o = Play::new(config(10, 0.0, 0.0), vec![p])
            .accept(4)
            .delay_stats()
            .run();
        assert_eq!(
            o.events(),
            [C, rx_ev(0), Q, free_ev(22), C, C, tx_ev(0, 10), free_ev(6)],
            "iteration 2: its own read, then the tx read directly before tx_burst"
        );
        let ev = o.events();
        let tx_pos = ev.iter().position(|e| matches!(e, Ev::Tx { .. })).unwrap();
        assert_eq!(ev[tx_pos - 1], C);
    }

    #[test]
    fn with_delay_stats_the_extra_read_does_not_change_the_wait_decision_or_first_loss_time() {
        // the send decision and the first-loss time use the iteration's `now` (read 2), the
        // statistics the extra read (read 3)
        let mut m = FakeMbufs::new(4, 0, 64);
        let p = m.ptrs();
        let mut play = Play::new(config(100, 0.0, 0.0), vec![p])
            .accept(1)
            .clock(Clk::Script(vec![0.0, TX_MAX_WAIT_NS, 3.0 * TX_MAX_WAIT_NS]))
            .delay_stats();
        play.start_ns = 0.0;
        let o = play.run();
        assert_eq!(o.tx.calls.len(), 1);
        let ev = o.stats.first_loss.current().unwrap();
        assert_eq!(ev.reason, FirstLossReason::DescriptorFull);
        assert_eq!(ev.relative_ns, TX_MAX_WAIT_NS, "the iteration's now");
    }

    #[test]
    fn the_delay_instance_stores_the_push_time_in_accepted_mbufs_only() {
        // cap 3 of 5 packets; the iteration's now is 1234.5 (read 1), later reads are bigger:
        // the 3 accepted mbufs carry 1234.5 as f64, the 2 rejected ones keep their sentinel.
        let mut m = FakeMbufs::new(5, 0, 64);
        for i in 0..5 {
            m.set_hash(i, HASH_SENTINEL);
        }
        let p = m.ptrs();
        let o = Play::new(config(3, 0.0, 0.0), vec![p])
            .clock(Clk::Script(vec![1234.5, 1234.5 + TX_MAX_WAIT_NS, 1e6]))
            .delay_stats()
            .run();
        assert_eq!(o.tx.offered().len(), 3);
        for i in 0..3 {
            assert_eq!(f64::from_bits(m.hash(i)), 1234.5, "accepted mbuf {i}");
        }
        for i in 3..5 {
            assert_eq!(m.hash(i), HASH_SENTINEL, "rejected mbuf {i}");
        }
    }
}
