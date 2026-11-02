//! Unit tests for the `dpds` library crate (single-core DPDS with preof-style
//! DPDK telemetry). None of them needs an EAL. They must compile and pass
//! both with and without `--features telemetry`; tests that need real
//! counters are gated on `#[cfg(feature = "telemetry")]`.
//!
//! ## Public API these tests assume (all in the `dpds` lib crate)
//!
//! `unsafe fn` = the caller promises valid, owned mbufs (tests call them in
//! `unsafe {}` with fake mbufs that mocks never dereference).
//!
//! ```text
//! dpds::consts   MAX_BURST_SIZE: usize = 16;  NIC_DESCRIPTORS: u16 = 65535;
//!                MBUF_CACHE_SIZE: u32 = 512;  MTU: u32 = 1500;
//!                DEFAULT_NUM_MBUFS: u32 = 1_048_575;
//!                HW_STATS_POLL_INTERVAL_MS: u64 = 250
//!               
//!
//! dpds::cli      struct Cli { eal_args: Vec<String>, config: PathBuf, num_mbufs: u32 }
//!                  (stats_file is gone, `-o` is an unknown argument)
//!                fn parse_args(&[String]) -> Result<Cli, CliError>; CliError: Error
//!                Usage: dpds <EAL args...> -- [--config <path>] [--num-mbufs <N>]
//!
//! dpds::setup    enum SetupError { LcoreCount(usize), PortCount(usize) }: Debug+Display+Error
//!                fn validate_lcore_count(lcores: usize) -> Result<(), SetupError>  (Ok only for 2)
//!                fn validate_port_count(ports: usize) -> Result<(), SetupError>    (Ok for 1, 2)
//!
//! dpds::quit     struct Quit; Quit::new() (const fn) / Default / request(&self) /
//!                is_requested(&self) -> bool;  static QUIT: Quit
//!
//! dpds::io       trait RxBurst  { fn rx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 }
//!                trait TxBurst  { fn tx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 }
//!                trait MbufFree { unsafe fn free(&mut self, mbuf: RawMbuf) }   <- unsafe fn now
//!                NicRx / NicTx / PktmbufFree
//!                (RawMbuf = dpdk::RawMbuf = *mut dpdk_sys::rte_mbuf)
//!
//! dpds::rx       Rx::new(num_ports: u16);  Rx::next_port(&self) -> u16
//!                Rx::step(&mut self, q: &mut impl RxBurst, t: &mut Telemetry<'_>,
//!                         batch: &mut [RawMbuf; MAX_BURST_SIZE]) -> usize        (safe fn)
//! dpds::worker   Worker::new(buffer: dpds_core::buffer::Buffer, num_ports: u16);
//!                Worker::num_ports(&self) -> u16;  Worker::has_backlog(&self) -> bool
//!                unsafe Worker::step(&mut self, t: &mut Telemetry<'_>, free: &mut impl MbufFree,
//!                         now_ns: impl FnMut() -> f64, in_batch: &[RawMbuf],
//!                         out_batch: &mut [RawMbuf; MAX_BURST_SIZE]) -> usize
//! dpds::tx       Tx::new(num_ports: u16);  Tx::pending(&self, port: u16) -> usize
//!                unsafe Tx::step(&mut self, nic: &mut impl TxBurst, free: &mut impl MbufFree,
//!                         t: &mut Telemetry<'_>, now_ns: impl FnMut() -> f64, batch: &[RawMbuf])
//!                Tx::cleanup(&mut self, nic, free, t: &mut Telemetry<'_>,
//!                         now_ns: impl FnMut() -> f64)                           (safe fn)
//! dpds::forward  unsafe fn run(worker: Worker, rx_nic: &mut impl RxBurst,
//!                         tx_nic: &mut impl TxBurst, free: &mut impl MbufFree,
//!                         t: &mut Telemetry<'_>, quit: &Quit, now_ns: impl FnMut() -> f64)
//!                (the fused RX -> Worker -> TX loop, 7 params = clippy's limit, no #[allow];
//!                 the port count is `worker.num_ports()`;
//!                 semantics at the top of forward.rs)
//!
//! dpds::first_loss   enum FirstLossReason { EmulatorReject, DescriptorFull, RxMissed,
//!                        RxNoMbuf, RxError, TxError }: Debug+Clone+Copy+PartialEq+Eq+Display
//!                    enum Stage { Dpds, Nic }: Display ("dpds", "nic")
//!                    enum StageLossDetail { Single{from}, RingFull{from,to},
//!                        HardwareDrop{from, count: u32} }
//!                    struct StageLossEvent { reason, detail, relative_ns: f64 }: Debug+Clone+Copy+PartialEq
//!                    struct StageLossTracker: new() / Default / try_record(&self, reason,
//!                        detail, relative_ns) -> bool / current(&self) -> Option<StageLossEvent>
//!                        (telemetry: lock-free atomics, align 64; otherwise unit struct `StageLossTracker`)
//!                    struct FirstLossFields { reason: String, detail: String, relative_ns: u64 }
//!                    fn format_first_loss(Option<StageLossEvent>) -> FirstLossFields   (always compiled)
//!                    fn detect_hardware_drop(&PortStats, &PortStats) -> Option<(FirstLossReason, u32)>
//!
//! dpds::counters  struct Counter  { inc, add(u64), get }, struct MaxGauge { observe(u64),
//!                    observe_with(impl FnOnce() -> u64), get, take }: Default
//!                    (unit structs without the feature, atomics with it; same API)
//!                 fn mean_2dp(total: u64, count: u64) -> String
//!                 struct DpdsCounters (align 64, Default): Counter fields polls, rx_pkts,
//!                    rx_full_bursts, emulator_rejects, pops, tx_calls, tx_pkts, tx_sent,
//!                    tx_short_calls; MaxGauge fields tx_batch_max, buffer_len_max;
//!                    EPOCH_COUNTER_NAMES: [&str; 9] (that counter order), EPOCH_GAUGE_NAMES:
//!                    [&str; 2] = [tx_batch_max, buffer_len_max]; cumulative_snapshot() -> [u64; 9]
//!                 type DpdsEpochLog = EpochLog<9, 2>
//!                 const EPOCH_CAPACITY: usize = 32;  const EPOCH_IDLE_GAP_NS: f64 = 500_000_000.0
//!                 fn should_close_epoch(now_ns, last_arrival_ns, saw_arrival_this_epoch) -> bool
//!                 struct EpochBoundary { start_uptime_ns: f64, end_uptime_ns: f64 }  (not gated)
//!                 struct EpochDriver: new(start_uptime_ns: f64) / tick(&mut self, now_ns: f64,
//!                    arrivals: u64) -> Option<EpochBoundary>; with the feature the pub fields
//!                    last_arrival_ns, saw_arrival_this_epoch, epoch_start_uptime_ns
//!                 struct EpochFirstLoss: new() / Default / record(&mut self, &StageLossTracker,
//!                    reason, detail, relative_ns) / take(&mut self) -> Option<StageLossEvent>
//!                 struct EpochRecord<NC, NG> { start_uptime_ns: u64, end_uptime_ns: u64,
//!                    counters: [u64; NC], gauges: [u64; NG] }: Debug+Clone+Copy+PartialEq+Eq (not gated)
//!                 struct EpochLog<NC, NG>: new() / Default / epochs_closed() -> u32 /
//!                    epochs_dropped() -> u32 / close(&self, cumulative: [u64; NC], gauges: [u64; NG],
//!                    start: u64, end: u64, first_loss: Option<StageLossEvent>) /
//!                    record(&self, usize) -> Option<EpochRecord> / first_loss(&self, usize)
//!                    -> Option<Option<StageLossEvent>> / aggregate_gauge_max(&self, usize, live: u64) -> u64
//!                 fn epochs_reply<NC, NG>(&EpochLog, &[&str; NC], &[&str; NG]) -> Reply  (feature only is ok)
//!                 struct NicEpochRecord { start_uptime_ns, end_uptime_ns: f64, rx_packets,
//!                    imissed, rx_port_discards: u64 }: Debug+Clone+Copy+PartialEq+Default (not gated)
//!                 type NicEpochLog = EpochLog<3, 0>  (EPOCH_COUNTER_NAMES = [rx_packets, imissed,
//!                    rx_port_discards], EPOCH_GAUGE_NAMES = [])
//!                 struct NicEpochTracker: new(stage_start_ns: f64, rx_packets_cum: u64,
//!                    imissed_cum: u64, rx_port_discards_cum: u64) / poll(&mut self, now_ns: f64,
//!                    rx_packets_cum: u64, imissed_cum: u64, read_port_discards: impl FnOnce() ->
//!                    Option<u64>) -> Option<NicEpochRecord>
//!                 Without the `telemetry` feature Counter, MaxGauge, EpochLog, EpochDriver,
//!                    EpochFirstLoss, NicEpochTracker, StageLossTracker are ZERO-SIZED UNIT STRUCTS
//!                    (tests construct them as `Counter`, `MaxGauge`, `EpochLog`, `EpochFirstLoss`,
//!                    `StageLossTracker`) and their methods are no-ops (see counters.rs `zero_cost`).
//!
//! dpds::reply    enum Value { Uint(u64), Str(String), UintArray(Vec<u64>), StrArray(Vec<String>) }
//!                  : Debug+Clone+PartialEq+Eq;   type Reply = Vec<(String, Value)>
//!
//! dpds::stats    struct Stats { first_loss: StageLossTracker, counters: DpdsCounters,
//!                    epochs: DpdsEpochLog } (pub fields, Default);
//!                  Stats::stats_reply(&self, uptime_ns: u64) -> Reply     (/dpds/stats)
//!                  Stats::epochs_reply(&self) -> Reply                    (/dpds/epochs)
//!                  Stats::register_telemetry(&'static Stats, start_ns: f64) -> Result<(), DpdkError>
//!                struct Telemetry<'a> { pub stats: &'a Stats, .. }; Telemetry::new(&'a Stats,
//!                    start_ns: f64); record_loss(&mut self, reason, detail, now_ns: f64);
//!                    tick(&mut self, now_ns: f64, arrivals: u64)
//!                struct PortInfo { name: String, rx_ring, tx_ring, rx_ring_requested,
//!                    tx_ring_requested: u16 }: Debug+Clone+PartialEq+Eq
//!                fn ports_reply(&[PortInfo]) -> Reply                    (/dpds/ports)
//!                fn register_port_telemetry(Vec<PortInfo>) -> Result<(), DpdkError>
//!                struct NicHwStats { pub first_loss: Vec<StageLossTracker>, pub epochs:
//!                    Vec<NicEpochLog> }; NicHwStats::new(num_ports: usize);
//!                    hw_first_loss_reply(&self, param: &str) -> Option<Reply>   (/nic/hw_first_loss,<port>)
//!                    epochs_reply(&self, param: &str) -> Option<Reply>          (/nic/epochs,<port>)
//!                    register_telemetry(&'static NicHwStats) -> Result<(), DpdkError>
//!                struct NicPoller: new(port: usize, start_ns: f64, initial: &PortStats,
//!                    initial_discards: u64); poll(&mut self, now_ns: f64, curr: &PortStats,
//!                    hw: &NicHwStats, read_port_discards: impl FnOnce() -> Option<u64>)
//!                The `*_reply`/`ports_reply` builders are only referenced by tests under the
//!                feature (they may be `#[cfg(feature = "telemetry")]`-gated); everything else
//!                in dpds::stats exists in both builds (register_* are Ok(()) no-ops without it).
//! ```
//!
//! Semantics are documented at the top of each test module.

mod cli;
mod counters;
mod first_loss;
mod forward;
mod quit;
mod rx;
mod setup;
mod stats;
mod support;
mod tx;
mod worker;
