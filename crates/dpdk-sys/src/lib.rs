#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(unnecessary_transmutes)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

// ── Constants that bindgen cannot fold (macro expressions) ───────────────────

// RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE = RTE_BIT64(16) = 1u64 << 16
pub const RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE: u64 = 1u64 << 16;
