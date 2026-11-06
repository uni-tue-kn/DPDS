//! A fake DPDK telemetry socket (SOCK_SEQPACKET) for the supervisor readiness probe
//! and the real `Poller`. Like DPDK's: on connect one datagram
//! `{"version", "pid", "max_output_len"}`, then one reply datagram per request
//! `{"<command without ,param>": value}`; an unknown command gets `null`.
//!
//! `/dpds/delay_hist,<dir>[,<page>]` answers in pages of at most [`PAGE_BINS`] bins like the
//! real `dpds` ([`hist_page`]); `FakeTelemetryServer::serving` slices the histograms of its snapshot.
//!
//! Connections take their behaviour from a script of [`Mode`]s: the n-th accepted
//! connection uses `modes[n]`, the last mode repeats. [`FakeTelemetryServer::break_all`]
//! drops every open connection (a transport error for its client).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{json, Value};
use socket2::{Domain, SockAddr, Socket, Type};

use dpds_controller::stats::Snapshot;

use crate::common::full_hists;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Greeting, then answer every request from the handler.
    Serve,
    /// Greeting, then close the connection (the client sees a transport error).
    CloseAfterGreeting,
    /// Greeting, then read requests and never answer (the client's read times out).
    Hang,
    /// Greeting, then answer every request with `null` ("no such command").
    Nulls,
    /// Like `Serve`, but the connection is closed on a request for a histogram page past page 0
    /// (`/dpds/delay_hist,<dir>,<page>` with `page > 0`): a transport error in the middle of a poll.
    CloseOnLaterPage,
}

/// Bins per page of the real `/dpds/delay_hist` reply.
pub const PAGE_BINS: usize = 500;

/// The reply of `/dpds/delay_hist,<param>` (`param` is `<dir>` or `<dir>,<page>`) for the full
/// histograms `hists` (one object per direction, `bins` holding all bins): the page's slice of
/// `bins` plus `page` and `first_bin`, every other field copied. `None` (a null reply) for a bad
/// param or a page that does not exist; a histogram without bins has an empty page 0.
pub fn hist_page(hists: &[Value], param: &str) -> Option<Value> {
    let (dir, page) = match param.split_once(',') {
        Some((d, p)) => (d, p.parse::<usize>().ok()?),
        None => (param, 0),
    };
    let h = hists.get(dir.parse::<usize>().ok()?)?;
    let first = page.checked_mul(PAGE_BINS)?;
    let Some(bins) = h["bins"].as_array() else {
        return (page == 0).then(|| h.clone());
    };
    if first >= bins.len().max(1) {
        return None;
    }
    let mut out = h.clone();
    out["bins"] = Value::Array(bins[first..(first + PAGE_BINS).min(bins.len())].to_vec());
    out["page"] = json!(page);
    out["first_bin"] = json!(first);
    Some(out)
}

type Handler = Box<dyn Fn(&str) -> Option<Value> + Send + Sync>;

struct Shared {
    pid: u32,
    stop: AtomicBool,
    generation: AtomicUsize,
    connections: AtomicUsize,
    modes: Mutex<Vec<Mode>>,
    queries: Mutex<Vec<String>>,
    handler: Handler,
}

