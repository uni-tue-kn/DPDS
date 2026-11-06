//! One direction's handle for [`forward::run`](crate::forward::run): its delay
//! buffer and its two ports.

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

    /// RX port (queue 0).
    pub fn rx_port(&self) -> u16 {
        self.rx_port
    }

    /// TX port (queue 0).
    pub fn tx_port(&self) -> u16 {
        self.tx_port
    }

    /// Whether packets still wait in the delay buffer.
    pub fn has_backlog(&self) -> bool {
        !self.buffer.is_empty()
    }
}
