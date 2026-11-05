//! `ConfigDto`: the REST/storage form of `dpds_core::Config`, with named
//! fields (also for Gilbert-Elliot) and a schema for the OpenAPI document.
//! `dpds-core` itself stays free of utoipa.

use std::fmt;

use dpds_core::buffer::Buffer;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Most bins a delay histogram may have. `dpds` serves it in pages of 500 bins (DPDK
/// telemetry arrays hold 512 entries), see `telemetry::poll_once`.
pub const MAX_HISTOGRAM_BINS: u32 = 4096;

/// Largest `buffer_capacity` (the hugepage need and the allocation grow with it).
pub const MAX_BUFFER_CAPACITY: u64 = 1 << 31;

#[derive(Debug, Clone)]
pub struct DtoError(String);

impl fmt::Display for DtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DtoError {}

impl DtoError {
    pub fn new(message: String) -> DtoError {
        DtoError(message)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub enum DistributionDto {
    DeterministicDistribution,
    LogNormalDistribution,
    NormalDistribution,
    GammaDistribution,
    UniformDistribution,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub enum LimiterModeDto {
    Spacer,
    TokenBucketFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DelayerDto {
    pub distribution: DistributionDto,
    pub mean_ns: f64,
    pub std_ns: f64,
    pub activate_correlation: bool,
    pub half_life_period_ns: f64,
    /// Required, no default: the packet rate (pps) the correlated delay starts
    /// with. Finite and at least 1.
    pub initialization_rate_pps: f64,
}

impl DelayerDto {
    /// The delayer of a link without one: no delay. Its `initialization_rate_pps` is
    /// never used (correlation is off); 1 is the smallest value `dpds-core` accepts.
    pub fn none() -> DelayerDto {
        DelayerDto {
            distribution: DistributionDto::DeterministicDistribution,
            mean_ns: 0.0,
            std_ns: 0.0,
            activate_correlation: false,
            half_life_period_ns: 0.0,
            initialization_rate_pps: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LimiterDto {
    pub mode: LimiterModeDto,
    pub rate_bps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
pub enum LossModelDto {
    Random { p: f64 },
    GilbertElliot { p: f64, r: f64, k: f64, h: f64 },
}

/// Optional delay histogram settings of a direction; anything omitted is
/// chosen automatically by `dpds` (256 bins, mean +- 6 std, at least 1 us wide).
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HistogramDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bins: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lo_ns: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hi_ns: Option<f64>,
}

impl HistogramDto {
    /// 1 <= bins <= 4096, `lo_ns` and `hi_ns` finite and not negative, `lo_ns < hi_ns`.
    pub fn validate(&self) -> Result<(), DtoError> {
        self.validate_as("histogram")
    }

    /// [`HistogramDto::validate`] for the IAT histogram (messages name "IAT histogram").
    pub fn validate_iat(&self) -> Result<(), DtoError> {
        self.validate_as("IAT histogram")
    }

    fn validate_as(&self, what: &str) -> Result<(), DtoError> {
        let bad = |m: String| Err(DtoError(m));
        if let Some(n) = self.bins {
            if !(1..=MAX_HISTOGRAM_BINS).contains(&n) {
                return bad(format!(
                    "{what} bins = {n}: must be between 1 and {MAX_HISTOGRAM_BINS}"
                ));
            }
        }
        for (name, v) in [("lo_ns", self.lo_ns), ("hi_ns", self.hi_ns)] {
            if let Some(v) = v {
                if !v.is_finite() || v < 0.0 {
                    return bad(format!(
                        "{what} {name} = {v}: must be finite and not negative"
                    ));
                }
            }
        }
        if let (Some(lo), Some(hi)) = (self.lo_ns, self.hi_ns) {
            if lo >= hi {
                return bad(format!("{what} lo_ns = {lo} must be below hi_ns = {hi}"));
            }
        }
        Ok(())
    }
}

/// Mirror of `dpds_core::Config` plus the optional `histogram` of the direction
/// (a `dpds` option that lives beside, not inside, the core config). A stage that is
/// `null` or absent is not added: it maps to a no-op stage of the core config.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigDto {
    pub buffer_capacity: usize,
    pub reordering: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delayer: Option<DelayerDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limiter: Option<LimiterDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_model: Option<LossModelDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram: Option<HistogramDto>,
    /// Optional IAT histogram settings, the same shape as `histogram`; omitted fields
    /// keep the defaults (`lo_ns` 0, `hi_ns` 4e9 / `initialization_rate_pps`, 256 bins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iat_histogram: Option<HistogramDto>,
    /// Record the intended delay, lateness and histogram of this direction (absent: on;
    /// off: `dpds` skips that work, counters and drop causes stay). Kept as sent, so the
    /// running configuration echoes the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_stats: Option<bool>,
}

impl From<DistributionDto> for RVDistribution {
    fn from(d: DistributionDto) -> Self {
        match d {
            DistributionDto::DeterministicDistribution => RVDistribution::DeterministicDistribution,
            DistributionDto::LogNormalDistribution => RVDistribution::LogNormalDistribution,
            DistributionDto::NormalDistribution => RVDistribution::NormalDistribution,
            DistributionDto::GammaDistribution => RVDistribution::GammaDistribution,
            DistributionDto::UniformDistribution => RVDistribution::UniformDistribution,
        }
    }
}

impl From<LimiterModeDto> for LimiterMode {
    fn from(m: LimiterModeDto) -> Self {
        match m {
            LimiterModeDto::Spacer => LimiterMode::Spacer,
            LimiterModeDto::TokenBucketFilter => LimiterMode::TokenBucketFilter,
        }
    }
}

fn probability(name: &str, p: f64) -> Result<(), DtoError> {
    if (0.0..=1.0).contains(&p) {
        Ok(())
    } else {
        Err(DtoError(format!(
            "loss_model {name} = {p}: must be a probability between 0 and 1"
        )))
    }
}

impl ConfigDto {
    /// Converts and validates: delay mean/std, loss probabilities and limiter
    /// rate here (`dpds-core` accepts a negative std), the rest (distribution
    /// parameters, half-life) by building a `Buffer` of capacity zero, which
    /// allocates nothing.
    pub fn to_core(&self) -> Result<Config, DtoError> {
        if self.buffer_capacity as u64 > MAX_BUFFER_CAPACITY {
            return Err(DtoError(format!(
                "buffer_capacity = {}: must be at most {MAX_BUFFER_CAPACITY} (2^31)",
                self.buffer_capacity
            )));
        }
        let loss_model = match &self.loss_model {
            None => LossModel::Random(0.0),
            Some(LossModelDto::Random { p }) => {
                probability("p", *p)?;
                LossModel::Random(*p)
            }
            Some(LossModelDto::GilbertElliot { p, r, k, h }) => {
                for (name, v) in [("p", p), ("r", r), ("k", k), ("h", h)] {
                    probability(name, *v)?;
                }
                // dpds-core always starts the model in the Good state.
                LossModel::GilbertElliot(*p, *r, *k, *h, true)
            }
        };
        let d = self.delayer.clone().unwrap_or_else(DelayerDto::none);
        for (name, v) in [("mean_ns", d.mean_ns), ("std_ns", d.std_ns)] {
            if !v.is_finite() || v < 0.0 {
                return Err(DtoError(format!(
                    "delayer {name} = {v}: must be finite and not negative"
                )));
            }
        }
        let limiter = match &self.limiter {
            None => LimiterConfig {
                mode: LimiterMode::Spacer,
                activate: false,
                rate_bps: 0.0,
            },
            Some(l) => {
                if !(l.rate_bps.is_finite() && l.rate_bps > 0.0) {
                    return Err(DtoError(format!(
                        "limiter rate_bps = {}: must be positive",
                        l.rate_bps
                    )));
                }
                LimiterConfig {
                    mode: l.mode.into(),
                    activate: true,
                    rate_bps: l.rate_bps,
                }
            }
        };
        let config = Config {
            buffer_capacity: self.buffer_capacity,
            reordering: self.reordering,
            delayer: DelayerConfig {
                distribution: d.distribution.into(),
                mean_ns: d.mean_ns,
                std_ns: d.std_ns,
                activate_correlation: d.activate_correlation,
                half_life_period_ns: d.half_life_period_ns,
                initialization_rate_pps: d.initialization_rate_pps,
            },
            limiter,
            loss_model,
            seed: self.seed,
        };
        Buffer::try_new(Config {
            buffer_capacity: 0,
            ..config
        })
        .map_err(|e| DtoError(e.to_string()))?;
        if let Some(h) = &self.histogram {
            h.validate()?;
        }
        if let Some(h) = &self.iat_histogram {
            h.validate_iat()?;
        }
        Ok(config)
    }
}