pub struct FakeTelemetryServer {
    shared: Arc<Shared>,
    path: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl FakeTelemetryServer {
    /// Listens on `path` (parent directories are created). The greeting names `pid`.
    pub fn start(
        path: &Path,
        pid: u32,
        modes: Vec<Mode>,
        handler: impl Fn(&str) -> Option<Value> + Send + Sync + 'static,
    ) -> FakeTelemetryServer {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(path);
        let listener = Socket::new(Domain::UNIX, Type::SEQPACKET, None).unwrap();
        listener.bind(&SockAddr::unix(path).unwrap()).unwrap();
        listener.listen(16).unwrap();
        listener.set_nonblocking(true).unwrap();
        let shared = Arc::new(Shared {
            pid,
            stop: AtomicBool::new(false),
            generation: AtomicUsize::new(0),
            connections: AtomicUsize::new(0),
            modes: Mutex::new(modes),
            queries: Mutex::new(Vec::new()),
            handler: Box::new(handler),
        });
        let s = shared.clone();
        let thread = thread::spawn(move || {
            let mut workers = Vec::new();
            while !s.stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let n = s.connections.fetch_add(1, Ordering::SeqCst);
                        let mode = {
                            let modes = s.modes.lock().unwrap();
                            modes
                                .get(n)
                                .or(modes.last())
                                .copied()
                                .unwrap_or(Mode::Serve)
                        };
                        let generation = s.generation.load(Ordering::SeqCst);
                        let s2 = s.clone();
                        workers.push(thread::spawn(move || {
                            connection(&s2, conn, mode, generation);
                        }));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
            for w in workers {
                let _ = w.join();
            }
        });
        FakeTelemetryServer {
            shared,
            path: path.to_path_buf(),
            thread: Some(thread),
        }
    }

    /// A server that answers like a `dpds` with the replies of `snapshot`.
    pub fn serving(path: &Path, pid: u32, snapshot: Snapshot) -> FakeTelemetryServer {
        let hists = full_hists(&snapshot);
        FakeTelemetryServer::start(path, pid, vec![Mode::Serve], move |cmd| {
            let (name, param) = cmd.split_once(',').unwrap_or((cmd, ""));
            let idx: usize = param.parse().unwrap_or(usize::MAX);
            match name {
                "/dpds/ports" => Some(snapshot.ports.clone()),
                "/dpds/stats" => snapshot.dirs.get(idx).cloned(),
                "/dpds/delay_hist" => hist_page(&hists, param),
                "/ethdev/xstats" => snapshot.eth.get(idx).cloned(),
                _ => None,
            }
        })
    }

    /// New connections use `modes` from index 0 on.
    pub fn set_modes(&self, modes: Vec<Mode>) {
        self.shared.connections.store(0, Ordering::SeqCst);
        *self.shared.modes.lock().unwrap() = modes;
    }

    /// Drops every connection that is open now.
    pub fn break_all(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Connections accepted so far (a client that reconnects raises it).
    pub fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }

    /// Every request received so far, in order.
    pub fn queries(&self) -> Vec<String> {
        self.shared.queries.lock().unwrap().clone()
    }
}

impl Drop for FakeTelemetryServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn connection(s: &Shared, mut conn: Socket, mode: Mode, generation: usize) {
    conn.set_nonblocking(false).unwrap();
    conn.set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let greeting = json!({"version": "fake", "pid": s.pid, "max_output_len": 16384});
    if conn.send(greeting.to_string().as_bytes()).is_err() || mode == Mode::CloseAfterGreeting {
        return;
    }
    let mut buf = [0u8; 1024];
    while !s.stop.load(Ordering::Relaxed) && s.generation.load(Ordering::SeqCst) == generation {
        let n = match conn.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(_) => return,
        };
        let cmd = String::from_utf8_lossy(&buf[..n]).into_owned();
        s.queries.lock().unwrap().push(cmd.clone());
        if mode == Mode::CloseOnLaterPage && is_later_page(&cmd) {
            return;
        }
        let value = match mode {
            Mode::Serve | Mode::CloseOnLaterPage => (s.handler)(&cmd),
            Mode::Nulls => None,
            Mode::Hang | Mode::CloseAfterGreeting => continue,
        };
        let name = cmd.split(',').next().unwrap_or(&cmd).to_string();
        let reply = json!({ name: value.unwrap_or(Value::Null) });
        if conn.send(reply.to_string().as_bytes()).is_err() {
            return;
        }
    }
}

fn is_later_page(cmd: &str) -> bool {
    let parts: Vec<&str> = cmd.split(',').collect();
    parts.len() == 3 && parts[0] == "/dpds/delay_hist" && parts[2] != "0"
}

/// A short temp dir below `/tmp` (the socket path must fit `sun_path`).
pub fn short_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dpt")
        .tempdir_in("/tmp")
        .unwrap()
}
