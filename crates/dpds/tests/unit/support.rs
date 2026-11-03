//! Shared test doubles: fake mbufs, mock NIC/free implementations, clocks and
//! an ordered event log.
//!
//! A "fake mbuf" is a heap-allocated, zeroed `dpdk_sys::rte_mbuf` with `port`
//! and `pkt_len` set. It is only ever handed to code that must not call DPDK
//! on it (the mock `MbufFree` never dereferences it; the loop may prefetch
//! the header line, which is harmless on a valid heap address).
//!
//! The optional [`Log`] records every observable interaction of one loop run
//! in order (`Clock`, `Rx`, `Tx`, `Free`, `Quit`), so a test can assert the
//! exact per-iteration sequence: clock, RX, rejects freed,
//! TX, unsent freed.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use dpdk::RawMbuf;
use dpds::io::{MbufFree, RxBurst, TxBurst};
use dpds_core::Config;

/// One observable interaction of the loop with its environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ev {
    /// One read of the clock closure.
    Clock,
    /// `rx_burst(port, bufs)` with `bufs.len()`.
    Rx { port: u16, slots: usize },
    /// `tx_burst(port, bufs)` with `bufs.len()`.
    Tx { port: u16, n: usize },
    /// `free_bulk(mbufs)` with `mbufs.len()`.
    Free { n: usize },
    /// The test requested quit (pushed by the test's own hook).
    Quit,
}

pub type Log = Rc<RefCell<Vec<Ev>>>;

pub fn new_log() -> Log {
    Rc::new(RefCell::new(Vec::new()))
}

fn log_ev(log: &Option<Log>, ev: Ev) {
    if let Some(l) = log {
        l.borrow_mut().push(ev);
    }
}

/// Number of `Clock` events in the log.
pub fn clock_reads(log: &Log) -> usize {
    log.borrow().iter().filter(|e| **e == Ev::Clock).count()
}

pub struct FakeMbufs {
    mbufs: Box<[dpdk_sys::rte_mbuf]>,
}

impl FakeMbufs {
    /// `n` fake mbufs, all on `port`, all `pkt_len` bytes long.
    pub fn new(n: usize, port: u16, pkt_len: u32) -> FakeMbufs {
        // A boxed slice: the mbufs never move, so raw pointers to them stay valid.
        let mbufs = (0..n)
            .map(|_| {
                let mut m: dpdk_sys::rte_mbuf = unsafe { std::mem::zeroed() };
                m.__bindgen_anon_1.__bindgen_anon_1.port = port;
                m.__bindgen_anon_2.__bindgen_anon_1.pkt_len = pkt_len;
                m
            })
            .collect();
        FakeMbufs { mbufs }
    }

    pub fn ptr(&mut self, i: usize) -> RawMbuf {
        &mut self.mbufs[i] as *mut _
    }

    pub fn ptrs(&mut self) -> Vec<RawMbuf> {
        (0..self.mbufs.len()).map(|i| self.ptr(i)).collect()
    }

    /// Sets the packet length of mbuf `i` (what `rte_pktmbuf_pkt_len` reads).
    #[cfg(feature = "telemetry")]
    pub fn set_pkt_len(&mut self, i: usize, len: u32) {
        self.mbufs[i].__bindgen_anon_2.__bindgen_anon_1.pkt_len = len;
    }

    pub fn set_port(&mut self, i: usize, port: u16) {
        self.mbufs[i].__bindgen_anon_1.__bindgen_anon_1.port = port;
    }

    pub fn port(&self, i: usize) -> u16 {
        port_of(&self.mbufs[i] as *const _ as RawMbuf)
    }
}

pub fn port_of(m: RawMbuf) -> u16 {
    unsafe { (*m).__bindgen_anon_1.__bindgen_anon_1.port }
}

/// Mock RX: every `rx_burst` call pops the next scripted batch (an empty
/// script means "no packets"). Records `(port, bufs.len())` of every call.
/// `on_call(n)` (if set) runs at the start of the n-th call (1-based), so a
/// test can request quit from "inside the NIC".
#[derive(Default)]
pub struct MockRx<'a> {
    pub script: VecDeque<Vec<RawMbuf>>,
    pub calls: Vec<(u16, usize)>,
    pub on_call: Option<Box<dyn FnMut(usize) + 'a>>,
    pub log: Option<Log>,
}

impl<'a> MockRx<'a> {
    pub fn with_script(script: Vec<Vec<RawMbuf>>) -> MockRx<'a> {
        MockRx {
            script: script.into(),
            ..Default::default()
        }
    }
}

impl RxBurst for MockRx<'_> {
    fn rx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 {
        self.calls.push((port, bufs.len()));
        log_ev(
            &self.log,
            Ev::Rx {
                port,
                slots: bufs.len(),
            },
        );
        let n = self.calls.len();
        if let Some(f) = self.on_call.as_mut() {
            f(n);
        }
        let Some(next) = self.script.pop_front() else {
            return 0;
        };
        assert!(next.len() <= bufs.len(), "script batch exceeds buffer");
        bufs[..next.len()].copy_from_slice(&next);
        next.len() as u16
    }
}

/// Mock TX: accepts at most `accept_limit` mbufs per call (default: all).
/// Records `(port, mbufs passed)` for every call, in order.
pub struct MockTx {
    pub accept_limit: usize,
    pub calls: Vec<(u16, Vec<RawMbuf>)>,
    pub log: Option<Log>,
}

impl Default for MockTx {
    fn default() -> Self {
        MockTx {
            accept_limit: usize::MAX,
            calls: Vec::new(),
            log: None,
        }
    }
}

