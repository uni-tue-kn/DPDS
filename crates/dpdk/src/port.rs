use std::cell::Cell;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::mem;

use crate::error::{check, DpdkError};
use crate::mempool::Mempool;

/// Six-byte Ethernet MAC address.
pub type MacAddr = [u8; 6];

// ── Port enumeration ─────────────────────────────────────────────────────────

/// Returns the number of available Ethernet ports.
#[inline(always)]
pub fn rte_eth_dev_count_avail() -> u16 {
    unsafe { dpdk_sys::rte_eth_dev_count_avail() }
}

/// Iterator over all valid (ownerless) Ethernet port IDs.
///
/// Mirrors `RTE_ETH_FOREACH_DEV` from `<rte_ethdev.h>`.
pub struct PortIter {
    next: u16,
}

impl Iterator for PortIter {
    type Item = u16;

    #[inline(always)]
    fn next(&mut self) -> Option<u16> {
        let port = unsafe { dpdk_sys::rte_eth_find_next(self.next) };
        if (port as u32) < dpdk_sys::RTE_MAX_ETHPORTS {
            self.next = port + 1;
            Some(port)
        } else {
            None
        }
    }
}

/// Iterate over all valid Ethernet port IDs (equivalent to
/// `RTE_ETH_FOREACH_DEV`).
#[inline(always)]
pub fn foreach_dev() -> PortIter {
    PortIter { next: 0 }
}

/// Returns a port's DPDK device name (e.g. `"0000:65:00.1"` for a PCI
/// function, see `rte_eth_dev_get_name_by_port`'s own doc comment for the
/// other possible forms). Control-path call, usable before `Port::init`
/// (e.g. to resolve a `--tx-pci`-style device name against DPDK's own port
/// list) as well as after — it only needs a valid port id, not a live
/// [`Port`] handle, so unlike every [`Port`] accessor below it's a free
/// function rather than a method.
pub fn device_name(port: u16) -> Result<String, DpdkError> {
    // RTE_ETH_NAME_MAX_LEN (`<rte_ethdev.h>`) is itself a `#define` alias
    // of RTE_DEV_NAME_MAX_LEN (`<rte_dev.h>`, 64) -- bindgen doesn't
    // reliably resolve a macro-to-macro alias into a usable Rust const, so
    // the buffer size is pinned here by hand instead of via a
    // `dpdk_sys::RTE_ETH_NAME_MAX_LEN` constant.
    const RTE_ETH_NAME_MAX_LEN: usize = 64;
    let mut buf = [0 as std::ffi::c_char; RTE_ETH_NAME_MAX_LEN];
    check(unsafe { dpdk_sys::rte_eth_dev_get_name_by_port(port, buf.as_mut_ptr()) })?;
    Ok(unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned())
}

/// TX offloads [`Port::init`] requests for a device reporting
/// `tx_offload_capa`: `RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE` if the device
/// offers it, nothing else. Fast-free is valid here because every mbuf
/// comes from one pool, is direct and single-segment, and is never
/// reference-counted -- the same promise
/// [`crate::mempool::MbufFreer::free_bulk`] relies on.
pub fn tx_offloads_for(tx_offload_capa: u64) -> u64 {
    tx_offload_capa & dpdk_sys::RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE
}

// ── Port handle ──────────────────────────────────────────────────────────────

