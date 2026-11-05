//! Delay statistics and the delay histogram of one direction.
//!
//! The *intended delay* of an accepted packet is `send_time - now` at push
//! (it includes the Spacer's queueing offset); the *lateness* of a popped
//! packet is `now - send_time` at pop (poll granularity, head-of-line
//! blocking, loop latency). Realized delay = intended delay + lateness.
//!
//! The range types ([`HistRange`], [`HistogramSpec`], [`Bin`]) are pure and
//! exist in every build. The accumulators ([`DelayStats`], [`DelayBatch`]) only
//! record with the `telemetry` feature; without it every per-packet method is
//! an empty `#[inline(always)]` function.
//!
//! # Numerics
//!
//! The mean and the variance are kept as `(count, mean, M2)` (Welford /
//! Chan's parallel update), fed once per loop iteration from a burst-local
//! accumulator that sums `x - first_sample_of_the_burst` and
//! `(x - first_sample_of_the_burst)^2`. Shifting by a sample of the burst
//! keeps the sums small relative to the spread however large the delay itself
//! is (naive `sum` / `sum of squares` in `f64` loses the whole variance for
//! `1e10 ns +- 1 us`), and the per-packet work is two adds and two
//! multiplies; the only divisions are the two of the once-per-burst merge.
//! Plain Welford would need a division per packet. The cumulative state is
//! written by the worker only (relaxed load + store, like `Counter`); a
//! reader can see `count`, `mean` and `M2` of two different iterations, which
//! the documentation of the replies mentions.
//!
//! # Histogram
//!
//! Linear bins over the half-open range `[lo, lo + bins * bin_ns)` plus an
//! underflow and an overflow count. The bin index is one subtract and one
//! multiply by the reciprocal of the bin width, precomputed at startup.

use std::fmt;
#[cfg(feature = "telemetry")]
use std::sync::atomic::{AtomicU64, Ordering};

use dpds_core::limiter::LimiterMode;
use dpds_core::Config;

#[cfg(feature = "telemetry")]
use crate::consts::BURST_SIZE;
use crate::reply::{Reply, Value};

/// Default number of histogram bins.
pub const HIST_BINS: usize = 256;
/// Largest number of bins. The histogram is read in pages of at most
/// [`PAGE_BINS`] bins (`/dpds/delay_hist,<dir>,<page>`).
pub const MAX_BINS: usize = 4096;
/// Most bins in one page of the histogram reply (one DPDK telemetry array holds at most 512
/// entries).
pub const PAGE_BINS: usize = 500;
/// Minimum width (ns) of the automatic histogram range.
pub const MIN_WIDTH_NS: f64 = 1000.0;

/// Where a delay falls in a [`HistRange`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bin {
    Underflow,
    Index(usize),
    Overflow,
}

/// The per-link `"histogram"` entry of the links file: every field
/// optional, a missing one takes the automatic value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HistogramSpec {
    pub bins: Option<usize>,
    pub lo_ns: Option<f64>,
    pub hi_ns: Option<f64>,
}

impl HistogramSpec {
    /// The checks that need no config: `bins` in `1..=4096`, `lo_ns` / `hi_ns`
    /// finite and not negative, and `lo_ns < hi_ns` if both are given.
    pub fn validate(&self) -> Result<(), HistogramError> {
        if let Some(bins) = self.bins {
            if !(1..=MAX_BINS).contains(&bins) {
                return Err(HistogramError(format!(
                    "bins = {bins} is out of range, it must be an integer in 1..={MAX_BINS}"
                )));
            }
        }
        for (name, v) in [("lo_ns", self.lo_ns), ("hi_ns", self.hi_ns)] {
            if let Some(v) = v {
                if !v.is_finite() || v < 0.0 {
                    return Err(HistogramError(format!(
                        "{name} = {v} must be a finite number >= 0"
                    )));
                }
            }
        }
        if let (Some(lo), Some(hi)) = (self.lo_ns, self.hi_ns) {
            if lo >= hi {
                return Err(HistogramError(format!(
                    "lo_ns = {lo} must be below hi_ns = {hi}"
                )));
            }
        }
        Ok(())
    }
}

/// An invalid histogram setting; the message names the offending field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistogramError(String);