impl TxBurst for MockTx {
    fn tx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 {
        log_ev(
            &self.log,
            Ev::Tx {
                port,
                n: bufs.len(),
            },
        );
        self.calls.push((port, bufs.to_vec()));
        bufs.len().min(self.accept_limit) as u16
    }
}

impl MockTx {
    /// Every mbuf offered to the NIC, in call order.
    pub fn offered(&self) -> Vec<RawMbuf> {
        self.calls.iter().flat_map(|c| c.1.clone()).collect()
    }
}

/// Mock free: records every `free_bulk` call (the slice it was given).
#[derive(Default)]
pub struct MockFree {
    pub bulk_calls: Vec<Vec<RawMbuf>>,
    pub log: Option<Log>,
}

impl MbufFree for MockFree {
    unsafe fn free_bulk(&mut self, mbufs: &mut [RawMbuf]) {
        log_ev(&self.log, Ev::Free { n: mbufs.len() });
        self.bulk_calls.push(mbufs.to_vec());
    }
}

impl MockFree {
    /// Every freed mbuf, in call order.
    pub fn freed(&self) -> Vec<RawMbuf> {
        self.bulk_calls.iter().flatten().copied().collect()
    }
}

/// A clock returning `times[0]`, `times[1]`, ... (one value per read) and
/// then the last value forever. Counts its reads in the returned cell and
/// pushes `Ev::Clock` into `log` if given. Panics after an absurd number of
/// reads, so a loop that never terminates fails instead of hanging the suite.
pub fn scripted_clock(times: &[f64], log: Option<&Log>) -> (impl FnMut() -> f64, Rc<Cell<u64>>) {
    let reads = Rc::new(Cell::new(0u64));
    let r = reads.clone();
    let times = times.to_vec();
    let log = log.cloned();
    (
        move || {
            let i = r.get();
            r.set(i + 1);
            assert!(i < 200_000, "the loop does not terminate");
            log_ev(&log, Ev::Clock);
            times[(i as usize).min(times.len() - 1)]
        },
        reads,
    )
}

/// A clock starting at `start` that advances by `step` ns on every read
/// (the n-th read, 0-based, returns `start + n * step`).
pub fn ticking_clock(
    start: f64,
    step: f64,
    log: Option<&Log>,
) -> (impl FnMut() -> f64, Rc<Cell<u64>>) {
    let reads = Rc::new(Cell::new(0u64));
    let r = reads.clone();
    let log = log.cloned();
    (
        move || {
            let i = r.get();
            r.set(i + 1);
            assert!(i < 200_000, "the loop does not terminate");
            log_ev(&log, Ev::Clock);
            start + i as f64 * step
        },
        reads,
    )
}

// ── configs ─────────────────────────────────────────────────────────

/// A deterministic-delay config built from JSON (so the tests do not depend
/// on the exact field list of `Config`): no limiter, `Random(loss)` loss.
pub fn config(capacity: usize, delay_ns: f64, loss: f64) -> Config {
    config_full(capacity, false, delay_ns, loss, None)
}

/// As [`config`], with a `reordering` switch and an optional active Spacer
/// limiter at `rate_bps`. The limiter's limit is derived from the capacity
/// (`capacity * 1542 * 8` bit), there is nothing to configure.
pub fn config_full(
    capacity: usize,
    reordering: bool,
    delay_ns: f64,
    loss: f64,
    limiter: Option<f64>,
) -> Config {
    let (activate, rate_bps) = match limiter {
        Some(r) => (true, r),
        None => (false, 1e9),
    };
    serde_json::from_value(serde_json::json!({
        "buffer_capacity": capacity,
        "reordering": reordering,
        "delayer": {
            "distribution": "DeterministicDistribution",
            "mean_ns": delay_ns,
            "std_ns": 0.0,
            "activate_correlation": false,
            "half_life_period_ns": 1.0,
            "initialization_rate_pps": 15000000.0
        },
        "limiter": {
            "mode": "Spacer",
            "activate": activate,
            "rate_bps": rate_bps
        },
        "loss_model": { "Random": loss }
    }))
    .expect("test config must deserialize")
}

/// The general config builder of the drop-cause / delay-statistics tests.
/// `limiter` = `(mode, rate_bps)` with `mode` the JSON name
/// (`"Spacer"` / `"TokenBucketFilter"`); `loss` is the JSON of the loss model,
/// e.g. `json!({"Random": 0.0})` or `json!({"GilbertElliot": [1.0, 0.0, 0.0, 1.0, true]})`.
pub fn config_general(
    capacity: usize,
    reordering: bool,
    distribution: &str,
    mean_ns: f64,
    std_ns: f64,
    limiter: Option<(&str, f64)>,
    loss: serde_json::Value,
) -> Config {
    let (mode, activate, rate_bps) = match limiter {
        Some((m, r)) => (m, true, r),
        None => ("Spacer", false, 1e9),
    };
    serde_json::from_value(serde_json::json!({
        "buffer_capacity": capacity,
        "reordering": reordering,
        "delayer": {
            "distribution": distribution,
            "mean_ns": mean_ns,
            "std_ns": std_ns,
            "activate_correlation": false,
            "half_life_period_ns": 1.0,
            "initialization_rate_pps": 15000000.0
        },
        "limiter": {
            "mode": mode,
            "activate": activate,
            "rate_bps": rate_bps
        },
        "loss_model": loss,
        "seed": 1
    }))
    .expect("test config must deserialize")
}
