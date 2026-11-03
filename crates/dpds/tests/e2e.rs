//! End-to-end tests: run the real `dpds` binary (one worker lcore per
//! direction: 1 port = `-l 0-1`, 2 ports = `-l 0-2`) against DPDK virtual
//! devices.
//!
//! Environment: DPDK 26.07 with the `net_pcap` and `net_null` PMDs, no root,
//! no hugepages (`--no-huge -m 256 --no-pci --in-memory`).
//!
//! ## Oracle
//!
//! Exact packet counts are checked through TWO independent channels:
//! 1. the `tx_pcap` output files (frame by frame against the input), and
//! 2. with `--features telemetry`: DPDK's telemetry socket,
//!    `/dpds/stats,<dir>` (`tx_sent == N`, `dpds_rejects == 0`, ...), queried
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
//! Network topology: a link is one direction, ingress port -> egress port, with
//! its own worker lcore, its own config, its own mempool and its own
//! telemetry counters (`/dpds/stats,<d>`, d = link index). The `--config*`
//! shorthand builds the links `dir0` and `dir1`: direction `d` = traffic
//! received on port `d`, config `--config<d>`; with two ports frames read
//! from port 0 leave on port 1 and vice versa; with one port they leave on
//! the port they came in on (one hairpin link `dir0`). The input files hold
//! a different number of frames so a mix-up cannot go unnoticed.
//!
//! Pool size: there is no `--num-mbufs` (it is a usage error). Direction d's
//! pool holds `buffer_capacity_d + rx_desc(port d) + tx_desc(port e(d)) + 768 +
//! 64` mbufs, derived at startup from the applied descriptor counts (rings are
//! requested with 4096 descriptors; net_null applies them unchanged). Tests
//! that need a small pool use a small `buffer_capacity`; the size is read back
//! from `/dpds/ports` (`dir<d>_pool_size`) and the startup log.
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
#[cfg(feature = "telemetry")]
const BURST_SIZE: u64 = 32;
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
    config_full(delay_ns, loss, buffer_capacity, false)
}

/// Reordering mode with a constant delay: equal send times must keep the
/// entry order.
fn config_reordering(delay_ns: u64) -> String {
    config_full(delay_ns, 0.0, 4096, true)
}

