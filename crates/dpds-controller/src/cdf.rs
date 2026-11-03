//! Expected delay histogram (K3): the CDFs of the delay distributions of `dpds-core`
//! and the expected count per histogram bin.
//!
//! Self-contained instead of `statrs`: everything needed is the regularised
//! incomplete gamma function (the normal CDF is `P(1/2, z^2/2)`), about 100 lines,
//! and `statrs` would pull `nalgebra` and `rand` into a controller that otherwise has
//! neither. The unit tests below check it against known values.

use crate::dto::{DelayerDto, DistributionDto};

/// Shape above which `gamma_pq` uses the Wilson-Hilferty approximation: the series
/// needs about `sqrt(a)` terms and the distribution is within 0.3 % of normal by then.
const WILSON_HILFERTY_ABOVE: f64 = 1e5;

/// `ln Gamma(x)` for `x > 0`: shifts `x` above 15 with `Gamma(x+1) = x Gamma(x)`, then
/// the Stirling series (error below 1e-13).
fn ln_gamma(mut x: f64) -> f64 {
    let mut shift = 0.0;
    while x < 15.0 {
        shift += x.ln();
        x += 1.0;
    }
    let inv = 1.0 / x;
    let inv2 = inv * inv;
    let series = inv * (1.0 / 12.0 - inv2 * (1.0 / 360.0 - inv2 * (1.0 / 1260.0 - inv2 / 1680.0)));
    (x - 0.5) * x.ln() - x + 0.5 * (2.0 * std::f64::consts::PI).ln() + series - shift
}

/// The regularised incomplete gamma functions `(P(a, x), Q(a, x))`, `a > 0`, `x >= 0`.
/// Series for `x < a + 1`, continued fraction (modified Lentz) above.
fn gamma_pq(a: f64, x: f64) -> (f64, f64) {
    if x <= 0.0 {
        return (0.0, 1.0);
    }
    if a > WILSON_HILFERTY_ABOVE {
        let z = ((x / a).cbrt() - 1.0 + 1.0 / (9.0 * a)) * 3.0 * a.sqrt();
        let p = normal_cdf(z);
        return (p, 1.0 - p);
    }
    const EPS: f64 = 1e-16;
    const TINY: f64 = 1e-300;
    const MAX_ITER: u32 = 100_000;
    let prefactor = (-x + a * x.ln() - ln_gamma(a)).exp();
    if x < a + 1.0 {
        let mut term = 1.0 / a;
        let mut sum = term;
        for n in 1..=MAX_ITER {
            term *= x / (a + f64::from(n));
            sum += term;
            if term < sum * EPS {
                break;
            }
        }
        let p = (sum * prefactor).min(1.0);
        (p, 1.0 - p)
    } else {
        let mut b = x + 1.0 - a;
        let mut c = 1.0 / TINY;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..=MAX_ITER {
            let i = f64::from(i);
            let an = -i * (i - a);
            b += 2.0;
            d = an * d + b;
            if d.abs() < TINY {
                d = TINY;
            }
            c = b + an / c;
            if c.abs() < TINY {
                c = TINY;
            }
            d = 1.0 / d;
            let delta = d * c;
            h *= delta;
            if (delta - 1.0).abs() < EPS {
                break;
            }
        }
        let q = (h * prefactor).min(1.0);
        (1.0 - q, q)
    }
}

/// The standard normal CDF.
pub fn normal_cdf(z: f64) -> f64 {
    // P(1/2, z^2/2) = erf(|z| / sqrt 2), Q = erfc
    let (_, q) = gamma_pq(0.5, z * z / 2.0);
    if z < 0.0 {
        0.5 * q
    } else {
        1.0 - 0.5 * q
    }
}

/// The regularised lower incomplete gamma function `P(a, x)` (the CDF of a Gamma
/// variable with shape `a` at `x` in units of the scale).
pub fn gamma_p(a: f64, x: f64) -> f64 {
    gamma_pq(a, x).0
}

/// A delay distribution as `dpds` samples it, after the clamp of negative delays to 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DelayModel {
    /// All mass at one point.
    Point(f64),
    Normal {
        mean: f64,
        std: f64,
    },
    LogNormal {
        mu: f64,
        sigma: f64,
    },
    Gamma {
        shape: f64,
        scale: f64,
    },
    Uniform {
        lo: f64,
        hi: f64,
    },
}

