//! Unit tests for the `dpds` library crate (one worker lcore per direction,
//! preof-style DPDK telemetry). None of them needs an EAL. They must compile
//! and pass both with and without `--features telemetry`; tests that need
//! real counters are gated on `#[cfg(feature = "telemetry")]`.
//!
//! These tests fix the test-facing API below.
//! `unsafe fn` = the caller promises valid, owned mbufs (tests call them in
//! `unsafe {}` with fake mbufs that mocks never dereference).
//!
//! ```text
//! dpds::consts   BURST_SIZE: usize = 32            (renamed from MAX_BURST_SIZE, which is gone)
//!                NIC_DESCRIPTORS: u16 = 4096;  MBUF_CACHE_SIZE: u32 = 512;  MTU: u32 = 1500;
//!                HW_STATS_POLL_INTERVAL_MS: u64 = 250
//!                (DEFAULT_NUM_MBUFS is gone: the pool size is derived, see setup)
//!
//! dpds::cli      struct Cli { eal_args: Vec<String>, config: Option<PathBuf>,
//!                             config0: Option<PathBuf>, config1: Option<PathBuf>,
//!                             (no num_mbufs: the pool size is derived)
//!                  (raw flags, None when absent)
//!                fn parse_args(&[String]) -> Result<Cli, CliError>
//!                  flags: --config --config0 --config1 (--num-mbufs is an unknown flag);
//!                  a flag given twice, a missing value, an unknown flag => Err whose Display
//!                  names the offending flag and contains the usage text ("Usage").
//!                Cli::direction_configs(&self, num_ports: usize)
//!                  -> Result<Vec<PathBuf>, CliError>
//!                  one path per direction: config<d> else config else "config.json";
//!                  Err naming "--config1" if num_ports == 1 and --config1 was given.
//!                CliError: std::error::Error + Debug + Clone + PartialEq + Display
//!
//! dpds::setup    enum SetupError { LcoreCount { found: usize, required: usize },
//!                                  PortCount(usize) }   (PoolTooSmall is gone)
//!                    : Debug + Clone + Copy + PartialEq + Eq + Display + Error
//!                fn validate_lcore_count(lcores: usize, num_ports: usize) -> Result<(), SetupError>
//!                  Ok iff lcores == 1 + num_ports (lcores counts the main lcore);
//!                  Err(LcoreCount { found, required: 1 + num_ports }); Display contains
//!                  "1 port needs 2 lcores, e.g. -l 0-1" resp.
//!                  "2 ports need 3 lcores, e.g. -l 0-2" and the found count.
//!                fn validate_port_count(ports: usize) -> Result<(), SetupError>  (unchanged: 1, 2)
//!                fn egress_port(direction: u16, num_ports: usize) -> u16
//!                  (num_ports == 1 -> 0; num_ports == 2 -> direction ^ 1)
//!                fn required_mbufs(buffer_capacity: usize, rx_desc: u16, tx_desc: u16) -> u64
//!                  = buffer_capacity + rx_desc + tx_desc + MBUF_CACHE_SIZE * 3 / 2
//!                    + 2 * BURST_SIZE   (integer arithmetic, saturating, never panics)
//!                  = the pool size of a direction (rx of port d, tx of port e(d));
//!                  `check_pool` is gone.
//!
//! dpds::quit     struct Quit (new/Default/request/is_requested); static QUIT   (unchanged)
//!
//! dpds::io       trait RxBurst  { fn rx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 }
//!                trait TxBurst  { fn tx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 }
//!                trait MbufFree { unsafe fn free_bulk(&mut self, mbufs: &mut [RawMbuf]) }
//!                  the single-mbuf `free` is removed; the loop never calls free_bulk with
//!                  an empty slice.
//!                (RawMbuf = dpdk::RawMbuf = *mut dpdk_sys::rte_mbuf)
//!
//! dpds::worker   struct Worker (Send; owns the direction's Buffer)
//!                Worker::new(buffer: dpds_core::buffer::Buffer, rx_port: u16, tx_port: u16)
//!                Worker::rx_port(&self) -> u16;  Worker::tx_port(&self) -> u16
//!                Worker::has_backlog(&self) -> bool          (!buffer.is_empty())
//!                (everything else of Worker is internal; modules `rx` and `tx` are removed)
//!
//! dpds::forward  unsafe fn run(worker: Worker, rx_nic: &mut impl RxBurst,
//!                         tx_nic: &mut impl TxBurst, free: &mut impl MbufFree,
//!                         t: &mut Telemetry<'_>, quit: &Quit,
//!                         now_ns: impl FnMut() -> f64)       (7 params, no #[allow])
//!                one direction per call: RX on worker.rx_port(), TX on worker.tx_port().
//!                The clock is a plain closure; tests count its calls.
//!                Per iteration, observable order: Clock, [Rx(rx_port, BURST_SIZE slots)
//!                unless quit], [Free(all rejects) if any], [Tx(tx_port, k) if k > 0],
//!                [Free(unsent) if any]. Details at the top of forward.rs.
//!
//! dpds::first_loss, dpds::counters, dpds::reply   (unchanged, see their test modules)
//!
//! dpds::stats    struct Stats { first_loss, counters, epochs }  (one per direction; pub fields,
//!                    Default); Stats::stats_reply(&self, uptime_ns: u64) -> Reply and
//!                    Stats::epochs_reply(&self) -> Reply (unchanged);
//!                    Stats::register_telemetry is REMOVED (see DirectionStats)
//!                struct Telemetry<'a>  (unchanged: new / record_loss / tick, pub stats)
//!                struct DirectionStats { pub stats: Vec<Stats> }
//!                    DirectionStats::new(num_directions: usize)
//!                    DirectionStats::stats_reply(&self, param: &str, uptime_ns: u64) -> Option<Reply>
//!                    DirectionStats::epochs_reply(&self, param: &str) -> Option<Reply>
//!                      param = plain decimal index of an existing direction ("0", "1");
//!                      anything else ("", "2", "x", "-1", "+1", " 0", "0,1") => None
//!                      (registration then leaves the dict empty, like /nic/epochs,<port>)
//!                    DirectionStats::register_telemetry(&'static DirectionStats, start_ns: f64)
//!                      -> Result<(), DpdkError>   (registers /dpds/stats and /dpds/epochs;
//!                      Ok(()) no-op without the feature)
//!                struct PortInfo { name, rx_ring, tx_ring, rx_ring_requested,
//!                    tx_ring_requested }                                (unchanged)
//!                struct DirectionInfo { rx_port: u16, tx_port: u16, lcore: u32,
//!                    config: String, pool_size: u32, raw_free: bool }
//!                    : Debug + Clone + PartialEq + Eq
//!                fn ports_reply(ports: &[PortInfo], directions: &[DirectionInfo]) -> Reply
//!                    keys in this order: num_ports, per port the five port<i>_* keys (as
//!                    today), then num_directions, then per direction d in order
//!                    dir<d>_rx_port, dir<d>_tx_port, dir<d>_lcore, dir<d>_config (Str),
//!                    dir<d>_pool_size, dir<d>_raw_free (Uint 0/1).
//!                fn register_port_telemetry(Vec<PortInfo>, Vec<DirectionInfo>)
//!                    -> Result<(), DpdkError>
//!                NicHwStats / NicPoller                                         (unchanged)
//! ```
//!
//! Semantics are documented at the top of each test module.

mod cli;
mod counters;
mod first_loss;
mod forward;
mod quit;
mod setup;
mod stats;
mod support;
mod worker;
