//! C3 input validation and the `buffer_capacity` cap of C5, all through the body of
//! `POST /api/emulation/start` (spec 22; a rejected value is a 400 `{"message"}` naming the value,
//! and nothing is spawned). `POST /api/emulation/validate` applies the same rules.
//!
//! - port names (spec v7 S1): a PCI address `^[0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-7]$`
//!   (normalised to lower case) or exactly a vdev `^net_(null|ring)[0-9]+$`: no `,key=value`
//!   devargs at all and no `net_pcap` (it needs `rx_pcap`/`tx_pcap`/`iface` devargs, which
//!   write files as root or bypass the management-NIC guard); nothing with whitespace, a
//!   leading `-`, or a smuggled option;
//! - `settings.eal_extra_args` of the body is an allowlist of tokens: `--no-huge`, `--no-pci`,
//!   `-m <n>`, `-n <n>`, `--socket-mem=<list>` and `--log-level=<spec>`. DPDK 26.07's
//!   long options take their value after `=` (tested by experiment: `--log-level 7`
//!   as two tokens is "unknown argument"), so the long forms are single tokens here;
//!   the two-token long forms are left open (not tested either way);
//! - the mlx5 devargs are fixed to PREOF's `txq_mpw_en=1` (spec v17): `settings.mlx5_devargs`
//!   does not exist, so any body that carries it is a 400 naming the field, whatever the value;
//! - `buffer_capacity` is at most 2^31 (400 above).

use crate::common::*;
use dpds_controller::dto::ConfigDto;
use serde_json::{json, Value};

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

/// A one-link start body.
fn one(name: &str, ingress: &str, egress: &str) -> Setup {
    setup(vec![link(name, ingress, egress)])
}

// ---- port names ------------------------------------------------------------

#[tokio::test]
async fn valid_pci_and_vdev_names_are_accepted() {
    for (i, port) in [
        "0000:65:00.0",
        "0000:b3:00.7",
        "ffff:ff:1f.3",
        "net_null0",
        "net_null12",
        "net_ring0",
        "net_ring3",
        "net_ring0123",
    ]
    .iter()
    .enumerate()
    {
        // as a hairpin (ingress and egress) and as the egress of a link from a vdev;
        // validate answers the plan, so the lcore plan of an unknown PCI address is fine too
        let a = app();
        let r = a.validate(&one(&format!("l{i}"), port, port)).await;
        assert_eq!(r.status, 200, "{port}: {}", r.text());
        let r = a.validate(&one(&format!("m{i}"), "net_null9", port)).await;
        assert_eq!(r.status, 200, "{port} as egress: {}", r.text());
    }
}

#[tokio::test]
async fn vdev_ports_start() {
    for (i, port) in ["net_null0", "net_null12", "net_ring0", "net_ring0123"]
        .iter()
        .enumerate()
    {
        let a = app();
        let r = a.start(&one(&format!("l{i}"), port, port)).await;
        assert_eq!(r.status, 202, "{port}: {}", r.text());
    }
}

#[tokio::test]
async fn pci_addresses_are_reported_in_lower_case_and_conflict_across_case() {
    let a = app();
    a.start_ok(vec![link("up", "0000:B3:00.0", "0000:B3:00.1")])
        .await;
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["config"]["links"][0]["ingress"], "0000:b3:00.0", "{v}");
    assert_eq!(v["config"]["links"][0]["egress"], "0000:b3:00.1", "{v}");
    // the same ingress in another case is the same port: 409 naming the first link
    let b = app();
    let r = b
        .start_links(vec![
            link("up", "0000:B3:00.0", "0000:B3:00.1"),
            link("again", "0000:b3:00.0", "0000:b3:00.2"),
        ])
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().contains("up"), "{}", r.text());
    let r = b
        .start_links(vec![
            link("up", "0000:B3:00.0", "0000:B3:00.1"),
            link("again2", "0000:b3:00.3", "0000:B3:00.1"),
        ])
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().contains("up"), "{}", r.text());
}

/// Spec 33c item 2: two links whose ports differ only in the case of the hex digits name the same
/// port, whatever the route: start, validate and the hints post all answer 409 naming the first
/// link (the hints post used to be the one that could miss it).
#[tokio::test]
async fn ports_that_differ_only_in_case_are_a_reused_port_on_start_validate_and_hints() {
    let cases = [
        (
            "ingress",
            [
                link("first", "0000:AB:00.0", "0000:17:00.0"),
                link("second", "0000:ab:00.0", "0000:65:00.0"),
            ],
        ),
        (
            "ingress, the lower-case link first",
            [
                link("first", "0000:ab:00.0", "0000:17:00.0"),
                link("second", "0000:AB:00.0", "0000:65:00.0"),
            ],
        ),
        (
            "egress",
            [
                link("first", "0000:17:00.0", "0000:Af:00.1"),
                link("second", "0000:65:00.0", "0000:aF:00.1"),
            ],
        ),
        (
            "ingress in mixed case",
            [
                link("first", "0000:Ab:Cd.0", "0000:17:00.0"),
                link("second", "0000:aB:cD.0", "0000:65:00.0"),
            ],
        ),
    ];
    for (what, links) in cases {
        let s = setup(links.to_vec());
        for (route, path) in [
            ("start", "/api/emulation/start"),
            ("validate", "/api/emulation/validate"),
            ("hints", "/api/host/hints"),
        ] {
            let a = app();
            let r = a.post(path, s.body()).await;
            assert_eq!(r.status, 409, "{route}, {what}: {}", r.text());
            assert!(
                r.message().contains("first"),
                "{route}, {what} names the first link: {}",
                r.text()
            );
            assert!(
                a.supervisor.plans().is_empty(),
                "{route}, {what}: nothing started"
            );
        }
    }
}

