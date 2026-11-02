//! Read-only access to packet bytes and mbuf header fields, plus the
//! prefetch helpers for those reads. Everything else in this workspace
//! only ever moves `RawMbuf` pointers around without looking inside them;
//! this module is the one place that does, and it never writes to an mbuf
//! (no dynamic fields are registered).

use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::port::RawMbuf;

/// Issues an L1 prefetch for the cache line containing `addr`: `prfm
/// pldl1keep` on aarch64, `_mm_prefetch(_MM_HINT_T0)` on x86_64, nothing
/// elsewhere. The one place the architecture-specific instructions live --
/// every other prefetch helper in this module goes through here, so the
/// `target_arch` cfg split exists exactly once. A prefetch hint never faults,
/// even for a null or otherwise invalid `addr`, so this never dereferences it.
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

/// Computes the address of mbuf `m`'s first segment's packet data --
/// `buf_addr + data_off`, the same computation [`Packet::data`] makes
/// (mirroring the `rte_pktmbuf_mtod` macro, which isn't itself
/// bindgen-reachable, being function-like).
///
/// `m` must point to a live mbuf: this reads `buf_addr`/`data_off`
/// through it to compute the address it returns.
#[inline(always)]
fn data_addr(m: *const dpdk_sys::rte_mbuf) -> *const u8 {
    unsafe {
        let data_off = (*m).__bindgen_anon_1.__bindgen_anon_1.data_off;
        ((*m).buf_addr as *const u8).add(data_off as usize)
    }
}

/// Prefetches `rte_mbuf`'s first cache line (bytes 0..64: `buf_addr`,
/// `data_off`/`refcnt`/`nb_segs`, `ol_flags`, `pkt_len`/`data_len`,
/// `pool`) -- the line `m` itself points at -- into L1.
///
/// Unlike every other prefetch helper in this module, `m` need not point
/// to a live mbuf: the address being prefetched IS `m`, so nothing here
/// ever dereferences it. This is what makes it safe to use on a whole
/// batch in a pass ahead of the loop that reads it, e.g. for
/// `pkt_len` reads of mbufs whose line 0 may have been cold since RX.
#[inline(always)]
pub fn prefetch_header(m: *const dpdk_sys::rte_mbuf) {
    prefetch_t0(m as *const i8);
}

/// Prefetches the cache line holding mbuf `m`'s first segment's packet
/// data (via [`data_addr`]) into L1 -- the bytes the header parser
/// ([`Packet::data`]) is about to read.
///
/// `m` must point to a live mbuf: unlike [`prefetch_header`], this
/// dereferences `m` (through [`data_addr`]) to compute which address to
/// prefetch. That read is a synchronous load of line 0, so this only pays
/// off where line 0 is already hot, as right after `rx_burst`, which has
/// just written it.
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

/// Overwrites the port id stored in `m` (`rte_mbuf::port`).
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

/// A borrowed, read-only view of one mbuf's packet data.
///
/// Read-only by design: packet bytes are never written through this view.
/// Per-packet metadata (a parsed header, an arrival timestamp) should travel
/// next to the pointer in the caller's own structs rather than in the mbuf.
///
/// Stores a raw pointer plus `PhantomData<&'a rte_mbuf>`, which enforces
/// the borrow-checker lifetime contract that the `Packet` must not outlive
/// the mbuf (per [`Packet::from_raw`]'s safety doc).
///
/// The only unsafe step is [`Packet::from_raw`] itself — converting the
/// raw pointer handed back by `rx_burst` into a lifetime-bound `Packet`.
/// Every accessor after that is a safe `&self` method, the same
/// single-unsafe-boundary shape `Port`/`RxQueue` already use elsewhere in
/// this crate.
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
        // Same computation as the `rte_pktmbuf_mtod` macro (not itself
        // bindgen-reachable, since it's a function-like macro, not a
        // static-inline function): base address of the segment's buffer,
        // offset by how much headroom precedes the packet data.
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

    /// Builds a zeroed `rte_mbuf` on the stack with `buf_addr` pointing at
    /// `buf`'s first byte and `data_off` set as given -- the same two
    /// fields [`Packet::data`] reads, via the same bindgen union path
    /// (`__bindgen_anon_1.__bindgen_anon_1.data_off`). Valid for anything
    /// `data_addr`/`prefetch_header`/`prefetch_data` do; no EAL needed.
    fn mbuf_with(buf: &[u8], data_off: u16) -> dpdk_sys::rte_mbuf {
        let mut m: dpdk_sys::rte_mbuf = unsafe { std::mem::zeroed() };
        m.buf_addr = buf.as_ptr() as *mut std::ffi::c_void;
        m.__bindgen_anon_1.__bindgen_anon_1.data_off = data_off;
        m
    }

    /// `data_addr` and `Packet::data` compute the same address
    /// independently; pin that they agree.
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
        // Must be derived from buf_addr, not from the mbuf struct's own
        // address (they're deliberately different local buffers here).
        assert_ne!(got, &m as *const _ as *const u8);
    }

    #[test]
    fn prefetch_header_does_not_dereference_a_null_pointer() {
        // Must not panic or fault: prefetch_header only ever prefetches the
        // address `m` itself (cache line 0), never reads through it.
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
        // No EAL in this test binary: `prefetch_data` must need nothing
        // beyond the mbuf's own `buf_addr`/`data_off`.
        prefetch_data(&m);
    }
}