impl fmt::Display for HistogramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid histogram: {}", self.0)
    }
}

impl std::error::Error for HistogramError {}

/// The range and resolution of a direction's delay histogram.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistRange {
    /// Lower edge (inclusive) of bin 0.
    pub lo_ns: f64,
    /// Width of every bin.
    pub bin_ns: f64,
    /// Number of bins.
    pub bins: usize,
}

impl Default for HistRange {
    /// The window of "mean 0, std 0": `[0, 1 us)` in 256 bins.
    fn default() -> Self {
        HistRange::new(0.0, MIN_WIDTH_NS / HIST_BINS as f64)
    }
}

/// The automatic window `[lo, hi)` of a config (see [`HistRange::from_config`]).
fn automatic_window(config: &Config) -> (f64, f64) {
    let (mean, std) = (config.delayer.mean_ns, config.delayer.std_ns);
    let limiter = &config.limiter;
    let extra = if limiter.activate && matches!(limiter.mode, LimiterMode::Spacer) {
        let extra = dpds_core::derived_limit_bit(config.buffer_capacity) / limiter.rate_bps * 1e9;
        if extra.is_finite() {
            extra.max(0.0)
        } else {
            0.0
        }
    } else {
        0.0
    };
    let lo = (mean - 6.0 * std).max(0.0);
    let hi = mean + 6.0 * std + extra;
    if hi - lo < MIN_WIDTH_NS {
        let lo = ((lo + hi) / 2.0 - MIN_WIDTH_NS / 2.0).max(0.0);
        (lo, lo + MIN_WIDTH_NS)
    } else {
        (lo, hi)
    }
}

/// The bin of `d` in `[lo, hi)` of `bins` bins of reciprocal width `inv`.
#[inline(always)]
fn place(lo: f64, hi: f64, inv: f64, bins: usize, d: f64) -> Bin {
    if d < lo {
        Bin::Underflow
    } else if d >= hi {
        Bin::Overflow
    } else {
        // `as usize` truncates (floor for d >= lo); the min guards the
        // rounding of `inv` right below `hi`.
        Bin::Index((((d - lo) * inv) as usize).min(bins - 1))
    }
}

impl HistRange {
    /// `HIST_BINS` bins of `bin_ns` from `lo_ns`.
    pub fn new(lo_ns: f64, bin_ns: f64) -> HistRange {
        HistRange {
            lo_ns,
            bin_ns,
            bins: HIST_BINS,
        }
    }

    /// `bins` bins of `bin_ns` from `lo_ns`. Zero bins is an error (a
    /// histogram needs a bin to place a delay in).
    pub fn with_bins(lo_ns: f64, bin_ns: f64, bins: usize) -> Result<HistRange, HistogramError> {
        if bins == 0 {
            return Err(HistogramError("bins = 0, it must be at least 1".into()));
        }
        Ok(HistRange {
            lo_ns,
            bin_ns,
            bins,
        })
    }

    /// The automatic range of a direction's config, 256 bins:
    ///
    /// ```text
    /// lo = max(0, mean - 6 std)
    /// hi = mean + 6 std + extra       extra = capacity * 12336 / rate_bps * 1e9 for an
    ///                                 active Spacer (its queueing allowance), else 0
    /// if hi - lo < 1000 ns:           mid = (lo + hi) / 2
    ///                                 lo = max(0, mid - 500);  hi = lo + 1000
    /// bin_ns = (hi - lo) / 256
    /// ```
    pub fn from_config(config: &Config) -> HistRange {
        let (lo, hi) = automatic_window(config);
        HistRange::new(lo, (hi - lo) / HIST_BINS as f64)
    }

    /// The range of a link's IAT (departure gap) histogram: `[0, hi)` in 256
    /// bins, `hi = 4 * 1e9 / initialization_rate_pps` ns (4 mean gaps at the
    /// expected rate), whatever the delay distribution or limiter.
    pub fn iat_from_config(config: &Config) -> HistRange {
        let hi = 4.0 * 1e9 / config.delayer.initialization_rate_pps;
        HistRange::new(0.0, hi / HIST_BINS as f64)
    }