/// A configured and started Ethernet port — the **control-plane** handle.
///
/// The lifetime parameter ties the port to the [`Mempool`] backing its RX
/// queues: the borrow checker guarantees the pool cannot be freed while the
/// device could still deliver packets into it.
///
/// The device is stopped and closed on drop.
///
/// The data plane lives on per-queue handles instead: [`Port::queues`]
/// splits the port into [`RxQueue`]/[`TxQueue`] handles, one per configured
/// queue, each of which can be moved to (and polled from) exactly one
/// thread.  `Port` itself stays `!Sync` because DPDK's ethdev control-path
/// functions are not thread-safe against each other either.
pub struct Port<'pool> {
    id: u16,
    nb_rx_queues: u16,
    nb_tx_queues: u16,
    device_name: String,
    /// RX/TX descriptor ring sizes as actually applied by the driver
    /// (post-`rte_eth_dev_adjust_nb_rx_tx_desc`) — may silently differ
    /// from what was requested (see `requested_rxd`/`requested_txd`
    /// below), e.g. a driver clamping 65,535 down to its own max.
    nb_rxd: u16,
    nb_txd: u16,
    /// RX/TX descriptor ring sizes as requested by the caller, before any
    /// driver clamping — kept alongside the applied values above so a
    /// caller (or telemetry reader) can tell the two apart instead of only
    /// ever seeing what was actually applied.
    requested_rxd: u16,
    requested_txd: u16,
    /// Whether `RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE` was requested (see
    /// [`tx_offloads_for`]).
    mbuf_fast_free: bool,
    _pool: PhantomData<&'pool Mempool>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<'pool> Port<'pool> {
    /// Initialise one Ethernet port with `nb_queues` RX and TX queues each,
    /// and start it.
    ///
    /// Mirrors the `port_init()` function in DPDK's own `examples/skeleton/
    /// basicfwd.c`, generalised to multiple queues:
    /// 1. Validates the port ID and queue count.
    /// 2. Queries device capabilities and enables `MBUF_FAST_FREE` when
    ///    available ([`tx_offloads_for`]; valid here because all mbufs come
    ///    from one pool and are never reference-counted).
    /// 3. With more than one queue, enables RSS so the NIC spreads flows
    ///    across the RX queues (hashing IP/TCP/UDP fields, restricted to
    ///    what the device supports; per-flow packet order is preserved).
    /// 4. Configures the device and adjusts descriptor ring sizes to
    ///    hardware limits.
    /// 5. Sets up every RX and TX queue on the port's own NUMA socket.
    /// 6. Starts the device and enables promiscuous mode.
    ///
    /// `mtu` is the requested MTU (`rte_eth_rxmode::mtu`: the max L3 payload
    /// size, excluding the 14-byte Ethernet header and 4-byte FCS) --
    /// callers whose frames carry extra header overhead beyond plain
    /// Ethernet+IP (e.g. tunnel or VLAN encapsulation) must pad it in
    /// accordingly, or oversized frames get dropped at the NIC before ever
    /// reaching DPDK.
    pub fn init(
        port: u16,
        pool: &'pool Mempool,
        nb_rxd: u16,
        nb_txd: u16,
        nb_queues: u16,
        mtu: u32,
    ) -> Result<Self, DpdkError> {
        if unsafe { dpdk_sys::rte_eth_dev_is_valid_port(port) } == 0 {
            return Err(DpdkError(-19)); // -ENODEV
        }
        if nb_queues == 0 {
            return Err(DpdkError(-22)); // -EINVAL
        }

        let device_name = device_name(port)?;

        // ── 1. Gather device capabilities ────────────────────────────────────
        let mut dev_info: dpdk_sys::rte_eth_dev_info = unsafe { mem::zeroed() };
        check(unsafe { dpdk_sys::rte_eth_dev_info_get(port, &mut dev_info) })?;
        if nb_queues > dev_info.max_rx_queues || nb_queues > dev_info.max_tx_queues {
            return Err(DpdkError(-22)); // -EINVAL
        }
        if mtu < dev_info.min_mtu as u32 || mtu > dev_info.max_mtu as u32 {
            return Err(DpdkError(-22)); // -EINVAL
        }

        let mut port_conf: dpdk_sys::rte_eth_conf = unsafe { mem::zeroed() };
        port_conf.rxmode.mtu = mtu;
        port_conf.txmode.offloads = tx_offloads_for(dev_info.tx_offload_capa);
        let mbuf_fast_free =
            port_conf.txmode.offloads & dpdk_sys::RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE != 0;
        if nb_queues > 1 {
            port_conf.rxmode.mq_mode = dpdk_sys::rte_eth_rx_mq_mode_RTE_ETH_MQ_RX_RSS;
            // NULL key = driver default; hash profile from wrapper.h (the
            // RTE_ETH_RSS_* macros are not bindgen-expressible).
            port_conf.rx_adv_conf.rss_conf.rss_key = std::ptr::null_mut();
            port_conf.rx_adv_conf.rss_conf.rss_hf =
                unsafe { dpdk_sys::rte_eth_rss_ip_tcp_udp() } & dev_info.flow_type_rss_offloads;
        }

        // ── 2. Configure the device ───────────────────────────────────────────
        check(unsafe { dpdk_sys::rte_eth_dev_configure(port, nb_queues, nb_queues, &port_conf) })?;

        let requested_rxd = nb_rxd;
        let requested_txd = nb_txd;
        let mut nb_rxd = nb_rxd;
        let mut nb_txd = nb_txd;
        check(unsafe {
            dpdk_sys::rte_eth_dev_adjust_nb_rx_tx_desc(port, &mut nb_rxd, &mut nb_txd)
        })?;

        // ── 3. RX queues ──────────────────────────────────────────────────────
        for q in 0..nb_queues {
            check(unsafe {
                dpdk_sys::rte_eth_rx_queue_setup(
                    port,
                    q,
                    nb_rxd,
                    rte_eth_dev_socket_id(port) as u32,
                    std::ptr::null(), // use driver defaults
                    pool.as_ptr(),
                )
            })?;
        }

        // ── 4. TX queues ──────────────────────────────────────────────────────
        let mut txconf = dev_info.default_txconf;
        txconf.offloads = port_conf.txmode.offloads;
        for q in 0..nb_queues {
            check(unsafe {
                dpdk_sys::rte_eth_tx_queue_setup(
                    port,
                    q,
                    nb_txd,
                    rte_eth_dev_socket_id(port) as u32,
                    &txconf,
                )
            })?;
        }

        // ── 5. Start ──────────────────────────────────────────────────────────
        check(unsafe { dpdk_sys::rte_eth_dev_start(port) })?;

        // ── 6. Promiscuous mode ───────────────────────────────────────────────
        check(unsafe { dpdk_sys::rte_eth_promiscuous_enable(port) })?;

        Ok(Port {
            id: port,
            nb_rx_queues: nb_queues,
            nb_tx_queues: nb_queues,
            device_name,
            nb_rxd,
            nb_txd,
            requested_rxd,
            requested_txd,
            mbuf_fast_free,
            _pool: PhantomData,
            _not_sync: PhantomData,
        })
    }

    /// The DPDK port ID.
    #[inline(always)]
    pub fn id(&self) -> u16 {
        self.id
    }

    /// The port's DPDK device name (e.g. `"0000:65:00.1"`), captured once
    /// at [`Port::init`] time via [`device_name`] (the free function of
    /// the same name).
    #[inline(always)]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// RX descriptor ring size actually applied by the driver (post-
    /// `rte_eth_dev_adjust_nb_rx_tx_desc`) — see [`Port::rx_ring_size_requested`]
    /// for what was asked for before any driver clamping.
    #[inline(always)]
    pub fn rx_ring_size(&self) -> u16 {
        self.nb_rxd
    }

    /// TX descriptor ring size actually applied by the driver — see
    /// [`Port::rx_ring_size`]'s doc comment.
    #[inline(always)]
    pub fn tx_ring_size(&self) -> u16 {
        self.nb_txd
    }

    /// RX descriptor ring size as requested at [`Port::init`], before any
    /// driver clamping.
    #[inline(always)]
    pub fn rx_ring_size_requested(&self) -> u16 {
        self.requested_rxd
    }

    /// TX descriptor ring size as requested at [`Port::init`], before any
    /// driver clamping.
    #[inline(always)]
    pub fn tx_ring_size_requested(&self) -> u16 {
        self.requested_txd
    }

    /// Whether `RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE` is enabled on this
    /// port, i.e. whether the device reported the capability at
    /// [`Port::init`] (see [`tx_offloads_for`]). A device that doesn't
    /// (e.g. mlx5 with MPRQ on, whose RX mbufs may be attached to shared
    /// external buffers) can hand out mbufs that must not be freed with
    /// [`crate::mempool::MbufFreer::free_bulk`].
    #[inline(always)]
    pub fn mbuf_fast_free(&self) -> bool {
        self.mbuf_fast_free
    }

    /// The port's current negotiated link speed (e.g. `"100 Gbps"`), via
    /// `rte_eth_link_get`/`rte_eth_link_speed_to_str`. Only meaningful once
    /// the device is started (see [`Port::init`]'s step 5), since the link
    /// has to come up before a speed is negotiated.
    pub fn link_speed(&self) -> Result<String, DpdkError> {
        let mut link: dpdk_sys::rte_eth_link = unsafe { mem::zeroed() };
        check(unsafe { dpdk_sys::rte_eth_link_get(self.id, &mut link) })?;
        let link_speed = unsafe { link.__bindgen_anon_1.__bindgen_anon_1.link_speed };
        let speed = unsafe { CStr::from_ptr(dpdk_sys::rte_eth_link_speed_to_str(link_speed)) };
        Ok(speed.to_string_lossy().into_owned())
    }

    /// Whether the port's link is currently up, via `rte_eth_link_get_nowait`
    /// (reports the current state without waiting for autonegotiation to
    /// finish). Only meaningful once the device is started (see
    /// [`Port::init`]'s step 5).
    pub fn link_up(&self) -> Result<bool, DpdkError> {
        let mut link: dpdk_sys::rte_eth_link = unsafe { mem::zeroed() };
        check(unsafe { dpdk_sys::rte_eth_link_get_nowait(self.id, &mut link) })?;
        Ok(unsafe { link.__bindgen_anon_1.__bindgen_anon_1.link_status() } != 0)
    }

    /// Split the port into its per-queue **data-plane** handles.
    ///
    /// Returns one [`RxQueue`] and one [`TxQueue`] per configured queue.
    /// The handles borrow the port mutably, so while any of them is alive
    /// the port cannot be dropped (stopping the device under a live queue)
    /// or split a second time (duplicating a handle).  Each handle is
    /// therefore the unique capability to poll its (port, queue, direction)
    /// — exactly DPDK's threading rule, encoded in the borrow checker.
    pub fn queues(&mut self) -> (Vec<RxQueue<'_>>, Vec<TxQueue<'_>>) {
        let rx = (0..self.nb_rx_queues)
            .map(|q| RxQueue {
                port_id: self.id,
                queue_id: q,
                _port: PhantomData,
                _not_sync: PhantomData,
            })
            .collect();
        let tx = (0..self.nb_tx_queues)
            .map(|q| TxQueue {
                port_id: self.id,
                queue_id: q,
                _port: PhantomData,
                _not_sync: PhantomData,
            })
            .collect();
        (rx, tx)
    }
}

