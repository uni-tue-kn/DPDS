//! Thin traits over the NIC, the mempool and the inter-stage rings, so the
//! stage logic can be driven by mocks without an EAL, plus their DPDK
//! implementations.

use std::sync::Arc;

use dpdk::port::rte_pktmbuf_free;
use dpdk::{RawMbuf, Ring, RxQueue, TxQueue};

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
    fn free(&mut self, mbuf: RawMbuf);
}

/// Enqueue into an inter-stage ring.
pub trait Enqueue {
    /// Enqueues as many of `bufs` as fit, in order; returns that count.
    fn enqueue_burst(&mut self, bufs: &[RawMbuf]) -> u16;
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
///
/// The stages only call [`MbufFree::free`] with mbufs they own (not yet
/// enqueued, sent or freed), which is what makes this safe in practice.
pub struct PktmbufFree;

#[inline(always)]
fn free_owned_mbuf(mbuf: RawMbuf) {
    // SAFETY: see `PktmbufFree`.
    unsafe { rte_pktmbuf_free(mbuf) };
}

impl MbufFree for PktmbufFree {
    #[inline(always)]
    fn free(&mut self, mbuf: RawMbuf) {
        free_owned_mbuf(mbuf);
    }
}

impl Enqueue for Arc<Ring> {
    #[inline(always)]
    fn enqueue_burst(&mut self, bufs: &[RawMbuf]) -> u16 {
        // SAFETY of the cast: `*mut rte_mbuf` and `*mut c_void` have the same layout.
        let objs = unsafe { std::slice::from_raw_parts(bufs.as_ptr().cast(), bufs.len()) };
        Ring::enqueue_burst(self, objs)
    }
}

/// Dequeues a burst of mbufs from an inter-stage ring.
#[inline(always)]
pub fn dequeue_burst(ring: &Ring, bufs: &mut [RawMbuf]) -> u16 {
    // SAFETY of the cast: `*mut rte_mbuf` and `*mut c_void` have the same layout.
    let objs = unsafe { std::slice::from_raw_parts_mut(bufs.as_mut_ptr().cast(), bufs.len()) };
    ring.dequeue_burst(objs)
}
