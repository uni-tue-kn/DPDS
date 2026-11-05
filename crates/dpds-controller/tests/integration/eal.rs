//! Pure functions of the supervisor (B3): EAL argv, links file, drain timeout.

use std::path::PathBuf;
use std::time::Duration;

use crate::common::*;
use dpds_controller::eal::{drain_timeout, eal_argv, links_file_json, EalInput, EalPort};
use dpds_controller::lcores::DirectionPlan;
use dpds_core::delayer::RVDistribution;
use dpds_core::dropper::LossModel;
use dpds_core::limiter::LimiterMode;
use dpds_core::{Config, DelayerConfig, LimiterConfig};
use serde_json::json;

fn port(name: &str, driver: Option<&str>) -> EalPort {
    EalPort {
        name: name.into(),
        driver: driver.map(str::to_string),
    }
}

fn plan_dir(dir: u32, link: &str, rx: &str, tx: &str, lcore: u32) -> DirectionPlan {
    DirectionPlan {
        dir,
        link: link.into(),
        rx: rx.into(),
        tx: tx.into(),
        lcore,
    }
}

fn input(ports: Vec<EalPort>) -> EalInput {
    EalInput {
        main_lcore: 0,
        worker_lcores: vec![2, 3],
        ports,
        links_file: PathBuf::from("/run/dpds-controller/links.json"),
    }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn two_mlx5_pci_ports_exact_argv() {
    let argv = eal_argv(&input(vec![
        port(NIC0, Some("mlx5_core")),
        port(NIC1, Some("mlx5_core")),
    ]));
    assert_eq!(
        argv,
        strs(&[
            "-l",
            "0,2,3",
            "--main-lcore",
            "0",
            "--in-memory",
            "--file-prefix",
            "dpds-ctl",
            "-a",
            "0000:17:00.0,txq_mpw_en=1",
            "-a",
            "0000:65:00.0,txq_mpw_en=1",
            "--",
            "--links",
            "/run/dpds-controller/links.json",
        ])
    );
}

#[test]
fn vdev_ports_use_vdev_and_get_no_devargs() {
    let argv = eal_argv(&input(vec![
        port("net_null0", None),
        port("net_pcap1", None),
    ]));
    assert_eq!(
        argv[7..13],
        strs(&[
            "--vdev",
            "net_null0",
            "--vdev",
            "net_pcap1",
            "--",
            "--links"
        ])[..]
    );
    assert!(!argv.iter().any(|a| a.contains("txq_mpw_en")));
    assert!(!argv.contains(&"-a".to_string()));
}

#[test]
fn mlx5_devargs_only_for_pci_ports_bound_to_mlx5_core() {
    let argv = eal_argv(&input(vec![
        port(INTEL, Some("vfio-pci")),
        port(MGMT, Some("tg3")),
        port("0000:aa:00.0", None),
        port(NIC0, Some("mlx5_core")),
    ]));
    let joined = argv.join(" ");
    assert!(joined.contains("-a 0000:b3:00.0 "), "{joined}");
    assert!(joined.contains("-a 0000:03:00.0 "), "{joined}");
    assert!(joined.contains("-a 0000:aa:00.0 "), "{joined}");
    assert!(joined.contains("-a 0000:17:00.0,txq_mpw_en=1 "), "{joined}");
    assert_eq!(joined.matches("txq_mpw_en").count(), 1, "{joined}");
}

#[test]
fn the_mlx5_devargs_are_exactly_txq_mpw_en_1_with_no_trailing_comma() {
    // spec v17: PREOF's `txq_mpw_en=1`, fixed, not configurable
    let argv = eal_argv(&input(vec![port(NIC0, Some("mlx5_core"))]));
    let i = argv.iter().position(|a| a == "-a").unwrap();
    assert_eq!(argv[i + 1], format!("{NIC0},txq_mpw_en=1"));
    for a in &argv {
        if a.contains(NIC0) {
            assert!(!a.ends_with(','), "{a}");
            assert_eq!(a.matches(',').count(), 1, "{a}");
        }
    }
}

#[test]
fn no_devargs_other_than_txq_mpw_en_1_are_ever_generated() {
    for driver in [Some("mlx5_core"), Some("vfio-pci"), Some("tg3"), None] {
        let argv = eal_argv(&input(vec![port(NIC0, driver), port("net_null0", None)]));
        let joined = argv.join(" ");
        assert!(!joined.to_lowercase().contains("mprq"), "{argv:?}");
        // the only `,key=value` anywhere is `,txq_mpw_en=1` on an mlx5_core PCI port
        let devargs = joined.matches('=').count();
        let expected = usize::from(driver == Some("mlx5_core"));
        assert_eq!(devargs, expected, "{argv:?}");
    }
}

#[test]
fn links_file_path_is_the_last_argument() {
    let mut i = input(vec![port("net_null0", None)]);
    i.links_file = PathBuf::from("/tmp/x/links.json");
    let argv = eal_argv(&i);
    assert_eq!(
        argv[argv.len() - 2..],
        strs(&["--links", "/tmp/x/links.json"])[..]
    );
}

/// The dpds_core::Config behind `cfg_json(2.0e6, 0.0)` with the Deterministic delay,
/// Gilbert-Elliot loss and seed 9 that `special_cfg()` produces.
fn special_cfg() -> serde_json::Value {
    let mut v = cfg_json(2.0e6, 0.0);
    v["delayer"]["distribution"] = json!("DeterministicDistribution");
    v["loss_model"] = json!({"type":"GilbertElliot","p":0.01,"r":0.3,"k":1.0,"h":0.5});
    v["seed"] = json!(9);
    v
}

fn special_core() -> Config {
    Config {
        buffer_capacity: 100000,
        reordering: false,
        delayer: DelayerConfig {
            distribution: RVDistribution::DeterministicDistribution,
            mean_ns: 2.0e6,
            std_ns: 0.0,
            activate_correlation: false,
            half_life_period_ns: 1.0e6,
            initialization_rate_pps: 15e6,
        },
        // no limiter stage in `cfg_json`: the missing stage is the inactive limiter, rate 0
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 0.0,
        },
        loss_model: LossModel::GilbertElliot(0.01, 0.3, 1.0, 0.5, true),
        seed: Some(9),
    }
}

