//! RX stage: poll the ports round-robin.

use std::sync::atomic::Ordering::{Acquire, Release};

use dpdk::RawMbuf;

use crate::consts::MAX_BURST_SIZE;
use crate::io::{Enqueue, MbufFree, RxBurst};
use crate::quit::Quit;
use crate::stats::{add, burst_bucket, RxStats};

pub struct Rx {
    port: u16,
    num_ports: u16,
}

impl Rx {
    pub fn new(num_ports: u16) -> Rx {
        Rx { port: 0, num_ports }
    }

    /// The port the next [`step`](Rx::step) polls.
    pub fn next_port(&self) -> u16 {
        self.port
    }

    /// Polls queue 0 of the current port once, then moves on to the next
    /// port (even if nothing was received). Returns the number of mbufs
    /// written to the front of `batch`.
    #[inline(always)]
    pub fn step(
        &mut self,
        q: &mut impl RxBurst,
        stats: &RxStats,
        batch: &mut [RawMbuf; MAX_BURST_SIZE],
    ) -> usize {
        let n = q.rx_burst(self.port, &mut batch[..]) as usize;
        if n > 0 {
            add(&stats.received, n as u64);
            add(&stats.burst_hist[burst_bucket(n)], 1);
        }
        self.port += 1;
        if self.port == self.num_ports {
            self.port = 0;
        }
        n
    }
}

/// Hands a received batch to the next stage; what does not fit is freed and
/// counted as `rx_ring_drops`.
#[inline(always)]
pub fn enqueue_burst(
    ring: &mut impl Enqueue,
    free: &mut impl MbufFree,
    stats: &RxStats,
    batch: &[RawMbuf],
) {
    let sent = ring.enqueue_burst(batch) as usize;
    add(&stats.enqueued, sent as u64);
    if sent < batch.len() {
        add(&stats.rx_ring_drops, (batch.len() - sent) as u64);
        for &m in &batch[sent..] {
            free.free(m);
        }
    }
}

/// Standalone RX lcore: receive and enqueue until told to quit, then tell the worker.
pub fn run(
    num_ports: u16,
    nic: &mut impl RxBurst,
    ring: &mut impl Enqueue,
    free: &mut impl MbufFree,
    stats: &RxStats,
    quit: &Quit,
) {
    let mut rx = Rx::new(num_ports);
    let mut batch = [std::ptr::null_mut(); MAX_BURST_SIZE];
    while !quit.receive.load(Acquire) {
        let n = rx.step(nic, stats, &mut batch);
        if n > 0 {
            enqueue_burst(ring, free, stats, &batch[..n]);
        }
    }
    quit.worker.store(true, Release);
}