// ── Data-plane queue handles ─────────────────────────────────────────────────

/// RX handle for one (port, queue) pair.
///
/// `Send` but not `Sync`, and never duplicated (see [`Port::queues`]): a
/// queue is polled by exactly one thread, per DPDK's threading rules.  Burst
/// calls perform no validation — holding the handle is proof the queue
/// exists, so the fast path carries no extra branch.
pub struct RxQueue<'port> {
    port_id: u16,
    queue_id: u16,
    _port: PhantomData<&'port ()>,
    _not_sync: PhantomData<Cell<()>>,
}

impl RxQueue<'_> {
    /// Receive a burst of packets into `bufs`.
    ///
    /// Returns the number of packets actually received (≤ `bufs.len()`).
    /// The received mbufs are owned by the caller and must eventually be
    /// transmitted via [`TxQueue::tx_burst`] or freed with
    /// [`rte_pktmbuf_free`].
    #[inline(always)]
    pub fn rx_burst(&mut self, bufs: &mut [RawMbuf]) -> u16 {
        let n = bufs.len().min(u16::MAX as usize) as u16;
        unsafe { dpdk_sys::rte_eth_rx_burst(self.port_id, self.queue_id, bufs.as_mut_ptr(), n) }
    }
}

/// TX handle for one (port, queue) pair.
///
/// Same ownership rules as [`RxQueue`].
pub struct TxQueue<'port> {
    port_id: u16,
    queue_id: u16,
    _port: PhantomData<&'port ()>,
    _not_sync: PhantomData<Cell<()>>,
}

