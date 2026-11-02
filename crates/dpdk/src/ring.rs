use std::ffi::{c_void, CString};
use std::marker::PhantomData;
use std::mem::size_of;
use std::ptr::NonNull;

use crate::error::DpdkError;

/// `rte_ring` element size for an element type of `size` bytes, or `None`
/// if DPDK would reject it: `size` is 0, not a multiple of 4
/// (`rte_ring_create_elem`'s EINVAL condition), or doesn't fit in a `u32`.
pub const fn checked_ring_esize(size: usize) -> Option<u32> {
    if size == 0 || !size.is_multiple_of(4) || size > u32::MAX as usize {
        None
    } else {
        Some(size as u32)
    }
}

/// [`checked_ring_esize`], panicking where that returns `None`. Usable in
/// a `const` context, so an invalid element type fails at compile time
/// once [`Ring::ESIZE`] is evaluated.
pub const fn ring_esize(size: usize) -> u32 {
    match checked_ring_esize(size) {
        Some(esize) => esize,
        None => panic!("rte_ring element size must be a positive multiple of 4 that fits in u32"),
    }
}

/// Owned `rte_ring` holding elements of type `T` by value: a lock-free,
/// fixed-capacity queue used for handoff between pipeline stages pinned to
/// different lcores. Built on DPDK's element ring API
/// (`rte_ring_create_elem`, `rte_ring_*_burst_elem`), so `T` can carry
/// per-packet metadata next to the mbuf pointer instead of storing it in the
/// mbuf. The default `T = *mut c_void` is the classic pointer ring (element
/// size 8, the same code path `rte_ring_create`/`rte_ring_*_burst` take).
///
/// Every ring in this application's pipeline has exactly one producer
/// lcore and one consumer lcore (the forwarding graph fans a stage's
/// output out to several *distinct* rings, and fans several stages' output
/// into several *distinct* rings — never many writers into the same
/// ring), so every ring is created single-producer/single-consumer:
/// `RING_F_SP_ENQ | RING_F_SC_DEQ`. That lets `rte_ring` skip its
/// multi-producer/multi-consumer CAS retry loop entirely.
pub struct Ring<T: Copy = *mut c_void> {
    ptr: NonNull<dpdk_sys::rte_ring>,
    _elem: PhantomData<T>,
}

impl<T: Copy> Ring<T> {
    /// Element size handed to DPDK; evaluating it rejects an invalid `T` at
    /// compile time (see [`ring_esize`]).
    const ESIZE: u32 = ring_esize(size_of::<T>());

    /// Creates a single-producer/single-consumer ring of `count` slots.
    /// `count` must be a power of two: DPDK 26.07's `rte_ring_create_elem`
    /// fails with EINVAL otherwise (it does not round), and one slot always
    /// stays unused, so the usable capacity is `count - 1`.
    pub fn create_spsc(name: &str, count: u32, socket_id: i32) -> Result<Self, DpdkError> {
        let c_name = CString::new(name).expect("ring name contains interior NUL");
        let flags = dpdk_sys::RING_F_SP_ENQ | dpdk_sys::RING_F_SC_DEQ;
        let ptr = unsafe {
            dpdk_sys::rte_ring_create_elem(c_name.as_ptr(), Self::ESIZE, count, socket_id, flags)
        };
        NonNull::new(ptr)
            .map(|ptr| Ring {
                ptr,
                _elem: PhantomData,
            })
            .ok_or_else(DpdkError::from_rte_errno)
    }

    /// Enqueues as many of `objs` as fit; returns the number actually
    /// enqueued (`<= objs.len()`). The caller must handle the remainder
    /// (e.g. free or retry) — mirrors `TxQueue::tx_burst`'s contract.
    #[inline(always)]
    pub fn enqueue_burst(&self, objs: &[T]) -> u16 {
        let n = objs.len().min(u16::MAX as usize) as u16;
        unsafe {
            dpdk_sys::rte_ring_sp_enqueue_burst_elem(
                self.ptr.as_ptr(),
                objs.as_ptr() as *const c_void,
                Self::ESIZE,
                n as u32,
                std::ptr::null_mut(),
            ) as u16
        }
    }

