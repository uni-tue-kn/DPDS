//! Spec 22, optional stages: in the `config` of a link of the start body, `delayer`, `limiter`
//! and `loss_model` are optional (absent or `null` = the stage is not added). The limiter has no
//! `activate` flag any more: a present limiter is active. `buffer_capacity`, `reordering`,
//! `seed`, `histogram` and `delay_stats` stay as they were.
//!
//! A missing stage becomes a no-op `dpds_core` stage in the links file `dpds` receives:
//! - delayer: `DeterministicDistribution`, mean 0, std 0, no correlation, half-life 0 and an
//!   internal `initialization_rate_pps` of 1 (never exposed, never used without correlation);
//! - limiter: `Spacer`, `activate: false`, `rate_bps` 0;
//! - loss model: `Random(0)`.
//!
//! A present stage requires and validates all of its fields (a present delayer needs
//! `initialization_rate_pps`: the rule "the rate is always configured, never preset" holds
//! whenever the delayer is added). The DTO-level mapping is in `dto.rs`, the links-file
//! function in `eal.rs`, the drain estimate and hints in `drain_hint.rs`; this module goes through
//! the HTTP API.

use crate::common::*;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};
use serde_json::{json, Value};

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

fn delayer(mean_ns: f64) -> Value {
    json!({"distribution": "DeterministicDistribution", "mean_ns": mean_ns, "std_ns": 0.0,
           "activate_correlation": false, "half_life_period_ns": 0.0,
           "initialization_rate_pps": 1.0e6})
}

