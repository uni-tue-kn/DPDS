//! Lcore assignment (B7) on fake sysfs trees.

use crate::common::*;
use dpds_controller::lcores::assign_lcores;

/// bare_metal: online 0-7, isolated {2,3} (node 0) and {6,7} (node 1).
const NODE0: [u32; 2] = [2, 3];
const NODE1: [u32; 2] = [6, 7];

fn ok(
    host: &FakeHost,
    st: &dpds_controller::model::StoredState,
) -> dpds_controller::lcores::Assignment {
    let a = assign_lcores(&host.roots(), st).unwrap_or_else(|e| panic!("assignment failed: {e}"));
    a
}

#[test]
fn single_port_link_prefers_the_nic_numa_node() {
    let h = FakeHost::bare_metal();
    let a = ok(&h, &state_with(vec![link(1, "l0", &[NIC0], None)]));
    assert_eq!(a.main_lcore, 0);
    assert_eq!(a.directions.len(), 1);
    let d = &a.directions[0];
    assert_eq!(
        (d.dir, d.link.as_str(), d.rx.as_str(), d.tx.as_str()),
        (0, "l0", NIC0, NIC0)
    );
    assert!(NODE0.contains(&d.lcore), "{d:?}");

    let a = ok(&h, &state_with(vec![link(1, "l1", &[NIC1], None)]));
    assert!(NODE1.contains(&a.directions[0].lcore), "{:?}", a.directions);
}

#[test]
fn two_port_link_gives_one_direction_per_rx_port_on_that_ports_node() {
    let h = FakeHost::bare_metal();
    let a = ok(&h, &state_with(vec![link(1, "a-b", &[NIC0, NIC1], None)]));
    assert_eq!(a.directions.len(), 2);
    let (d0, d1) = (&a.directions[0], &a.directions[1]);
    assert_eq!((d0.dir, d0.rx.as_str(), d0.tx.as_str()), (0, NIC0, NIC1));
    assert_eq!((d1.dir, d1.rx.as_str(), d1.tx.as_str()), (1, NIC1, NIC0));
    assert_eq!(d0.link, "a-b");
    assert_eq!(d1.link, "a-b");
    assert!(NODE0.contains(&d0.lcore), "{d0:?}");
    assert!(NODE1.contains(&d1.lcore), "{d1:?}");
}

#[test]
fn reversed_numa_order_is_honoured_literally_not_re_sorted() {
    // dir0 receives on NIC1 (node 1), dir1 on NIC0 (node 0): dir0's lcore is the LARGER one
    let h = FakeHost::bare_metal();
    let a = ok(&h, &state_with(vec![link(1, "b-a", &[NIC1, NIC0], None)]));
    assert_eq!(a.directions.len(), 2);
    assert!(NODE1.contains(&a.directions[0].lcore), "{:?}", a.directions);
    assert!(NODE0.contains(&a.directions[1].lcore), "{:?}", a.directions);
    assert!(a.directions[0].lcore > a.directions[1].lcore);
}

