//! The data-plane loop of one direction (one worker lcore).

use dpdk::mbuf::{pkt_len, prefetch_header};

use crate::consts::BURST_SIZE;
use crate::first_loss::{FirstLossReason, Stage, StageLossDetail};
use crate::io::{MbufFree, RxBurst, TxBurst};
use crate::quit::Quit;
use crate::stats::Telemetry;
use crate::worker::Worker;

/// First rejected push of the run: kept out of line so the loop stays tight.
#[cold]
#[inline(never)]
fn record_reject(t: &mut Telemetry<'_>, now: f64) {
    t.record_loss(
        FirstLossReason::DpdsReject,
        StageLossDetail::Single { from: Stage::Dpds },
        now,
    );
}

/// First short `tx_burst` of the run, see [`record_reject`].
#[cold]
#[inline(never)]
fn record_descriptor_full(t: &mut Telemetry<'_>, now: f64) {
    t.record_loss(
        FirstLossReason::DescriptorFull,
        StageLossDetail::RingFull {
            from: Stage::Dpds,
            to: Stage::Nic,
        },
        now,
    );
}

/// RX on `worker.rx_port()` -> delay/limit/loss buffer -> TX on
/// `worker.tx_port()` until told to quit, then drains the buffer (packets
/// leave at their send time) before returning.
///
/// One iteration, in this order:
/// 1. read `quit` (relaxed);
/// 2. read the clock once: `now` serves push, pop, tick and first-loss
///    times, nothing else in the loop reads a clock;
/// 3. unless quit: one `rx_burst` of `BURST_SIZE`;
/// 4. push every received mbuf with its packet length; refused ones are
///    collected, counted (`dpds_rejects`, `DpdsReject` first loss)
///    and freed with one bulk free after the loop;
/// 5. pop up to `BURST_SIZE` packets due at `now`;
/// 6. prefetch the header line of every popped mbuf and of the next
///    queued ones (see [`Buffer::peek_nth`](dpds_core::buffer::Buffer::peek_nth));
/// 7. if anything was popped, one `tx_burst` in the same iteration; the
///    unsent tail is freed with one bulk free (`tx_short_calls`,
///    `DescriptorFull` first loss);
/// 8. sample `buffer_len_max` (after the pop), `t.tick(now, received)`;
/// 9. stop if quit was read as requested and the buffer is empty.
///
/// So once quit is observed nothing is received any more; everything received
/// before is emitted (at its send time) or freed.
///
/// # Safety
///
/// `rx_nic` must hand out valid mbufs the caller owns (as `rte_eth_rx_burst`
/// does), and `tx_nic`/`free` must take over ownership as documented on
/// [`TxBurst`]/[`MbufFree`].
pub unsafe fn run(
    mut worker: Worker,
    rx_nic: &mut impl RxBurst,
    tx_nic: &mut impl TxBurst,
    free: &mut impl MbufFree,
    t: &mut Telemetry<'_>,
    quit: &Quit,
    mut now_ns: impl FnMut() -> f64,
) {
    let (rx_port, tx_port) = (worker.rx_port(), worker.tx_port());
    let buffer = &mut worker.buffer;
    let stats = t.stats;
    let c = &stats.counters;
    let mut batch = [std::ptr::null_mut(); BURST_SIZE];
    let mut rejected = [std::ptr::null_mut(); BURST_SIZE];
    let mut popped = [std::ptr::null_mut(); BURST_SIZE];
    loop {
        let quit_requested = quit.is_requested();
        let now = now_ns();
        c.polls.inc();

        // Once told to quit, stop receiving and only drain the delay buffer:
        // on a live NIC, receiving on would refill it forever.
        let received = if quit_requested {
            0
        } else {
            (rx_nic.rx_burst(rx_port, &mut batch) as usize).min(BURST_SIZE)
        };
        c.rx_pkts.add(received as u64);
        if received == BURST_SIZE {
            c.rx_full_bursts.inc();
        }

        let mut rejects = 0;
        for &m in &batch[..received] {
            // SAFETY: valid mbuf owned by us (see above).
            let len = unsafe { pkt_len(m) };
            if !buffer.push(m.cast(), len, now) {
                if rejects == 0 {
                    record_reject(t, now);
                }
                rejected[rejects] = m;
                rejects += 1;
            }
        }
        if rejects > 0 {
            c.dpds_rejects.add(rejects as u64);
            // SAFETY: valid mbufs owned by us, the buffer refused them.
            unsafe { free.free_bulk(&mut rejected[..rejects]) };
        }

        let mut popped_len = 0;
        while popped_len < BURST_SIZE {
            let Some(m) = buffer.pop_due(now) else { break };
            popped[popped_len] = m.cast();
            popped_len += 1;
        }
        let popped_len = popped_len.min(BURST_SIZE);
        if popped_len > 0 {
            c.pops.add(popped_len as u64);
            let popped = &mut popped[..popped_len];
            for &m in popped.iter() {
                prefetch_header(m);
            }
            for i in 0..popped_len {
                match buffer.peek_nth(i) {
                    Some(next) => prefetch_header(next.cast()),
                    None => break,
                }
            }

            let sent = (tx_nic.tx_burst(tx_port, popped) as usize).min(popped_len);
            c.tx_calls.inc();
            c.tx_pkts.add(popped_len as u64);
            c.tx_batch_max.observe(popped_len as u64);
            c.tx_sent.add(sent as u64);
            if sent < popped_len {
                c.tx_short_calls.inc();
                record_descriptor_full(t, now);
                // SAFETY: the NIC did not take these mbufs, we still own them.
                unsafe { free.free_bulk(&mut popped[sent..]) };
            }
        }

        // Sampled once per iteration after the pop, like preof's link stages.
        c.buffer_len_max.observe_with(|| buffer.len() as u64);
        // Also on idle iterations: an epoch only ends in an idle gap.
        t.tick(now, received as u64);
        if quit_requested && buffer.is_empty() {
            break;
        }
    }
}
