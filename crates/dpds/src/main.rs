//! DPDS: DPDK-based network-link emulator. See the README for usage.
//!
//! One worker lcore per link (own config, buffer and mempool; a link is one
//! direction: traffic received on its ingress port leaves on its egress port).
//! The EAL main lcore handles signals and polls the NIC hardware counters; each
//! worker runs RX -> DPDS -> TX (`dpds::forward::run`). The links come from the
//! `--links` file or, with `--config*`, from the one or two probed ports: one
//! port is a hairpin link, two ports are the links port 0 -> port 1 and
//! port 1 -> port 0.

use std::error::Error;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use dpdk::eal::{self, foreach_worker, lcore_count, main_lcore};
use dpdk::mempool::MbufFreer;
use dpdk::port::{self, adjust_nb_rx_tx_desc, foreach_dev, shutdown_port};
use dpdk::{now_ns, Clock, Eal, Mempool, Port, PortDiscardXstats, RxQueue, TxQueue};
use dpds::cli::parse_args;
use dpds::consts::{
    HW_STATS_POLL_INTERVAL_MS, MBUF_CACHE_SIZE, MBUF_DATA_ROOM, MTU, NIC_DESCRIPTORS,
};
use dpds::delay_stats::HistRange;
use dpds::forward;
use dpds::io::{NicRx, NicTx, PktmbufFree, RawFree};
use dpds::links::{load_links, resolve_directions, LinkSpec};
use dpds::quit::QUIT;
use dpds::setup::{required_mbufs, validate_lcore_count, validate_port_count};
use dpds::stats::{
    register_port_telemetry, DirectionInfo, DirectionStats, NicHwStats, NicPoller, PortInfo,
    Telemetry,
};
use dpds::worker::Worker;
use dpds_core::buffer::Buffer;
use dpds_core::Config;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dpds: {e}");
            ExitCode::FAILURE
        }
    }
}

// ── Signals ──────────────────────────────────────────────────────────────────
// SIGINT/SIGTERM set the quit flag: workers stop receiving, drain their delay
// buffers and return; the main lcore then shuts down.

extern "C" fn on_signal(_signum: i32) {
    QUIT.request();
}

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
const SIG_ERR: isize = -1;

extern "C" {
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> isize;
}

// ── Memory locking ───────────────────────────────────────────────────────────
// Lock the process into RAM before EAL starts: with `MCL_FUTURE` every later mapping
// (EAL heap, mempools, the delay buffers) is locked and populated when created, so no
// worker lcore takes a page fault mid-run.

/// `mlockall(2)` flag (Linux): lock every page mapped now.
const MCL_CURRENT: i32 = 1;
/// `mlockall(2)` flag (Linux): lock every page mapped later too.
const MCL_FUTURE: i32 = 2;

/// `getrlimit(2)` resource (Linux): locked-memory limit.
const RLIMIT_MEMLOCK: i32 = 8;
/// `RLIM_INFINITY` (Linux, 64-bit `rlim_t`).
const RLIM_INFINITY: u64 = u64::MAX;

/// `struct rlimit` (Linux, 64-bit `rlim_t`).
#[repr(C)]
struct Rlimit {
    cur: u64,
    max: u64,
}

extern "C" {
    fn mlockall(flags: i32) -> i32;
    fn geteuid() -> u32;
    fn getrlimit(resource: i32, rlim: *mut Rlimit) -> i32;
    fn setrlimit(resource: i32, rlim: *const Rlimit) -> i32;
}

/// What `lock_memory` does for a given effective uid and `RLIMIT_MEMLOCK`.
#[derive(Debug, PartialEq, Eq)]
enum MemlockPlan {
    Lock,
    /// Raise the soft limit to unlimited (the hard limit allows it), then lock.
    RaiseThenLock,
    /// Don't lock: `mlockall` would succeed while the process is small, then `MCL_FUTURE`
    /// counts every later mapping against the finite limit and EAL's allocations fail.
    Skip,
}

fn memlock_plan(euid: u32, soft: u64, hard: u64) -> MemlockPlan {
    if euid == 0 || soft == RLIM_INFINITY {
        MemlockPlan::Lock
    } else if hard == RLIM_INFINITY {
        MemlockPlan::RaiseThenLock
    } else {
        MemlockPlan::Skip
    }
}

