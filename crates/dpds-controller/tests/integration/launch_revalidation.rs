//! `POST /api/emulation/start` validates the body it carries (ports, port reuse, link names): the
//! start is refused (400, or 409) with a `{"message"}` that names the offending value; nothing is
//! spawned and the emulation stays stopped. `POST /api/emulation/validate` answers the same errors.

use crate::common::*;

fn app() -> TestApp {
    TestApp::new(FakeHost::bare_metal())
}

async fn assert_refused(a: &TestApp, s: &Setup, named: &str) {
    for r in [a.start(s).await, a.validate(s).await] {
        assert!(
            r.status == 400 || r.status == 409,
            "must be refused, got {}: {}",
            r.status,
            r.text()
        );
        let m = r.message();
        assert!(m.contains(named), "the message names {named:?}: {m}");
    }
    assert!(a.supervisor.plans().is_empty(), "nothing may be spawned");
    assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
}

#[tokio::test]
async fn a_valid_body_starts() {
    // control: the refusals below are about the values, not about this setup
    let a = app();
    let r = a
        .start_links(vec![link("ok", "net_null0", "net_null1")])
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(a.supervisor.plans().len(), 1);
}

#[tokio::test]
async fn ports_that_the_rules_forbid_refuse_the_start() {
    for bad in [
        "net_pcap0,rx_pcap=/tmp/in.pcap,tx_pcap=/tmp/out.pcap",
        "net_pcap0,tx_pcap=/etc/cron.d/x",
        "net_pcap0",
        "net_null0,size=64",
        "net_ring0,iface=eth0",
        "--vdev=net_null1",
        "net_null0 --vdev=net_null1",
        "0000:65:00.9",
        "",
    ] {
        // the bad name as the ingress and as the egress of a link: both are validated
        for as_ingress in [true, false] {
            let l = if as_ingress {
                link("old", bad, "net_null1")
            } else {
                link("old", "net_null1", bad)
            };
            let a = app();
            for r in [
                a.start_links(vec![l.clone()]).await,
                a.validate(&setup(vec![l])).await,
            ] {
                assert!(
                    r.status == 400 || r.status == 409,
                    "{bad:?} (ingress {as_ingress}): {} {}",
                    r.status,
                    r.text()
                );
                let m = r.message();
                assert!(m.contains(bad.trim()) || m.contains("port"), "{bad:?}: {m}");
            }
            assert!(a.supervisor.plans().is_empty(), "{bad:?} was spawned");
            assert_eq!(a.get("/api/emulation").await.json()["state"], "stopped");
        }
    }
}

#[tokio::test]
async fn a_bad_egress_or_a_bad_second_link_refuses_the_start() {
    let a = app();
    assert_refused(
        &a,
        &setup(vec![
            link("fine", "net_null0", "net_null0"),
            link("mixed", "net_null1", "net_pcap2,iface=eth0"),
        ]),
        "net_pcap2",
    )
    .await;
    assert_refused(
        &a,
        &setup(vec![
            link("fine", "net_null0", "net_null0"),
            link("bad-in", "net_pcap3,iface=eth0", "net_null1"),
        ]),
        "net_pcap3",
    )
    .await;
}

#[tokio::test]
async fn reusing_a_port_as_ingress_or_egress_refuses_the_start_naming_a_link() {
    for links in [
        vec![
            link("first", "net_null0", "net_null1"),
            link("second", "net_null0", "net_null2"),
        ],
        vec![
            link("first", "net_null0", "net_null1"),
            link("second", "net_null2", "net_null1"),
        ],
    ] {
        let a = app();
        let r = a.start_links(links).await;
        assert_eq!(r.status, 409, "{}", r.text());
        let m = r.message();
        assert!(m.contains("first"), "names the other link: {m}");
        assert!(a.supervisor.plans().is_empty(), "nothing may be spawned");
    }
}

#[tokio::test]
async fn a_refusal_does_not_block_a_later_start_with_a_fixed_body() {
    let a = app();
    let bad = setup(vec![link("old", "net_null0", "net_pcap0,iface=eth0")]);
    assert!(a.start(&bad).await.status >= 400);
    let r = a
        .start_links(vec![link("old", "net_null0", "net_null0")])
        .await;
    assert_eq!(r.status, 202, "{}", r.text());
    assert_eq!(a.supervisor.plans().len(), 1);
}

#[tokio::test]
async fn a_refused_start_keeps_a_running_emulation_and_its_configuration() {
    let a = app();
    a.start_ok(vec![link("run", "net_null0", "net_null0")])
        .await;
    let before = a.get("/api/emulation").await.json();
    let r = a
        .start_links(vec![link("x", "net_pcap0", "net_null1")])
        .await;
    assert!(r.status == 400 || r.status == 409, "{}", r.text());
    assert_eq!(a.get("/api/emulation").await.json(), before);
    assert_eq!(a.supervisor.plans().len(), 1);
}
