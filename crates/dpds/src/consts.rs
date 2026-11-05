//! Static settings (C `common/config.h`).

/// RX burst size and per-iteration pop budget of every worker (preof's
/// `net_config::BURST_SIZE`).
pub const BURST_SIZE: usize = 32;

/// Capacity of the TX staging array: due packets are sent with one `tx_burst`
/// of up to this many (spec 57).
pub const TX_BURST_SIZE: usize = 64;

/// A staged batch is sent at the latest this long (ns) after the send time of
/// its oldest packet, however few packets it holds.
pub const TX_MAX_WAIT_NS: f64 = 10_000.0;

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

/// DPDK's `RTE_TEL_MAX_DICT_ENTRIES`: the most keys one telemetry dict (one
/// reply) holds. `/dpds/ports` has `3 + 5 * ports + 10 * directions` of them.
pub const MAX_TELEMETRY_DICT_ENTRIES: usize = 256;
