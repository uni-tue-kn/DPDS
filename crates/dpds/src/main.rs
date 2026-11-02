//! DPDS: DPDK-based network-link emulator. See the README for usage.
//!
//! Single data-plane core, like preof's `baseline`: the EAL main lcore
//! handles signals and polls the NIC hardware counters; one worker lcore
//! runs RX -> emulator -> TX (`dpds::forward::run`).

use std::error::Error;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dpdk::eal::{self, foreach_worker, lcore_count};
use dpdk::port::{self, foreach_dev, shutdown_port};
use dpdk::{now_ns, Eal, Mempool, Port, PortDiscardXstats};
use dpds::cli::{parse_args, Cli};
use dpds::consts::{HW_STATS_POLL_INTERVAL_MS, MBUF_CACHE_SIZE, MTU, NIC_DESCRIPTORS};
use dpds::forward;
use dpds::io::{NicRx, NicTx, PktmbufFree};
use dpds::quit::QUIT;
use dpds::setup::{validate_lcore_count, validate_port_count};
use dpds::stats::{register_port_telemetry, NicHwStats, NicPoller, PortInfo, Stats, Telemetry};
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
// SIGINT/SIGTERM set the quit flag: the worker stops receiving, drains the
// delay buffer, flushes TX and returns; the main lcore then shuts down.

extern "C" fn on_signal(_signum: i32) {
    QUIT.request();
}

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
const SIG_ERR: isize = -1;

extern "C" {
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> isize;
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

/// The queue handles of all ports, split per direction and indexed by port id.
struct Nics {
    rx: NicRx,
    tx: NicTx,
    infos: Vec<PortInfo>,
}

/// Initialises and starts every port (one RX and one TX queue each) and
/// leaks it: `remote_launch` needs `'static` queue handles, and `Port::queues`
/// only hands out `'static` ones for a `'static` port. The ports are stopped
/// explicitly with `shutdown_port` at the end.
fn init_ports(ids: &[u16], pool: &'static Mempool) -> Result<Nics> {
    let mut ports = Vec::new();
    for &id in ids {
        // On an early return the already started ports are dropped, which stops them.
        let port = Port::init(id, pool, NIC_DESCRIPTORS, NIC_DESCRIPTORS, 1, MTU)
            .map_err(|e| format!("failed to initialize port {id}: {e}"))?;
        if !port.link_up()? {
            return Err(format!("link is down on port {id}").into());
        }
        println!(
            "Port {id} ({}): RX ring {} (requested {}), TX ring {} (requested {})",
            port.device_name(),
            port.rx_ring_size(),
            port.rx_ring_size_requested(),
            port.tx_ring_size(),
            port.tx_ring_size_requested()
        );
        ports.push(port);
    }

    let mut infos = Vec::new();
    let mut rx = Vec::new();
    let mut tx = Vec::new();
    for port in ports {
        infos.push(PortInfo {
            name: port.device_name().to_string(),
            rx_ring: port.rx_ring_size(),
            tx_ring: port.tx_ring_size(),
            rx_ring_requested: port.rx_ring_size_requested(),
            tx_ring_requested: port.tx_ring_size_requested(),
        });
        let port: &'static mut Port<'static> = Box::leak(Box::new(port));
        let (mut r, mut t) = port.queues();
        rx.extend(r.drain(..1));
        tx.extend(t.drain(..1));
    }
    Ok(Nics {
        rx: NicRx(rx),
        tx: NicTx(tx),
        infos,
    })
}

// ── Main lcore: NIC hardware counters ────────────────────────────────────────

/// One `NicPoller` per port, with the port-buffer discard xstats resolved
/// once (telemetry builds only: firmware xstats queries are suspected of
/// amplifying NIC stalls, so a plain build never makes them).
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

/// Polls every port's hardware counters every `HW_STATS_POLL_INTERVAL_MS`
/// until the worker is done. Writes only into `hw`, never into worker state.
fn poll_nics(
    pollers: &mut [(u16, NicPoller, Option<PortDiscardXstats>)],
    hw: &NicHwStats,
    worker_done: &AtomicBool,
) {
    while !worker_done.load(Ordering::Acquire) {
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

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let Cli {
        eal_args,
        config,
        num_mbufs,
    } = parse_args(&args)?;

    // Dropped last: cleans up the EAL after everything else is gone.
    let _eal = Eal::new(&eal_args).map_err(|e| format!("EAL initialization failed: {e}"))?;

    // Everything that can fail without side effects comes before the ports start.
    validate_lcore_count(lcore_count())?;
    let worker_lcore = foreach_worker().next().ok_or("no worker lcore available")?;
    let port_ids: Vec<u16> = foreach_dev().collect();
    validate_port_count(port_ids.len())?;
    if port_ids.iter().copied().ne(0..port_ids.len() as u16) {
        return Err(format!("expected port ids 0..{}, got {port_ids:?}", port_ids.len()).into());
    }
    let num_ports = port_ids.len() as u16;
    let config = Config::from_file(&config)?;
    let worker = Worker::new(Buffer::new(config), num_ports);

    let pool_size = num_mbufs
        .checked_mul(num_ports as u32)
        .ok_or("--num-mbufs too large")?;
    // Leaked: the ports' queue handles must be `'static` (see `init_ports`).
    let pool: &'static Mempool = Box::leak(Box::new(
        Mempool::create(
            "mbuf_pool",
            pool_size,
            MBUF_CACHE_SIZE,
            Mempool::DEFAULT_BUF_SIZE,
            port::rte_socket_id(),
        )
        .map_err(|e| format!("failed to initialize mbuf pool: {e}"))?,
    ));
    let nics = init_ports(&port_ids, pool)?;

    let result = serve(&port_ids, worker, worker_lcore, nics);

    // The ports were leaked (no `Drop`), stop and close them by id.
    for id in port_ids {
        shutdown_port(id);
    }
    result
}

