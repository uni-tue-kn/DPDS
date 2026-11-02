//! Thin traits over the NIC and the mempool, so the loop logic can be driven
//! by mocks without an EAL, plus their DPDK implementations.

use dpdk::mempool::MbufFreer;
use dpdk::port::rte_pktmbuf_free_bulk;
use dpdk::{RawMbuf, RxQueue, TxQueue};

/// Receive a burst on queue 0 of `port`.
pub trait RxBurst {
    /// Fills `bufs` from the front; returns how many mbufs were received.
    fn rx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16;
}

/// Transmit a burst on queue 0 of `port`.
pub trait TxBurst {
    /// Returns how many mbufs (from the front) the NIC accepted; the caller
    /// still owns the rest.
    fn tx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16;
}

/// Return mbufs to their mempool.
pub trait MbufFree {
    /// Frees every mbuf in `mbufs` (never called with an empty slice).
    ///
    /// # Safety
    ///
    /// Every entry must be a valid mbuf that the caller owns (not enqueued,
    /// sent or freed yet); none of them may be used afterwards.
    unsafe fn free_bulk(&mut self, mbufs: &mut [RawMbuf]);
}

/// RX queue 0 of the direction's RX port.
pub struct NicRx {
    port: u16,
    queue: RxQueue<'static>,
}

impl NicRx {
    pub fn new(port: u16, queue: RxQueue<'static>) -> NicRx {
        NicRx { port, queue }
    }
}

impl RxBurst for NicRx {
    #[inline(always)]
    fn rx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 {
        debug_assert_eq!(port, self.port);
        self.queue.rx_burst(bufs)
    }
}

/// TX queue 0 of the direction's egress port.
pub struct NicTx {
    port: u16,
    queue: TxQueue<'static>,
}

impl NicTx {
    pub fn new(port: u16, queue: TxQueue<'static>) -> NicTx {
        NicTx { port, queue }
    }
}

impl TxBurst for NicTx {
    #[inline(always)]
    fn tx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 {
        debug_assert_eq!(port, self.port);
        self.queue.tx_burst(bufs)
    }
}

/// Frees mbufs with `rte_pktmbuf_free_bulk`: correct for any mbuf (reads
/// refcount, segments and pool of each one).
pub struct PktmbufFree;

impl MbufFree for PktmbufFree {
    #[inline(always)]
    unsafe fn free_bulk(&mut self, mbufs: &mut [RawMbuf]) {
        // SAFETY: the caller upholds the trait's contract.
        unsafe { rte_pktmbuf_free_bulk(mbufs) };
    }
}

/// Frees mbufs straight back into one mempool with `rte_mbuf_raw_free_bulk`
/// without reading them. Only valid when every mbuf of the direction is
/// direct, single-segment, refcount 1 and from this pool. The TX port's
/// `MBUF_FAST_FREE` promises that for the mbufs the NIC did not accept; the
/// rejected RX mbufs are single-segment because RX scatter is off and the data
/// room fits the largest frame.
pub struct RawFree(pub MbufFreer);

impl MbufFree for RawFree {
    #[inline(always)]
    unsafe fn free_bulk(&mut self, mbufs: &mut [RawMbuf]) {
        // SAFETY: the caller upholds the trait's contract; the construction
        // of `RawFree` promises the pool/refcount/segment conditions.
        unsafe { self.0.free_bulk(mbufs) };
    }
}
