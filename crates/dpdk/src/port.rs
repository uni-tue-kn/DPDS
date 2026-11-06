use std::cell::Cell;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::mem;

use crate::error::{check, DpdkError};
use crate::mempool::Mempool;

/// Six-byte Ethernet MAC address.
pub type MacAddr = [u8; 6];

// ── Port enumeration ─────────────────────────────────────────────────────────

/// Number of available Ethernet ports.
#[inline(always)]
pub fn rte_eth_dev_count_avail() -> u16 {
    unsafe { dpdk_sys::rte_eth_dev_count_avail() }
}

/// Iterator over all valid (ownerless) Ethernet port IDs (`RTE_ETH_FOREACH_DEV`).
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

#[inline(always)]
pub fn foreach_dev() -> PortIter {
    PortIter { next: 0 }
}

/// A port's DPDK device name (e.g. `"0000:65:00.1"` for a PCI function).
/// Control-path call; needs a valid port id, not a [`Port`], so it works before
/// `Port::init`.
pub fn device_name(port: u16) -> Result<String, DpdkError> {
    // RTE_ETH_NAME_MAX_LEN is a `#define` alias of RTE_DEV_NAME_MAX_LEN (64), which
    // bindgen does not reliably resolve, so the size is pinned by hand.
    const RTE_ETH_NAME_MAX_LEN: usize = 64;
    let mut buf = [0 as std::ffi::c_char; RTE_ETH_NAME_MAX_LEN];
    check(unsafe { dpdk_sys::rte_eth_dev_get_name_by_port(port, buf.as_mut_ptr()) })?;
    Ok(unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned())
}

/// TX offloads [`Port::init`] requests for a device reporting `tx_offload_capa`:
/// `RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE` if offered, nothing else. Valid because
/// every mbuf comes from one pool, is direct and single-segment, and is never
/// reference-counted (the promise [`crate::mempool::MbufFreer::free_bulk`] relies on).
pub fn tx_offloads_for(tx_offload_capa: u64) -> u64 {
    tx_offload_capa & dpdk_sys::RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE
}

/// The RX and TX descriptor ring sizes the driver of `port` would apply for the
/// requested ones (`rte_eth_dev_adjust_nb_rx_tx_desc`: clamped to the device's
/// limits and alignment), as [`Port::init`] does. Needs only a valid port, so it
/// works before the pool exists.
pub fn adjust_nb_rx_tx_desc(port: u16, nb_rxd: u16, nb_txd: u16) -> Result<(u16, u16), DpdkError> {
    let (mut rxd, mut txd) = (nb_rxd, nb_txd);
    check(unsafe { dpdk_sys::rte_eth_dev_adjust_nb_rx_tx_desc(port, &mut rxd, &mut txd) })?;
    Ok((rxd, txd))
}

// ── Port handle ──────────────────────────────────────────────────────────────

