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
/// pool alive for as long as any copy is used (in `peof` the pool is
/// leaked for `'static`).
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

/// Mbuf data-room size for a deployment whose largest real frame is
/// `max_frame_len` bytes, instead of [`Mempool::DEFAULT_BUF_SIZE`]
/// (DPDK's generic default, sized for a standard 1518-byte frame with
/// room to spare). `RTE_PKTMBUF_HEADROOM` (128 B) plus `max_frame_len`
/// rounded up to the next multiple of 64 -- the 64-byte rounding isn't
/// required by any DPDK API, it's just a cache-line-friendly size for the
/// allocation, mirroring how `RTE_MBUF_DEFAULT_BUF_SIZE` itself rounds
/// `RTE_MBUF_DEFAULT_DATAROOM` (2048) to a round number past the
/// standard frame. Sizing the pool's mbufs against this deployment's own
/// `net_config::MAX_FRAME_LEN` instead of the generic default shrinks
/// each mbuf (2176 -> 1664 B), which matters at this pipeline's
/// multi-million-mbuf pool sizes.
pub const fn mbuf_buf_size_for_frame(max_frame_len: u16) -> u16 {
    const STRIDE: u16 = 64;
    let rounded = max_frame_len.div_ceil(STRIDE) * STRIDE;
    dpdk_sys::RTE_PKTMBUF_HEADROOM as u16 + rounded
}

// Tests for `mbuf_buf_size_for_frame`: `RTE_PKTMBUF_HEADROOM +
// round_up(max_frame_len, 64)`; see each test's doc comment for the exact
// pinned behaviour.

#[cfg(test)]
mod tests {
    use super::*;

    // `net_config::MAX_FRAME_LEN` (1526 = 14-byte Ethernet header +
    // NIC_MTU(1508) + 4-byte FCS) duplicated as a plain literal here, not
    // imported -- this crate (`dpdk`) sits below `net-config` in the
    // dependency graph (see `net-config/src/lib.rs`'s own doc comment:
    // `dpdk` is meant to stay a generic, PEOF-agnostic DPDK wrapper with no
    // PEOF-deployment-specific sizing knowledge), so it cannot depend on
    // `net-config` to read this value back.
    const MAX_FRAME_LEN: u16 = 1526;

    // RTE_PKTMBUF_HEADROOM is 128 on every DPDK build this project targets
    // (dpdk-sys's generated bindings) -- asserted here once so every other
    // test in this module can use the literal `128` directly without each
    // one re-deriving it from `dpdk_sys::RTE_PKTMBUF_HEADROOM` by hand.
    #[test]
    fn rte_pktmbuf_headroom_is_128() {
        assert_eq!(dpdk_sys::RTE_PKTMBUF_HEADROOM, 128);
    }

    /// `MAX_FRAME_LEN` (1526) is already not a multiple of 64 -- rounds up
    /// to 1536, the real-world case this function exists for.
    #[test]
    fn max_frame_len_rounds_up_to_1536_plus_headroom() {
        assert_eq!(mbuf_buf_size_for_frame(MAX_FRAME_LEN), 128 + 1536);
        assert_eq!(mbuf_buf_size_for_frame(MAX_FRAME_LEN), 1664);
    }

    /// A frame length that is ALREADY an exact multiple of 64 must not be
    /// rounded up any further (round-up, not round-up-and-add-one-stride).
    #[test]
    fn exact_multiple_of_64_is_unchanged_by_rounding() {
        assert_eq!(mbuf_buf_size_for_frame(1536), 128 + 1536);
        assert_eq!(mbuf_buf_size_for_frame(1536), 1664);
    }

    /// One byte past a multiple of 64 must round all the way up to the
    /// NEXT multiple, not stay at the same one -- pins the boundary
    /// direction of the rounding.
    #[test]
    fn one_byte_past_a_multiple_of_64_rounds_up_to_the_next_multiple() {
        assert_eq!(mbuf_buf_size_for_frame(1537), 128 + 1600);
        assert_eq!(mbuf_buf_size_for_frame(1537), 1728);
    }

    /// General property, independent of the three pinned literals above:
    /// the result can never be smaller than headroom + the raw
    /// (unrounded) frame length -- rounding only ever adds slack, never
    /// removes it.
    #[test]
    fn result_is_always_at_least_headroom_plus_the_raw_frame_len() {
        for frame_len in [
            0u16,
            1,
            63,
            64,
            65,
            1518,
            1526,
            1536,
            1537,
            9000,
            u16::MAX - 200,
        ] {
            let result = mbuf_buf_size_for_frame(frame_len);
            assert!(
                result as u32 >= 128u32 + frame_len as u32,
                "mbuf_buf_size_for_frame({frame_len}) = {result}, expected >= {}",
                128u32 + frame_len as u32
            );
        }
    }

    /// The property this function exists to guarantee for mlx5 MPRQ's
    /// memcpy fallback: the data room PAST the headroom must be able to
    /// hold one full `MAX_FRAME_LEN`-size frame without RX scatter.
    #[test]
    fn data_room_past_headroom_fits_a_full_max_frame_len_frame() {
        let result = mbuf_buf_size_for_frame(MAX_FRAME_LEN);
        let headroom = dpdk_sys::RTE_PKTMBUF_HEADROOM as u16;
        assert!(
            result - headroom >= MAX_FRAME_LEN,
            "data room past headroom ({}) must be >= MAX_FRAME_LEN ({MAX_FRAME_LEN})",
            result - headroom
        );
    }

    /// Must be smaller than `Mempool::DEFAULT_BUF_SIZE` for this
    /// deployment's real `MAX_FRAME_LEN` -- the whole point of this
    /// function is to size each mbuf down from DPDK's generic default to
    /// this deployment's own, smaller real need (fewer bytes wasted per
    /// mbuf across a multi-million-mbuf pool).
    #[test]
    fn result_for_max_frame_len_is_smaller_than_the_dpdk_default_buf_size() {
        let result = mbuf_buf_size_for_frame(MAX_FRAME_LEN);
        assert!(
            result < Mempool::DEFAULT_BUF_SIZE,
            "mbuf_buf_size_for_frame(MAX_FRAME_LEN) = {result} must be < \
             Mempool::DEFAULT_BUF_SIZE ({}); that was the entire point of sizing the pool \
             against this deployment's real max frame instead of the generic DPDK default",
            Mempool::DEFAULT_BUF_SIZE
        );
    }
}

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
        // A `Vec`'s empty slice too (the shape peof_stage's reject sink has).
        let mut empty: Vec<RawMbuf> = Vec::new();
        unsafe { freer.free_bulk(&mut empty) };
    }
}
