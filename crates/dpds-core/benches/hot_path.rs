//! Microbenchmark of the per-packet cost of `dpds_core::buffer::Buffer`, in
//! isolation from DPDK (no NIC, no rings): the call pattern of the real data
//! plane replayed against synthetic, never dereferenced packet pointers.
//!
//! One iteration = one burst: a single `now` is taken per burst (bursts are a
//! fixed time apart), all 32 packets are pushed with that `now`, then
//! `pop_due(now)` is called until nothing is due or the pop budget of 32 is
//! used up. This is the loop of the `dpds` worker.
//!
//! Two load levels: the `SMALL` cases (50 us delay, ~400 packets queued, 16 Ki
//! capacity) and the `LARGE` ones (10 ms delay at ~8.1 Mpps, ~80 000 queued,
//! 256 Ki capacity; `VERY_LARGE`: ~1 000 000 queued, 2 Mi capacity) that show
//! what a real queue costs in cache misses and heap depth.
//!
//! Every case (FIFO and reordering, deterministic and normal delay, Spacer,
//! random and Gilbert-Elliot loss) runs a verification pass first (every
//! accepted pointer is popped exactly once, FIFO pops in push order,
//! reordering pops in non-decreasing send time), then times `REPEATS`
//! repetitions and prints the median ns per offered packet.
//!
//! ```sh
//! cargo bench -p dpds-core --bench hot_path
//! ```
//!
//! Without `--bench` (as in `cargo test --all-targets`) the binary does one
//! tiny smoke pass of every case.
//!
//! `harness = false`: plain `std::time::Instant` and `std::hint::black_box`, no
//! criterion. Numbers are only meaningful relative to each other on the same
//! machine.
//!
//! ## Per-function instruction counts (callgrind)
//!
//! Build the bench with debug info, then run the binary under callgrind with a
//! single case selected through `HOT_PATH_CASE` (a substring of the case name)
//! and a small `HOT_PATH_BURSTS`:
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
//! valgrind`.)

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
const PACKET_BYTES: u32 = 1500;
/// Bursts per timed repetition / per verification pass / in a smoke pass.
const TIMED_BURSTS: usize = 100_000;
const VERIFY_BURSTS: usize = 5_000;
const SMOKE_BURSTS: usize = 200;
const REPEATS: usize = 7;

/// Queue size of a case: how long packets are delayed, how fast they arrive and
/// how much room the buffer has. Queued packets = `mean_ns / spacing_ns * BURST`.
#[derive(Clone, Copy)]
struct Load {
    /// Time between two bursts: 32 x 1500 B at ~100 Gb/s.
    spacing_ns: f64,
    mean_ns: f64,
    std_ns: f64,
    capacity: usize,
    /// Spacer rate (only used by cases with the limiter).
    limiter_bps: f64,
}

impl Load {
    /// Bursts until the queue is full (steady state); the timed and verified
    /// runs cover several of these.
    fn fill_bursts(&self) -> usize {
        (self.mean_ns / self.spacing_ns).ceil() as usize
    }
}

/// ~100 Gb/s, 50 us: ~400 packets queued.
const SMALL: Load = Load {
    spacing_ns: 3_900.0,
    mean_ns: 50_000.0,
    std_ns: 5_000.0,
    capacity: 1 << 14,
    // 200 Gb/s: keeps up with the ~100 Gb/s offered load, so the limiter
    // computes offsets but never fills up and drops.
    limiter_bps: 200e9,
};

/// 8.1 Mpps of 1500 B (~97 Gb/s), 10 +- 1 ms: ~80 000 queued.
const LARGE: Load = Load {
    spacing_ns: 3_950.0,
    mean_ns: 10_000_000.0,
    std_ns: 1_000_000.0,
    capacity: 1 << 18,
    limiter_bps: 100e9,
};

/// Same rate, 123.5 +- 1 ms: ~1 000 000 queued.
const VERY_LARGE: Load = Load {
    mean_ns: 123_500_000.0,
    capacity: 1 << 21,
    ..LARGE
};

struct Case {
    name: &'static str,
    load: Load,
    reordering: bool,
    distribution: RVDistribution,
    limiter: bool,
    loss: LossModel,
}

