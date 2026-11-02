//! The fused data-plane loop on the single worker lcore.

use crate::consts::MAX_BURST_SIZE;
use crate::io::{MbufFree, RxBurst, TxBurst};
use crate::quit::Quit;
use crate::rx::Rx;
use crate::stats::Telemetry;
use crate::tx::Tx;
use crate::worker::Worker;

/// RX -> worker -> TX until told to quit, then drains the delay buffer
/// completely and flushes TX before returning.
///
/// One iteration: read `quit` FIRST; `polls += 1`; unless quit was already
/// requested, one `Rx::step`; `Worker::step` with that batch (also run when
/// the batch is empty); `Tx::step` with the popped packets; `t.tick(now,
/// received)`; then stop if quit was read as requested and the worker has no
/// backlog. After the loop `Tx::cleanup`. So once quit is observed nothing
/// is received any more, everything received before is emitted (at its
/// send time) or freed.
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
    let mut rx = Rx::new(worker.num_ports());
    let mut tx = Tx::new(worker.num_ports());
    let mut batch = [std::ptr::null_mut(); MAX_BURST_SIZE];
    let mut popped = [std::ptr::null_mut(); MAX_BURST_SIZE];
    loop {
        // Read first, so everything received after this point still goes
        // through the worker in the same iteration.
        let quit_requested = quit.is_requested();
        t.stats.counters.polls.inc();
        // Once told to quit, stop receiving and only drain the delay buffer:
        // on a live NIC, receiving on would refill it forever.
        let received = if quit_requested {
            0
        } else {
            rx.step(rx_nic, t, &mut batch)
        };
        // SAFETY: the caller guarantees valid, owned mbufs from `rx_nic`.
        let count = unsafe { worker.step(t, free, &mut now_ns, &batch[..received], &mut popped) };
        // SAFETY: as above; the popped mbufs are owned by us.
        unsafe { tx.step(tx_nic, free, t, &mut now_ns, &popped[..count]) };
        // Also on idle iterations: an epoch only ends in an idle gap. Gated so
        // the feature-off build does not read the clock for nothing.
        #[cfg(feature = "telemetry")]
        t.tick(now_ns(), received as u64);
        if quit_requested && !worker.has_backlog() {
            break;
        }
    }
    tx.cleanup(tx_nic, free, t, now_ns);
}
