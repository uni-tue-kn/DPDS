//! End-to-end characterisation of the safe wrappers on a real EAL with one
//! `net_null` vdev (no hugepages, no PCI, no root, no NIC).
//!
//! DPDK allows one EAL per process, so everything runs inside a single test
//! function (an integration-test file is its own process). The oracle for each
//! check is either DPDK itself (raw `dpdk_sys` calls) or a documented property
//! of `net_null`: its RX hands out freshly allocated 64-byte mbufs
//! (`packet_size` default 64, port id stamped in the mbuf) and its TX frees
//! what it is given and reports all of it as sent.
//!
//! The mempool is created with `cache_size = 0`, so `rte_mempool_in_use_count`
//! is exact (no per-lcore cache can hide mbufs).

use std::ptr::null_mut;

use dpdk::eal::{foreach_worker, lcore_count};
use dpdk::mbuf::{self, Packet};
use dpdk::port::{
    device_name, eth_stats_get, foreach_dev, rte_eth_dev_count_avail, rte_lcore_id,
    rte_pktmbuf_free, RawMbuf,
};
use dpdk::{now_ns, DpdkError, Eal, Mempool, Port, Ring};

const PKT_SIZE: u32 = 64; // net_null's default packet_size

fn eal_args() -> Vec<String> {
    [
        "eal_port_test",
        "--no-huge",
        "-m",
        "256",
        "--no-pci",
        "--in-memory",
        "--no-telemetry",
        "--file-prefix",
        "dpdk_crate_eal_port",
        "-l",
        "0",
        "--vdev",
        "net_null0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn in_use(m: RawMbuf) -> u32 {
    unsafe { dpdk_sys::rte_mempool_in_use_count((*m).pool) }
}

#[test]
fn net_null_end_to_end() {
    let args = eal_args();
    let _eal = Eal::new(&args).expect("EAL must initialise with --no-huge --vdev net_null0");

    // ---- EAL guard, lcores ------------------------------------------------
    assert_eq!(
        Eal::new(&args).err(),
        Some(DpdkError(-114)),
        "a second Eal in one process is -EALREADY"
    );
    assert_eq!(lcore_count(), 1, "-l 0: main lcore only");
    assert_eq!(foreach_worker().count(), 0, "no worker lcores");
    assert_eq!(rte_lcore_id(), 0);

    // ---- port enumeration ---------------------------------------------------
    assert_eq!(rte_eth_dev_count_avail(), 1);
    assert_eq!(foreach_dev().collect::<Vec<_>>(), vec![0]);
    assert_eq!(device_name(0).unwrap(), "net_null0");
    assert!(device_name(42).is_err(), "unknown port id");

    // ---- clock --------------------------------------------------------------
    let t0 = now_ns();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let t1 = now_ns();
    assert!(t0 > 0.0 && t0.is_finite());
    assert!(
        (19e6..2e9).contains(&(t1 - t0)),
        "20 ms sleep measured as {} ns",
        t1 - t0
    );

    // ---- mempool + port -----------------------------------------------------
    let pool =
        Mempool::create("eal_port_pool", 1023, 0, Mempool::DEFAULT_BUF_SIZE, 0).expect("pool");
    assert!(
        Mempool::create("eal_port_pool", 63, 0, Mempool::DEFAULT_BUF_SIZE, 0).is_err(),
        "pool names are unique"
    );
    assert!(
        Port::init(7, &pool, 256, 256, 1, 1500).is_err(),
        "invalid port id is an error (-ENODEV)"
    );
    assert!(
        Port::init(0, &pool, 256, 256, 0, 1500).is_err(),
        "zero queues is an error"
    );
    assert!(
        Port::init(0, &pool, 256, 256, 1, 10).is_err(),
        "MTU below the device minimum is an error"
    );
    let mut port = Port::init(0, &pool, 256, 256, 1, 1500).expect("port init");

    assert_eq!(port.id(), 0);
    assert_eq!(port.device_name(), "net_null0");
    assert_eq!(port.rx_ring_size_requested(), 256);
    assert_eq!(port.tx_ring_size_requested(), 256);
    assert!(port.rx_ring_size() > 0 && port.tx_ring_size() > 0);

    // mbuf_fast_free() must be exactly "device reports the capability".
    let mut info: dpdk_sys::rte_eth_dev_info = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { dpdk_sys::rte_eth_dev_info_get(0, &mut info) }, 0);
    assert_eq!(
        port.mbuf_fast_free(),
        info.tx_offload_capa & dpdk_sys::RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE != 0
    );
    assert!(port.link_up().is_ok());
    assert!(!port.link_speed().expect("link speed").is_empty());

    let zero = eth_stats_get(0).unwrap();
    assert_eq!((zero.ipackets, zero.opackets), (0, 0));
    assert!(eth_stats_get(42).is_err());

    // ---- RX: mbuf accessors ---------------------------------------------------
    let (mut rx, mut tx) = port.queues();
    assert_eq!((rx.len(), tx.len()), (1, 1), "one queue each");
    let mut bufs: [RawMbuf; 32] = [null_mut(); 32];

    let n = rx[0].rx_burst(&mut bufs);
    assert_eq!(n, 32, "net_null fills any requested burst");
    assert_eq!(in_use(bufs[0]), 32);
    assert_eq!(eth_stats_get(0).unwrap().ipackets, 32);
    for &m in &bufs {
        assert_eq!(unsafe { mbuf::pkt_len(m) }, PKT_SIZE);
        assert_eq!(unsafe { mbuf::port(m) }, 0, "net_null stamps its port id");
    }
    let packet = unsafe { Packet::from_raw(bufs[0]) };
    assert_eq!(packet.pkt_len(), PKT_SIZE);
    assert_eq!(packet.data().len(), PKT_SIZE as usize);
    mbuf::prefetch_header(bufs[0]);
    mbuf::prefetch_data(bufs[0]);

    // set_port round-trips (and only touches the port field).
    unsafe { mbuf::set_port(bufs[1], 7) };
    assert_eq!(unsafe { mbuf::port(bufs[1]) }, 7);
    assert_eq!(unsafe { mbuf::port(bufs[2]) }, 0);
    assert_eq!(unsafe { mbuf::pkt_len(bufs[1]) }, PKT_SIZE);

    // ---- TX: ownership passes to the driver, which frees ----------------------
    let sent = tx[0].tx_burst(&mut bufs[..20]);
    assert_eq!(sent, 20);
    assert_eq!(eth_stats_get(0).unwrap().opackets, 20);
    assert_eq!(in_use(bufs[20]), 12, "20 of the 32 were freed by TX");

    // ---- freeing the other 12: single free for 4, bulk free for 8 -------------
    for &m in &bufs[20..24] {
        unsafe { rte_pktmbuf_free(m) };
    }
    assert_eq!(in_use(bufs[24]), 8);
    let freer = pool.freer();
    unsafe { freer.free_bulk(&mut bufs[24..32]) };
    unsafe { freer.free_bulk(&mut bufs[..0]) }; // empty: no-op
    let probe = rx[0].rx_burst(&mut bufs[..1]);
    assert_eq!(probe, 1);
    assert_eq!(in_use(bufs[0]), 1, "everything else is back in the pool");
    unsafe { rte_pktmbuf_free(bufs[0]) };

    // ---- zero-length bursts are harmless --------------------------------------
    assert_eq!(rx[0].rx_burst(&mut bufs[..0]), 0);
    assert_eq!(tx[0].tx_burst(&mut bufs[..0]), 0);

    drop((rx, tx));

    // ---- Ring: usable capacity is count - 1, FIFO, depth reporting ------------
    assert!(
        Ring::<*mut std::ffi::c_void>::create_spsc("r_bad", 6, -1).is_err(),
        "ring size must be a power of two"
    );
    let ring = Ring::create_spsc("eal_port_ring", 8, -1).expect("ring");
    let items: Vec<*mut std::ffi::c_void> = (1..=10usize).map(|i| i as *mut _).collect();
    assert_eq!(ring.enqueue_burst(&items), 7, "8 slots hold 7");
    assert_eq!(ring.enqueue_burst(&items[..1]), 0, "full");
    let mut out = [null_mut(); 4];
    assert_eq!(ring.dequeue_burst(&mut out), 4);
    assert_eq!(out.map(|p| p as usize), [1, 2, 3, 4]);
    let mut two = [null_mut(); 2];
    // 3 entries waiting before this call, 2 dequeued, 1 left: depth 3.
    assert_eq!(ring.dequeue_burst_with_depth(&mut two), (2, 3));
    assert_eq!(two.map(|p| p as usize), [5, 6]);
    assert_eq!(ring.dequeue_burst(&mut out), 1);
    assert_eq!(out[0] as usize, 7);
    assert_eq!(ring.dequeue_burst(&mut out), 0, "empty");
    drop(ring);
    drop(port);
}
