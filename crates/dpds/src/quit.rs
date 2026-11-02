//! Shutdown flags. The signal handler sets `receive`; every stage sets the
//! next stage's flag once it has finished, so the pipeline drains in order:
//! RX, then Worker, then TX, then main.
//!
//! A stage must read its flag BEFORE its last look at its input and stop only
//! if the flag was already set and that look found nothing: the previous
//! stage sets the flag only after its final enqueue, so nothing can arrive
//! after such an empty look.

use std::sync::atomic::{AtomicBool, Ordering};

pub struct Quit {
    pub receive: AtomicBool,
    pub worker: AtomicBool,
    pub transmit: AtomicBool,
}

impl Quit {
    pub const fn new() -> Quit {
        Quit {
            receive: AtomicBool::new(false),
            worker: AtomicBool::new(false),
            transmit: AtomicBool::new(false),
        }
    }

    /// Tells every stage to stop (used when the pipeline could not be launched completely).
    pub fn set_all(&self) {
        self.receive.store(true, Ordering::Release);
        self.worker.store(true, Ordering::Release);
        self.transmit.store(true, Ordering::Release);
    }
}

impl Default for Quit {
    fn default() -> Self {
        Quit::new()
    }
}

/// The process-wide flags used by the binary.
pub static QUIT: Quit = Quit::new();