const NO_LOSS: LossModel = LossModel::Random(0.0);

const CASES: [Case; 13] = [
    Case {
        name: "fifo / deterministic delay",
        load: SMALL,
        reordering: false,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "fifo / normal delay",
        load: SMALL,
        reordering: false,
        distribution: RVDistribution::NormalDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "fifo / spacer limiter",
        load: SMALL,
        reordering: false,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: true,
        loss: NO_LOSS,
    },
    Case {
        name: "fifo / random loss 1%",
        load: SMALL,
        reordering: false,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: LossModel::Random(0.01),
    },
    Case {
        name: "fifo / gilbert-elliot loss",
        load: SMALL,
        reordering: false,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: LossModel::GilbertElliot(0.01, 0.3, 1.0, 0.5, true),
    },
    Case {
        name: "reordering / deterministic delay",
        load: SMALL,
        reordering: true,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "reordering / normal delay",
        load: SMALL,
        reordering: true,
        distribution: RVDistribution::NormalDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "large: fifo / deterministic delay",
        load: LARGE,
        reordering: false,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "large: fifo / normal delay",
        load: LARGE,
        reordering: false,
        distribution: RVDistribution::NormalDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "large: reordering / deterministic delay",
        load: LARGE,
        reordering: true,
        distribution: RVDistribution::DeterministicDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "large: reordering / normal delay",
        load: LARGE,
        reordering: true,
        distribution: RVDistribution::NormalDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
    Case {
        name: "large: reordering / normal / spacer",
        load: LARGE,
        reordering: true,
        distribution: RVDistribution::NormalDistribution,
        limiter: true,
        loss: NO_LOSS,
    },
    Case {
        name: "very large: reordering / normal delay",
        load: VERY_LARGE,
        reordering: true,
        distribution: RVDistribution::NormalDistribution,
        limiter: false,
        loss: NO_LOSS,
    },
];
impl Case {
    fn config(&self) -> Config {
        Config {
            buffer_capacity: self.load.capacity,
            reordering: self.reordering,
            delayer: DelayerConfig {
                distribution: self.distribution,
                mean_ns: self.load.mean_ns,
                std_ns: self.load.std_ns,
                activate_correlation: false,
                half_life_period_ns: 50_000_000.0,
                initialization_rate_pps: 8_200_000.0,
            },
            limiter: LimiterConfig {
                mode: LimiterMode::Spacer,
                activate: self.limiter,
                rate_bps: self.load.limiter_bps,
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
fn replay(
    buf: &mut Buffer,
    spacing_ns: f64,
    bursts: usize,
    mut tracker: Option<&mut Tracker>,
) -> Totals {
    let mut t = Totals::default();
    let mut next_id = 0usize;
    for burst in 0..bursts {
        let now = burst as f64 * spacing_ns;
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
    let totals = replay(&mut buf, case.load.spacing_ns, bursts, Some(&mut tracker));
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
            let totals = replay(&mut buf, case.load.spacing_ns, black_box(bursts), None);
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
    let repeats = if full { REPEATS } else { 1 };

    if full {
        println!(
            "dpds-core hot path: bursts of {BURST}, {timed_bursts} bursts (large cases: at least 8 queue fills) x {repeats} repeats, median ns per packet"
        );
    } else {
        println!("dpds-core hot path: bursts of {BURST}, smoke pass");
    }
    for case in CASES
        .iter()
        .filter(|c| only.as_ref().is_none_or(|o| c.name.contains(o.as_str())))
    {
        // The large cases need several queue fills to reach (and stay in) steady state.
        let fill = case.load.fill_bursts();
        let (verify_bursts, bursts) = if full {
            (
                VERIFY_BURSTS.min(timed_bursts).max(3 * fill),
                timed_bursts.max(8 * fill),
            )
        } else {
            (SMOKE_BURSTS, SMOKE_BURSTS)
        };
        verify(case, verify_bursts);
        let ns = time(case, bursts, repeats);
        println!("{:<40} {ns:>8.1} ns/pkt", case.name);
    }
}