/// Registers telemetry, launches the worker and runs the main lcore's poll
/// loop until the worker has finished.
fn serve(port_ids: &[u16], worker: Worker, worker_lcore: u32, nics: Nics) -> Result<()> {
    // The shared zero point of every `uptime_ns` / `relative_ns` / epoch time.
    let start_ns = now_ns();
    let stats: &'static Stats = Box::leak(Box::default());
    let hw: &'static NicHwStats = Box::leak(Box::new(NicHwStats::new(port_ids.len())));
    let register = |e: dpdk::DpdkError| format!("cannot register telemetry command: {e}");
    Stats::register_telemetry(stats, start_ns).map_err(register)?;
    NicHwStats::register_telemetry(hw).map_err(register)?;
    register_port_telemetry(nics.infos.clone()).map_err(register)?;
    let mut pollers = nic_pollers(port_ids, start_ns);

    install_signal_handlers()?;

    static WORKER_DONE: AtomicBool = AtomicBool::new(false);
    let Nics {
        rx: mut rx_nic,
        tx: mut tx_nic,
        ..
    } = nics;
    eal::remote_launch(
        move || {
            let mut telemetry = Telemetry::new(stats, start_ns);
            // SAFETY: `rx_nic` hands out valid mbufs we own, `tx_nic` and
            // `PktmbufFree` take them over.
            unsafe {
                forward::run(
                    worker,
                    &mut rx_nic,
                    &mut tx_nic,
                    &mut PktmbufFree,
                    &mut telemetry,
                    &QUIT,
                    now_ns,
                )
            };
            WORKER_DONE.store(true, Ordering::Release);
        },
        worker_lcore,
    )
    .map_err(|e| format!("failed to launch lcore {worker_lcore}: {e}"))?;
    println!(
        "DPDS running on lcore {worker_lcore} with {} port(s). Ctrl+C to stop.",
        port_ids.len()
    );

    poll_nics(&mut pollers, hw, &WORKER_DONE);
    println!("Shutting down...");
    eal::wait_lcore(worker_lcore)
        .map_err(|e| format!("lcore {worker_lcore} did not shut down cleanly: {e}"))?;
    Ok(())
}
