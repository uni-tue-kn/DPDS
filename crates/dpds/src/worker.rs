//! Worker step: the delay / rate-limit / loss emulator between RX and TX.

use dpdk::mbuf::{pkt_len, port, prefetch_header, set_port};
use dpdk::RawMbuf;
use dpds_core::buffer::Buffer;

use crate::consts::MAX_BURST_SIZE;
use crate::first_loss::{FirstLossReason, Stage, StageLossDetail};
use crate::io::MbufFree;
use crate::stats::Telemetry;

pub struct Worker {
    buffer: Buffer,
    num_ports: u16,
}

// SAFETY: the buffer's raw pointers are mbufs it exclusively owns until they
// are popped, and `Buffer` has no other thread-affine state.
unsafe impl Send for Worker {}

impl Worker {
    pub fn new(buffer: Buffer, num_ports: u16) -> Worker {
        Worker { buffer, num_ports }
    }

    /// Number of ports (1 or 2) this worker was built for.
    pub fn num_ports(&self) -> u16 {
        self.num_ports
    }

    /// Whether packets are still waiting in the delay buffer.
    pub fn has_backlog(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Ingests `in_batch` into the delay buffer (flipping the port with two
    /// ports; a refused push frees the mbuf, counts `emulator_rejects` and
    /// records an `EmulatorReject` first loss), pops up to
    /// `MAX_BURST_SIZE` due packets into `out_batch`, counts them as `pops`,
    /// samples `buffer_len_max` once (after the pop, as preof does) and
    /// returns how many.
    ///
    /// # Safety
    ///
    /// Every mbuf in `in_batch` must be valid and owned by the caller;
    /// ownership passes to the worker (it frees it or hands it back in
    /// `out_batch`).
    pub unsafe fn step(
        &mut self,
        t: &mut Telemetry<'_>,
        free: &mut impl MbufFree,
        mut now_ns: impl FnMut() -> f64,
        in_batch: &[RawMbuf],
        out_batch: &mut [RawMbuf; MAX_BURST_SIZE],
    ) -> usize {
        let c = &t.stats.counters;

        if !in_batch.is_empty() {
            // One timestamp for the whole batch.
            let now = now_ns();
            let mut rejects = 0u64;
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
                    if rejects == 0 {
                        t.record_loss(
                            FirstLossReason::EmulatorReject,
                            StageLossDetail::Single { from: Stage::Dpds },
                            now,
                        );
                    }
                    rejects += 1;
                    // SAFETY: valid mbuf owned by us, the buffer refused it.
                    unsafe { free.free(m) };
                }
            }
            c.emulator_rejects.add(rejects);
        }

        let max = MAX_BURST_SIZE.min(self.buffer.len());
        let mut count = 0;
        if max > 0 {
            let now = now_ns();
            while count < max {
                match self.buffer.front_time() {
                    Some(t) if t <= now => {}
                    _ => break,
                }
                let Some(m) = self.buffer.pop() else { break };
                out_batch[count] = m.cast();
                count += 1;
            }
            c.pops.add(count as u64);
        }

        // Sampled once per step after the pop, like preof's link stages.
        c.buffer_len_max.observe_with(|| self.buffer.len() as u64);
        count
    }
}