#[test]
fn links_file_json_is_v2_one_entry_per_directed_link() {
    let links = vec![
        model_link(1, &link("a-to-b", NIC0, NIC1)),
        model_link(2, &link("b-to-a", NIC1, NIC0)),
        model_link(
            3,
            &link_cfg("loop", "net_null0", "net_null0", special_cfg()),
        ),
    ];
    let dirs = vec![
        plan_dir(0, "a-to-b", NIC0, NIC1, 7),
        plan_dir(1, "b-to-a", NIC1, NIC0, 2),
        plan_dir(2, "loop", "net_null0", "net_null0", 3),
    ];
    let mut v = links_file_json(&links, &dirs).unwrap();
    // `delay_stats` is optional and defaults to true: an explicit `true` is the same file
    for l in v["links"].as_array_mut().unwrap() {
        if l["delay_stats"] == json!(true) {
            l.as_object_mut().unwrap().remove("delay_stats");
        }
    }

    assert_eq!(v["links"].as_array().unwrap().len(), 3);
    let default_cfg = serde_json::to_value(cfg_dto(1.0e6, 1.0e5).to_core().unwrap()).unwrap();
    // exact JSON: the file dpds parses (config = dpds_core::Config's own JSON, the lcore of the
    // direction plan literally, no controller-only keys)
    assert_eq!(
        v,
        json!({ "links": [
            { "name": "a-to-b", "ingress": NIC0, "egress": NIC1,
              "config": default_cfg.clone(), "lcore": 7 },
            { "name": "b-to-a", "ingress": NIC1, "egress": NIC0,
              "config": default_cfg.clone(), "lcore": 2 },
            { "name": "loop", "ingress": "net_null0", "egress": "net_null0",
              "config": serde_json::to_value(special_core()).unwrap(), "lcore": 3 },
        ]})
    );
}

