//! Unit tests for the `dpds` library crate. None of them needs an EAL.
//!
//! ## Public API these tests assume (all re-exported/declared from `dpds::lib`)
//!
//! ```text
//! dpds::consts   MAX_BURST_SIZE: usize = 16
//!                BURST_HIST_BUCKETS: usize = 5      (log2(MAX_BURST_SIZE) + 1)
//!                RING_SIZE: u32 = 65536             (inter-stage rings; usable 65535)
//!                NIC_DESCRIPTORS: u16 = 65535       (RX/TX descriptors requested)
//!                MBUF_CACHE_SIZE: u32 = 512
//!                MTU: u32 = 1500
//!                DEFAULT_NUM_MBUFS: u32 = 1_048_575 (per port)
//!
//! dpds::cli      struct Cli { eal_args: Vec<String>, config: PathBuf,
//!                             stats_file: Option<PathBuf>, num_mbufs: u32 }
//!                fn parse_args(args: &[String]) -> Result<Cli, CliError>
//!                struct CliError: Display + std::error::Error (message holds usage)
//!
//! dpds::layout   enum Layout { Combined, RxAndWorkerTx, Separate }
//!                Layout::from_lcore_count(lcores: usize) -> Result<Layout, LayoutError>
//!                Layout::worker_lcores(self) -> usize
//!                Layout::rings(self) -> usize
//!                fn validate_port_count(ports: usize) -> Result<(), LayoutError>
//!                enum LayoutError { LcoreCount(usize), PortCount(usize) }: Display
//!
//! dpds::io       traits over the NIC / mempool, implemented by mocks in tests:
//!                trait RxBurst  { fn rx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16; }
//!                trait TxBurst  { fn tx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16; }
//!                trait MbufFree { fn free(&mut self, mbuf: RawMbuf); }
//!                trait Enqueue  { fn enqueue_burst(&mut self, bufs: &[RawMbuf]) -> u16; }
//!                (RawMbuf = dpdk::RawMbuf = *mut dpdk_sys::rte_mbuf)
//!
//! dpds::stats    RxStats / WorkerStats / TxStats: #[repr(align(64))], Default,
//!                  AtomicU64 pub fields as in the C structs
//!                  (rx: received, enqueued, rx_ring_drops, burst_hist: [AtomicU64; 5];
//!                   worker: received, sent, emulator_drops, tx_ring_drops;
//!                   tx: received, transmitted, tx_drops, burst_hist: [AtomicU64; 5])
//!                PipelineStats { rx: RxStats, worker: WorkerStats, tx: TxStats }: Default
//!                  PipelineStats::snapshot(&self) -> StatsSnapshot   (Relaxed loads)
//!                StatsSnapshot { rx: RxSnapshot, worker: WorkerSnapshot, tx: TxSnapshot }
//!                  plain u64 fields with the same names; Default, Clone, Copy, PartialEq, Debug
//!                NicStats { imissed, ierrors, oerrors, rx_nombuf: u64 }: Default, Clone, Copy
//!                fn burst_bucket(n: usize) -> usize                       (floor(log2 n), n > 0)
//!                fn format_burst_hist(label: &str, cur: &[u64; 5], prev: &[u64; 5],
//!                                     delta_pkts: u64) -> String
//!                fn format_dashboard(cur: &StatsSnapshot, prev: &StatsSnapshot,
//!                                    nics: &[(u16, NicStats)], overwrite: bool) -> String
//!                fn format_csv(stats: &StatsSnapshot, nics: &[(u16, NicStats)]) -> String
//!
//! dpds::rx       Rx::new(num_ports: u16) -> Rx;  Rx::next_port(&self) -> u16
//!                Rx::step(&mut self, q: &mut impl RxBurst, stats: &RxStats,
//!                         batch: &mut [RawMbuf; MAX_BURST_SIZE]) -> usize
//!                fn enqueue_burst(ring: &mut impl Enqueue, free: &mut impl MbufFree,
//!                                 stats: &RxStats, batch: &[RawMbuf])
//!
//! dpds::worker   Worker::new(buffer: dpds_core::buffer::Buffer, num_ports: u16) -> Worker
//!                Worker::has_backlog(&self) -> bool
//!                Worker::step(&mut self, stats: &WorkerStats, free: &mut impl MbufFree,
//!                             now_ns: impl FnMut() -> f64, in_batch: &[RawMbuf],
//!                             out_batch: &mut [RawMbuf; MAX_BURST_SIZE]) -> usize
//!                fn enqueue_burst(ring: &mut impl Enqueue, free: &mut impl MbufFree,
//!                                 stats: &WorkerStats, batch: &[RawMbuf])
//!
//! dpds::tx       Tx::new(num_ports: u16) -> Tx;  Tx::pending(&self, port: u16) -> usize
//!                Tx::step(&mut self, nic: &mut impl TxBurst, free: &mut impl MbufFree,
//!                         stats: &TxStats, batch: &[RawMbuf])
//!                Tx::cleanup(&mut self, nic: &mut impl TxBurst, free: &mut impl MbufFree,
//!                            stats: &TxStats)
//! ```
//!
//! Semantics are documented at the top of each test module.

mod cli;
mod layout;
mod rx;
mod stats;
mod support;
mod tx;
mod worker;