/// `mlockall(MCL_CURRENT | MCL_FUTURE)`, unless a finite `RLIMIT_MEMLOCK` makes it harmful
/// (see [`MemlockPlan`]; the skip warns and returns `Ok`). Failure is the caller's warning:
/// running unlocked still works.
fn lock_memory() -> std::result::Result<(), std::io::Error> {
    let mut lim = Rlimit { cur: 0, max: 0 };
    // SAFETY: getrlimit writes one `struct rlimit` into `lim`, which has that layout.
    if unsafe { getrlimit(RLIMIT_MEMLOCK, &mut lim) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: geteuid takes no arguments and touches no memory.
    match memlock_plan(unsafe { geteuid() }, lim.cur, lim.max) {
        MemlockPlan::Lock => {}
        MemlockPlan::RaiseThenLock => {
            let unlimited = Rlimit {
                cur: RLIM_INFINITY,
                max: lim.max,
            };
            // SAFETY: setrlimit reads one `struct rlimit` from `unlimited`.
            if unsafe { setrlimit(RLIMIT_MEMLOCK, &unlimited) } != 0 {
                let e = std::io::Error::last_os_error();
                return Err(std::io::Error::new(
                    e.kind(),
                    format!("cannot raise RLIMIT_MEMLOCK: {e}"),
                ));
            }
        }
        MemlockPlan::Skip => {
            eprintln!(
                "WARNING: RLIMIT_MEMLOCK is {} KiB, not unlimited: continuing unlocked (locking \
                 would make later allocations fail; run as root or set LimitMEMLOCK=infinity / \
                 ulimit -l unlimited)",
                lim.cur / 1024
            );
            return Ok(());
        }
    }
    // SAFETY: mlockall takes only flags and touches no Rust memory.
    if unsafe { mlockall(MCL_CURRENT | MCL_FUTURE) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn install_signal_handlers() -> Result<()> {
    for sig in [SIGINT, SIGTERM] {
        if unsafe { signal(sig, on_signal) } == SIG_ERR {
            return Err(format!("cannot install handler for signal {sig}").into());
        }
    }
    Ok(())
}

// ── Setup ────────────────────────────────────────────────────────────────────

/// A direction as set up before its worker starts.
struct Direction {
    worker: Worker,
    /// The config file, or the links file.
    config: String,
    rx_name: String,
    tx_name: String,
    lcore: u32,
    delay_stats: bool,
    freer: MbufFreer,
    raw_free: bool,
}

/// One mempool of `num_mbufs` per link on the socket of its ingress port (the
/// calling lcore's if the port reports none). Leaked: queue handles and workers
/// need `'static` access.
fn create_pool(
    direction: usize,
    rx_port: u16,
    num_mbufs: u64,
) -> Result<(&'static Mempool, String, u32)> {
    let name = format!("dpds_pool_{direction}");
    let failed = |why: &dyn std::fmt::Display| {
        format!(
            "direction {direction}: cannot create mbuf pool {name} with {num_mbufs} mbufs: {why}"
        )
    };
    let count = u32::try_from(num_mbufs).map_err(|_| failed(&"more than u32::MAX mbufs"))?;
    let port_socket = port::rte_eth_dev_socket_id(rx_port);
    let socket = u32::try_from(port_socket).unwrap_or_else(|_| port::rte_socket_id());
    let pool = Mempool::create(&name, count, MBUF_CACHE_SIZE, MBUF_DATA_ROOM, socket)
        .map_err(|e| failed(&e))?;
    Ok((Box::leak(Box::new(pool)), name, socket))
}

fn burst_mode(mode: Option<String>) -> String {
    mode.unwrap_or_else(|| "unknown".into())
}

struct Queues {
    rx: Vec<Option<RxQueue<'static>>>,
    tx: Vec<Option<TxQueue<'static>>>,
}

// ── Main lcore: NIC hardware counters ────────────────────────────────────────

/// One `NicPoller` per port, with the port-buffer discard xstats resolved once
/// (telemetry builds only: firmware xstats queries are suspected of amplifying
/// NIC stalls).
fn nic_pollers(ids: &[u16], start_ns: f64) -> Vec<(u16, NicPoller, Option<PortDiscardXstats>)> {
    ids.iter()
        .map(|&id| {
            let discards = if cfg!(feature = "telemetry") {
                match PortDiscardXstats::resolve(id) {
                    Ok(d) => Some(d),
                    Err(e) => {
                        eprintln!(
                            "dpds: port {id}: cannot resolve the port-buffer discard xstats, \
                             /nic/epochs rx_port_discards stays 0: {e}"
                        );
                        None
                    }
                }
            } else {
                None
            };
            let initial = port::eth_stats_get(id).unwrap_or_default();
            let initial_discards = discards.as_ref().and_then(|d| d.read()).unwrap_or(0);
            let poller = NicPoller::new(id as usize, start_ns, &initial, initial_discards);
            (id, poller, discards)
        })
        .collect()
}

/// Polls every port's hardware counters every `HW_STATS_POLL_INTERVAL_MS` until
/// all `workers` are done. Writes only into `hw`.
fn poll_nics(
    pollers: &mut [(u16, NicPoller, Option<PortDiscardXstats>)],
    hw: &NicHwStats,
    workers_done: &AtomicUsize,
    workers: usize,
) {
    while workers_done.load(Ordering::Acquire) < workers {
        std::thread::sleep(Duration::from_millis(HW_STATS_POLL_INTERVAL_MS));
        for (id, poller, discards) in pollers.iter_mut() {
            // A transient failure just skips this port's tick.
            if let Ok(curr) = port::eth_stats_get(*id) {
                poller.poll(now_ns(), &curr, hw, || {
                    discards.as_ref().and_then(|d| d.read())
                });
            }
        }
    }
}

// ── Main flow ────────────────────────────────────────────────────────────────

/// The links of the run and where each link's config comes from.
struct Layout {
    links: Vec<LinkSpec>,
    /// Per link: the config file, or the links file.
    sources: Vec<String>,
}

/// The `--config*` mode: one port is a link from the port to itself,
/// two ports are the links port 0 -> port 1 and port 1 -> port 0.
fn shorthand_links(cli: &dpds::cli::Cli, probed: &[(String, u16)]) -> Result<Layout> {
    validate_port_count(probed.len())?;
    let paths = cli.direction_configs(probed.len())?;
    let mut links = Vec::new();
    for (d, path) in paths.iter().enumerate() {
        links.push(LinkSpec {
            ingress: probed[d].0.clone(),
            egress: probed[(d + 1) % probed.len()].0.clone(),
            config: Config::from_file(path).map_err(Box::<dyn Error>::from)?,
            lcore: None,
            histogram: Default::default(),
            iat_histogram: Default::default(),
            delay_stats: true,
        });
    }
    Ok(Layout {
        links,
        sources: paths.iter().map(|p| p.display().to_string()).collect(),
    })
}

fn run() -> Result<()> {
    // Before EAL; non-fatal, a container or non-root run just stays unlocked.
    if let Err(e) = lock_memory() {
        eprintln!("WARNING: mlockall failed, continuing unlocked: {e}");
    }

    let args: Vec<String> = std::env::args().collect();
    let cli = parse_args(&args)?;

    // The links file needs no EAL: a bad file fails before anything starts.
    let links_file = match &cli.links {
        Some(path) => Some((path, load_links(path)?)),
        None => None,
    };

    // Dropped last: cleans up the EAL after everything else is gone.
    let _eal = Eal::new(&cli.eal_args).map_err(|e| format!("EAL initialization failed: {e}"))?;

    // Everything that can fail without side effects comes before the ports start.
    let port_ids: Vec<u16> = foreach_dev().collect();
    if port_ids.iter().copied().ne(0..port_ids.len() as u16) {
        return Err(format!("expected port ids 0..{}, got {port_ids:?}", port_ids.len()).into());
    }
    let probed: Vec<(String, u16)> = port_ids
        .iter()
        .map(|&id| Ok((port::device_name(id)?, id)))
        .collect::<std::result::Result<_, dpdk::DpdkError>>()?;
    let layout = match links_file {
        Some((path, links)) => {
            let sources = vec![path.display().to_string(); links.len()];
            Layout { links, sources }
        }
        None => shorthand_links(&cli, &probed)?,
    };
    validate_lcore_count(lcore_count(), layout.links.len())?;
    let workers: Vec<u32> = foreach_worker().collect();
    let dirs = resolve_directions(&layout.links, &probed, main_lcore(), &workers)?;

    // Buffers and histogram ranges: a bad config or histogram names its link.
    let mut buffers = Vec::new();
    let mut ranges = Vec::new();
    for (d, dir) in dirs.iter().enumerate() {
        let label = if cli.links.is_some() {
            layout.links[dir.link].label()
        } else {
            layout.sources[d].clone()
        };
        buffers.push(Buffer::try_new(dir.config).map_err(|e| format!("{label}: {e}"))?);
        ranges.push((
            HistRange::from_spec(&dir.config, &dir.histogram)
                .map_err(|e| format!("{label}: {e}"))?,
            HistRange::iat_from_spec(&dir.config, &dir.iat_histogram)
                .map_err(|e| format!("{label}: IAT histogram: {e}"))?,
            dir.delay_stats,
        ));
    }
    let name_of = |id: u16| probed[id as usize].0.clone();

    // Applied ring sizes come from `rte_eth_dev_adjust_nb_rx_tx_desc`, which works
    // on an unconfigured port, so the pools exist before the ports are set up.
    let applied: Vec<(u16, u16)> = port_ids
        .iter()
        .map(|&id| {
            adjust_nb_rx_tx_desc(id, NIC_DESCRIPTORS, NIC_DESCRIPTORS)
                .map_err(|e| format!("cannot query the descriptor limits of port {id}: {e}"))
        })
        .collect::<std::result::Result<_, _>>()?;

    // One pool per link; every port gets the pool of the link that receives on
    // it. An egress-only port is never polled but its RX ring is still filled:
    // it takes the pool of the link that transmits on it, which grows by the ring.
    let mut pool_of_port = vec![None; port_ids.len()];
    for (d, dir) in dirs.iter().enumerate() {
        pool_of_port[dir.rx_port as usize] = Some(d);
    }
    let mut pools = Vec::new();
    let mut pool_sizes = Vec::new();
    for (d, (dir, buffer)) in dirs.iter().zip(&buffers).enumerate() {
        let mut size = required_mbufs(
            buffer.capacity(),
            applied[dir.rx_port as usize].0,
            applied[dir.tx_port as usize].1,
        );
        if pool_of_port[dir.tx_port as usize].is_none() {
            pool_of_port[dir.tx_port as usize] = Some(d);
            size += applied[dir.tx_port as usize].0 as u64;
        }
        pools.push(create_pool(d, dir.rx_port, size)?);
        pool_sizes.push(size as u32);
    }
    // An early return drops (and so stops) the ports started so far.
    let mut ports = Vec::new();
    for &id in &port_ids {
        let port = Port::init(
            id,
            pools[pool_of_port[id as usize].ok_or("a probed port belongs to no link")?].0,
            NIC_DESCRIPTORS,
            NIC_DESCRIPTORS,
            1,
            MTU,
        )
        .map_err(|e| format!("failed to initialize port {id}: {e}"))?;
        if !port.link_up()? {
            return Err(format!("link is down on port {id}").into());
        }
        // Count from this process's start; a PMD without a reset is not fatal, its
        // counters then count from the NIC's start.
        if let Err(e) = port::eth_stats_reset(id) {
            eprintln!("dpds: warning: could not reset the NIC counters of port {id}: {e}");
        }
        let socket = port::rte_eth_dev_socket_id(id);
        println!(
            "Port {id} ({}): socket {socket}, RX ring {} (requested {}), TX ring {} (requested {}), \
             RX burst: {}, TX burst: {}, link {}",
            port.device_name(),
            port.rx_ring_size(),
            port.rx_ring_size_requested(),
            port.tx_ring_size(),
            port.tx_ring_size_requested(),
            burst_mode(port.rx_burst_mode()),
            burst_mode(port.tx_burst_mode()),
            port.link_speed().unwrap_or_else(|e| format!("unknown ({e})")),
        );
        ports.push(port);
    }

    let mut directions = Vec::new();
    for (d, (dir, buffer)) in dirs.iter().zip(buffers).enumerate() {
        let (rx_port, tx_port, lcore) = (dir.rx_port, dir.tx_port, dir.lcore);
        let (pool, pool_name, pool_socket) = pools[d].clone();
        let lcore_socket = unsafe { dpdk_sys::rte_lcore_to_socket_id(lcore) };
        if port::rte_eth_dev_socket_id(rx_port) >= 0 && lcore_socket != pool_socket {
            eprintln!(
                "dpds: warning: worker lcore {lcore} of direction {d} is on socket {lcore_socket} \
                 but port {rx_port} is on socket {pool_socket}"
            );
        }
        let raw_free =
            ports[rx_port as usize].mbuf_fast_free() && ports[tx_port as usize].mbuf_fast_free();
        let link = layout.links[dir.link].label();
        println!(
            "Direction {d}: lcore {lcore}, {link}, port {rx_port} -> port {tx_port}, \
             config {}, pool {pool_name} with {} mbufs on socket {pool_socket}, free: {}",
            layout.sources[d],
            pool_sizes[d],
            if raw_free {
                "raw bulk (MBUF_FAST_FREE on both ports)"
            } else {
                "pktmbuf bulk (MBUF_FAST_FREE not active on both ports)"
            }
        );
        directions.push(Direction {
            worker: Worker::new(buffer, rx_port, tx_port),
            config: layout.sources[d].clone(),
            rx_name: name_of(rx_port),
            tx_name: name_of(tx_port),
            lcore,
            delay_stats: dir.delay_stats,
            freer: pool.freer(),
            raw_free,
        });
    }

    let port_infos: Vec<PortInfo> = ports
        .iter()
        .map(|p| PortInfo {
            name: p.device_name().to_string(),
            rx_ring: p.rx_ring_size(),
            tx_ring: p.tx_ring_size(),
            rx_ring_requested: p.rx_ring_size_requested(),
            tx_ring_requested: p.tx_ring_size_requested(),
        })
        .collect();

    // Leaked: `remote_launch` needs `'static` queue handles, which `Port::queues`
    // only hands out for a `'static` port. `shutdown_port` stops the ports at the end.
    let mut queues = Queues {
        rx: Vec::new(),
        tx: Vec::new(),
    };
    for port in ports {
        let port: &'static mut Port<'static> = Box::leak(Box::new(port));
        let (mut r, mut t) = port.queues();
        queues.rx.push(r.drain(..1).next());
        queues.tx.push(t.drain(..1).next());
    }

    let result = serve(
        &port_ids,
        directions,
        queues,
        port_infos,
        &pool_sizes,
        ranges,
        layout.links.len(),
    );

    // The ports were leaked (no `Drop`): stop them by id.
    for id in port_ids {
        shutdown_port(id);
    }
    result
}

/// Registers telemetry, launches one worker per direction and polls the NICs
/// on the main lcore until all workers have finished.
fn serve(
    port_ids: &[u16],
    directions: Vec<Direction>,
    mut queues: Queues,
    port_infos: Vec<PortInfo>,
    pool_sizes: &[u32],
    ranges: Vec<(HistRange, HistRange, bool)>,
    num_links: usize,
) -> Result<()> {
    let clock = Clock::new();
    // Zero point of every `uptime_ns` / `relative_ns` / epoch time.
    let start_ns = clock.now_ns();
    let stats: &'static DirectionStats = Box::leak(Box::new(DirectionStats::with_hists(ranges)));
    let hw: &'static NicHwStats = Box::leak(Box::new(NicHwStats::new(port_ids.len())));
    let register = |e: dpdk::DpdkError| format!("cannot register telemetry command: {e}");
    DirectionStats::register_telemetry(stats, start_ns).map_err(register)?;
    NicHwStats::register_telemetry(hw).map_err(register)?;
    let direction_infos = directions
        .iter()
        .zip(pool_sizes)
        .map(|(d, &pool_size)| DirectionInfo {
            rx_port: d.worker.rx_port(),
            tx_port: d.worker.tx_port(),
            lcore: d.lcore,
            config: d.config.clone(),
            pool_size,
            raw_free: d.raw_free,
            rx_name: d.rx_name.clone(),
            tx_name: d.tx_name.clone(),
            delay_stats: d.delay_stats,
        })
        .collect();
    register_port_telemetry(port_infos, direction_infos, num_links).map_err(register)?;
    let mut pollers = nic_pollers(port_ids, start_ns);

    install_signal_handlers()?;

    static WORKERS_DONE: AtomicUsize = AtomicUsize::new(0);
    let num_directions = directions.len();

    // Take every direction's queue handles first, so a failure cannot leave
    // launched workers running. Each queue belongs to exactly one direction.
    let mut handles = Vec::new();
    for dir in &directions {
        let rx_queue = queues.rx[dir.worker.rx_port() as usize]
            .take()
            .ok_or("RX queue already in use")?;
        let tx_queue = queues.tx[dir.worker.tx_port() as usize]
            .take()
            .ok_or("TX queue already in use")?;
        handles.push((rx_queue, tx_queue));
    }

    let mut launched = Vec::new();
    for (d, (dir, (rx_queue, tx_queue))) in directions.into_iter().zip(handles).enumerate() {
        let Direction {
            worker,
            lcore,
            freer,
            raw_free,
            ..
        } = dir;
        let rx_port = worker.rx_port();
        let tx_port = worker.tx_port();
        let launch = eal::remote_launch(
            move || {
                let mut rx_nic = NicRx::new(rx_port, rx_queue);
                let mut tx_nic = NicTx::new(tx_port, tx_queue);
                let mut telemetry = Telemetry::new(&stats.stats[d], start_ns);
                let now = move || clock.now_ns();
                // SAFETY: `rx_nic` hands out valid mbufs we own, `tx_nic` and the free
                // implementation take them over. `RawFree` is only chosen when both
                // ports of the direction use MBUF_FAST_FREE. The TX port's flag covers
                // the mbufs it did not accept (direct, refcount 1, from the
                // direction's pool). The rejected RX mbufs are single-segment because
                // RX scatter is off and the data room fits the largest frame; they are
                // direct with refcount 1 as received from the direction's pool.
                unsafe {
                    if raw_free {
                        let mut free = RawFree(freer);
                        forward::run(
                            worker,
                            &mut rx_nic,
                            &mut tx_nic,
                            &mut free,
                            &mut telemetry,
                            &QUIT,
                            now,
                        );
                    } else {
                        forward::run(
                            worker,
                            &mut rx_nic,
                            &mut tx_nic,
                            &mut PktmbufFree,
                            &mut telemetry,
                            &QUIT,
                            now,
                        );
                    }
                }
                WORKERS_DONE.fetch_add(1, Ordering::Release);
            },
            lcore,
        );
        if let Err(e) = launch {
            // Stop the workers already running.
            QUIT.request();
            for &l in &launched {
                let _ = eal::wait_lcore(l);
            }
            return Err(format!("failed to launch lcore {lcore}: {e}").into());
        }
        launched.push(lcore);
    }
    println!(
        "DPDS running on lcore(s) {launched:?} with {} port(s). Ctrl+C to stop.",
        port_ids.len()
    );

    poll_nics(&mut pollers, hw, &WORKERS_DONE, num_directions);
    println!("Shutting down...");
    for lcore in launched {
        eal::wait_lcore(lcore)
            .map_err(|e| format!("lcore {lcore} did not shut down cleanly: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{lock_memory, memlock_plan, MemlockPlan, RLIM_INFINITY};

    /// Smoke test: `mlockall` legitimately fails without `CAP_IPC_LOCK` and with a low
    /// `RLIMIT_MEMLOCK`, so this asserts neither `Ok` nor `Err`, only that the call does not
    /// crash the process.
    #[test]
    fn lock_memory_does_not_panic() {
        let result: Result<(), std::io::Error> = lock_memory();
        let _ = result;
    }

    const MIB8: u64 = 8 << 20;

    #[test]
    fn root_always_locks() {
        for (soft, hard) in [
            (0, 0),
            (MIB8, MIB8),
            (MIB8, RLIM_INFINITY),
            (RLIM_INFINITY, RLIM_INFINITY),
        ] {
            assert_eq!(memlock_plan(0, soft, hard), MemlockPlan::Lock);
        }
    }

    #[test]
    fn an_unlimited_soft_limit_locks() {
        assert_eq!(
            memlock_plan(1000, RLIM_INFINITY, RLIM_INFINITY),
            MemlockPlan::Lock
        );
    }

    #[test]
    fn a_finite_soft_limit_under_an_unlimited_hard_limit_is_raised() {
        assert_eq!(
            memlock_plan(1000, MIB8, RLIM_INFINITY),
            MemlockPlan::RaiseThenLock
        );
        assert_eq!(
            memlock_plan(1000, 0, RLIM_INFINITY),
            MemlockPlan::RaiseThenLock
        );
    }

    #[test]
    fn a_finite_hard_limit_skips_the_lock() {
        for (soft, hard) in [(MIB8, MIB8), (0, 0), (1024, MIB8)] {
            assert_eq!(memlock_plan(1000, soft, hard), MemlockPlan::Skip);
        }
    }
}