    /// The range of a [`HistogramSpec`]: a missing `bins` is 256, a missing
    /// `lo_ns` / `hi_ns` the automatic one of `config`. Explicit values are
    /// used as given (the minimum width applies to the automatic range only).
    pub fn from_spec(config: &Config, spec: &HistogramSpec) -> Result<HistRange, HistogramError> {
        spec.validate()?;
        let (auto_lo, auto_hi) = automatic_window(config);
        let lo = spec.lo_ns.unwrap_or(auto_lo);
        let hi = spec.hi_ns.unwrap_or(auto_hi);
        if lo >= hi {
            return Err(HistogramError(format!(
                "lo_ns = {lo} must be below hi_ns = {hi} (a value that is not given takes the \
                 automatic one, derived from the config)"
            )));
        }
        let bins = spec.bins.unwrap_or(HIST_BINS);
        HistRange::with_bins(lo, (hi - lo) / bins as f64, bins)
    }

    /// The (exclusive) upper edge.
    pub fn hi_ns(&self) -> f64 {
        self.lo_ns + self.bins as f64 * self.bin_ns
    }

    /// Where `delay_ns` falls: `[lo, hi)` is split into `bins` bins, everything
    /// below is `Underflow`, everything from `hi` on `Overflow`.
    pub fn bin(&self, delay_ns: f64) -> Bin {
        place(
            self.lo_ns,
            self.hi_ns(),
            1.0 / self.bin_ns,
            self.bins,
            delay_ns,
        )
    }
}

/// The send times of the packets accepted and popped in one loop iteration,
/// collected by the loop (one store per packet) and folded into the
/// statistics by [`DelayStats::record_accepts`] / [`DelayStats::record_pops`]
/// after the respective loop. Collecting first keeps the floating-point
/// accumulators out of the push loop, which calls `Buffer::push_outcome`: a
/// callee that uses `d8..d15` (the normal-delay sampler does) would save and
/// restore every accumulator through the stack on each call. Created once per
/// worker, [`begin`](Self::begin) starts an iteration. A zero-sized no-op
/// without the `telemetry` feature.
#[cfg(feature = "telemetry")]
pub struct DelayBatch {
    accepted: [u64; BURST_SIZE],
    n_accepted: usize,
    popped: [u64; BURST_SIZE],
    n_popped: usize,
    /// Send time of the last packet popped before this iteration, if any
    /// (the IAT reference, carried across iterations).
    prev_send_ns: Option<u64>,
}
#[cfg(not(feature = "telemetry"))]
pub struct DelayBatch;

#[cfg(feature = "telemetry")]
impl DelayBatch {
    pub fn new() -> DelayBatch {
        DelayBatch {
            accepted: [0; BURST_SIZE],
            n_accepted: 0,
            popped: [0; BURST_SIZE],
            n_popped: 0,
            prev_send_ns: None,
        }
    }

    /// Starts a loop iteration.
    #[inline(always)]
    pub fn begin(&mut self) {
        self.n_accepted = 0;
        self.n_popped = 0;
    }

    /// A packet accepted by the buffer, leaving at `send_time_ns`.
    #[inline(always)]
    pub fn accepted(&mut self, send_time_ns: u64) {
        self.accepted[self.n_accepted] = send_time_ns;
        self.n_accepted += 1;
    }

    /// A packet popped from the buffer, which was due at `send_time_ns`.
    #[inline(always)]
    pub fn popped(&mut self, send_time_ns: u64) {
        self.popped[self.n_popped] = send_time_ns;
        self.n_popped += 1;
    }
}

#[cfg(not(feature = "telemetry"))]
impl DelayBatch {
    pub fn new() -> DelayBatch {
        DelayBatch
    }

    #[inline(always)]
    pub fn begin(&mut self) {}

    #[inline(always)]
    pub fn accepted(&mut self, _send_time_ns: u64) {}

    #[inline(always)]
    pub fn popped(&mut self, _send_time_ns: u64) {}
}

impl Default for DelayBatch {
    fn default() -> Self {
        DelayBatch::new()
    }
}

