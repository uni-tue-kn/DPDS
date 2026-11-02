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

/// Most bins a delay histogram may have (DPDK telemetry arrays hold 512 entries).
pub const MAX_HISTOGRAM_BINS: u32 = 500;

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
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LimiterDto {
    pub mode: LimiterModeDto,
    pub activate: bool,
    pub rate_bps: f64,
    pub limit_bit: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
pub enum LossModelDto {
    Random {
        p: f64,
    },
    GilbertElliot {
        p: f64,
        r: f64,
        k: f64,
        h: f64,
        #[serde(default = "default_true")]
        state_is_good: bool,
    },
}

fn default_true() -> bool {
    true
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
    /// 1 <= bins <= 500, `lo_ns` and `hi_ns` finite and not negative, `lo_ns < hi_ns`.
    pub fn validate(&self) -> Result<(), DtoError> {
        let bad = |m: String| Err(DtoError(m));
        if let Some(n) = self.bins {
            if !(1..=MAX_HISTOGRAM_BINS).contains(&n) {
                return bad(format!(
                    "histogram bins = {n}: must be between 1 and {MAX_HISTOGRAM_BINS}"
                ));
            }
        }
        for (name, v) in [("lo_ns", self.lo_ns), ("hi_ns", self.hi_ns)] {
            if let Some(v) = v {
                if !v.is_finite() || v < 0.0 {
                    return bad(format!(
                        "histogram {name} = {v}: must be finite and not negative"
                    ));
                }
            }
        }
        if let (Some(lo), Some(hi)) = (self.lo_ns, self.hi_ns) {
            if lo >= hi {
                return bad(format!("histogram lo_ns = {lo} must be below hi_ns = {hi}"));
            }
        }
        Ok(())
    }
}

/// Mirror of `dpds_core::Config` plus the optional `histogram` of the direction
/// (a `dpds` option that lives beside, not inside, the core config).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigDto {
    pub buffer_capacity: usize,
    pub reordering: bool,
    pub delayer: DelayerDto,
    pub limiter: LimiterDto,
    pub loss_model: LossModelDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram: Option<HistogramDto>,
    /// Record the intended delay, lateness and histogram of this direction
    /// (off: `dpds` skips that work; counters and drop causes stay).
    #[serde(default = "default_true")]
    pub delay_stats: bool,
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

impl From<RVDistribution> for DistributionDto {
    fn from(d: RVDistribution) -> Self {
        match d {
            RVDistribution::DeterministicDistribution => DistributionDto::DeterministicDistribution,
            RVDistribution::LogNormalDistribution => DistributionDto::LogNormalDistribution,
            RVDistribution::NormalDistribution => DistributionDto::NormalDistribution,
            RVDistribution::GammaDistribution => DistributionDto::GammaDistribution,
            RVDistribution::UniformDistribution => DistributionDto::UniformDistribution,
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

impl From<LimiterMode> for LimiterModeDto {
    fn from(m: LimiterMode) -> Self {
        match m {
            LimiterMode::Spacer => LimiterModeDto::Spacer,
            LimiterMode::TokenBucketFilter => LimiterModeDto::TokenBucketFilter,
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
        let loss_model = match self.loss_model {
            LossModelDto::Random { p } => {
                probability("p", p)?;
                LossModel::Random(p)
            }
            LossModelDto::GilbertElliot {
                p,
                r,
                k,
                h,
                state_is_good,
            } => {
                for (name, v) in [("p", p), ("r", r), ("k", k), ("h", h)] {
                    probability(name, v)?;
                }
                LossModel::GilbertElliot(p, r, k, h, state_is_good)
            }
        };
        for (name, v) in [
            ("mean_ns", self.delayer.mean_ns),
            ("std_ns", self.delayer.std_ns),
        ] {
            if !v.is_finite() || v < 0.0 {
                return Err(DtoError(format!(
                    "delayer {name} = {v}: must be finite and not negative"
                )));
            }
        }
        let l = &self.limiter;
        if l.activate && !(l.rate_bps.is_finite() && l.rate_bps > 0.0) {
            return Err(DtoError(format!(
                "limiter rate_bps = {}: must be positive when the limiter is active",
                l.rate_bps
            )));
        }
        if l.activate && !(l.limit_bit.is_finite() && l.limit_bit >= 0.0) {
            return Err(DtoError(format!(
                "limiter limit_bit = {}: must not be negative when the limiter is active",
                l.limit_bit
            )));
        }
        let config = Config {
            buffer_capacity: self.buffer_capacity,
            reordering: self.reordering,
            delayer: DelayerConfig {
                distribution: self.delayer.distribution.into(),
                mean_ns: self.delayer.mean_ns,
                std_ns: self.delayer.std_ns,
                activate_correlation: self.delayer.activate_correlation,
                half_life_period_ns: self.delayer.half_life_period_ns,
            },
            limiter: LimiterConfig {
                mode: l.mode.into(),
                activate: l.activate,
                rate_bps: l.rate_bps,
                limit_bit: l.limit_bit,
            },
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
        Ok(config)
    }

    /// The DTO of a core config (without histogram settings).
    pub fn from_core(c: &Config) -> ConfigDto {
        ConfigDto {
            buffer_capacity: c.buffer_capacity,
            reordering: c.reordering,
            delayer: DelayerDto {
                distribution: c.delayer.distribution.into(),
                mean_ns: c.delayer.mean_ns,
                std_ns: c.delayer.std_ns,
                activate_correlation: c.delayer.activate_correlation,
                half_life_period_ns: c.delayer.half_life_period_ns,
            },
            limiter: LimiterDto {
                mode: c.limiter.mode.into(),
                activate: c.limiter.activate,
                rate_bps: c.limiter.rate_bps,
                limit_bit: c.limiter.limit_bit,
            },
            loss_model: match c.loss_model {
                LossModel::Random(p) => LossModelDto::Random { p },
                LossModel::GilbertElliot(p, r, k, h, state_is_good) => {
                    LossModelDto::GilbertElliot {
                        p,
                        r,
                        k,
                        h,
                        state_is_good,
                    }
                }
            },
            seed: c.seed,
            histogram: None,
            delay_stats: true,
        }
    }
}
