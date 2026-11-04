//! Lcore assignment (B7) on fake sysfs trees. Spec 22: the plan of a start body is read from
//! `POST /api/emulation/validate` (`plan_for`), the same plan `POST /api/emulation/start`
//! launches; the controller has no stored links to plan.

use crate::common::*;

/// bare_metal: online 0-7, isolated {2,3} (node 0) and {6,7} (node 1).
const NODE0: [u32; 2] = [2, 3];
const NODE1: [u32; 2] = [6, 7];

fn ok(host: &FakeHost, st: &Setup) -> Assignment {
    plan_for(host, st).unwrap_or_else(|e| panic!("assignment failed: {e}"))
}

#[test]
fn hairpin_link_prefers_the_nic_numa_node() {
    let h = FakeHost::bare_metal();
    let a = ok(&h, &setup(vec![link("l0", NIC0, NIC0, None)]));
    assert_eq!(a.main_lcore, 0);
    assert_eq!(a.directions.len(), 1);
    let d = &a.directions[0];
    assert_eq!(
        (d.dir, d.link.as_str(), d.rx.as_str(), d.tx.as_str()),
        (0, "l0", NIC0, NIC0)
    );
    assert!(NODE0.contains(&d.lcore), "{d:?}");

    let a = ok(&h, &setup(vec![link("l1", NIC1, NIC1, None)]));
    assert!(NODE1.contains(&a.directions[0].lcore), "{:?}", a.directions);
}

#[test]
fn one_direction_per_link_rx_is_ingress_tx_is_egress() {
    let h = FakeHost::bare_metal();
    let a = ok(&h, &setup(vec![link("a-to-b", NIC0, NIC1, None)]));
    assert_eq!(a.directions.len(), 1);
    let d = &a.directions[0];
    assert_eq!((d.dir, d.rx.as_str(), d.tx.as_str()), (0, NIC0, NIC1));
    assert_eq!(d.link, "a-to-b");
    assert!(NODE0.contains(&d.lcore), "{d:?}");
}

#[test]
fn the_lcore_follows_the_numa_node_of_the_ingress_nic_not_of_the_egress() {
    let h = FakeHost::bare_metal();
    // ingress NIC0 (node 0), egress NIC1 (node 1): node 0
    let a = ok(&h, &setup(vec![link("x", NIC0, NIC1, None)]));
    assert!(NODE0.contains(&a.directions[0].lcore), "{:?}", a.directions);
    // ingress NIC1 (node 1), egress NIC0 (node 0): node 1
    let a = ok(&h, &setup(vec![link("y", NIC1, NIC0, None)]));
    assert!(NODE1.contains(&a.directions[0].lcore), "{:?}", a.directions);
    // a vdev as ingress has no node; an egress on node 1 must not drag the pick there
    // (lowest free isolated core wins)
    let a = ok(&h, &setup(vec![link("v", "net_null0", NIC1, None)]));
    assert_eq!(a.directions[0].lcore, 2, "{:?}", a.directions);
}

#[test]
fn bidirectional_pair_is_two_links_each_on_its_own_ingress_node() {
    let h = FakeHost::bare_metal();
    let a = ok(
        &h,
        &setup(vec![
            link("a-to-b", NIC0, NIC1, None),
            link("b-to-a", NIC1, NIC0, None),
        ]),
    );
    assert_eq!(a.directions.len(), 2);
    let (d0, d1) = (&a.directions[0], &a.directions[1]);
    assert_eq!((d0.dir, d0.rx.as_str(), d0.tx.as_str()), (0, NIC0, NIC1));
    assert_eq!((d1.dir, d1.rx.as_str(), d1.tx.as_str()), (1, NIC1, NIC0));
    assert_eq!((d0.link.as_str(), d1.link.as_str()), ("a-to-b", "b-to-a"));
    assert!(NODE0.contains(&d0.lcore), "{d0:?}");
    assert!(NODE1.contains(&d1.lcore), "{d1:?}");
}

