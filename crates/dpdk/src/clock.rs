//! `f64` nanosecond timestamps from the CPU's timestamp counter, the convention
//! of `dpds-core`.

use std::sync::OnceLock;

static TSC_HZ: OnceLock<u64> = OnceLock::new();
static NS_PER_CYCLE: OnceLock<f64> = OnceLock::new();

fn tsc_hz() -> u64 {
    *TSC_HZ.get_or_init(|| unsafe { dpdk_sys::rte_get_tsc_hz() })
}

fn ns_per_cycle() -> f64 {
    *NS_PER_CYCLE.get_or_init(|| 1e9 / (tsc_hz() as f64))
}

/// Current time in nanoseconds from `rte_rdtsc()`. Not tied to a wall-clock
/// epoch: only differences are meaningful.
#[inline(always)]
pub fn now_ns() -> f64 {
    let cycles = unsafe { dpdk_sys::rte_rdtsc() };
    cycles_to_ns(cycles, ns_per_cycle())
}

/// A copy of the TSC scale factor for hot loops: [`Clock::now_ns`] is one
/// `rdtsc` and one multiply, without [`now_ns`]'s `OnceLock` check, on the same
/// timeline. Create it once after the EAL is up and copy it into each worker.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    ns_per_cycle: f64,
}

impl Clock {
    /// Reads the TSC frequency from the initialised EAL.
    ///
    /// # Panics
    ///
    /// If the EAL is not initialised yet (`rte_get_tsc_hz()` is 0).
    pub fn new() -> Self {
        let hz = unsafe { dpdk_sys::rte_get_tsc_hz() };
        assert!(hz != 0, "Clock::new() called before the EAL is initialised");
        Clock {
            ns_per_cycle: 1e9 / (hz as f64),
        }
    }

    #[inline(always)]
    pub fn now_ns(&self) -> f64 {
        let cycles = unsafe { dpdk_sys::rte_rdtsc() };
        cycles_to_ns(cycles, self.ns_per_cycle)
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts a TSC cycle count to nanoseconds with a precomputed `ns_per_cycle`
/// (`1e9 / tsc_hz()`). Separate from [`now_ns`] so it is testable without DPDK.
#[inline(always)]
pub(crate) fn cycles_to_ns(cycles: u64, ns_per_cycle: f64) -> f64 {
    (cycles as f64) * ns_per_cycle
}

#[cfg(test)]
mod tests {
    use super::*;

    // The expected values are computed via division, so exact equality is not guaranteed.
    const EPS: f64 = 1e-6;

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn zero_cycles_is_zero_ns() {
        assert_eq!(cycles_to_ns(0, 1e9 / 3_000_000_000.0), 0.0);
        assert_eq!(cycles_to_ns(0, 1e9 / 2_400_000_000.0), 0.0);
    }

    #[test]
    fn known_3ghz_scale_factor_produces_exact_expected_value() {
        // At 3 GHz, 3_000_000_000 cycles are 1 s = 1e9 ns.
        let ns_per_cycle = 1e9 / 3_000_000_000.0;
        let ns = cycles_to_ns(3_000_000_000, ns_per_cycle);
        assert!(approx_eq(ns, 1e9, EPS), "expected ~{} ns, got {}", 1e9, ns);
    }

    #[test]
    fn known_2_4ghz_scale_factor_produces_exact_expected_value() {
        // A second, non-round clock rate catches a hardcoded 3 GHz.
        let ns_per_cycle = 1e9 / 2_400_000_000.0;
        let ns = cycles_to_ns(2_400_000_000, ns_per_cycle);
        assert!(approx_eq(ns, 1e9, EPS), "expected ~{} ns, got {}", 1e9, ns);
    }

    #[test]
    fn scaling_is_linear() {
        // Fails on a nonlinear implementation (e.g. an added offset).
        let ns_per_cycle = 1e9 / 3_000_000_000.0;
        let c: u64 = 123_456_789;
        let single = cycles_to_ns(c, ns_per_cycle);
        let double = cycles_to_ns(2 * c, ns_per_cycle);
        assert!(
            approx_eq(double, 2.0 * single, EPS),
            "expected {} to be ~2x {}",
            double,
            single
        );
    }
}
