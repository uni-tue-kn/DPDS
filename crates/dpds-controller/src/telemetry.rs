//! DPDK telemetry client (B4): a blocking SOCK_SEQPACKET connection, and a
//! poll thread that caches the latest snapshots for the REST endpoints.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use socket2::{Domain, SockAddr, Socket, Type};

use crate::dto::MAX_HISTOGRAM_BINS;
pub use crate::stats::Polled;
use crate::stats::Snapshot;
use crate::supervisor::{State, Supervisor};

/// DPDK `--file-prefix` of the child; fixes the socket path below the runtime dir.
pub const FILE_PREFIX: &str = "dpds-ctl";

/// The dedicated subdirectory of the runtime dir that the controller creates and
/// removes; it is the `RUNTIME_DIRECTORY` (DPDK's runtime dir) of the child.
pub fn instance_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("instance")
}

/// Socket of a `dpds` started with `RUNTIME_DIRECTORY=<runtime_dir>/instance`.
pub fn socket_path(runtime_dir: &Path) -> PathBuf {
    instance_dir(runtime_dir)
        .join("dpdk")
        .join(FILE_PREFIX)
        .join("dpdk_telemetry.v2")
}

/// A telemetry connection. The server answers every command with one datagram.
pub struct Client {
    socket: Socket,
}

impl Client {
    /// Connects and returns the client with the server's greeting (`{"version", "pid", ...}`).
    pub fn connect(path: &Path) -> io::Result<(Client, Value)> {
        let socket = Socket::new(Domain::UNIX, Type::SEQPACKET, None)?;
        socket.connect(&SockAddr::unix(path)?)?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        socket.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut client = Client { socket };
        let greeting = client.read_reply()?;
        Ok((client, greeting))
    }

    fn read_reply(&mut self) -> io::Result<Value> {
        // max_output_len is 16 KiB; leave room for a larger server
        let mut buf = vec![0u8; 64 * 1024];
        let n = self.socket.read(&mut buf)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "telemetry socket closed",
            ));
        }
        serde_json::from_slice(&buf[..n]).map_err(io::Error::other)
    }

    /// Sends `cmd` (e.g. `/dpds/stats,0`) and returns the value of the reply.
    /// Unknown commands and errors come back as `null` and are an `Err`.
    pub fn query(&mut self, cmd: &str) -> io::Result<Value> {
        self.query_opt(cmd)?
            .ok_or_else(|| io::Error::other(format!("no reply for {cmd}")))
    }

    /// Like [`Client::query`], but a `null` or missing reply is `Ok(None)`: the request was
    /// answered, so the stream is still in step. Only transport errors are an `Err`.
    pub fn query_opt(&mut self, cmd: &str) -> io::Result<Option<Value>> {
        self.socket.write_all(cmd.as_bytes())?;
        let mut reply = self.read_reply()?;
        let name = cmd.split(',').next().unwrap_or(cmd);
        Ok(match reply.get_mut(name).map(Value::take) {
            Some(Value::Null) | None => None,
            Some(v) => Some(v),
        })
    }
}

pub trait TelemetrySource: Send + Sync + 'static {
    fn latest(&self) -> Option<Polled>;

    /// Every retained poll of the current run, oldest first, as (age in ms, snapshot);
    /// the newest entry is the latest poll. Empty without data. A source without a
    /// ring has just the latest poll.
    fn history(&self) -> Vec<(u64, Snapshot)> {
        self.latest()
            .map(|p| vec![(p.age_ms, p.cur)])
            .unwrap_or_default()
    }

    /// The poll interval of the run being started (a no-op for sources that do not poll).
    fn set_poll_interval(&self, _ms: u64) {}
}

/// Most bins in one page of `/dpds/delay_hist`.
const PAGE_BINS: u64 = 500;

/// One page of `/dpds/delay_hist` of direction `d`, which must say it is `page` of a
/// histogram of `num_bins` bins and carry exactly the bins it should. Replies are matched to
/// requests by send order only, so a replayed, cut or mixed-up page is caught here. The error
/// says what is wrong with the page.
fn check_page(page: u64, num_bins: u64, reply: &Value) -> Result<Vec<u64>, String> {
    let first = page * PAGE_BINS;
    let want = PAGE_BINS.min(num_bins - first);
    let field = |key: &str| reply.get(key).and_then(Value::as_u64);
    if field("page") != Some(page) || field("first_bin") != Some(first) {
        return Err("the page says it is another page".into());
    }
    if field("num_bins") != Some(num_bins) {
        return Err("num_bins differs from page 0".into());
    }
    let bins = reply
        .get("bins")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(Value::as_u64).collect::<Option<Vec<u64>>>())
        .ok_or("no bins, or a bin that is not a count")?;
    if bins.len() as u64 != want {
        return Err(format!("{} bins, expected {want}", bins.len()));
    }
    Ok(bins)
}