#[test]
fn three_link_ring_gets_numa_local_lcores_by_ingress_nic() {
    // A(NIC0, node 0) -> B(NIC1, node 1) -> C(INTEL, node 1) -> A
    let h = FakeHost::bare_metal();
    let a = ok(
        &h,
        &setup(vec![
            link("a-to-b", NIC0, NIC1, None),
            link("b-to-c", NIC1, INTEL, None),
            link("c-to-a", INTEL, NIC0, None),
        ]),
    );
    let d = &a.directions;
    assert_eq!(d.len(), 3);
    let plan: Vec<(u32, &str, &str, &str)> = d
        .iter()
        .map(|x| (x.dir, x.link.as_str(), x.rx.as_str(), x.tx.as_str()))
        .collect();
    assert_eq!(
        plan,
        [
            (0, "a-to-b", NIC0, NIC1),
            (1, "b-to-c", NIC1, INTEL),
            (2, "c-to-a", INTEL, NIC0)
        ]
    );
    assert!(NODE0.contains(&d[0].lcore), "{d:?}");
    assert!(NODE1.contains(&d[1].lcore), "{d:?}");
    assert!(NODE1.contains(&d[2].lcore), "{d:?}");
    let mut l: Vec<u32> = d.iter().map(|x| x.lcore).collect();
    l.sort_unstable();
    l.dedup();
    assert_eq!(l.len(), 3, "worker lcores must be distinct: {d:?}");
}

#[test]
fn reversed_numa_order_is_honoured_literally_not_re_sorted() {
    // link 0 receives on NIC1 (node 1), link 1 on NIC0 (node 0): link 0's lcore is the LARGER one
    let h = FakeHost::bare_metal();
    let st = setup(vec![
        link("b-to-a", NIC1, NIC0, None),
        link("a-to-b", NIC0, NIC1, None),
    ]);
    let a = ok(&h, &st);
    assert_eq!(a.directions.len(), 2);
    assert!(NODE1.contains(&a.directions[0].lcore), "{:?}", a.directions);
    assert!(NODE0.contains(&a.directions[1].lcore), "{:?}", a.directions);
    assert!(a.directions[0].lcore > a.directions[1].lcore);
}

#[test]
fn directions_are_numbered_in_link_order() {
    let h = FakeHost::bare_metal();
    let st = setup(vec![
        link("a-to-b", NIC0, NIC1, None),
        link("b-to-a", NIC1, NIC0, None),
        link("loop", "net_null0", "net_null0", None),
    ]);
    let a = ok(&h, &st);
    let names: Vec<(u32, &str)> = a
        .directions
        .iter()
        .map(|d| (d.dir, d.link.as_str()))
        .collect();
    assert_eq!(names, [(0, "a-to-b"), (1, "b-to-a"), (2, "loop")]);
    assert_eq!(a.directions[2].rx, "net_null0");
    assert_eq!(a.directions[2].tx, "net_null0");
    let mut l: Vec<u32> = a.directions.iter().map(|d| d.lcore).collect();
    l.sort_unstable();
    l.dedup();
    assert_eq!(l.len(), 3);
    for x in l {
        assert!([2, 3, 6, 7].contains(&x), "worker {x} is not isolated");
    }
    assert!(NODE0.contains(&a.directions[0].lcore), "{:?}", a.directions);
    assert!(NODE1.contains(&a.directions[1].lcore), "{:?}", a.directions);
}

#[test]
fn no_links_is_nothing_to_start_not_a_plan() {
    // the controller keeps no links: an empty body has nothing to plan
    let e = plan_for(&FakeHost::bare_metal(), &setup(vec![])).expect_err("no links");
    assert!(e.to_lowercase().contains("nothing to start"), "{e}");
}

