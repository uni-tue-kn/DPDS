use rand::rngs::SmallRng;
use rand::SeedableRng;
use rand_distr::{Distribution, Gamma, LogNormal, Normal, Uniform};

use crate::BuildError;

const SQRT_3: f64 = 1.7320508075688772;

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RVDistribution {
    DeterministicDistribution,
    LogNormalDistribution,
    NormalDistribution,
    GammaDistribution,
    UniformDistribution,
}

enum RV {
    Deterministic(f64),
    LogNormal(LogNormal<f64>),
    Normal(Normal<f64>),
    Gamma(Gamma<f64>),
    Uniform(Uniform<f64>),
}

struct HalfLifePeriodCounter {
    update_period_ns: f64,
    last_update_ns: f64,
    tracked_packets: u64,
}

impl HalfLifePeriodCounter {
    fn new() -> HalfLifePeriodCounter {
        HalfLifePeriodCounter {
            update_period_ns: 1_000_000., // Update every 1ms
            last_update_ns: 0.,
            tracked_packets: 0,
        }
    }

    fn track_packet_ppns(&mut self, current_time_ns: f64) -> Option<f64> {
        // Set the last_update_ns for the first packet to track to the current time
        if self.tracked_packets == 0 {
            self.last_update_ns = current_time_ns;
        }

        let duration_since_last_update_ns = (current_time_ns - self.last_update_ns).max(0.);

        let result = if duration_since_last_update_ns > self.update_period_ns {
            let rate_ppns = self.tracked_packets as f64 / duration_since_last_update_ns;
            self.last_update_ns = current_time_ns;
            self.tracked_packets = 0;

            Some(rate_ppns)
        } else {
            None
        };

        // Track this packet for the next rate
        self.tracked_packets += 1;
        result
    }
}

pub struct Delayer {
    rng: SmallRng,
    distribution: RV,
    rv_distribution: RVDistribution,
    configured_mean_ns: f64,
    configured_std_ns: f64,
    configured_half_life_period_ns: f64,
    activate_correlation: bool,
    correlation_weight: f64,
    x_im1_ns: f64,
    half_life_period_counter: HalfLifePeriodCounter,
}

impl Delayer {
    fn calculate_correlation_weight(half_life_period_ns: f64, rate_ppns: f64) -> f64 {
        let k = half_life_period_ns * rate_ppns;
        1. / 2f64.powf(1. / k)
    }

    fn calculate_correlated_std_ns(std_ns: f64, correlation_weight: f64) -> f64 {
        (((1f64 + correlation_weight) / (1f64 - correlation_weight)) * std_ns * std_ns).sqrt()
    }

    fn initialize_rv(
        mean_ns: f64,
        std_ns: f64,
        rv_distribution: RVDistribution,
    ) -> Result<RV, BuildError> {
        let invalid = |reason: String| BuildError::Distribution {
            distribution: rv_distribution,
            mean_ns,
            std_ns,
            reason,
        };
        Ok(match rv_distribution {
            RVDistribution::DeterministicDistribution => RV::Deterministic(mean_ns),
            RVDistribution::NormalDistribution => {
                RV::Normal(Normal::new(mean_ns, std_ns).map_err(|e| invalid(e.to_string()))?)
            }
            RVDistribution::LogNormalDistribution => RV::LogNormal(
                LogNormal::from_mean_cv(mean_ns, std_ns / mean_ns).map_err(|e| {
                    invalid(format!(
                        "coefficient of variation std_ns / mean_ns = {}: {e}",
                        std_ns / mean_ns
                    ))
                })?,
            ),
            RVDistribution::GammaDistribution => {
                let alpha = (mean_ns / std_ns) * (mean_ns / std_ns);
                let beta = (std_ns * std_ns) / mean_ns;
                RV::Gamma(
                    Gamma::new(alpha, beta)
                        .map_err(|e| invalid(format!("alpha = {alpha}, beta = {beta}: {e}")))?,
                )
            }
            RVDistribution::UniformDistribution => {
                let low = mean_ns - SQRT_3 * std_ns;
                let high = mean_ns + SQRT_3 * std_ns;
                RV::Uniform(
                    Uniform::new(low, high)
                        .map_err(|e| invalid(format!("low = {low}, high = {high}: {e}")))?,
                )
            }
        })
    }