    /// Dequeues into `objs`, returning the number actually dequeued
    /// (`<= objs.len()`).
    #[inline(always)]
    pub fn dequeue_burst(&self, objs: &mut [T]) -> u16 {
        let n = objs.len().min(u16::MAX as usize) as u16;
        unsafe {
            dpdk_sys::rte_ring_sc_dequeue_burst_elem(
                self.ptr.as_ptr(),
                objs.as_mut_ptr() as *mut c_void,
                Self::ESIZE,
                n as u32,
                std::ptr::null_mut(),
            ) as u16
        }
    }

    /// Same as [`dequeue_burst`](Self::dequeue_burst), but also returns the
    /// ring's own depth at the moment of this call: the number of entries
    /// that were actually waiting in the ring just before this dequeue, i.e.
    /// the dequeued count plus `rte_ring_sc_dequeue_burst_elem`'s own
    /// `available` out-param (how many entries were still left in the ring
    /// after this call drained up to `objs.len()` of them). Used to sample
    /// the high-water mark of a consumer's lag behind its producer without a
    /// second, separate call.
    #[inline(always)]
    pub fn dequeue_burst_with_depth(&self, objs: &mut [T]) -> (u16, u32) {
        let n = objs.len().min(u16::MAX as usize) as u16;
        let mut available: u32 = 0;
        let dequeued = unsafe {
            dpdk_sys::rte_ring_sc_dequeue_burst_elem(
                self.ptr.as_ptr(),
                objs.as_mut_ptr() as *mut c_void,
                Self::ESIZE,
                n as u32,
                &mut available,
            ) as u16
        };
        (dequeued, dequeued as u32 + available)
    }
}

impl<T: Copy> Drop for Ring<T> {
    fn drop(&mut self) {
        unsafe { dpdk_sys::rte_ring_free(self.ptr.as_ptr()) };
    }
}

/// Marker for types that may be passed by value from one lcore to another
/// through a [`Ring`], which makes `Ring<T>` `Send + Sync`.
///
/// # Safety
/// Copying a `T` into the ring on one thread and reading the copy on
/// another must be sound: `T` must be plain data whose pointers (if any)
/// hand over ownership of what they point at, never share it (so e.g.
/// `&Cell<_>` must not implement this). Raw pointers are not `Send`, so
/// this can't simply be `T: Send`.
pub unsafe trait RingElem: Copy {}

// SAFETY: the pointer ring carries mbuf pointers; each one is handed over
// with ownership of its mbuf (the producer never touches it after the
// enqueue), so moving it to the consumer lcore shares nothing.
unsafe impl RingElem for *mut c_void {}

// Safety: a `Ring` is handed to exactly one producer lcore and one
// consumer lcore by construction in this pipeline (never both from the
// same thread concurrently in a way `rte_ring`'s SP/SC mode doesn't
// already support); the underlying `rte_ring` synchronizes the actual
// enqueue/dequeue. Elements are moved by value between those two lcores,
// which `RingElem` vouches is sound for `T`.
unsafe impl<T: RingElem> Send for Ring<T> {}
unsafe impl<T: RingElem> Sync for Ring<T> {}

// ── Generic element ring ────────────────────────────────────────────────────
//
// `Ring<T>` wraps DPDK's element ring API, with `T` copied by value into each
// slot; the pointer ring is the `T = *mut c_void` case (esize 8).
//
// The element size passed to DPDK comes from `ring_esize`, which panics if
// `size` is 0, not a multiple of 4 (DPDK's EINVAL condition) or doesn't fit
// in a `u32`; `checked_ring_esize` returns `None` in exactly those cases.
// `Ring<T>` is `Send + Sync` only for element types marked `RingElem`.
//
// Creating and using a ring needs EAL, so only the size rule and the type-level
// properties are tested here.
#[cfg(test)]
mod elem_tests {
    use std::ffi::c_void;
    use std::mem::size_of;

    use super::{checked_ring_esize, ring_esize, Ring, RingElem};

    /// A pointer plus two 8-byte words of per-packet metadata.
    #[derive(Clone, Copy)]
    #[allow(dead_code)]
    struct Elem24 {
        ptr: *mut c_void,
        a: u64,
        b: f64,
    }

    // SAFETY: the pointer moves with ownership of what it points at; the
    // two words are plain data.
    unsafe impl RingElem for Elem24 {}