#[test]
fn main_lcore_default_is_the_lowest_non_isolated_online_core() {
    let h = FakeHost::bare_metal();
    h.write("sys/devices/system/cpu/isolated", "0-3\n");
    h.cmdline("ro isolcpus=0-3 nohz_full=0-3 rcu_nocbs=0-3");
    let a = ok(&h, &setup(vec![link("l", NIC0, NIC0, None)]));
    assert_eq!(a.main_lcore, 4);
}

#[test]
fn main_lcore_setting_overrides_and_is_not_used_for_a_worker() {
    let h = FakeHost::bare_metal();
    let a = ok(&h, &setup_main(vec![link("l", NIC0, NIC0, None)], 1));
    assert_eq!(a.main_lcore, 1);
    // main on isolated core 2: the only other node-0 isolated core is left for the worker
    let a = ok(&h, &setup_main(vec![link("l", NIC0, NIC0, None)], 2));
    assert_eq!(a.main_lcore, 2);
    assert_eq!(a.directions[0].lcore, 3);
}

#[test]
fn cmdline_is_the_fallback_when_the_isolated_file_is_missing() {
    let h = FakeHost::bare_metal();
    h.remove("sys/devices/system/cpu/isolated");
    let a = ok(&h, &setup(vec![link("l", NIC1, NIC1, None)]));
    assert_eq!(a.main_lcore, 0);
    assert!(NODE1.contains(&a.directions[0].lcore), "{:?}", a.directions);
}

#[test]
fn per_link_override_is_used() {
    let h = FakeHost::bare_metal();
    let a = ok(&h, &setup(vec![link("l", NIC0, NIC0, Some(3))]));
    assert_eq!(a.directions[0].lcore, 3);
    // an override on a link whose ingress sits on the other node is still used literally
    let a = ok(&h, &setup(vec![link("ab", NIC0, NIC1, Some(7))]));
    assert_eq!(a.directions[0].lcore, 7);
    let a = ok(
        &h,
        &setup(vec![
            link("ab", NIC0, NIC1, Some(2)),
            link("ba", NIC1, NIC0, Some(7)),
        ]),
    );
    assert_eq!(
        a.directions.iter().map(|d| d.lcore).collect::<Vec<_>>(),
        [2, 7]
    );
}

#[test]
fn override_may_name_a_non_isolated_core() {
    // the hints warn about it; the assignment itself honours the operator
    let a = ok(
        &FakeHost::bare_metal(),
        &setup(vec![link("l", NIC0, NIC0, Some(4))]),
    );
    assert_eq!(a.directions[0].lcore, 4);
}

#[test]
fn override_is_mixed_with_auto_assignment_without_reusing_cores() {
    let h = FakeHost::bare_metal();
    let st = setup(vec![
        link("pinned", NIC1, NIC1, Some(6)),
        link("auto", INTEL, NIC0, None),
    ]);
    let a = ok(&h, &st);
    assert_eq!(a.directions[0].lcore, 6);
    assert_eq!(a.directions[1].lcore, 7);
}

#[test]
fn invalid_overrides_are_errors() {
    let h = FakeHost::bare_metal();
    let bad = [
        (
            "same core in two links",
            setup(vec![
                link("a", NIC0, NIC0, Some(2)),
                link("b", NIC1, NIC1, Some(2)),
            ]),
        ),
        (
            "same core in the two links of a pair",
            setup(vec![
                link("a", NIC0, NIC1, Some(2)),
                link("b", NIC1, NIC0, Some(2)),
            ]),
        ),
        (
            "equals main lcore",
            setup_main(vec![link("l", NIC0, NIC0, Some(2))], 2),
        ),
        (
            "core does not exist",
            setup(vec![link("l", NIC0, NIC0, Some(99))]),
        ),
    ];
    for (what, st) in bad {
        let r = plan_for(&h, &st);
        match r {
            Ok(a) => panic!("{what}: expected an error, got {:?}", a.directions),
            Err(e) => assert!(!e.to_string().is_empty(), "{what}"),
        }
    }
}

