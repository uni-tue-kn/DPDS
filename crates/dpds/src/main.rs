//! DPDS: DPDK-based network-link emulator. See the README for usage.

use std::error::Error;
use std::fs;
use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::Ordering::{Acquire, Release};
use std::sync::Arc;
use std::time::Duration;

use dpdk::eal::{self, foreach_worker, lcore_count};
use dpdk::port::{self, foreach_dev, shutdown_port};
use dpdk::{Eal, Mempool, Port, Ring};
use dpds::cli::{parse_args, Cli};
use dpds::consts::{BURST_HIST_BUCKETS, MBUF_CACHE_SIZE, MTU, NIC_DESCRIPTORS, RING_SIZE};
use dpds::io::{NicRx, NicTx, PktmbufFree};
use dpds::layout::{validate_port_count, Layout};
use dpds::quit::QUIT;
use dpds::stats::{
    format_burst_hist, format_csv, format_dashboard, NicStats, PipelineStats, StatsSnapshot,
};
use dpds::worker::Worker;
use dpds::{combined, rx, tx, worker};
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
// SIGINT/SIGTERM start the ordered shutdown: RX stops first and every stage
// passes the request on once it has drained (see `dpds::quit`).

extern "C" fn on_signal(_signum: i32) {
    QUIT.receive.store(true, Release);
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
    ids: Vec<u16>,
    rx: NicRx,
    tx: NicTx,
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
        ports.push(port);
    }

    let mut rx = Vec::new();
    let mut tx = Vec::new();
    for port in ports {
        let port: &'static mut Port<'static> = Box::leak(Box::new(port));
        let (mut r, mut t) = port.queues();
        rx.extend(r.drain(..1));
        tx.extend(t.drain(..1));
    }
    Ok(Nics {
        ids: ids.to_vec(),
        rx: NicRx(rx),
        tx: NicTx(tx),
    })
}

fn nic_stats(ids: &[u16]) -> Vec<(u16, NicStats)> {
    ids.iter()
        .map(|&id| {
            let s = port::eth_stats_get(id).unwrap_or_default();
            let nic = NicStats {
                imissed: s.imissed,
                ierrors: s.ierrors,
                oerrors: s.oerrors,
                rx_nombuf: s.rx_nombuf,
            };
            (id, nic)
        })
        .collect()
}

/// Launches the stages of `layout` on the worker lcores, recording each
/// lcore in `launched` as soon as it runs. Same lcore assignment as the C app.
fn launch(
    layout: Layout,
    lcores: &[u32],
    worker: Worker,
    nics: Nics,
    rings: Vec<Arc<Ring>>,
    stats: &'static PipelineStats,
    launched: &mut Vec<u32>,
) -> std::result::Result<(), dpdk::DpdkError> {
    let num_ports = nics.ids.len() as u16;
    let mut run_on = |lcore: u32, f: Box<dyn FnOnce() + Send>| -> std::result::Result<(), _> {
        eal::remote_launch(f, lcore)?;
        launched.push(lcore);
        Ok(())
    };
    match layout {
        Layout::Combined => {
            println!(
                "Starting in 2-core mode: RX+Worker+TX combined on core {}",
                lcores[0]
            );
            let (mut rx_nic, mut tx_nic) = (nics.rx, nics.tx);
            run_on(
                lcores[0],
                Box::new(move || {
                    combined::run_rx_worker_tx(
                        worker,
                        num_ports,
                        &mut rx_nic,
                        &mut tx_nic,
                        &mut PktmbufFree,
                        stats,
                        &QUIT,
                    )
                }),
            )
        }
        Layout::RxAndWorkerTx => {
            println!(
                "Starting in 3-core mode: Worker+TX on core {}, RX on core {}",
                lcores[0], lcores[1]
            );
            let (mut rx_nic, mut tx_nic) = (nics.rx, nics.tx);
            let (mut rx_ring, wt_ring) = (rings[0].clone(), rings[0].clone());
            run_on(
                lcores[0],
                Box::new(move || {
                    combined::run_worker_tx(
                        worker,
                        num_ports,
                        &wt_ring,
                        &mut tx_nic,
                        &mut PktmbufFree,
                        stats,
                        &QUIT,
                    )
                }),
            )?;
            run_on(
                lcores[1],
                Box::new(move || {
                    rx::run(
                        num_ports,
                        &mut rx_nic,
                        &mut rx_ring,
                        &mut PktmbufFree,
                        &stats.rx,
                        &QUIT,
                    )
                }),
            )
        }
        Layout::Separate => {
            println!(
                "Starting in 4-core mode: TX on core {}, Worker on core {}, RX on core {}",
                lcores[0], lcores[1], lcores[2]
            );
            let (mut rx_nic, mut tx_nic) = (nics.rx, nics.tx);
            let (mut rx_ring, wk_in) = (rings[0].clone(), rings[0].clone());
            let (mut wk_out, tx_ring) = (rings[1].clone(), rings[1].clone());
            run_on(
                lcores[0],
                Box::new(move || {
                    tx::run(
                        num_ports,
                        &tx_ring,
                        &mut tx_nic,
                        &mut PktmbufFree,
                        &stats.tx,
                        &QUIT,
                    )
                }),
            )?;
            run_on(
                lcores[1],
                Box::new(move || {
                    worker::run(
                        worker,
                        &wk_in,
                        &mut wk_out,
                        &mut PktmbufFree,
                        &stats.worker,
                        &QUIT,
                    )
                }),
            )?;
            run_on(
                lcores[2],
                Box::new(move || {
                    rx::run(
                        num_ports,
                        &mut rx_nic,
                        &mut rx_ring,
                        &mut PktmbufFree,
                        &stats.rx,
                        &QUIT,
                    )
                }),
            )
        }
    }
}

