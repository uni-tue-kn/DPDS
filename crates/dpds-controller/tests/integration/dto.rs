//! DTO <-> `dpds_core::Config` conversion (B1).

use crate::common::*;
use dpds_controller::dto::ConfigDto;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};
use serde_json::json;

fn dbg(c: &Config) -> String {
    format!("{c:?}")
}

fn core_config(loss: LossModel, seed: Option<u64>) -> Config {
    Config {
        buffer_capacity: 4096,
        reordering: true,
        delayer: DelayerConfig {
            distribution: RVDistribution::LogNormalDistribution,
            mean_ns: 5.0e6,
            std_ns: 5.0e5,
            activate_correlation: true,
            half_life_period_ns: 5.0e7,
            initialization_rate_pps: 15e6,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::TokenBucketFilter,
            activate: true,
            rate_bps: 1.0e9,
        },
        loss_model: loss,
        seed,
    }
}

fn with_loss(loss: serde_json::Value) -> serde_json::Value {
    let mut v = cfg_json(1.0e6, 1.0e5);
    v["loss_model"] = loss;
    v
}

#[test]
fn json_with_random_loss_converts_to_the_core_config() {
    let mut v = cfg_json(5.0e6, 5.0e5);
    v["buffer_capacity"] = json!(4096);
    v["reordering"] = json!(true);
    v["delayer"]["distribution"] = json!("LogNormalDistribution");
    v["delayer"]["activate_correlation"] = json!(true);
    v["delayer"]["half_life_period_ns"] = json!(5.0e7);
    v["limiter"] = json!({"mode":"TokenBucketFilter","activate":true,"rate_bps":1.0e9});
    v["loss_model"] = json!({"type":"Random","p":0.25});
    v["seed"] = json!(7);
    let core = dto_from(v).to_core().unwrap();
    assert_eq!(
        dbg(&core),
        dbg(&core_config(LossModel::Random(0.25), Some(7)))
    );
}

#[test]
fn gilbert_elliot_uses_named_fields() {
    let v = with_loss(
        json!({"type":"GilbertElliot","p":0.01,"r":0.3,"k":1.0,"h":0.5,"state_is_good":false}),
    );
    let core = dto_from(v).to_core().unwrap();
    match core.loss_model {
        LossModel::GilbertElliot(p, r, k, h, good) => {
            assert_eq!((p, r, k, h, good), (0.01, 0.3, 1.0, 0.5, false));
        }
        other => panic!("wrong loss model {other:?}"),
    }
}

#[test]
fn gilbert_elliot_state_is_good_defaults_to_true() {
    let v = with_loss(json!({"type":"GilbertElliot","p":0.01,"r":0.3,"k":1.0,"h":0.5}));
    let core = dto_from(v).to_core().unwrap();
    assert!(matches!(
        core.loss_model,
        LossModel::GilbertElliot(_, _, _, _, true)
    ));
}

#[test]
fn seed_is_optional() {
    let core = dto_from(cfg_json(1.0e6, 0.0)).to_core().unwrap();
    assert_eq!(core.seed, None);
}

#[test]
fn from_core_serialises_named_gilbert_elliot_fields_and_roundtrips() {
    let c = core_config(LossModel::GilbertElliot(0.01, 0.3, 1.0, 0.5, true), Some(3));
    let dto = ConfigDto::from_core(&c);
    let v = serde_json::to_value(&dto).unwrap();
    assert_eq!(
        strip_nulls(&v["loss_model"]),
        json!({"type":"GilbertElliot","p":0.01,"r":0.3,"k":1.0,"h":0.5,"state_is_good":true})
    );
    assert_eq!(v["delayer"]["distribution"], "LogNormalDistribution");
    assert_eq!(v["limiter"]["mode"], "TokenBucketFilter");
    assert_eq!(dbg(&dto.to_core().unwrap()), dbg(&c));
    // and through JSON text
    let back: ConfigDto = serde_json::from_value(v).unwrap();
    assert_eq!(dbg(&back.to_core().unwrap()), dbg(&c));
}

#[test]
fn random_loss_roundtrips() {
    let c = core_config(LossModel::Random(0.1), None);
    let v = serde_json::to_value(ConfigDto::from_core(&c)).unwrap();
    assert_eq!(
        strip_nulls(&v["loss_model"]),
        json!({"type":"Random","p":0.1})
    );
}