#[test]
fn invalid_override_error_names_the_link() {
    let h = FakeHost::bare_metal();
    let err = plan_for(
        &h,
        &setup(vec![
            link("fine", NIC0, NIC0, Some(2)),
            link("broken", NIC1, NIC1, Some(99)),
        ]),
    )
    .expect_err("core 99 does not exist")
    .to_string();
    assert!(err.contains("broken"), "{err}");
}

#[test]
fn override_in_descending_order_is_honoured_literally() {
    let h = FakeHost::bare_metal();
    let a = ok(
        &h,
        &setup(vec![
            link("ab", NIC0, NIC1, Some(7)),
            link("ba", NIC1, NIC0, Some(2)),
        ]),
    );
    assert_eq!(
        a.directions.iter().map(|d| d.lcore).collect::<Vec<_>>(),
        [7, 2]
    );
    assert_eq!(
        (a.directions[0].rx.as_str(), a.directions[1].rx.as_str()),
        (NIC0, NIC1)
    );
}

#[test]
fn not_enough_isolated_cores_is_a_clear_error() {
    let h = FakeHost::bare_metal(); // 4 isolated cores
    let links: Vec<_> = (0..5)
        .map(|i| {
            let p = format!("net_null{i}");
            link(&format!("l{i}"), &p, &p, None)
        })
        .collect();
    let err = plan_for(&h, &setup(links))
        .expect_err("must fail")
        .to_string();
    assert!(err.contains("isolated"), "{err}");
    assert!(
        err.contains('5') && err.contains('4'),
        "message should name needed (5) and available (4): {err}"
    );
}

#[test]
fn exactly_enough_isolated_cores_works() {
    let h = FakeHost::bare_metal();
    let links: Vec<_> = (0..4)
        .map(|i| {
            let p = format!("net_null{i}");
            link(&format!("l{i}"), &p, &p, None)
        })
        .collect();
    let a = ok(&h, &setup(links));
    let mut l: Vec<u32> = a.directions.iter().map(|d| d.lcore).collect();
    l.sort_unstable();
    assert_eq!(l, [2, 3, 6, 7]);
}

#[test]
fn host_without_isolation_fails_unless_overridden() {
    let h = FakeHost::bare();
    let err = plan_for(&h, &setup(vec![link("l", NIC0, NIC0, None)]))
        .expect_err("no isolated cores")
        .to_string();
    assert!(err.contains("isolated"), "{err}");
    let a = ok(&h, &setup(vec![link("l", NIC0, NIC0, Some(2))]));
    assert_eq!(a.directions[0].lcore, 2);
    assert_eq!(a.main_lcore, 0);
}

#[test]
fn preferred_node_exhausted_spills_to_the_other_node() {
    let h = FakeHost::bare_metal();
    // a third NIC on node 1: three hairpins whose ingress NICs all sit on node 1
    let extra = "0000:b4:00.0";
    h.add_nic(&Nic::mlx5(extra, "enp180s0", Some(1)));
    let links: Vec<_> = [NIC1, INTEL, extra]
        .iter()
        .enumerate()
        .map(|(i, p)| link(&format!("l{i}"), p, p, None))
        .collect();
    let a = ok(&h, &setup(links));
    let l: Vec<u32> = a.directions.iter().map(|d| d.lcore).collect();
    assert!(
        NODE1.contains(&l[0]) && NODE1.contains(&l[1]) && l[0] != l[1],
        "{l:?}"
    );
    assert!(
        NODE0.contains(&l[2]),
        "third direction spills to node 0: {l:?}"
    );
}

#[test]
fn vm_without_numa_information_still_assigns_isolated_cores() {
    let h = FakeHost::vm();
    let a = ok(
        &h,
        &setup(vec![link("l", "0000:00:05.0", "0000:00:05.0", None)]),
    );
    assert_eq!(a.main_lcore, 0);
    assert!([2, 3].contains(&a.directions[0].lcore));
}