/// A configured and started Ethernet port: the control-plane handle, stopped and
/// closed on drop.
///
/// The lifetime ties the port to the [`Mempool`] backing its RX queues, so the
/// pool cannot be freed while the device could still deliver packets into it.
///
/// The data plane uses per-queue handles ([`Port::queues`]), each moved to and
/// polled from exactly one thread. `Port` is `!Sync` because DPDK's ethdev
/// control-path functions are not thread-safe against each other.
pub struct Port<'pool> {
    id: u16,
    nb_rx_queues: u16,
    nb_tx_queues: u16,
    device_name: String,
    /// RX/TX descriptor ring sizes applied by the driver
    /// (post-`rte_eth_dev_adjust_nb_rx_tx_desc`); may silently differ from the
    /// requested ones, e.g. a driver clamping 65,535 down to its own max.
    nb_rxd: u16,
    nb_txd: u16,
    /// RX/TX descriptor ring sizes requested by the caller, before any driver
    /// clamping.
    requested_rxd: u16,
    requested_txd: u16,
    mbuf_fast_free: bool,
    _pool: PhantomData<&'pool Mempool>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<'pool> Port<'pool> {
    /// Initialises and starts one Ethernet port with `nb_queues` RX and TX
    /// queues each, in promiscuous mode. `MBUF_FAST_FREE` is enabled when the
    /// device offers it ([`tx_offloads_for`]); with more than one queue RSS
    /// spreads IP/TCP/UDP flows across the RX queues (restricted to what the
    /// device supports; per-flow order is preserved). Descriptor ring sizes are
    /// adjusted to hardware limits.
    ///
    /// `mtu` is the requested MTU (`rte_eth_rxmode::mtu`: the max L3 payload,
    /// excluding the 14-byte Ethernet header and 4-byte FCS); frames with extra
    /// header overhead (tunnel, VLAN) need it padded, or the NIC drops oversized
    /// frames before DPDK sees them.
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

        // Device capabilities
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

        // Configure the device
        check(unsafe { dpdk_sys::rte_eth_dev_configure(port, nb_queues, nb_queues, &port_conf) })?;

        let requested_rxd = nb_rxd;
        let requested_txd = nb_txd;
        let (nb_rxd, nb_txd) = adjust_nb_rx_tx_desc(port, nb_rxd, nb_txd)?;

        // RX queues
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

        // TX queues
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

        // Start
        check(unsafe { dpdk_sys::rte_eth_dev_start(port) })?;

        // Promiscuous mode
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

    #[inline(always)]
    pub fn id(&self) -> u16 {
        self.id
    }

    /// The port's DPDK device name (e.g. `"0000:65:00.1"`), captured at
    /// [`Port::init`].
    #[inline(always)]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// RX descriptor ring size applied by the driver; see
    /// [`Port::rx_ring_size_requested`] for the requested one.
    #[inline(always)]
    pub fn rx_ring_size(&self) -> u16 {
        self.nb_rxd
    }

    /// TX descriptor ring size applied by the driver.
    #[inline(always)]
    pub fn tx_ring_size(&self) -> u16 {
        self.nb_txd
    }

    /// RX descriptor ring size requested at [`Port::init`], before driver clamping.
    #[inline(always)]
    pub fn rx_ring_size_requested(&self) -> u16 {
        self.requested_rxd
    }

    /// TX descriptor ring size requested at [`Port::init`], before driver clamping.
    #[inline(always)]
    pub fn tx_ring_size_requested(&self) -> u16 {
        self.requested_txd
    }

    /// Whether `RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE` is enabled, i.e. the device
    /// reported the capability at [`Port::init`]. A device without it (e.g. mlx5
    /// with MPRQ on, whose RX mbufs may use shared external buffers) can hand out
    /// mbufs that must not be freed with [`crate::mempool::MbufFreer::free_bulk`].
    #[inline(always)]
    pub fn mbuf_fast_free(&self) -> bool {
        self.mbuf_fast_free
    }

    /// The RX burst function the driver uses on queue 0 (e.g. `"Vector AVX2"`),
    /// or `None` if the driver does not report one. For a startup log line.
    pub fn rx_burst_mode(&self) -> Option<String> {
        let mut mode: dpdk_sys::rte_eth_burst_mode = unsafe { mem::zeroed() };
        (unsafe { dpdk_sys::rte_eth_rx_burst_mode_get(self.id, 0, &mut mode) } == 0)
            .then(|| burst_mode_info(&mode))
    }

    /// The TX burst function the driver uses on queue 0, or `None` if the driver
    /// does not report one. For a startup log line.
    pub fn tx_burst_mode(&self) -> Option<String> {
        let mut mode: dpdk_sys::rte_eth_burst_mode = unsafe { mem::zeroed() };
        (unsafe { dpdk_sys::rte_eth_tx_burst_mode_get(self.id, 0, &mut mode) } == 0)
            .then(|| burst_mode_info(&mode))
    }

    /// The negotiated link speed (e.g. `"100 Gbps"`). Only meaningful once the
    /// device is started, since the link has to come up first.
    pub fn link_speed(&self) -> Result<String, DpdkError> {
        let mut link: dpdk_sys::rte_eth_link = unsafe { mem::zeroed() };
        check(unsafe { dpdk_sys::rte_eth_link_get(self.id, &mut link) })?;
        let link_speed = unsafe { link.__bindgen_anon_1.__bindgen_anon_1.link_speed };
        let speed = unsafe { CStr::from_ptr(dpdk_sys::rte_eth_link_speed_to_str(link_speed)) };
        Ok(speed.to_string_lossy().into_owned())
    }

    /// Whether the link is currently up (`rte_eth_link_get_nowait`: does not
    /// wait for autonegotiation). Only meaningful once the device is started.
    pub fn link_up(&self) -> Result<bool, DpdkError> {
        let mut link: dpdk_sys::rte_eth_link = unsafe { mem::zeroed() };
        check(unsafe { dpdk_sys::rte_eth_link_get_nowait(self.id, &mut link) })?;
        Ok(unsafe { link.__bindgen_anon_1.__bindgen_anon_1.link_status() } != 0)
    }

    /// Splits the port into one [`RxQueue`] and one [`TxQueue`] per configured
    /// queue. The handles borrow the port mutably, so while any is alive the port
    /// cannot be dropped (stopping the device under a live queue) or split again
    /// (duplicating a handle): each handle is the unique capability to poll its
    /// (port, queue, direction), DPDK's threading rule in the borrow checker.
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

