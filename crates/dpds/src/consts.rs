//! Static settings (C `common/config.h`).

/// Largest burst handled by every stage.
pub const MAX_BURST_SIZE: usize = 16;

/// Burst-size histogram buckets. Bucket `i` covers burst sizes
/// `[2^i, 2^(i+1) - 1]`; the last bucket is exactly `MAX_BURST_SIZE`.
/// Requires `MAX_BURST_SIZE` to be a power of two.
pub const BURST_HIST_BUCKETS: usize = MAX_BURST_SIZE.trailing_zeros() as usize + 1;

/// Inter-stage software ring size. DPDK 26.07 rings must be a power of two,
/// so the usable capacity is `RING_SIZE - 1` = 65535 (the C app's size).
pub const RING_SIZE: u32 = 65536;

/// RX/TX descriptors requested per NIC queue (the driver clamps to what it
/// supports via `rte_eth_dev_adjust_nb_rx_tx_desc`).
pub const NIC_DESCRIPTORS: u16 = 65535;

/// Per-lcore mbuf cache size of the mempool.
pub const MBUF_CACHE_SIZE: u32 = 512;

/// MTU requested from every port.
pub const MTU: u32 = 1500;

/// Default number of mbufs per port (the pool holds this times the number of ports).
pub const DEFAULT_NUM_MBUFS: u32 = 1024 * 1024 - 1;