#[test]
fn links_file_json_carries_each_links_initialization_rate_in_its_config() {
    let mut a = cfg_json(1.0e6, 1.0e5);
    a["delayer"]["initialization_rate_pps"] = json!(250_000.0);
    let mut b = cfg_json(1.0e6, 1.0e5);
    b["delayer"]["initialization_rate_pps"] = json!(7.5e6);
    let links = vec![
        model_link(1, &link_cfg("slow", NIC0, NIC1, a)),
        model_link(2, &link_cfg("fast", NIC1, NIC0, b)),
    ];
    let dirs = vec![
        plan_dir(0, "slow", NIC0, NIC1, 2),
        plan_dir(1, "fast", NIC1, NIC0, 3),
    ];
    let v = links_file_json(&links, &dirs).unwrap();
    assert_eq!(
        v["links"][0]["config"]["delayer"]["initialization_rate_pps"],
        json!(250_000.0),
        "{v}"
    );
    assert_eq!(
        v["links"][1]["config"]["delayer"]["initialization_rate_pps"],
        json!(7.5e6),
        "{v}"
    );
}

#[test]
fn links_file_json_has_none_of_the_v1_keys() {
    let l = vec![model_link(1, &link("a-to-b", NIC0, NIC1))];
    let d = [plan_dir(0, "a-to-b", NIC0, NIC1, 2)];
    let v = links_file_json(&l, &d).unwrap();
    for k in [
        "ports",
        "configs",
        "lcores",
        "histograms",
        "workers",
        "worker",
        "id",
    ] {
        assert!(
            v["links"][0].get(k).is_none(),
            "`{k}` must not be in a v2 link: {v}"
        );
    }
}

#[test]
fn lcores_are_taken_by_link_name_from_the_direction_plan_in_any_order() {
    // links in one order, plans in another: the lcore is looked up by link name
    let links = vec![
        model_link(1, &link("first", NIC0, NIC1)),
        model_link(2, &link("second", NIC1, NIC0)),
    ];
    let dirs = vec![
        plan_dir(1, "second", NIC1, NIC0, 2),
        plan_dir(0, "first", NIC0, NIC1, 7),
    ];
    let v = links_file_json(&links, &dirs).unwrap();
    assert_eq!(v["links"][0]["name"], "first");
    assert_eq!(v["links"][0]["lcore"], 7);
    assert_eq!(v["links"][1]["name"], "second");
    assert_eq!(v["links"][1]["lcore"], 2);
}

#[test]
fn links_file_json_histogram_and_delay_stats_only_when_set() {
    let mut off = cfg_json(1.0e6, 1.0e5);
    off["delay_stats"] = json!(false);
    off["histogram"] = json!({"bins": 64, "lo_ns": 0.0, "hi_ns": 2.0e6});
    let mut bins_only = cfg_json(1.0e6, 1.0e5);
    bins_only["histogram"] = json!({"bins": 100});
    let links = vec![
        model_link(1, &link("plain", NIC0, NIC1)),
        model_link(2, &link_cfg("off", NIC1, NIC0, off)),
        model_link(3, &link_cfg("bins", INTEL, INTEL, bins_only)),
    ];
    let dirs = vec![
        plan_dir(0, "plain", NIC0, NIC1, 2),
        plan_dir(1, "off", NIC1, NIC0, 3),
        plan_dir(2, "bins", INTEL, INTEL, 6),
    ];
    let v = links_file_json(&links, &dirs).unwrap();
    let l = &v["links"];
    assert!(l[0].get("histogram").is_none(), "{v}");
    assert!(
        l[0].get("delay_stats").is_none() || l[0]["delay_stats"] == json!(true),
        "{v}"
    );
    assert_eq!(l[1]["delay_stats"], json!(false));
    assert_eq!(l[1]["histogram"]["bins"], 64);
    assert_close(f(&l[1]["histogram"], "lo_ns"), 0.0);
    assert_close(f(&l[1]["histogram"], "hi_ns"), 2.0e6);
    assert_eq!(l[2]["histogram"]["bins"], 100);
    assert!(
        l[2]["histogram"].get("lo_ns").is_none_or(|x| x.is_null()),
        "{v}"
    );
    assert!(l[2].get("delay_stats").is_none() || l[2]["delay_stats"] == json!(true));
}

