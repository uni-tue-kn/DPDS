//! C3 input validation and the `buffer_capacity` cap of C5, all through the REST API
//! (a rejected value is a 400 `{"message"}` naming the value, and nothing is stored).
//!
//! - port names (spec v7 S1): a PCI address `^[0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-7]$`
//!   (stored in lower case) or exactly a vdev `^net_(null|ring)[0-9]+$`: no `,key=value`
//!   devargs at all and no `net_pcap` (it needs `rx_pcap`/`tx_pcap`/`iface` devargs, which
//!   write files as root or bypass the management-NIC guard); nothing with whitespace, a
//!   leading `-`, or a smuggled option;
//! - `settings.eal_extra_args` is an allowlist of tokens: `--no-huge`, `--no-pci`,
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

fn settings(eal: Value) -> Value {
    json!({"main_lcore": null, "eal_extra_args": eal, "poll_interval_ms": 500})
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
        let a = app();
        // as a hairpin (ingress and egress) and as the egress of a link from a vdev
        let r = a.create_link(&format!("l{i}"), port, port).await;
        assert_eq!(r.status, 201, "{port}: {}", r.text());
        let a = app();
        let r = a.create_link(&format!("m{i}"), "net_null9", port).await;
        assert_eq!(r.status, 201, "{port} as egress: {}", r.text());
    }
}

#[tokio::test]
async fn pci_addresses_are_stored_in_lower_case_and_conflict_across_case() {
    let a = app();
    let r = a.create_link("up", "0000:B3:00.0", "0000:B3:00.1").await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["ingress"], "0000:b3:00.0");
    assert_eq!(r.json()["egress"], "0000:b3:00.1");
    // the same ingress in another case is the same port: 409 naming the first link
    let r = a.create_link("again", "0000:b3:00.0", "0000:b3:00.2").await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert!(r.message().contains("up"), "{}", r.text());
    let r = a
        .create_link("again2", "0000:b3:00.3", "0000:B3:00.1")
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
    // spec v7 S1: no devargs at all, no net_pcap
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
async fn invalid_port_names_are_a_400_naming_the_value_on_create() {
    // control: valid ports in the same body shape are accepted
    assert_eq!(
        app()
            .create_link("ok", "net_null0", "net_null1")
            .await
            .status,
        201
    );
    for bad in BAD_PORTS {
        let a = app();
        for as_ingress in [true, false] {
            let r = if as_ingress {
                a.create_link("x", bad, "net_null1").await
            } else {
                a.create_link("x", "net_null1", bad).await
            };
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
        }
        assert_eq!(
            a.get("/api/links").await.json(),
            json!([]),
            "{bad:?} stored"
        );
    }
}

#[tokio::test]
async fn invalid_port_names_are_a_400_on_update_as_ingress_and_as_egress() {
    let a = app();
    let id = a.create_link("x", "net_null0", "net_null0").await.json()["id"]
        .as_u64()
        .unwrap();
    for bad in [
        "--x",
        "net_null0 --vdev=net_null1",
        "0000:65:00.9",
        "net_pcap0,tx_pcap=/etc/x",
        "net_null0,size=64",
    ] {
        let r = a
            .put(
                &format!("/api/links/{id}"),
                link_body("x", bad, "net_null0"),
            )
            .await;
        assert_eq!(r.status, 400, "ingress {bad:?}");
        r.message();
        let r = a
            .put(
                &format!("/api/links/{id}"),
                link_body("x", "net_null0", bad),
            )
            .await;
        assert_eq!(r.status, 400, "egress {bad:?}");
        r.message();
        let r = a.create_link("two", "net_null1", bad).await;
        assert_eq!(r.status, 400, "egress of a new link {bad:?}");
    }
    // the stored link is unchanged
    let v = a.get(&format!("/api/links/{id}")).await.json();
    assert_eq!(v["ingress"], "net_null0");
    assert_eq!(v["egress"], "net_null0");
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
        let r = a.put("/api/settings", settings(args.clone())).await;
        assert_eq!(r.status, 200, "{what}: {}", r.text());
        assert_eq!(r.json()["eal_extra_args"], args, "{what}");
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
        let r = a.put("/api/settings", settings(args.clone())).await;
        assert_eq!(r.status, 400, "{args}: {}", r.text());
        assert!(r.message().contains(named), "{args}: {}", r.text());
        assert_eq!(
            a.get("/api/settings").await.json()["eal_extra_args"],
            json!([]),
            "{args} must not be stored"
        );
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
        let r = a.put("/api/settings", settings(args.clone())).await;
        assert_eq!(r.status, 400, "{args}: {}", r.text());
        r.message();
    }
}

// ---- mlx5_devargs (gone, spec v17) ----------------------------------------------------

#[tokio::test]
async fn any_body_with_mlx5_devargs_is_a_400_naming_the_field_whatever_the_value() {
    for d in [
        "",
        "txq_mpw_en=1",
        "txq_mpw_en=1,txq_inline_max=0",
        "rxq_cqe_comp_en=1,rx_vec_en=1",
        "tx_pp=500",
        "mprq_en=1",
        "bogus_key=1",
        "txq_mpw_en=1,",
    ] {
        let a = app();
        let mut body = settings(json!([]));
        body["mlx5_devargs"] = json!(d);
        let r = a.put("/api/settings", body).await;
        assert_eq!(r.status, 400, "{d:?}: {}", r.text());
        assert!(r.message().contains("mlx5_devargs"), "{d:?}: {}", r.text());
        assert!(
            absent(&a.get("/api/settings").await.json(), "mlx5_devargs"),
            "{d:?} must not be stored"
        );
    }
    // not only strings: a null or a number is the same unknown field
    for v in [json!(null), json!(1), json!(["txq_mpw_en=1"])] {
        let a = app();
        let mut body = settings(json!([]));
        body["mlx5_devargs"] = v.clone();
        let r = a.put("/api/settings", body).await;
        assert_eq!(r.status, 400, "{v}: {}", r.text());
    }
}

// ---- buffer_capacity ---------------------------------------------------------

#[tokio::test]
async fn buffer_capacity_up_to_two_to_the_31_is_valid_above_it_is_a_400() {
    let a = app();
    let mut body = link_body("big", "net_null0", "net_null0");
    body["config"]["buffer_capacity"] = json!(2_147_483_648u64);
    assert_eq!(a.post("/api/links", body).await.status, 201);
    for over in [2_147_483_649u64, 4_294_967_296, u64::MAX] {
        let mut body = link_body("bigger", "net_null1", "net_null1");
        body["config"]["buffer_capacity"] = json!(over);
        let r = a.post("/api/links", body).await;
        assert_eq!(r.status, 400, "{over}: {}", r.text());
        assert!(r.message().contains("buffer_capacity"), "{over}");
    }
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