fn noop() -> Config {
    Config {
        buffer_capacity: 100000,
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

/// Starts one link with `config` and returns the `config` of its entry in the links file.
async fn file_config(config: Value) -> Value {
    let a = app();
    let r = a
        .start_links(vec![link_cfg("l", "net_null0", "net_null0", config)])
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
    a.supervisor.plans()[0].links_json["links"][0]["config"].clone()
}

// ---- the mapping, through start ----------------------------------------------------------------

#[tokio::test]
async fn a_config_with_only_the_buffer_starts_and_reaches_dpds_as_the_noop_stages() {
    let got = file_config(cfg_bare()).await;
    assert_eq!(got, serde_json::to_value(noop()).unwrap());
}

#[tokio::test]
async fn null_stages_are_the_same_as_absent_ones() {
    let mut c = cfg_bare();
    c["delayer"] = json!(null);
    c["limiter"] = json!(null);
    c["loss_model"] = json!(null);
    assert_eq!(file_config(c).await, serde_json::to_value(noop()).unwrap());
}

#[tokio::test]
async fn each_stage_is_mapped_independently() {
    let want = serde_json::to_value(noop()).unwrap();

    let mut c = cfg_bare();
    c["delayer"] = delayer(3.0e6);
    let got = file_config(c).await;
    assert_eq!(got["delayer"]["mean_ns"], json!(3.0e6));
    assert_eq!(got["delayer"]["initialization_rate_pps"], json!(1.0e6));
    assert_eq!(got["limiter"], want["limiter"]);
    assert_eq!(got["loss_model"], want["loss_model"]);

    let mut c = cfg_bare();
    c["limiter"] = json!({"mode": "TokenBucketFilter", "rate_bps": 8.0e9});
    let got = file_config(c).await;
    assert_eq!(got["delayer"], want["delayer"]);
    assert_eq!(got["limiter"]["mode"], json!("TokenBucketFilter"));
    assert_eq!(got["limiter"]["activate"], json!(true));
    assert_eq!(got["limiter"]["rate_bps"], json!(8.0e9));
    assert_eq!(got["loss_model"], want["loss_model"]);

    let mut c = cfg_bare();
    c["loss_model"] = json!({"type": "GilbertElliot", "p": 0.01, "r": 0.3, "k": 1.0, "h": 0.5});
    let got = file_config(c).await;
    assert_eq!(got["delayer"], want["delayer"]);
    assert_eq!(got["limiter"], want["limiter"]);
    assert_eq!(
        got["loss_model"],
        serde_json::to_value(LossModel::GilbertElliot(0.01, 0.3, 1.0, 0.5, true)).unwrap()
    );
}

#[tokio::test]
async fn the_other_config_fields_survive_a_config_without_stages() {
    let mut c = cfg_bare();
    c["buffer_capacity"] = json!(512);
    c["reordering"] = json!(true);
    c["seed"] = json!(11);
    let a = app();
    let mut l = link_cfg("l", "net_null0", "net_null0", c);
    l["config"]["delay_stats"] = json!(false);
    l["config"]["histogram"] = json!({"bins": 32, "lo_ns": 0.0, "hi_ns": 1000.0});
    assert_eq!(a.start_links(vec![l]).await.status, 202);
    let file = &a.supervisor.plans()[0].links_json["links"][0];
    assert_eq!(file["config"]["buffer_capacity"], json!(512));
    assert_eq!(file["config"]["reordering"], json!(true));
    assert_eq!(file["config"]["seed"], json!(11));
    assert_eq!(file["delay_stats"], json!(false));
    assert_eq!(file["histogram"]["bins"], json!(32));
}

#[tokio::test]
async fn links_with_and_without_stages_can_run_side_by_side() {
    let a = app();
    let r = a
        .start_links(vec![
            link_cfg("plain", NIC0, NIC1, cfg_bare()),
            link_cfg("full", NIC1, NIC0, cfg_full(1.0e6, 1.0e5, 1.0e9)),
        ])
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
    let links = &a.supervisor.plans()[0].links_json["links"];
    assert_eq!(links[0]["config"]["limiter"]["activate"], json!(false));
    assert_eq!(links[1]["config"]["limiter"]["activate"], json!(true));
    assert_eq!(links[0]["config"]["delayer"]["mean_ns"], json!(0.0));
    assert_eq!(links[1]["config"]["delayer"]["mean_ns"], json!(1.0e6));
}

#[tokio::test]
async fn the_internal_rate_of_the_noop_delayer_is_never_reported_to_clients() {
    let a = app();
    a.start_ok(vec![link_cfg("l", "net_null0", "net_null0", cfg_bare())])
        .await;
    let v = a.get("/api/emulation").await.json();
    let text = v["config"].to_string();
    assert!(!text.contains("initialization_rate_pps"), "{text}");
    assert!(!text.contains("activate"), "{text}");
}

#[tokio::test]
async fn the_drain_timeout_of_a_link_without_stages_is_the_margin_alone() {
    let a = app();
    a.start_ok(vec![link_cfg("l", "net_null0", "net_null0", cfg_bare())])
        .await;
    let t = a.supervisor.plans()[0].drain_timeout.as_secs_f64();
    assert!((t - 5.0).abs() < 1e-9, "{t}");
}

// ---- `activate` is gone -------------------------------------------------------------------------

#[tokio::test]
async fn limiter_activate_is_a_400_naming_the_field_whatever_the_value() {
    for activate in [json!(true), json!(false), json!(null), json!(1)] {
        let a = app();
        let mut c = cfg_json(1.0e6, 0.0);
        c["limiter"] = json!({"mode": "Spacer", "activate": activate, "rate_bps": 1.0e9});
        let s = setup(vec![link_cfg("l", "net_null0", "net_null0", c)]);
        for r in [
            a.start(&s).await,
            a.validate(&s).await,
            a.post("/api/host/hints", s.body()).await,
        ] {
            assert_eq!(r.status, 400, "activate {activate}: {}", r.text());
            assert!(r.message().contains("activate"), "{}", r.text());
        }
        assert!(a.supervisor.plans().is_empty());
    }
}

// ---- a present stage is validated whole ------------------------------------------------------

#[tokio::test]
async fn a_present_stage_without_a_required_field_is_a_400_naming_it() {
    let cases = [
        ("limiter", json!({"rate_bps": 1.0e9}), "mode"),
        ("limiter", json!({"mode": "Spacer"}), "rate_bps"),
        ("limiter", json!({}), "mode"),
        ("delayer", json!({}), "distribution"),
        (
            "delayer",
            json!({"distribution": "NormalDistribution", "mean_ns": 1.0, "std_ns": 1.0,
                   "activate_correlation": false, "half_life_period_ns": 0.0}),
            "initialization_rate_pps",
        ),
        (
            "delayer",
            json!({"distribution": "NormalDistribution", "mean_ns": 1.0,
                   "activate_correlation": false, "half_life_period_ns": 0.0,
                   "initialization_rate_pps": 1.0}),
            "std_ns",
        ),
        ("loss_model", json!({"type": "Random"}), "p"),
        ("loss_model", json!({"p": 0.1}), "type"),
    ];
    for (stage, value, named) in cases {
        let a = app();
        let mut c = cfg_bare();
        c[stage] = value.clone();
        let r = a
            .start_links(vec![link_cfg("l", "net_null0", "net_null0", c)])
            .await;
        assert_eq!(r.status, 400, "{stage} {value}: {}", r.text());
        assert!(r.message().contains(named), "{stage} {value}: {}", r.text());
        assert!(a.supervisor.plans().is_empty());
    }
}

#[tokio::test]
async fn a_present_stage_is_validated_with_the_old_rules() {
    let bad: Vec<(&str, Value, &str)> = vec![
        (
            "limiter rate 0",
            json!({"limiter": {"mode": "Spacer", "rate_bps": 0.0}}),
            "rate_bps",
        ),
        (
            "limiter rate negative",
            json!({"limiter": {"mode": "TokenBucketFilter", "rate_bps": -5.0}}),
            "rate_bps",
        ),
        (
            "negative std",
            json!({"delayer": {"distribution": "NormalDistribution", "mean_ns": 1.0e6,
                "std_ns": -1.0, "activate_correlation": false, "half_life_period_ns": 0.0,
                "initialization_rate_pps": 1.0e6}}),
            "std_ns",
        ),
        (
            "negative mean",
            json!({"delayer": {"distribution": "NormalDistribution", "mean_ns": -1.0,
                "std_ns": 1.0, "activate_correlation": false, "half_life_period_ns": 0.0,
                "initialization_rate_pps": 1.0e6}}),
            "mean_ns",
        ),
        (
            "rate below one",
            json!({"delayer": {"distribution": "NormalDistribution", "mean_ns": 1.0e6,
                "std_ns": 1.0, "activate_correlation": false, "half_life_period_ns": 0.0,
                "initialization_rate_pps": 0.5}}),
            "initialization_rate_pps",
        ),
        (
            "negative half-life",
            json!({"delayer": {"distribution": "NormalDistribution", "mean_ns": 1.0e6,
                "std_ns": 1.0, "activate_correlation": false, "half_life_period_ns": -1.0,
                "initialization_rate_pps": 1.0e6}}),
            "half_life",
        ),
        (
            "loss p above one",
            json!({"loss_model": {"type": "Random", "p": 1.5}}),
            "p",
        ),
        (
            "gilbert-elliot k below zero",
            json!({"loss_model": {"type": "GilbertElliot", "p": 0.1, "r": 0.2, "k": -0.1, "h": 0.3}}),
            "k",
        ),
    ];
    for (what, stage, named) in bad {
        let a = app();
        let mut c = cfg_bare();
        for (k, v) in stage.as_object().unwrap() {
            c[k] = v.clone();
        }
        let s = setup(vec![link_cfg("l", "net_null0", "net_null0", c)]);
        let r = a.start(&s).await;
        assert_eq!(r.status, 400, "{what}: {}", r.text());
        assert!(r.message().contains(named), "{what}: {}", r.text());
        assert_eq!(a.validate(&s).await.status, 400, "validate {what}");
        assert!(a.supervisor.plans().is_empty(), "{what}");
    }
}

#[tokio::test]
async fn a_valid_present_stage_is_accepted_next_to_missing_ones() {
    for (what, stage) in [
        ("delayer", json!({"delayer": delayer(5.0e6)})),
        (
            "limiter",
            json!({"limiter": {"mode": "Spacer", "rate_bps": 1.0}}),
        ),
        ("loss", json!({"loss_model": {"type": "Random", "p": 1.0}})),
    ] {
        let a = app();
        let mut c = cfg_bare();
        for (k, v) in stage.as_object().unwrap() {
            c[k] = v.clone();
        }
        let r = a
            .start_links(vec![link_cfg("l", "net_null0", "net_null0", c)])
            .await;
        assert_eq!(r.status, 202, "{what}: {}", r.text());
    }
}

#[tokio::test]
async fn the_correlation_rate_is_required_with_the_delayer_and_never_without_it() {
    // present delayer: the rate is required, no preset
    let a = app();
    let mut d = delayer(1.0e6);
    d.as_object_mut().unwrap().remove("initialization_rate_pps");
    let mut c = cfg_bare();
    c["delayer"] = d;
    let r = a
        .start_links(vec![link_cfg("l", "net_null0", "net_null0", c)])
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("initialization_rate_pps"));
    // missing delayer: nothing to ask for, and a stray rate outside a delayer is an unknown field
    let mut c = cfg_bare();
    c["initialization_rate_pps"] = json!(1.0);
    let r = a
        .start_links(vec![link_cfg("l", "net_null0", "net_null0", c)])
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("initialization_rate_pps"));
}