#[test]
fn links_file_json_has_no_limit_bit_in_the_limiter_of_a_link() {
    let links = vec![model_link(
        1,
        &link_cfg("lim", "net_null0", "net_null0", {
            let mut c = cfg_json(1.0e6, 0.0);
            c["limiter"] = json!({"mode": "TokenBucketFilter", "rate_bps": 2.0e9});
            c
        }),
    )];
    let d = [plan_dir(0, "lim", "net_null0", "net_null0", 1)];
    let v = links_file_json(&links, &d).unwrap();
    let text = v.to_string();
    assert!(!text.contains("limit_bit"), "{text}");
    // dpds parses `dpds_core::Config`: the present limiter stage is active there
    let lim = v["links"][0]["config"]["limiter"].as_object().unwrap();
    let mut keys: Vec<&str> = lim.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["activate", "mode", "rate_bps"], "{text}");
    assert_eq!(lim["activate"], json!(true), "{text}");
    assert_eq!(lim["mode"], json!("TokenBucketFilter"), "{text}");
    assert_eq!(lim["rate_bps"], json!(2.0e9), "{text}");
}

#[test]
fn links_file_json_fails_for_an_invalid_config() {
    let l = model_link(
        1,
        &link_cfg("bad", "net_null0", "net_null0", cfg_json(1.0e6, -1.0)),
    );
    let d = [plan_dir(0, "bad", "net_null0", "net_null0", 1)];
    assert!(links_file_json(&[l], &d).is_err());
}

// ---- optional stages: what the links file carries for a missing stage -------------------------

