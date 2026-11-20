//! Access to packet bytes and mbuf header fields, plus the prefetch helpers for
//! those reads. The rest of the workspace only moves `RawMbuf` pointers around;
//! this module is the one place that looks inside them.

use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::port::RawMbuf;

/// L1 prefetch of the cache line containing `addr`: `prfm pldl1keep` on
/// aarch64, `_mm_prefetch(_MM_HINT_T0)` on x86_64, nothing elsewhere. A
/// prefetch hint never faults, even for a null or invalid `addr`.
#[inline(always)]
fn prefetch_t0(addr: *const i8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_mm_prefetch(addr, std::arch::x86_64::_MM_HINT_T0);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!(
            "prfm pldl1keep, [{0}]",
            in(reg) addr,
            options(readonly, nostack, preserves_flags)
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = addr;
}

/// Address of mbuf `m`'s first segment's packet data: `buf_addr + data_off`
/// (the `rte_pktmbuf_mtod` macro, which bindgen cannot reach).
///
/// `m` must point to a live mbuf: this reads `buf_addr`/`data_off` through it.
#[inline(always)]
fn data_addr(m: *const dpdk_sys::rte_mbuf) -> *const u8 {
    unsafe {
        let data_off = (*m).__bindgen_anon_1.__bindgen_anon_1.data_off;
        ((*m).buf_addr as *const u8).add(data_off as usize)
    }
}

/// Prefetches the mbuf's first cache line (`buf_addr`, `data_off`/`refcnt`/
/// `nb_segs`, `ol_flags`, `pkt_len`/`data_len`, `pool`) into L1.
///
/// Unlike the other prefetch helper, `m` need not point to a live mbuf: the
/// address prefetched is `m` itself and is never dereferenced. That makes it
/// safe on a whole batch ahead of the loop that reads `pkt_len`.
#[inline(always)]
pub fn prefetch_header(m: *const dpdk_sys::rte_mbuf) {
    prefetch_t0(m as *const i8);
}

/// Prefetches the cache line holding the first segment's packet data into L1.
///
/// `m` must point to a live mbuf: unlike [`prefetch_header`], this dereferences
/// `m` to compute the address. That is a synchronous load of line 0, so it only
/// pays off where line 0 is already hot, right after `rx_burst`.
#[inline(always)]
pub fn prefetch_data(m: *const dpdk_sys::rte_mbuf) {
    prefetch_t0(data_addr(m) as *const i8);
}

/// The port id stored in `m` (`rte_mbuf::port`).
///
/// # Safety
/// `m` must point to a live mbuf.
#[inline(always)]
pub unsafe fn port(m: RawMbuf) -> u16 {
    (*m).__bindgen_anon_1.__bindgen_anon_1.port
}

/// Overwrites the port id in `m` (`rte_mbuf::port`).
///
/// # Safety
/// `m` must point to a live mbuf that nothing else is accessing.
#[inline(always)]
pub unsafe fn set_port(m: RawMbuf, port: u16) {
    (*m).__bindgen_anon_1.__bindgen_anon_1.port = port;
}

/// Total packet length of `m` in bytes (`rte_mbuf::pkt_len`).
///
/// # Safety
/// `m` must point to a live mbuf.
#[inline(always)]
pub unsafe fn pkt_len(m: RawMbuf) -> u32 {
    (*m).__bindgen_anon_2.__bindgen_anon_1.pkt_len
}

/// Stores `ns` in the 8-byte `hash` union of `m` (`rte_mbuf::hash`, first cache
/// line, next to `pkt_len`). DPDS keeps a packet's arrival time there while the
/// delay buffer holds it: with one RX queue and no RSS the field is unused, and
/// mlx5 TX never reads it. Unaligned write: the union is only 4-byte aligned.
///
/// # Safety
/// `m` must point to a live mbuf that nothing else is accessing.
#[inline(always)]
pub unsafe fn set_arrival_ns(m: RawMbuf, ns: f64) {
    let hash = std::ptr::addr_of_mut!((*m).__bindgen_anon_2.__bindgen_anon_1.__bindgen_anon_2.hash);
    hash.cast::<f64>().write_unaligned(ns);
}

/// The value [`set_arrival_ns`] stored in `m`.
///
/// # Safety
/// `m` must point to a live mbuf that nothing else is accessing, and
/// [`set_arrival_ns`] must have been called on it since it left the pool.
#[inline(always)]
pub unsafe fn arrival_ns(m: RawMbuf) -> f64 {
    let hash = std::ptr::addr_of!((*m).__bindgen_anon_2.__bindgen_anon_1.__bindgen_anon_2.hash);
    hash.cast::<f64>().read_unaligned()
}

/// A borrowed, read-only view of one mbuf's packet data. The `PhantomData`
/// lifetime keeps the `Packet` from outliving the mbuf (see
/// [`Packet::from_raw`]); that is the only unsafe step, every accessor is safe.
pub struct Packet<'a> {
    mbuf: NonNull<dpdk_sys::rte_mbuf>,
    _marker: PhantomData<&'a dpdk_sys::rte_mbuf>,
}

