//! Static settings (C `common/config.h`).

/// RX burst size and per-iteration pop budget of every worker (preof's
/// `net_config::BURST_SIZE`).
pub const BURST_SIZE: usize = 32;

/// RX/TX descriptors requested per NIC queue (the driver clamps to what it
/// supports via `rte_eth_dev_adjust_nb_rx_tx_desc`).
pub const NIC_DESCRIPTORS: u16 = 4096;

/// Per-lcore mbuf cache size of each mempool.
pub const MBUF_CACHE_SIZE: u32 = 512;

/// MTU requested from every port.
pub const MTU: u32 = 1500;

/// Main lcore's poll interval for the NIC hardware counters
/// (`eth_stats_get`) and for noticing the quit flag.
pub const HW_STATS_POLL_INTERVAL_MS: u64 = 250;
