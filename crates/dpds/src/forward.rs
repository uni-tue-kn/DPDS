//! The data-plane loop of one direction (one worker lcore).

use dpdk::mbuf::{arrival_ns, pkt_len, prefetch_header, set_arrival_ns};
use dpds_core::buffer::PushOutcome;

use crate::consts::{BURST_SIZE, TX_BURST_SIZE, TX_MAX_WAIT_NS};
use crate::counters::frame_bucket;
use crate::delay_stats::DelayBatch;
use crate::first_loss::{DropCause, FirstLossReason, Stage, StageLossDetail};
use crate::io::{MbufFree, RxBurst, TxBurst};
use crate::quit::Quit;
use crate::stats::Telemetry;
use crate::worker::Worker;

/// First rejected push of the run: kept out of line so the loop stays tight.
#[cold]
#[inline(never)]
fn record_reject(t: &mut Telemetry<'_>, now: f64, cause: DropCause) {
    t.record_loss(
        FirstLossReason::DpdsReject,
        StageLossDetail::Reject {
            from: Stage::Dpds,
            cause,
        },
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

/// RX on `worker.rx_port()` -> delay/limit/loss buffer -> TX staging -> TX on
/// `worker.tx_port()` until told to quit, then drains the buffer (packets
/// leave at their send time, plus at most the staging wait) before returning.
///
/// One iteration, in this order:
/// 1. read `quit` (relaxed);
/// 2. read the clock once: `now` serves push, pop, tick, first-loss times and
///    the send decision;
/// 3. unless quit: one `rx_burst` of `BURST_SIZE`;
/// 4. push every received mbuf with its packet length
///    (`Buffer::push_outcome`); refused ones are collected, counted
///    (`dpds_rejects` and the cause: `loss_drops`, else `limiter_drops`, else
///    `buffer_full_drops`; `DpdsReject` first loss naming the cause) and
///    freed with one bulk free after the loop; with the delay statistics the
///    accepted ones get `now` (their arrival) written into their mbuf;
/// 5. pop the packets due at `now` into the free slots of the staging array
///    (`TX_BURST_SIZE` slots, kept across iterations): at most
///    `TX_BURST_SIZE - staged`, and at most `BURST_SIZE`;
/// 6. prefetch the header line of every popped mbuf and of the next
///    queued ones (see [`Buffer::peek_nth`](dpds_core::buffer::Buffer::peek_nth));
/// 7. if the array is full or its oldest packet (`staged[0]`, pops come out in
///    send-time order) is due for `TX_MAX_WAIT_NS` already, one `tx_burst` of
///    everything staged, also in an iteration that popped nothing; the unsent
///    tail is freed with one bulk free (`tx_short_calls`, `DescriptorFull`
///    first loss); the array is empty afterwards. With the delay statistics
///    the clock is read once more right before the `tx_burst` (the TX enqueue
///    time), the arrivals of the staged mbufs are read from the mbufs before
///    it (afterwards the driver owns them), and the delay and lateness of the
///    packets the NIC accepted are recorded (telemetry builds);
/// 8. sample `buffer_len_max` (after the pop), `t.tick(now, received)`;
/// 9. stop if quit was read as requested, the buffer is empty and nothing is
///    staged.
///
/// So once quit is observed nothing is received any more; everything received
/// before is emitted (at its send time, plus the staging wait) or freed. The
/// last partial batch is not flushed at quit: it goes out when its wait is
/// over.
///
/// With the direction's delay statistics switched off (`Stats::delay`,
/// `"delay_stats": false`) the loop is the instance without the statistics:
/// the choice is made once, before the loop, and the arrival stamp, the
/// extra clock read and the recording are not compiled into it (no
/// per-packet and no per-burst work). Without the `telemetry` feature the
/// statistics are never recorded, so the loop is that instance either way.
///
/// # Safety
///
/// `rx_nic` must hand out valid mbufs the caller owns (as `rte_eth_rx_burst`
/// does), and `tx_nic`/`free` must take over ownership as documented on
/// [`TxBurst`]/[`MbufFree`].
pub unsafe fn run(
    worker: Worker,
    rx_nic: &mut impl RxBurst,
    tx_nic: &mut impl TxBurst,
    free: &mut impl MbufFree,
    t: &mut Telemetry<'_>,
    quit: &Quit,
    now_ns: impl FnMut() -> f64,
) {
    // SAFETY: the caller's contract is the one of `run_loop`.
    unsafe {
        if cfg!(feature = "telemetry") && t.stats.delay.enabled() {
            run_loop::<true>(worker, rx_nic, tx_nic, free, t, quit, now_ns);
        } else {
            run_loop::<false>(worker, rx_nic, tx_nic, free, t, quit, now_ns);
        }
    }
}

/// The loop of [`run`]; `DELAY` is whether the delay statistics are recorded
/// (which costs the extra clock read before each `tx_burst`).
///
/// # Safety
///
/// As [`run`].
unsafe fn run_loop<const DELAY: bool>(
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
    // The TX staging array, its send times (`send[0]` decides when a partial
    // batch has to go) and, for the delay statistics, the arrivals read just
    // before `tx_burst`.
    let mut staged = [std::ptr::null_mut(); TX_BURST_SIZE];
    let mut send = [0u64; TX_BURST_SIZE];
    let mut arrivals = [0.0f64; TX_BURST_SIZE];
    let mut staged_len = 0;
    let mut delays = DelayBatch::new();
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

        // Frame sizes of this burst: at most BURST_SIZE (32) per bucket, so one
        // u64 of u8 counters on the stack, flushed once per burst.
        let mut sizes = [0u8; 8];
        let mut rejects = 0;
        let (mut lost, mut limited, mut full) = (0u64, 0u64, 0u64);
        for &m in &batch[..received] {
            // SAFETY: valid mbuf owned by us (see above).
            let len = unsafe { pkt_len(m) };
            sizes[frame_bucket(len)] += 1;
            let cause = match buffer.push_outcome(m.cast(), len, now) {
                PushOutcome::Accepted { .. } => {
                    if DELAY {
                        // SAFETY: valid mbuf owned by us, now in the buffer.
                        unsafe { set_arrival_ns(m, now) };
                    }
                    continue;
                }
                PushOutcome::Lost => {
                    lost += 1;
                    DropCause::Loss
                }
                PushOutcome::LimiterDrop => {
                    limited += 1;
                    DropCause::Limiter
                }
                PushOutcome::BufferFull => {
                    full += 1;
                    DropCause::BufferFull
                }
            };
            if rejects == 0 {
                record_reject(t, now, cause);
            }
            rejected[rejects] = m;
            rejects += 1;
        }
        if received > 0 {
            for (counter, &n) in stats.frame_sizes.0.iter().zip(&sizes) {
                counter.add(n as u64);
            }
        }
        if rejects > 0 {
            c.dpds_rejects.add(rejects as u64);
            c.loss_drops.add(lost);
            c.limiter_drops.add(limited);
            c.buffer_full_drops.add(full);
            // SAFETY: valid mbufs owned by us, the buffer refused them.
            unsafe { free.free_bulk(&mut rejected[..rejects]) };
        }

        let first_popped = staged_len;
        let limit = (staged_len + BURST_SIZE).min(TX_BURST_SIZE);
        while staged_len < limit {
            let Some((m, send_time_ns)) = buffer.pop_due_with_time(now) else {
                break;
            };
            staged[staged_len] = m.cast();
            send[staged_len] = send_time_ns;
            staged_len += 1;
        }
        let popped_len = staged_len - first_popped;
        c.out_of_order.set(buffer.out_of_order());
        if popped_len > 0 {
            c.pops.add(popped_len as u64);
            if DELAY {
                stats
                    .delay
                    .record_pops(&mut delays, &send[first_popped..staged_len]);
            }
            for &m in &staged[first_popped..staged_len] {
                prefetch_header(m);
            }
            for i in 0..popped_len {
                match buffer.peek_nth(i) {
                    Some(next) => prefetch_header(next.cast()),
                    None => break,
                }
            }
        }

        if staged_len > 0 && (staged_len == TX_BURST_SIZE || now - send[0] as f64 >= TX_MAX_WAIT_NS)
        {
            let n = staged_len;
            let batch = &mut staged[..n];
            let tx_ns = if DELAY {
                for (a, &m) in arrivals.iter_mut().zip(batch.iter()) {
                    // SAFETY: valid mbuf owned by us, stamped at its push.
                    *a = unsafe { arrival_ns(m) };
                }
                now_ns()
            } else {
                now
            };
            let sent = (tx_nic.tx_burst(tx_port, batch) as usize).min(n);
            c.tx_calls.inc();
            c.tx_pkts.add(n as u64);
            c.tx_batch_max.observe(n as u64);
            c.tx_sent.add(sent as u64);
            if sent < n {
                c.tx_short_calls.inc();
                record_descriptor_full(t, now);
                // SAFETY: the NIC did not take these mbufs, we still own them.
                unsafe { free.free_bulk(&mut batch[sent..]) };
            }
            if DELAY {
                stats
                    .delay
                    .record_sent(&send[..sent], &arrivals[..sent], tx_ns);
            }
            staged_len = 0;
        }

        // Sampled once per iteration after the pop, like preof's link stages.
        c.buffer_len_max.observe_with(|| buffer.len() as u64);
        // Also on idle iterations: an epoch only ends in an idle gap.
        t.tick(now, received as u64);
        if quit_requested && buffer.is_empty() && staged_len == 0 {
            break;
        }
    }
}
