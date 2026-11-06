//! `Port::rx_burst_mode()` / `tx_burst_mode()` (queue 0) and `Clock` (reads
//! `now_ns` without the per-call `OnceLock` check) on a real EAL with one
//! `net_null` vdev. Single test function: one EAL per process.

use dpdk::clock::Clock;
use dpdk::{now_ns, Eal, Mempool, Port};

#[test]
fn burst_mode_strings_and_clock_value() {
    let args: Vec<String> = [
        "eal_new_api_test",
        "--no-huge",
        "-m",
        "256",
        "--no-pci",
        "--in-memory",
        "--no-telemetry",
        "--file-prefix",
        "dpdk_crate_eal_new_api",
        "-l",
        "0",
        "--vdev",
        "net_null0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let _eal = Eal::new(&args).expect("EAL");

    // ---- Clock converts like the free function ---------------------------------
    // Same TSC and scale, so a `Clock` read falls between two free-function reads.
    let clock = Clock::new();
    let mut last = 0.0f64;
    for _ in 0..1000 {
        let a = now_ns();
        let c = clock.now_ns();
        let b = now_ns();
        assert!(a <= c && c <= b, "{a} <= {c} <= {b}");
        assert!(c >= last, "clock must not go backwards");
        last = c;
    }
    assert!(last > 0.0 && last.is_finite());
    // Usable from another thread (worker lcores are spawned threads).
    fn assert_send<T: Send>() {}
    assert_send::<Clock>();
    let from_thread = std::thread::spawn(move || clock.now_ns()).join().unwrap();
    assert!(from_thread >= last);

    // ---- burst mode strings -------------------------------------------------
    let pool = Mempool::create("eal_new_api_pool", 255, 0, Mempool::DEFAULT_BUF_SIZE, 0).unwrap();
    let port = Port::init(0, &pool, 256, 256, 1, 1500).expect("port");

    // Oracle: the raw DPDK calls on queue 0 (`Some(info)` iff the driver reports a mode).
    let raw = |rx: bool| -> Option<String> {
        let mut mode: dpdk_sys::rte_eth_burst_mode = unsafe { std::mem::zeroed() };
        let ret = unsafe {
            if rx {
                dpdk_sys::rte_eth_rx_burst_mode_get(0, 0, &mut mode)
            } else {
                dpdk_sys::rte_eth_tx_burst_mode_get(0, 0, &mut mode)
            }
        };
        (ret == 0).then(|| {
            unsafe { std::ffi::CStr::from_ptr(mode.info.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        })
    };
    assert_eq!(port.rx_burst_mode(), raw(true));
    assert_eq!(port.tx_burst_mode(), raw(false));
    for mode in [port.rx_burst_mode(), port.tx_burst_mode()]
        .into_iter()
        .flatten()
    {
        assert!(!mode.is_empty());
    }
    assert_eq!(port.rx_burst_mode(), port.rx_burst_mode());
}
