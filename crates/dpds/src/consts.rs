//! Static settings (C `common/config.h`).

/// RX burst size of every worker.
pub const BURST_SIZE: usize = 32;

/// Capacity of the TX staging array: due packets go out in one `tx_burst` of up
/// to this many.
pub const TX_BURST_SIZE: usize = 64;

/// A staged batch is sent at the latest this long (ns) after the send time of
/// its oldest packet, however few packets it holds.
pub const TX_MAX_WAIT_NS: f64 = 10_000.0;

/// RX/TX descriptors requested per NIC queue; the driver clamps it
/// (`rte_eth_dev_adjust_nb_rx_tx_desc`).
pub const NIC_DESCRIPTORS: u16 = 4096;

/// Per-lcore mbuf cache size of each mempool.
pub const MBUF_CACHE_SIZE: u32 = 512;

/// Data room of every mbuf: headroom plus 2048 B (DPDK's default). It fits the largest frame and
/// net_af_packet's default frame size of 2048 B without RX scatter.
pub const MBUF_DATA_ROOM: u16 = dpdk::Mempool::DEFAULT_BUF_SIZE;

/// MTU requested from every port.
pub const MTU: u32 = 1500;

/// Main lcore's poll interval for the NIC hardware counters and the quit flag.
pub const HW_STATS_POLL_INTERVAL_MS: u64 = 250;

/// DPDK's `RTE_TEL_MAX_DICT_ENTRIES`: the most keys in one telemetry dict.
pub const MAX_TELEMETRY_DICT_ENTRIES: usize = 256;
