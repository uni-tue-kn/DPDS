//! Tests for `Config::from_file` (the Rust replacement of the C-string
//! `read_config`).
//!
//! API assumed:
//! - `dpds_core::Config::from_file(path: impl AsRef<Path>) -> Result<Config, dpds_core::ConfigError>`
//! - `dpds_core::ConfigError: std::error::Error` (Display must be non-empty)
//! - `Config`/`DelayerConfig`/`LimiterConfig` keep their `pub` fields and serde format.

#![cfg(feature = "json")]

use std::path::{Path, PathBuf};

use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, ConfigError};

fn write(dir: &tempfile::TempDir, name: &str, contents: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

const VALID: &str = r#"{
    "buffer_capacity": 1234,
    "reordering": true,
    "delayer": {
        "distribution": "GammaDistribution",
        "mean_ns": 2000.0,
        "std_ns": 100.0,
        "activate_correlation": false,
        "half_life_period_ns": 5.0,
        "initialization_rate_pps": 2500000.0
    },
    "limiter": {
        "mode": "TokenBucketFilter",
        "activate": true,
        "rate_bps": 1e9,
        "limit_bit": 4096.0
    },
    "loss_model": { "Random": 0.25 }
}"#;

#[test]
fn from_file_parses_a_valid_file_field_by_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "ok.json", VALID);

    let cfg = Config::from_file(&path).expect("valid config must parse");

    assert_eq!(cfg.buffer_capacity, 1234);
    assert!(cfg.reordering);
    assert!(matches!(
        cfg.delayer.distribution,
        RVDistribution::GammaDistribution
    ));
    assert_eq!(cfg.delayer.mean_ns, 2000.0);
    assert_eq!(cfg.delayer.std_ns, 100.0);
    assert!(!cfg.delayer.activate_correlation);
    assert_eq!(cfg.delayer.half_life_period_ns, 5.0);
    assert_eq!(cfg.delayer.initialization_rate_pps, 2_500_000.0);
    assert!(matches!(cfg.limiter.mode, LimiterMode::TokenBucketFilter));
    assert!(cfg.limiter.activate);
    assert_eq!(cfg.limiter.rate_bps, 1e9);
    assert_eq!(cfg.limiter.limit_bit, 4096.0);
    assert!(matches!(cfg.loss_model, LossModel::Random(p) if p == 0.25));
}

#[test]
fn from_file_accepts_path_pathbuf_str_and_string() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "ok.json", VALID);

    assert!(Config::from_file(&path).is_ok()); // &PathBuf
    assert!(Config::from_file(path.clone()).is_ok()); // PathBuf
    assert!(Config::from_file(path.as_path()).is_ok()); // &Path
    assert!(Config::from_file(path.to_str().unwrap()).is_ok()); // &str
    let owned: String = path.to_str().unwrap().into();
    assert!(Config::from_file(owned).is_ok()); // String
}

#[test]
fn from_file_parses_the_gilbert_elliot_loss_model() {
    let dir = tempfile::tempdir().unwrap();
    let json = VALID.replace(
        r#"{ "Random": 0.25 }"#,
        r#"{ "GilbertElliot": [0.01, 0.3, 1.0, 0.5, true] }"#,
    );
    let path = write(&dir, "ge.json", &json);

    let cfg = Config::from_file(path).unwrap();
    assert!(matches!(
        cfg.loss_model,
        LossModel::GilbertElliot(p, r, k, h, true) if p == 0.01 && r == 0.3 && k == 1.0 && h == 0.5
    ));
}

#[test]
fn from_file_parses_the_example_config_shipped_with_the_crate() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let cfg = Config::from_file(path).expect("config.example.json must parse");
    assert_eq!(cfg.buffer_capacity, 100000);
    assert!(cfg.delayer.activate_correlation);
    assert!(cfg.limiter.activate);
    let rate = cfg.delayer.initialization_rate_pps;
    assert!(rate.is_finite() && rate >= 1.0, "example rate {rate}");
}

#[test]
fn from_file_parses_the_workspace_root_config() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.json");
    let cfg = Config::from_file(path).expect("config.json at the repo root must parse");
    assert_eq!(cfg.buffer_capacity, 1_000_000);
    assert!(!cfg.reordering);
    assert!(!cfg.limiter.activate);
    assert!(matches!(cfg.loss_model, LossModel::Random(p) if p == 0.0));
    let rate = cfg.delayer.initialization_rate_pps;
    assert!(rate.is_finite() && rate >= 1.0, "config.json rate {rate}");
}

