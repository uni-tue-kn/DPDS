use std::ffi::CStr;
use std::{error, fmt};

/// A negative errno-style DPDK return code (e.g. `-19` for `ENODEV`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DpdkError(pub i32);

impl DpdkError {
    /// Builds an error from the calling lcore's `rte_errno`, for DPDK APIs
    /// (`rte_eal_init`, `rte_pktmbuf_pool_create`, ...) that report the cause
    /// there instead of in the return code.
    pub(crate) fn from_rte_errno() -> Self {
        DpdkError(-unsafe { dpdk_sys::rte_errno_value() })
    }
}

impl fmt::Display for DpdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // rte_strerror takes a positive errno value, understands DPDK codes
        // (E_RTE_*), is safe before rte_eal_init and returns a per-thread static buffer.
        let msg = unsafe { CStr::from_ptr(dpdk_sys::rte_strerror(self.0.abs())) };
        write!(f, "{} (code {})", msg.to_string_lossy(), self.0)
    }
}

impl error::Error for DpdkError {}

#[inline]
pub(crate) fn check(ret: i32) -> Result<i32, DpdkError> {
    if ret < 0 {
        Err(DpdkError(ret))
    } else {
        Ok(ret)
    }
}