    /// Same shape as `Elem24`, but not marked `RingElem`.
    #[derive(Clone, Copy)]
    #[allow(dead_code)]
    struct Unmarked {
        ptr: *mut c_void,
    }

    #[test]
    fn ring_esize_of_a_pointer_is_8() {
        assert_eq!(ring_esize(size_of::<*mut c_void>()), 8);
    }

    #[test]
    fn ring_esize_of_a_24_byte_element_is_24() {
        assert_eq!(size_of::<Elem24>(), 24);
        assert_eq!(ring_esize(size_of::<Elem24>()), 24);
    }

    #[test]
    fn ring_esize_is_usable_in_a_const_context() {
        const ESIZE: u32 = ring_esize(16);
        assert_eq!(ESIZE, 16);
    }

    #[test]
    #[should_panic]
    fn ring_esize_rejects_a_size_not_a_multiple_of_4() {
        let _ = ring_esize(6);
    }

    #[test]
    #[should_panic]
    fn ring_esize_rejects_zero() {
        let _ = ring_esize(0);
    }

    #[test]
    #[should_panic]
    fn ring_esize_rejects_a_size_that_does_not_fit_in_u32() {
        // A multiple of 4, so only the u32 range can reject it.
        let _ = ring_esize(u32::MAX as usize + 1);
    }

    #[test]
    fn checked_ring_esize_accepts_every_positive_multiple_of_4() {
        for size in [4usize, 8, 12, 16, 24, 32, 64, 4096] {
            assert_eq!(checked_ring_esize(size), Some(size as u32), "size {size}");
        }
        // Largest multiple of 4 that still fits.
        assert_eq!(
            checked_ring_esize(u32::MAX as usize - 3),
            Some(u32::MAX - 3)
        );
    }

    #[test]
    fn checked_ring_esize_rejects_zero_non_multiples_of_4_and_oversized() {
        assert_eq!(checked_ring_esize(0), None);
        for size in [1usize, 2, 3, 5, 6, 7, 9, 22, 25] {
            assert_eq!(checked_ring_esize(size), None, "size {size}");
        }
        assert_eq!(checked_ring_esize(u32::MAX as usize + 1), None);
    }

    /// `Ring` without a type argument still means the pointer ring, so
    /// `Arc<Ring>` call sites keep compiling.
    #[test]
    fn ring_defaults_to_a_pointer_ring() {
        let none: Option<&Ring> = None;
        let _same: Option<&Ring<*mut c_void>> = none;
    }

    fn assert_send_sync<T: Send + Sync>() {}

    /// Each ring is shared (`Arc`) between one producer and one consumer
    /// lcore, so it must be `Send + Sync` even when `T` holds a raw pointer.
    #[test]
    fn rings_of_pointers_and_of_pointer_carrying_elements_are_send_and_sync() {
        assert_send_sync::<Ring>();
        assert_send_sync::<Ring<Elem24>>();
    }

    /// `IS_SEND_SYNC` resolves to the inherent `true` only when `T: Send +
    /// Sync`; otherwise it falls back to the blanket trait's `false`. This
    /// is the `impls` crate pattern, a negative trait check on stable Rust
    /// without a compile-fail harness.
    struct Probe<T>(std::marker::PhantomData<T>);
    trait NotSendSync {
        const IS_SEND_SYNC: bool = false;
    }
    impl<T> NotSendSync for T {}
    impl<T: Send + Sync> Probe<T> {
        const IS_SEND_SYNC: bool = true;
    }

    /// `Ring<T>` must not be `Send`/`Sync` for an arbitrary `Copy` element:
    /// a safe caller could otherwise move a non-thread-safe value (e.g. a
    /// shared `&Cell<_>`) to another lcore. Only `RingElem` opts in. The
    /// checks are `const` blocks, so a regression fails the build.
    #[test]
    fn ring_is_send_and_sync_only_for_ring_elem_types() {
        const {
            assert!(Probe::<Ring<Elem24>>::IS_SEND_SYNC);
            assert!(Probe::<Ring>::IS_SEND_SYNC);
            assert!(!Probe::<Ring<Unmarked>>::IS_SEND_SYNC);
            assert!(!Probe::<Ring<&'static std::cell::Cell<u32>>>::IS_SEND_SYNC);
        }
    }
}
