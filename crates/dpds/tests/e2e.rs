//! End-to-end tests: run the real `dpds` binary against DPDK virtual devices.
//!
//! Environment: DPDK 26.07 with the `net_pcap` and `net_null` PMDs, no root,
//! no hugepages (`--no-huge -m 256 --no-pci --in-memory`). Tests that need N
//! lcores use cores `0..N` (`-l 0-(N-1)`) and return early (printing a
//! "skipping" note) if the machine has fewer CPUs.
//!
//! Facts about DPDK's `net_pcap` PMD relied on here (verified against the
//! v26.07 driver source `drivers/net/pcap/pcap_ethdev.c` and with
//! `dpdk-testpmd`, see the report that accompanies these tests):
//! - `rx_pcap=<file>` is read once; at EOF `rx_burst` returns 0 forever
//!   (it only loops with the `infinite_rx=1` devarg, which is not used here);
//! - `tx_pcap=<file>` is created/truncated when the vdev is probed and is
//!   flushed after every `tx_burst`, so it can be polled while dpds runs;
//! - the link comes up on `rte_eth_dev_start`;
//! - `rte_eth_dev_stop` closes the dumper.
//!
//! Test protocol (exact-count oracle): write N frames per input port, run
//! dpds with zero loss / limiter off / a fixed 1 ms delay, wait until the
//! output pcap files hold all frames, linger 700 ms to catch duplicates,
//! send SIGINT, require exit status 0, then compare the output pcaps frame by
//! frame with the input and check the `-o` CSV counters.
//!
//! Network topology: with two ports the worker flips the port, so frames read
//! from port 0 leave on port 1 and vice versa. The two input files hold a
//! different number of frames so a mix-up cannot go unnoticed.
//!
//! Tests are serialised with a global lock (each spawns busy-polling lcores,
//! and EAL runtime directories are per `--file-prefix`, which is also unique
//! per test). Every child is killed on drop, and every wait has a timeout.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

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

/// Returns `true` (after printing a note) when the host has fewer than `n` CPUs.
fn skip_for_cpus(n: usize) -> bool {
    let have = thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1);
    if have < n {
        eprintln!("skipping: needs {n} CPUs, host has {have}");
        true
    } else {
        false
    }
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
// config / CSV helpers
// ---------------------------------------------------------------------------

fn config_json(delay_ns: u64, loss: f64) -> String {
    format!(
        r#"{{
    "buffer_capacity": 4096,
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

fn parse_csv(path: &Path) -> HashMap<String, u64> {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("stats CSV {} not written: {e}", path.display()));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "CSV must be header + one row:\n{text}");
    let header: Vec<&str> = lines[0].split(',').collect();
    let row: Vec<&str> = lines[1].split(',').collect();
    assert_eq!(header.len(), row.len(), "column count mismatch:\n{text}");
    header
        .iter()
        .zip(row)
        .map(|(h, v)| {
            (
                h.to_string(),
                v.parse::<u64>()
                    .unwrap_or_else(|_| panic!("non-numeric {h}={v}")),
            )
        })
        .collect()
}