#[tokio::test]
async fn the_other_config_fields_keep_their_rules_without_stages() {
    let a = app();
    let mut c = cfg_bare();
    c["buffer_capacity"] = json!(2_147_483_649u64);
    let r = a
        .start_links(vec![link_cfg("l", "net_null0", "net_null0", c)])
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("buffer_capacity"));
    let mut c = cfg_bare();
    c["histogram"] = json!({"bins": 4097});
    let r = a
        .start_links(vec![link_cfg("l", "net_null0", "net_null0", c)])
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(
        r.message().contains("4096"),
        "spec 30: names the maximum: {}",
        r.message()
    );
    // a config without a buffer is still incomplete
    let r = a
        .start_links(vec![link_cfg(
            "l",
            "net_null0",
            "net_null0",
            json!({"reordering": false}),
        )])
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.message().contains("buffer_capacity"));
}

// ---- OpenAPI ------------------------------------------------------------------------------------

#[tokio::test]
async fn openapi_marks_the_three_stages_optional_and_has_no_activate() {
    let a = app();
    let v = a.get("/api/openapi.json").await.json();
    let cfg = &v["components"]["schemas"]["ConfigDto"];
    let required: Vec<&str> = cfg["required"]
        .as_array()
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for stage in ["delayer", "limiter", "loss_model"] {
        assert!(
            cfg["properties"][stage].is_object(),
            "{stage} missing: {cfg}"
        );
        assert!(
            !required.contains(&stage),
            "{stage} must be optional: {required:?}"
        );
    }
    assert!(required.contains(&"buffer_capacity"), "{required:?}");
    let lim = &v["components"]["schemas"]["LimiterDto"];
    assert!(lim["properties"].get("activate").is_none(), "{lim}");
    let lim_required: Vec<&str> = lim["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        lim_required.contains(&"mode") && lim_required.contains(&"rate_bps"),
        "{lim_required:?}"
    );
}

// ---- hints ---------------------------------------------------------------------------------------

#[tokio::test]
async fn hints_for_links_without_stages_are_the_ordinary_ones() {
    let a = app();
    let r = a
        .post(
            "/api/host/hints",
            setup(vec![link_cfg("l", NIC0, NIC0, cfg_bare())]).body(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let h = r.json();
    for id in HINT_IDS {
        let _ = hint(&h, id);
    }
    assert!(
        h.as_array()
            .unwrap()
            .iter()
            .all(|x| x["id"] != "drain_timeout"),
        "{h}"
    );
}
