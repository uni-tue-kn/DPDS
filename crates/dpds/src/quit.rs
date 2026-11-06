//! The shutdown flag: the signal handler sets it, every worker loop and the
//! main lcore read it.
//!
//! Accesses are `Relaxed`: the flag publishes no data, workers only need to see
//! it eventually.

use std::sync::atomic::{AtomicBool, Ordering};

pub struct Quit(AtomicBool);

impl Quit {
    pub const fn new() -> Quit {
        Quit(AtomicBool::new(false))
    }

    /// Async-signal-safe: one atomic store.
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

/// The process-wide flag.
pub static QUIT: Quit = Quit::new();
