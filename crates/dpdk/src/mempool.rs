use std::ffi::CString;
use std::ptr::NonNull;

use crate::error::DpdkError;

/// Owned handle to an `rte_mempool` (packet buffer pool).
///
/// The pool is freed via `rte_mempool_free` on drop.
pub struct Mempool {
    ptr: NonNull<dpdk_sys::rte_mempool>,
}

impl Mempool {
    /// Create a packet-buffer (mbuf) pool.
    ///
    /// # Arguments
    /// * `name`          – unique pool name
    /// * `n`             – total number of mbufs
    /// * `cache_size`    – per-lcore cache size
    /// * `data_room_size`– size of the data buffer in each mbuf (use
    ///   [`Mempool::DEFAULT_BUF_SIZE`] for the DPDK default)
    /// * `socket_id`     – NUMA socket; pass `rte_socket_id()` for the
    ///   current socket
    pub fn create(
        name: &str,
        n: u32,
        cache_size: u32,
        data_room_size: u16,
        socket_id: u32,
    ) -> Result<Self, DpdkError> {
        let c_name = CString::new(name).expect("pool name contains interior NUL");
        let ptr = unsafe {
            dpdk_sys::rte_pktmbuf_pool_create(
                c_name.as_ptr(),
                n,
                cache_size,
                0, // priv_size – per-packet metadata travels next to the pointer (ring elements), never in the mbuf
                data_room_size,
                socket_id as i32,
            )
        };
        NonNull::new(ptr)
            .map(|ptr| Mempool { ptr })
            .ok_or_else(DpdkError::from_rte_errno)
    }

    /// DPDK's default mbuf data-room size (`RTE_MBUF_DEFAULT_BUF_SIZE`).
    pub const DEFAULT_BUF_SIZE: u16 = dpdk_sys::RTE_MBUF_DEFAULT_BUF_SIZE as u16;

    #[inline(always)]
    pub(crate) fn as_ptr(&self) -> *mut dpdk_sys::rte_mempool {
        self.ptr.as_ptr()
    }

    /// A [`MbufFreer`] for this pool, to hand to worker lcores (`Mempool`
    /// itself is `!Sync`, so a `&Mempool` can't be shared across them).
    /// Calls nothing in DPDK.
    #[inline(always)]
    pub fn freer(&self) -> MbufFreer {
        MbufFreer { pool: self.ptr }
    }
}

/// Copyable handle that returns mbufs to one [`Mempool`] without reading
/// them: [`MbufFreer::free_bulk`] uses `rte_mbuf_raw_free_bulk`, which only
/// puts the pointers into the calling lcore's mempool cache, whereas
/// `rte_pktmbuf_free_bulk` reads each mbuf's `next`, `refcnt`, `ol_flags`,
/// `nb_segs` and `pool` (cache lines 0 and 1) to find out how to free it.
///
/// Holds a bare pool pointer with no lifetime: the caller must keep the
/// pool alive for as long as any copy is used (e.g. by leaking it for
/// `'static`).
#[derive(Clone, Copy)]
pub struct MbufFreer {
    pool: NonNull<dpdk_sys::rte_mempool>,
}

// SAFETY: `MbufFreer` only carries the pool pointer. `rte_mempool_put_bulk`
// (behind `rte_mbuf_raw_free_bulk`) goes through the calling lcore's own
// cache and is multi-producer safe for a pool created without
// `RTE_MEMPOOL_F_SP_PUT` (`rte_pktmbuf_pool_create` never sets it), so
// putting from any lcore is sound. Keeping the pool alive is the caller's job (see
// the type's doc comment).
unsafe impl Send for MbufFreer {}

impl MbufFreer {
    /// Returns every mbuf in `mbufs` to this freer's pool with
    /// `rte_mbuf_raw_free_bulk`, without reading any of them. An empty
    /// slice returns without calling into DPDK (the bulk put requires a
    /// count of at least 1).
    ///
    /// # Safety
    /// The pool this freer came from must still be alive (`MbufFreer`
    /// carries no lifetime, see the type's doc comment). Every entry must
    /// be a non-NULL mbuf from this pool that is direct,
    /// refcnt 1, single segment (`nb_segs` 1, `next` NULL) and not used
    /// anywhere else afterwards -- the same promise
    /// `RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE` makes to a driver. With
    /// `RTE_LIBRTE_MBUF_DEBUG` off, DPDK checks none of this.
    #[inline(always)]
    pub unsafe fn free_bulk(self, mbufs: &mut [crate::port::RawMbuf]) {
        if mbufs.is_empty() {
            return;
        }
        debug_assert!(
            mbufs.iter().all(|m| !m.is_null()),
            "MbufFreer::free_bulk: NULL mbuf in batch"
        );
        dpdk_sys::rte_mbuf_raw_free_bulk(
            self.pool.as_ptr(),
            mbufs.as_mut_ptr(),
            mbufs.len() as u32,
        );
    }
}

impl Drop for Mempool {
    fn drop(&mut self) {
        unsafe { dpdk_sys::rte_mempool_free(self.ptr.as_ptr()) };
    }
}

// Safety: rte_mempool is reference-counted inside DPDK; we enforce exclusive
// ownership on the Rust side through the non-Copy Mempool struct.
unsafe impl Send for Mempool {}

// ── `MbufFreer` ─────────────────────────────────────────────────────────────
//
// `Mempool::freer` returns a small `Copy` handle (`Send`, so it can be moved
// into a worker lcore's closure) whose `free_bulk` returns every mbuf in
// `mbufs` to this pool with `rte_mbuf_raw_free_bulk`. Preconditions are the
// caller's (same as `RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE`): every mbuf is from
// this pool, direct, refcnt 1, single segment, `next` NULL, and none is NULL.
// An empty slice is a no-op that must not call into DPDK at all
// (`rte_mbuf_raw_free_bulk` requires count >= 1 and its put path reads the
// pool struct).
//
// The real free needs a live EAL and mempool, so it is not tested here. On
// the testbed, `rx_nombuf` staying 0 over a long run is the check that no mbuf
// leaks.
#[cfg(test)]
mod freer_tests {
    use std::mem::ManuallyDrop;
    use std::ptr::NonNull;

    use super::{MbufFreer, Mempool};
    use crate::port::RawMbuf;

    fn assert_copy_and_send<T: Copy + Send>() {}

    #[test]
    fn mbuf_freer_is_copy_and_send() {
        assert_copy_and_send::<MbufFreer>();
    }

    /// The pool pointer is dangling, so any DPDK call that reads the pool
    /// would fault. `ManuallyDrop` keeps `Mempool::drop` (`rte_mempool_free`)
    /// from running on it.
    #[test]
    fn mbuf_freer_free_bulk_of_an_empty_slice_never_touches_the_pool() {
        let pool = ManuallyDrop::new(Mempool {
            ptr: NonNull::dangling(),
        });
        let freer = pool.freer();
        let mut none: [RawMbuf; 0] = [];
        unsafe { freer.free_bulk(&mut none) };
        // A `Vec`'s empty slice too.
        let mut empty: Vec<RawMbuf> = Vec::new();
        unsafe { freer.free_bulk(&mut empty) };
    }
}