/// The same ports in one case each are not a conflict (the rule is about identity, not spelling).
#[tokio::test]
async fn ports_that_differ_in_case_and_in_the_digits_are_two_ports() {
    let s = setup(vec![
        link("first", "0000:AB:00.0", "0000:17:00.0"),
        link("second", "0000:ab:00.1", "0000:65:00.0"),
    ]);
    for path in [
        "/api/emulation/validate",
        "/api/host/hints",
        "/api/emulation/start",
    ] {
        let r = app().post(path, s.body()).await;
        assert!(r.status < 300, "{path}: {} {}", r.status, r.text());
    }
}

const BAD_PORTS: &[&str] = &[
    "",
    " ",
    "0000:65:00.8",
    "000:65:00.0",
    "0000:65:00",
    "0000:65:0.0",
    "00000:65:00.0",
    "0000:65:00.00",
    "gggg:65:00.0",
    "0000:65:00.0 ",
    " 0000:65:00.0",
    "0000:65:00.0\n",
    "0000:65:00.0,txq_mpw_en=1",
    "0000:65:00.0 --vdev=net_null0",
    "-a",
    "--x",
    "--vdev=net_null0",
    "-net_null0",
    "net_null",
    "net_tap0",
    "NET_NULL0",
    "net_null0 ",
    "net_null0\t--x",
    "net_null0 --vdev=net_null1",
    "net_null0,",
    "net_null0,size",
    "net_null0,size=",
    "net_null0,size=1 2",
    "net_null0,Size=1",
    "net_null0,--x=1",
    "net_null0,a=b,,c=d",
    "net_null0,a=b,c=d,e",
    "./net_null0",
    "net_null0;rm",
    // no devargs at all, no net_pcap
    "net_pcap0",
    "net_pcap3",
    "net_pcap1,rx_pcap=/tmp/in.pcap,tx_pcap=/tmp/out.pcap",
    "net_pcap0,tx_pcap=/etc/cron.d/x",
    "net_pcap0,iface=eth0",
    "net_null0,iface=eth0",
    "net_null0,size=64,copy=1",
    "net_null0,size=64",
    "net_ring0,nodeaction=x",
    "net_ring0,a=b",
    "net_tap0,iface=eth0",
    "net_af_packet0,iface=eth0",
];

#[tokio::test]
async fn invalid_port_names_are_a_400_naming_the_value() {
    // control: valid ports in the same body shape are accepted
    assert_eq!(
        app()
            .start(&one("ok", "net_null0", "net_null1"))
            .await
            .status,
        202
    );
    for bad in BAD_PORTS {
        let a = app();
        for as_ingress in [true, false] {
            let s = if as_ingress {
                one("x", bad, "net_null1")
            } else {
                one("x", "net_null1", bad)
            };
            let r = a.start(&s).await;
            assert_eq!(
                r.status,
                400,
                "{bad:?} (ingress {as_ingress}): {}",
                r.text()
            );
            let m = r.message();
            if !bad.trim().is_empty() {
                assert!(m.contains(bad.trim()) || m.contains("port"), "{bad:?}: {m}");
            }
            let r = a.validate(&s).await;
            assert_eq!(r.status, 400, "validate {bad:?} (ingress {as_ingress})");
        }
        assert!(a.supervisor.plans().is_empty(), "{bad:?} was spawned");
        assert_eq!(
            a.get("/api/emulation").await.json()["state"],
            "stopped",
            "{bad:?}"
        );
    }
}

// ---- mlx5_devargs (gone) ------------------------------------------------------------------

#[tokio::test]
async fn a_settings_member_with_mlx5_devargs_is_a_400_naming_settings_whatever_the_value() {
    for d in [
        json!(""),
        json!("txq_mpw_en=1"),
        json!("txq_mpw_en=1,txq_inline_max=0"),
        json!("rxq_cqe_comp_en=1,rx_vec_en=1"),
        json!("tx_pp=500"),
        json!("mprq_en=1"),
        json!("bogus_key=1"),
        json!("txq_mpw_en=1,"),
        // not only strings: a null or a number is the same unknown field
        json!(null),
        json!(1),
        json!(["txq_mpw_en=1"]),
    ] {
        let a = app();
        let mut body = one("l", "net_null0", "net_null0").body();
        body["settings"] = json!({ "mlx5_devargs": d });
        let r = a.post("/api/emulation/start", body).await;
        assert_eq!(r.status, 400, "{d}: {}", r.text());
        assert!(r.message().contains("settings"), "{d}: {}", r.text());
        assert!(a.supervisor.plans().is_empty(), "{d} was spawned");
    }
}

