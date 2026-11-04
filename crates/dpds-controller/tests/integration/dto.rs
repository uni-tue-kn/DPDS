//! DTO <-> `dpds_core::Config` conversion (B1). Spec 22 (optional stages): `delayer`, `limiter` and
//! `loss_model` may be absent or `null`; a missing stage maps to a no-op core stage (see
//! `noop_core`), a present one requires and validates all its fields; `limiter.activate` is gone.

use crate::common::*;
use dpds_controller::dto::ConfigDto;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};
use serde_json::json;

fn dbg<T: std::fmt::Debug>(c: &T) -> String {
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
    v["limiter"] = json!({"mode":"TokenBucketFilter","rate_bps":1.0e9});
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
fn gilbert_elliot_and_random_loss_roundtrip_through_json_with_named_fields() {
    let ge = json!({"type":"GilbertElliot","p":0.01,"r":0.3,"k":1.0,"h":0.5,"state_is_good":true});
    let mut v = cfg_json(1.0e6, 1.0e5);
    v["loss_model"] = ge.clone();
    v["limiter"] = json!({"mode":"TokenBucketFilter","rate_bps":1.0e9});
    v["seed"] = json!(3);
    let dto = dto_from(v.clone());
    let back = strip_nulls(&serde_json::to_value(&dto).unwrap());
    assert_eq!(back["loss_model"], ge);
    assert_eq!(back["delayer"]["distribution"], "NormalDistribution");
    assert_eq!(
        back["limiter"], v["limiter"],
        "no `activate` comes back: {back}"
    );
    assert_eq!(back["seed"], 3);
    let again: ConfigDto = serde_json::from_value(back).unwrap();
    assert_eq!(dbg(&again.to_core().unwrap()), dbg(&dto.to_core().unwrap()));
    // random loss
    let mut v = cfg_json(1.0e6, 1.0e5);
    v["loss_model"] = json!({"type":"Random","p":0.1});
    let back = strip_nulls(&serde_json::to_value(dto_from(v)).unwrap());
    assert_eq!(back["loss_model"], json!({"type":"Random","p":0.1}));
}

#[test]
fn unknown_distribution_or_missing_buffer_is_a_deserialisation_error() {
    let mut v = cfg_json(1.0e6, 0.0);
    v["delayer"]["distribution"] = json!("NoSuchDistribution");
    assert!(serde_json::from_value::<ConfigDto>(v).is_err());
    let mut v = cfg_json(1.0e6, 0.0);
    v.as_object_mut().unwrap().remove("buffer_capacity");
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
    let back = serde_json::to_value(&dto).unwrap();
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

// ---- optional stages (spec 22) -------------------------------------------------------------

/// The core config a config with no stage maps to: Deterministic 0 ns, no correlation, half-life
/// 0 and the internal rate 1; inactive Spacer, rate 0; `Random(0)` loss.
fn noop_core(buffer_capacity: usize) -> Config {
    Config {
        buffer_capacity,
        reordering: false,
        delayer: DelayerConfig {
            distribution: RVDistribution::DeterministicDistribution,
            mean_ns: 0.0,
            std_ns: 0.0,
            activate_correlation: false,
            half_life_period_ns: 0.0,
            initialization_rate_pps: 1.0,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 0.0,
        },
        loss_model: LossModel::Random(0.0),
        seed: None,
    }
}

#[test]
fn a_config_with_only_the_buffer_maps_to_the_noop_stages() {
    let core = dto_from(cfg_bare()).to_core().unwrap();
    assert_eq!(dbg(&core), dbg(&noop_core(100000)));
}

#[test]
fn null_and_absent_stages_are_the_same() {
    for stage in ["delayer", "limiter", "loss_model"] {
        let mut a = cfg_json(3.0e6, 1.0e5);
        a["limiter"] = json!({"mode": "Spacer", "rate_bps": 1.0e9});
        let mut absent_v = a.clone();
        absent_v.as_object_mut().unwrap().remove(stage);
        let mut null_v = a.clone();
        null_v[stage] = json!(null);
        let x = dto_from(absent_v).to_core().unwrap();
        let y = dto_from(null_v).to_core().unwrap();
        assert_eq!(dbg(&x), dbg(&y), "{stage}");
    }
}

#[test]
fn a_missing_delayer_is_deterministic_zero_whatever_the_other_stages_say() {
    let mut v = cfg_json(1.0e6, 1.0e5);
    v.as_object_mut().unwrap().remove("delayer");
    v["limiter"] = json!({"mode": "Spacer", "rate_bps": 2.0e9});
    v["loss_model"] = json!({"type": "Random", "p": 0.5});
    let core = dto_from(v).to_core().unwrap();
    assert_eq!(dbg(&core.delayer), dbg(&noop_core(0).delayer));
    assert!(core.limiter.activate);
    assert_eq!(core.limiter.rate_bps, 2.0e9);
    assert!(matches!(core.loss_model, LossModel::Random(p) if p == 0.5));
}

#[test]
fn a_missing_limiter_is_an_inactive_spacer_with_rate_zero() {
    let mut v = cfg_json(1.0e6, 1.0e5);
    v.as_object_mut().unwrap().remove("limiter");
    let core = dto_from(v).to_core().unwrap();
    assert_eq!(dbg(&core.limiter), dbg(&noop_core(0).limiter));
    assert!(!core.limiter.activate);
    assert_eq!(core.limiter.rate_bps, 0.0);
    assert_eq!(core.delayer.mean_ns, 1.0e6, "the delayer is untouched");
}

#[test]
fn a_missing_loss_model_is_random_zero() {
    let mut v = cfg_json(1.0e6, 1.0e5);
    v.as_object_mut().unwrap().remove("loss_model");
    let core = dto_from(v).to_core().unwrap();
    assert!(matches!(core.loss_model, LossModel::Random(p) if p == 0.0));
}

#[test]
fn a_present_limiter_is_active() {
    for mode in ["Spacer", "TokenBucketFilter"] {
        let mut v = cfg_json(1.0e6, 1.0e5);
        v["limiter"] = json!({"mode": mode, "rate_bps": 1.0e9});
        let core = dto_from(v).to_core().unwrap();
        assert!(core.limiter.activate, "{mode}");
        assert_eq!(core.limiter.rate_bps, 1.0e9);
    }
}

#[test]
fn the_limiter_activate_flag_no_longer_exists() {
    for activate in [json!(true), json!(false)] {
        let mut v = cfg_json(1.0e6, 1.0e5);
        v["limiter"] = json!({"mode": "Spacer", "activate": activate, "rate_bps": 1.0e9});
        let err = serde_json::from_value::<ConfigDto>(v)
            .expect_err("`activate` must be an unknown field")
            .to_string();
        assert!(err.contains("activate"), "{err}");
    }
    // and it is not written back
    let mut v = cfg_json(1.0e6, 1.0e5);
    v["limiter"] = json!({"mode": "Spacer", "rate_bps": 1.0e9});
    let out = serde_json::to_value(dto_from(v)).unwrap();
    assert!(!out.to_string().contains("activate_limiter"), "{out}");
    assert!(out["limiter"].get("activate").is_none(), "{out}");
}

#[test]
fn a_present_limiter_needs_a_positive_finite_rate() {
    for bad in [json!(0), json!(0.0), json!(-1.0), json!(-1.0e9)] {
        let mut v = cfg_json(1.0e6, 1.0e5);
        v["limiter"] = json!({"mode": "Spacer", "rate_bps": bad});
        let err = dto_from(v)
            .to_core()
            .expect_err("rate must be positive")
            .to_string();
        assert!(err.contains("rate_bps"), "{bad}: {err}");
    }
    // not a number at all
    let mut v = cfg_json(1.0e6, 1.0e5);
    v["limiter"] = json!({"mode": "Spacer", "rate_bps": "fast"});
    assert!(serde_json::from_value::<ConfigDto>(v).is_err());
}

#[test]
fn a_present_stage_requires_all_its_fields() {
    let cases = [
        (
            "limiter without mode",
            "limiter",
            json!({"rate_bps": 1.0e9}),
            "mode",
        ),
        (
            "limiter without rate",
            "limiter",
            json!({"mode": "Spacer"}),
            "rate_bps",
        ),
        ("empty limiter", "limiter", json!({}), "mode"),
        ("empty delayer", "delayer", json!({}), "distribution"),
        (
            "delayer without mean",
            "delayer",
            json!({"distribution": "NormalDistribution", "std_ns": 1.0,
                   "activate_correlation": false, "half_life_period_ns": 0.0,
                   "initialization_rate_pps": 1.0}),
            "mean_ns",
        ),
        (
            "delayer without std",
            "delayer",
            json!({"distribution": "NormalDistribution", "mean_ns": 1.0,
                   "activate_correlation": false, "half_life_period_ns": 0.0,
                   "initialization_rate_pps": 1.0}),
            "std_ns",
        ),
        (
            "delayer without correlation flag",
            "delayer",
            json!({"distribution": "NormalDistribution", "mean_ns": 1.0, "std_ns": 1.0,
                   "half_life_period_ns": 0.0, "initialization_rate_pps": 1.0}),
            "activate_correlation",
        ),
        (
            "delayer without half-life",
            "delayer",
            json!({"distribution": "NormalDistribution", "mean_ns": 1.0, "std_ns": 1.0,
                   "activate_correlation": false, "initialization_rate_pps": 1.0}),
            "half_life_period_ns",
        ),
        (
            "delayer without the rate",
            "delayer",
            json!({"distribution": "NormalDistribution", "mean_ns": 1.0, "std_ns": 1.0,
                   "activate_correlation": false, "half_life_period_ns": 0.0}),
            "initialization_rate_pps",
        ),
        (
            "random loss without p",
            "loss_model",
            json!({"type": "Random"}),
            "p",
        ),
        (
            "loss model without type",
            "loss_model",
            json!({"p": 0.1}),
            "type",
        ),
        (
            "gilbert-elliot without h",
            "loss_model",
            json!({"type": "GilbertElliot", "p": 0.1, "r": 0.2, "k": 0.3}),
            "h",
        ),
    ];
    for (what, stage, value, named) in cases {
        let mut v = cfg_json(1.0e6, 1.0e5);
        v[stage] = value;
        let err = serde_json::from_value::<ConfigDto>(v)
            .err()
            .unwrap_or_else(|| panic!("{what} must not deserialize"))
            .to_string();
        assert!(err.contains(named), "{what}: {err}");
    }
}

#[test]
fn a_present_delayer_is_validated_as_before_even_when_other_stages_are_missing() {
    let mut v = cfg_bare();
    v["delayer"] = json!({"distribution": "NormalDistribution", "mean_ns": 1.0e6,
        "std_ns": -5.0, "activate_correlation": false, "half_life_period_ns": 0.0,
        "initialization_rate_pps": 1.0});
    let err = dto_from(v).to_core().unwrap_err().to_string();
    assert!(err.contains("std_ns"), "{err}");
    for bad in [json!(0.0), json!(0.5), json!(-1.0)] {
        let mut v = cfg_bare();
        v["delayer"] = json!({"distribution": "NormalDistribution", "mean_ns": 1.0e6,
            "std_ns": 1.0, "activate_correlation": false, "half_life_period_ns": 0.0,
            "initialization_rate_pps": bad});
        let err = dto_from(v).to_core().unwrap_err().to_string();
        assert!(err.contains("initialization_rate_pps"), "{bad}: {err}");
    }
}

#[test]
fn a_present_loss_model_is_validated_even_when_other_stages_are_missing() {
    let mut v = cfg_bare();
    v["loss_model"] = json!({"type": "Random", "p": 1.5});
    assert!(dto_from(v).to_core().is_err());
}

#[test]
fn the_internal_noop_rate_is_never_exposed() {
    // a config without a delayer serialises without one (or with null), never with a rate of 1
    let out = strip_nulls(&serde_json::to_value(dto_from(cfg_bare())).unwrap());
    assert!(out.get("delayer").is_none(), "{out}");
    assert!(out.get("limiter").is_none(), "{out}");
    assert!(out.get("loss_model").is_none(), "{out}");
    assert!(
        !out.to_string().contains("initialization_rate_pps"),
        "{out}"
    );
    assert_eq!(out["buffer_capacity"], 100000);
}

#[test]
fn a_config_with_every_stage_still_roundtrips_exactly() {
    let v = cfg_full(2.0e6, 2.0e5, 4.0e9);
    let out = strip_nulls(&serde_json::to_value(dto_from(v.clone())).unwrap());
    for stage in ["delayer", "limiter", "loss_model"] {
        assert_eq!(out[stage], v[stage], "{stage}");
    }
}
