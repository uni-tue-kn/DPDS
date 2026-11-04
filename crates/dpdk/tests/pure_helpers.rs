//! Pure (no EAL, no hugepages) checks of the public helpers that were only
//! covered by in-source unit tests or not at all: the mbuf field accessors on a
//! hand-built `rte_mbuf`, the prefetch helpers, `tx_offloads_for`, `DpdkError`,
//! the ring element-size rule, the `Container` telemetry arrays.
//!
//! mbufs are built as zeroed `rte_mbuf` values on the stack with the fields set
//! through the same bindgen union paths the accessors read.

use dpdk::error::DpdkError;
use dpdk::mbuf::{self, Packet};
use dpdk::port::tx_offloads_for;
use dpdk::ring::{checked_ring_esize, ring_esize};

fn mbuf_with(
    buf: &[u8],
    data_off: u16,
    data_len: u16,
    pkt_len: u32,
    port: u16,
) -> dpdk_sys::rte_mbuf {
    let mut m: dpdk_sys::rte_mbuf = unsafe { std::mem::zeroed() };
    m.buf_addr = buf.as_ptr() as *mut std::ffi::c_void;
    m.__bindgen_anon_1.__bindgen_anon_1.data_off = data_off;
    m.__bindgen_anon_1.__bindgen_anon_1.port = port;
    m.__bindgen_anon_2.__bindgen_anon_1.pkt_len = pkt_len;
    m.__bindgen_anon_2.__bindgen_anon_1.data_len = data_len;
    m
}

#[test]
fn pkt_len_and_port_read_the_mbuf_header_fields() {
    let buf = [0u8; 64];
    let mut m = mbuf_with(&buf, 0, 60, 1518, 3);
    assert_eq!(
        unsafe { mbuf::pkt_len(&mut m) },
        1518,
        "pkt_len, not data_len"
    );
    assert_eq!(unsafe { mbuf::port(&mut m) }, 3);
}

#[test]
fn set_port_changes_only_the_port_field() {
    let buf = [0u8; 64];
    let mut m = mbuf_with(&buf, 16, 40, 1000, 3);
    unsafe { mbuf::set_port(&mut m, 65_535) };
    assert_eq!(unsafe { mbuf::port(&mut m) }, 65_535);
    assert_eq!(unsafe { mbuf::pkt_len(&mut m) }, 1000);
    assert_eq!(unsafe { m.__bindgen_anon_1.__bindgen_anon_1.data_off }, 16);
    unsafe { mbuf::set_port(&mut m, 0) };
    assert_eq!(unsafe { mbuf::port(&mut m) }, 0);
}

#[test]
fn packet_exposes_pkt_len_and_the_first_segment_bytes() {
    let mut buf = [0u8; 256];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = i as u8;
    }
    // Data starts after 128 bytes of headroom; first segment is 5 bytes of a
    // 9-byte packet (the rest would be in a second segment).
    let mut m = mbuf_with(&buf, 128, 5, 9, 0);
    let p = unsafe { Packet::from_raw(&mut m) };
    assert_eq!(p.pkt_len(), 9);
    assert_eq!(p.data(), &[128, 129, 130, 131, 132]);
}

#[test]
fn packet_with_an_empty_first_segment_has_empty_data() {
    let buf = [7u8; 16];
    let mut m = mbuf_with(&buf, 4, 0, 0, 0);
    let p = unsafe { Packet::from_raw(&mut m) };
    assert!(p.data().is_empty());
    assert_eq!(p.pkt_len(), 0);
}

#[test]
fn prefetch_helpers_never_fault_on_any_address() {
    // A prefetch is only a hint: null, dangling and unmapped addresses must be
    // fine (on aarch64 this exercises `prfm`, on x86_64 `_mm_prefetch`).
    for addr in [0usize, 1, 8, 0xdead_beef, usize::MAX - 63] {
        mbuf::prefetch_header(addr as *const dpdk_sys::rte_mbuf);
    }
    let buf = [0u8; 256];
    let m = mbuf_with(&buf, 128, 64, 64, 0);
    mbuf::prefetch_header(&m);
    mbuf::prefetch_data(&m);
}

#[test]
fn tx_offloads_request_only_mbuf_fast_free() {
    const FAST_FREE: u64 = dpdk_sys::RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE;
    assert_eq!(FAST_FREE.count_ones(), 1, "a single capability bit");
    assert_eq!(tx_offloads_for(FAST_FREE), FAST_FREE);
    assert_eq!(tx_offloads_for(FAST_FREE | 1), FAST_FREE);
    assert_eq!(tx_offloads_for(0), 0);
    assert_eq!(tx_offloads_for(!FAST_FREE), 0);
    assert_eq!(tx_offloads_for(u64::MAX), FAST_FREE);
}

#[test]
fn dpdk_error_display_names_the_errno_and_the_code() {
    // rte_strerror works before the EAL is up.
    let msg = DpdkError(-19).to_string();
    assert!(msg.contains("(code -19)"), "{msg}");
    assert!(
        msg.to_lowercase().contains("device"),
        "ENODEV text expected: {msg}"
    );
    let msg = DpdkError(-22).to_string();
    assert!(
        msg.contains("(code -22)") && msg.to_lowercase().contains("invalid"),
        "{msg}"
    );
    // Positive or negative input prints the same text.
    assert_eq!(
        DpdkError(19).to_string().split(" (").next(),
        DpdkError(-19).to_string().split(" (").next()
    );
}

#[test]
fn dpdk_error_is_a_comparable_copyable_std_error() {
    fn assert_error<E: std::error::Error + Copy + PartialEq + Send + Sync + 'static>() {}
    assert_error::<DpdkError>();
    assert_eq!(DpdkError(-1), DpdkError(-1));
    assert_ne!(DpdkError(-1), DpdkError(-2));
}

#[test]
fn ring_element_size_rule_matches_rte_ring_create_elem() {
    assert_eq!(checked_ring_esize(8), Some(8));
    assert_eq!(checked_ring_esize(4), Some(4));
    assert_eq!(checked_ring_esize(0), None);
    assert_eq!(checked_ring_esize(10), None);
    assert_eq!(ring_esize(16), 16);
}

#[test]
fn telemetry_containers_build_and_drop_without_an_eal() {
    let mut s = dpdk::telemetry::Container::new_string_array();
    s.add_string("a=1");
    s.add_string("");
    let mut u = dpdk::telemetry::Container::new_uint_array();
    u.add_uint(0).unwrap();
    u.add_uint(u64::MAX).unwrap();
    drop((s, u));
}
