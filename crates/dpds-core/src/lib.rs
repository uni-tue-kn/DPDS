#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use serde::{Deserialize, Serialize};
use std::{
    ffi::{c_char, CStr},
    fs::File,
    io::BufReader,
};

use crate::delayer::RVDistribution;
use crate::dropper::LossModel;
use crate::limiter::LimiterMode;

pub mod buffer;
pub mod delayer;
pub mod dropper;
pub mod limiter;

#[repr(C)]
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct DelayerConfig {
    pub distribution: RVDistribution, // which distribution to use for the delay
    pub mean_ns: f64,                // in nanoseconds
    pub std_ns: f64,                 // in nanoseconds
    pub activate_correlation: bool,  // true if correlation should be applied
    pub half_life_period_ns: f64,    // configurable memory of the UTEMA
}

#[repr(C)]
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct LimiterConfig {
    pub mode: LimiterMode, // which limiter to use (Spacer or TokenBucketFilter)
    pub activate: bool,    // true if limiter should be applied
    pub rate_bps: f64,     // in bits per second
    pub limit_bit: f64,    // in bits
}

#[repr(C)]
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Config {
    pub buffer_capacity: usize, // Max size of the buffer
    pub reordering: bool,       // true if reordering is permitted
    pub delayer: DelayerConfig,
    pub limiter: LimiterConfig,
    pub loss_model: LossModel, // which loss model to use for the dropper
}

/// Reads and parses the JSON configuration file at `path`.
///
/// # Safety
/// `path` must be a valid, NUL-terminated C string pointer.
#[no_mangle]
pub unsafe extern "C" fn read_config(path: *const c_char) -> Config {
    let path = CStr::from_ptr(path)
        .to_str()
        .unwrap_or_else(|err| panic!("Invalid UTF-8 sequence: {}", err));
    let file = File::open(path).unwrap_or_else(|err| panic!("Failed to open {path}: {err}"));

    serde_json::from_reader(BufReader::new(file))
        .unwrap_or_else(|err| panic!("Failed to convert JSON data to a config: {err}"))
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
