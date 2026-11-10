//! Microbenchmark of the per-packet cost of `dpds::forward::run`, driven by
//! mocks (no NIC, no EAL): the RX mock hands out a burst of fake mbufs per
//! call, the TX mock accepts everything, the free mock does nothing and the
//! clock advances by a fixed step per iteration. The mbufs are only read for
//! their `pkt_len` and header prefetch (and written for the arrival stamp).
//!
//! The first four cases are the original ones: a pool of 32 mbufs (always
//! L1-hot), full bursts of 32, ~hundreds of packets queued. The `10 ms` cases
//! are the realistic-load ones: 8.1 Mpps and 10 ms delay keep ~80 000 packets
//! queued, handed out from a pool of 131 072 mbufs spaced like the elements of a
//! real mempool, so every header line is cache-cold when the packet pops. The
//! `4 per RX call` case receives partial bursts.
//!
//! Run it without and with the `telemetry` feature to see what the delay
//! statistics cost:
//!
//! ```sh
//! cargo bench -p dpds --bench forward
//! cargo bench -p dpds --bench forward --features telemetry
//! ```
//!
//! Prints the median ns per received packet of each case over `REPEATS`
//! repetitions, with the delay statistics on and off (without the `telemetry`
//! feature both columns are the same loop). `harness = false`; numbers are only
//! meaningful relative to each other on the same machine. Without `--bench`
//! (as `cargo test --all-targets` runs it) it does one tiny smoke pass.

use std::hint::black_box;
use std::time::Instant;

use dpdk::RawMbuf;
use dpds::consts::BURST_SIZE;
use dpds::delay_stats::HistRange;
use dpds::forward::run;
use dpds::io::{MbufFree, RxBurst, TxBurst};
use dpds::quit::Quit;
use dpds::stats::{Stats, Telemetry};
use dpds::worker::Worker;
use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

const REPEATS: usize = 15;
/// Distance between two mbufs in a real mempool element (mbuf, headroom and a
/// 2 KiB data room): consecutive packets never share a cache line or a page.
const MBUF_STRIDE: usize = 2_304;
/// Buffer capacity of the large-occupancy cases (the default is 100 000).
const LARGE_CAPACITY: usize = 1 << 18;

/// One fake mbuf with the padding of a mempool element.
#[derive(Clone, Copy)]
#[repr(C)]
struct Slot {
    mbuf: dpdk_sys::rte_mbuf,
    _data: [u8; MBUF_STRIDE - std::mem::size_of::<dpdk_sys::rte_mbuf>()],
}

/// How the RX mock feeds the loop.
#[derive(Clone, Copy)]
struct Load {
    /// Distinct mbufs handed out in a cycle; a multiple of `per_call`, and larger
    /// than the number of packets queued.
    pool: usize,
    /// Packets per `rx_burst` call (at most `BURST_SIZE`).
    per_call: usize,
    /// Clock step per iteration.
    step_ns: f64,
    /// Loop iterations (= `rx_burst` calls) of one repetition.
    iterations: usize,
}

/// The original cases: 32 mbufs, full bursts, 32 packets per 1 us (~32 Mpps).
const FULL: Load = Load {
    pool: BURST_SIZE,
    per_call: BURST_SIZE,
    step_ns: 1_000.0,
    iterations: 200_000,
};

/// Full bursts at 8.1 Mpps from 131 072 cache-cold mbufs.
const COLD: Load = Load {
    pool: 131_072,
    per_call: BURST_SIZE,
    step_ns: 3_950.0,
    iterations: 100_000,
};

/// 4 packets per call at 8.1 Mpps.
const PARTIAL: Load = Load {
    pool: 4_096,
    per_call: 4,
    step_ns: 494.0,
    iterations: 800_000,
};

struct Rx {
    /// Pointers into the mbuf pool, in the order they are handed out.
    ptrs: Vec<RawMbuf>,
    per_call: usize,
    next: usize,
    calls: usize,
    stop_after: usize,
    quit: &'static Quit,
}

impl RxBurst for Rx {
    fn rx_burst(&mut self, _port: u16, bufs: &mut [RawMbuf]) -> u16 {
        self.calls += 1;
        if self.calls >= self.stop_after {
            self.quit.request();
        }
        let end = self.next + self.per_call;
        bufs[..self.per_call].copy_from_slice(&self.ptrs[self.next..end]);
        self.next = if end == self.ptrs.len() { 0 } else { end };
        self.per_call as u16
    }
}

struct Tx;
impl TxBurst for Tx {
    fn tx_burst(&mut self, _port: u16, bufs: &mut [RawMbuf]) -> u16 {
        black_box(bufs.len() as u16)
    }
}

struct Free;
impl MbufFree for Free {
    unsafe fn free_bulk(&mut self, mbufs: &mut [RawMbuf]) {
        black_box(mbufs.len());
    }
}