/// RX handle for one (port, queue) pair: `Send` but not `Sync`, never duplicated
/// (see [`Port::queues`]). Burst calls do no validation, since holding the
/// handle proves the queue exists.
pub struct RxQueue<'port> {
    port_id: u16,
    queue_id: u16,
    _port: PhantomData<&'port ()>,
    _not_sync: PhantomData<Cell<()>>,
}

impl RxQueue<'_> {
    /// Receives a burst into `bufs`; returns the number received. The caller owns
    /// the mbufs and must transmit them ([`TxQueue::tx_burst`]) or free them
    /// ([`rte_pktmbuf_free`]).
    #[inline(always)]
    pub fn rx_burst(&mut self, bufs: &mut [RawMbuf]) -> u16 {
        let n = bufs.len().min(u16::MAX as usize) as u16;
        unsafe { dpdk_sys::rte_eth_rx_burst(self.port_id, self.queue_id, bufs.as_mut_ptr(), n) }
    }
}

/// TX handle for one (port, queue) pair; same ownership rules as [`RxQueue`].
pub struct TxQueue<'port> {
    port_id: u16,
    queue_id: u16,
    _port: PhantomData<&'port ()>,
    _not_sync: PhantomData<Cell<()>>,
}

impl TxQueue<'_> {
    /// Transmits a burst from `bufs`; returns the number sent, whose ownership
    /// passes to the driver. The caller must free the packets at indices `nb_tx..`
    /// with [`rte_pktmbuf_free`].
    #[inline(always)]
    pub fn tx_burst(&mut self, bufs: &mut [RawMbuf]) -> u16 {
        let n = bufs.len().min(u16::MAX as usize) as u16;
        unsafe { dpdk_sys::rte_eth_tx_burst(self.port_id, self.queue_id, bufs.as_mut_ptr(), n) }
    }
}

/// Stops and closes the device `port_id`.
///
/// A free function because callers `Box::leak` their `Port` for `'static` queue
/// handles (needed by [`crate::eal::remote_launch`]), which borrows it for
/// `'static`, so it cannot be borrowed again at shutdown. A leaked value's
/// `Drop` never runs, so shutdown code must call this explicitly, or the device
/// stays started when `rte_eal_cleanup` tries to close it (ethdev then logs
/// "Cannot close started device").
pub fn shutdown_port(port_id: u16) {
    // Stop before close. A failed stop leaves the device's dev_started flag
    // set, which makes close() fail too, so log both return codes.
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
        // Errors cannot be propagated from Drop. Only reached for a non-leaked
        // Port; see `shutdown_port` for the leaked case.
        shutdown_port(self.id);
    }
}

// ── Statistics ───────────────────────────────────────────────────────────────

/// Basic per-port counters (subset of `rte_eth_stats`).
#[derive(Debug, Default, Clone, Copy)]
pub struct PortStats {
    pub ipackets: u64,
    pub opackets: u64,
    pub ibytes: u64,
    pub obytes: u64,
    /// RX packets dropped by the hardware (e.g. full RX descriptor ring).
    pub imissed: u64,
    pub ierrors: u64,
    pub oerrors: u64,
    /// RX mbuf allocation failures: the mempool is undersized.
    pub rx_nombuf: u64,
}

/// Resets a port's basic statistics and xstats to 0.
pub fn eth_stats_reset(port: u16) -> Result<(), DpdkError> {
    check(unsafe { dpdk_sys::rte_eth_stats_reset(port) })?;
    check(unsafe { dpdk_sys::rte_eth_xstats_reset(port) })?;
    Ok(())
}

/// Reads a port's basic statistics. Control-path API: callable from any thread,
/// so it takes a raw port ID rather than a [`Port`] (unknown ports return an
/// error).
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

// ── Port-buffer discard xstats (by id) ──────────────────────────────────────

/// The 16 mlx5 xstat names of the NIC RX-overflow telemetry: per priority (0-7)
/// the port-buffer discard count (`rx_prio<n>_buf_discard_packets`) and the
/// congestion-management discard count (`rx_prio<n>_cong_discard_packets`). Not
/// every port exposes all 16; [`PortDiscardXstats::resolve`] skips missing ones.
fn candidate_discard_xstat_names() -> impl Iterator<Item = String> {
    (0..8).flat_map(|prio| {
        [
            format!("rx_prio{prio}_buf_discard_packets"),
            format!("rx_prio{prio}_cong_discard_packets"),
        ]
    })
}

