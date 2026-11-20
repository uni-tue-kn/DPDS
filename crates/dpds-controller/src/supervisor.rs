//! Supervision of the `dpds` child process.
//!
//! One long-lived thread owns the child: `PR_SET_PDEATHSIG` fires when the
//! thread that spawned the child exits, so the spawning thread must outlive it.
//! The child leads its own process group and gets `RUNTIME_DIRECTORY` set to the
//! dedicated `<runtime-dir>/instance` so that its telemetry socket is at a
//! short, known path; that subdirectory is the only thing ever removed.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::lcores::DirectionPlan;
use crate::telemetry::{instance_dir, socket_path, Client};

/// Lines of child output kept for the frontend.
const LOG_LINES: usize = 500;
/// Longest kept line of child output (the rest of a longer line is dropped).
const LOG_LINE_BYTES: usize = 4096;
const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

/// Everything needed to launch `dpds`; the supervisor writes `links_json` to `links_file`.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub dpds_bin: PathBuf,
    pub runtime_dir: PathBuf,
    pub links_file: PathBuf,
    pub links_json: Value,
    /// Arguments of `dpds` (EAL options, `--`, `--links <file>`).
    pub argv: Vec<String>,
    pub directions: Vec<DirectionPlan>,
    pub main_lcore: u32,
    pub drain_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct SupervisorStatus {
    pub state: State,
    pub pid: Option<u32>,
    /// Unix seconds.
    pub started_at: Option<u64>,
    pub error: Option<String>,
    pub log_tail: Vec<String>,
}

pub trait Supervisor: Send + Sync + 'static {
    /// Never blocks; failures show up as state `Failed`.
    fn start(&self, plan: LaunchPlan);
    /// Never blocks.
    fn stop(&self);
    fn status(&self) -> SupervisorStatus;
}

struct Status {
    state: State,
    pid: Option<u32>,
    started_at: Option<u64>,
    error: Option<String>,
}

struct Shared {
    status: Mutex<Status>,
    log: Mutex<VecDeque<String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn set(&self, state: State, pid: Option<u32>, started_at: Option<u64>, error: Option<String>) {
        *lock(&self.status) = Status {
            state,
            pid,
            started_at,
            error,
        };
    }

    fn push_log(&self, line: String) {
        let mut log = lock(&self.log);
        if log.len() == LOG_LINES {
            log.pop_front();
        }
        log.push_back(line);
    }
}

enum Cmd {
    Start(LaunchPlan),
    Stop,
    Shutdown,
}

pub struct ProcessSupervisor {
    shared: Arc<Shared>,
    tx: Mutex<Sender<Cmd>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ProcessSupervisor {
    /// `start_timeout`: how long `dpds` may take to answer on its telemetry socket.
    pub fn new(start_timeout: Duration) -> ProcessSupervisor {
        let shared = Arc::new(Shared {
            status: Mutex::new(Status {
                state: State::Stopped,
                pid: None,
                started_at: None,
                error: None,
            }),
            log: Mutex::new(VecDeque::new()),
        });
        let (tx, rx) = mpsc::channel();
        let s = shared.clone();
        let thread = thread::spawn(move || worker(&s, &rx, start_timeout));
        ProcessSupervisor {
            shared,
            tx: Mutex::new(tx),
            thread: Mutex::new(Some(thread)),
        }
    }

    /// Stops the child like `stop` (drain, then SIGKILL) and waits for it; the
    /// supervisor cannot be used afterwards. Called when the controller exits.
    pub fn shutdown(&self) {
        let _ = lock(&self.tx).send(Cmd::Shutdown);
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
    }
}

impl Drop for ProcessSupervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Supervisor for ProcessSupervisor {
    fn start(&self, plan: LaunchPlan) {
        {
            let mut st = lock(&self.shared.status);
            if matches!(st.state, State::Starting | State::Running | State::Stopping) {
                return;
            }
            *st = Status {
                state: State::Starting,
                pid: None,
                started_at: None,
                error: None,
            };
        }
        lock(&self.shared.log).clear();
        let _ = lock(&self.tx).send(Cmd::Start(plan));
    }

    fn stop(&self) {
        {
            let mut st = lock(&self.shared.status);
            if !matches!(st.state, State::Starting | State::Running) {
                return;
            }
            st.state = State::Stopping;
        }
        let _ = lock(&self.tx).send(Cmd::Stop);
    }

    fn status(&self) -> SupervisorStatus {
        let st = lock(&self.shared.status);
        SupervisorStatus {
            state: st.state,
            pid: st.pid,
            started_at: st.started_at,
            error: st.error.clone(),
            log_tail: lock(&self.shared.log).iter().cloned().collect(),
        }
    }
}

fn worker(shared: &Arc<Shared>, rx: &Receiver<Cmd>, start_timeout: Duration) {
    loop {
        match rx.recv() {
            Ok(Cmd::Start(plan)) => {
                if run_instance(shared, rx, &plan, start_timeout) {
                    return;
                }
            }
            Ok(Cmd::Stop) => {}
            Ok(Cmd::Shutdown) | Err(_) => return,
        }
    }
}

fn describe(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(c), _) => format!("exit status {c}"),
        (None, Some(s)) => format!("signal {s}"),
        _ => status.to_string(),
    }
}

