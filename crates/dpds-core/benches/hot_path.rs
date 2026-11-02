//! Microbenchmark of the per-packet cost of `dpds_core::buffer::Buffer`, in
//! isolation from DPDK (no NIC, no rings): the call pattern of the real data
//! plane replayed against synthetic, never dereferenced packet pointers.
//!
//! One iteration = one burst: a single `now` is taken per burst (bursts are a
//! fixed time apart), all 32 packets are pushed with that `now`, then
//! `pop_due(now)` is called until nothing is due or the pop budget of 32 is
//! used up. This is the loop of the `dpds` worker.
//!
//! Cases: deterministic FIFO, normal-delay FIFO, Spacer limiter, Random(0.01)
//! loss, Gilbert-Elliot loss, reordering deterministic, reordering normal.
//! Every case runs a verification pass first (every accepted pointer is popped
//! exactly once, FIFO pops in push order, reordering pops in non-decreasing
//! send time, and in non-decreasing `front_time` for the deterministic
//! cases), then times `REPEATS` repetitions (each asserting the popped count
//! and a pointer checksum against the accepted ones) and prints the median
//! ns per offered packet.
//!
//! ```sh
//! cargo bench -p dpds-core --bench hot_path
//! ```
//!
//! Run without `--bench` (that is what `cargo test --all-targets` does) the
//! binary only does one tiny smoke pass of every case.
//!
//! `harness = false`: a plain `fn main()` with `std::time::Instant` and
//! `std::hint::black_box`, no criterion; the only outlier handling is the
//! median over the repetitions. Numbers are only meaningful relative to each
//! other on the same machine.
//!
//! ## Per-function instruction counts (callgrind)
//!
//! Build the bench with debug info (the `bench` profile inherits `release`),
//! then run the binary under callgrind with a single case selected through the
//! `HOT_PATH_CASE` environment variable (a substring of the case name) and a
//! small `HOT_PATH_BURSTS` so that callgrind finishes in seconds:
//!
//! ```sh
//! CARGO_PROFILE_BENCH_DEBUG=true cargo bench -p dpds-core --bench hot_path --no-run
//! BIN=$(find target/release/deps -maxdepth 1 -name 'hot_path-*' -executable | head -1)
//! HOT_PATH_CASE=spacer HOT_PATH_BURSTS=20000 valgrind --tool=callgrind \
//!     --callgrind-out-file=callgrind.spacer.out "$BIN" --bench
//! callgrind_annotate callgrind.spacer.out
//! ```
//!
//! (`valgrind` is not installed by default: `apt-get update && apt-get install
//! valgrind`.) Do not `perf record` a busy-polling DPDK core in a VM; this
//! benchmark is single-threaded and short, so profiling it is fine.

use std::ffi::c_void;
use std::hint::black_box;
use std::time::Instant;

use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

/// Packets per burst and the pop budget per iteration (`BURST_SIZE` of `dpds`).
const BURST: usize = 32;
/// Time between two bursts: 32 x 1500 B at ~100 Gb/s.
const BURST_SPACING_NS: f64 = 3_900.0;
const PACKET_BYTES: u32 = 1500;
const DELAY_MEAN_NS: f64 = 50_000.0;
const DELAY_STD_NS: f64 = 5_000.0;
/// Bursts per timed repetition / per verification pass / in a smoke pass.
const TIMED_BURSTS: usize = 100_000;
const VERIFY_BURSTS: usize = 5_000;
const SMOKE_BURSTS: usize = 200;
const REPEATS: usize = 7;

struct Case {
    name: &'static str,
    reordering: bool,
    distribution: RVDistribution,
    limiter: bool,
    loss: LossModel,
}

const NO_LOSS: LossModel = LossModel::Random(0.0);

