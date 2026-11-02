//! `Worker`: the per-direction handle `forward::run` consumes. Everything
//! the old `Worker::step` did (push, pop, rejects, counters) is exercised
//! through `forward::run` in `forward.rs`; what remains here is the
//! constructor, the accessors and the `Send` bound (the worker is moved to
//! its lcore).

use dpds::worker::Worker;
use dpds_core::buffer::Buffer;

use crate::support::*;

#[test]
fn a_worker_is_send_so_it_can_be_launched_on_a_worker_lcore() {
    fn assert_send<T: Send>() {}
    assert_send::<Worker>();
}

#[test]
fn a_new_worker_has_its_ports_and_no_backlog() {
    let w = Worker::new(Buffer::new(config(100, 0.0, 0.0)), 1, 0);
    assert_eq!(w.rx_port(), 1);
    assert_eq!(w.tx_port(), 0);
    assert!(!w.has_backlog());

    let w = Worker::new(Buffer::new(config(100, 0.0, 0.0)), 0, 0);
    assert_eq!((w.rx_port(), w.tx_port()), (0, 0));
}
