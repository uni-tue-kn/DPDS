//! Optional features `serde` (derives) and `json` (`Config::from_file`,
//! `ConfigError`, serde_json). `default = ["json"]`.
//!
//! Everything here is feature-gated, so `cargo test -p dpds-core
//! --no-default-features` compiles (the other test files use no serde/json):
//! - `--no-default-features`: this file is empty;
//! - `--no-default-features --features serde`: the trait-bound test only;
//! - default (`json`): also the JSON round trips and the `seed` field format.
//!
//! The Config literal in the other test files needs no serde at all.

#[cfg(feature = "serde")]
mod serde_derives {
    use dpds_core::delayer::RVDistribution;
    use dpds_core::dropper::LossModel;
    use dpds_core::limiter::LimiterMode;
    use dpds_core::{Config, DelayerConfig, LimiterConfig};

    fn assert_serde<T: serde::Serialize + for<'de> serde::Deserialize<'de>>() {}

    #[test]
    fn all_config_types_implement_serialize_and_deserialize() {
        assert_serde::<Config>();
        assert_serde::<DelayerConfig>();
        assert_serde::<LimiterConfig>();
        assert_serde::<LossModel>();
        assert_serde::<RVDistribution>();
        assert_serde::<LimiterMode>();
    }
}

#[cfg(feature = "json")]
mod json {
    use dpds_core::delayer::RVDistribution;
    use dpds_core::dropper::LossModel;
    use dpds_core::limiter::LimiterMode;
    use dpds_core::{Config, DelayerConfig, LimiterConfig};

    fn config(seed: Option<u64>) -> Config {
        Config {
            buffer_capacity: 321,
            reordering: true,
            delayer: DelayerConfig {
                distribution: RVDistribution::LogNormalDistribution,
                mean_ns: 1500.5,
                std_ns: 20.25,
                activate_correlation: true,
                half_life_period_ns: 4000.0,
                initialization_rate_pps: 2.5e6,
            },
            limiter: LimiterConfig {
                mode: LimiterMode::TokenBucketFilter,
                activate: true,
                rate_bps: 2.5e9,
                limit_bit: 8192.0,
            },
            loss_model: LossModel::GilbertElliot(0.01, 0.3, 1.0, 0.5, true),
            seed,
        }
    }

    fn roundtrip(cfg: &Config) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.json");
        std::fs::write(&path, serde_json::to_string_pretty(cfg).unwrap()).unwrap();
        Config::from_file(&path).unwrap()
    }

    #[test]
    fn config_survives_a_serialize_parse_round_trip_with_and_without_seed() {
        for seed in [None, Some(0), Some(42), Some(u64::MAX)] {
            let back = roundtrip(&config(seed));
            assert_eq!(back.seed, seed);
            assert_eq!(back.buffer_capacity, 321);
            assert!(back.reordering);
            assert_eq!(back.delayer.mean_ns, 1500.5);
            assert_eq!(back.delayer.std_ns, 20.25);
            assert_eq!(back.delayer.initialization_rate_pps, 2.5e6);
            assert!(matches!(
                back.delayer.distribution,
                RVDistribution::LogNormalDistribution
            ));
            assert!(matches!(back.limiter.mode, LimiterMode::TokenBucketFilter));
            assert_eq!(back.limiter.limit_bit, 8192.0);
            assert!(matches!(
                back.loss_model,
                LossModel::GilbertElliot(p, r, k, h, true) if p == 0.01 && r == 0.3 && k == 1.0 && h == 0.5
            ));
        }
    }

    const NO_SEED: &str = r#"{
        "buffer_capacity": 10, "reordering": false,
        "delayer": {"distribution": "DeterministicDistribution", "mean_ns": 1.0,
                    "std_ns": 0.0, "activate_correlation": false, "half_life_period_ns": 1.0,
                    "initialization_rate_pps": 15000000.0},
        "limiter": {"mode": "Spacer", "activate": false, "rate_bps": 1e9, "limit_bit": 1e9},
        "loss_model": {"Random": 0.0}
    }"#;

    fn parse(json: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.json");
        std::fs::write(&path, json).unwrap();
        Config::from_file(&path).unwrap()
    }

    #[test]
    fn seed_is_optional_in_json_and_defaults_to_none() {
        assert_eq!(parse(NO_SEED).seed, None);
    }

    #[test]
    fn seed_is_read_from_json() {
        let json = NO_SEED.replacen('{', r#"{"seed": 12345,"#, 1);
        assert_eq!(parse(&json).seed, Some(12345));
        let json = NO_SEED.replacen('{', r#"{"seed": null,"#, 1);
        assert_eq!(parse(&json).seed, None);
    }

    #[test]
    fn a_negative_or_non_integer_seed_is_a_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["-1", "1.5", "\"abc\""] {
            let json = NO_SEED.replacen('{', &format!("{{\"seed\": {bad},"), 1);
            let path = dir.path().join("bad.json");
            std::fs::write(&path, json).unwrap();
            assert!(Config::from_file(&path).is_err(), "seed {bad}");
        }
    }
}
