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
//! Network topology: direction `d` = traffic received on port `d`, handled by
//! its own worker lcore, its own config (`--config<d>`), its own mempool and
//! its own telemetry counters (`/dpds/stats,<d>`). With two ports frames read
//! from port 0 leave on port 1 and vice versa; with one port they leave on
//! the port they came in on. The two input files hold a different number of
//! frames so a mix-up cannot go unnoticed.
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
        let mut a = vec!["--config".into(), self.config.display().to_string()];
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

#[test]
fn one_port_with_one_lcore_is_rejected_naming_the_requirement() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::with_lcores("0", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert!(
        out.contains("1 port needs 2 lcores, e.g. -l 0-1"),
        "the requirement is named\n{out}"
    );
}

#[test]
fn one_port_with_three_lcores_is_rejected_there_is_one_worker_per_port() {
    require_cpus(3);
    let _g = serial();
    let mut env = Env::with_lcores("0-2", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let out = expect_failure(&env);
    assert!(
        out.contains("1 port needs 2 lcores, e.g. -l 0-1"),
        "the requirement is named\n{out}"
    );
    assert!(out.contains('3'), "the lcore count found is named\n{out}");
}

#[test]
fn two_ports_with_two_lcores_are_rejected_naming_the_requirement() {
    require_cpus(2);
    let _g = serial();
    let mut env = Env::with_lcores("0-1", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let out = expect_failure(&env);
    assert!(
        out.contains("2 ports need 3 lcores, e.g. -l 0-2"),
        "the requirement is named\n{out}"
    );
    assert!(out.contains('2'), "the lcore count found is named\n{out}");
}

#[test]
fn two_ports_with_four_lcores_are_rejected_too() {
    require_cpus(4);
    let _g = serial();
    let mut env = Env::with_lcores("0-3", &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    env.vdevs.push("net_null1".into());
    let out = expect_failure(&env);
    assert!(
        out.contains("2 ports need 3 lcores, e.g. -l 0-2"),
        "the requirement is named\n{out}"
    );
    assert!(out.contains('4'), "the lcore count found is named\n{out}");
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
