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
    setup(vec![link(name, ingress, egress, None)])
}

fn with_eal(eal: Value) -> Setup {
    setup_eal(vec![link("l", "net_null0", "net_null0", None)], eal)
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
    a.start_ok(vec![link("up", "0000:B3:00.0", "0000:B3:00.1", None)])
        .await;
    let v = a.get("/api/emulation").await.json();
    assert_eq!(v["config"]["links"][0]["ingress"], "0000:b3:00.0", "{v}");
    assert_eq!(v["config"]["links"][0]["egress"], "0000:b3:00.1", "{v}");
    // the same ingress in another case is the same port: 409 naming the first link
    let b = app();
    let r = b
        .start_links(vec![
            link("up", "0000:B3:00.0", "0000:B3:00.1", None),
            link("again", "0000:b3:00.0", "0000:b3:00.2", None),
        ])
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().contains("up"), "{}", r.text());
    let r = b
        .start_links(vec![
            link("up", "0000:B3:00.0", "0000:B3:00.1", None),
            link("again2", "0000:b3:00.3", "0000:B3:00.1", None),
        ])
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().contains("up"), "{}", r.text());
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

// ---- eal_extra_args ----------------------------------------------------------

#[tokio::test]
async fn allowed_eal_arguments_are_accepted() {
    for (args, what) in [
        (json!([]), "empty"),
        (json!(["--no-huge"]), "--no-huge"),
        (json!(["--no-pci"]), "--no-pci"),
        (json!(["-m", "256"]), "-m 256"),
        (json!(["-n", "4"]), "-n 4"),
        (json!(["--no-huge", "-m", "256", "--no-pci"]), "the e2e set"),
        (json!(["--socket-mem=1024,1024"]), "--socket-mem="),
        (
            json!(["--log-level=lib.eal:debug"]),
            "--log-level= type:level",
        ),
        (json!(["--log-level=7"]), "--log-level= number"),
        (json!(["-m", "1024", "-n", "2", "--no-huge"]), "mixed order"),
    ] {
        let a = app();
        let r = a.start(&with_eal(args.clone())).await;
        assert_eq!(r.status, 202, "{what}: {}", r.text());
        // the argv carries them, the running configuration reports them
        let v = a.get("/api/emulation").await.json();
        assert_eq!(v["config"]["settings"]["eal_extra_args"], args, "{what}");
        let argv = &a.supervisor.plans()[0].argv;
        let wanted: Vec<String> = args
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        assert!(
            wanted.is_empty() || argv.windows(wanted.len()).any(|w| w == wanted.as_slice()),
            "{what}: {argv:?}"
        );
    }
}

#[tokio::test]
async fn other_eal_arguments_are_a_400_naming_the_token() {
    // (arguments, a token the message must name)
    for (args, named) in [
        (json!(["-d", "lib.so"]), "-d"),
        (json!(["-d"]), "-d"),
        (json!(["--file-prefix", "x"]), "--file-prefix"),
        (json!(["--file-prefix=x"]), "--file-prefix"),
        (json!(["-l", "0-3"]), "-l"),
        (json!(["-c", "0xf"]), "-c"),
        (json!(["--vdev", "net_null0"]), "--vdev"),
        (json!(["-a", "0000:00:00.0"]), "-a"),
        (json!(["--proc-type", "secondary"]), "--proc-type"),
        (json!(["--in-memory"]), "--in-memory"),
        (json!(["--main-lcore", "3"]), "--main-lcore"),
        (json!(["--huge-dir", "/x"]), "--huge-dir"),
        (
            json!(["--definitely-not-an-eal-option"]),
            "--definitely-not-an-eal-option",
        ),
        (json!(["foo"]), "foo"),
        (json!([""]), ""),
        (json!(["--no-huge", "extra"]), "extra"),
        (json!(["-m"]), "-m"),
        (json!(["-m", "abc"]), "abc"),
        (json!(["-m", "-1"]), "-1"),
        (json!(["-m", "256", "-d", "evil.so"]), "-d"),
        (json!(["-n", "0x"]), "0x"),
        (json!(["--socket-mem=1024;rm"]), "--socket-mem"),
        (json!(["--log-level=lib.eal:debug --x"]), "--log-level"),
        (json!(["--log-level=-d"]), "--log-level"),
    ] {
        let a = app();
        let r = a.start(&with_eal(args.clone())).await;
        assert_eq!(r.status, 400, "{args}: {}", r.text());
        assert!(r.message().contains(named), "{args}: {}", r.text());
        assert!(a.supervisor.plans().is_empty(), "{args} was spawned");
        let r = a.validate(&with_eal(args.clone())).await;
        assert_eq!(r.status, 400, "validate {args}: {}", r.text());
    }
}

#[tokio::test]
async fn a_token_with_whitespace_is_not_an_exact_token() {
    for args in [
        json!(["-m 256"]),
        json!(["--no-huge --no-pci"]),
        json!(["-d lib.so"]),
        json!(["--no-huge\n-d"]),
    ] {
        let a = app();
        let r = a.start(&with_eal(args.clone())).await;
        assert_eq!(r.status, 400, "{args}: {}", r.text());
        r.message();
    }
}

// ---- mlx5_devargs (gone) ------------------------------------------------------------------

#[tokio::test]
async fn any_settings_with_mlx5_devargs_are_a_400_naming_the_field_whatever_the_value() {
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
        let mut s = one("l", "net_null0", "net_null0");
        s.settings["mlx5_devargs"] = d.clone();
        let r = a.start(&s).await;
        assert_eq!(r.status, 400, "{d}: {}", r.text());
        assert!(r.message().contains("mlx5_devargs"), "{d}: {}", r.text());
        assert!(a.supervisor.plans().is_empty(), "{d} was spawned");
    }
}

// ---- buffer_capacity ---------------------------------------------------------

#[tokio::test]
async fn buffer_capacity_up_to_two_to_the_31_is_valid_above_it_is_a_400() {
    let a = app();
    let mut l = link("big", "net_null0", "net_null0", None);
    l["config"]["buffer_capacity"] = json!(2_147_483_648u64);
    assert_eq!(a.validate(&setup(vec![l])).await.status, 200);
    for over in [2_147_483_649u64, 4_294_967_296, u64::MAX] {
        let mut l = link("bigger", "net_null1", "net_null1", None);
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