impl TxQueue<'_> {
    /// Transmit a burst of packets from `bufs`.
    ///
    /// Returns the number of packets actually sent; ownership of those mbufs
    /// passes to the driver.  The caller must free any packets at indices
    /// `nb_tx..` with [`rte_pktmbuf_free`].
    #[inline(always)]
    pub fn tx_burst(&mut self, bufs: &mut [RawMbuf]) -> u16 {
        let n = bufs.len().min(u16::MAX as usize) as u16;
        unsafe { dpdk_sys::rte_eth_tx_burst(self.port_id, self.queue_id, bufs.as_mut_ptr(), n) }
    }
}

/// Stops and closes the device identified by `port_id`.
///
/// Free function rather than a `Port` method: callers that `Box::leak`
/// their `Port` for `'static` queue handles (needed by
/// [`crate::eal::remote_launch`]'s `'static` bound) have that borrow
/// widened by the compiler to live for `'static` too -- so the `Port`
/// itself can never be borrowed again afterwards, not even at shutdown,
/// even once every lcore holding a queue handle has actually been joined.
/// Going through the raw port ID instead sidesteps that borrow entirely.
///
/// A leaked value's `Drop` never runs, so shutdown code must call this
/// explicitly -- relying on `Drop` alone silently skips it, leaving the
/// device started when `rte_eal_cleanup` later tries to close it (ethdev
/// then logs "Cannot close started device").
pub fn shutdown_port(port_id: u16) {
    // Stop before close, per the ethdev API contract.  Log the return
    // codes since a failed stop leaves the device's internal dev_started
    // flag set, which makes close() fail too.
    unsafe {
        let stop_ret = dpdk_sys::rte_eth_dev_stop(port_id);
        if stop_ret != 0 {
            eprintln!(
                "rte_eth_dev_stop(port {port_id}) failed: {}",
                DpdkError(stop_ret)
            );
        }
        let close_ret = dpdk_sys::rte_eth_dev_close(port_id);
        if close_ret < 0 {
            eprintln!(
                "rte_eth_dev_close(port {port_id}) failed: {}",
                DpdkError(close_ret)
            );
        }
    }
}

