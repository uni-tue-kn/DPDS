//! The per-direction loop `dpds::forward::run` (unsafe fn: the RX mock hands
//! out fake mbufs that are valid and owned by the test). One call = ONE
//! direction: it receives on `worker.rx_port()` and transmits on
//! `worker.tx_port()`; the egress port is fixed, the mbuf `port` field is
//! never written.
//!
//! One iteration, in observable order:
//!  1. `quit` is read first;
//!  2. ONE clock read `now` (also in the iteration that sees quit, and no
//!     other read anywhere in the loop, not on the reject path, not on the
//!     TX-error path, not for telemetry);
//!  3. unless quit: ONE `rx_burst(rx_port, BURST_SIZE slots)`;
//!  4. every received mbuf is pushed with `now`; rejects are collected and
//!     freed with ONE `free_bulk` call (all rejects, in push order) after the
//!     RX loop; no call when there are none;
//!  5. up to `BURST_SIZE` (32) due packets are popped with the same `now`;
//!  6. (prefetch: not observable);
//!  7. if k > 0: ONE `tx_burst(tx_port, popped[..k])` in the SAME iteration;
//!     the unsent tail `popped[sent..k]` is freed with ONE `free_bulk`;
//!  8. telemetry `buffer_len_max` after the pops, `tick(now, received)`;
//!  9. stop if quit was read as requested and the buffer is empty.
//!
//! Consequences: once quit was observed nothing is received any more, what
//! was received is emitted at its send time (drain) or freed, and the loop
//! terminates.
//!
//! The tests record the interactions in an ordered log (`support::Ev`) and
//! compare whole sequences. `Ev::Quit` is pushed by the test's hook inside
//! the `rx_burst` call that requests quit (after the `Rx` event).
//!
//! Without a delay every packet is due in the iteration it arrives
//! (`pop_due(now)` with send time == now). Tests use `DeterministicDistribution`
//! delays, so a packet pushed at `t` is due at exactly `(t + delay) as u64`.

use std::collections::HashSet;

use dpds::consts::BURST_SIZE;
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
            clk: Clk::Fixed(0.0),
            stats: Box::leak(Box::default()),
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
fn the_clock_is_read_before_rx_and_tx_happens_in_the_same_iteration_as_the_pop() {
    let mut m = FakeMbufs::new(5, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()]).run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), Q, tx_ev(0, 5), C],
        "iteration 1: clock, rx, tx (no deferral to the next iteration); iteration 2 sees quit"
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
fn a_full_burst_of_32_is_taken_through_in_one_tx_burst() {
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()]).run();
    assert_eq!(o.events(), [C, rx_ev(0), Q, tx_ev(0, 32), C]);
    assert_eq!(o.tx.calls[0].1, p);
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
            [C, rx_ev(rx_port), Q, tx_ev(tx_port, 4), C],
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
fn delayed_packets_are_drained_after_quit_at_their_send_time_one_clock_read_per_iteration() {
    // 10 packets ingested in iteration 1 (t = 0), 5 in iteration 2 (t = 50,
    // quit is requested during that rx), delay 1000 ns, clock +50 ns per read.
    // Iteration i reads t = (i - 1) * 50. Due: 1000 (iteration 21) and
    // 1050 (iteration 22). A second clock read in any iteration would shift
    // these times and the read count.
    let mut m = FakeMbufs::new(15, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 1000.0, 0.0), batches(&p, &[10, 5]))
        .quit_at(2)
        .clock(Clk::Tick(0.0, 50.0))
        .run();
    assert_eq!(o.rx_calls.len(), 2, "rx stops once quit is observed");
    assert_eq!(
        o.tx_times(0.0, 50.0),
        [(1000.0, 10), (1050.0, 5)],
        "packets leave exactly when due, not flushed at quit time"
    );
    assert_eq!(o.tx.offered(), p, "every packet, in order");
    assert!(o.free.bulk_calls.is_empty());
    assert_eq!(
        o.clock_reads(),
        22,
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
        .clock(Clk::Script(vec![1000.0, 1499.0, 1500.0]))
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), C, rx_ev(0), C, rx_ev(0), Q, tx_ev(0, 3), C],
        "nothing at 1499, all three at 1500 (equal counts as due)"
    );
    assert_eq!(o.tx.offered(), p);
}

#[test]
fn push_and_pop_use_the_same_now_so_a_zero_delay_packet_leaves_in_its_own_iteration() {
    // Clock jumps by a lot on every read; the packet must still go out in
    // the iteration it arrived in (a second read would also work, but a
    // stale/earlier pop time would not).
    let mut m = FakeMbufs::new(2, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), vec![p.clone()])
        .clock(Clk::Tick(1e9, 1e9))
        .run();
    assert_eq!(o.events(), [C, rx_ev(0), Q, tx_ev(0, 2), C]);
}

// ── pop budget ───────────────────────────────────────────────────────

