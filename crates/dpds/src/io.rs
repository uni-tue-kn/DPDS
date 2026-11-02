//! Thin traits over the NIC and the mempool, so the loop logic can be driven
//! by mocks without an EAL, plus their DPDK implementations.

use dpdk::port::rte_pktmbuf_free;
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

/// Return one mbuf to its mempool.
pub trait MbufFree {
    /// # Safety
    ///
    /// `mbuf` must be a valid mbuf that the caller owns (not enqueued, sent
    /// or freed yet); it must not be used afterwards.
    unsafe fn free(&mut self, mbuf: RawMbuf);
}

/// RX queue 0 of every port, indexed by port id.
pub struct NicRx(pub Vec<RxQueue<'static>>);

impl RxBurst for NicRx {
    #[inline(always)]
    fn rx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 {
        self.0[port as usize].rx_burst(bufs)
    }
}

/// TX queue 0 of every port, indexed by port id.
pub struct NicTx(pub Vec<TxQueue<'static>>);

impl TxBurst for NicTx {
    #[inline(always)]
    fn tx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 {
        self.0[port as usize].tx_burst(bufs)
    }
}

/// Frees mbufs with `rte_pktmbuf_free`.
pub struct PktmbufFree;

impl MbufFree for PktmbufFree {
    #[inline(always)]
    unsafe fn free(&mut self, mbuf: RawMbuf) {
        // SAFETY: the caller upholds the trait's contract.
        unsafe { rte_pktmbuf_free(mbuf) };
    }
}