fn sum_prefix(csv: &HashMap<String, u64>, prefix: &str) -> u64 {
    csv.iter()
        .filter(|(k, _)| k.starts_with(prefix))
        .map(|(_, v)| v)
        .sum()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// process harness
// ---------------------------------------------------------------------------

struct Run {
    child: Child,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
    status: Option<ExitStatus>,
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
    fn spawn(args: &[String]) -> Run {
        let mut child = Command::new(DPDS)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn dpds");
        let stdout = collect(child.stdout.take().unwrap());
        let stderr = collect(child.stderr.take().unwrap());
        Run {
            child,
            stdout,
            stderr,
            status: None,
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
    fn wait_until(&mut self, what: &str, timeout: Duration, mut cond: impl FnMut(&Run) -> bool) {
        let deadline = Instant::now() + timeout;
        loop {
            if cond(self) {
                return;
            }
            if let Some(st) = self.poll_exit() {
                // One last look: the condition may have become true right before exit.
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

    /// Latest `Total:` value of the dashboard's `RX:` line, if any was printed.
    fn rx_total(&self) -> Option<u64> {
        let out = strip_ansi(&self.stdout.lock().unwrap());
        out.lines()
            .filter(|l| l.starts_with("RX:"))
            .filter_map(|l| {
                let rest = l.split("Total:").nth(1)?;
                rest.trim().split(' ').next()?.parse::<u64>().ok()
            })
            .next_back()
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
    lcores: usize,
    vdevs: Vec<String>,
    config: PathBuf,
    csv: PathBuf,
    prefix: String,
}

impl Env {
    fn new(lcores: usize, config: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        fs::write(&config_path, config).unwrap();
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        Env {
            csv: dir.path().join("stats.csv"),
            config: config_path,
            prefix: format!("dpds-e2e-{}-{id}", std::process::id()),
            vdevs: Vec::new(),
            lcores,
            dir,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
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
            &format!("0-{}", self.lcores - 1),
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
        vec![
            "--config".into(),
            self.config.display().to_string(),
            "-o".into(),
            self.csv.display().to_string(),
            "--num-mbufs".into(),
            NUM_MBUFS.into(),
        ]
    }

    fn args(&self) -> Vec<String> {
        let mut a = self.eal_args();
        a.push("--".into());
        a.extend(self.app_args());
        a
    }

    fn spawn(&self) -> Run {
        Run::spawn(&self.args())
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

fn assert_final_output(run: &Run) {
    let out = run.output();
    assert!(
        out.contains("--- Emulator Stats (1s interval) ---"),
        "{out}"
    );
    assert!(out.contains("=== Final Stats ==="), "{out}");
    assert!(out.contains("=== Cumulative Burst Histograms ==="), "{out}");
    assert!(
        out.contains("RX burst/s:") && out.contains("TX burst/s:"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// exact-count oracle
// ---------------------------------------------------------------------------

fn exact_count(lcores: usize, in_frames: &[Vec<Vec<u8>>]) {
    if skip_for_cpus(lcores) {
        return;
    }
    let _g = serial();
    let ports = in_frames.len();
    let mut env = Env::new(lcores, &config_json(1_000_000, 0.0));
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
    run.signal(libc::SIGINT);
    let st = run.wait_exit();
    assert_clean_exit(st, &run);
    assert_final_output(&run);

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

    let csv = parse_csv(&env.csv);
    let total = total as u64;
    assert_eq!(csv["rx_received"], total);
    assert_eq!(csv["worker_received"], total);
    assert_eq!(csv["worker_sent"], total);
    assert_eq!(csv["tx_transmitted"], total);
    assert_eq!(csv["rx_ring_drops"], 0);
    assert_eq!(csv["worker_emulator_drops"], 0);
    assert_eq!(csv["worker_tx_ring_drops"], 0);
    assert_eq!(csv["tx_drops"], 0);
    assert!(sum_prefix(&csv, "rx_burst_") >= 1);
    assert!(sum_prefix(&csv, "tx_burst_") >= 1);
    for p in 0..ports {
        for c in ["imissed", "ierrors", "oerrors", "rx_nombuf"] {
            assert_eq!(csv[&format!("nic_{p}_{c}")], 0, "nic_{p}_{c}");
        }
    }
    assert!(!csv.contains_key(&format!("nic_{ports}_imissed")));
}

#[test]
fn pcap_one_port_two_lcores_combined() {
    exact_count(2, &[frames(0, 1000)]);
}

#[test]
fn pcap_one_port_three_lcores_rx_plus_worker_tx() {
    exact_count(3, &[frames(0, 1000)]);
}

#[test]
fn pcap_one_port_four_lcores_separate_stages() {
    exact_count(4, &[frames(0, 1000)]);
}

#[test]
fn pcap_two_ports_two_lcores_combined() {
    exact_count(2, &[frames(0, 1000), frames(1, 600)]);
}

#[test]
fn pcap_two_ports_three_lcores_rx_plus_worker_tx() {
    exact_count(3, &[frames(0, 1000), frames(1, 600)]);
}

#[test]
fn pcap_two_ports_four_lcores_separate_stages() {
    exact_count(4, &[frames(0, 1000), frames(1, 600)]);
}

#[test]
fn pcap_one_port_input_of_exactly_one_burst_and_less_than_one_burst() {
    // Boundary sizes around MAX_BURST_SIZE = 16: 1, 15, 16, 17 frames.
    for n in [1u32, 15, 16, 17] {
        exact_count(3, &[frames(0, n)]);
    }
}

#[test]
fn pcap_two_ports_one_input_empty() {
    // Port 1 has nothing to send; everything from port 0 must leave on port 1.
    exact_count(3, &[frames(0, 300), Vec::new()]);
}

// ---------------------------------------------------------------------------
// loss
// ---------------------------------------------------------------------------

#[test]
fn full_loss_transmits_nothing_and_counts_emulator_drops() {
    const N: u32 = 500;
    if skip_for_cpus(3) {
        return;
    }
    let _g = serial();
    let mut env = Env::new(3, &config_json(0, 1.0));
    add_pcap_port(&mut env, 0, &frames(0, N));
    let mut run = env.spawn();

    // The dashboard (printed once a second) tells us when RX has read the file.
    run.wait_until("RX to read the input pcap", START_TIMEOUT, |r| {
        r.rx_total() == Some(N as u64)
    });
    run.signal(libc::SIGINT);
    let st = run.wait_exit();
    assert_clean_exit(st, &run);

    assert_eq!(read_pcap(&env.path("out0.pcap")).len(), 0);
    let csv = parse_csv(&env.csv);
    assert_eq!(csv["rx_received"], N as u64);
    assert_eq!(csv["worker_received"], N as u64);
    assert_eq!(csv["worker_emulator_drops"], N as u64);
    assert_eq!(csv["worker_sent"], 0);
    assert_eq!(csv["tx_transmitted"], 0);
}

// ---------------------------------------------------------------------------
// ordered shutdown: packets still delayed in the buffer must be flushed
// ---------------------------------------------------------------------------

#[test]
fn sigint_drains_packets_still_held_in_the_delay_buffer() {
    const N: u32 = 300;
    for lcores in [2usize, 4] {
        if skip_for_cpus(lcores) {
            continue;
        }
        let _g = serial();
        // 3 s delay: when the first dashboard tick (~1 s after start) shows
        // all packets received, none of them can have been transmitted yet.
        let mut env = Env::new(lcores, &config_json(3_000_000_000, 0.0));
        add_pcap_port(&mut env, 0, &frames(0, N));
        let mut run = env.spawn();

        run.wait_until("RX to read the input pcap", START_TIMEOUT, |r| {
            r.rx_total() == Some(N as u64)
        });
        let early = read_pcap(&env.path("out0.pcap")).len();
        assert_eq!(
            early, 0,
            "{lcores} lcores: packets left before their delay elapsed"
        );
        run.signal(libc::SIGINT);
        let st = run.wait_exit();
        assert_clean_exit(st, &run);

        let got = read_pcap(&env.path("out0.pcap"));
        assert_eq!(
            got,
            frames(0, N),
            "{lcores} lcores: shutdown lost or reordered delayed packets"
        );
        let csv = parse_csv(&env.csv);
        assert_eq!(csv["tx_transmitted"], N as u64, "{lcores} lcores");
    }
}

// ---------------------------------------------------------------------------
// net_null smoke tests (line-rate packet source, packets are just freed)
// ---------------------------------------------------------------------------

fn null_smoke(lcores: usize, ports: usize, sig: i32) {
    if skip_for_cpus(lcores) {
        return;
    }
    let _g = serial();
    let mut env = Env::new(lcores, &config_json(0, 0.0));
    for p in 0..ports {
        env.vdevs.push(format!("net_null{p}"));
    }
    let mut run = env.spawn();

    // Two dashboard ticks with traffic flowing.
    run.wait_until("RX traffic on the dashboard", START_TIMEOUT, |r| {
        r.rx_total().is_some_and(|t| t > 0)
    });
    let first = run.rx_total().unwrap();
    run.wait_until("RX total to keep growing", START_TIMEOUT, |r| {
        r.rx_total().is_some_and(|t| t > first)
    });
    assert!(
        run.poll_exit().is_none(),
        "dpds died while running\n{}",
        run.output()
    );

    run.signal(sig);
    let st = run.wait_exit();
    assert_clean_exit(st, &run);
    assert_final_output(&run);

    let csv = parse_csv(&env.csv);
    let rx = csv["rx_received"];
    assert!(rx > 0);
    assert!(csv["tx_transmitted"] > 0, "nothing transmitted");
    // Every packet is accounted for after the ordered shutdown.
    assert_eq!(
        csv["worker_received"] + csv["rx_ring_drops"],
        rx,
        "packets lost between RX and worker"
    );
    assert_eq!(
        csv["worker_sent"] + csv["worker_emulator_drops"] + csv["worker_tx_ring_drops"],
        csv["worker_received"],
        "worker packets unaccounted for"
    );
    assert_eq!(
        csv["tx_transmitted"] + csv["tx_drops"],
        csv["worker_sent"],
        "packets lost between worker and TX"
    );
    assert!(sum_prefix(&csv, "rx_burst_") >= 1);
    assert!(sum_prefix(&csv, "tx_burst_") >= 1);
    for p in 0..ports {
        assert!(csv.contains_key(&format!("nic_{p}_imissed")));
    }
}

#[test]
fn null_two_lcores_one_port_sigint() {
    null_smoke(2, 1, libc::SIGINT);
}

#[test]
fn null_three_lcores_one_port_sigint() {
    null_smoke(3, 1, libc::SIGINT);
}

#[test]
fn null_four_lcores_one_port_sigint() {
    null_smoke(4, 1, libc::SIGINT);
}

#[test]
fn null_four_lcores_two_ports_sigint() {
    null_smoke(4, 2, libc::SIGINT);
}

#[test]
fn null_two_lcores_two_ports_sigterm() {
    null_smoke(2, 2, libc::SIGTERM);
}

// ---------------------------------------------------------------------------
// error paths: must exit with a status code (no panic/abort, no hang)
// ---------------------------------------------------------------------------

fn expect_failure(env: &Env, args: Vec<String>) -> String {
    let mut run = Run::spawn(&args);
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
    assert!(
        !env.csv.exists(),
        "no stats file may be written on a failed start"
    );
    out
}

#[test]
fn one_lcore_is_rejected() {
    let _g = serial();
    let mut env = Env::new(1, &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    expect_failure(&env, env.args());
}

#[test]
fn five_lcores_are_rejected() {
    if skip_for_cpus(5) {
        return;
    }
    let _g = serial();
    let mut env = Env::new(5, &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    expect_failure(&env, env.args());
}

#[test]
fn zero_ports_are_rejected() {
    if skip_for_cpus(2) {
        return;
    }
    let _g = serial();
    let env = Env::new(2, &config_json(0, 0.0));
    expect_failure(&env, env.args());
}

#[test]
fn three_ports_are_rejected() {
    if skip_for_cpus(2) {
        return;
    }
    let _g = serial();
    let mut env = Env::new(2, &config_json(0, 0.0));
    for p in 0..3 {
        env.vdevs.push(format!("net_null{p}"));
    }
    expect_failure(&env, env.args());
}

#[test]
fn unknown_app_argument_is_rejected_with_usage() {
    if skip_for_cpus(2) {
        return;
    }
    let _g = serial();
    let mut env = Env::new(2, &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    let mut args = env.args();
    args.push("--bogus".into());
    let out = expect_failure(&env, args);
    assert!(out.contains("Usage"), "usage message expected\n{out}");
}

#[test]
fn missing_config_file_is_an_error_exit_not_a_crash() {
    if skip_for_cpus(2) {
        return;
    }
    let _g = serial();
    let mut env = Env::new(2, &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    fs::remove_file(&env.config).unwrap();
    expect_failure(&env, env.args());
}

#[test]
fn malformed_config_file_is_an_error_exit_not_a_crash() {
    let _g = serial();
    for lcores in [2usize, 3, 4] {
        if skip_for_cpus(lcores) {
            continue;
        }
        let mut env = Env::new(lcores, "{ definitely not json");
        env.vdevs.push("net_null0".into());
        expect_failure(&env, env.args());
    }
}

#[test]
fn default_config_path_is_config_json_in_the_working_directory() {
    if skip_for_cpus(2) {
        return;
    }
    let _g = serial();
    let mut env = Env::new(2, &config_json(0, 0.0));
    env.vdevs.push("net_null0".into());
    // No `--config`: dpds must pick up ./config.json from its cwd.
    let mut args = env.eal_args();
    args.push("--".into());
    args.extend(["-o".into(), env.csv.display().to_string()]);
    args.extend(["--num-mbufs".into(), NUM_MBUFS.into()]);

    let mut child = Command::new(DPDS)
        .args(&args)
        .current_dir(env.dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = collect(child.stdout.take().unwrap());
    let stderr = collect(child.stderr.take().unwrap());
    let mut run = Run {
        child,
        stdout,
        stderr,
        status: None,
    };
    run.wait_until("RX traffic on the dashboard", START_TIMEOUT, |r| {
        r.rx_total().is_some_and(|t| t > 0)
    });
    run.signal(libc::SIGINT);
    let st = run.wait_exit();
    assert_clean_exit(st, &run);
    assert!(parse_csv(&env.csv)["tx_transmitted"] > 0);
}
