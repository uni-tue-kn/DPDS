//! DPDK telemetry client (B4): a blocking SOCK_SEQPACKET connection, and a
//! poll thread that caches the latest snapshots for the REST endpoints.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use socket2::{Domain, SockAddr, Socket, Type};

pub use crate::stats::Polled;
use crate::stats::Snapshot;
use crate::store;
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
        self.socket.write_all(cmd.as_bytes())?;
        let mut reply = self.read_reply()?;
        let name = cmd.split(',').next().unwrap_or(cmd);
        match reply.get_mut(name).map(Value::take) {
            Some(Value::Null) | None => Err(io::Error::other(format!("no reply for {cmd}"))),
            Some(v) => Ok(v),
        }
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
        // the histogram only exists in newer builds; its absence is not an error
        snap.hists.push(
            client
                .query(&format!("/dpds/delay_hist,{d}"))
                .unwrap_or(Value::Null),
        );
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
/// Reads the poll interval from the stored settings on every cycle.
pub struct Poller {
    cell: Mutex<Option<Cache>>,
}

impl Poller {
    pub fn spawn(
        state_dir: PathBuf,
        runtime_dir: PathBuf,
        supervisor: Arc<dyn Supervisor>,
    ) -> Arc<Poller> {
        let poller = Arc::new(Poller {
            cell: Mutex::new(None),
        });
        let p = poller.clone();
        thread::spawn(move || p.run(&state_dir, &runtime_dir, supervisor.as_ref()));
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

    fn run(&self, state_dir: &Path, runtime_dir: &Path, supervisor: &dyn Supervisor) {
        let mut conn: Option<Client> = None;
        let mut conn_pid: Option<u32> = None;
        loop {
            let interval = store::load(state_dir).map_or(500, |s| s.settings.poll_interval_ms);
            thread::sleep(Duration::from_millis(interval.max(1)));
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
