//! # dpds-core
//!
//! The core of DPDS, a software network-link emulator: it models the three
//! effects a link imposes on packets -- **delay**, **rate limiting** and
//! **packet loss** -- without knowing anything about the packets themselves.
//! The crate is DPDK-free; the host (a DPDK data plane, a simulator, a test)
//! owns the packets and asks the [`Buffer`](buffer::Buffer) *when* and
//! *whether* each one should leave.
//!
//! ## The loop
//!
//! Build a [`Buffer`](buffer::Buffer) from a [`Config`], then per iteration:
//! read the clock once, `push` every arriving packet (a rejected packet was
//! dropped by the loss model or the limiter, or the buffer is full: the caller
//! still owns and must free it), and `pop_due` until nothing is due any more.
//!
//! ```
//! use std::ffi::c_void;
//! use dpds_core::{buffer::Buffer, Config, DelayerConfig, LimiterConfig};
//! use dpds_core::delayer::RVDistribution;
//! use dpds_core::dropper::LossModel;
//! use dpds_core::limiter::LimiterMode;
//!
//! let config = Config {
//!     buffer_capacity: 1024,
//!     reordering: false,
//!     delayer: DelayerConfig {
//!         distribution: RVDistribution::DeterministicDistribution,
//!         mean_ns: 1_000.0, // 1 us of delay
//!         std_ns: 0.0,
//!         activate_correlation: false,
//!         half_life_period_ns: 1.0,
//!     },
//!     limiter: LimiterConfig {
//!         mode: LimiterMode::Spacer,
//!         activate: false,
//!         rate_bps: 1e9,
//!         limit_bit: 1e6,
//!     },
//!     loss_model: LossModel::Random(0.0),
//!     seed: Some(42),
//! };
//! let mut buf = Buffer::try_new(config).expect("valid configuration");
//!
//! // A packet is an opaque pointer; here just a number. `now` is any
//! // monotonic clock in nanoseconds.
//! let packet = 1usize as *mut c_void;
//! assert!(buf.push(packet, 1500, /* now_ns */ 0.0));
//!
//! assert_eq!(buf.pop_due(999.0), None); // not due yet
//! assert_eq!(buf.pop_due(1_000.0), Some(packet)); // due at send time (`<=`)
//! assert!(buf.is_empty());
//! ```
//!
//! ## Ownership and threads
//!
//! The buffer stores the packet pointers (`*mut c_void`) but never
//! dereferences, copies or frees them: the caller keeps each pointer valid
//! until it is popped. Because of the raw pointers `Buffer` is not `Send`; a
//! host that moves one to another thread must vouch that it owns the packets.
//!
//! ## Seeding
//!
//! [`Config::seed`] `= None` seeds the delay and loss random number generators
//! from the OS entropy source. `Some(s)` seeds both deterministically (and
//! differently from each other), so the same seed and the same input sequence
//! give identical push results and send times. Seeded streams are reproducible
//! only for the same `rand` version and the same target word size.
//!
//! **API change:** `Config.seed` is a new public field and `Config` is not
//! `#[non_exhaustive]`, so every `Config { .. }` struct literal needs
//! `seed: None` (JSON files need no change).
//!
//! ## Ordering
//!
//! With `reordering = false` the buffer is a FIFO: packets leave in push order
//! and a packet with a late send time blocks the ones behind it. With
//! `reordering = true` packets leave in send-time order. Packets that enter at
//! the same instant (same `now_ns`) and get the same send time keep their entry
//! order: every packet's send time gets a tie offset of 1/64 ns per earlier
//! packet of that instant. See [`buffer::Buffer`] for the exact rule.
//!
//! Caveat: a burst of n packets pushed at one instant shifts its last keys by
//! up to (n-1)/64 ns, so a packet from a later instant that is less than that
//! apart can sort between them. With the 32-packet burst of `dpds` that is at
//! most about 0.5 ns.
//!
//! ## Features
//!
//! - `serde`: `Serialize`/`Deserialize` for [`Config`] and the types in it.
//! - `json` (default, implies `serde`): `Config::from_file` and
//!   `ConfigError`. Build with `--no-default-features` to avoid serde and
//!   serde_json entirely and construct [`Config`] in code.

use std::fmt;
#[cfg(feature = "json")]
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use crate::delayer::RVDistribution;
use crate::dropper::LossModel;
use crate::limiter::LimiterMode;

