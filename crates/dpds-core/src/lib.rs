use serde::{Deserialize, Serialize};
use std::{
    fmt,
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct DelayerConfig {
    pub distribution: RVDistribution, // which distribution to use for the delay
    pub mean_ns: f64,                 // in nanoseconds
    pub std_ns: f64,                  // in nanoseconds
    pub activate_correlation: bool,   // true if correlation should be applied
    pub half_life_period_ns: f64,     // configurable memory of the UTEMA
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct LimiterConfig {
    pub mode: LimiterMode, // which limiter to use (Spacer or TokenBucketFilter)
    pub activate: bool,    // true if limiter should be applied
    pub rate_bps: f64,     // in bits per second
    pub limit_bit: f64,    // in bits
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Config {
    pub buffer_capacity: usize, // Max size of the buffer
    pub reordering: bool,       // true if reordering is permitted
    pub delayer: DelayerConfig,
    pub limiter: LimiterConfig,
    pub loss_model: LossModel, // which loss model to use for the dropper
}

/// Why a configuration file could not be loaded.
#[derive(Debug)]
pub enum ConfigError {
    /// The file could not be opened or read.
    Io(std::io::Error, PathBuf),
    /// The file is not valid JSON for a [`Config`].
    Parse(serde_json::Error, PathBuf),
}

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

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io(err, _) => Some(err),
            ConfigError::Parse(err, _) => Some(err),
        }
    }
}

impl Config {
    /// Reads and parses the JSON configuration file at `path`.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|e| ConfigError::Io(e, path.to_path_buf()))?;
        serde_json::from_reader(BufReader::new(file))
            .map_err(|e| ConfigError::Parse(e, path.to_path_buf()))
    }
}

#[cfg(test)]
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