impl DelayModel {
    /// The model of a configured delayer and whether it is exact. With
    /// `activate_correlation` the stationary distribution is approximated as
    /// `Normal(mean, std)` (not exact; a deterministic delay stays exact).
    /// `dpds-core` parameterisation: LogNormal `sigma^2 = ln(1 + cv^2)`,
    /// `mu = ln(mean) - sigma^2 / 2`; Gamma shape `(mean/std)^2`, scale `std^2/mean`;
    /// Uniform `mean +- sqrt(3) std`. A zero std (or a mean of 0 for LogNormal and
    /// Gamma, which `dpds` rejects) is a point mass.
    pub fn from_delayer(d: &DelayerDto) -> (DelayModel, bool) {
        let (mean, std) = (d.mean_ns, d.std_ns);
        let point = DelayModel::Point(mean);
        if matches!(d.distribution, DistributionDto::DeterministicDistribution) {
            return (point, true);
        }
        let degenerate = !(std > 0.0 && std.is_finite() && mean.is_finite());
        if degenerate {
            return (point, true);
        }
        let positive = mean > 0.0;
        let normal = DelayModel::Normal { mean, std };
        if d.activate_correlation {
            return (normal, false);
        }
        let model = match d.distribution {
            DistributionDto::DeterministicDistribution => point,
            DistributionDto::NormalDistribution => normal,
            DistributionDto::LogNormalDistribution if positive => {
                let sigma2 = (1.0 + (std / mean) * (std / mean)).ln();
                DelayModel::LogNormal {
                    mu: mean.ln() - sigma2 / 2.0,
                    sigma: sigma2.sqrt(),
                }
            }
            DistributionDto::GammaDistribution if positive => DelayModel::Gamma {
                shape: (mean / std) * (mean / std),
                scale: std * std / mean,
            },
            DistributionDto::UniformDistribution => DelayModel::Uniform {
                lo: mean - 3f64.sqrt() * std,
                hi: mean + 3f64.sqrt() * std,
            },
            _ => point,
        };
        (model, true)
    }

    /// `P(delay < x)` of the clamped delay `max(X, 0)`: 0 for `x <= 0`, else `P(X < x)`.
    fn below(&self, x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        match *self {
            DelayModel::Point(m) => f64::from(u8::from(x > m.max(0.0))),
            DelayModel::Normal { mean, std } => normal_cdf((x - mean) / std),
            DelayModel::LogNormal { mu, sigma } => normal_cdf((x.ln() - mu) / sigma),
            DelayModel::Gamma { shape, scale } => gamma_p(shape, x / scale),
            DelayModel::Uniform { lo, hi } => ((x - lo) / (hi - lo)).clamp(0.0, 1.0),
        }
    }