impl Drop for Port<'_> {
    fn drop(&mut self) {
        // Errors cannot be propagated from Drop; the device is going away
        // either way. Only reached for a non-leaked Port -- see
        // `shutdown_port` for the leaked case (every `Port` in this
        // codebase today).
        shutdown_port(self.id);
    }
}

// ── Statistics ───────────────────────────────────────────────────────────────

/// Basic per-port counters (subset of `rte_eth_stats`).
#[derive(Debug, Default, Clone, Copy)]
pub struct PortStats {
    /// Successfully received packets.
    pub ipackets: u64,
    /// Successfully transmitted packets.
    pub opackets: u64,
    /// Successfully received bytes.
    pub ibytes: u64,
    /// Successfully transmitted bytes.
    pub obytes: u64,
    /// RX packets dropped by the hardware (e.g. full RX descriptor ring).
    pub imissed: u64,
    /// Erroneous received packets.
    pub ierrors: u64,
    /// Failed transmitted packets.
    pub oerrors: u64,
    /// RX mbuf allocation failures — a sign the mempool is undersized.
    pub rx_nombuf: u64,
}

/// Read a port's basic statistics.
///
/// This is a control-path API: DPDK validates the port ID (unknown ports
/// return an error) and it may be called from any thread, so it takes a raw
/// port ID rather than a [`Port`] handle — e.g. from a CLI thread while the
/// forwarding thread owns the `Port`.
pub fn eth_stats_get(port: u16) -> Result<PortStats, DpdkError> {
    let mut s: dpdk_sys::rte_eth_stats = unsafe { mem::zeroed() };
    check(unsafe { dpdk_sys::rte_eth_stats_get(port, &mut s) })?;
    Ok(PortStats {
        ipackets: s.ipackets,
        opackets: s.opackets,
        ibytes: s.ibytes,
        obytes: s.obytes,
        imissed: s.imissed,
        ierrors: s.ierrors,
        oerrors: s.oerrors,
        rx_nombuf: s.rx_nombuf,
    })
}