    /// Creates a delayer seeded from the OS entropy source.
    ///
    /// # Panics
    ///
    /// If the distribution rejects `mean_ns` / `std_ns`, or `half_life_period_ns`
    /// is negative, NaN or infinite, or with correlation so long that its
    /// weight is not below 1; use
    /// [`Delayer::try_new`] to get an error instead.
    pub fn new(
        rv_distribution: RVDistribution,
        mean_ns: f64,
        std_ns: f64,
        activate_correlation: bool,
        half_life_period_ns: f64,
        initialization_rate_pps: f64,
    ) -> Self {
        Self::try_new(
            rv_distribution,
            mean_ns,
            std_ns,
            activate_correlation,
            half_life_period_ns,
            initialization_rate_pps,
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    /// Like [`Delayer::new`], but returns an error for invalid distribution
    /// parameters or a `half_life_period_ns` that is negative, NaN or infinite
    /// (0 is valid) instead of panicking. An `initialization_rate_pps` that is
    /// not finite or below 1 is an error too, with or without correlation. With
    /// `activate_correlation`, a half-life so long that the smoothing weight at
    /// `initialization_rate_pps` is not below 1 is an error as well. Only the
    /// initial parameters are validated, see [`Delayer::delayer_sample_ns`].
    pub fn try_new(
        rv_distribution: RVDistribution,
        mean_ns: f64,
        std_ns: f64,
        activate_correlation: bool,
        half_life_period_ns: f64,
        initialization_rate_pps: f64,
    ) -> Result<Self, BuildError> {
        Self::build(
            rv_distribution,
            mean_ns,
            std_ns,
            activate_correlation,
            half_life_period_ns,
            initialization_rate_pps,
            SmallRng::from_rng(&mut rand::rng()),
        )
    }

    /// Like [`Delayer::try_new`], but with a deterministic RNG: the same seed
    /// gives the same samples for the same sequence of calls.
    pub fn try_new_seeded(
        rv_distribution: RVDistribution,
        mean_ns: f64,
        std_ns: f64,
        activate_correlation: bool,
        half_life_period_ns: f64,
        initialization_rate_pps: f64,
        seed: u64,
    ) -> Result<Self, BuildError> {
        Self::build(
            rv_distribution,
            mean_ns,
            std_ns,
            activate_correlation,
            half_life_period_ns,
            initialization_rate_pps,
            SmallRng::seed_from_u64(seed),
        )
    }

    fn build(
        rv_distribution: RVDistribution,
        mean_ns: f64,
        std_ns: f64,
        activate_correlation: bool,
        half_life_period_ns: f64,
        initialization_rate_pps: f64,
        rng: SmallRng,
    ) -> Result<Self, BuildError> {
        if !initialization_rate_pps.is_finite() || initialization_rate_pps < 1. {
            return Err(BuildError::InitializationRate {
                initialization_rate_pps,
            });
        }
        if !half_life_period_ns.is_finite() || half_life_period_ns < 0. {
            return Err(BuildError::HalfLifePeriod {
                half_life_period_ns,
            });
        }
        let configured_mean_ns = mean_ns;
        let configured_std_ns = std_ns;
        let mut std_ns = std_ns;

        let mut weight = 0.;
        let initialization_rate_ppns = initialization_rate_pps / 1e9;

        if activate_correlation {
            weight = Delayer::calculate_correlation_weight(
                half_life_period_ns,
                initialization_rate_ppns,
            );
            // A weight of 1 (a half-life so long that 2^(-1/k) rounds to 1)
            // has no finite innovation std and would freeze the delay.
            if weight.is_nan() || weight >= 1. {
                return Err(BuildError::HalfLifePeriod {
                    half_life_period_ns,
                });
            }
            std_ns = Delayer::calculate_correlated_std_ns(configured_std_ns, weight);
        }

        let distribution = Delayer::initialize_rv(configured_mean_ns, std_ns, rv_distribution)?;

        Ok(Delayer {
            rng,
            rv_distribution,
            configured_mean_ns,
            configured_std_ns,
            configured_half_life_period_ns: half_life_period_ns,
            activate_correlation,
            distribution,
            correlation_weight: weight,
            x_im1_ns: configured_mean_ns,
            half_life_period_counter: HalfLifePeriodCounter::new(),
        })
    }

    /// Samples a delay (in ns) for a packet arriving at `current_time_ns`.
    ///
    /// When correlation is active, the rate estimate is refreshed every 1 ms
    /// and the sampled delay is smoothed against the previous one (EMA).
    /// Negative samples are clamped to zero.
    ///
    /// # Panics
    ///
    /// With correlation, if the distribution rejects the parameters derived
    /// from a refreshed rate estimate (a huge finite half-life combined with a
    /// much higher measured rate than the initialisation rate can overflow).
    pub fn delayer_sample_ns(&mut self, current_time_ns: f64) -> f64 {
        if self.activate_correlation {
            if let Some(rate_ppns) = self
                .half_life_period_counter
                .track_packet_ppns(current_time_ns)
            {
                self.correlation_weight = Delayer::calculate_correlation_weight(
                    self.configured_half_life_period_ns,
                    rate_ppns,
                );

                let new_std_ns = Delayer::calculate_correlated_std_ns(
                    self.configured_std_ns,
                    self.correlation_weight,
                );

                // Inline storage: old distribution is dropped in place, no heap churn.
                self.distribution = Delayer::initialize_rv(
                    self.configured_mean_ns,
                    new_std_ns,
                    self.rv_distribution,
                )
                .unwrap_or_else(|e| panic!("{e}"));
            }
        }

        let r_i = match &self.distribution {
            RV::Deterministic(mean) => *mean,
            RV::LogNormal(dist) => dist.sample(&mut self.rng),
            RV::Normal(dist) => dist.sample(&mut self.rng),
            RV::Gamma(dist) => dist.sample(&mut self.rng),
            RV::Uniform(dist) => dist.sample(&mut self.rng),
        };

        let delay = if self.activate_correlation {
            let x_i =
                self.correlation_weight * self.x_im1_ns + (1. - self.correlation_weight) * r_i;
            self.x_im1_ns = x_i;
            x_i
        } else {
            r_i
        };

        // Clamp negative delays to 0 (e.g. from distributions with heavy left tails)
        delay.max(0.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_distribution_returns_the_mean() {
        let mut d = Delayer::new(
            RVDistribution::DeterministicDistribution,
            1000.,
            0.,
            false,
            50_000_000.,
            15_000_000.,
        );
        assert_eq!(d.delayer_sample_ns(0.), 1000.);
        assert_eq!(d.delayer_sample_ns(1_000_000.), 1000.);
    }

    #[test]
    fn samples_are_never_negative() {
        let mut d = Delayer::new(
            RVDistribution::NormalDistribution,
            10.,
            1000., // large std relative to mean -> would otherwise go negative
            false,
            50_000_000.,
            15_000_000.,
        );
        for i in 0..1000 {
            assert!(d.delayer_sample_ns(i as f64) >= 0.);
        }
    }
}