/// A direction whose histogram is absent for this poll.
const NO_HISTOGRAM: (Value, Vec<u64>) = (Value::Null, Vec::new());

/// The delay histogram of direction `d`: page 0 without its bins, and all bins of all pages.
/// Page 0 gives the number of bins, so the pages 1.. follow in order, none past the last.
/// A page 0 that is null or empty (delay statistics off, or a `dpds` without the feature)
/// means no histogram, silently. Any other bad page (wrong, missing or short data, or
/// `num_bins` out of range) also means no histogram for this poll, with a warning: a
/// histogram is never put together from pieces that do not fit. Only a transport error is an
/// `Err`, because it can put the replies out of step with the requests.
fn poll_histogram(client: &mut Client, d: u64) -> io::Result<(Value, Vec<u64>)> {
    let broken = |page: u64, what: &str| {
        eprintln!(
            "dpds-controller: /dpds/delay_hist direction {d} page {page}: {what}; no histogram"
        );
        NO_HISTOGRAM
    };
    let mut page0 = match client.query_opt(&format!("/dpds/delay_hist,{d}"))? {
        None => return Ok(NO_HISTOGRAM),
        Some(v) if v.as_object().is_some_and(|o| o.is_empty()) => return Ok(NO_HISTOGRAM),
        Some(v) => v,
    };
    let num_bins = page0.get("num_bins").and_then(Value::as_u64).unwrap_or(0);
    if num_bins == 0 || num_bins > u64::from(MAX_HISTOGRAM_BINS) {
        return Ok(broken(
            0,
            &format!("num_bins {num_bins} is not in 1..=4096"),
        ));
    }
    let mut bins = match check_page(0, num_bins, &page0) {
        Ok(bins) => bins,
        Err(what) => return Ok(broken(0, &what)),
    };
    for page in 1..num_bins.div_ceil(PAGE_BINS) {
        let Some(reply) = client.query_opt(&format!("/dpds/delay_hist,{d},{page}"))? else {
            return Ok(broken(page, "null reply"));
        };
        match check_page(page, num_bins, &reply) {
            Ok(more) => bins.extend(more),
            Err(what) => return Ok(broken(page, &what)),
        }
    }
    if let Some(page0) = page0.as_object_mut() {
        page0.remove("bins");
    }
    Ok((page0, bins))
}

/// One full poll: ports, per-direction stats and histogram, per-port NIC stats.
pub fn poll_once(client: &mut Client) -> io::Result<Snapshot> {
    let ports = client.query("/dpds/ports")?;
    let count = |key: &str| ports.get(key).and_then(Value::as_u64).unwrap_or(0);
    let (n_dirs, n_ports) = (count("num_directions"), count("num_ports"));
    let mut snap = Snapshot {
        ports: Value::Null,
        ..Snapshot::default()
    };
    for d in 0..n_dirs {
        snap.dirs.push(client.query(&format!("/dpds/stats,{d}"))?);
        let (hist, bins) = poll_histogram(client, d)?;
        snap.hists.push(hist);
        snap.hist_bins.push(bins);
    }
    for p in 0..n_ports {
        snap.eth.push(client.query(&format!("/ethdev/stats,{p}"))?);
    }
    snap.ports = ports;
    Ok(snap)
}

/// Consecutive failed polls after which the cached data is dropped.
const MAX_FAILURES: u32 = 5;

/// The longest histogram window (K3) the ring has to cover.
const RING_SECONDS: u64 = 300;

/// Most snapshots the ring keeps, whatever the poll interval (bounds the memory at
/// very short intervals; the window is then shorter than 300 s).
const RING_MAX: usize = 6000;