const CASES: [Case; 7] = [
    Case {
        name: "fifo / deterministic delay",
        reordering: false,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "fifo / normal delay",
        reordering: false,
        distribution: RVDistribution::NormalDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "fifo / spacer limiter",
        reordering: false,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: true,
        loss: NO_LOSS,
    },
    Case {
        name: "fifo / random loss 1%",
        reordering: false,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: LossModel::Random(0.01),
    },
    Case {
        name: "fifo / gilbert-elliot loss",
        reordering: false,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: LossModel::GilbertElliot(0.01, 0.3, 1.0, 0.5, true),
    },
    Case {
        name: "reordering / deterministic delay",
        reordering: true,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "reordering / normal delay",
        reordering: true,
        distribution: RVDistribution::NormalDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
];

impl Case {
    fn config(&self) -> Config {
        Config {
            buffer_capacity: 1 << 14,
            reordering: self.reordering,
            delayer: DelayerConfig {
                distribution: self.distribution,
                mean_ns: DELAY_MEAN_NS,
                std_ns: DELAY_STD_NS,
                activate_correlation: false,
                half_life_period_ns: 50_000_000.0,
            },
            // 200 Gb/s: keeps up with the ~100 Gb/s offered load, so the
            // limiter computes offsets but never fills up and drops.
            limiter: LimiterConfig {
                mode: LimiterMode::Spacer,
                activate: self.limiter,
                rate_bps: 200e9,
                limit_bit: 1e9,
            },
            loss_model: self.loss,
            seed: Some(1),
        }
    }

    /// Whether the popped `front_time`s must be non-decreasing (FIFO with a
    /// random delay is not ordered by send time).
    fn front_time_sorted(&self) -> bool {
        self.reordering || matches!(self.distribution, RVDistribution::DeterministicDistribution)
    }
}

/// Packet pointer of packet number `id` (never dereferenced).
#[inline(always)]
fn ptr(id: usize) -> *mut c_void {
    (id + 1) as *mut c_void
}

#[inline(always)]
fn id_of(p: *mut c_void) -> usize {
    p as usize - 1
}

#[derive(Default)]
struct Totals {
    accepted: u64,
    popped: u64,
    accepted_sum: u64,
    popped_sum: u64,
}

/// Per-packet status for the verification pass.
struct Tracker {
    /// 0 = never accepted, 1 = accepted and still queued, 2 = popped.
    state: Vec<u8>,
    last_id: Option<usize>,
    last_front: f64,
    fifo: bool,
    front_sorted: bool,
}

impl Tracker {
    fn on_pop(&mut self, id: usize, front: f64) {
        assert_eq!(self.state[id], 1, "packet {id} popped but not queued");
        self.state[id] = 2;
        if self.fifo {
            assert!(self.last_id.is_none_or(|l| l < id), "FIFO order broken");
        }
        if self.front_sorted {
            assert!(front >= self.last_front, "front_time went backwards");
        }
        self.last_id = Some(id);
        self.last_front = front;
    }
}

/// One `pop_due(now)`; `false` once nothing is due.
#[inline(always)]
fn pop_one(buf: &mut Buffer, now: f64, t: &mut Totals, tracker: &mut Option<&mut Tracker>) -> bool {
    // The head time is only read when tracking, so the timed path is just
    // `pop_due`.
    let front = if tracker.is_some() {
        buf.front_time()
    } else {
        None
    };
    let Some(p) = buf.pop_due(now) else {
        return false;
    };
    t.popped += 1;
    t.popped_sum = t.popped_sum.wrapping_add(id_of(p) as u64);
    if let Some(tr) = tracker {
        tr.on_pop(id_of(p), front.expect("popped from an empty buffer"));
    }
    true
}

/// Replays `bursts` bursts into `buf`. With `tracker` every pop is checked
/// individually (slow, only for the verification pass); without it only the
/// totals are collected.
fn replay(buf: &mut Buffer, bursts: usize, mut tracker: Option<&mut Tracker>) -> Totals {
    let mut t = Totals::default();
    let mut next_id = 0usize;
    for burst in 0..bursts {
        let now = burst as f64 * BURST_SPACING_NS;
        for _ in 0..BURST {
            let id = next_id;
            next_id += 1;
            if buf.push(ptr(id), black_box(PACKET_BYTES), now) {
                t.accepted += 1;
                t.accepted_sum = t.accepted_sum.wrapping_add(id as u64);
                if let Some(tr) = tracker.as_deref_mut() {
                    tr.state[id] = 1;
                }
            }
        }
        for _ in 0..BURST {
            if !pop_one(buf, now, &mut t, &mut tracker) {
                break;
            }
        }
    }
    // Flush what is still queued (not part of the timing).
    while pop_one(buf, f64::INFINITY, &mut t, &mut tracker) {}
    t
}

fn check_totals(name: &str, t: &Totals, buf: &Buffer) {
    assert!(buf.is_empty(), "{name}: buffer not drained");
    assert_eq!(t.accepted, t.popped, "{name}: accepted != popped");
    assert_eq!(t.accepted_sum, t.popped_sum, "{name}: pointer checksum");
    assert!(t.accepted > 0, "{name}: nothing accepted");
}

fn verify(case: &Case, bursts: usize) {
    let mut buf = Buffer::new(case.config());
    let mut tracker = Tracker {
        state: vec![0; bursts * BURST],
        last_id: None,
        last_front: 0.0,
        fifo: !case.reordering,
        front_sorted: case.front_time_sorted(),
    };
    let totals = replay(&mut buf, bursts, Some(&mut tracker));
    check_totals(case.name, &totals, &buf);
    assert!(
        !tracker.state.contains(&1),
        "{}: an accepted packet was never popped",
        case.name
    );
}

/// Median ns per offered packet over `repeats` repetitions.
fn time(case: &Case, bursts: usize, repeats: usize) -> f64 {
    let mut samples: Vec<f64> = (0..repeats)
        .map(|_| {
            let mut buf = Buffer::new(case.config());
            let start = Instant::now();
            let totals = replay(&mut buf, black_box(bursts), None);
            let elapsed = start.elapsed();
            check_totals(case.name, &totals, &buf);
            elapsed.as_nanos() as f64 / (bursts * BURST) as f64
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.parse().ok()
}

fn main() {
    // `cargo bench` passes `--bench`; `cargo test --all-targets` does not.
    let full = std::env::args().any(|a| a == "--bench");
    let only = std::env::var("HOT_PATH_CASE").ok();
    let timed_bursts = env_usize("HOT_PATH_BURSTS").unwrap_or(TIMED_BURSTS);
    let (verify_bursts, bursts, repeats) = if full {
        (VERIFY_BURSTS.min(timed_bursts), timed_bursts, REPEATS)
    } else {
        (SMOKE_BURSTS, SMOKE_BURSTS, 1)
    };

    println!(
        "dpds-core hot path: bursts of {BURST}, {bursts} bursts x {repeats} repeats, median ns per packet{}",
        if full { "" } else { " (smoke pass)" }
    );
    for case in CASES
        .iter()
        .filter(|c| only.as_ref().is_none_or(|o| c.name.contains(o.as_str())))
    {
        verify(case, verify_bursts);
        let ns = time(case, bursts, repeats);
        println!("{:<36} {ns:>8.1} ns/pkt", case.name);
    }
}
