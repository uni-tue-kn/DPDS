//! Worker stage: the delay / rate-limit / loss emulator in front of TX.

use std::sync::atomic::Ordering::{Acquire, Release};

use dpdk::mbuf::{pkt_len, port, prefetch_header, set_port};
use dpdk::{RawMbuf, Ring};
use dpds_core::buffer::Buffer;

use crate::consts::MAX_BURST_SIZE;
use crate::io::{dequeue_burst, Enqueue, MbufFree};
use crate::quit::Quit;
use crate::stats::{add, WorkerStats};

pub struct Worker {
    buffer: Buffer,
    num_ports: u16,
}

// SAFETY: the buffer's raw pointers are mbufs it exclusively owns until they
// are popped, and `Buffer` has no other thread-affine state, so the worker
// can be moved to the lcore that runs it.
unsafe impl Send for Worker {}

impl Worker {
    pub fn new(buffer: Buffer, num_ports: u16) -> Worker {
        Worker { buffer, num_ports }
    }

    /// Whether packets are still waiting in the delay buffer.
    pub fn has_backlog(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Ingests `in_batch` into the delay buffer (flipping the port with two
    /// ports, freeing what the buffer refuses as `emulator_drops`), then pops
    /// up to `MAX_BURST_SIZE` packets that are due into `out_batch`. Returns
    /// how many were popped; counting them as sent is up to the caller.
    ///
    /// All mbufs must be valid and owned by the caller.
    #[inline(always)]
    pub fn step(
        &mut self,
        stats: &WorkerStats,
        free: &mut impl MbufFree,
        mut now_ns: impl FnMut() -> f64,
        in_batch: &[RawMbuf],
        out_batch: &mut [RawMbuf; MAX_BURST_SIZE],
    ) -> usize {
        add(&stats.received, in_batch.len() as u64);

        if !in_batch.is_empty() {
            // One timestamp for the whole batch.
            let now = now_ns();
            for (i, &m) in in_batch.iter().enumerate() {
                if let Some(&ahead) = in_batch.get(i + 3) {
                    prefetch_header(ahead);
                }
                // SAFETY: valid mbuf owned by us (see above).
                let len = unsafe {
                    // With two NICs send through the other port; with one keep it.
                    if self.num_ports > 1 {
                        set_port(m, port(m) ^ 1);
                    }
                    pkt_len(m)
                };
                if !self.buffer.push(m.cast(), len, now) {
                    add(&stats.emulator_drops, 1);
                    free.free(m);
                }
            }
        }

        let max = MAX_BURST_SIZE.min(self.buffer.len());
        if max == 0 {
            return 0;
        }
        let now = now_ns();
        let mut count = 0;
        while count < max {
            match self.buffer.front_time() {
                Some(t) if t <= now => {}
                _ => break,
            }
            let Some(m) = self.buffer.pop() else { break };
            out_batch[count] = m.cast();
            count += 1;
        }
        count
    }
}

/// Hands released packets to the next stage; what does not fit is freed and
/// counted as `tx_ring_drops`.
#[inline(always)]
pub fn enqueue_burst(
    ring: &mut impl Enqueue,
    free: &mut impl MbufFree,
    stats: &WorkerStats,
    batch: &[RawMbuf],
) {
    let sent = ring.enqueue_burst(batch) as usize;
    add(&stats.sent, sent as u64);
    if sent < batch.len() {
        add(&stats.tx_ring_drops, (batch.len() - sent) as u64);
        for &m in &batch[sent..] {
            free.free(m);
        }
    }
}

/// Standalone worker lcore: ring in, ring out. Runs until told to quit, the
/// ring is empty and the delay buffer is drained, then tells TX.
pub fn run(
    mut worker: Worker,
    input: &Ring,
    output: &mut impl Enqueue,
    free: &mut impl MbufFree,
    stats: &WorkerStats,
    quit: &Quit,
) {
    let mut batch = [std::ptr::null_mut(); MAX_BURST_SIZE];
    let mut send_batch = [std::ptr::null_mut(); MAX_BURST_SIZE];
    loop {
        // Read before dequeuing, see `quit`.
        let quit_requested = quit.worker.load(Acquire);
        let n = dequeue_burst(input, &mut batch) as usize;
        let count = worker.step(stats, free, dpdk::now_ns, &batch[..n], &mut send_batch);
        if count > 0 {
            enqueue_burst(output, free, stats, &send_batch[..count]);
        }
        if quit_requested && n == 0 && !worker.has_backlog() {
            break;
        }
    }
    quit.transmit.store(true, Release);
}
