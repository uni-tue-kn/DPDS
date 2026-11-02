//! Static settings (C `common/config.h`).

/// Largest burst handled by every stage.
pub const MAX_BURST_SIZE: usize = 16;

/// RX/TX descriptors requested per NIC queue (the driver clamps to what it
/// supports via `rte_eth_dev_adjust_nb_rx_tx_desc`).
pub const NIC_DESCRIPTORS: u16 = 65535;

/// Per-lcore mbuf cache size of the mempool.
pub const MBUF_CACHE_SIZE: u32 = 512;

/// MTU requested from every port.
pub const MTU: u32 = 1500;

/// Default number of mbufs per port (the pool holds this times the number of ports).
pub const DEFAULT_NUM_MBUFS: u32 = 1024 * 1024 - 1;

/// Main lcore's poll interval for the NIC hardware counters
/// (`eth_stats_get`) and for noticing the quit flag.
pub const HW_STATS_POLL_INTERVAL_MS: u64 = 250;
