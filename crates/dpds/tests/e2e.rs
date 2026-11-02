//! End-to-end tests: run the real `dpds` binary (single data-plane core:
//! EAL main lcore + one worker, `-l 0-1`) against DPDK virtual devices.
//!
//! Environment: DPDK 26.07 with the `net_pcap` and `net_null` PMDs, no root,
//! no hugepages (`--no-huge -m 256 --no-pci --in-memory`).
//!
//! ## Oracle
//!
//! Exact packet counts are checked through TWO independent channels:
//! 1. the `tx_pcap` output files (frame by frame against the input), and
//! 2. with `--features telemetry`: DPDK's telemetry socket,
//!    `/dpds/stats` (`tx_sent == N`, `emulator_rejects == 0`, ...), queried
//!    BEFORE the SIGINT, plus `/dpds/epochs`, `/dpds/ports`, `/nic/*`.
//!
//! Tests that need the socket oracle are `#[cfg(feature = "telemetry")]`;
//! the suite also has to pass without the feature (then nothing of dpds may
//! be registered on the socket, see `without_the_feature_nothing_is_registered`).
//!
//! ## DPDK telemetry socket facts (verified by experiment with
//! `dpdk-testpmd` and a SOCK_SEQPACKET client on DPDK 26.07, non-root)
//!
//! - The socket exists in `--in-memory` mode too. Path:
//!   `$XDG_RUNTIME_DIR/dpdk/<file-prefix>/dpdk_telemetry.v2`, or
//!   `/tmp/dpdk/<file-prefix>/dpdk_telemetry.v2` when XDG_RUNTIME_DIR is unset
//!   (non-root). It is removed again when the process exits.
//! - `sockaddr_un.sun_path` holds 108 bytes: a longer path makes
//!   `rte_eal_init` FAIL (`TELEMETRY: Error with socket binding, path too
//!   long`, exit code 1). So the tests point XDG_RUNTIME_DIR at a short
//!   `/tmp/dpds-x......` directory (also keeps /tmp/dpdk clean).
//! - SOCK_SEQPACKET: on connect the server first sends the info message
//!   `{"version":"DPDK 26.07.0","pid":N,"max_output_len":16384}`; then every
//!   request (e.g. `/ethdev/stats,0`) gets exactly one datagram back:
//!   `{"<command WITHOUT the ,param>":<value>}`, at most max_output_len
//!   bytes. A command that does not exist (or returned an error) answers
//!   `{"/cmd":null}`; `/` lists all commands.
//! - Commands registered by the application are only there once its startup
//!   code has run: clients retry until the command is non-null.
//!
//! ## Other tests
//!
//! Network topology: with two ports the worker flips the port, so frames read
//! from port 0 leave on port 1 and vice versa. The two input files hold a
//! different number of frames so a mix-up cannot go unnoticed.
//!
//! Tests are serialised with a global lock (each spawns a busy-polling lcore,
//! and EAL runtime directories are per `--file-prefix`, which is also unique
//! per test). Every child is killed on drop, and every wait has a timeout.
//! A host with too few CPUs is a test FAILURE with a clear message, not a
//! silent pass.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

const DPDS: &str = env!("CARGO_BIN_EXE_dpds");
const NUM_MBUFS: &str = "8192";
const FRAME_LEN: usize = 64;
const START_TIMEOUT: Duration = Duration::from_secs(40);
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

static SERIAL: Mutex<()> = Mutex::new(());
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Fails (does not skip) when the host has fewer than `n` CPUs.
fn require_cpus(n: usize) {
    let have = thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1);
    assert!(
        have >= n,
        "these e2e tests need {n} CPUs (EAL lcores 0..{n}) but the host has {have}; run them on a machine with enough CPUs instead of skipping them"
    );
}

// ---------------------------------------------------------------------------
// pcap helpers (classic pcap, little endian, microsecond timestamps)
// ---------------------------------------------------------------------------

/// A frame carrying `tag` (which input port) and `seq` (frame number) so that
/// every frame is unique and traceable.
fn frame(tag: u8, seq: u32) -> Vec<u8> {
    let mut f = vec![0u8; FRAME_LEN];
    f[0..6].copy_from_slice(&[0xff; 6]); // dst
    f[6..12].copy_from_slice(&[0x02, 0, 0, 0, 0, tag]); // src
    f[12..14].copy_from_slice(&[0x08, 0x00]); // IPv4 ethertype (payload is not parsed)
    f[14..18].copy_from_slice(&seq.to_be_bytes());
    for (i, b) in f[18..].iter_mut().enumerate() {
        *b = (i as u8) ^ tag;
    }
    f
}

fn frames(tag: u8, n: u32) -> Vec<Vec<u8>> {
    (0..n).map(|seq| frame(tag, seq)).collect()
}

fn write_pcap(path: &Path, frames: &[Vec<u8>]) {
    let mut out = Vec::new();
    out.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes()); // magic
    out.extend_from_slice(&2u16.to_le_bytes()); // version major
    out.extend_from_slice(&4u16.to_le_bytes()); // version minor
    out.extend_from_slice(&0i32.to_le_bytes()); // thiszone
    out.extend_from_slice(&0u32.to_le_bytes()); // sigfigs
    out.extend_from_slice(&65535u32.to_le_bytes()); // snaplen
    out.extend_from_slice(&1u32.to_le_bytes()); // linktype: Ethernet
    for (i, f) in frames.iter().enumerate() {
        out.extend_from_slice(&0u32.to_le_bytes()); // ts_sec
        out.extend_from_slice(&(i as u32).to_le_bytes()); // ts_usec
        out.extend_from_slice(&(f.len() as u32).to_le_bytes()); // incl_len
        out.extend_from_slice(&(f.len() as u32).to_le_bytes()); // orig_len
        out.extend_from_slice(f);
    }
    fs::write(path, out).unwrap();
}

