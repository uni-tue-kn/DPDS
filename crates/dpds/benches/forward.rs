//! Microbenchmark of the per-packet cost of `dpds::forward::run`, the loop of
//! one worker lcore, driven by mocks (no NIC, no EAL): the RX mock hands out a
//! full burst of 32 fake mbufs per call, the TX mock accepts everything, the
//! free mock does nothing and the clock advances by a fixed step per
//! iteration. The mbufs are never dereferenced beyond their `pkt_len` and the
//! prefetch of their header line.
//!
//! Run it once without and once with the delay statistics of the `telemetry`
//! feature to see what they cost:
//!
//! ```sh
//! cargo bench -p dpds --bench forward
//! cargo bench -p dpds --bench forward --features telemetry
//! ```
//!
//! Prints the median ns per received packet of each case over `REPEATS`
//! repetitions, once with the direction's delay statistics on and once with
//! them switched off (`"delay_stats": false`; with the `telemetry` feature off
//! both columns are the same loop). `harness = false`; numbers are only meaningful relative to
//! each other on the same machine (the dev container is aarch64, the
//! production hosts x86). Run without `--bench` (what `cargo test
//! --all-targets` does) it only does one tiny smoke pass.

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
/// Loop iterations (= RX bursts of 32 packets) of one repetition.
const ITERATIONS: usize = 200_000;
/// Clock step per iteration.
const STEP_NS: f64 = 1_000.0;

struct Rx {
    burst: [RawMbuf; BURST_SIZE],
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
        bufs[..BURST_SIZE].copy_from_slice(&self.burst);
        BURST_SIZE as u16
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
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1e11,
            limit_bit: 1e9,
        },
        loss_model: LossModel::Random(loss),
        seed: Some(1),
    }
}

/// Runs the loop for `iterations` RX bursts (plus the drain) and returns the
/// elapsed seconds.
fn once(config: Config, delay_stats: bool, iterations: usize) -> f64 {
    let mut mbufs: Vec<dpdk_sys::rte_mbuf> = (0..BURST_SIZE)
        .map(|_| {
            let mut m: dpdk_sys::rte_mbuf = unsafe { std::mem::zeroed() };
            m.__bindgen_anon_2.__bindgen_anon_1.pkt_len = 64;
            m
        })
        .collect();
    let burst: [RawMbuf; BURST_SIZE] = std::array::from_fn(|i| &mut mbufs[i] as *mut _);
    let quit: &'static Quit = Box::leak(Box::new(Quit::new()));
    let stats: &'static Stats = Box::leak(Box::new(Stats::with_delay_stats(
        HistRange::default(),
        delay_stats,
    )));
    let mut rx = Rx {
        burst,
        calls: 0,
        stop_after: iterations,
        quit,
    };
    let mut t = Telemetry::new(stats, 0.0);
    let mut clock = 0.0;
    let now = move || {
        clock += STEP_NS;
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
    let (repeats, iterations) = if bench {
        (REPEATS, ITERATIONS)
    } else {
        (1, 100)
    };
    let cases = [
        (
            "no delay, no loss",
            config(RVDistribution::DeterministicDistribution, 0.0, 0.0, 0.0),
        ),
        (
            "deterministic 5 us",
            config(RVDistribution::DeterministicDistribution, 5_000.0, 0.0, 0.0),
        ),
        (
            "normal 5 us +- 0.5 us",
            config(RVDistribution::NormalDistribution, 5_000.0, 500.0, 0.0),
        ),
        (
            "normal 5 us +- 0.5 us, 1 % loss",
            config(RVDistribution::NormalDistribution, 5_000.0, 500.0, 0.01),
        ),
    ];
    println!(
        "forward::run, telemetry {}, {} bursts of {BURST_SIZE} per repetition",
        if cfg!(feature = "telemetry") {
            "on"
        } else {
            "off"
        },
        iterations
    );
    println!("  {:34} {:>14} {:>14}", "", "stats on", "stats off");
    let packets = (iterations * BURST_SIZE) as f64;
    for (name, config) in cases {
        let ns_per_packet = |delay_stats| {
            let mut times: Vec<f64> = (0..repeats)
                .map(|_| once(config, delay_stats, iterations))
                .collect();
            times.sort_by(f64::total_cmp);
            times[times.len() / 2] / packets * 1e9
        };
        let (on, off) = (ns_per_packet(true), ns_per_packet(false));
        println!("  {name:34} {on:9.2} ns/p {off:9.2} ns/p");
    }
}