/// A Uniform-distribution delay with a negative standard deviation
/// (`low > high`, which `Uniform::new` rejects; rand_distr's `Normal` would
/// accept a negative std): invalid parameters must be a clean startup error,
/// not a panic.
fn config_bad_params() -> String {
    config_json(0, 0.0)
        .replace(
            r#""distribution": "DeterministicDistribution""#,
            r#""distribution": "UniformDistribution""#,
        )
        .replace(r#""std_ns": 0.0"#, r#""std_ns": -1000.0"#)
}

/// The message must name the bad parameter (its config name or the
/// distribution it belongs to).
fn assert_names_bad_params(out: &str) {
    assert!(
        out.contains("std") || out.to_lowercase().contains("uniform"),
        "the message names the bad parameter\n{out}"
    );
}

fn config_full(delay_ns: u64, loss: f64, buffer_capacity: usize, reordering: bool) -> String {
    format!(
        r#"{{
    "buffer_capacity": {buffer_capacity},
    "reordering": {reordering},
    "delayer": {{
        "distribution": "DeterministicDistribution",
        "mean_ns": {delay_ns}.0,
        "std_ns": 0.0,
        "activate_correlation": false,
        "half_life_period_ns": 1000000.0,
        "initialization_rate_pps": 15000000.0
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
    /// `Some`: the app args are `--links <file>` instead of `--config <file>`.
    links: Option<PathBuf>,
}

impl Env {
    /// `-l 0-1`: the main lcore plus the worker of the one direction.
    fn new(config: &str) -> Env {
        Env::with_lcores("0-1", config)
    }

    /// `-l 0-2`: the main lcore plus one worker per direction (two ports).
    fn two_ports(config: &str) -> Env {
        Env::with_lcores("0-2", config)
    }

    /// Writes an extra config file next to the default one.
    fn write_config(&self, name: &str, contents: &str) -> PathBuf {
        let p = self.path(name);
        fs::write(&p, contents).unwrap();
        p
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
            links: None,
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
        let mut a = match &self.links {
            Some(l) => vec!["--links".into(), l.display().to_string()],
            None => vec!["--config".into(), self.config.display().to_string()],
        };
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
        "DPDS Stats",
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

/// Waits until direction `dir` has received `n` packets. With telemetry:
/// `/dpds/stats,<dir>` `rx_pkts >= n`. Without: a fixed grace period (pcap
/// files are tiny).
fn wait_rx(run: &mut Run, dir: usize, n: u64) {
    if cfg!(feature = "telemetry") {
        let cmd = format!("/dpds/stats,{dir}");
        run.wait_until(&format!("{cmd} rx_pkts >= {n}"), START_TIMEOUT, |r| {
            r.try_query(&cmd).is_some_and(|v| u(&v, "rx_pkts") >= n)
        });
    } else {
        thread::sleep(Duration::from_millis(2500));
    }
}

/// Waits until the pcap file holds at least `n` frames.
fn wait_frames(run: &mut Run, path: &Path, n: usize) {
    run.wait_until(
        &format!("{n} frames in {}", path.display()),
        START_TIMEOUT,
        |_| read_pcap(path).len() >= n,
    );
}

// ---------------------------------------------------------------------------
// exact-count oracle
// ---------------------------------------------------------------------------

fn exact_count(in_frames: &[Vec<Vec<u8>>]) {
    let ports = in_frames.len();
    require_cpus(ports + 1);
    let _g = serial();
    let cfg = config_json(1_000_000, 0.0);
    let mut env = if ports == 2 {
        Env::two_ports(&cfg)
    } else {
        Env::new(&cfg)
    };
    for (i, f) in in_frames.iter().enumerate() {
        add_pcap_port(&mut env, i, f);
    }
    // Where each input stream must come out: with two ports direction d
    // leaves on port d ^ 1, with one port frames leave on the port they came in on.
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

    // Telemetry oracle, queried BEFORE the SIGINT, per direction.
    #[cfg(feature = "telemetry")]
    {
        for (d, input) in in_frames.iter().enumerate() {
            let n = input.len() as u64;
            let st = run.query(&format!("/dpds/stats,{d}"));
            assert_eq!(u(&st, "rx_pkts"), n, "dir {d}: {st}");
            assert_eq!(u(&st, "pops"), n, "dir {d}: {st}");
            assert_eq!(u(&st, "tx_pkts"), n, "dir {d}: {st}");
            assert_eq!(u(&st, "tx_sent"), n, "dir {d}: {st}");
            assert_eq!(u(&st, "dpds_rejects"), 0, "dir {d}: {st}");
            assert_eq!(u(&st, "tx_drops"), 0, "dir {d}: {st}");
            assert_eq!(u(&st, "tx_short_calls"), 0, "dir {d}: {st}");
            if n > 0 {
                assert!(
                    u(&st, "tx_calls") >= 1 && u(&st, "tx_calls") <= n,
                    "dir {d}: {st}"
                );
                assert!(
                    u(&st, "tx_batch_max") >= 1 && u(&st, "tx_batch_max") <= BURST_SIZE,
                    "dir {d}: {st}"
                );
                let per_call: f64 = s(&st, "tx_pkts_per_call")
                    .parse()
                    .expect("a decimal string");
                assert!((1.0..=BURST_SIZE as f64).contains(&per_call), "{st}");
                assert_eq!(
                    per_call,
                    (u(&st, "tx_pkts") as f64 / u(&st, "tx_calls") as f64 * 100.0).round() / 100.0,
                    "tx_pkts_per_call is the 2-decimal mean: {st}"
                );
            } else {
                assert_eq!(u(&st, "tx_calls"), 0, "dir {d}: {st}");
                assert_eq!(u(&st, "tx_batch_max"), 0, "dir {d}: {st}");
                assert_eq!(s(&st, "tx_pkts_per_call"), "0.00", "dir {d}: {st}");
            }
            assert!(u(&st, "polls") >= 1, "dir {d}: {st}");
            assert!(u(&st, "uptime_ns") > 0, "dir {d}: {st}");
            // Sampled after the pop: never more than what was received.
            assert!(u(&st, "buffer_len_max") <= n, "dir {d}: {st}");
            assert_eq!(s(&st, "first_loss_reason"), "Clean", "dir {d}: {st}");
            assert_eq!(s(&st, "first_loss_detail"), "", "dir {d}: {st}");
            assert_eq!(u(&st, "first_loss_relative_ns"), 0, "dir {d}: {st}");
        }
        // hardware counters of the NICs themselves: nothing lost
        for p in 0..ports {
            let hw = run.query(&format!("/nic/hw_first_loss,{p}"));
            assert_eq!(s(&hw, "hw_first_loss_reason"), "Clean", "{hw}");
        }
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
    // Boundary sizes around BURST_SIZE = 32 (RX burst and pop budget).
    for n in [1u32, 31, 32, 33, 64, 65] {
        exact_count(&[frames(0, n)]);
    }
}

#[test]
fn pcap_two_ports_input_around_the_burst_size() {
    exact_count(&[frames(0, 32), frames(1, 33)]);
    exact_count(&[frames(0, 65), frames(1, 31)]);
}

#[test]
fn pcap_two_ports_one_input_empty() {
    // Port 1 has nothing to send; everything from port 0 must leave on port 1.
    exact_count(&[frames(0, 300), Vec::new()]);
    exact_count(&[Vec::new(), frames(1, 300)]);
}

// ---------------------------------------------------------------------------
// per-direction configs (two net_pcap ports, `-l 0-2`)
// ---------------------------------------------------------------------------

/// Two pcap ports; `c0`/`c1` are passed as `--config0`/`--config1` when given,
/// `base` is the plain `--config`.
fn dir_env(
    base: &str,
    c0: Option<&str>,
    c1: Option<&str>,
    in0: &[Vec<u8>],
    in1: &[Vec<u8>],
) -> Env {
    let mut env = Env::two_ports(base);
    add_pcap_port(&mut env, 0, in0);
    add_pcap_port(&mut env, 1, in1);
    for (flag, name, c) in [
        ("--config0", "cfg0.json", c0),
        ("--config1", "cfg1.json", c1),
    ] {
        if let Some(c) = c {
            let p = env.write_config(name, c);
            env.extra_app_args
                .extend([flag.to_string(), p.display().to_string()]);
        }
    }
    env
}

#[test]
fn dir0_loses_everything_dir1_is_lossless() {
    const N0: u32 = 300;
    const N1: u32 = 500;
    require_cpus(3);
    let _g = serial();
    let env = dir_env(
        &config_json(0, 0.0),
        Some(&config_json(0, 1.0)),
        Some(&config_json(0, 0.0)),
        &frames(0, N0),
        &frames(1, N1),
    );
    let (out0, out1) = (env.path("out0.pcap"), env.path("out1.pcap"));
    let mut run = env.spawn();

    // direction 1 (rx port 1) leaves on port 0
    wait_frames(&mut run, &out0, N1 as usize);
    wait_rx(&mut run, 0, N0 as u64);
    thread::sleep(Duration::from_millis(700));

    #[cfg(feature = "telemetry")]
    {
        let d0 = run.query("/dpds/stats,0");
        assert_eq!(u(&d0, "rx_pkts"), N0 as u64, "{d0}");
        assert_eq!(u(&d0, "dpds_rejects"), N0 as u64, "{d0}");
        assert_eq!(u(&d0, "pops"), 0, "{d0}");
        assert_eq!(u(&d0, "tx_sent"), 0, "{d0}");
        assert_eq!(u(&d0, "tx_calls"), 0, "{d0}");
        assert_eq!(s(&d0, "first_loss_reason"), "DpdsReject", "{d0}");
        let d1 = run.query("/dpds/stats,1");
        assert_eq!(u(&d1, "rx_pkts"), N1 as u64, "{d1}");
        assert_eq!(u(&d1, "dpds_rejects"), 0, "{d1}");
        assert_eq!(u(&d1, "tx_sent"), N1 as u64, "{d1}");
        assert_eq!(u(&d1, "tx_pkts"), N1 as u64, "{d1}");
        assert_eq!(s(&d1, "first_loss_reason"), "Clean", "{d1}");
    }

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert_eq!(
        read_pcap(&out1).len(),
        0,
        "direction 0 loses everything: nothing leaves on port 1\n{}",
        run.output()
    );
    assert!(
        read_pcap(&out0) == frames(1, N1),
        "port 0 carries exactly direction 1's frames, in order"
    );
}

#[test]
fn config_d_overrides_config_per_direction_and_the_other_direction_uses_config() {
    const N0: u32 = 200;
    const N1: u32 = 350;
    require_cpus(3);
    let _g = serial();
    // --config = 100 % loss for everybody, --config0 = lossless for dir 0 only.
    let env = dir_env(
        &config_json(0, 1.0),
        Some(&config_json(0, 0.0)),
        None,
        &frames(0, N0),
        &frames(1, N1),
    );
    let (out0, out1) = (env.path("out0.pcap"), env.path("out1.pcap"));
    let mut run = env.spawn();
    wait_frames(&mut run, &out1, N0 as usize);
    wait_rx(&mut run, 1, N1 as u64);
    thread::sleep(Duration::from_millis(700));

    #[cfg(feature = "telemetry")]
    {
        let d1 = run.query("/dpds/stats,1");
        assert_eq!(u(&d1, "dpds_rejects"), N1 as u64, "{d1}");
        let d0 = run.query("/dpds/stats,0");
        assert_eq!(u(&d0, "tx_sent"), N0 as u64, "{d0}");
    }

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(read_pcap(&out1) == frames(0, N0), "direction 0 is lossless");
    assert_eq!(
        read_pcap(&out0).len(),
        0,
        "direction 1 uses --config (loss)"
    );
}

#[test]
fn each_direction_has_its_own_delay_and_drains_it_at_sigint() {
    const N0: u32 = 200;
    const N1: u32 = 300;
    require_cpus(3);
    let _g = serial();
    // dir 0 passes through, dir 1 holds everything for 10 s: 4x the
    // telemetry-off grace period of `wait_rx` (2.5 s), so a loaded host has 7.5 s
    // of slack before anything of dir 1 is due. Not longer: the SIGINT drain
    // still honours the delay (a 60 s delay overran EXIT_TIMEOUT, 30 s).
    let env = dir_env(
        &config_json(0, 0.0),
        Some(&config_json(0, 0.0)),
        Some(&config_json(10_000_000_000, 0.0)),
        &frames(0, N0),
        &frames(1, N1),
    );
    let (out0, out1) = (env.path("out0.pcap"), env.path("out1.pcap"));
    let mut run = env.spawn();
    wait_frames(&mut run, &out1, N0 as usize);
    wait_rx(&mut run, 1, N1 as u64);
    assert_eq!(
        read_pcap(&out0).len(),
        0,
        "direction 1's packets left before their 10 s delay elapsed"
    );
    #[cfg(feature = "telemetry")]
    {
        let d1 = run.query("/dpds/stats,1");
        assert_eq!(u(&d1, "tx_sent"), 0, "{d1}");
        assert_eq!(u(&d1, "buffer_len_max"), N1 as u64, "{d1}");
        let d0 = run.query("/dpds/stats,0");
        assert_eq!(u(&d0, "tx_sent"), N0 as u64, "{d0}");
        assert_eq!(u(&d0, "buffer_len_max"), 0, "{d0}");
    }
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(
        read_pcap(&out0) == frames(1, N1),
        "shutdown lost or reordered the delayed packets of direction 1"
    );
    assert!(read_pcap(&out1) == frames(0, N0));
}

#[test]
fn reordering_mode_with_a_constant_delay_keeps_the_frame_order() {
    // Frames are numbered in their payload (bytes 14..18). Direction 0 runs
    // in reordering mode, direction 1 in FIFO mode.
    const N0: u32 = 2000;
    const N1: u32 = 1500;
    require_cpus(3);
    let _g = serial();
    let env = dir_env(
        &config_json(0, 0.0),
        Some(&config_reordering(1_000_000)),
        Some(&config_json(1_000_000, 0.0)),
        &frames(0, N0),
        &frames(1, N1),
    );
    let (out0, out1) = (env.path("out0.pcap"), env.path("out1.pcap"));
    let mut run = env.spawn();
    wait_frames(&mut run, &out1, N0 as usize);
    wait_frames(&mut run, &out0, N1 as usize);
    thread::sleep(Duration::from_millis(700));
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);

    let got = read_pcap(&out1);
    assert_eq!(got.len(), N0 as usize, "reordering direction: exact count");
    let seqs: Vec<u32> = got
        .iter()
        .map(|f| u32::from_be_bytes(f[14..18].try_into().unwrap()))
        .collect();
    assert_eq!(
        seqs,
        (0..N0).collect::<Vec<_>>(),
        "reordering mode with a constant delay must keep the entry order"
    );
    assert!(got == frames(0, N0), "and the frames are intact");
    assert!(read_pcap(&out0) == frames(1, N1));
}

// ---------------------------------------------------------------------------
// loss
// ---------------------------------------------------------------------------

#[test]
fn full_loss_transmits_nothing_and_counts_dpds_rejects() {
    const N: u32 = 500;
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 1.0));
    add_pcap_port(&mut env, 0, &frames(0, N));
    let mut run = env.spawn();

    wait_rx(&mut run, 0, N as u64);

    #[cfg(feature = "telemetry")]
    {
        let st = run.query("/dpds/stats,0");
        assert_eq!(u(&st, "rx_pkts"), N as u64, "{st}");
        assert_eq!(u(&st, "dpds_rejects"), N as u64, "{st}");
        assert_eq!(u(&st, "pops"), 0, "{st}");
        assert_eq!(u(&st, "tx_sent"), 0, "{st}");
        assert_eq!(u(&st, "tx_calls"), 0, "{st}");
        assert_eq!(s(&st, "first_loss_reason"), "DpdsReject", "{st}");
        // the detail names the cause of the first reject: the loss model here
        assert_eq!(s(&st, "first_loss_detail"), "from=dpds,cause=loss", "{st}");
        assert_eq!(u(&st, "loss_drops"), N as u64, "{st}");
        assert_eq!(u(&st, "limiter_drops"), 0, "{st}");
        assert_eq!(u(&st, "buffer_full_drops"), 0, "{st}");
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

    wait_rx(&mut run, 0, N as u64);
    assert_eq!(
        read_pcap(&env.path("out0.pcap")).len(),
        0,
        "packets left before their delay elapsed"
    );
    #[cfg(feature = "telemetry")]
    {
        let st = run.query("/dpds/stats,0");
        assert_eq!(u(&st, "tx_sent"), 0, "{st}");
        assert_eq!(u(&st, "dpds_rejects"), 0, "{st}");
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

#[test]
fn sigterm_drains_like_sigint() {
    const N: u32 = 100;
    require_cpus(3);
    let _g = serial();
    let env = dir_env(
        &config_json(2_000_000_000, 0.0),
        None,
        None,
        &frames(0, N),
        &frames(1, N),
    );
    let (out0, out1) = (env.path("out0.pcap"), env.path("out1.pcap"));
    let mut run = env.spawn();
    wait_rx(&mut run, 0, N as u64);
    wait_rx(&mut run, 1, N as u64);
    run.signal(libc::SIGTERM);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(read_pcap(&out1) == frames(0, N));
    assert!(read_pcap(&out0) == frames(1, N));
}

// ---------------------------------------------------------------------------
// net_null smoke tests (line-rate packet source, packets are just freed)
// ---------------------------------------------------------------------------

fn null_smoke(ports: usize, sig: i32) {
    require_cpus(ports + 1);
    let _g = serial();
    let cfg = config_json(0, 0.0);
    let mut env = if ports == 2 {
        Env::two_ports(&cfg)
    } else {
        Env::new(&cfg)
    };
    for p in 0..ports {
        env.vdevs.push(format!("net_null{p}"));
    }
    let mut run = env.spawn();

    #[cfg(feature = "telemetry")]
    for d in 0..ports {
        // Traffic is flowing and keeps growing in every direction.
        let cmd = format!("/dpds/stats,{d}");
        run.wait_until("rx traffic", START_TIMEOUT, |r| {
            r.try_query(&cmd).is_some_and(|v| u(&v, "rx_pkts") > 0)
        });
        let first = u(&run.query(&cmd), "rx_pkts");
        run.wait_until("rx_pkts to keep growing", START_TIMEOUT, |r| {
            r.try_query(&cmd).is_some_and(|v| u(&v, "rx_pkts") > first)
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

    /// A query that must not produce a valid reply: no reply at all, or a
    /// reply without `key` (an empty dict), like `/nic/epochs,<bad port>`.
    fn assert_error_reply(run: &Run, command: &str, key: &str) {
        assert!(
            run.try_query(command).is_none_or(|v| v.get(key).is_none()),
            "{command} must be an error reply"
        );
    }

    /// The pool size derived for direction `dir` of a `/dpds/ports`
    /// reply: `buffer_capacity + rx ring of the RX port + tx ring of the TX
    /// port + 512 * 3 / 2 + 2 * 32` (applied ring sizes as reported).
    fn expected_pool(p: &Value, dir: usize, buffer_capacity: u64) -> u64 {
        let rx_port = u(p, &format!("dir{dir}_rx_port"));
        let tx_port = u(p, &format!("dir{dir}_tx_port"));
        buffer_capacity
            + u(p, &format!("port{rx_port}_rx_ring"))
            + u(p, &format!("port{tx_port}_tx_ring"))
            + 768
            + 64
    }

    #[test]
    fn the_commands_are_registered_next_to_dpdks_own() {
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(0, 0.0));
        env.vdevs.push("net_null0".into());
        let mut run = env.spawn();
        // registered at startup, before the workers are launched
        run.query("/dpds/stats,0");
        let list = run.command_list();
        for c in [
            "/dpds/stats",
            "/dpds/epochs",
            "/dpds/delay_hist",
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
    fn ports_reports_names_ring_sizes_and_the_directions_of_two_ports() {
        require_cpus(3);
        let _g = serial();
        let mut env = Env::two_ports(&config_json(0, 0.0));
        add_pcap_port(&mut env, 0, &frames(0, 4));
        add_pcap_port(&mut env, 1, &frames(1, 4));
        let cfg0 = env.write_config("cfg0.json", &config_json(0, 0.0));
        let cfg1 = env.write_config("cfg1.json", &config_json(0, 0.0));
        env.extra_app_args = vec![
            "--config0".into(),
            cfg0.display().to_string(),
            "--config1".into(),
            cfg1.display().to_string(),
        ];
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
            assert_eq!(u(&p, &format!("port{i}_rx_ring_requested")), 4096, "{p}");
            assert_eq!(u(&p, &format!("port{i}_tx_ring_requested")), 4096, "{p}");
            assert!(
                rx <= 4096 && tx <= 4096,
                "applied never exceeds requested: {p}"
            );
        }
        assert!(p.get("port2_name").is_none(), "{p}");

        assert_eq!(u(&p, "num_directions"), 2, "{p}");
        // the --config* shorthand with two ports is two links, `dir0`
        // (p0 -> p1, config0) and `dir1` (p1 -> p0, config1)
        assert_eq!(u(&p, "num_links"), 2, "{p}");
        for d in 0..2 {
            assert_eq!(s(&p, &format!("dir{d}_link")), format!("dir{d}"), "{p}");
            assert_eq!(
                s(&p, &format!("dir{d}_rx_name")),
                format!("net_pcap{d}"),
                "{p}"
            );
            assert_eq!(
                s(&p, &format!("dir{d}_tx_name")),
                format!("net_pcap{}", d ^ 1),
                "{p}"
            );
        }
        // direction d receives on port d and leaves on port d ^ 1
        assert_eq!(u(&p, "dir0_rx_port"), 0, "{p}");
        assert_eq!(u(&p, "dir0_tx_port"), 1, "{p}");
        assert_eq!(u(&p, "dir1_rx_port"), 1, "{p}");
        assert_eq!(u(&p, "dir1_tx_port"), 0, "{p}");
        // the i-th EAL worker lcore runs direction i: `-l 0-2` -> 1, 2
        assert_eq!(u(&p, "dir0_lcore"), 1, "{p}");
        assert_eq!(u(&p, "dir1_lcore"), 2, "{p}");
        assert_eq!(s(&p, "dir0_config"), cfg0.display().to_string(), "{p}");
        assert_eq!(s(&p, "dir1_config"), cfg1.display().to_string(), "{p}");
        // both configs have buffer_capacity 4096: the derived pool size
        assert_eq!(u(&p, "dir0_pool_size"), expected_pool(&p, 0, 4096), "{p}");
        assert_eq!(u(&p, "dir1_pool_size"), expected_pool(&p, 1, 4096), "{p}");
        // net_pcap has no MBUF_FAST_FREE offload: pktmbuf bulk free
        assert_eq!(u(&p, "dir0_raw_free"), 0, "{p}");
        assert_eq!(u(&p, "dir1_raw_free"), 0, "{p}");
        assert!(p.get("dir2_rx_port").is_none(), "{p}");
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn each_directions_pool_is_derived_from_its_own_capacity_and_rings() {
        require_cpus(3);
        let _g = serial();
        let mut env = Env::two_ports(&config_json_cap(0, 0.0, 1000));
        env.vdevs.push("net_null0".into());
        env.vdevs.push("net_null1".into());
        let cfg0 = env.write_config("cfg0.json", &config_json_cap(0, 0.0, 1000));
        let cfg1 = env.write_config("cfg1.json", &config_json_cap(0, 0.0, 3000));
        env.extra_app_args = vec![
            "--config0".into(),
            cfg0.display().to_string(),
            "--config1".into(),
            cfg1.display().to_string(),
        ];
        let mut run = env.spawn();
        let p = run.query("/dpds/ports");
        let pool0 = u(&p, "dir0_pool_size");
        let pool1 = u(&p, "dir1_pool_size");
        assert_eq!(pool0, expected_pool(&p, 0, 1000), "{p}");
        assert_eq!(pool1, expected_pool(&p, 1, 3000), "{p}");
        // different buffer_capacity, same rings: the pools differ by exactly 2000
        assert_eq!(pool1 - pool0, 2000, "{p}");
        // net_null accepts the requested 4096 descriptors unchanged
        assert_eq!(pool0, 1000 + 4096 + 4096 + 768 + 64, "{p}");
        assert_eq!(pool1, 3000 + 4096 + 4096 + 768 + 64, "{p}");
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn the_pool_size_does_not_depend_on_the_other_directions_config() {
        // Same rings, swapped capacities: each direction follows its own.
        require_cpus(3);
        let _g = serial();
        let mut env = Env::two_ports(&config_json_cap(0, 0.0, 3000));
        env.vdevs.push("net_null0".into());
        env.vdevs.push("net_null1".into());
        let cfg1 = env.write_config("cfg1.json", &config_json_cap(0, 0.0, 500));
        env.extra_app_args = vec!["--config1".into(), cfg1.display().to_string()];
        let mut run = env.spawn();
        let p = run.query("/dpds/ports");
        assert_eq!(u(&p, "dir0_pool_size"), expected_pool(&p, 0, 3000), "{p}");
        assert_eq!(u(&p, "dir1_pool_size"), expected_pool(&p, 1, 500), "{p}");
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn ports_reports_the_single_direction_of_one_port() {
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(0, 0.0));
        env.vdevs.push("net_null0".into());
        let mut run = env.spawn();
        let p = run.query("/dpds/ports");
        assert_eq!(u(&p, "num_ports"), 1, "{p}");
        assert_eq!(s(&p, "port0_name"), "net_null0", "{p}");
        assert!(p.get("port1_name").is_none(), "{p}");
        assert_eq!(u(&p, "num_directions"), 1, "{p}");
        assert_eq!(u(&p, "num_links"), 1, "{p}");
        // the shorthand with one port is the single hairpin link `dir0`
        assert_eq!(s(&p, "dir0_link"), "dir0", "{p}");
        assert_eq!(s(&p, "dir0_rx_name"), "net_null0", "{p}");
        assert_eq!(s(&p, "dir0_tx_name"), "net_null0", "{p}");
        assert_eq!(u(&p, "dir0_rx_port"), 0, "{p}");
        assert_eq!(u(&p, "dir0_tx_port"), 0, "one port: loopback {p}");
        assert_eq!(u(&p, "dir0_lcore"), 1, "{p}");
        assert_eq!(
            s(&p, "dir0_config"),
            env.config.display().to_string(),
            "{p}"
        );
        // one port: RX and TX ring of port 0, buffer_capacity 4096
        assert_eq!(u(&p, "dir0_pool_size"), expected_pool(&p, 0, 4096), "{p}");
        assert!(u(&p, "dir0_raw_free") <= 1, "{p}");
        assert!(p.get("dir1_rx_port").is_none(), "{p}");
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn stats_and_epochs_take_a_direction_parameter_and_reject_bad_ones() {
        const N0: u32 = 250;
        const N1: u32 = 400;
        require_cpus(3);
        let _g = serial();
        let env = dir_env(
            &config_json(1_000_000, 0.0),
            None,
            None,
            &frames(0, N0),
            &frames(1, N1),
        );
        let mut run = env.spawn();
        wait_rx(&mut run, 0, N0 as u64);
        wait_rx(&mut run, 1, N1 as u64);

        // each direction has its own stats ...
        let d0 = run.query("/dpds/stats,0");
        let d1 = run.query("/dpds/stats,1");
        assert_eq!(u(&d0, "rx_pkts"), N0 as u64, "{d0}");
        assert_eq!(u(&d1, "rx_pkts"), N1 as u64, "{d1}");
        for st in [&d0, &d1] {
            for k in [
                "uptime_ns",
                "polls",
                "rx_pkts",
                "rx_full_bursts",
                "dpds_rejects",
                "pops",
                "tx_calls",
                "tx_pkts",
                "tx_sent",
                "tx_drops",
                "tx_short_calls",
                "tx_batch_max",
                "tx_pkts_per_call",
                "buffer_len_max",
                "first_loss_reason",
                "first_loss_detail",
                "first_loss_relative_ns",
                "loss_drops",
                "limiter_drops",
                "buffer_full_drops",
                "delay_mean_ns",
                "delay_std_ns",
                "delay_min_ns",
                "delay_max_ns",
                "lateness_mean_ns",
                "lateness_max_ns",
            ] {
                assert!(st.get(k).is_some(), "{k} missing: {st}");
            }
        }

        // ... and its own epochs (an idle gap > 500 ms after traffic closes one)
        for (d, n) in [(0, N0), (1, N1)] {
            let cmd = format!("/dpds/epochs,{d}");
            run.wait_until("one closed epoch", START_TIMEOUT, |r| {
                r.try_query(&cmd)
                    .is_some_and(|v| u(&v, "epochs_closed") >= 1)
            });
            let ep = run.query(&cmd);
            assert_eq!(arr_u(&ep, "rx_pkts").iter().sum::<u64>(), n as u64, "{ep}");
            assert_eq!(arr_u(&ep, "tx_sent").iter().sum::<u64>(), n as u64, "{ep}");
        }

        // missing or invalid direction: an error reply
        for bad in ["", ",2", ",x", ",-1", ",", ",0,1", ",01x"] {
            assert_error_reply(&run, &format!("/dpds/stats{bad}"), "rx_pkts");
            assert_error_reply(&run, &format!("/dpds/epochs{bad}"), "epochs_closed");
        }
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn with_one_port_only_direction_0_exists() {
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(0, 0.0));
        add_pcap_port(&mut env, 0, &frames(0, 10));
        let mut run = env.spawn();
        run.query("/dpds/stats,0");
        run.query("/dpds/epochs,0");
        assert_error_reply(&run, "/dpds/stats,1", "rx_pkts");
        assert_error_reply(&run, "/dpds/epochs,1", "epochs_closed");
        assert_error_reply(&run, "/dpds/stats", "rx_pkts");
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

        wait_rx(&mut run, 0, N as u64);
        // pcap EOF = no more arrivals; > 500 ms later the epoch is closed
        run.wait_until("one closed epoch", START_TIMEOUT, |r| {
            r.try_query("/dpds/epochs,0")
                .is_some_and(|v| u(&v, "epochs_closed") >= 1)
        });
        // ... and stays at one: idle ticks after a close emit no empty epochs
        thread::sleep(Duration::from_millis(1500));
        let ep = run.query("/dpds/epochs,0");

        assert_eq!(u(&ep, "epochs_closed"), 1, "{ep}");
        assert_eq!(u(&ep, "epochs_dropped"), 0, "{ep}");
        for k in [
            "epoch_start_uptime_ns",
            "epoch_end_uptime_ns",
            "first_loss_relative_ns",
            "polls",
            "rx_pkts",
            "rx_full_bursts",
            "dpds_rejects",
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
        assert_eq!(arr_u(&ep, "dpds_rejects"), [0], "{ep}");
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
        let st = run.query("/dpds/stats,0");
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
        run.query("/dpds/epochs,0");
        thread::sleep(Duration::from_millis(1800));
        let ep = run.query("/dpds/epochs,0");
        assert_eq!(
            u(&ep, "epochs_closed"),
            0,
            "an idle start must not emit empty epochs: {ep}"
        );
        assert_eq!(arr_u(&ep, "rx_pkts"), Vec::<u64>::new(), "{ep}");
        let st = run.query("/dpds/stats,0");
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
        wait_rx(&mut run, 0, N as u64);

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

    // ── mbuf leaks: the derived pool is small, it must not run dry ──────

    /// net_null produces packets as fast as it is polled. The pool of each
    /// direction is derived from its (small) `buffer_capacity`, so the loop
    /// only keeps running if every mbuf is returned: `rx_pkts` far beyond the
    /// pool size and `rx_nombuf == 0`. `configs` = the config of each
    /// direction (`--config<d>`), one net_null port per config.
    fn no_leak(configs: &[String], expect_rejects: bool) {
        let ports = configs.len();
        require_cpus(ports + 1);
        let _g = serial();
        let mut env = if ports == 2 {
            Env::two_ports(&configs[0])
        } else {
            Env::new(&configs[0])
        };
        for p in 0..ports {
            env.vdevs.push(format!("net_null{p}"));
        }
        for (d, c) in configs.iter().enumerate() {
            let path = env.write_config(&format!("cfg{d}.json"), c);
            env.extra_app_args
                .extend([format!("--config{d}"), path.display().to_string()]);
        }
        let mut run = env.spawn();

        // The derived pool size of each direction (all configs here use a
        // buffer_capacity of at most 4096, so a pool is at most ~13k mbufs).
        let ports_reply = run.query("/dpds/ports");
        let pools: Vec<u64> = (0..ports)
            .map(|d| u(&ports_reply, &format!("dir{d}_pool_size")))
            .collect();
        for (d, pool) in pools.iter().enumerate() {
            assert!(
                (1..=20_000).contains(pool),
                "dir {d}: the derived pool must be small, got {pool}: {ports_reply}"
            );
        }

        for (d, pool) in pools.iter().enumerate() {
            let cmd = format!("/dpds/stats,{d}");
            run.wait_until("rx_pkts far above the pool size", START_TIMEOUT, |r| {
                r.try_query(&cmd)
                    .is_some_and(|v| u(&v, "rx_pkts") > 20 * pool)
            });
        }
        // run for a few seconds
        thread::sleep(Duration::from_secs(3));
        for (d, pool) in pools.iter().enumerate() {
            let st = run.query(&format!("/dpds/stats,{d}"));
            assert!(u(&st, "rx_pkts") > 40 * pool, "dir {d}: {st}");
            if expect_rejects {
                assert!(
                    u(&st, "dpds_rejects") > 0,
                    "dir {d}: the loss/buffer must reject: {st}"
                );
                assert_eq!(s(&st, "first_loss_reason"), "DpdsReject", "{st}");
            }
            let eth = run.query(&format!("/ethdev/stats,{d}"));
            assert_eq!(
                u(&eth, "rx_nombuf"),
                0,
                "an mbuf leak starves the pool: {eth}"
            );
            let hw = run.query(&format!("/nic/hw_first_loss,{d}"));
            assert_ne!(s(&hw, "hw_first_loss_reason"), "RX_NoMbuf", "{hw}");
        }
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }

    #[test]
    fn full_loss_on_a_small_pool_does_not_leak_mbufs() {
        // capacity 4096: pool 4096 + 4096 + 4096 + 768 + 64 = 13120
        no_leak(&[config_json(0, 1.0)], true);
    }

    #[test]
    fn delay_with_a_tiny_buffer_does_not_leak_mbufs() {
        // buffer-full rejects are freed too (capacity 64: pool 9088)
        no_leak(&[config_json_cap(1_000_000, 0.0, 64)], true);
    }

    #[test]
    fn plain_forwarding_on_a_small_pool_does_not_leak_mbufs() {
        no_leak(&[config_json(0, 0.0)], false);
    }

    #[test]
    fn the_derived_pool_never_runs_dry_with_a_full_buffer() {
        // A 10 ms delay at line rate keeps the buffer (4096) full with rejects:
        // the worst case for a pool that is exactly 4096 + rings + slack.
        no_leak(&[config_json(10_000_000, 0.0)], true);
    }

    #[test]
    fn two_directions_with_different_configs_do_not_leak_mbufs() {
        // dir 0: everything lost (rejects freed in bulk), dir 1: delay with a
        // small buffer (rejects and a full TX path), one pool each.
        no_leak(
            &[config_json(0, 1.0), config_json_cap(1_000_000, 0.0, 64)],
            true,
        );
    }

    #[test]
    fn the_stats_are_consistent_with_each_other_under_load() {
        require_cpus(2);
        let _g = serial();
        let mut env = Env::new(&config_json(0, 0.5));
        env.vdevs.push("net_null0".into());
        let mut run = env.spawn();
        run.wait_until("traffic", START_TIMEOUT, |r| {
            r.try_query("/dpds/stats,0")
                .is_some_and(|v| u(&v, "rx_pkts") > 100_000)
        });
        let st = run.query("/dpds/stats,0");
        // read at slightly different moments by the telemetry thread, so only
        // inequalities that hold at any moment: nothing sent that was not received
        assert!(
            u(&st, "pops") + u(&st, "dpds_rejects") <= u(&st, "rx_pkts") + BURST_SIZE,
            "{st}"
        );
        assert!(u(&st, "tx_sent") <= u(&st, "tx_pkts"), "{st}");
        assert_eq!(
            u(&st, "tx_drops"),
            u(&st, "tx_pkts").saturating_sub(u(&st, "tx_sent")),
            "{st}"
        );
        assert!(u(&st, "rx_full_bursts") <= u(&st, "polls"), "{st}");
        assert!(u(&st, "dpds_rejects") > 0, "50 % loss: {st}");
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
    assert_ne!(code, 101, "a Rust panic is not a clean error exit\n{out}");
    assert!(
        !out.contains("panicked at"),
        "a panic is not a clean error exit\n{out}"
    );
    out
}

fn expect_failure(env: &Env) -> String {
    expect_failure_args(env.args(), env.xdg.path())
}

/// The lcore-count error counts LINKS (for `--links` and for the `--config*`
/// shorthand, one link per port): "<n> link(s) need <n+1> lcores (main + one
/// worker per link), e.g. -l 0-<n>", plus the lcore count found. The grammatical
/// number of the head is free ("1 link needs" / "1 link(s) need").
fn assert_lcore_requirement(out: &str, links: usize, found: usize) {
    let lcores = links + 1;
    let heads = [
        format!("{links} link needs {lcores} lcores"),
        format!("{links} links need {lcores} lcores"),
        format!("{links} link(s) need {lcores} lcores"),
    ];
    assert!(
        heads.iter().any(|h| out.contains(h)),
        "the requirement counts {links} link(s)\n{out}"
    );
    let tail = format!("lcores (main + one worker per link), e.g. -l 0-{links}");
    assert!(out.contains(&tail), "{tail:?} is named\n{out}");
    assert!(
        !out.contains(" port needs") && !out.contains(" ports need"),
        "the requirement no longer counts ports\n{out}"
    );
    assert!(
        out.contains(&found.to_string()),
        "the lcore count found ({found}) is named\n{out}"
    );
}

#[test]
fn one_port_with_one_lcore_is_rejected_naming_the_requirement() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::with_lcores("0", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert_lcore_requirement(&out, 1, 1);
}

#[test]
fn one_port_with_three_lcores_is_rejected_there_is_one_worker_per_port() {
    require_cpus(3);
    let _g = serial();
    let mut env = Env::with_lcores("0-2", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert_lcore_requirement(&out, 1, 3);
}

#[test]
fn two_ports_with_two_lcores_are_rejected_naming_the_requirement() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::with_lcores("0-1", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let out = expect_failure(&env);
    assert_lcore_requirement(&out, 2, 2);
}

#[test]
fn two_ports_with_four_lcores_are_rejected_too() {
    require_cpus(4);
    let _g = serial();
    let mut env = Env::with_lcores("0-3", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let out = expect_failure(&env);
    assert_lcore_requirement(&out, 2, 4);
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
fn a_flag_given_twice_is_rejected_with_usage() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    // the harness already passes --config
    env.extra_app_args = vec!["--config".into(), env.config.display().to_string()];
    let out = expect_failure(&env);
    assert!(out.contains("Usage"), "usage message expected\n{out}");
    assert!(out.contains("--config"), "{out}");
}

#[test]
fn config1_with_one_port_is_rejected() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    env.extra_app_args = vec!["--config1".into(), env.config.display().to_string()];
    let out = expect_failure(&env);
    assert!(out.contains("--config1"), "{out}");
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

// ── initialization_rate_pps: required in every config, validated at startup ──

const RATE_LINE: &str = r#""initialization_rate_pps": 15000000.0"#;

/// `config_json` without the rate (the field is required, no default).
fn config_without_rate() -> String {
    let cfg = config_json(0, 0.0);
    assert!(cfg.contains(RATE_LINE), "the e2e config carries the rate");
    let without = cfg.replace(&format!(",\n        {RATE_LINE}"), "");
    assert!(
        !without.contains("initialization_rate_pps"),
        "test setup: rate removed"
    );
    without
}

#[test]
fn a_config_without_initialization_rate_pps_fails_startup_naming_the_field() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_without_rate());
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert!(out.contains("initialization_rate_pps"), "{out}");
}

#[test]
fn a_config_of_direction_1_without_initialization_rate_pps_fails_startup() {
    require_cpus(3);
    let _g = serial();
    let mut env = Env::two_ports(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let bad = env.write_config("cfg1.json", &config_without_rate());
    env.extra_app_args = vec!["--config1".into(), bad.display().to_string()];
    let out = expect_failure(&env);
    assert!(out.contains("initialization_rate_pps"), "{out}");
}

#[test]
fn an_invalid_initialization_rate_fails_startup_naming_the_field() {
    require_cpus(2);
    let _g = serial();
    for bad in ["0.0", "0.5", "-15000000.0"] {
        let json =
            config_json(0, 0.0).replace(RATE_LINE, &format!(r#""initialization_rate_pps": {bad}"#));
        assert!(json.contains(bad), "test setup");
        let mut env = Env::new(&json);
        env.vdevs.push("net_null0".into());
        let out = expect_failure(&env);
        assert!(out.contains("initialization_rate_pps"), "rate {bad}\n{out}");
    }
}

#[test]
fn invalid_distribution_parameters_are_a_clean_error_naming_the_parameter() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_bad_params());
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert_names_bad_params(&out);
}

#[test]
fn a_bad_config_of_direction_1_fails_startup() {
    require_cpus(3);
    let _g = serial();
    let mut env = Env::two_ports(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let bad = env.write_config("cfg1.json", &config_bad_params());
    env.extra_app_args = vec!["--config1".into(), bad.display().to_string()];
    let out = expect_failure(&env);
    assert_names_bad_params(&out);
}

#[test]
fn a_missing_config_file_of_direction_1_fails_startup() {
    require_cpus(3);
    let _g = serial();
    let mut env = Env::two_ports(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let missing = env.path("does-not-exist.json");
    env.extra_app_args = vec!["--config1".into(), missing.display().to_string()];
    let out = expect_failure(&env);
    assert!(out.contains("does-not-exist.json"), "{out}");
}

// ── pool size (N): derived, no --num-mbufs ────────────────────────────

#[test]
fn num_mbufs_is_an_unknown_flag_and_a_usage_error() {
    require_cpus(2);
    let _g = serial();
    for value in [Some("16384"), Some("100"), None] {
        let mut env = Env::new(&config_json(0, 0.0));
        env.vdevs.push("net_null0".into());
        env.extra_app_args = std::iter::once("--num-mbufs".to_string())
            .chain(value.map(String::from))
            .collect();
        let out = expect_failure(&env);
        assert!(
            out.contains("--num-mbufs"),
            "{value:?}: names the flag\n{out}"
        );
        assert!(out.contains("Usage"), "{value:?}: prints the usage\n{out}");
        assert!(
            out.contains("unknown"),
            "{value:?}: it is an unknown argument\n{out}"
        );
        // the usage text does not advertise the removed option any more
        let usage = out.lines().find(|l| l.contains("Usage")).unwrap();
        assert!(!usage.contains("num-mbufs"), "{usage}");
    }
}

#[test]
fn the_startup_log_shows_the_derived_pool_size_of_a_direction() {
    require_cpus(2);
    let _g = serial();
    // net_null applies the requested 4096 descriptors:
    // 1000 + 4096 + 4096 + 768 + 64 = 10024 mbufs
    let mut env = Env::new(&config_json_cap(0, 0.0, 1000));
    env.vdevs.push("net_null0".into());
    let mut run = env.spawn();
    run.wait_until("the direction 0 startup line", START_TIMEOUT, |r| {
        r.output().lines().any(|l| l.contains("Direction 0"))
    });
    let out = run.output();
    let line = out.lines().find(|l| l.contains("Direction 0")).unwrap();
    assert!(line.contains("10024"), "derived size missing: {line}");
    assert!(line.contains("dpds_pool_0"), "{line}");
    assert!(!out.contains("too small"), "{out}");
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
}

#[test]
fn the_startup_log_shows_each_directions_own_pool_size() {
    require_cpus(3);
    let _g = serial();
    let mut env = Env::two_ports(&config_json_cap(0, 0.0, 1000));
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let cfg1 = env.write_config("cfg1.json", &config_json_cap(0, 0.0, 3000));
    env.extra_app_args = vec!["--config1".into(), cfg1.display().to_string()];
    let mut run = env.spawn();
    run.wait_until("both direction startup lines", START_TIMEOUT, |r| {
        let o = r.output();
        o.lines().any(|l| l.contains("Direction 0")) && o.lines().any(|l| l.contains("Direction 1"))
    });
    let out = run.output();
    let line = |d: usize| {
        out.lines()
            .find(|l| l.contains(&format!("Direction {d}")))
            .unwrap()
            .to_string()
    };
    assert!(line(0).contains("10024"), "{}", line(0)); // 1000 + 8192 + 832
    assert!(line(1).contains("12024"), "{}", line(1)); // 3000 + 8192 + 832
    assert!(line(0).contains("dpds_pool_0"), "{}", line(0));
    assert!(line(1).contains("dpds_pool_1"), "{}", line(1));
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
}

#[test]
fn a_small_buffer_capacity_gives_a_small_pool_that_starts_fine() {
    // The old way to get a small pool was `--num-mbufs`; now buffer_capacity 0
    // (everything is rejected) derives 0 + 4096 + 4096 + 768 + 64 = 9024.
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json_cap(0, 0.0, 0));
    env.vdevs.push("net_null0".into());
    let mut run = env.spawn();
    run.wait_until("the direction 0 startup line", START_TIMEOUT, |r| {
        r.output().lines().any(|l| l.contains("Direction 0"))
    });
    thread::sleep(Duration::from_millis(1500));
    assert!(run.poll_exit().is_none(), "{}", run.output());
    assert!(run.output().contains("9024"), "{}", run.output());
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
}

/// A pool that cannot be created (here: a buffer_capacity whose mbufs exceed
/// the 256 MB of `-m 256`) is a clean error naming the direction and the size
/// that was tried. There is no "too small" message any more.
fn assert_pool_creation_error(out: &str, direction: usize, size: u64) {
    assert!(
        out.to_lowercase()
            .contains(&format!("direction {direction}")),
        "the failing direction {direction} is not named\n{out}"
    );
    assert!(
        out.contains(&size.to_string()),
        "the size that was tried ({size}) is not named\n{out}"
    );
    assert!(!out.contains("too small"), "{out}");
    assert!(!out.contains("--num-mbufs"), "{out}");
}

#[test]
fn a_pool_that_cannot_be_created_names_the_direction_and_the_size_tried() {
    require_cpus(2);
    let _g = serial();
    // 5_000_000 + 4096 + 4096 + 768 + 64 = 5_009_024 mbufs of ~2 KB each
    let mut env = Env::new(&config_json_cap(0, 0.0, 5_000_000));
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert_pool_creation_error(&out, 0, 5_009_024);
}

#[test]
fn the_failing_direction_is_named_when_only_direction_1_is_too_big() {
    require_cpus(3);
    let _g = serial();
    let mut env = Env::two_ports(&config_json_cap(0, 0.0, 1000));
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let big = env.write_config("cfg1.json", &config_json_cap(0, 0.0, 5_000_000));
    env.extra_app_args = vec!["--config1".into(), big.display().to_string()];
    let out = expect_failure(&env);
    assert_pool_creation_error(&out, 1, 5_009_024);
}

#[test]
fn default_config_path_is_config_json_in_the_working_directory() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    // No --config: dpds must pick up ./config.json from its cwd.
    let mut args = env.eal_args();
    args.push("--".into());

    let mut run = Run::spawn_in(
        &args,
        Some(env.xdg.path()),
        Some(env.socket_path()),
        Some(env.dir.path()),
    );
    #[cfg(feature = "telemetry")]
    run.wait_until("traffic", START_TIMEOUT, |r| {
        r.try_query("/dpds/stats,0")
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

// ---------------------------------------------------------------------------
// README: the "Without hardware" examples run as written
// ---------------------------------------------------------------------------

/// The `./target/release/dpds ...` commands of the first fenced block under
/// "### Without hardware" in README.md, backslash continuations joined,
/// split on whitespace (the examples use no quoting).
fn readme_without_hardware_commands() -> Vec<Vec<String>> {
    let readme = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"))
        .expect("README.md");
    let section = readme
        .split("### Without hardware")
        .nth(1)
        .expect("README has a 'Without hardware' section");
    let block = section
        .split("```")
        .nth(1)
        .expect("a fenced block in that section");
    let mut commands = Vec::new();
    let mut cur = String::new();
    for line in block.lines().skip(1) {
        // skip(1): the info string (```bash)
        let line = line.trim();
        if cur.is_empty() && (line.is_empty() || line.starts_with('#')) {
            continue;
        }
        if let Some(head) = line.strip_suffix('\\') {
            cur.push_str(head);
            cur.push(' ');
        } else {
            cur.push_str(line);
            commands.push(
                std::mem::take(&mut cur)
                    .split_whitespace()
                    .map(String::from)
                    .collect(),
            );
        }
    }
    assert!(
        cur.is_empty(),
        "dangling line continuation in the README block"
    );
    commands
}

/// The EAL `--file-prefix` of a command (`--file-prefix x` or
/// `--file-prefix=x`, EAL part only), DPDK's default `rte` when absent.
fn file_prefix(cmd: &[String]) -> String {
    let eal = cmd.split(|a| a == "--").next().unwrap();
    for (i, a) in eal.iter().enumerate() {
        if a == "--file-prefix" {
            return eal.get(i + 1).expect("--file-prefix needs a value").clone();
        }
        if let Some(v) = a.strip_prefix("--file-prefix=") {
            return v.to_string();
        }
    }
    "rte".to_string()
}

#[test]
fn the_file_prefix_of_a_command_is_parsed_with_dpdks_default() {
    let c = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
    assert_eq!(file_prefix(&c("dpds -l 0-1 -- --config a.json")), "rte");
    assert_eq!(file_prefix(&c("dpds --file-prefix foo -l 0-1")), "foo");
    assert_eq!(file_prefix(&c("dpds --file-prefix=bar -l 0-1 --")), "bar");
    // an app argument of the same name is not the EAL's
    assert_eq!(file_prefix(&c("dpds -l 0-1 -- --file-prefix x")), "rte");
}

#[test]
fn the_readme_without_hardware_examples_start_cleanly_and_stop_with_sigint() {
    require_cpus(2);
    let _g = serial();
    let commands = readme_without_hardware_commands();
    assert!(
        commands.len() >= 2,
        "expected the net_null and the net_pcap example, found {commands:?}"
    );
    for cmd in commands {
        assert_eq!(
            cmd[0], "./target/release/dpds",
            "README example does not start with the binary: {cmd:?}"
        );
        // A scratch working directory that looks like a checkout root (the
        // top-level files, e.g. config.json and any config.vdev.json, are
        // symlinked) and holds the input pcap the example names.
        let cwd = tempfile::tempdir().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for entry in fs::read_dir(&root).unwrap().flatten() {
            let path = entry.path();
            if path.is_file() {
                std::os::unix::fs::symlink(&path, cwd.path().join(entry.file_name())).unwrap();
            }
        }
        for arg in &cmd {
            for part in arg.split(',') {
                if let Some(rx) = part.strip_prefix("rx_pcap=") {
                    let p = cwd.path().join(rx);
                    let _ = fs::remove_file(&p);
                    write_pcap(&p, &frames(0, 64));
                }
            }
        }
        let xdg = tempfile::Builder::new()
            .prefix("dpds-x")
            .tempdir_in("/tmp")
            .unwrap();
        // DPDK's default --file-prefix is `rte` (not the program name); use
        // the prefix the command names, if it names one.
        let socket = xdg
            .path()
            .join("dpdk")
            .join(file_prefix(&cmd))
            .join("dpdk_telemetry.v2");
        let mut run = Run::spawn_in(&cmd[1..], Some(xdg.path()), Some(socket), Some(cwd.path()));
        run.command_list(); // fails fast, with the output, if it exits early
        thread::sleep(Duration::from_millis(1500));
        assert!(
            run.poll_exit().is_none(),
            "README example {cmd:?} exited on its own\n{}",
            run.output()
        );
        run.signal(libc::SIGINT);
        let st = run.wait_exit();
        assert_clean_exit(st, &run);
        let out = run.output();
        assert!(!out.contains("too small"), "{cmd:?}\n{out}");
        assert!(!out.contains("panicked"), "{cmd:?}\n{out}");
    }
}

// ---------------------------------------------------------------------------
// directed links: `--links <file>` (v2), drop causes, delay statistics and
// histogram
// ---------------------------------------------------------------------------
//
// Test-facing behaviour (see crates/dpds/tests/unit/links.rs for the unit API):
// - `--links <file>`, JSON `{"links": [{"name", "ingress", "egress", "config",
//   "lcore"?, "histogram"?, "delay_stats"?}]}`, is mutually exclusive with
//   `--config*`. A link is ONE direction: packets arriving on `ingress` leave
//   on `egress` with the link's config.
// - Ports are DPDK device names (here `net_pcap<i>` / `net_null<i>` vdevs),
//   resolved by name. Direction index = link index (file order). `ingress ==
//   egress` is a hairpin. A port is the ingress of at most one link and the
//   egress of at most one link; every probed port must be used. `-l` must
//   name exactly `1 + number of links` lcores; the i-th worker lcore runs
//   link i (or the link's explicit `lcore`, all links or none).
// - `/dpds/ports`: `num_links` (= `num_directions`), per link `dir<d>_link`,
//   `dir<d>_rx_name` (ingress), `dir<d>_tx_name` (egress); the `--config*`
//   shorthand is one link `dir0` (1 port: p0 -> p0) or two links `dir0`
//   (p0 -> p1, config0) and `dir1` (p1 -> p0, config1). The reply has
//   `3 + 5 * ports + 10 * links` keys.
// - `/dpds/stats,<d>`: `loss_drops`, `limiter_drops`, `buffer_full_drops`
//   (their sum is `dpds_rejects`), `delay_*`/`lateness_*` (numbers, as Uint or
//   2-decimal strings), first-loss detail "from=dpds,cause=<loss|limiter|buffer_full>".
// - `/dpds/delay_hist,<d>`: `lo_ns`, `bin_ns` (numbers), `bins` (256 uints),
//   `underflow`, `overflow`; the range rule is in unit/delay_stats.rs.

/// One link object of the links file: `extra` is `""` or further members,
/// e.g. `"lcore": 2`.
fn link_json_extra(name: &str, ingress: &str, egress: &str, config: &str, extra: &str) -> String {
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!(", {extra}")
    };
    format!(
        r#"{{ "name": "{name}", "ingress": "{ingress}", "egress": "{egress}", "config": {config}{extra} }}"#
    )
}

/// One link object of the links file.
fn link_json(name: &str, ingress: &str, egress: &str, config: &str) -> String {
    link_json_extra(name, ingress, egress, config, "")
}

/// As [`link_json`] with the optional `"histogram"` object (its JSON text,
/// e.g. `{"bins": 100}`).
fn link_json_hist(name: &str, ingress: &str, egress: &str, config: &str, hist: &str) -> String {
    link_json_extra(
        name,
        ingress,
        egress,
        config,
        &format!(r#""histogram": {hist}"#),
    )
}

/// As [`link_json`] with an explicit `"lcore"`.
fn link_json_lcore(name: &str, ingress: &str, egress: &str, config: &str, lcore: u32) -> String {
    link_json_extra(
        name,
        ingress,
        egress,
        config,
        &format!(r#""lcore": {lcore}"#),
    )
}

/// As [`link_json`] with the optional `"delay_stats"` flag.
fn link_json_delay_stats(
    name: &str,
    ingress: &str,
    egress: &str,
    config: &str,
    flag: bool,
) -> String {
    link_json_extra(
        name,
        ingress,
        egress,
        config,
        &format!(r#""delay_stats": {flag}"#),
    )
}

/// An environment in `--links` mode.
fn links_env(lcore_list: &str, links: &[String]) -> Env {
    let mut env = Env::with_lcores(lcore_list, &config_json(0, 0.0));
    let path = env.path("links.json");
    fs::write(&path, format!(r#"{{ "links": [{}] }}"#, links.join(",\n"))).unwrap();
    env.links = Some(path);
    env
}

/// A seeded config with another delay distribution.
#[cfg(feature = "telemetry")]
fn config_dist(distribution: &str, mean_ns: u64, std_ns: u64) -> String {
    config_json(mean_ns, 0.0)
        .replace(
            r#""distribution": "DeterministicDistribution""#,
            &format!(r#""distribution": "{distribution}""#),
        )
        .replace(r#""std_ns": 0.0"#, &format!(r#""std_ns": {std_ns}.0"#))
        .replacen('{', "{\n    \"seed\": 12345,", 1)
}

#[cfg(feature = "telemetry")]
/// A Spacer config that admits exactly three 64-byte frames at one instant
/// and spaces them 7.04 ms apart (1e5 bit/s; a 64-byte frame is
/// (64 + 4 + 20) * 8 = 704 bit on the wire, the limit is three of them).
fn config_spacer_three() -> String {
    let base = config_json(0, 0.0);
    let cfg = base.replace(
        r#""activate": false,
        "rate_bps": 100000000000.0,
        "limit_bit": 6152000000.0"#,
        r#""activate": true,
        "rate_bps": 100000.0,
        "limit_bit": 2112.0"#,
    );
    assert_ne!(cfg, base, "the limiter block of config_json changed");
    cfg
}

fn pcap_out(env: &Env, i: usize) -> PathBuf {
    env.path(&format!("out{i}.pcap"))
}

/// A number that may be a JSON number or a decimal string.
#[cfg(feature = "telemetry")]
fn num(v: &Value, key: &str) -> f64 {
    match &v[key] {
        Value::Number(n) => n.as_f64().unwrap(),
        Value::String(t) => t
            .parse()
            .unwrap_or_else(|_| panic!("{key} is not a number in {v}")),
        other => panic!("{key} is {other:?}, not a number, in {v}"),
    }
}

#[test]
fn links_file_with_a_pair_and_a_hairpin_forwards_every_link_by_its_own_config() {
    // the classic bidirectional pair as two links: "a-to-b" net_pcap0 ->
    // net_pcap1 (1 ms), "b-to-a" net_pcap1 -> net_pcap0 (no delay); plus a
    // hairpin "loop" on net_pcap2 (2 ms). `-l 0-3`.
    const N: [u32; 3] = [300, 200, 100];
    require_cpus(4);
    let _g = serial();
    let mut env = links_env(
        "0-3",
        &[
            link_json(
                "a-to-b",
                "net_pcap0",
                "net_pcap1",
                &config_json(1_000_000, 0.0),
            ),
            link_json("b-to-a", "net_pcap1", "net_pcap0", &config_json(0, 0.0)),
            link_json(
                "loop",
                "net_pcap2",
                "net_pcap2",
                &config_json(2_000_000, 0.0),
            ),
        ],
    );
    for (i, n) in N.iter().enumerate() {
        add_pcap_port(&mut env, i, &frames(i as u8, *n));
    }
    let outs: Vec<PathBuf> = (0..3).map(|i| pcap_out(&env, i)).collect();
    let mut run = env.spawn();

    // pcap0 -> pcap1: out1 gets in0; pcap1 -> pcap0: out0 gets in1; loop: out2 gets in2
    wait_frames(&mut run, &outs[1], N[0] as usize);
    wait_frames(&mut run, &outs[0], N[1] as usize);
    wait_frames(&mut run, &outs[2], N[2] as usize);
    thread::sleep(Duration::from_millis(700));

    #[cfg(feature = "telemetry")]
    {
        let p = run.query("/dpds/ports");
        assert_eq!(u(&p, "num_ports"), 3, "{p}");
        assert_eq!(u(&p, "num_directions"), 3, "{p}");
        assert_eq!(u(&p, "num_links"), 3, "one direction per link: {p}");
        for (d, link, rx, tx) in [
            (0, "a-to-b", "net_pcap0", "net_pcap1"),
            (1, "b-to-a", "net_pcap1", "net_pcap0"),
            (2, "loop", "net_pcap2", "net_pcap2"),
        ] {
            assert_eq!(s(&p, &format!("dir{d}_link")), link, "{p}");
            assert_eq!(s(&p, &format!("dir{d}_rx_name")), rx, "{p}");
            assert_eq!(s(&p, &format!("dir{d}_tx_name")), tx, "{p}");
            // the i-th worker lcore runs link i: `-l 0-3` -> 1, 2, 3
            assert_eq!(u(&p, &format!("dir{d}_lcore")), 1 + d as u64, "{p}");
            assert!(
                s(&p, &format!("dir{d}_config")).contains("links.json"),
                "the config source is the links file: {p}"
            );
        }
        assert!(p.get("dir3_link").is_none(), "{p}");
        assert_eq!(p.as_object().unwrap().len(), 3 + 5 * 3 + 10 * 3, "{p}");
        // names and ids agree
        for d in 0..3 {
            let rx_id = u(&p, &format!("dir{d}_rx_port"));
            assert_eq!(
                s(&p, &format!("port{rx_id}_name")),
                s(&p, &format!("dir{d}_rx_name")),
                "{p}"
            );
            let tx_id = u(&p, &format!("dir{d}_tx_port"));
            assert_eq!(
                s(&p, &format!("port{tx_id}_name")),
                s(&p, &format!("dir{d}_tx_name")),
                "{p}"
            );
        }
        for (d, &n) in N.iter().enumerate() {
            let st = run.query(&format!("/dpds/stats,{d}"));
            let n = n as u64;
            assert_eq!(u(&st, "rx_pkts"), n, "dir {d}: {st}");
            assert_eq!(u(&st, "tx_sent"), n, "dir {d}: {st}");
            assert_eq!(u(&st, "dpds_rejects"), 0, "dir {d}: {st}");
            for k in ["loss_drops", "limiter_drops", "buffer_full_drops"] {
                assert_eq!(u(&st, k), 0, "dir {d} {k}: {st}");
            }
            assert_eq!(s(&st, "first_loss_reason"), "Clean", "dir {d}: {st}");
        }
        // each link has its own delay: 1 ms, none, 2 ms
        for (d, delay) in [(0, 1_000_000.0), (1, 0.0), (2, 2_000_000.0)] {
            let st = run.query(&format!("/dpds/stats,{d}"));
            let mean = num(&st, "delay_mean_ns");
            assert!(
                (delay - 1.0..=delay).contains(&mean),
                "dir {d}: mean {mean} for a delay of {delay}: {st}"
            );
        }
    }

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(read_pcap(&outs[1]) == frames(0, N[0]), "link a-to-b");
    assert!(read_pcap(&outs[0]) == frames(1, N[1]), "link b-to-a");
    assert!(read_pcap(&outs[2]) == frames(2, N[2]), "the hairpin link");
}

#[test]
fn a_ring_of_three_ports_forwards_each_link_by_its_own_config_with_exact_counts() {
    // The user's example: A = net_pcap0, B = net_pcap1, C = net_pcap2 and the
    // links A -> B (1 ms), B -> C (100 % loss), C -> A (2 ms). Every port is
    // the ingress of one link and the egress of another. `-l 0-3`.
    //   out1 (B) must hold exactly in0 (N[0] frames), out0 (A) exactly in2
    //   (N[2] frames) and out2 (C) nothing, whichever port id each device got.
    const N: [u32; 3] = [300, 200, 100];
    require_cpus(4);
    for reverse_creation in [false, true] {
        let _g = serial();
        let mut env = links_env(
            "0-3",
            &[
                link_json(
                    "a-to-b",
                    "net_pcap0",
                    "net_pcap1",
                    &config_json(1_000_000, 0.0),
                ),
                link_json("b-to-c", "net_pcap1", "net_pcap2", &config_json(0, 1.0)),
                link_json(
                    "c-to-a",
                    "net_pcap2",
                    "net_pcap0",
                    &config_json(2_000_000, 0.0),
                ),
            ],
        );
        let order: [usize; 3] = if reverse_creation {
            [2, 1, 0]
        } else {
            [0, 1, 2]
        };
        for i in order {
            add_pcap_port(&mut env, i, &frames(i as u8, N[i]));
        }
        let outs: Vec<PathBuf> = (0..3).map(|i| pcap_out(&env, i)).collect();
        let mut run = env.spawn();
        wait_frames(&mut run, &outs[1], N[0] as usize);
        wait_frames(&mut run, &outs[0], N[2] as usize);
        wait_rx(&mut run, 1, N[1] as u64);
        thread::sleep(Duration::from_millis(700));

        #[cfg(feature = "telemetry")]
        {
            let p = run.query("/dpds/ports");
            assert_eq!(u(&p, "num_ports"), 3, "{p}");
            assert_eq!(u(&p, "num_directions"), 3, "{p}");
            assert_eq!(u(&p, "num_links"), 3, "{p}");
            assert_eq!(
                p.as_object().unwrap().len(),
                3 + 5 * 3 + 10 * 3,
                "3 + 5 * ports + 10 * links keys: {p}"
            );
            let id_of = |name: &str| {
                (0..3)
                    .find(|i| s(&p, &format!("port{i}_name")) == name)
                    .unwrap_or_else(|| panic!("{name} not in {p}"))
            };
            for (d, link, rx, tx) in [
                (0, "a-to-b", "net_pcap0", "net_pcap1"),
                (1, "b-to-c", "net_pcap1", "net_pcap2"),
                (2, "c-to-a", "net_pcap2", "net_pcap0"),
            ] {
                assert_eq!(s(&p, &format!("dir{d}_link")), link, "{p}");
                assert_eq!(s(&p, &format!("dir{d}_rx_name")), rx, "{p}");
                assert_eq!(s(&p, &format!("dir{d}_tx_name")), tx, "{p}");
                assert_eq!(u(&p, &format!("dir{d}_rx_port")), id_of(rx), "{p}");
                assert_eq!(u(&p, &format!("dir{d}_tx_port")), id_of(tx), "{p}");
                assert_eq!(u(&p, &format!("dir{d}_lcore")), 1 + d as u64, "{p}");
            }
            // A -> B: lossless, ~1 ms
            let st = run.query("/dpds/stats,0");
            assert_eq!(u(&st, "rx_pkts"), N[0] as u64, "{st}");
            assert_eq!(u(&st, "tx_sent"), N[0] as u64, "{st}");
            assert_eq!(u(&st, "dpds_rejects"), 0, "{st}");
            let mean = num(&st, "delay_mean_ns");
            assert!((999_999.0..=1_000_000.0).contains(&mean), "{st}");
            // B -> C: 100 % loss, every frame counted as a loss drop
            let st = run.query("/dpds/stats,1");
            assert_eq!(u(&st, "rx_pkts"), N[1] as u64, "{st}");
            assert_eq!(u(&st, "loss_drops"), N[1] as u64, "{st}");
            assert_eq!(u(&st, "dpds_rejects"), N[1] as u64, "{st}");
            assert_eq!(u(&st, "tx_sent"), 0, "{st}");
            // C -> A: lossless, ~2 ms
            let st = run.query("/dpds/stats,2");
            assert_eq!(u(&st, "rx_pkts"), N[2] as u64, "{st}");
            assert_eq!(u(&st, "tx_sent"), N[2] as u64, "{st}");
            assert_eq!(u(&st, "dpds_rejects"), 0, "{st}");
            let mean = num(&st, "delay_mean_ns");
            assert!((1_999_999.0..=2_000_000.0).contains(&mean), "{st}");
            // the NIC counters agree: port B received nothing it did not send on
            for (name, rx, tx) in [
                ("net_pcap0", N[0], N[2]),
                ("net_pcap1", N[1], N[0]),
                ("net_pcap2", N[2], 0),
            ] {
                let eth = run.query(&format!("/ethdev/stats,{}", id_of(name)));
                assert_eq!(u(&eth, "ipackets"), rx as u64, "{name}: {eth}");
                assert_eq!(u(&eth, "opackets"), tx as u64, "{name}: {eth}");
            }
        }

        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
        assert!(
            read_pcap(&outs[1]) == frames(0, N[0]),
            "A -> B carries exactly A's frames (reverse_creation={reverse_creation})"
        );
        assert!(
            read_pcap(&outs[0]) == frames(2, N[2]),
            "C -> A carries exactly C's frames (reverse_creation={reverse_creation})"
        );
        assert_eq!(
            read_pcap(&outs[2]).len(),
            0,
            "B -> C loses everything (reverse_creation={reverse_creation})"
        );
    }
}

#[test]
fn a_lossless_ring_of_three_delivers_every_stream_exactly_once() {
    // no loss anywhere: each of the three streams comes out on the next port
    // and nowhere else, byte for byte (A -> B, B -> C, C -> A)
    const N: [u32; 3] = [257, 64, 1000];
    require_cpus(4);
    let _g = serial();
    let mut env = links_env(
        "0-3",
        &[
            link_json("a-to-b", "net_pcap0", "net_pcap1", &config_json(0, 0.0)),
            link_json("b-to-c", "net_pcap1", "net_pcap2", &config_json(0, 0.0)),
            link_json("c-to-a", "net_pcap2", "net_pcap0", &config_json(0, 0.0)),
        ],
    );
    for (i, n) in N.iter().enumerate() {
        add_pcap_port(&mut env, i, &frames(i as u8, *n));
    }
    let outs: Vec<PathBuf> = (0..3).map(|i| pcap_out(&env, i)).collect();
    let mut run = env.spawn();
    run.wait_until("all frames in the output pcaps", START_TIMEOUT, |_| {
        outs.iter().map(|p| read_pcap(p).len()).sum::<usize>() >= N.iter().sum::<u32>() as usize
    });
    thread::sleep(Duration::from_millis(700)); // duplicates or stragglers would show up here
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(read_pcap(&outs[1]) == frames(0, N[0]), "A -> B");
    assert!(read_pcap(&outs[2]) == frames(1, N[1]), "B -> C");
    assert!(read_pcap(&outs[0]) == frames(2, N[2]), "C -> A");
}

#[test]
fn a_hairpin_link_sends_the_frames_back_out_of_the_port_they_came_in_on() {
    const N: u32 = 400;
    require_cpus(2);
    let _g = serial();
    let mut env = links_env(
        "0-1",
        &[link_json(
            "hp",
            "net_pcap0",
            "net_pcap0",
            &config_json(1_000_000, 0.0),
        )],
    );
    add_pcap_port(&mut env, 0, &frames(0, N));
    let out0 = pcap_out(&env, 0);
    let mut run = env.spawn();
    wait_frames(&mut run, &out0, N as usize);
    thread::sleep(Duration::from_millis(700));

    #[cfg(feature = "telemetry")]
    {
        let p = run.query("/dpds/ports");
        assert_eq!(u(&p, "num_ports"), 1, "{p}");
        assert_eq!(u(&p, "num_directions"), 1, "{p}");
        assert_eq!(u(&p, "num_links"), 1, "{p}");
        assert_eq!(s(&p, "dir0_link"), "hp", "{p}");
        assert_eq!(s(&p, "dir0_rx_name"), "net_pcap0", "{p}");
        assert_eq!(s(&p, "dir0_tx_name"), "net_pcap0", "{p}");
        assert_eq!(u(&p, "dir0_rx_port"), 0, "{p}");
        assert_eq!(u(&p, "dir0_tx_port"), 0, "{p}");
        assert_eq!(p.as_object().unwrap().len(), 3 + 5 + 10, "{p}");
        let st = run.query("/dpds/stats,0");
        assert_eq!(u(&st, "rx_pkts"), N as u64, "{st}");
        assert_eq!(u(&st, "tx_sent"), N as u64, "{st}");
        assert_eq!(u(&st, "dpds_rejects"), 0, "{st}");
    }

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(read_pcap(&out0) == frames(0, N));
}

#[test]
fn a_port_used_only_as_egress_works_and_is_never_polled() {
    // One link net_pcap0 -> net_pcap1. net_pcap1 is only an egress: its input
    // file holds frames nobody may read (no link has it as ingress), and
    // nothing leaves on net_pcap0 (it is only an ingress).
    const N: u32 = 300;
    const M: u32 = 50;
    require_cpus(2);
    let _g = serial();
    let mut env = links_env(
        "0-1",
        &[link_json(
            "one-way",
            "net_pcap0",
            "net_pcap1",
            &config_json(0, 0.0),
        )],
    );
    add_pcap_port(&mut env, 0, &frames(0, N));
    add_pcap_port(&mut env, 1, &frames(1, M));
    let (out0, out1) = (pcap_out(&env, 0), pcap_out(&env, 1));
    let mut run = env.spawn();
    wait_frames(&mut run, &out1, N as usize);
    thread::sleep(Duration::from_millis(700));

    #[cfg(feature = "telemetry")]
    {
        let p = run.query("/dpds/ports");
        assert_eq!(u(&p, "num_ports"), 2, "both probed ports are listed: {p}");
        assert_eq!(u(&p, "num_directions"), 1, "{p}");
        assert_eq!(u(&p, "num_links"), 1, "{p}");
        assert_eq!(s(&p, "dir0_link"), "one-way", "{p}");
        assert_eq!(s(&p, "dir0_rx_name"), "net_pcap0", "{p}");
        assert_eq!(s(&p, "dir0_tx_name"), "net_pcap1", "{p}");
        assert!(p.get("dir1_link").is_none(), "{p}");
        assert_eq!(p.as_object().unwrap().len(), 3 + 5 * 2 + 10, "{p}");
        // The egress-only port still needs an RX pool for `Port::init`, so the
        // link that transmits on it carries that port's applied RX ring too:
        // capacity + rx_ring(ingress) + tx_ring(egress) + rx_ring(egress-only)
        // + 768 + 64.
        let (rx_port, tx_port) = (u(&p, "dir0_rx_port"), u(&p, "dir0_tx_port"));
        let plain = 4096
            + u(&p, &format!("port{rx_port}_rx_ring"))
            + u(&p, &format!("port{tx_port}_tx_ring"))
            + 832;
        let pinned = plain + u(&p, &format!("port{tx_port}_rx_ring"));
        assert!(pinned > plain, "the egress-only RX ring is not empty: {p}");
        assert_eq!(u(&p, "dir0_pool_size"), pinned, "{p}");
        let st = run.query("/dpds/stats,0");
        assert_eq!(u(&st, "rx_pkts"), N as u64, "{st}");
        assert_eq!(u(&st, "tx_sent"), N as u64, "{st}");
        // the egress-only port's RX queue is not polled: it received nothing
        let eth1 = run.query("/ethdev/stats,1");
        assert_eq!(u(&eth1, "ipackets"), 0, "{eth1}");
        assert_eq!(u(&eth1, "opackets"), N as u64, "{eth1}");
        let eth0 = run.query("/ethdev/stats,0");
        assert_eq!(u(&eth0, "ipackets"), N as u64, "{eth0}");
        assert_eq!(u(&eth0, "opackets"), 0, "{eth0}");
    }

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(
        read_pcap(&out1) == frames(0, N),
        "exactly the ingress stream"
    );
    assert_eq!(
        read_pcap(&out0).len(),
        0,
        "nothing leaves on the ingress-only port"
    );
}

#[test]
fn links_follow_the_file_order_not_the_port_id_order() {
    // The file lists net_pcap1 -> net_pcap0 first (100 % loss) and net_pcap0 ->
    // net_pcap1 second (lossless), whichever port id the two devices got
    // (vdev creation order is varied).
    const N0: u32 = 200;
    const N1: u32 = 300;
    require_cpus(3);
    for reverse_creation in [false, true] {
        let _g = serial();
        let mut env = links_env(
            "0-2",
            &[
                link_json("ba", "net_pcap1", "net_pcap0", &config_json(0, 1.0)),
                link_json("ab", "net_pcap0", "net_pcap1", &config_json(0, 0.0)),
            ],
        );
        if reverse_creation {
            add_pcap_port(&mut env, 1, &frames(1, N1));
            add_pcap_port(&mut env, 0, &frames(0, N0));
        } else {
            add_pcap_port(&mut env, 0, &frames(0, N0));
            add_pcap_port(&mut env, 1, &frames(1, N1));
        }
        let (out0, out1) = (pcap_out(&env, 0), pcap_out(&env, 1));
        let mut run = env.spawn();
        // link 1 = net_pcap0 -> net_pcap1, lossless
        wait_frames(&mut run, &out1, N0 as usize);
        thread::sleep(Duration::from_millis(700));

        #[cfg(feature = "telemetry")]
        {
            let p = run.query("/dpds/ports");
            assert_eq!(u(&p, "num_links"), 2, "{p}");
            assert_eq!(s(&p, "dir0_link"), "ba", "{p}");
            assert_eq!(s(&p, "dir0_rx_name"), "net_pcap1", "{p}");
            assert_eq!(s(&p, "dir0_tx_name"), "net_pcap0", "{p}");
            assert_eq!(s(&p, "dir1_link"), "ab", "{p}");
            assert_eq!(s(&p, "dir1_rx_name"), "net_pcap0", "{p}");
            assert_eq!(s(&p, "dir1_tx_name"), "net_pcap1", "{p}");
            let id_of = |name: &str| {
                (0..2)
                    .find(|i| s(&p, &format!("port{i}_name")) == name)
                    .unwrap_or_else(|| panic!("{name} not in {p}"))
            };
            assert_eq!(u(&p, "dir0_rx_port"), id_of("net_pcap1"), "{p}");
            assert_eq!(u(&p, "dir0_tx_port"), id_of("net_pcap0"), "{p}");
            assert_eq!(u(&p, "dir1_rx_port"), id_of("net_pcap0"), "{p}");
            assert_eq!(u(&p, "dir1_tx_port"), id_of("net_pcap1"), "{p}");
            // link 0 saw the 300 frames of net_pcap1 and lost them all
            let d0 = run.query("/dpds/stats,0");
            assert_eq!(u(&d0, "rx_pkts"), N1 as u64, "{d0}");
            assert_eq!(u(&d0, "loss_drops"), N1 as u64, "{d0}");
            assert_eq!(u(&d0, "tx_sent"), 0, "{d0}");
            let d1 = run.query("/dpds/stats,1");
            assert_eq!(u(&d1, "rx_pkts"), N0 as u64, "{d1}");
            assert_eq!(u(&d1, "tx_sent"), N0 as u64, "{d1}");
            assert_eq!(u(&d1, "dpds_rejects"), 0, "{d1}");
        }

        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
        assert!(
            read_pcap(&out1) == frames(0, N0),
            "net_pcap0 -> net_pcap1 uses the second link (reverse_creation={reverse_creation})"
        );
        assert_eq!(
            read_pcap(&out0).len(),
            0,
            "net_pcap1 -> net_pcap0 uses the first link, 100 % loss (reverse_creation={reverse_creation})"
        );
    }
}

#[test]
fn a_net_null_hairpin_link_runs_next_to_a_pcap_pair() {
    const N: u32 = 50;
    require_cpus(4);
    let _g = serial();
    let mut env = links_env(
        "0-3",
        &[
            link_json("nul", "net_null2", "net_null2", &config_json(0, 0.0)),
            link_json("a-to-b", "net_pcap0", "net_pcap1", &config_json(0, 0.0)),
            link_json("b-to-a", "net_pcap1", "net_pcap0", &config_json(0, 0.0)),
        ],
    );
    add_pcap_port(&mut env, 0, &frames(0, N));
    add_pcap_port(&mut env, 1, &frames(1, N));
    env.vdevs.push("net_null2".into());
    let (out0, out1) = (pcap_out(&env, 0), pcap_out(&env, 1));
    let mut run = env.spawn();
    wait_frames(&mut run, &out0, N as usize);
    wait_frames(&mut run, &out1, N as usize);

    #[cfg(feature = "telemetry")]
    {
        let p = run.query("/dpds/ports");
        // the hairpin link is direction 0 (file order), then the pair
        assert_eq!(s(&p, "dir0_link"), "nul", "{p}");
        assert_eq!(s(&p, "dir0_rx_name"), "net_null2", "{p}");
        assert_eq!(s(&p, "dir0_tx_name"), "net_null2", "{p}");
        assert_eq!(s(&p, "dir1_link"), "a-to-b", "{p}");
        assert_eq!(s(&p, "dir2_link"), "b-to-a", "{p}");
        assert_eq!(u(&p, "num_links"), 3, "{p}");
        run.wait_until("net_null traffic on direction 0", START_TIMEOUT, |r| {
            r.try_query("/dpds/stats,0")
                .is_some_and(|v| u(&v, "rx_pkts") > 0 && u(&v, "tx_sent") > 0)
        });
        for d in [1, 2] {
            let st = run.query(&format!("/dpds/stats,{d}"));
            assert_eq!(u(&st, "rx_pkts"), N as u64, "{st}");
            assert_eq!(u(&st, "tx_sent"), N as u64, "{st}");
        }
    }

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(read_pcap(&out1) == frames(0, N));
    assert!(read_pcap(&out0) == frames(1, N));
}

#[cfg(feature = "telemetry")]
mod links_telemetry {
    use super::*;

    fn bins_of(h: &Value) -> Vec<u64> {
        arr_u(h, "bins")
    }

    /// Every accepted packet is in exactly one of bins/underflow/overflow.
    fn hist_total(h: &Value) -> u64 {
        bins_of(h).iter().sum::<u64>() + u(h, "underflow") + u(h, "overflow")
    }

    #[test]
    fn drop_causes_are_counted_per_link_and_the_first_loss_names_the_cause() {
        // link 0 (net_pcap0 -> net_pcap1): 100 % loss. link 1 (net_pcap1 ->
        // net_pcap0): a buffer of 8 packets and a delay nothing leaves within
        // (5 s; SIGINT drains it): all frames after the first 8 find the buffer full.
        const N0: u32 = 100;
        const N1: u32 = 100;
        require_cpus(3);
        let _g = serial();
        let mut env = links_env(
            "0-2",
            &[
                link_json("a-to-b", "net_pcap0", "net_pcap1", &config_json(0, 1.0)),
                link_json(
                    "b-to-a",
                    "net_pcap1",
                    "net_pcap0",
                    &config_json_cap(5_000_000_000, 0.0, 8),
                ),
            ],
        );
        add_pcap_port(&mut env, 0, &frames(0, N0));
        add_pcap_port(&mut env, 1, &frames(1, N1));
        let out0 = pcap_out(&env, 0);
        let mut run = env.spawn();
        wait_rx(&mut run, 0, N0 as u64);
        wait_rx(&mut run, 1, N1 as u64);
        thread::sleep(Duration::from_millis(500));

        let d0 = run.query("/dpds/stats,0");
        assert_eq!(u(&d0, "loss_drops"), N0 as u64, "{d0}");
        assert_eq!(u(&d0, "limiter_drops"), 0, "{d0}");
        assert_eq!(u(&d0, "buffer_full_drops"), 0, "{d0}");
        assert_eq!(u(&d0, "dpds_rejects"), N0 as u64, "{d0}");
        assert_eq!(s(&d0, "first_loss_reason"), "DpdsReject", "{d0}");
        assert_eq!(s(&d0, "first_loss_detail"), "from=dpds,cause=loss", "{d0}");

        let d1 = run.query("/dpds/stats,1");
        assert_eq!(u(&d1, "loss_drops"), 0, "{d1}");
        assert_eq!(u(&d1, "limiter_drops"), 0, "{d1}");
        assert_eq!(u(&d1, "buffer_full_drops"), N1 as u64 - 8, "{d1}");
        assert_eq!(u(&d1, "dpds_rejects"), N1 as u64 - 8, "{d1}");
        assert_eq!(u(&d1, "tx_sent"), 0, "nothing is due yet: {d1}");
        assert_eq!(u(&d1, "buffer_len_max"), 8, "{d1}");
        assert_eq!(s(&d1, "first_loss_reason"), "DpdsReject", "{d1}");
        assert_eq!(
            s(&d1, "first_loss_detail"),
            "from=dpds,cause=buffer_full",
            "{d1}"
        );
        // exactly the 8 accepted packets have an intended delay of ~5 s
        let mean = num(&d1, "delay_mean_ns");
        assert!((4_999_999_000.0..=5_000_000_000.0).contains(&mean), "{d1}");
        let h = run.query("/dpds/delay_hist,1");
        assert_eq!(hist_total(&h), 8, "only accepted packets are binned: {h}");

        // the epochs carry the causes too (an idle gap closes an epoch)
        run.wait_until("a closed epoch on direction 0", START_TIMEOUT, |r| {
            r.try_query("/dpds/epochs,0")
                .is_some_and(|v| u(&v, "epochs_closed") >= 1)
        });
        let ep = run.query("/dpds/epochs,0");
        assert_eq!(
            arr_u(&ep, "loss_drops").iter().sum::<u64>(),
            N0 as u64,
            "{ep}"
        );
        assert_eq!(arr_u(&ep, "limiter_drops").iter().sum::<u64>(), 0, "{ep}");
        assert_eq!(
            arr_u(&ep, "buffer_full_drops").iter().sum::<u64>(),
            0,
            "{ep}"
        );
        assert_eq!(
            arr_u(&ep, "dpds_rejects").iter().sum::<u64>(),
            N0 as u64,
            "{ep}"
        );

        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
        assert_eq!(
            read_pcap(&out0).len(),
            8,
            "the SIGINT drain emits the 8 held packets"
        );
        assert!(read_pcap(&out0) == frames(1, N1)[..8].to_vec());
    }

    #[test]
    fn limiter_drops_are_counted_and_the_histogram_covers_the_spacer_queueing() {
        // Hairpin link with a Spacer that admits three 64-byte frames at one
        // instant (see config_spacer_three): of 100 frames 3 are accepted.
        const N: u32 = 100;
        require_cpus(2);
        let _g = serial();
        let mut env = links_env(
            "0-1",
            &[link_json(
                "l",
                "net_pcap0",
                "net_pcap0",
                &config_spacer_three(),
            )],
        );
        add_pcap_port(&mut env, 0, &frames(0, N));
        let out0 = pcap_out(&env, 0);
        let mut run = env.spawn();
        wait_frames(&mut run, &out0, 3);
        wait_rx(&mut run, 0, N as u64);
        thread::sleep(Duration::from_millis(300));

        let st = run.query("/dpds/stats,0");
        assert_eq!(u(&st, "rx_pkts"), N as u64, "{st}");
        assert_eq!(u(&st, "limiter_drops"), N as u64 - 3, "{st}");
        assert_eq!(u(&st, "loss_drops"), 0, "{st}");
        assert_eq!(u(&st, "buffer_full_drops"), 0, "{st}");
        assert_eq!(u(&st, "dpds_rejects"), N as u64 - 3, "{st}");
        assert_eq!(u(&st, "tx_sent"), 3, "{st}");
        assert_eq!(
            s(&st, "first_loss_detail"),
            "from=dpds,cause=limiter",
            "{st}"
        );
        // the third frame waits two frame times: ~14.08 ms
        let max = num(&st, "delay_max_ns");
        assert!((14_079_000.0..=14_081_000.0).contains(&max), "{st}");
        assert!(
            num(&st, "delay_min_ns") < 1000.0,
            "the first frame is not delayed: {st}"
        );
        // range [0, 21.12 ms] (the queueing allowance of the limiter): all 3 inside
        let h = run.query("/dpds/delay_hist,0");
        assert_eq!(hist_total(&h), 3, "{h}");
        assert_eq!((u(&h, "underflow"), u(&h, "overflow")), (0, 0), "{h}");
        assert!(num(&h, "lo_ns") < 1000.0, "{h}");
        let hi = num(&h, "lo_ns") + 256.0 * num(&h, "bin_ns");
        assert!(
            hi >= 21_000_000.0,
            "the range reaches the limiter's allowance: {h}"
        );

        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
        assert_eq!(read_pcap(&out0).len(), 3);
    }

    #[test]
    fn the_delay_histogram_has_the_documented_shape_for_a_deterministic_and_a_normal_delay() {
        // link 0: deterministic 2 ms, 500 frames. link 1: Normal(5 ms, 0.5 ms),
        // 2000 frames, seeded. Both lossless with a big buffer.
        const N0: u32 = 500;
        const N1: u32 = 2000;
        require_cpus(3);
        let _g = serial();
        let mut env = links_env(
            "0-2",
            &[
                link_json(
                    "a-to-b",
                    "net_pcap0",
                    "net_pcap1",
                    &config_json(2_000_000, 0.0),
                ),
                link_json(
                    "b-to-a",
                    "net_pcap1",
                    "net_pcap0",
                    &config_dist("NormalDistribution", 5_000_000, 500_000),
                ),
            ],
        );
        add_pcap_port(&mut env, 0, &frames(0, N0));
        add_pcap_port(&mut env, 1, &frames(1, N1));
        let mut run = env.spawn();
        for (d, n) in [(0, N0), (1, N1)] {
            let cmd = format!("/dpds/stats,{d}");
            run.wait_until(&format!("{cmd} tx_sent == {n}"), START_TIMEOUT, |r| {
                r.try_query(&cmd)
                    .is_some_and(|v| u(&v, "tx_sent") >= n as u64)
            });
        }

        // deterministic delay: a 1 us window around 2 ms
        let st = run.query("/dpds/stats,0");
        assert_eq!(u(&st, "dpds_rejects"), 0, "{st}");
        for k in ["delay_min_ns", "delay_max_ns"] {
            let v = num(&st, k);
            assert!((1_999_999.0..=2_000_000.0).contains(&v), "{k}: {st}");
        }
        assert!(num(&st, "delay_std_ns") < 1.0, "{st}");
        let h0 = run.query("/dpds/delay_hist,0");
        let mut keys: Vec<&str> = h0.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "bin_ns",
                "bins",
                "lo_ns",
                "num_bins",
                "overflow",
                "underflow"
            ],
            "{h0}"
        );
        assert_eq!(bins_of(&h0).len(), 256, "{h0}");
        assert_eq!(hist_total(&h0), N0 as u64, "{h0}");
        assert_eq!((u(&h0, "underflow"), u(&h0, "overflow")), (0, 0), "{h0}");
        assert!((num(&h0, "lo_ns") - 1_999_500.0).abs() < 1.0, "{h0}");
        assert!((num(&h0, "bin_ns") - 3.90625).abs() < 1e-6, "{h0}");
        // delay = 2e6 - (0..1) ns: bin 127 or 128 (the window centre)
        let b0 = bins_of(&h0);
        assert_eq!(b0[127] + b0[128], N0 as u64, "{h0}");

        // normal delay: range [mean - 6 std, mean + 6 std] = [2 ms, 8 ms]
        let st = run.query("/dpds/stats,1");
        assert_eq!(u(&st, "dpds_rejects"), 0, "{st}");
        let mean = num(&st, "delay_mean_ns");
        assert!(
            (4_900_000.0..=5_100_000.0).contains(&mean),
            "mean {mean}: {st}"
        );
        let std = num(&st, "delay_std_ns");
        assert!((450_000.0..=550_000.0).contains(&std), "std {std}: {st}");
        assert!(
            num(&st, "delay_min_ns") <= mean && mean <= num(&st, "delay_max_ns"),
            "{st}"
        );
        assert!(num(&st, "delay_min_ns") >= 2_000_000.0, "{st}");
        assert!(num(&st, "delay_max_ns") <= 8_000_000.0, "{st}");
        assert!(num(&st, "lateness_mean_ns") >= 0.0, "{st}");
        assert!(
            num(&st, "lateness_mean_ns") <= num(&st, "lateness_max_ns"),
            "{st}"
        );
        assert!(
            num(&st, "lateness_max_ns") < 2.0e9,
            "a poll loop is not 2 s late: {st}"
        );

        let h1 = run.query("/dpds/delay_hist,1");
        assert_eq!(bins_of(&h1).len(), 256, "{h1}");
        assert_eq!(u(&h1, "num_bins"), 256, "{h1}");
        assert_eq!(hist_total(&h1), N1 as u64, "{h1}");
        assert_eq!(
            (u(&h1, "underflow"), u(&h1, "overflow")),
            (0, 0),
            "6 sigma: {h1}"
        );
        let (lo, bin) = (num(&h1, "lo_ns"), num(&h1, "bin_ns"));
        assert!((lo - 2_000_000.0).abs() < 1.0, "{h1}");
        assert!((bin - 23_437.5).abs() < 1e-3, "{h1}");
        let b1 = bins_of(&h1);
        let core: u64 = b1[64..=192].iter().sum(); // +- 3 sigma (sigma = 21.3 bins)
        assert!(
            core * 100 >= N1 as u64 * 99,
            "{core} of {N1} within 3 sigma: {h1}"
        );
        assert!(
            b1[..16].iter().sum::<u64>() == 0 && b1[240..].iter().sum::<u64>() == 0,
            "{h1}"
        );
        // the mean of the bin centres agrees with delay_mean_ns within a bin
        let centre_mean = b1
            .iter()
            .enumerate()
            .map(|(i, c)| (lo + (i as f64 + 0.5) * bin) * *c as f64)
            .sum::<f64>()
            / N1 as f64;
        assert!((centre_mean - mean).abs() < bin, "{centre_mean} vs {mean}");

        // cumulative: a second query never decreases a bin
        let again = bins_of(&run.query("/dpds/delay_hist,1"));
        assert!(again.iter().zip(&b1).all(|(a, b)| a >= b));

        // bad link params are error replies, like /dpds/stats
        for bad in ["", ",2", ",x", ",-1", ",", ",0,1"] {
            let reply = run.try_query(&format!("/dpds/delay_hist{bad}"));
            assert!(
                reply.is_none_or(|v| v.get("bins").is_none()),
                "/dpds/delay_hist{bad} must be an error reply"
            );
        }
        run.signal(libc::SIGINT);
        assert_clean_exit(run.wait_exit(), &run);
    }
}

#[cfg(feature = "telemetry")]
#[test]
fn a_custom_histogram_per_link_sets_bins_and_range() {
    // link 0: deterministic 2 ms with 100 bins over [1_999_000, 2_001_000)
    // (20 ns each). link 1: Normal(5 ms, 0.5 ms) with only `bins` given: 50 bins
    // over the automatic range [2 ms, 8 ms) (120 us each).
    const N0: u32 = 300;
    const N1: u32 = 1500;
    require_cpus(3);
    let _g = serial();
    let mut env = links_env(
        "0-2",
        &[
            link_json_hist(
                "a-to-b",
                "net_pcap0",
                "net_pcap1",
                &config_json(2_000_000, 0.0),
                r#"{ "bins": 100, "lo_ns": 1999000, "hi_ns": 2001000 }"#,
            ),
            link_json_hist(
                "b-to-a",
                "net_pcap1",
                "net_pcap0",
                &config_dist("NormalDistribution", 5_000_000, 500_000),
                r#"{ "bins": 50 }"#,
            ),
        ],
    );
    add_pcap_port(&mut env, 0, &frames(0, N0));
    add_pcap_port(&mut env, 1, &frames(1, N1));
    let mut run = env.spawn();
    for (d, n) in [(0, N0), (1, N1)] {
        let cmd = format!("/dpds/stats,{d}");
        run.wait_until(&format!("{cmd} tx_sent == {n}"), START_TIMEOUT, |r| {
            r.try_query(&cmd)
                .is_some_and(|v| u(&v, "tx_sent") >= n as u64)
        });
    }

    let h0 = run.query("/dpds/delay_hist,0");
    assert_eq!(u(&h0, "num_bins"), 100, "{h0}");
    let b0 = arr_u(&h0, "bins");
    assert_eq!(b0.len(), 100, "{h0}");
    assert!((num(&h0, "lo_ns") - 1_999_000.0).abs() < 1e-3, "{h0}");
    assert!((num(&h0, "bin_ns") - 20.0).abs() < 1e-6, "{h0}");
    assert_eq!((u(&h0, "underflow"), u(&h0, "overflow")), (0, 0), "{h0}");
    // the delay is 2e6 - (0..1) ns: 1000 ns above lo = 50 bins (49.95..50)
    assert_eq!(b0[49] + b0[50], N0 as u64, "{h0}");

    let h1 = run.query("/dpds/delay_hist,1");
    assert_eq!(u(&h1, "num_bins"), 50, "{h1}");
    let b1 = arr_u(&h1, "bins");
    assert_eq!(b1.len(), 50, "{h1}");
    assert!((num(&h1, "lo_ns") - 2_000_000.0).abs() < 1.0, "{h1}");
    assert!((num(&h1, "bin_ns") - 120_000.0).abs() < 1e-3, "{h1}");
    let total = b1.iter().sum::<u64>() + u(&h1, "underflow") + u(&h1, "overflow");
    assert_eq!(total, N1 as u64, "{h1}");
    // sigma = 4.2 bins around bin 25: +-3 sigma is bins 12..=37
    let core: u64 = b1[12..=37].iter().sum();
    assert!(core * 100 >= N1 as u64 * 99, "{core} of {N1}: {h1}");

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
}

// ---------------------------------------------------------------------------
// the --config shorthand: dir0 (and dir1)
// ---------------------------------------------------------------------------

#[cfg(feature = "telemetry")]
#[test]
fn the_shorthand_with_two_ports_is_two_links_dir0_with_config0_and_dir1_with_config1() {
    // `--config0` is the link dir0 net_pcap0 -> net_pcap1 (100 % loss),
    // `--config1` the link dir1 net_pcap1 -> net_pcap0 (lossless).
    const N0: u32 = 120;
    const N1: u32 = 90;
    require_cpus(3);
    let _g = serial();
    let env = dir_env(
        &config_json(0, 0.0),
        Some(&config_json(0, 1.0)),
        Some(&config_json(0, 0.0)),
        &frames(0, N0),
        &frames(1, N1),
    );
    let (out0, out1) = (pcap_out(&env, 0), pcap_out(&env, 1));
    let mut run = env.spawn();
    wait_frames(&mut run, &out0, N1 as usize);
    wait_rx(&mut run, 0, N0 as u64);
    thread::sleep(Duration::from_millis(500));

    let p = run.query("/dpds/ports");
    assert_eq!(u(&p, "num_directions"), 2, "{p}");
    assert_eq!(u(&p, "num_links"), 2, "two links: {p}");
    assert_eq!(s(&p, "dir0_link"), "dir0", "{p}");
    assert_eq!(s(&p, "dir1_link"), "dir1", "{p}");
    assert_eq!(s(&p, "dir0_rx_name"), "net_pcap0", "{p}");
    assert_eq!(s(&p, "dir0_tx_name"), "net_pcap1", "{p}");
    assert_eq!(s(&p, "dir1_rx_name"), "net_pcap1", "{p}");
    assert_eq!(s(&p, "dir1_tx_name"), "net_pcap0", "{p}");
    assert!(s(&p, "dir0_config").ends_with("cfg0.json"), "{p}");
    assert!(s(&p, "dir1_config").ends_with("cfg1.json"), "{p}");
    assert_eq!(p.as_object().unwrap().len(), 3 + 5 * 2 + 10 * 2, "{p}");
    let d0 = run.query("/dpds/stats,0");
    assert_eq!(u(&d0, "loss_drops"), N0 as u64, "config0 loses: {d0}");
    let d1 = run.query("/dpds/stats,1");
    assert_eq!(u(&d1, "tx_sent"), N1 as u64, "config1 is lossless: {d1}");

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert_eq!(read_pcap(&out1).len(), 0, "dir0 (config0) loses everything");
    assert!(
        read_pcap(&out0) == frames(1, N1),
        "dir1 (config1) is lossless"
    );
}

#[cfg(feature = "telemetry")]
#[test]
fn the_shorthand_with_one_port_is_the_single_hairpin_link_dir0() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::new(&config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let cfg0 = env.write_config("cfg0.json", &config_json(0, 0.0));
    env.extra_app_args = vec!["--config0".into(), cfg0.display().to_string()];
    let mut run = env.spawn();
    let p = run.query("/dpds/ports");
    assert_eq!(u(&p, "num_links"), 1, "{p}");
    assert_eq!(u(&p, "num_directions"), 1, "{p}");
    assert_eq!(s(&p, "dir0_link"), "dir0", "{p}");
    assert_eq!(s(&p, "dir0_rx_name"), "net_null0", "{p}");
    assert_eq!(s(&p, "dir0_tx_name"), "net_null0", "{p}");
    assert_eq!(
        s(&p, "dir0_config"),
        cfg0.display().to_string(),
        "config0 wins: {p}"
    );
    assert!(p.get("dir1_link").is_none(), "{p}");
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
}

// ---------------------------------------------------------------------------
// --links: startup errors must be clean error exits that name the problem
// ---------------------------------------------------------------------------

#[test]
fn links_with_the_wrong_lcore_count_names_the_requirement() {
    require_cpus(4);
    let _g = serial();
    // two links are 2 workers: 3 lcores, not 2
    let mut env = links_env(
        "0-1",
        &[
            link_json("a-to-b", "net_pcap0", "net_pcap1", &config_json(0, 0.0)),
            link_json("b-to-a", "net_pcap1", "net_pcap0", &config_json(0, 0.0)),
        ],
    );
    add_pcap_port(&mut env, 0, &frames(0, 1));
    add_pcap_port(&mut env, 1, &frames(1, 1));
    let out = expect_failure(&env);
    assert_lcore_requirement(&out, 2, 2);

    // a ring of three links needs `-l 0-3`; `-l 0-2` is too few
    let mut env = links_env(
        "0-2",
        &[
            link_json("a-to-b", "net_null0", "net_null1", &config_json(0, 0.0)),
            link_json("b-to-c", "net_null1", "net_null2", &config_json(0, 0.0)),
            link_json("c-to-a", "net_null2", "net_null0", &config_json(0, 0.0)),
        ],
    );
    for i in 0..3 {
        env.vdevs.push(format!("net_null{i}"));
    }
    let out = expect_failure(&env);
    assert_lcore_requirement(&out, 3, 3);

    // ... and too many is an error as well (a hairpin is one link)
    let mut env = links_env(
        "0-3",
        &[link_json(
            "loop",
            "net_null0",
            "net_null0",
            &config_json(0, 0.0),
        )],
    );
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert_lcore_requirement(&out, 1, 4);
}

#[test]
fn explicit_lcores_in_descending_order_are_honoured_literally() {
    // `-l 0-2`: the workers are 1 and 2, link 0 asks for lcore 2 and link 1 for
    // lcore 1 (the opposite of the default ascending assignment).
    const N0: u32 = 120;
    const N1: u32 = 80;
    require_cpus(3);
    let _g = serial();
    let mut env = links_env(
        "0-2",
        &[
            link_json_lcore("a-to-b", "net_pcap0", "net_pcap1", &config_json(0, 0.0), 2),
            link_json_lcore("b-to-a", "net_pcap1", "net_pcap0", &config_json(0, 0.0), 1),
        ],
    );
    add_pcap_port(&mut env, 0, &frames(0, N0));
    add_pcap_port(&mut env, 1, &frames(1, N1));
    let (out0, out1) = (pcap_out(&env, 0), pcap_out(&env, 1));
    let mut run = env.spawn();
    wait_frames(&mut run, &out1, N0 as usize);
    wait_frames(&mut run, &out0, N1 as usize);
    thread::sleep(Duration::from_millis(500));

    // the startup log shows where each direction runs
    let log = run.output();
    assert!(log.contains("Direction 0: lcore 2"), "{log}");
    assert!(log.contains("Direction 1: lcore 1"), "{log}");

    #[cfg(feature = "telemetry")]
    {
        let p = run.query("/dpds/ports");
        assert_eq!(u(&p, "dir0_lcore"), 2, "{p}");
        assert_eq!(u(&p, "dir1_lcore"), 1, "{p}");
        assert_eq!(u(&run.query("/dpds/stats,0"), "rx_pkts"), N0 as u64);
        assert_eq!(u(&run.query("/dpds/stats,1"), "rx_pkts"), N1 as u64);
    }

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(read_pcap(&out1) == frames(0, N0));
    assert!(read_pcap(&out0) == frames(1, N1));
}

#[test]
fn bad_explicit_lcores_are_clean_exits_naming_the_problem() {
    require_cpus(3);
    let _g = serial();
    let cfg = || config_json(0, 0.0);
    let two = |a: u32, b: u32| {
        [
            link_json_lcore("a-to-b", "net_null0", "net_null1", &cfg(), a),
            link_json_lcore("b-to-a", "net_null1", "net_null0", &cfg(), b),
        ]
    };

    // the main lcore (0) cannot run a link
    let out = links_failure(&two(0, 1), 2, "0-2");
    assert!(out.contains("main"), "main lcore\n{out}");

    // an lcore that is not an EAL worker
    let out = links_failure(&two(1, 9), 2, "0-2");
    assert!(
        out.contains('9') && out.contains("worker"),
        "not a worker\n{out}"
    );

    // the same lcore twice
    let out = links_failure(&two(1, 1), 2, "0-2");
    assert!(out.contains("lcore"), "duplicate\n{out}");

    // an array instead of one lcore (the v1 per-direction shape)
    let out = links_failure(
        &[link_json_extra(
            "one",
            "net_null0",
            "net_null0",
            &cfg(),
            r#""lcore": [1]"#,
        )],
        1,
        "0-1",
    );
    assert!(!out.is_empty(), "array: a clean error exit\n{out}");

    // some links with an lcore, some without: all or none
    let out = links_failure(
        &[
            link_json_lcore("with", "net_null0", "net_null1", &cfg(), 1),
            link_json("without", "net_null1", "net_null0", &cfg()),
        ],
        2,
        "0-2",
    );
    assert!(out.contains("lcore"), "mixing\n{out}");
}

#[test]
fn links_excludes_config_flags_with_a_usage_error() {
    require_cpus(2);
    let _g = serial();
    for flag in ["--config", "--config0", "--config1"] {
        let mut env = links_env(
            "0-1",
            &[link_json(
                "loop",
                "net_null0",
                "net_null0",
                &config_json(0, 0.0),
            )],
        );
        env.vdevs.push("net_null0".into());
        let cfg = env.config.display().to_string();
        env.extra_app_args = vec![flag.into(), cfg];
        let out = expect_failure(&env);
        assert!(out.contains("Usage"), "{flag}\n{out}");
        assert!(out.contains("--links"), "{flag}\n{out}");
        assert!(out.contains(flag), "{flag}\n{out}");
    }
}

/// `--links` with the given `links` JSON array text, vdevs `net_null<i>` for
/// `0..null_ports`, and `-l <lcores>`; runs until it fails and returns the
/// output.
fn links_failure(links: &[String], null_ports: usize, lcores: &str) -> String {
    let mut env = links_env(lcores, links);
    for i in 0..null_ports {
        env.vdevs.push(format!("net_null{i}"));
    }
    expect_failure(&env)
}

#[test]
fn a_link_config_without_initialization_rate_pps_is_a_clean_exit_naming_the_field() {
    require_cpus(2);
    let _g = serial();
    let out = links_failure(
        &[link_json(
            "l",
            "net_null0",
            "net_null0",
            &config_without_rate(),
        )],
        1,
        "0-1",
    );
    assert!(out.contains("initialization_rate_pps"), "{out}");
}

#[test]
fn links_file_errors_are_clean_exits_naming_the_problem() {
    require_cpus(3);
    let _g = serial();
    let cfg = || config_json(0, 0.0);

    // an ingress that no probed device has
    let out = links_failure(
        &[link_json("ab", "net_null9", "net_null0", &cfg())],
        1,
        "0-1",
    );
    assert!(out.contains("net_null9"), "unknown ingress\n{out}");

    // an egress that no probed device has
    let out = links_failure(
        &[link_json("ab", "net_null0", "net_null9", &cfg())],
        1,
        "0-1",
    );
    assert!(out.contains("net_null9"), "unknown egress\n{out}");

    // a probed port that no link uses (neither as ingress nor as egress)
    let out = links_failure(
        &[link_json("l", "net_null0", "net_null0", &cfg())],
        2,
        "0-1",
    );
    assert!(out.contains("net_null1"), "unused probed port\n{out}");

    // a probed port used by a link as egress only is NOT unused
    // (covered by a_port_used_only_as_egress_works_and_is_never_polled)

    // a port that is the ingress of two links
    let out = links_failure(
        &[
            link_json("a", "net_null0", "net_null1", &cfg()),
            link_json("b", "net_null0", "net_null2", &cfg()),
        ],
        3,
        "0-2",
    );
    assert!(out.contains("net_null0"), "ingress twice\n{out}");
    assert!(out.contains("ingress"), "ingress twice\n{out}");

    // a port that is the egress of two links
    let out = links_failure(
        &[
            link_json("a", "net_null0", "net_null2", &cfg()),
            link_json("b", "net_null1", "net_null2", &cfg()),
        ],
        3,
        "0-2",
    );
    assert!(out.contains("net_null2"), "egress twice\n{out}");
    assert!(out.contains("egress"), "egress twice\n{out}");

    // a port that is ingress twice and egress twice
    let out = links_failure(
        &[
            link_json("a", "net_null0", "net_null0", &cfg()),
            link_json("b", "net_null0", "net_null0", &cfg()),
        ],
        1,
        "0-2",
    );
    assert!(out.contains("net_null0"), "ingress and egress twice\n{out}");

    // a hairpin plus a link that uses the same port as ingress
    let out = links_failure(
        &[
            link_json("hp", "net_null0", "net_null0", &cfg()),
            link_json("x", "net_null0", "net_null1", &cfg()),
        ],
        2,
        "0-2",
    );
    assert!(out.contains("net_null0"), "hairpin plus ingress\n{out}");

    // an empty ingress or egress
    let out = links_failure(&[link_json("blank", "", "net_null0", &cfg())], 1, "0-1");
    assert!(out.contains("blank"), "empty ingress\n{out}");
    let out = links_failure(&[link_json("blank2", "net_null0", "", &cfg())], 1, "0-1");
    assert!(out.contains("blank2"), "empty egress\n{out}");

    // bad and duplicate link names
    let out = links_failure(
        &[link_json("bad name!", "net_null0", "net_null0", &cfg())],
        1,
        "0-1",
    );
    assert!(out.contains("bad name!"), "bad name\n{out}");
    let out = links_failure(
        &[
            link_json("same", "net_null0", "net_null0", &cfg()),
            link_json("same", "net_null1", "net_null1", &cfg()),
        ],
        2,
        "0-2",
    );
    assert!(out.contains("same"), "duplicate name\n{out}");
    let long = "x".repeat(33);
    let out = links_failure(
        &[link_json(&long, "net_null0", "net_null0", &cfg())],
        1,
        "0-1",
    );
    assert!(out.contains(&long), "name over 32 characters\n{out}");

    // a link config with invalid parameters names the link (and the parameter)
    let out = links_failure(
        &[link_json(
            "badcfg",
            "net_null0",
            "net_null0",
            &config_bad_params(),
        )],
        1,
        "0-1",
    );
    assert!(out.contains("badcfg"), "the link is named\n{out}");
    assert_names_bad_params(&out);
}

#[test]
fn the_v1_link_shape_is_a_clean_exit_naming_the_unknown_key() {
    require_cpus(2);
    let _g = serial();
    let v1 = format!(
        r#"{{ "name": "old", "ports": ["net_null0"], "configs": [{}] }}"#,
        config_json(0, 0.0)
    );
    let out = links_failure(&[v1], 1, "0-1");
    assert!(out.contains("ports"), "the unknown key is named\n{out}");
    assert!(out.to_lowercase().contains("unknown"), "{out}");
}

#[test]
fn bad_histogram_settings_are_clean_exits_naming_the_link() {
    require_cpus(2);
    let _g = serial();
    let cfg = || config_json(1_000_000, 0.0); // automatic range [999_500, 1_000_500)
    for (hist, what) in [
        (r#"{ "bins": 0 }"#, "bins 0"),
        (r#"{ "bins": 501 }"#, "bins 501"),
        (r#"{ "lo_ns": 5000, "hi_ns": 5000 }"#, "lo == hi"),
        (r#"{ "lo_ns": -1 }"#, "negative lo"),
        // valid on its own, but beyond the automatic hi: caught at startup
        (r#"{ "lo_ns": 5000000 }"#, "lo above the automatic hi"),
        (r#"{ "hi_ns": 100 }"#, "hi below the automatic lo"),
    ] {
        let out = links_failure(
            &[link_json_hist(
                "histlink",
                "net_null0",
                "net_null0",
                &cfg(),
                hist,
            )],
            1,
            "0-1",
        );
        assert!(out.contains("histlink"), "{what}: the link is named\n{out}");
    }
    // the histogram is one object per link, not the v1 array
    let out = links_failure(
        &[link_json_hist(
            "lenlink",
            "net_null0",
            "net_null0",
            &cfg(),
            "[{}]",
        )],
        1,
        "0-1",
    );
    assert!(!out.is_empty(), "{out}");
}

#[test]
fn a_missing_or_malformed_links_file_is_an_error_exit_not_a_crash() {
    require_cpus(2);
    let _g = serial();
    let mut env = links_env(
        "0-1",
        &[link_json(
            "loop",
            "net_null0",
            "net_null0",
            &config_json(0, 0.0),
        )],
    );
    env.vdevs.push("net_null0".into());
    env.links = Some(env.path("does-not-exist.json"));
    let out = expect_failure(&env);
    assert!(out.contains("does-not-exist.json"), "{out}");

    let bad = env.write_config("malformed.json", "{ \"links\": [ not json");
    env.links = Some(bad);
    expect_failure(&env);
    let empty = env.write_config("empty-list.json", r#"{ "links": [] }"#);
    env.links = Some(empty);
    expect_failure(&env);
}

// ---------------------------------------------------------------------------
// unknown keys, the telemetry dict limit, delay statistics off
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_key_in_the_links_file_is_a_clean_exit_naming_it() {
    require_cpus(2);
    let _g = serial();
    let cfg = || config_json(0, 0.0);

    // a typo of an optional link key (`lcores` for `lcore`)
    let typo = link_json_extra("l", "net_null0", "net_null0", &cfg(), r#""lcores": [1]"#);
    let out = links_failure(&[typo], 1, "0-1");
    assert!(out.contains("lcores"), "link level\n{out}");

    // a typo in the histogram object
    let hist = link_json_hist(
        "l",
        "net_null0",
        "net_null0",
        &cfg(),
        r#"{ "bins": 10, "lo": 5 }"#,
    );
    let out = links_failure(&[hist], 1, "0-1");
    assert!(out.contains("lo"), "histogram level\n{out}");
    assert!(
        out.to_lowercase().contains("unknown"),
        "serde's wording\n{out}"
    );

    // a typo at the top level
    let mut env = links_env("0-1", &[link_json("l", "net_null0", "net_null0", &cfg())]);
    env.vdevs.push("net_null0".into());
    let bad = env.write_config(
        "top-level-typo.json",
        &format!(
            r#"{{ "links": [{}], "lnks": [] }}"#,
            link_json("l", "net_null0", "net_null0", &cfg())
        ),
    );
    env.links = Some(bad);
    let out = expect_failure(&env);
    assert!(out.contains("lnks"), "top level\n{out}");
}

#[test]
fn a_links_file_over_the_telemetry_dict_limit_is_a_clean_exit_naming_the_maximum() {
    // 17 hairpin links = 17 ports = 17 links: 3 + 5 * 17 + 10 * 17 = 258 keys in
    // /dpds/ports, over DPDK's 256. The file is validated before any port is probed
    // or the lcores are counted, so one vdev and `-l 0-1` do not matter.
    require_cpus(2);
    let _g = serial();
    let links: Vec<String> = (0..17)
        .map(|i| {
            link_json(
                &format!("l{i}"),
                &format!("net_null{i}"),
                &format!("net_null{i}"),
                &config_json(0, 0.0),
            )
        })
        .collect();
    let out = links_failure(&links, 1, "0-1");
    assert!(out.contains("256"), "the maximum is named\n{out}");

    // 13 one-way links over 26 ports: 3 + 5 * 26 + 10 * 13 = 263 keys. (12 one-way
    // links, 3 + 120 + 120 = 243, fit: unit/links.rs.)
    let links: Vec<String> = (0..13)
        .map(|i| {
            link_json(
                &format!("w{i}"),
                &format!("net_null{}", 2 * i),
                &format!("net_null{}", 2 * i + 1),
                &config_json(0, 0.0),
            )
        })
        .collect();
    let out = links_failure(&links, 1, "0-1");
    assert!(out.contains("256"), "the maximum is named\n{out}");
}

#[test]
fn delay_stats_off_in_the_links_file_still_forwards_and_counts() {
    // link 0 (pcap0 -> pcap1, delay_stats off) and link 1 (pcap1 -> pcap0, on):
    // both forward every frame; with telemetry only link 1 has delay statistics.
    const N: [u32; 2] = [200, 150];
    require_cpus(3);
    let _g = serial();
    let mut env = links_env(
        "0-2",
        &[
            link_json_delay_stats(
                "a-to-b",
                "net_pcap0",
                "net_pcap1",
                &config_json(1_000_000, 0.0),
                false,
            ),
            link_json(
                "b-to-a",
                "net_pcap1",
                "net_pcap0",
                &config_json(2_000_000, 0.0),
            ),
        ],
    );
    for (i, n) in N.iter().enumerate() {
        add_pcap_port(&mut env, i, &frames(i as u8, *n));
    }
    let outs: Vec<PathBuf> = (0..2).map(|i| pcap_out(&env, i)).collect();
    let mut run = env.spawn();
    wait_frames(&mut run, &outs[1], N[0] as usize);
    wait_frames(&mut run, &outs[0], N[1] as usize);
    thread::sleep(Duration::from_millis(700));

    #[cfg(feature = "telemetry")]
    {
        const DELAY_KEYS: [&str; 6] = [
            "delay_mean_ns",
            "delay_std_ns",
            "delay_min_ns",
            "delay_max_ns",
            "lateness_mean_ns",
            "lateness_max_ns",
        ];
        let p = run.query("/dpds/ports");
        assert_eq!(u(&p, "dir0_delay_stats"), 0, "{p}");
        assert_eq!(u(&p, "dir1_delay_stats"), 1, "{p}");

        // counters and drop causes are there for both
        for (d, &n) in N.iter().enumerate() {
            let st = run.query(&format!("/dpds/stats,{d}"));
            assert_eq!(u(&st, "rx_pkts"), n as u64, "dir {d}: {st}");
            assert_eq!(u(&st, "tx_sent"), n as u64, "dir {d}: {st}");
            for k in [
                "dpds_rejects",
                "loss_drops",
                "limiter_drops",
                "buffer_full_drops",
            ] {
                assert_eq!(u(&st, k), 0, "dir {d} {k}: {st}");
            }
        }
        // dir 0: no delay keys at all; dir 1: all six, around its 2 ms
        let st0 = run.query("/dpds/stats,0");
        for k in DELAY_KEYS {
            assert!(st0.get(k).is_none(), "dir 0 must not report {k}: {st0}");
        }
        let st1 = run.query("/dpds/stats,1");
        for k in DELAY_KEYS {
            assert!(st1.get(k).is_some(), "dir 1 reports {k}: {st1}");
        }
        let mean = num(&st1, "delay_mean_ns");
        assert!((1_999_999.0..=2_000_000.0).contains(&mean), "{mean}");

        // the histogram of dir 1 has its counts, the one of dir 0 is empty
        let h1 = run.query("/dpds/delay_hist,1");
        let total =
            arr_u(&h1, "bins").iter().sum::<u64>() + u(&h1, "underflow") + u(&h1, "overflow");
        assert_eq!(total, N[1] as u64, "{h1}");
        match run.try_query("/dpds/delay_hist,0") {
            None => {} // an error reply is an empty histogram too
            Some(h0) => {
                assert!(
                    h0.get("bins")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty),
                    "dir 0 has no bins: {h0}"
                );
                for k in ["num_bins", "underflow", "overflow"] {
                    assert!(
                        h0.get(k).and_then(Value::as_u64).is_none_or(|n| n == 0),
                        "dir 0 {k}: {h0}"
                    );
                }
            }
        }
    }

    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
    assert!(read_pcap(&outs[1]) == frames(0, N[0]), "link 0");
    assert!(read_pcap(&outs[0]) == frames(1, N[1]), "link 1");
}

/// Chain a -> b -> c over net_null ports: port 0 is ingress-only, port 1 both,
/// port 2 egress-only. Only the link that transmits on the egress-only port
/// (b-to-c) has a pool above the plain formula, by that port's RX ring; the
/// link a-to-b (egress port 1 has an ingress link) keeps the plain formula.
#[cfg(feature = "telemetry")]
#[test]
fn the_pool_exceeds_the_plain_formula_only_for_the_link_into_an_egress_only_port() {
    require_cpus(3);
    let _g = serial();
    let mut env = links_env(
        "0-2",
        &[
            link_json(
                "a-to-b",
                "net_null0",
                "net_null1",
                &config_json_cap(0, 0.0, 1000),
            ),
            link_json(
                "b-to-c",
                "net_null1",
                "net_null2",
                &config_json_cap(0, 0.0, 2000),
            ),
        ],
    );
    for i in 0..3 {
        env.vdevs.push(format!("net_null{i}"));
    }
    let mut run = env.spawn();
    let p = run.query("/dpds/ports");
    let ring = |port: u64, dir: &str| u(&p, &format!("port{port}_{dir}_ring"));
    let id = |key: &str| u(&p, key);
    let (a_rx, a_tx) = (id("dir0_rx_port"), id("dir0_tx_port"));
    let (b_rx, b_tx) = (id("dir1_rx_port"), id("dir1_tx_port"));
    assert_eq!(
        u(&p, "dir0_pool_size"),
        1000 + ring(a_rx, "rx") + ring(a_tx, "tx") + 832,
        "plain formula for a-to-b: {p}"
    );
    assert_eq!(
        u(&p, "dir1_pool_size"),
        2000 + ring(b_rx, "rx") + ring(b_tx, "tx") + ring(b_tx, "rx") + 832,
        "b-to-c adds the RX ring of the egress-only port: {p}"
    );
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
}

#[cfg(feature = "telemetry")]
#[test]
fn delay_stats_default_to_on_and_ports_reports_the_switch_of_every_link() {
    require_cpus(3);
    let _g = serial();
    let mut env = links_env(
        "0-2",
        &[
            link_json("a", "net_null0", "net_null0", &config_json(0, 0.0)),
            link_json_delay_stats("b", "net_null1", "net_null1", &config_json(0, 0.0), false),
        ],
    );
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let mut run = env.spawn();
    let p = run.query("/dpds/ports");
    assert_eq!(u(&p, "dir0_delay_stats"), 1, "no `delay_stats` = on: {p}");
    assert_eq!(u(&p, "dir1_delay_stats"), 0, "{p}");
    // a dict of at most 256 entries
    assert!(p.as_object().unwrap().len() <= 256, "{p}");
    run.signal(libc::SIGINT);
    assert_clean_exit(run.wait_exit(), &run);
}

#[test]
fn a_bad_delay_stats_value_is_a_clean_exit() {
    require_cpus(2);
    let _g = serial();
    // the v1 per-direction array form is gone: `delay_stats` is one bool
    let out = links_failure(
        &[link_json_extra(
            "flaglink",
            "net_null0",
            "net_null0",
            &config_json(0, 0.0),
            r#""delay_stats": [true]"#,
        )],
        1,
        "0-1",
    );
    assert!(
        out.to_lowercase().contains("bool"),
        "a bool is expected\n{out}"
    );
}