/// A port's resolved port-buffer-discard xstat ids, cached so a read needs one
/// `rte_eth_xstats_get_by_id` call and no name lookup
/// (`rte_eth_xstats_get_id_by_name` is a string search, unsuitable for a fast path).
pub struct PortDiscardXstats {
    port: u16,
    ids: Vec<u64>,
}

impl PortDiscardXstats {
    /// Resolves which of [`candidate_discard_xstat_names`] exist on `port`: it
    /// fetches every xstat name (`rte_eth_xstats_get_names_by_id` with `ids =
    /// NULL`, first with a zero-length buffer to learn the count) and matches by
    /// name; the array index doubles as the id.
    pub fn resolve(port: u16) -> Result<Self, DpdkError> {
        let count = check(unsafe {
            dpdk_sys::rte_eth_xstats_get_names_by_id(
                port,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
            )
        })? as usize;

        let mut names: Vec<dpdk_sys::rte_eth_xstat_name> = vec![unsafe { mem::zeroed() }; count];
        let filled = check(unsafe {
            dpdk_sys::rte_eth_xstats_get_names_by_id(
                port,
                names.as_mut_ptr(),
                count as u32,
                std::ptr::null_mut(),
            )
        })? as usize;
        names.truncate(filled);

        let wanted: Vec<String> = candidate_discard_xstat_names().collect();
        let ids = names
            .iter()
            .enumerate()
            .filter_map(|(id, n)| {
                let name = unsafe { CStr::from_ptr(n.name.as_ptr()) }.to_string_lossy();
                wanted
                    .iter()
                    .any(|w| w == name.as_ref())
                    .then_some(id as u64)
            })
            .collect();

        Ok(PortDiscardXstats { port, ids })
    }

    /// Sums the current value of every resolved id with one
    /// `rte_eth_xstats_get_by_id` call. `None` if [`resolve`](Self::resolve)
    /// found no candidate on this port (some PMDs do not expose the counter) or
    /// the read failed: this feeds a best-effort telemetry poll.
    pub fn read(&self) -> Option<u64> {
        if self.ids.is_empty() {
            return None;
        }
        let mut values = vec![0u64; self.ids.len()];
        let filled = check(unsafe {
            dpdk_sys::rte_eth_xstats_get_by_id(
                self.port,
                self.ids.as_ptr(),
                values.as_mut_ptr(),
                self.ids.len() as u32,
            )
        })
        .ok()? as usize;
        Some(values[..filled].iter().sum())
    }
}

fn burst_mode_info(mode: &dpdk_sys::rte_eth_burst_mode) -> String {
    unsafe { CStr::from_ptr(mode.info.as_ptr()) }
        .to_string_lossy()
        .into_owned()
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
#[inline(always)]
pub fn rte_lcore_id() -> u32 {
    unsafe { dpdk_sys::rte_lcore_id() }
}

// ── Mbuf handling ────────────────────────────────────────────────────────────

/// Raw mbuf pointer, as used in burst buffers.
pub type RawMbuf = *mut dpdk_sys::rte_mbuf;

/// Frees a single mbuf back to its originating pool.
///
/// # Safety
/// `pkt` must be a valid, non-null mbuf pointer not aliased elsewhere.
#[inline(always)]
pub unsafe fn rte_pktmbuf_free(pkt: RawMbuf) {
    dpdk_sys::rte_pktmbuf_free(pkt);
}

/// Frees every mbuf in `mbufs` with `rte_pktmbuf_free_bulk`: each is returned
/// to its own originating pool, chained segments included, and entries may be
/// NULL. Unlike [`crate::mempool::MbufFreer::free_bulk`] this reads each mbuf
/// (refcount, `next`, `pool`), so it is correct for any mbuf, e.g. when the
/// port does not use `MBUF_FAST_FREE`. An empty slice does nothing.
///
/// # Safety
/// Every non-NULL entry must be a valid mbuf that is not used anywhere else
/// afterwards.
#[inline(always)]
pub unsafe fn rte_pktmbuf_free_bulk(mbufs: &mut [RawMbuf]) {
    if mbufs.is_empty() {
        return;
    }
    dpdk_sys::rte_pktmbuf_free_bulk(mbufs.as_mut_ptr(), mbufs.len() as u32);
}

// `tx_offloads_for` is a pure function of `tx_offload_capa`, so the decision is
// testable without an EAL; `Port::mbuf_fast_free` needs a live port and is not
// tested here.
#[cfg(test)]
mod tx_offload_tests {
    use super::tx_offloads_for;

    const FAST_FREE: u64 = dpdk_sys::RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE;

    #[test]
    fn tx_offloads_enable_fast_free_when_capable() {
        assert_eq!(tx_offloads_for(FAST_FREE), FAST_FREE);
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