#[cfg(feature = "telemetry")]
fn bump(a: &AtomicU64) {
    a.store(a.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
}

#[cfg(feature = "telemetry")]
fn load_f64(a: &AtomicU64) -> f64 {
    f64::from_bits(a.load(Ordering::Relaxed))
}

#[cfg(feature = "telemetry")]
fn store_f64(a: &AtomicU64, v: f64) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

/// The counts of bins `first..end` plus underflow and overflow.
#[cfg(feature = "telemetry")]
fn read_page(
    bins: &[AtomicU64],
    underflow: &AtomicU64,
    overflow: &AtomicU64,
    first: usize,
    end: usize,
) -> (Vec<u64>, u64, u64) {
    (
        bins[first..end]
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect(),
        underflow.load(Ordering::Relaxed),
        overflow.load(Ordering::Relaxed),
    )
}

/// The delay statistics and histogram of one direction: written by its worker
/// only, read by the telemetry thread. Without the `telemetry` feature it
/// only remembers the range and records nothing. A direction with the
/// statistics switched off (`"delay_stats": false`) allocates no bins, is
/// never recorded into (`forward::run` skips the calls) and reports no delay
/// keys and an empty histogram.
pub struct DelayStats {
    range: HistRange,
    iat_range: HistRange,
    enabled: bool,
    #[cfg(feature = "telemetry")]
    live: Live,
}

#[cfg(feature = "telemetry")]
struct Live {
    hi_ns: f64,
    inv_bin_ns: f64,
    count: AtomicU64,
    mean: AtomicU64,
    m2: AtomicU64,
    min: AtomicU64,
    max: AtomicU64,
    late_sum: AtomicU64,
    late_max: AtomicU64,
    underflow: AtomicU64,
    overflow: AtomicU64,
    bins: Box<[AtomicU64]>,
    iat_hi_ns: f64,
    iat_inv_bin_ns: f64,
    iat_underflow: AtomicU64,
    iat_overflow: AtomicU64,
    iat_bins: Box<[AtomicU64]>,
}

impl Default for DelayStats {
    fn default() -> Self {
        DelayStats::new(HistRange::default())
    }
}

impl DelayStats {
    /// Statistics switched on.
    pub fn new(range: HistRange) -> DelayStats {
        DelayStats::with_enabled(range, true)
    }

    /// With the statistics `enabled` or not (see the type docs).
    pub fn with_enabled(range: HistRange, enabled: bool) -> DelayStats {
        DelayStats::with_hists(range, HistRange::default(), enabled)
    }

    /// With the delay histogram `range`, the IAT histogram `iat_range` (the
    /// departure gaps, see [`record_pops`](Self::record_pops)) and the
    /// statistics `enabled` or not.
    pub fn with_hists(range: HistRange, iat_range: HistRange, enabled: bool) -> DelayStats {
        #[cfg(feature = "telemetry")]
        let zeroed = |n: usize| (0..if enabled { n } else { 0 }).map(|_| AtomicU64::new(0));
        DelayStats {
            range,
            iat_range,
            enabled,
            #[cfg(feature = "telemetry")]
            live: Live {
                hi_ns: range.hi_ns(),
                inv_bin_ns: 1.0 / range.bin_ns,
                count: AtomicU64::new(0),
                mean: AtomicU64::new(0),
                m2: AtomicU64::new(0),
                min: AtomicU64::new(0),
                max: AtomicU64::new(0),
                late_sum: AtomicU64::new(0),
                late_max: AtomicU64::new(0),
                underflow: AtomicU64::new(0),
                overflow: AtomicU64::new(0),
                bins: zeroed(range.bins).collect(),
                iat_hi_ns: iat_range.hi_ns(),
                iat_inv_bin_ns: 1.0 / iat_range.bin_ns,
                iat_underflow: AtomicU64::new(0),
                iat_overflow: AtomicU64::new(0),
                iat_bins: zeroed(iat_range.bins).collect(),
            },
        }
    }

    pub fn range(&self) -> HistRange {
        self.range
    }

    /// Whether the direction records delay statistics.
    #[inline(always)]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Folds the intended delays of the packets accepted in this iteration
    /// (pushed at `now`) into the statistics and the histogram.
    #[cfg(feature = "telemetry")]
    #[inline(always)]
    pub fn record_accepts(&self, batch: &DelayBatch, now: f64) {
        debug_assert!(self.enabled, "recording into switched-off delay statistics");
        let times = &batch.accepted[..batch.n_accepted];
        let Some(&first) = times.first() else { return };
        let l = &self.live;
        let (lo, bins) = (self.range.lo_ns, self.range.bins);
        let delay_of = |t: u64| (t as f64 - now).max(0.0);
        let shift = delay_of(first);
        let (mut s1, mut s2, mut min, mut max) = (0.0, 0.0, shift, shift);
        for &t in times {
            let delay = delay_of(t);
            let d = delay - shift;
            s1 += d;
            s2 += d * d;
            if delay < min {
                min = delay;
            }
            if delay > max {
                max = delay;
            }
            match place(lo, l.hi_ns, l.inv_bin_ns, bins, delay) {
                Bin::Underflow => bump(&l.underflow),
                Bin::Overflow => bump(&l.overflow),
                Bin::Index(i) => bump(&l.bins[i]),
            }
        }
        self.merge_delays(times.len() as u32, shift, s1, s2, min, max);
    }

    #[cfg(not(feature = "telemetry"))]
    #[inline(always)]
    pub fn record_accepts(&self, _batch: &DelayBatch, _now: f64) {}

    /// Folds the packets popped in this iteration (at `now`): their lateness
    /// into the lateness statistics and the gap between consecutive send times
    /// into the IAT histogram. The first packet of the run has no gap; the
    /// previous send time is carried across iterations in `batch`, and a
    /// negative gap (reordering) counts as 0.
    #[cfg(feature = "telemetry")]
    #[inline(always)]
    pub fn record_pops(&self, batch: &mut DelayBatch, now: f64) {
        debug_assert!(self.enabled, "recording into switched-off delay statistics");
        let times = &batch.popped[..batch.n_popped];
        let Some(&last) = times.last() else { return };
        let l = &self.live;
        let (mut sum, mut max) = (0.0, 0.0);
        let mut prev = batch.prev_send_ns;
        for &t in times {
            let late = (now - t as f64).max(0.0);
            sum += late;
            if late > max {
                max = late;
            }
            if let Some(p) = prev {
                let gap = t.saturating_sub(p) as f64;
                match place(
                    self.iat_range.lo_ns,
                    l.iat_hi_ns,
                    l.iat_inv_bin_ns,
                    self.iat_range.bins,
                    gap,
                ) {
                    Bin::Underflow => bump(&l.iat_underflow),
                    Bin::Overflow => bump(&l.iat_overflow),
                    Bin::Index(i) => bump(&l.iat_bins[i]),
                }
            }
            prev = Some(t);
        }
        batch.prev_send_ns = Some(last);
        store_f64(&l.late_sum, load_f64(&l.late_sum) + sum);
        if max > load_f64(&l.late_max) {
            store_f64(&l.late_max, max);
        }
    }

    #[cfg(not(feature = "telemetry"))]
    #[inline(always)]
    pub fn record_pops(&self, _batch: &mut DelayBatch, _now: f64) {}

    /// Chan's parallel update of `(count, mean, M2)` with a burst of `n`
    /// delays whose sums, shifted by `shift`, are `s1` and `s2`.
    #[cfg(feature = "telemetry")]
    #[inline(never)]
    fn merge_delays(&self, n: u32, shift: f64, s1: f64, s2: f64, min: f64, max: f64) {
        let l = &self.live;
        let nb = n as f64;
        let mean_b = shift + s1 / nb;
        let m2_b = (s2 - s1 * s1 / nb).max(0.0);
        let count = l.count.load(Ordering::Relaxed);
        let na = count as f64;
        let total = na + nb;
        let delta = mean_b - load_f64(&l.mean);
        store_f64(&l.mean, load_f64(&l.mean) + delta * nb / total);
        store_f64(
            &l.m2,
            load_f64(&l.m2) + m2_b + delta * delta * na * nb / total,
        );
        if count == 0 {
            store_f64(&l.min, min);
            store_f64(&l.max, max);
        } else {
            store_f64(&l.min, load_f64(&l.min).min(min));
            store_f64(&l.max, load_f64(&l.max).max(max));
        }
        l.count.store(count + n as u64, Ordering::Relaxed);
    }

    /// `delay_mean_ns`, `delay_std_ns`, `delay_min_ns`, `delay_max_ns`,
    /// `lateness_mean_ns`, `lateness_max_ns` for `/dpds/stats`. `popped` is the
    /// number of popped packets (the lateness count). Means and the
    /// (population) standard deviation are 2-decimal strings, min and max
    /// truncated uints; all 0 before the first packet. Empty when the
    /// statistics are switched off.
    pub fn stats_fields(&self, popped: u64) -> Reply {
        if !self.enabled {
            return Vec::new();
        }
        #[cfg(feature = "telemetry")]
        let (n, mean, m2, min, max, late_sum, late_max) = {
            let l = &self.live;
            (
                l.count.load(Ordering::Relaxed),
                load_f64(&l.mean),
                load_f64(&l.m2),
                load_f64(&l.min),
                load_f64(&l.max),
                load_f64(&l.late_sum),
                load_f64(&l.late_max),
            )
        };
        #[cfg(not(feature = "telemetry"))]
        let (n, mean, m2, min, max, late_sum, late_max) = (0u64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let std = if n > 0 { (m2 / n as f64).sqrt() } else { 0.0 };
        let late_mean = if popped > 0 {
            late_sum / popped as f64
        } else {
            0.0
        };
        vec![
            ("delay_mean_ns".into(), Value::Str(format!("{mean:.2}"))),
            ("delay_std_ns".into(), Value::Str(format!("{std:.2}"))),
            ("delay_min_ns".into(), Value::Uint(min as u64)),
            ("delay_max_ns".into(), Value::Uint(max as u64)),
            (
                "lateness_mean_ns".into(),
                Value::Str(format!("{late_mean:.2}")),
            ),
            ("lateness_max_ns".into(), Value::Uint(late_max as u64)),
        ]
    }

    /// Page `page` (0-based) of the `/dpds/delay_hist` reply: `lo_ns` and `bin_ns`
    /// (decimal strings, DPDK telemetry has no float setter), `num_bins` (of the
    /// whole histogram), `bins` (the page's slice, at most [`PAGE_BINS`] counts),
    /// `underflow`, `overflow`, `page` and `first_bin` (`PAGE_BINS * page`, the index
    /// of the page's first bin). Cumulative since start. `None` for a page past the
    /// last one. With the statistics switched off, page 0 is empty and there is no
    /// other page.
    pub fn hist_page(&self, page: usize) -> Option<Reply> {
        self.page_reply(self.range, page, |first, end| {
            #[cfg(feature = "telemetry")]
            {
                let l = &self.live;
                read_page(&l.bins, &l.underflow, &l.overflow, first, end)
            }
            #[cfg(not(feature = "telemetry"))]
            (vec![0u64; end - first], 0, 0)
        })
    }

    /// Page `page` of the `/dpds/iat_hist` reply: the histogram of the gaps
    /// between consecutive send times, same keys and paging as
    /// [`hist_page`](Self::hist_page).
    pub fn iat_hist_page(&self, page: usize) -> Option<Reply> {
        self.page_reply(self.iat_range, page, |first, end| {
            #[cfg(feature = "telemetry")]
            {
                let l = &self.live;
                read_page(&l.iat_bins, &l.iat_underflow, &l.iat_overflow, first, end)
            }
            #[cfg(not(feature = "telemetry"))]
            (vec![0u64; end - first], 0, 0)
        })
    }

    fn page_reply(
        &self,
        range: HistRange,
        page: usize,
        read: impl FnOnce(usize, usize) -> (Vec<u64>, u64, u64),
    ) -> Option<Reply> {
        if !self.enabled {
            return (page == 0).then(Vec::new);
        }
        let first = page.checked_mul(PAGE_BINS)?;
        if first >= range.bins {
            return None;
        }
        let end = range.bins.min(first + PAGE_BINS);
        let (bins, underflow, overflow) = read(first, end);
        Some(vec![
            ("lo_ns".into(), Value::Str(range.lo_ns.to_string())),
            ("bin_ns".into(), Value::Str(range.bin_ns.to_string())),
            ("num_bins".into(), Value::Uint(range.bins as u64)),
            ("bins".into(), Value::UintArray(bins)),
            ("underflow".into(), Value::Uint(underflow)),
            ("overflow".into(), Value::Uint(overflow)),
            ("page".into(), Value::Uint(page as u64)),
            ("first_bin".into(), Value::Uint(first as u64)),
        ])
    }
}
