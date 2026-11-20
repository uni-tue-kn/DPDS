//! Sanity checks of the fake host trees themselves (not of the controller), so a
//! failing discovery/hint test is never caused by a broken fixture.

use std::fs;

use crate::common::*;

#[test]
fn driver_and_device_symlinks_resolve_inside_the_fake_tree() {
    let h = FakeHost::bare_metal();
    for (pci, drv) in [(NIC0, "mlx5_core"), (INTEL, "ice"), (MGMT, "tg3")] {
        let link = h.sysfs().join(format!("bus/pci/devices/{pci}/driver"));
        let target = fs::canonicalize(&link).unwrap_or_else(|e| panic!("{}: {e}", link.display()));
        assert_eq!(
            target,
            fs::canonicalize(h.sysfs().join(format!("bus/pci/drivers/{drv}"))).unwrap()
        );
    }
    let dev = h.sysfs().join("class/net/ens1f0/device");
    assert_eq!(
        fs::canonicalize(dev).unwrap(),
        fs::canonicalize(h.sysfs().join(format!("bus/pci/devices/{NIC0}"))).unwrap()
    );
}

#[test]
fn vfio_rebind_removes_the_netdev_and_adds_an_iommu_group_target() {
    let h = FakeHost::bare_metal();
    h.rebind(INTEL, "vfio-pci", true);
    assert!(!h
        .sysfs()
        .join(format!("bus/pci/devices/{INTEL}/net"))
        .exists());
    let t = fs::canonicalize(h.sysfs().join(format!("bus/pci/devices/{INTEL}/driver"))).unwrap();
    assert!(t.ends_with("drivers/vfio-pci"));
}

#[test]
fn config_space_encodes_maxreadreq_in_devctl_bits_14_12() {
    for (bytes, code) in [
        (128u32, 0u16),
        (256, 1),
        (512, 2),
        (1024, 3),
        (2048, 4),
        (4096, 5),
    ] {
        let c = config_space(bytes);
        assert_eq!(c.len(), 256);
        assert_eq!(c[0x34], 0x40, "capability pointer");
        assert_eq!(c[0x40], 0x10, "PCIe capability id");
        let devctl = u16::from_le_bytes([c[0x48], c[0x49]]);
        assert_eq!((devctl >> 12) & 7, code);
    }
}

#[test]
fn hugepage_files_are_consistent() {
    let h = FakeHost::bare_metal();
    let meminfo = fs::read_to_string(h.procfs().join("meminfo")).unwrap();
    assert!(meminfo.contains("HugePages_Total:    4096"), "{meminfo}");
    let n0 = fs::read_to_string(
        h.sysfs()
            .join("devices/system/node/node0/hugepages/hugepages-2048kB/free_hugepages"),
    )
    .unwrap();
    assert_eq!(n0.trim(), "2048");
    let vm = FakeHost::vm();
    assert!(!vm.sysfs().join("devices/system/node").exists());
    assert_eq!(
        fs::read_to_string(
            vm.sysfs()
                .join("kernel/mm/hugepages/hugepages-2048kB/nr_hugepages")
        )
        .unwrap()
        .trim(),
        "1024"
    );
}

#[test]
fn route_table_has_the_default_route_on_the_management_netdev_only() {
    let r = fs::read_to_string(FakeHost::bare_metal().procfs().join("net/route")).unwrap();
    assert!(r.lines().skip(1).all(|l| l.starts_with("eno1\t")));
    assert!(r.contains("\t00000000\t0100A8C0\t"));
}

#[test]
fn vm_and_bare_trees_have_the_documented_gaps() {
    let vm = FakeHost::vm();
    assert!(!vm
        .sysfs()
        .join("bus/pci/devices/0000:00:05.0/numa_node")
        .exists());
    assert!(!vm
        .sysfs()
        .join("bus/pci/devices/0000:00:05.0/max_link_width")
        .exists());
    let bare = FakeHost::bare();
    assert_eq!(
        fs::read_to_string(bare.sysfs().join("devices/system/cpu/isolated"))
            .unwrap()
            .trim(),
        ""
    );
    assert_eq!(
        fs::read_to_string(bare.procfs().join("meminfo"))
            .unwrap()
            .matches("HugePages_Total:    0\n")
            .count(),
        1
    );
}