#[test]
fn directions_are_numbered_globally_in_link_order() {
    let h = FakeHost::bare_metal();
    let st = state_with(vec![
        link(1, "a-b", &[NIC0, NIC1], None),
        link(2, "loop", &["net_null0"], None),
    ]);
    let a = ok(&h, &st);
    let names: Vec<(u32, &str)> = a
        .directions
        .iter()
        .map(|d| (d.dir, d.link.as_str()))
        .collect();
    assert_eq!(names, [(0, "a-b"), (1, "a-b"), (2, "loop")]);
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
fn no_links_is_ok_with_a_main_lcore_and_no_directions() {
    let a = ok(&FakeHost::bare_metal(), &state_with(vec![]));
    assert_eq!(a.main_lcore, 0);
    assert!(a.directions.is_empty());
}

#[test]
fn main_lcore_default_is_the_lowest_non_isolated_online_core() {
    let h = FakeHost::bare_metal();
    h.write("sys/devices/system/cpu/isolated", "0-3\n");
    h.cmdline("ro isolcpus=0-3 nohz_full=0-3 rcu_nocbs=0-3");
    let a = ok(&h, &state_with(vec![link(1, "l", &[NIC0], None)]));
    assert_eq!(a.main_lcore, 4);
}

#[test]
fn main_lcore_setting_overrides_and_is_not_used_for_a_worker() {
    let h = FakeHost::bare_metal();
    let a = ok(&h, &state_with_main(vec![link(1, "l", &[NIC0], None)], 1));
    assert_eq!(a.main_lcore, 1);
    // main on isolated core 2: the only other node-0 isolated core is left for the worker
    let a = ok(&h, &state_with_main(vec![link(1, "l", &[NIC0], None)], 2));
    assert_eq!(a.main_lcore, 2);
    assert_eq!(a.directions[0].lcore, 3);
}

#[test]
fn cmdline_is_the_fallback_when_the_isolated_file_is_missing() {
    let h = FakeHost::bare_metal();
    h.remove("sys/devices/system/cpu/isolated");
    let a = ok(&h, &state_with(vec![link(1, "l", &[NIC1], None)]));
    assert_eq!(a.main_lcore, 0);
    assert!(NODE1.contains(&a.directions[0].lcore), "{:?}", a.directions);
}

#[test]
fn per_link_override_is_used() {
    let h = FakeHost::bare_metal();
    let a = ok(&h, &state_with(vec![link(1, "l", &[NIC0], Some(vec![3]))]));
    assert_eq!(a.directions[0].lcore, 3);
    let a = ok(
        &h,
        &state_with(vec![link(1, "ab", &[NIC0, NIC1], Some(vec![2, 7]))]),
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
        &state_with(vec![link(1, "l", &[NIC0], Some(vec![4]))]),
    );
    assert_eq!(a.directions[0].lcore, 4);
}

#[test]
fn override_is_mixed_with_auto_assignment_without_reusing_cores() {
    let h = FakeHost::bare_metal();
    let st = state_with(vec![
        link(1, "pinned", &[NIC1], Some(vec![6])),
        link(2, "auto", &[NIC1], None),
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
            "wrong length",
            state_with(vec![link(1, "l", &[NIC0, NIC1], Some(vec![2]))]),
        ),
        (
            "duplicate in link",
            state_with(vec![link(1, "l", &[NIC0, NIC1], Some(vec![2, 2]))]),
        ),
        (
            "same core in two links",
            state_with(vec![
                link(1, "a", &[NIC0], Some(vec![2])),
                link(2, "b", &[NIC1], Some(vec![2])),
            ]),
        ),
        (
            "equals main lcore",
            state_with_main(vec![link(1, "l", &[NIC0], Some(vec![2]))], 2),
        ),
        (
            "core does not exist",
            state_with(vec![link(1, "l", &[NIC0], Some(vec![99]))]),
        ),
    ];
    for (what, st) in bad {
        let r = assign_lcores(&h.roots(), &st);
        match r {
            Ok(a) => panic!("{what}: expected an error, got {:?}", a.directions),
            Err(e) => assert!(!e.to_string().is_empty(), "{what}"),
        }
    }
}

#[test]
fn override_in_descending_order_is_honoured_literally() {
    let h = FakeHost::bare_metal();
    let a = ok(
        &h,
        &state_with(vec![link(1, "ab", &[NIC0, NIC1], Some(vec![7, 2]))]),
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
        .map(|i| link(i + 1, &format!("l{i}"), &[&format!("net_null{i}")], None))
        .collect();
    let err = assign_lcores(&h.roots(), &state_with(links))
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
        .map(|i| link(i + 1, &format!("l{i}"), &[&format!("net_null{i}")], None))
        .collect();
    let a = ok(&h, &state_with(links));
    let mut l: Vec<u32> = a.directions.iter().map(|d| d.lcore).collect();
    l.sort_unstable();
    assert_eq!(l, [2, 3, 6, 7]);
}

#[test]
fn host_without_isolation_fails_unless_overridden() {
    let h = FakeHost::bare();
    let err = assign_lcores(&h.roots(), &state_with(vec![link(1, "l", &[NIC0], None)]))
        .expect_err("no isolated cores")
        .to_string();
    assert!(err.contains("isolated"), "{err}");
    let a = ok(&h, &state_with(vec![link(1, "l", &[NIC0], Some(vec![2]))]));
    assert_eq!(a.directions[0].lcore, 2);
    assert_eq!(a.main_lcore, 0);
}

#[test]
fn preferred_node_exhausted_spills_to_the_other_node() {
    let h = FakeHost::bare_metal();
    // NIC1 is used by three links here only to test the spill (the API would reject
    // reuse of a port; the assignment itself must not care)
    let links: Vec<_> = (0..3)
        .map(|i| link(i + 1, &format!("l{i}"), &[NIC1], None))
        .collect();
    let a = ok(&h, &state_with(links));
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
    let a = ok(&h, &state_with(vec![link(1, "l", &["0000:00:05.0"], None)]));
    assert_eq!(a.main_lcore, 0);
    assert!([2, 3].contains(&a.directions[0].lcore));
}
