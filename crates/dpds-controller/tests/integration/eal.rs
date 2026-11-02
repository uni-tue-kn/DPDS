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
        mlx5_devargs: "txq_mpw_en=1".into(),
        eal_extra_args: vec![],
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
fn empty_devargs_add_no_trailing_comma_and_custom_devargs_pass_through() {
    let mut i = input(vec![port(NIC0, Some("mlx5_core"))]);
    i.mlx5_devargs = String::new();
    assert!(eal_argv(&i).contains(&NIC0.to_string()));
    i.mlx5_devargs = "txq_mpw_en=1,rxq_cqe_comp_en=0".into();
    assert!(eal_argv(&i).contains(&format!("{NIC0},txq_mpw_en=1,rxq_cqe_comp_en=0")));
}

#[test]
fn mprq_is_never_generated() {
    for driver in [Some("mlx5_core"), Some("vfio-pci"), None] {
        let argv = eal_argv(&input(vec![port(NIC0, driver), port("net_null0", None)]));
        assert!(!argv.join(" ").contains("mprq"), "{argv:?}");
    }
}

#[test]
fn extra_args_come_after_the_ports_and_before_the_separator() {
    let mut i = input(vec![port("net_null0", None)]);
    i.worker_lcores = vec![1];
    i.eal_extra_args = strs(&["--no-huge", "-m", "256", "--no-pci"]);
    assert_eq!(
        eal_argv(&i),
        strs(&[
            "-l",
            "0,1",
            "--main-lcore",
            "0",
            "--in-memory",
            "--file-prefix",
            "dpds-ctl",
            "--vdev",
            "net_null0",
            "--no-huge",
            "-m",
            "256",
            "--no-pci",
            "--",
            "--links",
            "/run/dpds-controller/links.json",
        ])
    );
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

#[test]
fn links_file_json_has_the_dpds_format() {
    let mut ge = link(2, "loop", &["net_null0"], None);
    ge.configs[0] = dto_from({
        let mut v = cfg_json(2.0e6, 0.0);
        v["delayer"]["distribution"] = json!("DeterministicDistribution");
        v["loss_model"] = json!({"type":"GilbertElliot","p":0.01,"r":0.3,"k":1.0,"h":0.5});
        v["seed"] = json!(9);
        v
    });
    let links = vec![link(1, "a-b", &[NIC0, NIC1], Some(vec![2, 6])), ge];
    let dirs = vec![
        plan_dir(0, "a-b", NIC0, NIC1, 7),
        plan_dir(1, "a-b", NIC1, NIC0, 2),
        plan_dir(2, "loop", "net_null0", "net_null0", 3),
    ];
    let v = links_file_json(&links, &dirs).unwrap();

    let expected_cfg = Config {
        buffer_capacity: 100000,
        reordering: false,
        delayer: DelayerConfig {
            distribution: RVDistribution::DeterministicDistribution,
            mean_ns: 2.0e6,
            std_ns: 0.0,
            activate_correlation: false,
            half_life_period_ns: 1.0e6,
        },
        limiter: LimiterConfig {
            mode: LimiterMode::Spacer,
            activate: false,
            rate_bps: 1.0e9,
            limit_bit: 1.0e6,
        },
        loss_model: LossModel::GilbertElliot(0.01, 0.3, 1.0, 0.5, true),
        seed: Some(9),
    };
    assert_eq!(v["links"].as_array().unwrap().len(), 2);
    assert_eq!(v["links"][0]["name"], "a-b");
    assert_eq!(v["links"][0]["ports"], json!([NIC0, NIC1]));
    assert_eq!(v["links"][0]["configs"].as_array().unwrap().len(), 2);
    assert_eq!(v["links"][1]["name"], "loop");
    assert_eq!(v["links"][1]["ports"], json!(["net_null0"]));
    // configs are exactly dpds_core::Config's own JSON (what dpds parses)
    assert_eq!(
        v["links"][1]["configs"][0],
        serde_json::to_value(expected_cfg).unwrap()
    );
    // explicit per-direction lcores, literally as assigned (here descending: [7, 2])
    assert_eq!(v["links"][0]["lcores"], json!([7, 2]));
    assert_eq!(v["links"][1]["lcores"], json!([3]));
    // `workers` is controller-only and must not leak into the dpds file
    assert!(v["links"][0].get("workers").is_none());
    assert!(v["links"][0].get("id").is_none());
}

#[test]
fn links_file_json_fails_for_an_invalid_config() {
    let mut l = link(1, "bad", &["net_null0"], None);
    l.configs[0] = cfg_dto(1.0e6, -1.0);
    let d = [plan_dir(0, "bad", "net_null0", "net_null0", 1)];
    assert!(links_file_json(&[l], &d).is_err());
}

// ---- drain timeout -------------------------------------------------------

fn core(mean_ns: f64, std_ns: f64, limiter_on: bool, rate_bps: f64, limit_bit: f64) -> Config {
    let mut v = cfg_json(mean_ns, std_ns);
    v["limiter"] =
        json!({"mode":"Spacer","activate":limiter_on,"rate_bps":rate_bps,"limit_bit":limit_bit});
    dto_from(v).to_core().unwrap()
}

fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

#[test]
fn drain_timeout_is_mean_plus_6_std_plus_5_s() {
    let t = drain_timeout(&[core(10.0e6, 1.0e6, false, 1.0e9, 1.0e6)]);
    assert!((secs(t) - 5.016).abs() < 1e-6, "{t:?}");
}

#[test]
fn drain_timeout_adds_limit_over_rate_only_when_the_limiter_is_on() {
    let on = drain_timeout(&[core(10.0e6, 0.0, true, 1.0e9, 2.0e6)]); // 2 ms
    assert!((secs(on) - 5.012).abs() < 1e-6, "{on:?}");
    let off = drain_timeout(&[core(10.0e6, 0.0, false, 1.0e9, 2.0e9)]);
    assert!((secs(off) - 5.010).abs() < 1e-6, "{off:?}");
}

#[test]
fn drain_timeout_is_the_maximum_over_directions() {
    let t = drain_timeout(&[
        core(1.0e6, 0.0, false, 1.0e9, 1.0e6),
        core(2.0e9, 1.0e8, false, 1.0e9, 1.0e6), // 2.6 s
        core(3.0e6, 0.0, false, 1.0e9, 1.0e6),
    ]);
    assert!((secs(t) - 7.6).abs() < 1e-6, "{t:?}");
}

#[test]
fn drain_timeout_has_a_margin_even_without_delay() {
    let t = drain_timeout(&[core(0.0, 0.0, false, 1.0e9, 1.0e6)]);
    assert!((secs(t) - 5.0).abs() < 1e-6, "{t:?}");
}