/// The last captured line that `dpds` itself wrote (`dpds: ` prefix, not a warning), without the prefix.
fn dpds_reason(log: &VecDeque<String>) -> Option<&str> {
    log.iter()
        .rev()
        .filter_map(|l| l.strip_prefix("dpds: "))
        .find(|l| !l.starts_with("warning:"))
}

fn spawn(plan: &LaunchPlan) -> Result<Child, String> {
    // stale files of an earlier run (a crashed dpds leaves its socket behind)
    remove_runtime_dir(plan);
    let instance = instance_dir(&plan.runtime_dir);
    fs::create_dir_all(&instance)
        .map_err(|e| format!("cannot create {}: {e}", instance.display()))?;
    let json = serde_json::to_vec_pretty(&plan.links_json).map_err(|e| e.to_string())?;
    fs::write(&plan.links_file, json)
        .map_err(|e| format!("cannot write {}: {e}", plan.links_file.display()))?;
    let mut cmd = Command::new(&plan.dpds_bin);
    cmd.args(&plan.argv)
        .env("RUNTIME_DIRECTORY", &instance)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // SAFETY: the closure only calls prctl(2), which is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
        .map_err(|e| format!("cannot start {}: {e}", plan.dpds_bin.display()))
}

/// Removes the dedicated instance subdirectory (DPDK's runtime dir) and the
/// links file; nothing else in the runtime dir is touched.
fn remove_runtime_dir(plan: &LaunchPlan) {
    let _ = fs::remove_dir_all(instance_dir(&plan.runtime_dir));
    let _ = fs::remove_file(&plan.links_file);
}

/// Reads one line, keeping at most `LOG_LINE_BYTES` of it and dropping the rest.
/// `None` at the end of the stream.
fn read_line_capped(r: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let mut any = false;
    loop {
        let buf = r.fill_buf().ok()?;
        if buf.is_empty() {
            return any.then_some(line);
        }
        any = true;
        let (chunk, found) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], true),
            None => (buf, false),
        };
        let room = LOG_LINE_BYTES.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        let consumed = chunk.len() + usize::from(found);
        r.consume(consumed);
        if found {
            return Some(line);
        }
    }
}

fn capture(shared: &Arc<Shared>, stream: impl Read + Send + 'static) -> JoinHandle<()> {
    let shared = shared.clone();
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        while let Some(line) = read_line_capped(&mut reader) {
            let line = String::from_utf8_lossy(&line);
            let mut line = line.trim_end_matches('\r').to_string();
            // a cut multi-byte character becomes U+FFFD, which is longer
            while line.len() > LOG_LINE_BYTES {
                line.pop();
            }
            shared.push_log(line);
        }
    })
}

/// Waits (bounded) until the output readers saw EOF so the log is complete.
fn join_readers(readers: &[JoinHandle<()>]) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && !readers.iter().all(JoinHandle::is_finished) {
        thread::sleep(Duration::from_millis(10));
    }
}

/// `Ok(true)` once `dpds` answers `/dpds/ports` with the planned directions.
/// A transport error drops the client so the next probe reconnects; `last_error`
/// keeps the reason of the latest failed probe.
fn probe(
    plan: &LaunchPlan,
    pid: u32,
    client: &mut Option<Client>,
    last_error: &mut Option<String>,
) -> Result<bool, String> {
    if client.is_none() {
        let path = socket_path(&plan.runtime_dir);
        match Client::connect(&path) {
            Ok((c, greeting)) if greeting["pid"].as_u64() == Some(u64::from(pid)) => {
                *client = Some(c);
            }
            Ok((_, greeting)) => {
                *last_error = Some(format!(
                    "{} answered for pid {}, not {pid}",
                    path.display(),
                    greeting["pid"]
                ));
                return Ok(false);
            }
            Err(e) => {
                *last_error = Some(format!("cannot connect to {}: {e}", path.display()));
                return Ok(false);
            }
        }
    }
    let Some(c) = client.as_mut() else {
        return Ok(false);
    };
    let ports = match c.query("/dpds/ports") {
        Ok(p) => p,
        // the commands appear once dpds finished its setup; the connection may be dead
        Err(e) => {
            *client = None;
            *last_error = Some(e.to_string());
            return Ok(false);
        }
    };
    let text = |k: String| ports.get(&k).and_then(Value::as_str).map(str::to_lowercase);
    let num = |k: String| ports.get(&k).and_then(Value::as_u64);
    let planned = plan.directions.len() as u64;
    let mut mismatch = Vec::new();
    if num("num_directions".into()) != Some(planned) {
        mismatch.push(format!(
            "num_directions is {:?}, planned {planned}",
            num("num_directions".into())
        ));
    }
    for d in &plan.directions {
        let n = d.dir;
        if text(format!("dir{n}_rx_name")) != Some(d.rx.to_lowercase())
            || text(format!("dir{n}_tx_name")) != Some(d.tx.to_lowercase())
            || num(format!("dir{n}_lcore")) != Some(u64::from(d.lcore))
        {
            mismatch.push(format!(
                "direction {n} ({} -> {} on lcore {}) differs from what dpds reports",
                d.rx, d.tx, d.lcore
            ));
        }
    }
    if mismatch.is_empty() {
        Ok(true)
    } else {
        Err(format!(
            "dpds started with a different layout than planned: {}",
            mismatch.join("; ")
        ))
    }
}