/// The `dpds_core::Config` a config with no stage at all maps to (spec 22): Deterministic 0 ns
/// without correlation, half-life 0 and the internal rate 1; an inactive Spacer with rate 0;
/// `Random(0)` loss.
fn noop_core() -> Config {
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

fn file_config(cfg: serde_json::Value) -> serde_json::Value {
    let l = model_link(1, &link_cfg("l", "net_null0", "net_null0", cfg));
    let d = [plan_dir(0, "l", "net_null0", "net_null0", 1)];
    links_file_json(&[l], &d).unwrap()["links"][0]["config"].clone()
}

#[test]
fn a_config_without_stages_writes_the_noop_core_config() {
    assert_eq!(
        file_config(cfg_bare()),
        serde_json::to_value(noop_core()).unwrap()
    );
    // `null` is the same as absent, for each stage
    let mut c = cfg_bare();
    c["delayer"] = json!(null);
    c["limiter"] = json!(null);
    c["loss_model"] = json!(null);
    assert_eq!(file_config(c), serde_json::to_value(noop_core()).unwrap());
}

#[test]
fn each_missing_stage_is_replaced_independently_and_the_present_ones_are_kept() {
    let want = serde_json::to_value(noop_core()).unwrap();
    // delayer only
    let mut c = cfg_bare();
    c["delayer"] = cfg_json(2.0e6, 1.0e5)["delayer"].clone();
    let got = file_config(c);
    assert_eq!(got["delayer"]["mean_ns"], json!(2.0e6));
    assert_eq!(got["delayer"]["initialization_rate_pps"], json!(15.0e6));
    assert_eq!(got["limiter"], want["limiter"]);
    assert_eq!(got["loss_model"], want["loss_model"]);
    // limiter only
    let mut c = cfg_bare();
    c["limiter"] = json!({"mode": "Spacer", "rate_bps": 5.0e9});
    let got = file_config(c);
    assert_eq!(got["delayer"], want["delayer"]);
    assert_eq!(got["limiter"]["activate"], json!(true));
    assert_eq!(got["limiter"]["rate_bps"], json!(5.0e9));
    assert_eq!(got["loss_model"], want["loss_model"]);
    // loss only
    let mut c = cfg_bare();
    c["loss_model"] = json!({"type": "Random", "p": 0.25});
    let got = file_config(c);
    assert_eq!(got["delayer"], want["delayer"]);
    assert_eq!(got["limiter"], want["limiter"]);
    assert_eq!(
        got["loss_model"],
        serde_json::to_value(LossModel::Random(0.25)).unwrap()
    );
}

#[test]
fn the_buffer_seed_and_reordering_survive_without_stages() {
    let mut c = cfg_bare();
    c["buffer_capacity"] = json!(777);
    c["reordering"] = json!(true);
    c["seed"] = json!(5);
    let got = file_config(c);
    assert_eq!(got["buffer_capacity"], json!(777));
    assert_eq!(got["reordering"], json!(true));
    assert_eq!(got["seed"], json!(5));
}

// ---- drain timeout -------------------------------------------------------

/// The limiter's limit is derived: `capacity * 1542 * 8` bit.
fn core(mean_ns: f64, std_ns: f64, limiter_on: bool, rate_bps: f64, capacity: u64) -> Config {
    let mut v = cfg_json(mean_ns, std_ns);
    v["buffer_capacity"] = json!(capacity);
    if limiter_on {
        v["limiter"] = json!({"mode":"Spacer","rate_bps":rate_bps});
    }
    dto_from(v).to_core().unwrap()
}

fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

#[test]
fn drain_timeout_is_mean_plus_6_std_plus_5_s() {
    let t = drain_timeout(&[core(10.0e6, 1.0e6, false, 1.0e9, 1000)]);
    assert!((secs(t) - 5.016).abs() < 1e-6, "{t:?}");
}

#[test]
fn drain_timeout_adds_limit_over_rate_only_when_the_limiter_is_on() {
    // capacity 1000: 1000 * 1542 * 8 = 12.336 Mbit at 1 Gb/s = 12.336 ms
    let on = drain_timeout(&[core(10.0e6, 0.0, true, 1.0e9, 1000)]);
    assert!((secs(on) - 5.022336).abs() < 1e-6, "{on:?}");
    let off = drain_timeout(&[core(10.0e6, 0.0, false, 1.0e9, 1_000_000_000)]);
    assert!((secs(off) - 5.010).abs() < 1e-6, "{off:?}");
}

#[test]
fn drain_timeout_uses_the_derived_limit_of_the_capacity_for_both_modes_of_the_limiter() {
    // twice the capacity, twice the limiter term
    let one = drain_timeout(&[core(0.0, 0.0, true, 1.0e9, 1000)]);
    let two = drain_timeout(&[core(0.0, 0.0, true, 1.0e9, 2000)]);
    assert!((secs(one) - 5.012336).abs() < 1e-6, "{one:?}");
    assert!((secs(two) - 5.024672).abs() < 1e-6, "{two:?}");
    // the rate scales it inversely: 12.336 Mbit at 1 Mb/s = 12.336 s
    let slow = drain_timeout(&[core(0.0, 0.0, true, 1.0e6, 1000)]);
    assert!((secs(slow) - 17.336).abs() < 1e-6, "{slow:?}");
}

#[test]
fn drain_timeout_is_the_maximum_over_directions() {
    let t = drain_timeout(&[
        core(1.0e6, 0.0, false, 1.0e9, 1000),
        core(2.0e9, 1.0e8, false, 1.0e9, 1000), // 2.6 s
        core(3.0e6, 0.0, false, 1.0e9, 1000),
    ]);
    assert!((secs(t) - 7.6).abs() < 1e-6, "{t:?}");
}

#[test]
fn drain_timeout_has_a_margin_even_without_delay() {
    let t = drain_timeout(&[core(0.0, 0.0, false, 1.0e9, 1000)]);
    assert!((secs(t) - 5.0).abs() < 1e-6, "{t:?}");
}

#[test]
fn drain_timeout_of_configs_without_stages_is_the_margin_alone() {
    // the missing limiter is inactive (rate 0: no division by it), the missing delayer is 0 ns
    let none = dto_from(cfg_bare()).to_core().unwrap();
    let t = drain_timeout(&[none]);
    assert!((secs(t) - 5.0).abs() < 1e-9, "{t:?}");
    // each stage alone still counts: a 2 s delay, a 12.336 s limiter drain
    let mut dly = cfg_bare();
    dly["delayer"] = json!({"distribution": "DeterministicDistribution", "mean_ns": 2.0e9,
        "std_ns": 0.0, "activate_correlation": false, "half_life_period_ns": 0.0,
        "initialization_rate_pps": 1.0});
    let mut lim = cfg_bare();
    lim["buffer_capacity"] = json!(1000);
    lim["limiter"] = json!({"mode": "Spacer", "rate_bps": 1.0e6});
    let t = drain_timeout(&[
        dto_from(dly).to_core().unwrap(),
        dto_from(lim).to_core().unwrap(),
    ]);
    assert!((secs(t) - 17.336).abs() < 1e-6, "{t:?}");
}
