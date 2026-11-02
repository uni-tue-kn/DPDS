//! The single shutdown flag: the signal handler sets it, every worker loop
//! reads it at the top of every iteration, the main lcore polls it.
//!
//! All accesses are `Relaxed`: the flag publishes no data, each worker only
//! needs to see it eventually.

use std::sync::atomic::{AtomicBool, Ordering};

pub struct Quit(AtomicBool);

impl Quit {
    pub const fn new() -> Quit {
        Quit(AtomicBool::new(false))
    }

    /// Asks everything to stop (async-signal-safe: one atomic store).
    pub fn request(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl Default for Quit {
    fn default() -> Self {
        Quit::new()
    }
}

/// The process-wide flag used by the binary.
pub static QUIT: Quit = Quit::new();