fn kill_group(pid: u32) {
    // SAFETY: plain syscall; the child leads the process group `pid`.
    unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
}

/// Runs one `dpds` from spawn to exit. Returns true if the supervisor is to shut down.
fn run_instance(
    shared: &Arc<Shared>,
    rx: &Receiver<Cmd>,
    plan: &LaunchPlan,
    start_timeout: Duration,
) -> bool {
    let fail = |msg: String| {
        remove_runtime_dir(plan);
        shared.set(State::Failed, None, None, Some(msg));
    };
    let mut child = match spawn(plan) {
        Ok(c) => c,
        Err(e) => {
            fail(e);
            return false;
        }
    };
    let pid = child.id();
    let readers: Vec<JoinHandle<()>> = [
        child.stdout.take().map(|s| capture(shared, s)),
        child.stderr.take().map(|s| capture(shared, s)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let started = Instant::now();
    shared.set(State::Starting, Some(pid), None, None);

    let mut client = None;
    let mut last_error = None;
    let mut ready = false;
    let mut shutdown = None;
    let mut exited = false;
    let failure: Option<String> = loop {
        if let Ok(Some(status)) = child.try_wait() {
            exited = true;
            // a stop that was asked for (or is about to arrive) makes the exit a clean one
            let mut asked = lock(&shared.status).state == State::Stopping;
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    Cmd::Stop => asked = true,
                    Cmd::Shutdown => shutdown = Some(true),
                    Cmd::Start(_) => {}
                }
            }
            if asked || shutdown.is_some() {
                shutdown.get_or_insert(false);
                break None;
            }
            let what = if ready {
                "exited unexpectedly"
            } else {
                "exited"
            };
            // the child is gone, so its pipes close: wait for the readers, or the last
            // line (the reason) may not be in the log yet
            join_readers(&readers);
            let reason = dpds_reason(&lock(&shared.log))
                .map(|r| format!(": {r}"))
                .unwrap_or_default();
            break Some(format!("dpds {what} with {}{reason}", describe(status)));
        }
        match rx.recv_timeout(POLL) {
            Ok(Cmd::Stop) => {
                shutdown = Some(false);
                break None;
            }
            Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                shutdown = Some(true);
                break None;
            }
            Ok(Cmd::Start(_)) | Err(RecvTimeoutError::Timeout) => {}
        }
        if ready {
            continue;
        }
        match probe(plan, pid, &mut client, &mut last_error) {
            Ok(true) => {
                ready = true;
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                shared.set(State::Running, Some(pid), Some(now), None);
            }
            Ok(false) if started.elapsed() > start_timeout => {
                kill_group(pid);
                let _ = child.wait();
                let why = last_error
                    .map(|e| format!(" (last probe error: {e})"))
                    .unwrap_or_default();
                break Some(format!(
                    "timeout: dpds did not answer on its telemetry socket within {} s{why}",
                    start_timeout.as_secs()
                ));
            }
            Ok(false) => {}
            Err(msg) => {
                kill_group(pid);
                let _ = child.wait();
                break Some(msg);
            }
        }
    };

    if let Some(stop) = shutdown {
        if !exited {
            stop_child(shared, &mut child, plan.drain_timeout);
        }
        join_readers(&readers);
        remove_runtime_dir(plan);
        shared.set(State::Stopped, None, None, None);
        return stop;
    }
    join_readers(&readers);
    fail(failure.unwrap_or_default());
    false
}

/// SIGINT (dpds drains its buffers and exits), SIGKILL after the drain timeout.
fn stop_child(shared: &Shared, child: &mut Child, drain: Duration) {
    let pid = child.id();
    {
        let mut st = lock(&shared.status);
        st.state = State::Stopping;
    }
    // SAFETY: plain syscall on our own child.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
    let deadline = Instant::now() + drain;
    loop {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        if Instant::now() > deadline {
            shared.push_log(format!(
                "dpds did not exit within the drain timeout of {} s, killed",
                drain.as_secs()
            ));
            kill_group(pid);
            let _ = child.wait();
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}