// ── NUMA / lcore helpers ─────────────────────────────────────────────────────

/// NUMA socket ID of a port, or -1 if unknown.
#[inline(always)]
pub fn rte_eth_dev_socket_id(port: u16) -> i32 {
    unsafe { dpdk_sys::rte_eth_dev_socket_id(port) }
}

/// NUMA socket ID of the calling thread.
#[inline(always)]
pub fn rte_socket_id() -> u32 {
    unsafe { dpdk_sys::rte_socket_id() }
}

/// Logical core ID of the calling thread.
///
/// Wraps the `static inline rte_lcore_id()` which reads a thread-local
/// variable — guaranteed zero overhead when inlined.
#[inline(always)]
pub fn rte_lcore_id() -> u32 {
    unsafe { dpdk_sys::rte_lcore_id() }
}

// ── Mbuf handling ────────────────────────────────────────────────────────────

/// Raw mbuf pointer type re-exported for use in burst buffers.
pub type RawMbuf = *mut dpdk_sys::rte_mbuf;

/// Free a single mbuf back to its originating pool.
///
/// # Safety
/// `pkt` must be a valid, non-null mbuf pointer not aliased elsewhere.
#[inline(always)]
pub unsafe fn rte_pktmbuf_free(pkt: RawMbuf) {
    dpdk_sys::rte_pktmbuf_free(pkt);
}

// ── `tx_offloads_for` ───────────────────────────────────────────────────────
//
// `Port::init` takes the TX offloads it requests from `tx_offloads_for`, a
// pure function of the device's reported `tx_offload_capa`, so the decision
// is testable without EAL: exactly `RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE` when
// the capability bit is set and `0` otherwise. No other offload is
// requested, whatever else the device reports.
//
// `Port::mbuf_fast_free` reports the result; it needs a live port and is not
// tested here.
#[cfg(test)]
mod tx_offload_tests {
    use super::tx_offloads_for;

    const FAST_FREE: u64 = dpdk_sys::RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE;

    #[test]
    fn tx_offloads_enable_fast_free_when_capable() {
        assert_eq!(tx_offloads_for(FAST_FREE), FAST_FREE);
        // Other capability bits must not leak into the requested offloads.
        assert_eq!(
            tx_offloads_for(u64::MAX),
            FAST_FREE,
            "only MBUF_FAST_FREE may be requested, even if the device reports every offload"
        );
    }

    #[test]
    fn tx_offloads_omit_fast_free_when_not_capable() {
        assert_eq!(tx_offloads_for(0), 0);
        assert_eq!(
            tx_offloads_for(!FAST_FREE),
            0,
            "a device without the MBUF_FAST_FREE capability (e.g. mlx5 under MPRQ) must get \
             no offloads at all"
        );
    }
}