#[test]
fn from_file_parses_the_vdev_config() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.vdev.json");
    let cfg = Config::from_file(path).expect("config.vdev.json at the repo root must parse");
    let rate = cfg.delayer.initialization_rate_pps;
    assert!(
        rate.is_finite() && rate >= 1.0,
        "config.vdev.json rate {rate}"
    );
}

// `initialization_rate_pps` is required: no serde default, whether or not the
// correlation is active, and the error names the field.

#[test]
fn from_file_without_initialization_rate_pps_is_an_error_naming_the_field() {
    let dir = tempfile::tempdir().unwrap();
    let json = VALID.replace(",\n        \"initialization_rate_pps\": 2500000.0", "");
    assert!(!json.contains("initialization_rate_pps"), "test setup");
    let path = write(&dir, "norate.json", &json);
    let err = Config::from_file(path).expect_err("a config without the rate must not parse");
    assert!(matches!(err, ConfigError::Parse(..)), "{err:?}");
    assert!(
        err.to_string().contains("initialization_rate_pps"),
        "message {:?} must name the field",
        err.to_string()
    );
}

#[test]
fn from_file_requires_the_rate_also_with_correlation_off_and_on() {
    let dir = tempfile::tempdir().unwrap();
    for corr in ["false", "true"] {
        let json = VALID
            .replace(",\n        \"initialization_rate_pps\": 2500000.0", "")
            .replace(
                "\"activate_correlation\": false",
                &format!("\"activate_correlation\": {corr}"),
            );
        let path = write(&dir, "norate.json", &json);
        assert!(Config::from_file(path).is_err(), "correlation {corr}");
    }
}

#[test]
fn from_file_initialization_rate_pps_must_be_a_number() {
    let dir = tempfile::tempdir().unwrap();
    for bad in ["\"fast\"", "null", "true", "[1]"] {
        let json = VALID.replace("2500000.0", bad);
        let path = write(&dir, "badrate.json", &json);
        assert!(Config::from_file(path).is_err(), "rate {bad}");
    }
}

#[test]
fn from_file_accepts_an_integer_json_number_for_the_rate() {
    let dir = tempfile::tempdir().unwrap();
    let json = VALID.replace("2500000.0", "3000000");
    let cfg = Config::from_file(write(&dir, "int.json", &json)).unwrap();
    assert_eq!(cfg.delayer.initialization_rate_pps, 3_000_000.0);
}

#[test]
fn from_file_missing_file_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let err = Config::from_file(dir.path().join("does-not-exist.json")).unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[test]
fn from_file_on_a_directory_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Config::from_file(dir.path()).is_err());
}

#[test]
fn from_file_bad_json_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "bad.json", "{ this is not json");
    let err = Config::from_file(path).unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[test]
fn from_file_empty_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "empty.json", "");
    assert!(Config::from_file(path).is_err());
}

#[test]
fn from_file_valid_json_with_missing_field_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let json = VALID.replace(r#""buffer_capacity": 1234,"#, "");
    let path = write(&dir, "missing.json", &json);
    assert!(Config::from_file(path).is_err());
}

#[test]
fn from_file_valid_json_with_wrong_type_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let json = VALID.replace(r#""buffer_capacity": 1234"#, r#""buffer_capacity": "many""#);
    let path = write(&dir, "wrongtype.json", &json);
    assert!(Config::from_file(path).is_err());
}

#[test]
fn from_file_unknown_enum_variant_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let json = VALID.replace("GammaDistribution", "NoSuchDistribution");
    let path = write(&dir, "variant.json", &json);
    assert!(Config::from_file(path).is_err());
}

#[test]
fn config_error_is_a_std_error() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<ConfigError>();
}

#[test]
fn io_and_parse_errors_are_distinguishable_by_message() {
    let dir = tempfile::tempdir().unwrap();
    let missing = Config::from_file(dir.path().join("nope.json")).unwrap_err();
    let bad = Config::from_file(write(&dir, "bad.json", "{")).unwrap_err();
    assert_ne!(missing.to_string(), bad.to_string());
}