fn config(distribution: RVDistribution, mean_ns: f64, std_ns: f64, loss: f64) -> Config {
    Config {
        buffer_capacity: 100_000,
        reordering: false,
        delayer: DelayerConfig {
            distribution,
            mean_ns,
            std_ns,
            activate_correlation: false,
            half_life_period_ns: 1.0,
            initialization_rate_pps: 8_200_000.0,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1e11,
        },
        loss_model: LossModel::Random(loss),
        seed: Some(1),
    }
}

/// Runs the loop for `load.iterations` RX calls (plus the drain); returns the elapsed seconds.
fn once(load: &Load, config: Config, delay_stats: bool) -> f64 {
    assert_eq!(load.pool % load.per_call, 0);
    assert!(load.per_call <= BURST_SIZE);
    let mut slot: Slot = unsafe { std::mem::zeroed() };
    slot.mbuf.__bindgen_anon_2.__bindgen_anon_1.pkt_len = 64;
    let mut mbufs = vec![slot; load.pool];
    let ptrs = mbufs.iter_mut().map(|s| &mut s.mbuf as RawMbuf).collect();
    let quit: &'static Quit = Box::leak(Box::new(Quit::new()));
    let stats: &'static Stats = Box::leak(Box::new(Stats::with_delay_stats(
        HistRange::default(),
        delay_stats,
    )));
    let mut rx = Rx {
        ptrs,
        per_call: load.per_call,
        next: 0,
        calls: 0,
        stop_after: load.iterations,
        quit,
    };
    let mut t = Telemetry::new(stats, 0.0);
    let mut clock = 0.0;
    let step_ns = load.step_ns;
    let now = move || {
        clock += step_ns;
        clock
    };
    let worker = Worker::new(Buffer::new(config), 0, 0);
    let start = Instant::now();
    // SAFETY: the mocks never use the fake mbufs beyond reading them.
    unsafe { run(worker, &mut rx, &mut Tx, &mut Free, &mut t, quit, now) };
    let elapsed = start.elapsed().as_secs_f64();
    black_box(stats.counters.rx_pkts.get());
    drop(mbufs);
    elapsed
}

fn main() {
    let bench = std::env::args().any(|a| a == "--bench");
    let repeats = if bench { REPEATS } else { 1 };
    let large = |config| Config {
        buffer_capacity: LARGE_CAPACITY,
        ..config
    };
    let ten_ms = || config(RVDistribution::NormalDistribution, 10e6, 1e6, 0.0);
    let cases = [
        (
            "no delay, no loss",
            FULL,
            config(RVDistribution::DeterministicDistribution, 0.0, 0.0, 0.0),
        ),
        (
            "deterministic 5 us",
            FULL,
            config(RVDistribution::DeterministicDistribution, 5_000.0, 0.0, 0.0),
        ),
        (
            "normal 5 us +- 0.5 us",
            FULL,
            config(RVDistribution::NormalDistribution, 5_000.0, 500.0, 0.0),
        ),
        (
            "normal 5 us +- 0.5 us, 1 % loss",
            FULL,
            config(RVDistribution::NormalDistribution, 5_000.0, 500.0, 0.01),
        ),
        ("normal 10 ms +- 1 ms, cold", COLD, large(ten_ms())),
        (
            "reordering 10 ms +- 1 ms, cold",
            COLD,
            Config {
                reordering: true,
                ..large(ten_ms())
            },
        ),
        (
            "normal 5 us +- 0.5 us, 4 per RX call",
            PARTIAL,
            config(RVDistribution::NormalDistribution, 5_000.0, 500.0, 0.0),
        ),
    ];
    println!(
        "forward::run, telemetry {}, median of {repeats} repetitions",
        if cfg!(feature = "telemetry") {
            "on"
        } else {
            "off"
        },
    );
    println!("  {:38} {:>14} {:>14}", "", "stats on", "stats off");
    for (name, load, config) in cases {
        // Smoke pass: few iterations, a pool small enough to be quick but
        // larger than what ends up queued (no mbuf queued twice).
        let load = if bench {
            load
        } else {
            Load {
                pool: load.pool.min(4_096),
                iterations: 100,
                ..load
            }
        };
        let packets = (load.iterations * load.per_call) as f64;
        let ns_per_packet = |delay_stats| {
            let mut times: Vec<f64> = (0..repeats)
                .map(|_| once(&load, config, delay_stats))
                .collect();
            times.sort_by(f64::total_cmp);
            times[times.len() / 2] / packets * 1e9
        };
        let (on, off) = (ns_per_packet(true), ns_per_packet(false));
        println!("  {name:38} {on:9.2} ns/p {off:9.2} ns/p");
    }
}