/// Snapshots needed to cover [`RING_SECONDS`] at `interval_ms`, plus the one that is
/// just old enough.
fn ring_capacity(interval_ms: u64) -> usize {
    let n = (RING_SECONDS * 1000).div_ceil(interval_ms.max(1)) + 2;
    usize::try_from(n).map_or(RING_MAX, |n| n.min(RING_MAX))
}

struct Cache {
    polled: Polled,
    at: Instant,
    /// Polls that failed since `polled` was taken.
    failures: u32,
    /// Every poll of this run, oldest first; dropped with the cache.
    ring: VecDeque<(Instant, Snapshot)>,
}

/// Polls the running `dpds` in a background thread and caches the result.
/// Waits `interval_ms` between polls, read on every cycle.
pub struct Poller {
    cell: Mutex<Option<Cache>>,
    interval_ms: AtomicU64,
}

/// Poll interval until a start sets one.
const DEFAULT_POLL_INTERVAL_MS: u64 = 500;

impl Poller {
    pub fn spawn(runtime_dir: PathBuf, supervisor: Arc<dyn Supervisor>) -> Arc<Poller> {
        let poller = Arc::new(Poller {
            cell: Mutex::new(None),
            interval_ms: AtomicU64::new(DEFAULT_POLL_INTERVAL_MS),
        });
        let p = poller.clone();
        thread::spawn(move || p.run(&runtime_dir, supervisor.as_ref()));
        poller
    }

    fn cell(&self) -> std::sync::MutexGuard<'_, Option<Cache>> {
        self.cell.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn clear(&self) {
        *self.cell() = None;
    }

    fn succeeded(&self, cur: Snapshot, capacity: usize) {
        let mut cell = self.cell();
        let now = Instant::now();
        let (prev, mut ring) = match cell.take() {
            Some(c) => (Some(c.polled.cur), c.ring),
            None => (None, VecDeque::new()),
        };
        ring.push_back((now, cur.clone()));
        while ring.len() > capacity {
            ring.pop_front();
        }
        *cell = Some(Cache {
            polled: Polled {
                prev,
                cur,
                age_ms: 0,
                stale: false,
            },
            at: now,
            failures: 0,
            ring,
        });
    }

    /// Keeps the last data (flagged stale) until `MAX_FAILURES` polls in a row failed.
    fn failed(&self) {
        let mut cell = self.cell();
        if let Some(c) = cell.as_mut() {
            c.failures += 1;
            if c.failures >= MAX_FAILURES {
                *cell = None;
            }
        }
    }

    fn run(&self, runtime_dir: &Path, supervisor: &dyn Supervisor) {
        let mut conn: Option<Client> = None;
        let mut conn_pid: Option<u32> = None;
        loop {
            let interval = self.interval_ms.load(Ordering::Relaxed);
            thread::sleep(Duration::from_millis(interval));
            let status = supervisor.status();
            let pid = match (status.state, status.pid) {
                (State::Running, Some(pid)) => pid,
                _ => {
                    conn = None;
                    self.clear();
                    continue;
                }
            };
            if conn_pid != Some(pid) {
                conn = None;
                // a new run starts a new ring
                self.clear();
            }
            if conn.is_none() {
                match Client::connect(&socket_path(runtime_dir)) {
                    Ok((c, greeting)) if greeting["pid"].as_u64() == Some(u64::from(pid)) => {
                        conn = Some(c);
                        conn_pid = Some(pid);
                    }
                    _ => {
                        self.failed();
                        continue;
                    }
                }
            }
            let Some(client) = conn.as_mut() else {
                continue;
            };
            match poll_once(client) {
                Ok(cur) => self.succeeded(cur, ring_capacity(interval)),
                Err(_) => {
                    conn = None;
                    self.failed();
                }
            }
        }
    }
}

impl TelemetrySource for Poller {
    fn set_poll_interval(&self, ms: u64) {
        self.interval_ms.store(ms.max(1), Ordering::Relaxed);
    }

    fn history(&self) -> Vec<(u64, Snapshot)> {
        let cell = self.cell();
        let Some(c) = cell.as_ref() else {
            return Vec::new();
        };
        c.ring
            .iter()
            .map(|(at, s)| {
                (
                    u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX),
                    s.clone(),
                )
            })
            .collect()
    }

    fn latest(&self) -> Option<Polled> {
        self.cell().as_ref().map(|c| Polled {
            age_ms: u64::try_from(c.at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stale: c.failures > 0,
            ..c.polled.clone()
        })
    }
}