impl<'a> Packet<'a> {
    /// # Safety
    /// `m` must be a valid, non-null mbuf pointer with at least one
    /// segment, and the returned `Packet` must not outlive it (nor survive
    /// a later mutation of `data_off`/`data_len`/`pkt_len` on the same
    /// mbuf).
    #[inline(always)]
    pub unsafe fn from_raw(m: RawMbuf) -> Packet<'a> {
        Packet {
            mbuf: NonNull::new_unchecked(m),
            _marker: PhantomData,
        }
    }

    /// Total packet length in bytes across all segments.
    #[inline(always)]
    pub fn pkt_len(&self) -> u32 {
        unsafe {
            (*self.mbuf.as_ptr())
                .__bindgen_anon_2
                .__bindgen_anon_1
                .pkt_len
        }
    }

    /// The first segment's data.
    #[inline(always)]
    pub fn data(&self) -> &'a [u8] {
        let len = unsafe {
            (*self.mbuf.as_ptr())
                .__bindgen_anon_2
                .__bindgen_anon_1
                .data_len
        } as usize;
        // Same as the `rte_pktmbuf_mtod` macro: buffer base plus headroom.
        let data_off = unsafe {
            (*self.mbuf.as_ptr())
                .__bindgen_anon_1
                .__bindgen_anon_1
                .data_off
        };
        unsafe {
            let base = ((*self.mbuf.as_ptr()).buf_addr as *const u8).add(data_off as usize);
            std::slice::from_raw_parts(base, len)
        }
    }
}

#[cfg(test)]
mod prefetch_helper_tests {
    use super::{data_addr, prefetch_data, prefetch_header};

    /// A zeroed `rte_mbuf` with `buf_addr` pointing at `buf` and `data_off` as
    /// given: the two fields [`Packet::data`] reads. No EAL needed.
    fn mbuf_with(buf: &[u8], data_off: u16) -> dpdk_sys::rte_mbuf {
        let mut m: dpdk_sys::rte_mbuf = unsafe { std::mem::zeroed() };
        m.buf_addr = buf.as_ptr() as *mut std::ffi::c_void;
        m.__bindgen_anon_1.__bindgen_anon_1.data_off = data_off;
        m
    }

    /// `data_addr` and `Packet::data` compute the same address independently.
    #[test]
    fn data_addr_matches_packet_data() {
        let buf = [0u8; 256];
        let mut m = mbuf_with(&buf, 128);
        let packet = unsafe { super::Packet::from_raw(&mut m) };
        assert_eq!(data_addr(&m), packet.data().as_ptr());
    }

    #[test]
    fn data_addr_at_zero_data_off_is_buf_addr() {
        let buf = [0u8; 16];
        let m = mbuf_with(&buf, 0);
        assert_eq!(data_addr(&m), buf.as_ptr());
    }

    #[test]
    fn data_addr_adds_headroom_data_off() {
        // RTE_PKTMBUF_HEADROOM.
        let buf = [0u8; 256];
        let m = mbuf_with(&buf, 128);
        assert_eq!(data_addr(&m), unsafe { buf.as_ptr().add(128) });
    }

    #[test]
    fn data_addr_tracks_a_nontrivial_buf_addr() {
        let buf = [0u8; 32];
        let m = mbuf_with(&buf, 4);
        let expected = unsafe { buf.as_ptr().add(4) };
        let got = data_addr(&m);
        assert_eq!(got, expected);
        // Derived from buf_addr, not from the mbuf struct's own address.
        assert_ne!(got, &m as *const _ as *const u8);
    }

    #[test]
    fn prefetch_header_does_not_dereference_a_null_pointer() {
        // `prefetch_header` only prefetches the address `m` itself, never reads through it.
        prefetch_header(std::ptr::null());
    }

    #[test]
    fn prefetch_header_accepts_a_live_mbuf() {
        let buf = [0u8; 16];
        let m = mbuf_with(&buf, 0);
        prefetch_header(&m);
    }

    #[test]
    fn prefetch_data_accepts_a_live_mbuf() {
        let buf = [0u8; 16];
        let m = mbuf_with(&buf, 0);
        // Needs nothing beyond the mbuf's own `buf_addr`/`data_off` (no EAL here).
        prefetch_data(&m);
    }
}