/// Frames of a pcap file; a trailing partial record (writer mid-flush) is
/// ignored, a missing/short file yields no frames.
fn read_pcap(path: &Path) -> Vec<Vec<u8>> {
    let Ok(data) = fs::read(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if data.len() < 24 {
        return out;
    }
    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
    assert!(
        magic == 0xa1b2_c3d4 || magic == 0xa1b2_3c4d,
        "unexpected pcap magic {magic:#x} in {}",
        path.display()
    );
    let mut pos = 24;
    while pos + 16 <= data.len() {
        let incl = u32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap()) as usize;
        pos += 16;
        if pos + incl > data.len() {
            break;
        }
        out.push(data[pos..pos + incl].to_vec());
        pos += incl;
    }
    out
}

// ---------------------------------------------------------------------------
// config helper
// ---------------------------------------------------------------------------

fn config_json(delay_ns: u64, loss: f64) -> String {
    config_json_cap(delay_ns, loss, 4096)
}

fn config_json_cap(delay_ns: u64, loss: f64, buffer_capacity: usize) -> String {
    format!(
        r#"{{
    "buffer_capacity": {buffer_capacity},
    "reordering": false,
    "delayer": {{
        "distribution": "DeterministicDistribution",
        "mean_ns": {delay_ns}.0,
        "std_ns": 0.0,
        "activate_correlation": false,
        "half_life_period_ns": 1000000.0
    }},
    "limiter": {{
        "mode": "Spacer",
        "activate": false,
        "rate_bps": 100000000000.0,
        "limit_bit": 6152000000.0
    }},
    "loss_model": {{ "Random": {loss:?} }}
}}"#
    )
}

// ---------------------------------------------------------------------------
// telemetry client: SOCK_SEQPACKET, info message first, one JSON datagram per reply
// ---------------------------------------------------------------------------

struct TelClient {
    fd: i32,
    max_output_len: usize,
}

impl TelClient {
    /// Connects and consumes the initial info message.
    fn connect(path: &Path) -> std::io::Result<TelClient> {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        assert!(
            bytes.len() < addr.sun_path.len(),
            "socket path too long for sockaddr_un ({} bytes): {}",
            bytes.len(),
            path.display()
        );
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (i, b) in bytes.iter().enumerate() {
            addr.sun_path[i] = *b as libc::c_char;
        }
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let tv = libc::timeval {
            tv_sec: 5,
            tv_usec: 0,
        };
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &tv as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
        }
        let rc = unsafe {
            libc::connect(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(e);
        }
        let mut c = TelClient {
            fd,
            max_output_len: 0,
        };
        let info = c.recv()?;
        let v: Value = serde_json::from_str(&info)
            .unwrap_or_else(|e| panic!("info message is not JSON ({e}): {info:?}"));
        c.max_output_len = v["max_output_len"]
            .as_u64()
            .unwrap_or_else(|| panic!("info message without max_output_len: {info:?}"))
            as usize;
        assert!(
            v["pid"].as_u64().is_some(),
            "info message without pid: {info:?}"
        );
        Ok(c)
    }

    fn recv(&mut self) -> std::io::Result<String> {
        let mut buf = vec![0u8; 64 * 1024];
        let n = unsafe { libc::recv(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        assert!(n > 0, "telemetry socket closed");
        Ok(String::from_utf8(buf[..n as usize].to_vec()).expect("reply is not UTF-8"))
    }

    /// Sends one command (`/cmd` or `/cmd,param`) and returns the reply
    /// value. One request -> exactly one datagram `{"<cmd w/o param>": value}`.
    fn query(&mut self, command: &str) -> std::io::Result<Value> {
        let rc = unsafe {
            libc::send(
                self.fd,
                command.as_ptr() as *const libc::c_void,
                command.len(),
                0,
            )
        };
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let text = self.recv()?;
        assert!(
            text.len() <= self.max_output_len,
            "reply longer than max_output_len ({}): {} bytes",
            self.max_output_len,
            text.len()
        );
        let v: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("reply to {command} is not JSON ({e}): {text:?}"));
        let obj = v
            .as_object()
            .unwrap_or_else(|| panic!("reply is not an object: {text}"));
        assert_eq!(obj.len(), 1, "reply must have exactly one key: {text}");
        let name = command.split(',').next().unwrap();
        // DPDK drops the ",param" part of the command in the reply key
        Ok(obj
            .get(name)
            .unwrap_or_else(|| panic!("reply key must be {name:?} (param dropped): {text}"))
            .clone())
    }
}

impl Drop for TelClient {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

// ---------------------------------------------------------------------------
// process harness
// ---------------------------------------------------------------------------

struct Run {
    child: Child,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
    status: Option<ExitStatus>,
    socket: Option<PathBuf>,
}

fn collect(mut r: impl Read + Send + 'static) -> Arc<Mutex<String>> {
    let buf = Arc::new(Mutex::new(String::new()));
    let b = buf.clone();
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            b.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    });
    buf
}

impl Run {
    fn spawn_in(
        args: &[String],
        xdg: Option<&Path>,
        socket: Option<PathBuf>,
        cwd: Option<&Path>,
    ) -> Run {
        let mut cmd = Command::new(DPDS);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(x) = xdg {
            cmd.env("XDG_RUNTIME_DIR", x);
        }
        if let Some(c) = cwd {
            cmd.current_dir(c);
        }
        let mut child = cmd.spawn().expect("failed to spawn dpds");
        let stdout = collect(child.stdout.take().unwrap());
        let stderr = collect(child.stderr.take().unwrap());
        Run {
            child,
            stdout,
            stderr,
            status: None,
            socket,
        }
    }

