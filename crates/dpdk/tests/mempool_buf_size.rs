//! `dpdk::mempool::mbuf_buf_size_for_frame`, restored verbatim from preof
//! (commit 760c60c, `dpdk-peof/dpdk/src/mempool.rs`):
//!
//! ```ignore
//! pub const fn mbuf_buf_size_for_frame(max_frame_len: u16) -> u16 {
//!     const STRIDE: u16 = 64;
//!     let rounded = max_frame_len.div_ceil(STRIDE) * STRIDE;
//!     dpdk_sys::RTE_PKTMBUF_HEADROOM as u16 + rounded
//! }
//! ```
//!
//! i.e. headroom (128) + the frame length rounded up to a multiple of 64. The
//! tests are preof's own, ported to the public path, plus the DPDS values:
//! DPDS passes `MTU + 14 + 4` (`MTU = 1500`, so a 1518-byte frame) -> 1536 +
//! 128 = 1664.

use dpdk::mempool::mbuf_buf_size_for_frame;
use dpdk::Mempool;

// preof's MAX_FRAME_LEN (14 + 1508 + 4), a frame that is not a multiple of 64.
const PEOF_MAX_FRAME_LEN: u16 = 1526;

#[test]
fn rte_pktmbuf_headroom_is_128() {
    assert_eq!(dpdk_sys::RTE_PKTMBUF_HEADROOM, 128);
}

#[test]
fn default_buf_size_is_dpdks_2176() {
    assert_eq!(Mempool::DEFAULT_BUF_SIZE, 2176);
}

#[test]
fn max_frame_len_rounds_up_to_1536_plus_headroom() {
    assert_eq!(mbuf_buf_size_for_frame(PEOF_MAX_FRAME_LEN), 128 + 1536);
    assert_eq!(mbuf_buf_size_for_frame(PEOF_MAX_FRAME_LEN), 1664);
}

#[test]
fn exact_multiple_of_64_is_unchanged_by_rounding() {
    assert_eq!(mbuf_buf_size_for_frame(1536), 128 + 1536);
    assert_eq!(mbuf_buf_size_for_frame(64), 128 + 64);
}

#[test]
fn one_byte_past_a_multiple_of_64_rounds_up_to_the_next_multiple() {
    assert_eq!(mbuf_buf_size_for_frame(1537), 128 + 1600);
    assert_eq!(mbuf_buf_size_for_frame(1537), 1728);
    assert_eq!(mbuf_buf_size_for_frame(65), 128 + 128);
}

#[test]
fn zero_length_frame_is_just_the_headroom() {
    assert_eq!(mbuf_buf_size_for_frame(0), 128);
}

#[test]
fn dpds_frame_size_derived_from_mtu_1500() {
    // DPDS: mbuf_buf_size_for_frame(MTU + 14 + 4) with MTU = 1500.
    assert_eq!(mbuf_buf_size_for_frame(1500 + 14 + 4), 1664);
    // and jumbo: 9000 + 18 = 9018 -> 9024 + 128.
    assert_eq!(mbuf_buf_size_for_frame(9000 + 14 + 4), 9152);
}

#[test]
fn is_usable_in_const_context() {
    const SIZE: u16 = mbuf_buf_size_for_frame(1518);
    assert_eq!(SIZE, 1664);
}

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
            "mbuf_buf_size_for_frame({frame_len}) = {result}"
        );
    }
}

#[test]
fn rounding_adds_less_than_one_stride_and_lands_on_a_multiple_of_64() {
    // Exhaustive over every frame length for which the u16 result is defined.
    for frame_len in 0u16..=65_000 {
        let result = mbuf_buf_size_for_frame(frame_len);
        let data_room = result - 128;
        assert_eq!(data_room % 64, 0, "frame {frame_len}");
        assert!(
            data_room >= frame_len && data_room - frame_len < 64,
            "frame {frame_len}"
        );
    }
}

#[test]
fn data_room_past_headroom_fits_a_full_frame_without_rx_scatter() {
    for frame in [1518u16, PEOF_MAX_FRAME_LEN] {
        let result = mbuf_buf_size_for_frame(frame);
        assert!(result - dpdk_sys::RTE_PKTMBUF_HEADROOM as u16 >= frame);
    }
}

#[test]
fn result_for_a_standard_frame_is_smaller_than_the_dpdk_default() {
    for frame in [1518u16, PEOF_MAX_FRAME_LEN] {
        assert!(mbuf_buf_size_for_frame(frame) < Mempool::DEFAULT_BUF_SIZE);
    }
}