#[test]
fn unknown_distribution_or_missing_section_is_a_deserialisation_error() {
    let mut v = cfg_json(1.0e6, 0.0);
    v["delayer"]["distribution"] = json!("NoSuchDistribution");
    assert!(serde_json::from_value::<ConfigDto>(v).is_err());
    let mut v = cfg_json(1.0e6, 0.0);
    v.as_object_mut().unwrap().remove("limiter");
    assert!(serde_json::from_value::<ConfigDto>(v).is_err());
    let v = with_loss(json!({"type":"Bernoulli","p":0.1}));
    assert!(serde_json::from_value::<ConfigDto>(v).is_err());
}

#[test]
fn distribution_rejected_by_dpds_core_is_a_validation_error_naming_the_field() {
    let err = dto_from(cfg_json(1.0e6, -5.0))
        .to_core()
        .unwrap_err()
        .to_string();
    assert!(err.contains("std_ns"), "{err}");
}

#[test]
fn negative_half_life_is_a_validation_error() {
    let mut v = cfg_json(1.0e6, 1.0e5);
    v["delayer"]["half_life_period_ns"] = json!(-1.0);
    let err = dto_from(v).to_core().unwrap_err().to_string();
    assert!(err.contains("half_life"), "{err}");
}

// ---- initialization_rate_pps (spec v12 B) -----------------------------------------------

#[test]
fn the_initialization_rate_is_required_and_has_no_default() {
    let mut v = cfg_json(1.0e6, 1.0e5);
    v["delayer"]
        .as_object_mut()
        .unwrap()
        .remove("initialization_rate_pps");
    let err = serde_json::from_value::<ConfigDto>(v)
        .expect_err("a config without the rate must not deserialize")
        .to_string();
    assert!(err.contains("initialization_rate_pps"), "{err}");
}

#[test]
fn the_initialization_rate_reaches_the_core_config_and_comes_back() {
    let mut v = cfg_json(5.0e6, 5.0e5);
    v["delayer"]["initialization_rate_pps"] = json!(2.5e6);
    let dto = dto_from(v);
    let core = dto.to_core().unwrap();
    assert_eq!(core.delayer.initialization_rate_pps, 2.5e6);
    let back = serde_json::to_value(ConfigDto::from_core(&core)).unwrap();
    assert_eq!(back["delayer"]["initialization_rate_pps"], json!(2.5e6));
    // an integer JSON number is fine too
    let mut v = cfg_json(5.0e6, 5.0e5);
    v["delayer"]["initialization_rate_pps"] = json!(40_000);
    assert_eq!(
        dto_from(v)
            .to_core()
            .unwrap()
            .delayer
            .initialization_rate_pps,
        40_000.0
    );
}

#[test]
fn an_invalid_initialization_rate_is_a_validation_error_naming_the_field() {
    for bad in [
        json!(0.0),
        json!(0.5),
        json!(0.999),
        json!(-1.0),
        json!(-15.0e6),
    ] {
        for correlation in [false, true] {
            let mut v = cfg_json(1.0e6, 1.0e5);
            v["delayer"]["initialization_rate_pps"] = bad.clone();
            v["delayer"]["activate_correlation"] = json!(correlation);
            let err = dto_from(v)
                .to_core()
                .expect_err("an invalid rate must be rejected")
                .to_string();
            assert!(
                err.contains("initialization_rate_pps"),
                "rate {bad} (correlation {correlation}): {err}"
            );
        }
    }
}

#[test]
fn the_smallest_valid_initialization_rate_is_one_packet_per_second() {
    let mut v = cfg_json(1.0e6, 1.0e5);
    v["delayer"]["initialization_rate_pps"] = json!(1.0);
    assert!(dto_from(v).to_core().is_ok());
}

#[test]
fn loss_probabilities_outside_0_1_are_rejected() {
    for loss in [
        json!({"type":"Random","p":1.5}),
        json!({"type":"Random","p":-0.1}),
        json!({"type":"GilbertElliot","p":2.0,"r":0.3,"k":1.0,"h":0.5}),
        json!({"type":"GilbertElliot","p":0.1,"r":0.3,"k":-0.1,"h":0.5}),
        json!({"type":"GilbertElliot","p":0.1,"r":0.3,"k":1.0,"h":1.1}),
    ] {
        let err = dto_from(with_loss(loss.clone())).to_core();
        assert!(err.is_err(), "{loss} must be rejected");
        assert!(!err.unwrap_err().to_string().is_empty());
    }
}

#[test]
fn validation_does_not_allocate_the_buffer() {
    // 2^31 packets (the cap) would need ~4 TB of mbufs: only a zero-capacity validation copy can pass.
    let mut v = cfg_json(1.0e6, 1.0e5);
    v["buffer_capacity"] = json!(2_147_483_648u64);
    let core = dto_from(v)
        .to_core()
        .expect("must validate without allocating");
    assert_eq!(core.buffer_capacity, 2_147_483_648);
}
