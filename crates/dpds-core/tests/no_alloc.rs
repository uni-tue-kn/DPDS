//! After `Buffer::try_new`, the hot-path methods must never allocate,
//! reallocate or free, in any mode. A counting global allocator (per test
//! binary, hence this separate file) counts the calls made by the current
//! thread while a thread-local flag is armed.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::c_void;

use dpds_core::buffer::{Buffer, PushOutcome};
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};

thread_local! {
    /// `None`: disarmed; `Some(n)`: armed, `n` calls counted so far.
    static CALLS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct Counting;

fn count() {
    let _ = CALLS.try_with(|c| c.set(c.get().map(|n| n + 1)));
}

// SAFETY: every method forwards unchanged to `System`.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        count();
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        count();
        System.alloc_zeroed(l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        count();
        System.realloc(p, l, n)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        count();
        System.dealloc(p, l)
    }
}

#[global_allocator]
static A: Counting = Counting;

const LEN: u32 = 1000;
const FAR_NS: f64 = 2e9; // beyond the wheel horizon (~1.07 s)

/// Returns (accepted, popped).
type Workload = fn(&mut Buffer) -> (usize, usize);

/// name, mean_ns, std_ns, capacity, workload
const CASES: [(&str, f64, f64, usize, Workload); 4] = [
    ("steady", 2e6, 5e5, 4096, steady),
    ("fill_drain_twice", 2e6, 5e5, 4096, fill_drain_twice),
    ("one_instant", 2e6, 300.0, 4096, one_instant),
    ("far_future", 1e6, 1.2e9, 2048, far_future),
];

fn config(reordering: bool, spacer: bool, mean: f64, std: f64, capacity: usize) -> Config {
    Config {
        buffer_capacity: capacity,
        reordering,
        delayer: DelayerConfig {
            distribution: RVDistribution::NormalDistribution,
            mean_ns: mean,
            std_ns: std,
            activate_correlation: false,
            half_life_period_ns: 1.0,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: spacer,
            rate_bps: 1e11,
        },
        loss_model: LossModel::Random(0.0),
        seed: Some(11),
    }
}

fn push(b: &mut Buffer, n: &mut usize, t: f64) -> bool {
    *n += 1;
    b.push(*n as *mut c_void, LEN, t)
}

/// Pops everything due at `now` with `pop_due_with_time`, probing the
/// read-only accessors on the way. Returns the number popped.
fn drain_due(b: &mut Buffer, now: f64) -> usize {
    let mut popped = 0;
    while b.pop_due_with_time(now).is_some() {
        let _ = (b.front_time(), b.peek_nth(b.len() / 2));
        popped += 1;
    }
    popped
}

fn drain_all(b: &mut Buffer) -> usize {
    let mut popped = 0;
    while b.pop().is_some() {
        popped += 1;
    }
    popped
}

fn steady(b: &mut Buffer) -> (usize, usize) {
    let (mut n, mut accepted, mut popped) = (0, 0, 0);
    for step in 0..20_000 {
        let t = step as f64 * 1000.0;
        for _ in 0..2 {
            accepted += usize::from(push(b, &mut n, t));
        }
        while b.pop_due(t).is_some() {
            popped += 1;
        }
        let _ = (b.front_time(), b.peek_nth(0), b.len());
    }
    popped += drain_due(b, 1e12);
    (accepted, popped)
}

fn fill_drain_twice(b: &mut Buffer) -> (usize, usize) {
    let (mut n, mut accepted, mut popped) = (0, 0, 0);
    for round in 0..2 {
        let t = round as f64 * 1e7;
        for _ in 0..b.capacity() {
            accepted += usize::from(push(b, &mut n, t));
        }
        assert!(!push(b, &mut n, t), "buffer must be full");
        popped += drain_all(b);
    }
    (accepted, popped)
}

fn one_instant(b: &mut Buffer) -> (usize, usize) {
    let mut n = 0;
    let accepted = (0..b.capacity()).filter(|_| push(b, &mut n, 1e6)).count();
    (accepted, drain_all(b))
}

fn far_future(b: &mut Buffer) -> (usize, usize) {
    let (mut n, mut accepted, mut popped, mut far) = (0, 0, 0, 0);
    for (t, count) in [(0.0, b.capacity() / 2), (5e8, b.capacity() / 2)] {
        popped += drain_due(b, t);
        for _ in 0..count {
            n += 1;
            let out = b.push_outcome(n as *mut c_void, LEN, t);
            if let PushOutcome::Accepted { send_time_ns } = out {
                accepted += 1;
                far += usize::from(send_time_ns as f64 > t + FAR_NS);
            }
        }
    }
    assert!(far > 0, "workload must contain beyond-horizon packets");
    popped += drain_due(b, 1e11);
    (accepted, popped)
}

fn check_mode(mode: &str, reordering: bool, spacer: bool) {
    let mut failures = Vec::new();
    for (name, mean, std, capacity, workload) in CASES {
        let mut b = Buffer::try_new(config(reordering, spacer, mean, std, capacity)).unwrap();
        CALLS.with(|c| c.set(Some(0)));
        let (accepted, popped) = workload(&mut b);
        let calls = CALLS.with(|c| c.take()).unwrap();
        if accepted == 0 || accepted != popped || !b.is_empty() {
            failures.push(format!("{name}: accepted {accepted}, popped {popped}"));
        }
        if calls != 0 {
            failures.push(format!("{name}: {calls} alloc/realloc/dealloc calls"));
        }
    }
    assert!(failures.is_empty(), "{mode}: {failures:#?}");
}

#[test]
fn fifo_never_allocates_after_try_new() {
    check_mode("fifo", false, false);
}

#[test]
fn reordering_never_allocates_after_try_new() {
    check_mode("reordering", true, false);
}

#[test]
fn reordering_spacer_never_allocates_after_try_new() {
    check_mode("reordering+spacer", true, true);
}