#[test]
fn at_most_32_packets_are_popped_and_sent_per_iteration() {
    // 70 packets ingested at t = 0 over three iterations (delay 1000), then
    // everything is due (t = 2000): 32 + 32 + 6 in FIFO order, one TX per
    // iteration, one clock read per iteration.
    let mut m = FakeMbufs::new(70, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 1000.0, 0.0), batches(&p, &[32, 32, 6]))
        .quit_at(3)
        .clock(Clk::Script(vec![0.0, 0.0, 0.0, 2000.0]))
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
            tx_ev(0, 32),
            C,
            tx_ev(0, 32),
            C,
            tx_ev(0, 6),
        ]
    );
    assert_eq!(o.tx.offered(), p, "FIFO order across the budget splits");
}

#[test]
fn the_backlog_goes_first_and_the_budget_covers_old_and_new_packets_together() {
    // iteration 1: 32 packets (delay 100, not due at t = 0)
    // iteration 2 (t = 1000): 32 more arrive, the 32 old ones are due ->
    //   pop budget 32 -> exactly the old ones leave, the new ones are not
    //   due yet (ingested at 1000, due 1100).
    let mut m = FakeMbufs::new(64, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 100.0, 0.0), batches(&p, &[32, 32]))
        .quit_at(2)
        .clock(Clk::Script(vec![0.0, 1000.0, 1100.0]))
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), C, rx_ev(0), Q, tx_ev(0, 32), C, tx_ev(0, 32)]
    );
    assert_eq!(o.tx.calls[0].1, p[..32].to_vec());
    assert_eq!(o.tx.calls[1].1, p[32..].to_vec());
}

// ── rejects: one bulk free after the RX loop ─────────────────────────

#[test]
fn rejects_are_freed_by_one_bulk_call_with_all_of_them_in_push_order() {
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 1.0), vec![p.clone()]).run(); // 100 % loss
    assert_eq!(o.events(), [C, rx_ev(0), Q, free_ev(32), C]);
    assert_eq!(o.free.bulk_calls, [p], "one call, all 32 rejects, in order");
    assert!(o.tx.calls.is_empty(), "nothing popped: no tx_burst");
}

#[test]
fn rejects_and_pops_of_one_iteration_rejects_are_freed_before_the_tx() {
    // cap 10, zero delay: all 32 are pushed before the pop, so the first 10
    // fit and the other 22 are rejected.
    let mut m = FakeMbufs::new(BURST_SIZE, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(10, 0.0, 0.0), vec![p.clone()])
        .ports(0, 1)
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), Q, free_ev(22), tx_ev(1, 10), C],
        "rejects freed after the rx loop, then the tx of the same iteration"
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
    // Spacer, 1500 bit limit: one 100-byte packet fits, the second does not.
    let cfg = config_full(100, false, 0.0, 0.0, Some((1e9, 1500.0)));
    let mut m = FakeMbufs::new(2, 0, 100);
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
        [C, rx_ev(0), Q, tx_ev(1, 10), free_ev(7), C],
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
    assert_eq!(o.events(), [C, rx_ev(0), Q, tx_ev(0, 32), free_ev(32), C]);
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
        [C, rx_ev(0), Q, free_ev(22), tx_ev(0, 10), free_ev(6), C]
    );
    assert_eq!(o.free.bulk_calls[0], p[10..].to_vec(), "the rejects");
    assert_eq!(o.free.bulk_calls[1], p[4..10].to_vec(), "the unsent tail");
}

#[test]
fn unsent_packets_of_different_iterations_are_not_coalesced() {
    let mut m = FakeMbufs::new(10, 0, 64);
    let p = m.ptrs();
    let o = Play::new(config(100, 0.0, 0.0), batches(&p, &[5, 5]))
        .quit_at(2)
        .accept(3)
        .run();
    assert_eq!(
        o.events(),
        [
            C,
            rx_ev(0),
            tx_ev(0, 5),
            free_ev(2),
            C,
            rx_ev(0),
            Q,
            tx_ev(0, 5),
            free_ev(2),
            C
        ]
    );
    assert_eq!(o.free.bulk_calls[0], p[3..5].to_vec());
    assert_eq!(o.free.bulk_calls[1], p[8..].to_vec());
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
        .clock(Clk::Script(vec![123.0]))
        .run();
    assert_eq!(
        o.events(),
        [C, rx_ev(0), Q, free_ev(24), tx_ev(0, 8), free_ev(8), C]
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
    assert_eq!(o.tx.calls.len(), 2);
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
        assert!(bufs.len() <= BURST_SIZE);
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
        assert_eq!(c.tx_batch_max.get(), 32);
        assert_eq!(c.tx_calls.get(), 3);
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
            .clock(Clk::Fixed(5_000.0));
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
        assert_eq!(ev.relative_ns, 4_000.0, "the same now, no extra clock read");
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
        let o = Play::new(config(100, 0.0, 0.0), batches(&p, &[7, 3]))
            .quit_at(2)
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
        .clock(Clk::Script(vec![0.0, 0.0, 0.0, 5000.0]))
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
}
