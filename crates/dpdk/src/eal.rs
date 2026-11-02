use std::ffi::{c_void, CString};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::DpdkError;

/// Set once the EAL has been initialised in this process.  Never reset:
/// DPDK does not support re-initialisation after `rte_eal_cleanup`.
static EAL_TAKEN: AtomicBool = AtomicBool::new(false);

/// RAII guard for the DPDK Environment Abstraction Layer.
///
/// `rte_eal_init` is called on construction; `rte_eal_cleanup` is called on
/// drop.  Only one `Eal` may exist per process lifetime (DPDK restriction) —
/// this is enforced at runtime.
pub struct Eal {
    // rte_eal_init keeps pointers into argv (and may permute the array), so
    // both the array and the strings must outlive it, i.e. live until
    // rte_eal_cleanup in Drop.  Field order is irrelevant: neither Vec is
    // touched on drop before rte_eal_cleanup runs.
    _argv: Vec<*mut c_char>,
    _args: Vec<CString>,
}

impl Eal {
    /// Initialise the EAL from the process's command-line arguments,
    /// unmodified -- callers are responsible for including any EAL flags
    /// they need (the first entry is the program name, as in `argv`).
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
            // rte_eal_init returns -1 and reports the cause in rte_errno.
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
// Each pipeline stage is launched as an ordinary Rust closure on its own
// worker lcore. `rte_eal_remote_launch` itself only knows
// `extern "C" fn(*mut c_void) -> i32`, so a boxed trait-object closure is
// smuggled through the `arg` pointer and unpacked by a single trampoline.

extern "C" fn launch_trampoline(arg: *mut c_void) -> i32 {
    // Safety: `arg` is always a pointer produced by `remote_launch` below,
    // consumed exactly once (DPDK calls this trampoline exactly once per
    // successful `rte_eal_remote_launch`).
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
        // Launch failed: the trampoline will never run, so reclaim the
        // box here instead of leaking it.
        drop(unsafe { Box::from_raw(arg as *mut Box<dyn FnOnce() + Send>) });
        return Err(DpdkError(ret));
    }
    Ok(())
}

/// Blocks until the function launched on `lcore_id` (via [`remote_launch`])
/// returns.
pub fn wait_lcore(lcore_id: u32) -> Result<(), DpdkError> {
    let ret = unsafe { dpdk_sys::rte_eal_wait_lcore(lcore_id) };
    if ret < 0 {
        Err(DpdkError(ret))
    } else {
        Ok(())
    }
}

/// Number of enabled lcores, the main lcore included (`rte_lcore_count`).
pub fn lcore_count() -> usize {
    unsafe { dpdk_sys::rte_lcore_count() as usize }
}

/// Iterator over every enabled worker lcore (i.e. every lcore except the
/// main lcore), mirroring `RTE_LCORE_FOREACH_WORKER` — a macro, so not
/// itself bindgen-reachable; reimplemented on top of the plain function
/// `rte_get_next_lcore` it expands to.
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

/// Iterate over every enabled worker lcore (equivalent to
/// `RTE_LCORE_FOREACH_WORKER`).
#[inline(always)]
pub fn foreach_worker() -> WorkerLcoreIter {
    WorkerLcoreIter { next: u32::MAX }
}
