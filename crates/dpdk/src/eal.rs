use std::ffi::{c_void, CString};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::DpdkError;

/// Set once the EAL is initialised; never reset: DPDK does not support
/// re-initialisation after `rte_eal_cleanup`.
static EAL_TAKEN: AtomicBool = AtomicBool::new(false);

/// RAII guard for the DPDK Environment Abstraction Layer.
///
/// `rte_eal_init` runs on construction, `rte_eal_cleanup` on drop. Only one
/// `Eal` may exist per process lifetime (DPDK restriction), enforced at runtime.
pub struct Eal {
    // rte_eal_init keeps pointers into argv (and may permute it), so the array and
    // the strings must live until rte_eal_cleanup in Drop.
    _argv: Vec<*mut c_char>,
    _args: Vec<CString>,
}

impl Eal {
    /// Initialises the EAL from `args` as given: the caller includes the EAL
    /// flags it needs, and the first entry is the program name, as in `argv`.
    pub fn new(args: &[String]) -> Result<Self, DpdkError> {
        if EAL_TAKEN.swap(true, Ordering::SeqCst) {
            return Err(DpdkError(-114)); // -EALREADY
        }

        let c_strings: Vec<CString> = args
            .iter()
            .map(|s| CString::new(s.as_str()).expect("arg contains interior NUL"))
            .collect();
        let mut c_ptrs: Vec<*mut c_char> = c_strings
            .iter()
            .map(|s| s.as_ptr() as *mut c_char)
            .collect();

        let ret = unsafe { dpdk_sys::rte_eal_init(c_ptrs.len() as i32, c_ptrs.as_mut_ptr()) };
        if ret < 0 {
            // rte_eal_init returns -1 and sets rte_errno.
            return Err(DpdkError::from_rte_errno());
        }

        Ok(Eal {
            _argv: c_ptrs,
            _args: c_strings,
        })
    }
}

impl Drop for Eal {
    fn drop(&mut self) {
        unsafe { dpdk_sys::rte_eal_cleanup() };
    }
}

// ── Multi-core launch ────────────────────────────────────────────────────────
//
// `rte_eal_remote_launch` only takes `extern "C" fn(*mut c_void) -> i32`, so a
// boxed closure is passed through `arg` and unpacked by one trampoline.

extern "C" fn launch_trampoline(arg: *mut c_void) -> i32 {
    // Safety: `arg` is always a pointer produced by `remote_launch` below,
    // consumed exactly once (DPDK calls the trampoline once per successful
    // `rte_eal_remote_launch`).
    let f = unsafe { Box::from_raw(arg as *mut Box<dyn FnOnce() + Send>) };
    f();
    0
}

/// Runs `f` to completion on `lcore_id`, which must currently be idle
/// (`WAIT` state). Join it with [`wait_lcore`].
pub fn remote_launch(f: impl FnOnce() + Send + 'static, lcore_id: u32) -> Result<(), DpdkError> {
    let boxed: Box<dyn FnOnce() + Send> = Box::new(f);
    let arg = Box::into_raw(Box::new(boxed)) as *mut c_void;

    let ret = unsafe { dpdk_sys::rte_eal_remote_launch(Some(launch_trampoline), arg, lcore_id) };
    if ret != 0 {
        // The trampoline will never run: reclaim the box instead of leaking it.
        drop(unsafe { Box::from_raw(arg as *mut Box<dyn FnOnce() + Send>) });
        return Err(DpdkError(ret));
    }
    Ok(())
}

/// Blocks until the function launched on `lcore_id` returns.
pub fn wait_lcore(lcore_id: u32) -> Result<(), DpdkError> {
    let ret = unsafe { dpdk_sys::rte_eal_wait_lcore(lcore_id) };
    if ret < 0 {
        Err(DpdkError(ret))
    } else {
        Ok(())
    }
}

/// Number of enabled lcores, the main lcore included.
pub fn lcore_count() -> usize {
    unsafe { dpdk_sys::rte_lcore_count() as usize }
}

pub fn main_lcore() -> u32 {
    unsafe { dpdk_sys::rte_get_main_lcore() }
}

/// Iterator over every enabled worker lcore (all but the main one):
/// `RTE_LCORE_FOREACH_WORKER` is a macro bindgen cannot reach, so this wraps
/// `rte_get_next_lcore`.
pub struct WorkerLcoreIter {
    next: u32,
}

impl Iterator for WorkerLcoreIter {
    type Item = u32;

    #[inline(always)]
    fn next(&mut self) -> Option<u32> {
        let skip_main = 1;
        let wrap = 0;
        let i = unsafe { dpdk_sys::rte_get_next_lcore(self.next, skip_main, wrap) };
        if i >= dpdk_sys::RTE_MAX_LCORE {
            None
        } else {
            self.next = i;
            Some(i)
        }
    }
}

#[inline(always)]
pub fn foreach_worker() -> WorkerLcoreIter {
    WorkerLcoreIter { next: u32::MAX }
}
