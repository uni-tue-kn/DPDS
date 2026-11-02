//! One direction's handle for [`forward::run`](crate::forward::run): its
//! delay buffer and the two ports it uses.

use dpds_core::buffer::Buffer;

pub struct Worker {
    pub(crate) buffer: Buffer,
    rx_port: u16,
    tx_port: u16,
}

// SAFETY: the buffer's raw pointers are mbufs it exclusively owns until they
// are popped, and `Buffer` has no other thread-affine state.
unsafe impl Send for Worker {}

impl Worker {
    pub fn new(buffer: Buffer, rx_port: u16, tx_port: u16) -> Worker {
        Worker {
            buffer,
            rx_port,
            tx_port,
        }
    }

    /// The port this direction receives on (RX queue 0).
    pub fn rx_port(&self) -> u16 {
        self.rx_port
    }

    /// The port this direction transmits on (TX queue 0).
    pub fn tx_port(&self) -> u16 {
        self.tx_port
    }

    /// Whether packets are still waiting in the delay buffer.
    pub fn has_backlog(&self) -> bool {
        !self.buffer.is_empty()
    }
}