// ── Output ───────────────────────────────────────────────────────────────────

fn print_flushed(text: &str) {
    let mut out = std::io::stdout().lock();
    // A closed stdout must not stop the emulator.
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// Prints the dashboard once per second until the pipeline has shut down.
/// Returns the snapshot of the last print.
fn monitor(stats: &PipelineStats, nic_ids: &[u16]) -> StatsSnapshot {
    let mut prev = StatsSnapshot::default();
    let mut printed_once = false;
    while !QUIT.transmit.load(Acquire) {
        std::thread::sleep(Duration::from_secs(1));
        let cur = stats.snapshot();
        print_flushed(&format_dashboard(
            &cur,
            &prev,
            &nic_stats(nic_ids),
            printed_once,
        ));
        printed_once = true;
        prev = cur;
    }
    prev
}

fn print_final(stats: &PipelineStats, prev: &StatsSnapshot, nic_ids: &[u16]) -> StatsSnapshot {
    let cur = stats.snapshot();
    let zero = [0u64; BURST_HIST_BUCKETS];
    print_flushed("\n=== Final Stats ===\n");
    print_flushed(&format_dashboard(&cur, prev, &nic_stats(nic_ids), false));
    print_flushed("=== Cumulative Burst Histograms ===\n");
    print_flushed(&format_burst_hist(
        "RX",
        &cur.rx.burst_hist,
        &zero,
        cur.rx.received,
    ));
    print_flushed(&format_burst_hist(
        "TX",
        &cur.tx.burst_hist,
        &zero,
        cur.tx.transmitted,
    ));
    cur
}

// ── Main flow ────────────────────────────────────────────────────────────────

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let Cli {
        eal_args,
        config,
        stats_file,
        num_mbufs,
    } = parse_args(&args)?;

    // Dropped last: cleans up the EAL after everything else is gone.
    let _eal = Eal::new(&eal_args).map_err(|e| format!("EAL initialization failed: {e}"))?;

    // Everything that can fail without side effects comes before the first lcore launch.
    let layout = Layout::from_lcore_count(lcore_count())?;
    let port_ids: Vec<u16> = foreach_dev().collect();
    validate_port_count(port_ids.len())?;
    if port_ids.iter().copied().ne(0..port_ids.len() as u16) {
        return Err(format!("expected port ids 0..{}, got {port_ids:?}", port_ids.len()).into());
    }
    let num_ports = port_ids.len() as u16;
    let config = Config::from_file(&config)?;
    let worker = Worker::new(Buffer::new(config), num_ports);

    let socket_id = port::rte_socket_id();
    let rings = (0..layout.rings())
        .map(|i| {
            Ring::create_spsc(&format!("ring_{i}"), RING_SIZE, socket_id as i32)
                .map(Arc::new)
                .map_err(|e| format!("failed to create ring {i}: {e}"))
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;

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
            socket_id,
        )
        .map_err(|e| format!("failed to initialize mbuf pool: {e}"))?,
    ));
    let nics = init_ports(&port_ids, pool)?;

    let stats: &'static PipelineStats = Box::leak(Box::default());
    let lcores: Vec<u32> = foreach_worker().collect();

    let result = (|| {
        install_signal_handlers()?;
        let mut launched = Vec::new();
        if let Err(e) = launch(layout, &lcores, worker, nics, rings, stats, &mut launched) {
            // Stop whatever did start so the join below returns.
            QUIT.set_all();
            if let Err(join_err) = join(&launched) {
                eprintln!("dpds: {join_err}");
            }
            return Err(format!("failed to launch lcore: {e}").into());
        }

        let prev = monitor(stats, &port_ids);
        join(&launched)?;
        let cur = print_final(stats, &prev, &port_ids);

        if let Some(path) = &stats_file {
            let csv = format_csv(&cur, &nic_stats(&port_ids));
            fs::write(path, csv)
                .map_err(|e| format!("failed to write stats file {}: {e}", path.display()))?;
            println!("Stats saved to: {}", path.display());
        }
        Ok(())
    })();

    // The ports were leaked (no `Drop`), stop and close them by id.
    for id in port_ids {
        shutdown_port(id);
    }
    result
}

/// Waits for all launched lcores to return.
fn join(lcores: &[u32]) -> Result<()> {
    let mut result = Ok(());
    for &lcore in lcores {
        if let Err(e) = eal::wait_lcore(lcore) {
            result = Err(format!("lcore {lcore} did not shut down cleanly: {e}").into());
        }
    }
    result
}