    /// Expected counts of `total` samples for a histogram of `n` bins of width
    /// `bin` from `lo`: `(per bin, underflow, overflow)`. Bins are half-open
    /// `[edge, edge + bin)`; the mass of negative delays sits at 0, i.e. in the bin
    /// that contains 0 (the underflow if the range starts above 0).
    pub fn expected_counts(&self, lo: f64, bin: f64, n: usize, total: u64) -> (Vec<f64>, f64, f64) {
        let total = total as f64;
        let g: Vec<f64> = (0..=n).map(|i| self.below(lo + i as f64 * bin)).collect();
        let bins = g
            .windows(2)
            .map(|w| (w[1] - w[0]).max(0.0) * total)
            .collect();
        let under = g[0] * total;
        let over = (1.0 - g[n]).max(0.0) * total;
        (bins, under, over)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} vs {b} (tolerance {tol})");
    }

    #[test]
    fn ln_gamma_known_values() {
        close(ln_gamma(1.0), 0.0, 1e-13);
        close(ln_gamma(2.0), 0.0, 1e-13);
        close(ln_gamma(0.5), 0.572_364_942_924_700_1, 1e-13);
        close(ln_gamma(4.0), 6f64.ln(), 1e-13);
        close(ln_gamma(10.0), 362_880f64.ln(), 1e-13);
        close(ln_gamma(100.0), 359.134_205_369_575_4, 1e-11);
        close(ln_gamma(0.01), 4.599_479_878_042_021, 1e-12);
    }

    #[test]
    fn normal_cdf_known_values() {
        close(normal_cdf(0.0), 0.5, 1e-15);
        close(normal_cdf(1.0), 0.841_344_746_068_542_9, 1e-13);
        close(normal_cdf(-1.0), 0.158_655_253_931_457_07, 1e-13);
        close(normal_cdf(2.0), 0.977_249_868_051_820_8, 1e-13);
        close(normal_cdf(-2.0), 0.022_750_131_948_179_195, 1e-13);
        close(normal_cdf(3.0), 0.998_650_101_968_369_9, 1e-13);
        close(normal_cdf(1.959_963_984_540_054), 0.975, 1e-12);
    }

    #[test]
    fn normal_cdf_keeps_relative_precision_in_the_tail() {
        let t = normal_cdf(-5.0);
        assert!((t / 2.866_515_718_791_939_5e-7 - 1.0).abs() < 1e-10, "{t}");
        let t = normal_cdf(-10.0);
        assert!((t / 7.619_853_024_160_527e-24 - 1.0).abs() < 1e-10, "{t}");
        assert_eq!(normal_cdf(-40.0), 0.0);
        assert_eq!(normal_cdf(40.0), 1.0);
    }

    #[test]
    fn normal_cdf_is_symmetric_and_monotone() {
        let mut last = 0.0;
        for i in -80..=80 {
            let z = f64::from(i) / 10.0;
            let p = normal_cdf(z);
            close(p + normal_cdf(-z), 1.0, 1e-14);
            assert!(p >= last, "not monotone at {z}");
            last = p;
        }
    }

    #[test]
    fn gamma_p_shape_one_is_the_exponential_cdf() {
        for x in [0.01, 0.7, 1.9, 2.1, 5.0, 30.0] {
            close(gamma_p(1.0, x), 1.0 - (-x).exp(), 1e-12);
        }
    }

    #[test]
    fn gamma_p_integer_shapes_are_erlang_cdfs_in_both_branches() {
        // P(2, x) = 1 - e^-x (1 + x)
        for x in [0.5, 2.0, 2.9, 3.1, 8.0] {
            close(gamma_p(2.0, x), 1.0 - (-x).exp() * (1.0 + x), 1e-12);
        }
        // P(4, x) = 1 - e^-x (1 + x + x^2/2 + x^3/6): x = 2 uses the series, x = 10 the fraction
        for x in [2.0f64, 4.0, 5.5, 10.0, 20.0] {
            let erlang = 1.0 - (-x).exp() * (1.0 + x + x * x / 2.0 + x * x * x / 6.0);
            close(gamma_p(4.0, x), erlang, 1e-12);
        }
    }

    #[test]
    fn gamma_p_edges_and_complement() {
        assert_eq!(gamma_p(3.0, 0.0), 0.0);
        assert_eq!(gamma_p(3.0, -1.0), 0.0);
        let (p, q) = gamma_pq(2.5, 3.0);
        close(p + q, 1.0, 1e-15);
        close(gamma_p(0.5, 0.25), 0.520_499_877_813_046_5, 1e-12); // erf(0.5)
                                                                   // small x: P(a, x) = x^a / Gamma(a + 1) to first order
        close(gamma_p(0.2, 1e-9), 0.017_261_2, 1e-5);
    }

    #[test]
    fn gamma_p_large_shapes_stay_close_across_the_approximation_threshold() {
        let (a0, a1) = (WILSON_HILFERTY_ABOVE - 1.0, WILSON_HILFERTY_ABOVE + 1.0);
        let (below, above) = (gamma_p(a0, a0), gamma_p(a1, a1));
        close(below, above, 1e-4);
        // P(a, a) = 1/2 + 1 / (3 sqrt(2 pi a)) + ...
        close(
            gamma_p(1e6, 1e6),
            0.5 + 1.0 / (3.0 * (2.0 * std::f64::consts::PI * 1e6).sqrt()),
            1e-6,
        );
        assert_eq!(gamma_p(1e18, 1e18 * 2.0), 1.0);
    }

    fn delayer(d: DistributionDto, mean: f64, std: f64, corr: bool) -> DelayerDto {
        DelayerDto {
            distribution: d,
            mean_ns: mean,
            std_ns: std,
            activate_correlation: corr,
            half_life_period_ns: 1e6,
        }
    }

    #[test]
    fn the_parameterisation_is_the_one_of_dpds_core() {
        let (m, exact) = DelayModel::from_delayer(&delayer(
            DistributionDto::LogNormalDistribution,
            1000.0,
            500.0,
            false,
        ));
        assert!(exact);
        let DelayModel::LogNormal { mu, sigma } = m else {
            panic!("{m:?}")
        };
        close(sigma * sigma, 1.25f64.ln(), 1e-15);
        close(mu, 1000f64.ln() - 1.25f64.ln() / 2.0, 1e-15);
        // the median of a lognormal is e^mu
        close(m.below(mu.exp()), 0.5, 1e-13);

        let (m, _) = DelayModel::from_delayer(&delayer(
            DistributionDto::GammaDistribution,
            1000.0,
            500.0,
            false,
        ));
        assert_eq!(
            m,
            DelayModel::Gamma {
                shape: 4.0,
                scale: 250.0
            }
        );
        let (m, _) = DelayModel::from_delayer(&delayer(
            DistributionDto::UniformDistribution,
            1000.0,
            100.0,
            false,
        ));
        let DelayModel::Uniform { lo, hi } = m else {
            panic!("{m:?}")
        };
        close(lo, 1000.0 - 100.0 * 3f64.sqrt(), 1e-12);
        close(hi, 1000.0 + 100.0 * 3f64.sqrt(), 1e-12);
    }

    #[test]
    fn correlation_approximates_by_the_normal_except_for_deterministic_and_zero_std() {
        for d in [
            DistributionDto::NormalDistribution,
            DistributionDto::LogNormalDistribution,
            DistributionDto::GammaDistribution,
            DistributionDto::UniformDistribution,
        ] {
            let (m, exact) = DelayModel::from_delayer(&delayer(d, 1000.0, 300.0, true));
            assert_eq!(
                m,
                DelayModel::Normal {
                    mean: 1000.0,
                    std: 300.0
                }
            );
            assert!(!exact);
        }
        let (m, exact) = DelayModel::from_delayer(&delayer(
            DistributionDto::DeterministicDistribution,
            1000.0,
            300.0,
            true,
        ));
        assert_eq!((m, exact), (DelayModel::Point(1000.0), true));
        let (m, exact) = DelayModel::from_delayer(&delayer(
            DistributionDto::NormalDistribution,
            1000.0,
            0.0,
            true,
        ));
        assert_eq!((m, exact), (DelayModel::Point(1000.0), true));
    }

    #[test]
    fn unusable_parameters_become_a_point_mass() {
        for d in [
            DistributionDto::NormalDistribution,
            DistributionDto::LogNormalDistribution,
            DistributionDto::GammaDistribution,
            DistributionDto::UniformDistribution,
        ] {
            let (m, _) = DelayModel::from_delayer(&delayer(d, 500.0, 0.0, false));
            assert_eq!(m, DelayModel::Point(500.0));
        }
        for d in [
            DistributionDto::LogNormalDistribution,
            DistributionDto::GammaDistribution,
        ] {
            let (m, _) = DelayModel::from_delayer(&delayer(d, 0.0, 10.0, false));
            assert_eq!(m, DelayModel::Point(0.0));
        }
    }

    #[test]
    fn expected_counts_telescope_to_the_total_and_clamp_at_zero() {
        // Normal(1000, 1000), 8 bins of 500 from 0: bin 0 holds P(X < 500) = Phi(-0.5)
        let m = DelayModel::Normal {
            mean: 1000.0,
            std: 1000.0,
        };
        let (bins, under, over) = m.expected_counts(0.0, 500.0, 8, 10_000);
        close(bins[0], 3_085.375_387_26, 1e-6);
        close(bins[1], 1_914.624_612_74, 1e-6);
        assert_eq!(under, 0.0);
        close(over, 13.498_980_316, 1e-6);
        close(bins.iter().sum::<f64>() + under + over, 10_000.0, 1e-9);
    }

    #[test]
    fn a_range_starting_above_zero_puts_the_negative_mass_in_the_underflow() {
        let m = DelayModel::Normal {
            mean: 100.0,
            std: 200.0,
        };
        let (_, under, _) = m.expected_counts(50.0, 100.0, 4, 10_000);
        close(under, 4_012.936_743_171, 1e-6);
    }

    #[test]
    fn a_point_mass_lands_in_the_bin_of_its_value() {
        let m = DelayModel::Point(2700.0);
        let (bins, under, over) = m.expected_counts(0.0, 500.0, 8, 100);
        assert_eq!((under, over), (0.0, 0.0));
        assert_eq!(bins[5], 100.0);
        // exactly on an edge: the bin that starts there
        let (bins, ..) = DelayModel::Point(2500.0).expected_counts(0.0, 500.0, 8, 100);
        assert_eq!(bins[5], 100.0);
        // the upper edge itself is already overflow
        let (bins, _, over) = DelayModel::Point(4000.0).expected_counts(0.0, 500.0, 8, 100);
        assert_eq!((bins.iter().sum::<f64>(), over), (0.0, 100.0));
        // zero delay: bin 0 of a range from 0
        let (bins, under, _) = DelayModel::Point(0.0).expected_counts(0.0, 500.0, 8, 100);
        assert_eq!((bins[0], under), (100.0, 0.0));
        // negative means cannot happen (validated), but would clamp to 0 as well
        let (bins, ..) = DelayModel::Point(-5.0).expected_counts(0.0, 500.0, 8, 100);
        assert_eq!(bins[0], 100.0);
    }

    #[test]
    fn uniform_counts_are_linear_and_clamped() {
        let m = DelayModel::Uniform {
            lo: -100.0,
            hi: 300.0,
        };
        let (bins, under, over) = m.expected_counts(0.0, 100.0, 3, 400);
        assert_eq!((under, over), (0.0, 0.0));
        // a quarter of the mass is negative and sits in bin 0
        close(bins[0], 200.0, 1e-9);
        close(bins[1], 100.0, 1e-9);
        close(bins[2], 100.0, 1e-9);
    }
}
