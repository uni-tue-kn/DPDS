//! Layouts that run several stages in one loop on one lcore.

use std::sync::atomic::Ordering::{Acquire, Release};

use dpdk::Ring;

use crate::consts::MAX_BURST_SIZE;
use crate::io::{dequeue_burst, MbufFree, RxBurst, TxBurst};
use crate::quit::Quit;
use crate::rx::Rx;
use crate::stats::{add, PipelineStats};
use crate::tx::Tx;
use crate::worker::Worker;

/// Worker + TX on one lcore (RX runs elsewhere and feeds `input`). Runs until
/// told to quit, the ring is empty and the delay buffer is drained, then
/// flushes TX and tells main.
pub fn run_worker_tx(
    mut worker: Worker,
    num_ports: u16,
    input: &Ring,
    nic: &mut impl TxBurst,
    free: &mut impl MbufFree,
    stats: &PipelineStats,
    quit: &Quit,
) {
    let mut tx = Tx::new(num_ports);
    let mut batch = [std::ptr::null_mut(); MAX_BURST_SIZE];
    let mut send_batch = [std::ptr::null_mut(); MAX_BURST_SIZE];
    loop {
        // Read before dequeuing, see `quit`.
        let quit_requested = quit.worker.load(Acquire);
        let n = dequeue_burst(input, &mut batch) as usize;
        let count = worker.step(
            &stats.worker,
            free,
            dpdk::now_ns,
            &batch[..n],
            &mut send_batch,
        );
        add(&stats.worker.sent, count as u64);
        tx.step(nic, free, &stats.tx, &send_batch[..count]);
        if quit_requested && n == 0 && !worker.has_backlog() {
            break;
        }
    }
    tx.cleanup(nic, free, &stats.tx);
    quit.transmit.store(true, Release);
}

/// RX + Worker + TX on one lcore, no rings. Receives until told to quit, then
/// runs until the delay buffer is drained, then flushes TX and tells main.
pub fn run_rx_worker_tx(
    mut worker: Worker,
    num_ports: u16,
    rx_nic: &mut impl RxBurst,
    tx_nic: &mut impl TxBurst,
    free: &mut impl MbufFree,
    stats: &PipelineStats,
    quit: &Quit,
) {
    let mut rx = Rx::new(num_ports);
    let mut tx = Tx::new(num_ports);
    let mut batch = [std::ptr::null_mut(); MAX_BURST_SIZE];
    let mut send_batch = [std::ptr::null_mut(); MAX_BURST_SIZE];
    loop {
        // Once told to quit, stop receiving and only drain the delay buffer:
        // on a live NIC, receiving on would refill it forever. Everything
        // received in an iteration goes through the worker in that same
        // iteration, so nothing is lost.
        let quit_requested = quit.receive.load(Acquire);
        let n = if quit_requested {
            0
        } else {
            rx.step(rx_nic, &stats.rx, &mut batch)
        };
        let count = worker.step(
            &stats.worker,
            free,
            dpdk::now_ns,
            &batch[..n],
            &mut send_batch,
        );
        add(&stats.worker.sent, count as u64);
        tx.step(tx_nic, free, &stats.tx, &send_batch[..count]);
        if quit_requested && !worker.has_backlog() {
            break;
        }
    }
    tx.cleanup(tx_nic, free, &stats.tx);
    quit.worker.store(true, Release);
    quit.transmit.store(true, Release);
}
