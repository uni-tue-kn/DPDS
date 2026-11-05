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
//!                  Err(LcoreCount { found, required: 1 + num_ports }); Display counts
//!                  links: "<n> link(s) need <n+1> lcores (main + one worker per link),
//!                  e.g. -l 0-<n>" and contains the found count (see setup.rs).
//!                fn validate_port_count(ports: usize) -> Result<(), SetupError>  (unchanged: 1, 2)
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
//! ## v5 additions (multi-link, drop causes, delay statistics) -- all NEW API
//!
//! Full definitions (and the pinned semantics) are in the header of the module
//! named in brackets; this is the checklist for the implementer.
//!
//! ```text
//! dpds_core::buffer   PushOutcome, Buffer::push_outcome, Buffer::pop_due_with_time
//!                     (crates/dpds-core/tests/push_outcome.rs)
//! dpds::cli           Cli.links: Option<PathBuf>; `--links <file>`, mutually exclusive
//!                     with --config/--config0/--config1                       [cli.rs]
//! dpds::links         LinkSpec, parse_links, load_links, num_directions,
//!                     LinkDirection{.., histogram, lcore}, LinkSpec.{histograms, lcores},
//!                     resolve_directions(links, probed, main_lcore, workers),
//!                     LinksError, MAX_NAME_LEN                                [links.rs]
//! dpds::setup         validate_lcore_count(lcores, num_directions): unchanged fn, 1 + n [setup.rs]
//! dpds::counters      DpdsCounters.{loss_drops, limiter_drops, buffer_full_drops};
//!                     EPOCH_COUNTER_NAMES / cumulative_snapshot(): 12 entries, the
//!                     three causes appended; DpdsEpochLog = EpochLog<12, 2>   [causes.rs, counters.rs]
//! dpds::first_loss    first_loss_detail of a DpdsReject: "from=dpds,cause=loss|limiter|buffer_full"
//!                     (StageLossDetail::Single -> "from=dpds" stays)            [causes.rs]
//! dpds::delay_stats   HIST_BINS, MIN_WIDTH_NS, HistRange{lo_ns,bin_ns} (new, default,
//!                     from_config, hi_ns, bin), Bin{Underflow,Index,Overflow}   [delay_stats.rs]
//! dpds::stats         Stats::with_range(HistRange), Stats::delay_hist_reply(),
//!                     DirectionStats::with_ranges(Vec<HistRange>),
//!                     DirectionStats::delay_hist_reply(param) -> Option<Reply>,
//!                     stats_reply: 26 keys (+loss_drops, limiter_drops,
//!                     buffer_full_drops, delay_mean_ns, delay_std_ns, delay_min_ns,
//!                     delay_max_ns, lateness_mean_ns, lateness_max_ns),
//!                     DirectionInfo += link: String, rx_name: String, tx_name: String,
//!                     ports_reply(ports, directions, num_links: usize): after
//!                     num_directions comes `num_links`; per direction the six old
//!                     keys then dir<d>_link, dir<d>_rx_name, dir<d>_tx_name,
//!                     register_port_telemetry(ports, directions, num_links)    [stats.rs]
//! ```
//!
//! ## v6 fix round (D1, D2, D4, D5) -- new API
//!
//! ```text
//! dpds::links         #[serde(deny_unknown_fields)] on the file, link and histogram objects
//!                     (D1); parse_links also refuses a file whose /dpds/ports reply would
//!                     exceed 256 keys: 3 + 5 * ports + 10 * directions (D2);
//!                     LinkSpec.delay_stats: Vec<bool> (always configs.len() entries, default
//!                     all true), LinkDirection.delay_stats: bool (D5)         [links.rs]
//! dpds::delay_stats   HistRange::with_bins(..) -> Result<HistRange, HistogramError>, Err
//!                     for 0 bins (D4)                                 [delay_stats.rs]
//! dpds::stats         Stats::with_delay_stats(HistRange, enabled: bool),
//!                     DirectionStats::with_delay_settings(Vec<(HistRange, bool)>);
//!                     off: the six delay_* / lateness_* keys of stats_reply are absent and
//!                     delay_hist_reply is empty; DirectionInfo.delay_stats: bool and
//!                     ports_reply gets dir<d>_delay_stats (Uint 0/1) as the LAST key of a
//!                     direction (ten keys per direction)           [delay_stats.rs, stats.rs]
//! ```
//!
//! ## v10: directed links -- the v5/v6 `dpds::links` API above is REPLACED
//!
//! ```text
//! dpds::links         a link is one direction ingress -> egress (ingress == egress, a
//!                     hairpin, is allowed); each port at most once as ingress and at most
//!                     once as egress.
//!                     LinkSpec { name, ingress, egress, config, lcore: Option<u32>,
//!                                histogram, delay_stats: bool }   (v1 `ports`, `configs`,
//!                     `lcores`, `histograms`, the per-direction `delay_stats` array: gone;
//!                     the v1 keys are unknown fields)
//!                     parse_links / load_links / MAX_NAME_LEN / LinksError as before;
//!                     the dict limit is 3 + 5 * distinct ports + 10 * links <= 256
//!                     resolve_directions(links, probed, main_lcore, workers) returns ONE
//!                     LinkDirection per link (rx_port = ingress id, tx_port = egress id);
//!                     every probed port must be used as ingress and/or egress
//! dpds::stats         ports_reply keeps its shape; `num_links == num_directions`
//! dpds::setup         validate_lcore_count(1 + links, links)
//! ```
//!
//! Semantics are documented at the top of each test module.

mod causes;
mod cli;
mod counters;
mod delay_stats;
mod first_loss;
mod forward;
mod hist_pages;
mod links;
mod quit;
mod readme;
mod setup;
mod spec51;
mod stats;
mod support;
mod worker;
