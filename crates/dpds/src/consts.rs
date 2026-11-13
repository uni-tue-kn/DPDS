//! Static settings (C `common/config.h`).

/// RX burst size of every worker.
pub const BURST_SIZE: usize = 32;

/// Capacity of the TX staging array: due packets go out in one `tx_burst` of up
/// to this many.
pub const TX_BURST_SIZE: usize = 64;

/// A staged batch is sent at the latest this long (ns) after the send time of
/// its oldest packet, however few packets it holds.
pub const TX_MAX_WAIT_NS: f64 = 10_000.0;

/// RX descriptors requested per NIC queue; the driver clamps it
/// (`rte_eth_dev_adjust_nb_rx_tx_desc`).
pub const NIC_RX_DESCRIPTORS: u16 = 4096;

/// TX descriptors requested per NIC queue (clamped like the RX ones): a long ring, so that a burst
/// of due packets finds room. 32768 is the largest count mlx5 can use: the ethdev API takes a u16
/// and mlx5 rounds up to a power of two within it, so 65535 would wrap to 0 on a device that
/// allows it (`mlx5_tx_queue_pre_setup`, DPDK 26.07).
pub const NIC_TX_DESCRIPTORS: u16 = 32768;

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