pub mod buffer;
pub mod delayer;
pub mod dropper;
pub mod limiter;

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DelayerConfig {
    pub distribution: RVDistribution, // which distribution to use for the delay
    pub mean_ns: f64,                 // in nanoseconds
    pub std_ns: f64,                  // in nanoseconds
    pub activate_correlation: bool,   // true if correlation should be applied
    pub half_life_period_ns: f64,     // configurable memory of the UTEMA
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LimiterConfig {
    pub mode: LimiterMode, // which limiter to use (Spacer or TokenBucketFilter)
    pub activate: bool,    // true if limiter should be applied
    pub rate_bps: f64,     // in bits per second
    pub limit_bit: f64,    // in bits
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Config {
    pub buffer_capacity: usize, // Max size of the buffer
    pub reordering: bool,       // true if reordering is permitted
    pub delayer: DelayerConfig,
    pub limiter: LimiterConfig,
    pub loss_model: LossModel, // which loss model to use for the dropper
    /// API change: new field, struct literals need `seed: None`.
    ///
    /// `Some(s)` seeds the delayer and dropper random number generators
    /// deterministically; `None` (also when absent from the JSON) seeds them
    /// from the OS entropy source.
    #[cfg_attr(feature = "serde", serde(default))]
    pub seed: Option<u64>,
}

/// Why a [`Buffer`](buffer::Buffer) or [`Delayer`](delayer::Delayer) could not
/// be built from its parameters (the cases in which `new` panics).
#[derive(Debug, Clone)]
pub enum BuildError {
    /// The delay distribution rejects the configured `mean_ns` / `std_ns`.
    Distribution {
        distribution: RVDistribution,
        mean_ns: f64,
        std_ns: f64,
        /// The reason given by the distribution.
        reason: String,
    },
    /// `half_life_period_ns` is negative, NaN or infinite (0 is valid), or,
    /// with `activate_correlation`, so long that the smoothing weight derived
    /// from it and the initialisation rate is not below 1.
    HalfLifePeriod { half_life_period_ns: f64 },
    /// The buffer of `buffer_capacity` packets cannot be allocated.
    BufferCapacity { capacity: usize, reason: String },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Distribution {
                distribution,
                mean_ns,
                std_ns,
                reason,
            } => write!(
                f,
                "invalid delayer parameters for {distribution:?}: mean_ns = {mean_ns}, std_ns = {std_ns} ({reason})"
            ),
            BuildError::HalfLifePeriod {
                half_life_period_ns,
            } => write!(
                f,
                "invalid half_life_period_ns = {half_life_period_ns}: must be finite and not negative, and with correlation short enough for a smoothing weight below 1"
            ),
            BuildError::BufferCapacity { capacity, reason } => write!(
                f,
                "cannot allocate buffer_capacity = {capacity} packets ({reason})"
            ),
        }
    }
}

impl std::error::Error for BuildError {}

/// Why a configuration file could not be loaded.
#[cfg(feature = "json")]
#[derive(Debug)]
pub enum ConfigError {
    /// The file could not be opened or read.
    Io(std::io::Error, PathBuf),
    /// The file is not valid JSON for a [`Config`].
    Parse(serde_json::Error, PathBuf),
}

#[cfg(feature = "json")]
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(err, path) => {
                write!(f, "failed to read config {}: {err}", path.display())
            }
            ConfigError::Parse(err, path) => {
                write!(f, "failed to parse config {}: {err}", path.display())
            }
        }
    }
}

#[cfg(feature = "json")]
impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io(err, _) => Some(err),
            ConfigError::Parse(err, _) => Some(err),
        }
    }
}

#[cfg(feature = "json")]
impl Config {
    /// Reads and parses the JSON configuration file at `path`.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|e| ConfigError::Io(e, path.to_path_buf()))?;
        serde_json::from_reader(BufReader::new(file))
            .map_err(|e| ConfigError::Parse(e, path.to_path_buf()))
    }
}

#[cfg(all(test, feature = "json"))]
mod tests {
    use super::*;

    #[test]
    fn example_config_deserializes() {
        let json = include_str!("../config.example.json");
        let config: Config = serde_json::from_str(json).expect("example config must parse");
        assert_eq!(config.buffer_capacity, 100000);
        assert!(config.delayer.activate_correlation);
        assert!(config.limiter.activate);
    }
}