// ---- buffer_capacity ---------------------------------------------------------

#[tokio::test]
async fn buffer_capacity_up_to_two_to_the_31_is_valid_above_it_is_a_400() {
    let a = app();
    let mut l = link("big", "net_null0", "net_null0");
    l["config"]["buffer_capacity"] = json!(2_147_483_648u64);
    assert_eq!(a.validate(&setup(vec![l])).await.status, 200);
    for over in [2_147_483_649u64, 4_294_967_296, u64::MAX] {
        let mut l = link("bigger", "net_null1", "net_null1");
        l["config"]["buffer_capacity"] = json!(over);
        let r = a.start_links(vec![l]).await;
        assert_eq!(r.status, 400, "{over}: {}", r.text());
        assert!(r.message().contains("buffer_capacity"), "{over}");
    }
    assert!(a.supervisor.plans().is_empty());
}

#[test]
fn the_dto_rejects_a_buffer_capacity_above_two_to_the_31() {
    let mut c = cfg_json(1.0e6, 0.0);
    c["buffer_capacity"] = json!(2_147_483_648u64);
    assert!(dto_from(c.clone()).to_core().is_ok());
    c["buffer_capacity"] = json!(2_147_483_649u64);
    let dto: ConfigDto = dto_from(c);
    let e = dto.to_core().unwrap_err().to_string();
    assert!(e.contains("buffer_capacity"), "{e}");
}

// ---- delay accuracy mode (spec 45 B1) -----------------------------------------

/// A link config with `reordering`, the delayer's correlation and an optional limiter mode.
fn accuracy_link(name: &str, reordering: bool, correlation: bool, limiter: Option<&str>) -> Value {
    let mut c = cfg_json(1.0e6, 1.0e5);
    c["reordering"] = json!(reordering);
    c["delayer"]["activate_correlation"] = json!(correlation);
    if let Some(mode) = limiter {
        c["limiter"] = json!({ "mode": mode, "rate_bps": 1.0e9 });
    }
    link_cfg(name, "net_null0", "net_null0", c)
}

#[tokio::test]
async fn reordering_with_correlation_is_a_400_naming_the_combination_on_start_and_validate() {
    let a = app();
    let s = setup(vec![accuracy_link("l", true, true, None)]);
    for (route, r) in [
        ("start", a.start(&s).await),
        ("validate", a.validate(&s).await),
    ] {
        assert_eq!(r.status, 400, "{route}: {}", r.text());
        let m = r.message();
        assert!(
            m.contains("reordering") && m.contains("correlation"),
            "{route}: {m}"
        );
    }
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn reordering_with_the_spacer_limiter_is_a_400_naming_the_combination_on_start_and_validate()
{
    let a = app();
    let s = setup(vec![accuracy_link("l", true, false, Some("Spacer"))]);
    for (route, r) in [
        ("start", a.start(&s).await),
        ("validate", a.validate(&s).await),
    ] {
        assert_eq!(r.status, 400, "{route}: {}", r.text());
        let m = r.message();
        assert!(
            m.contains("reordering") && m.contains("TokenBucketFilter") && m.contains("Spacer"),
            "{route}: {m}"
        );
    }
    assert!(a.supervisor.plans().is_empty());
}

#[tokio::test]
async fn the_allowed_delay_accuracy_combinations_validate_and_start() {
    for (i, (reordering, correlation, limiter)) in [
        (false, false, None),
        (false, true, None),
        (false, false, Some("Spacer")),
        (false, true, Some("Spacer")),
        (false, true, Some("TokenBucketFilter")),
        (true, false, None),
        (true, false, Some("TokenBucketFilter")),
    ]
    .into_iter()
    .enumerate()
    {
        let a = app();
        let s = setup(vec![accuracy_link(
            &format!("l{i}"),
            reordering,
            correlation,
            limiter,
        )]);
        let why =
            format!("reordering {reordering}, correlation {correlation}, limiter {limiter:?}");
        let r = a.validate(&s).await;
        assert_eq!(r.status, 200, "validate {why}: {}", r.text());
        let r = a.start(&s).await;
        assert_eq!(r.status, 202, "start {why}: {}", r.text());
    }
}

#[test]
fn the_dto_rejects_the_two_reordering_combinations_naming_them() {
    let mut c = cfg_json(1.0e6, 0.0);
    c["reordering"] = json!(true);
    c["delayer"]["activate_correlation"] = json!(true);
    let e = dto_from(c).to_core().unwrap_err().to_string();
    assert!(e.contains("reordering") && e.contains("correlation"), "{e}");

    let mut c = cfg_full(1.0e6, 0.0, 1.0e9); // Spacer
    c["reordering"] = json!(true);
    let e = dto_from(c).to_core().unwrap_err().to_string();
    assert!(e.contains("reordering") && e.contains("Spacer"), "{e}");
}
