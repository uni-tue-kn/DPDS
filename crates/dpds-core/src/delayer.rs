use rand::rngs::SmallRng;
use rand::SeedableRng;
use rand_distr::{Distribution, Gamma, LogNormal, Normal, Uniform};
use serde::{Deserialize, Serialize};

const SQRT_3: f64 = 1.7320508075688772;

#[repr(C)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
            update_period_ns: 10_000_000., // Update every 10ms
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

    fn initialize_rv(mean_ns: f64, std_ns: f64, rv_distribution: RVDistribution) -> RV {
        match rv_distribution {
            RVDistribution::DeterministicDistribution => RV::Deterministic(mean_ns),
            RVDistribution::NormalDistribution => {
                RV::Normal(Normal::new(mean_ns, std_ns).unwrap_or_else(|_| {
                    panic!("No Normal distribution with mean {mean_ns} and std {std_ns} possible!")
                }))
            }
            RVDistribution::LogNormalDistribution => RV::LogNormal(
                LogNormal::from_mean_cv(mean_ns, std_ns / mean_ns).unwrap_or_else(|_| {
                    panic!(
                        "No LogNormal distribution with mean {mean_ns} and cvar {} possible!",
                        std_ns / mean_ns
                    )
                }),
            ),
            RVDistribution::GammaDistribution => {
                let alpha = (mean_ns / std_ns) * (mean_ns / std_ns);
                let beta = (std_ns * std_ns) / mean_ns;
                RV::Gamma(Gamma::new(alpha, beta).unwrap_or_else(|_| {
                    panic!("No Gamma distribution with alpha {alpha} and beta {beta} possible!")
                }))
            }
            RVDistribution::UniformDistribution => {
                let low = mean_ns - SQRT_3 * std_ns;
                let high = mean_ns + SQRT_3 * std_ns;
                RV::Uniform(Uniform::new(low, high).unwrap_or_else(|_| {
                    panic!("No Uniform distribution with low {low} and high {high} possible!")
                }))
            }
        }
    }

    pub fn new(
        rv_distribution: RVDistribution,
        mean_ns: f64,
        std_ns: f64,
        activate_correlation: bool,
        half_life_period_ns: f64,
        initialization_rate_pps: f64,
    ) -> Self {
        let rng = SmallRng::from_rng(&mut rand::rng());

        let configured_mean_ns = mean_ns;
        let configured_std_ns = std_ns;
        let mut std_ns = std_ns;

        let mut weight = 0.;
        let initialization_rate_ppns = initialization_rate_pps / 1e9;

        if activate_correlation {
            std_ns = Delayer::calculate_correlated_std_ns(configured_std_ns, weight);
            weight = Delayer::calculate_correlation_weight(
                half_life_period_ns,
                initialization_rate_ppns,
            );
        }

        let distribution = Delayer::initialize_rv(configured_mean_ns, std_ns, rv_distribution);

        Delayer {
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
        }
    }

    /// Samples a delay (in ns) for a packet arriving at `current_time_ns`.
    ///
    /// When correlation is active, the rate estimate is refreshed once per
    /// half-life update period and the sampled delay is smoothed against the
    /// previous one (EMA). Negative samples are clamped to zero.
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
                );
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