    fn output(&self) -> String {
        format!(
            "--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout.lock().unwrap(),
            self.stderr.lock().unwrap()
        )
    }

    /// Non-blocking: `Some(status)` once the child has exited.
    fn poll_exit(&mut self) -> Option<ExitStatus> {
        if self.status.is_none() {
            self.status = self.child.try_wait().expect("try_wait failed");
        }
        self.status
    }

    /// Waits for `cond`, failing fast (with the child's output) if the child
    /// exits first and failing on timeout.
    fn wait_until(
        &mut self,
        what: &str,
        timeout: Duration,
        mut cond: impl FnMut(&mut Run) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        loop {
            if cond(self) {
                return;
            }
            if let Some(st) = self.poll_exit() {
                if cond(self) {
                    return;
                }
                panic!(
                    "dpds exited early ({st}) while waiting for {what}\n{}",
                    self.output()
                );
            }
            assert!(
                Instant::now() < deadline,
                "timed out after {timeout:?} waiting for {what}\n{}",
                self.output()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// One telemetry query on a fresh connection. `None` while the socket is
    /// not there yet, refuses connections, or the command is not registered
    /// (yet): the caller retries.
    fn try_query(&self, command: &str) -> Option<Value> {
        let path = self
            .socket
            .as_ref()
            .expect("no telemetry socket for this run");
        if !path.exists() {
            return None;
        }
        let mut c = TelClient::connect(path).ok()?;
        match c.query(command) {
            Ok(Value::Null) | Err(_) => None,
            Ok(v) => Some(v),
        }
    }

    /// Queries until the command answers (it is registered at startup).
    #[cfg(feature = "telemetry")]
    fn query(&mut self, command: &str) -> Value {
        let mut out = None;
        self.wait_until(&format!("a reply to {command}"), START_TIMEOUT, |r| {
            out = r.try_query(command);
            out.is_some()
        });
        out.unwrap()
    }

    /// The raw `/` command list (works with and without the feature).
    fn command_list(&mut self) -> Vec<String> {
        let path = self.socket.clone().unwrap();
        let mut list = None;
        self.wait_until("the telemetry socket", START_TIMEOUT, |_| {
            if !path.exists() {
                return false;
            }
            let Ok(mut c) = TelClient::connect(&path) else {
                return false;
            };
            list = c.query("/").ok();
            list.is_some()
        });
        list.unwrap()
            .as_array()
            .expect("/ replies with an array")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    fn signal(&self, sig: i32) {
        let rc = unsafe { libc::kill(self.child.id() as i32, sig) };
        assert_eq!(rc, 0, "kill({sig}) failed");
    }

    fn wait_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            if let Some(st) = self.poll_exit() {
                // Let the collector threads drain the pipes.
                thread::sleep(Duration::from_millis(100));
                return st;
            }
            assert!(
                Instant::now() < deadline,
                "dpds did not exit within {EXIT_TIMEOUT:?}\n{}",
                self.output()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct Env {
    dir: tempfile::TempDir,
    /// Short XDG_RUNTIME_DIR (the telemetry socket path must fit sun_path).
    xdg: tempfile::TempDir,
    lcore_list: String,
    vdevs: Vec<String>,
    config: PathBuf,
    prefix: String,
    extra_app_args: Vec<String>,
}

impl Env {
    /// `-l 0-1`: the main lcore plus the one data-plane worker.
    fn new(config: &str) -> Env {
        Env::with_lcores("0-1", config)
    }

    fn with_lcores(lcore_list: &str, config: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let xdg = tempfile::Builder::new()
            .prefix("dpds-x")
            .tempdir_in("/tmp")
            .unwrap();
        let config_path = dir.path().join("config.json");
        fs::write(&config_path, config).unwrap();
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        Env {
            config: config_path,
            prefix: format!("e{}-{id}", std::process::id()),
            vdevs: Vec::new(),
            lcore_list: lcore_list.to_string(),
            extra_app_args: Vec::new(),
            xdg,
            dir,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn socket_path(&self) -> PathBuf {
        self.xdg
            .path()
            .join("dpdk")
            .join(&self.prefix)
            .join("dpdk_telemetry.v2")
    }

    fn eal_args(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "--no-huge",
            "-m",
            "256",
            "--no-pci",
            "--in-memory",
            "--file-prefix",
            &self.prefix,
            "-l",
            &self.lcore_list,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for v in &self.vdevs {
            a.push(format!("--vdev={v}"));
        }
        a
    }

    fn app_args(&self) -> Vec<String> {
        let mut a = vec![
            "--config".into(),
            self.config.display().to_string(),
            "--num-mbufs".into(),
            NUM_MBUFS.into(),
        ];
        a.extend(self.extra_app_args.iter().cloned());
        a
    }

    fn args(&self) -> Vec<String> {
        let mut a = self.eal_args();
        a.push("--".into());
        a.extend(self.app_args());
        a
    }

    fn spawn(&self) -> Run {
        Run::spawn_in(
            &self.args(),
            Some(self.xdg.path()),
            Some(self.socket_path()),
            None,
        )
    }
}

/// Adds one `net_pcap` port reading `in<idx>.pcap` and writing `out<idx>.pcap`.
fn add_pcap_port(env: &mut Env, idx: usize, input: &[Vec<u8>]) {
    let inp = env.path(&format!("in{idx}.pcap"));
    write_pcap(&inp, input);
    let out = env.path(&format!("out{idx}.pcap"));
    env.vdevs.push(format!(
        "net_pcap{idx},rx_pcap={},tx_pcap={}",
        inp.display(),
        out.display()
    ));
}

fn assert_clean_exit(st: ExitStatus, run: &Run) {
    assert_eq!(
        st.code(),
        Some(0),
        "expected exit status 0, got {st}\n{}",
        run.output()
    );
}

/// The per-second dashboard, the final-stats print, the burst histograms and
/// the CSV export are gone.
fn assert_no_old_output(run: &Run) {
    let out = run.output();
    for gone in [
        "Emulator Stats",
        "=== Final Stats ===",
        "Burst Histograms",
        "burst/s",
        "Stats saved to",
    ] {
        assert!(
            !out.contains(gone),
            "{gone:?} must not be printed any more:\n{out}"
        );
    }
}

// ---------------------------------------------------------------------------
// JSON field helpers
// ---------------------------------------------------------------------------

fn u(v: &Value, key: &str) -> u64 {
    v[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key} is not a uint in {v}"))
}

#[cfg(feature = "telemetry")]
fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string in {v}"))
}

#[cfg(feature = "telemetry")]
fn arr_u(v: &Value, key: &str) -> Vec<u64> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is not an array in {v}"))
        .iter()
        .map(|x| {
            x.as_u64()
                .unwrap_or_else(|| panic!("{key} holds a non-uint in {v}"))
        })
        .collect()
}

#[cfg(feature = "telemetry")]
fn arr_s(v: &Value, key: &str) -> Vec<String> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is not an array in {v}"))
        .iter()
        .map(|x| {
            x.as_str()
                .unwrap_or_else(|| panic!("{key} holds a non-string in {v}"))
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// "RX has read everything": the telemetry oracle with the feature, time without
// ---------------------------------------------------------------------------

/// Waits until dpds has received `n` packets. With telemetry: `/dpds/stats`
/// `rx_pkts >= n`. Without: a fixed grace period (pcap files are tiny).
fn wait_rx(run: &mut Run, n: u64) {
    if cfg!(feature = "telemetry") {
        run.wait_until(&format!("rx_pkts >= {n}"), START_TIMEOUT, |r| {
            r.try_query("/dpds/stats")
                .is_some_and(|v| u(&v, "rx_pkts") >= n)
        });
    } else {
        thread::sleep(Duration::from_millis(2500));
    }
}

// ---------------------------------------------------------------------------
// exact-count oracle
// ---------------------------------------------------------------------------

fn exact_count(in_frames: &[Vec<Vec<u8>>]) {
    require_cpus(2);
    let _g = serial();
    let ports = in_frames.len();
    let mut env = Env::new(&config_json(1_000_000, 0.0));
    for (i, f) in in_frames.iter().enumerate() {
        add_pcap_port(&mut env, i, f);
    }
    // Where each input stream must come out: with two ports the worker flips
    // the port, with one port frames leave on the port they came in on.
    let dest = |i: usize| if ports == 2 { 1 - i } else { i };
    let total: usize = in_frames.iter().map(Vec::len).sum();

    let outs: Vec<PathBuf> = (0..ports)
        .map(|i| env.path(&format!("out{i}.pcap")))
        .collect();
    let mut run = env.spawn();

    run.wait_until("all frames in the output pcaps", START_TIMEOUT, |_| {
        outs.iter().map(|p| read_pcap(p).len()).sum::<usize>() >= total
    });
    // Linger: duplicates or late stragglers would show up here.
    thread::sleep(Duration::from_millis(700));

    // Telemetry oracle, queried BEFORE the SIGINT.
    #[cfg(feature = "telemetry")]
    {
        let st = run.query("/dpds/stats");
        let total = total as u64;
        assert_eq!(u(&st, "rx_pkts"), total, "{st}");
        assert_eq!(u(&st, "pops"), total, "{st}");
        assert_eq!(u(&st, "tx_pkts"), total, "{st}");
        assert_eq!(u(&st, "tx_sent"), total, "{st}");
        assert_eq!(u(&st, "emulator_rejects"), 0, "{st}");
        assert_eq!(u(&st, "tx_drops"), 0, "{st}");
        assert_eq!(u(&st, "tx_short_calls"), 0, "{st}");
        assert!(
            u(&st, "tx_calls") >= 1 && u(&st, "tx_calls") <= total,
            "{st}"
        );
        assert!(
            u(&st, "tx_batch_max") >= 1 && u(&st, "tx_batch_max") <= 16,
            "{st}"
        );
        assert!(u(&st, "polls") >= 1, "{st}");
        assert!(u(&st, "uptime_ns") > 0, "{st}");
        // Zero delay: sampled after the pop, so the depth stays small.
        assert!(u(&st, "buffer_len_max") <= total, "{st}");
        assert_eq!(s(&st, "first_loss_reason"), "Clean", "{st}");
        assert_eq!(s(&st, "first_loss_detail"), "", "{st}");
        assert_eq!(u(&st, "first_loss_relative_ns"), 0, "{st}");
        let per_call: f64 = s(&st, "tx_pkts_per_call")
            .parse()
            .expect("a decimal string");
        assert!((1.0..=16.0).contains(&per_call), "{st}");
        assert_eq!(
            per_call,
            (u(&st, "tx_pkts") as f64 / u(&st, "tx_calls") as f64 * 100.0).round() / 100.0,
            "tx_pkts_per_call is the 2-decimal mean: {st}"
        );
        // hardware counters of the NIC itself: nothing lost
        let hw = run.query("/nic/hw_first_loss,0");
        assert_eq!(s(&hw, "hw_first_loss_reason"), "Clean", "{hw}");
    }

    run.signal(libc::SIGINT);
    let st = run.wait_exit();
    assert_clean_exit(st, &run);
    assert_no_old_output(&run);

    for (i, input) in in_frames.iter().enumerate() {
        let got = read_pcap(&outs[dest(i)]);
        assert_eq!(
            got.len(),
            input.len(),
            "port {i} stream: expected exactly {} frames on out{}\n{}",
            input.len(),
            dest(i),
            run.output()
        );
        // Zero delay jitter, no reordering: byte-identical, in order.
        assert!(got == *input, "frames on out{} differ from in{i}", dest(i));
    }
}

#[test]
fn pcap_one_port() {
    exact_count(&[frames(0, 1000)]);
}

#[test]
fn pcap_two_ports() {
    exact_count(&[frames(0, 1000), frames(1, 600)]);
}

#[test]
fn pcap_one_port_input_around_the_burst_size() {
    // Boundary sizes around MAX_BURST_SIZE = 16: 1, 15, 16, 17 frames.
    for n in [1u32, 15, 16, 17] {
        exact_count(&[frames(0, n)]);
    }
}

#[test]
fn pcap_two_ports_one_input_empty() {
    // Port 1 has nothing to send; everything from port 0 must leave on port 1.
    exact_count(&[frames(0, 300), Vec::new()]);
}

// ---------------------------------------------------------------------------
// loss
// ---------------------------------------------------------------------------

#[test]
fn full_loss_transmits_nothing_and_counts_emulator_rejects() {
    const N: u32 = 500;
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 1.0));
    add_pcap_port(&mut env, 0, &frames(0, N));
    let mut run = env.spawn();

    wait_rx(&mut run, N as u64);

    #[cfg(feature = "telemetry")]
    {
        let st = run.query("/dpds/stats");
        assert_eq!(u(&st, "rx_pkts"), N as u64, "{st}");
        assert_eq!(u(&st, "emulator_rejects"), N as u64, "{st}");
        assert_eq!(u(&st, "pops"), 0, "{st}");
        assert_eq!(u(&st, "tx_sent"), 0, "{st}");
        assert_eq!(u(&st, "tx_calls"), 0, "{st}");
        assert_eq!(s(&st, "first_loss_reason"), "EmulatorReject", "{st}");
        assert_eq!(s(&st, "first_loss_detail"), "from=dpds", "{st}");
        assert!(u(&st, "first_loss_relative_ns") > 0, "{st}");
        assert_eq!(s(&st, "tx_pkts_per_call"), "0.00", "{st}");
    }

    run.signal(libc::SIGINT);
    let st = run.wait_exit();
    assert_clean_exit(st, &run);
    assert_eq!(read_pcap(&env.path("out0.pcap")).len(), 0);
}

// ---------------------------------------------------------------------------
// ordered shutdown: packets still delayed in the buffer must be flushed
// ---------------------------------------------------------------------------

#[test]
fn sigint_drains_packets_still_held_in_the_delay_buffer() {
    const N: u32 = 300;
    require_cpus(2);
    let _g = serial();
    // 3 s delay: once RX has read everything none can have been sent yet.
    let mut env = Env::new(&config_json(3_000_000_000, 0.0));
    add_pcap_port(&mut env, 0, &frames(0, N));
    let mut run = env.spawn();

    wait_rx(&mut run, N as u64);
    assert_eq!(
        read_pcap(&env.path("out0.pcap")).len(),
        0,
        "packets left before their delay elapsed"
    );
    #[cfg(feature = "telemetry")]
    {
        let st = run.query("/dpds/stats");
        assert_eq!(u(&st, "tx_sent"), 0, "{st}");
        assert_eq!(u(&st, "emulator_rejects"), 0, "{st}");
        assert_eq!(
            u(&st, "buffer_len_max"),
            N as u64,
            "all of them wait in the buffer: {st}"
        );
    }
    run.signal(libc::SIGINT);
    let st = run.wait_exit();
    assert_clean_exit(st, &run);

    let got = read_pcap(&env.path("out0.pcap"));
    assert_eq!(
        got,
        frames(0, N),
        "shutdown lost or reordered delayed packets"
    );
}

// ---------------------------------------------------------------------------
// net_null smoke tests (line-rate packet source, packets are just freed)
// ---------------------------------------------------------------------------

fn null_smoke(ports: usize, sig: i32) {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    for p in 0..ports {
        env.vdevs.push(format!("net_null{p}"));
    }
    let mut run = env.spawn();

    #[cfg(feature = "telemetry")]
    {
        // Traffic is flowing and keeps growing.
        run.wait_until("rx traffic", START_TIMEOUT, |r| {
            r.try_query("/dpds/stats")
                .is_some_and(|v| u(&v, "rx_pkts") > 0)
        });
        let first = u(&run.query("/dpds/stats"), "rx_pkts");
        run.wait_until("rx_pkts to keep growing", START_TIMEOUT, |r| {
            r.try_query("/dpds/stats")
                .is_some_and(|v| u(&v, "rx_pkts") > first)
        });
    }
    #[cfg(not(feature = "telemetry"))]
    thread::sleep(Duration::from_millis(1500));
    assert!(
        run.poll_exit().is_none(),
        "dpds died while running\n{}",
        run.output()
    );

    run.signal(sig);
    let st = run.wait_exit();
    assert_clean_exit(st, &run);
    assert_no_old_output(&run);
}

#[test]
fn null_one_port_sigint() {
    null_smoke(1, libc::SIGINT);
}

#[test]
fn null_two_ports_sigint() {
    null_smoke(2, libc::SIGINT);
}

#[test]
fn null_one_port_sigterm() {
    null_smoke(1, libc::SIGTERM);
}

#[test]
fn null_two_ports_sigterm() {
    null_smoke(2, libc::SIGTERM);
}

// ---------------------------------------------------------------------------
// the telemetry socket: what is registered, and what the commands answer
// ---------------------------------------------------------------------------

#[test]
fn without_the_feature_nothing_is_registered() {
    if cfg!(feature = "telemetry") {
        return; // covered by the commands_are_registered... test
    }
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let mut run = env.spawn();
    let list = run.command_list();
    // The socket exists right after EAL init; give the app time to finish its
    // startup (signal handlers!) before the SIGINT below.
    thread::sleep(Duration::from_millis(1500));
    assert!(
        list.iter().any(|c| c == "/ethdev/stats"),
        "DPDK's own commands: {list:?}"
    );
    for c in &list {
        assert!(
            !c.starts_with("/dpds/") && !c.starts_with("/nic/"),
            "{c} registered although the telemetry feature is off"
        );
    }
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
}

#[cfg(feature = "telemetry")]
mod with_telemetry {
    use super::*;

    #[test]
    fn the_commands_are_registered_next_to_dpdks_own() {
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(0, 0.0));
        env.vdevs.push("net_null0".into());
        let mut run = env.spawn();
        // registered at startup, before the worker is launched
        run.query("/dpds/stats");
        let list = run.command_list();
        for c in [
            "/dpds/stats",
            "/dpds/epochs",
            "/dpds/ports",
            "/nic/hw_first_loss",
            "/nic/epochs",
            "/ethdev/stats",
        ] {
            assert!(list.iter().any(|x| x == c), "{c} missing from {list:?}");
        }
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn ports_reports_names_and_applied_vs_requested_ring_sizes() {
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(0, 0.0));
        add_pcap_port(&mut env, 0, &frames(0, 4));
        add_pcap_port(&mut env, 1, &frames(1, 4));
        let mut run = env.spawn();
        let p = run.query("/dpds/ports");
        assert_eq!(u(&p, "num_ports"), 2, "{p}");
        for i in 0..2 {
            assert_eq!(
                s(&p, &format!("port{i}_name")),
                format!("net_pcap{i}"),
                "{p}"
            );
            let rx = u(&p, &format!("port{i}_rx_ring"));
            let tx = u(&p, &format!("port{i}_tx_ring"));
            assert!(rx >= 1 && tx >= 1, "applied sizes: {p}");
            assert_eq!(u(&p, &format!("port{i}_rx_ring_requested")), 65535, "{p}");
            assert_eq!(u(&p, &format!("port{i}_tx_ring_requested")), 65535, "{p}");
            assert!(
                rx <= 65535 && tx <= 65535,
                "applied never exceeds requested: {p}"
            );
        }
        assert!(p.get("port2_name").is_none(), "{p}");
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);

        // one port
        let mut env = Env::new(&config_json(0, 0.0));
        env.vdevs.push("net_null0".into());
        let mut run = env.spawn();
        let p = run.query("/dpds/ports");
        assert_eq!(u(&p, "num_ports"), 1, "{p}");
        assert_eq!(s(&p, "port0_name"), "net_null0", "{p}");
        assert!(p.get("port1_name").is_none(), "{p}");
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn an_idle_gap_after_traffic_closes_exactly_one_epoch() {
        const N: u32 = 400;
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(1_000_000, 0.0));
        add_pcap_port(&mut env, 0, &frames(0, N));
        let mut run = env.spawn();

        wait_rx(&mut run, N as u64);
        // pcap EOF = no more arrivals; > 500 ms later the epoch is closed
        run.wait_until("one closed epoch", START_TIMEOUT, |r| {
            r.try_query("/dpds/epochs")
                .is_some_and(|v| u(&v, "epochs_closed") >= 1)
        });
        // ... and stays at one: idle ticks after a close emit no empty epochs
        thread::sleep(Duration::from_millis(1500));
        let ep = run.query("/dpds/epochs");

        assert_eq!(u(&ep, "epochs_closed"), 1, "{ep}");
        assert_eq!(u(&ep, "epochs_dropped"), 0, "{ep}");
        for k in [
            "epoch_start_uptime_ns",
            "epoch_end_uptime_ns",
            "first_loss_relative_ns",
            "polls",
            "rx_pkts",
            "rx_full_bursts",
            "emulator_rejects",
            "pops",
            "tx_calls",
            "tx_pkts",
            "tx_sent",
            "tx_short_calls",
            "tx_batch_max",
            "buffer_len_max",
        ] {
            assert_eq!(arr_u(&ep, k).len(), 1, "{k} has one entry per epoch: {ep}");
        }
        assert_eq!(arr_s(&ep, "first_loss_reason"), ["Clean"], "{ep}");
        assert_eq!(arr_s(&ep, "first_loss_detail"), [""], "{ep}");
        // the counters of the epoch are deltas that add up to the whole run
        assert_eq!(arr_u(&ep, "rx_pkts"), [N as u64], "{ep}");
        assert_eq!(arr_u(&ep, "pops"), [N as u64], "{ep}");
        assert_eq!(arr_u(&ep, "tx_pkts"), [N as u64], "{ep}");
        assert_eq!(arr_u(&ep, "tx_sent"), [N as u64], "{ep}");
        assert_eq!(arr_u(&ep, "emulator_rejects"), [0], "{ep}");
        assert!(arr_u(&ep, "polls")[0] >= 1, "{ep}");
        assert!(arr_u(&ep, "buffer_len_max")[0] <= N as u64, "{ep}");
        let (start, end) = (
            arr_u(&ep, "epoch_start_uptime_ns")[0],
            arr_u(&ep, "epoch_end_uptime_ns")[0],
        );
        assert!(end > start, "{ep}");
        assert!(
            end >= 500_000_000,
            "the boundary is more than 500 ms after the start: {ep}"
        );

        // the whole-process stats agree with the epoch
        let st = run.query("/dpds/stats");
        assert_eq!(u(&st, "tx_sent"), N as u64, "{st}");
        assert!(u(&st, "uptime_ns") >= end, "{st}");

        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn no_traffic_no_epochs() {
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(0, 0.0));
        add_pcap_port(&mut env, 0, &[]);
        let mut run = env.spawn();
        run.query("/dpds/epochs");
        thread::sleep(Duration::from_millis(1800));
        let ep = run.query("/dpds/epochs");
        assert_eq!(
            u(&ep, "epochs_closed"),
            0,
            "an idle start must not emit empty epochs: {ep}"
        );
        assert_eq!(arr_u(&ep, "rx_pkts"), Vec::<u64>::new(), "{ep}");
        let st = run.query("/dpds/stats");
        assert_eq!(u(&st, "rx_pkts"), 0, "{st}");
        assert!(u(&st, "polls") > 0, "the loop is spinning: {st}");
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn nic_commands_follow_the_main_lcores_poll() {
        const N: u32 = 200;
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(0, 0.0));
        add_pcap_port(&mut env, 0, &frames(0, N));
        let mut run = env.spawn();
        wait_rx(&mut run, N as u64);

        // /nic/epochs,0 is driven by the port's own ipackets from the 250 ms
        // poll loop: it closes an epoch once the arrivals have stopped for
        // more than 500 ms (so, ~1 s after the pcap ended)
        run.wait_until("a NIC epoch", START_TIMEOUT, |r| {
            r.try_query("/nic/epochs,0")
                .is_some_and(|v| u(&v, "epochs_closed") >= 1)
        });
        let ne = run.query("/nic/epochs,0");
        assert_eq!(
            arr_u(&ne, "rx_packets").iter().sum::<u64>(),
            N as u64,
            "{ne}"
        );
        assert_eq!(
            arr_u(&ne, "imissed"),
            vec![0; arr_u(&ne, "imissed").len()],
            "{ne}"
        );
        assert_eq!(
            arr_u(&ne, "rx_port_discards").len(),
            arr_u(&ne, "rx_packets").len(),
            "{ne}"
        );
        assert_eq!(
            arr_s(&ne, "first_loss_reason"),
            vec!["Clean"; arr_s(&ne, "first_loss_reason").len()],
            "{ne}"
        );
        let (a, b) = (
            arr_u(&ne, "epoch_start_uptime_ns"),
            arr_u(&ne, "epoch_end_uptime_ns"),
        );
        assert!(a[0] < b[0], "{ne}");

        let hw = run.query("/nic/hw_first_loss,0");
        assert_eq!(s(&hw, "hw_first_loss_reason"), "Clean", "{hw}");
        assert_eq!(s(&hw, "hw_first_loss_detail"), "", "{hw}");
        assert_eq!(u(&hw, "hw_first_loss_relative_ns"), 0, "{hw}");

        // only one port exists: ,1 is not a polled port
        assert!(run
            .try_query("/nic/hw_first_loss,1")
            .is_none_or(|v| v.get("hw_first_loss_reason").is_none()));
        assert!(run
            .try_query("/nic/epochs,1")
            .is_none_or(|v| v.get("epochs_closed").is_none()));

        // DPDK's own cumulative counters agree with the dpds ones
        let eth = run.query("/ethdev/stats,0");
        assert_eq!(u(&eth, "ipackets"), N as u64, "{eth}");
        assert_eq!(u(&eth, "opackets"), N as u64, "{eth}");
        assert_eq!(u(&eth, "imissed"), 0, "{eth}");

        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    // ── mbuf leaks: a tiny pool must not run dry ─────────────────────

    /// net_null produces packets as fast as it is polled. With a pool of
    /// 4096 mbufs the loop only keeps running if every mbuf is returned:
    /// `rx_pkts` far beyond the pool size and `rx_nombuf == 0`.
    fn no_leak(config: String, expect_rejects: bool) {
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config);
        env.vdevs.push("net_null0".into());
        env.extra_app_args = vec![]; // NUM_MBUFS is replaced below
        let mut args = env.eal_args();
        args.push("--".into());
        args.extend(["--config".into(), env.config.display().to_string()]);
        args.extend(["--num-mbufs".into(), "4096".into()]);
        let mut run = Run::spawn_in(&args, Some(env.xdg.path()), Some(env.socket_path()), None);

        run.wait_until("rx_pkts far above the pool size", START_TIMEOUT, |r| {
            r.try_query("/dpds/stats")
                .is_some_and(|v| u(&v, "rx_pkts") > 20 * 4096)
        });
        // run for a few seconds
        thread::sleep(Duration::from_secs(3));
        let st = run.query("/dpds/stats");
        assert!(u(&st, "rx_pkts") > 100 * 4096, "{st}");
        if expect_rejects {
            assert!(
                u(&st, "emulator_rejects") > 0,
                "the tiny buffer must overflow: {st}"
            );
            assert_eq!(s(&st, "first_loss_reason"), "EmulatorReject", "{st}");
        }
        let eth = run.query("/ethdev/stats,0");
        assert_eq!(
            u(&eth, "rx_nombuf"),
            0,
            "an mbuf leak starves the pool: {eth}"
        );
        let hw = run.query("/nic/hw_first_loss,0");
        assert_ne!(s(&hw, "hw_first_loss_reason"), "RX_NoMbuf", "{hw}");

        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn full_loss_on_a_tiny_pool_does_not_leak_mbufs() {
        no_leak(config_json(0, 1.0), true);
    }

    #[test]
    fn delay_with_a_tiny_buffer_does_not_leak_mbufs() {
        // buffer-full rejects are freed too
        no_leak(config_json_cap(1_000_000, 0.0, 64), true);
    }

    #[test]
    fn plain_forwarding_on_a_tiny_pool_does_not_leak_mbufs() {
        no_leak(config_json(0, 0.0), false);
    }

    #[test]
    fn the_stats_are_consistent_with_each_other_under_load() {
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(0, 0.5));
        env.vdevs.push("net_null0".into());
        let mut run = env.spawn();
        run.wait_until("traffic", START_TIMEOUT, |r| {
            r.try_query("/dpds/stats")
                .is_some_and(|v| u(&v, "rx_pkts") > 100_000)
        });
        let st = run.query("/dpds/stats");
        // read at slightly different moments by the telemetry thread, so only
        // inequalities that hold at any moment: nothing sent that was not received
        assert!(
            u(&st, "pops") + u(&st, "emulator_rejects") <= u(&st, "rx_pkts") + 16,
            "{st}"
        );
        assert!(u(&st, "tx_sent") <= u(&st, "tx_pkts"), "{st}");
        assert_eq!(
            u(&st, "tx_drops"),
            u(&st, "tx_pkts").saturating_sub(u(&st, "tx_sent")),
            "{st}"
        );
        assert!(u(&st, "rx_full_bursts") <= u(&st, "polls"), "{st}");
        assert!(u(&st, "emulator_rejects") > 0, "50 % loss: {st}");
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }
}

// ---------------------------------------------------------------------------
// error paths: must exit with a status code (no panic/abort, no hang)
// ---------------------------------------------------------------------------

fn expect_failure_args(args: Vec<String>, xdg: &Path) -> String {
    let mut run = Run::spawn_in(&args, Some(xdg), None, None);
    let deadline = Instant::now() + EXIT_TIMEOUT;
    let st = loop {
        if let Some(st) = run.poll_exit() {
            break st;
        }
        assert!(
            Instant::now() < deadline,
            "dpds hung instead of failing\n{}",
            run.output()
        );
        thread::sleep(Duration::from_millis(20));
    };
    thread::sleep(Duration::from_millis(100));
    let out = run.output();
    let code = st
        .code()
        .unwrap_or_else(|| panic!("killed by a signal ({st}), expected an error exit\n{out}"));
    assert_ne!(code, 0, "expected non-zero exit\n{out}");
    out
}

fn expect_failure(env: &Env) -> String {
    expect_failure_args(env.args(), env.xdg.path())
}

#[test]
fn one_lcore_is_rejected_and_the_message_names_the_count() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::with_lcores("0", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert!(
        out.contains('1') && out.contains('2'),
        "count and requirement named\n{out}"
    );
}

#[test]
fn three_lcores_are_rejected_there_is_exactly_one_worker() {
    require_cpus(3);
    let _g = serial();
    let mut env = Env::with_lcores("0-2", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert!(out.contains('3'), "the lcore count must be named\n{out}");
}

#[test]
fn four_lcores_are_rejected_too() {
    require_cpus(4);
    let _g = serial();
    let mut env = Env::with_lcores("0-3", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    expect_failure(&env);
}

#[test]
fn zero_ports_are_rejected() {
    require_cpus(2);
    let _g = serial();
    let env = Env::new(&config_json(0, 0.0));
    expect_failure(&env);
}

#[test]
fn three_ports_are_rejected() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    for p in 0..3 {
        env.vdevs.push(format!("net_null{p}"));
    }
    expect_failure(&env);
}

#[test]
fn unknown_app_argument_is_rejected_with_usage() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    env.extra_app_args = vec!["--bogus".into()];
    let out = expect_failure(&env);
    assert!(out.contains("Usage"), "usage message expected\n{out}");
}

#[test]
fn the_removed_csv_option_is_rejected() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let csv = env.path("stats.csv");
    env.extra_app_args = vec!["-o".into(), csv.display().to_string()];
    let out = expect_failure(&env);
    assert!(out.contains("Usage"), "{out}");
    assert!(!csv.exists(), "no CSV is ever written");
}

#[test]
fn missing_config_file_is_an_error_exit_not_a_crash() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    fs::remove_file(&env.config).unwrap();
    expect_failure(&env);
}

#[test]
fn malformed_config_file_is_an_error_exit_not_a_crash() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new("{ definitely not json");
    env.vdevs.push("net_null0".into());
    expect_failure(&env);
}

#[test]
fn default_config_path_is_config_json_in_the_working_directory() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    // No : dpds must pick up ./config.json from its cwd.
    let mut args = env.eal_args();
    args.push("--".into());
    args.extend(["--num-mbufs".into(), NUM_MBUFS.into()]);

    let mut run = Run::spawn_in(
        &args,
        Some(env.xdg.path()),
        Some(env.socket_path()),
        Some(env.dir.path()),
    );
    #[cfg(feature = "telemetry")]
    run.wait_until("traffic", START_TIMEOUT, |r| {
        r.try_query("/dpds/stats")
            .is_some_and(|v| u(&v, "rx_pkts") > 0)
    });
    #[cfg(not(feature = "telemetry"))]
    {
        thread::sleep(Duration::from_millis(1500));
        assert!(run.poll_exit().is_none(), "{}", run.output());
    }
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
}
