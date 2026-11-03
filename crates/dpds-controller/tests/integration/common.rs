//! Shared test support for `dpds-controller` (included with `mod common;`).
//!
//! # Test-facing Rust API the tests assume (library crate `dpds_controller`)
//!
//! One integration-test binary (`tests/integration/main.rs`), submodules per topic; no lint
//! suppressions, every helper here is used by some test.
//!
//! The tests were written to spec v5 Part B; everything below that the spec
//! does not fix is an ASSUMPTION of the tester, listed here so the implementer
//! can follow it (or tell the coordinator which choice should change).
//! Types returned to tests are inspected through `serde_json::to_value`
//! wherever possible, so field types matter less than JSON names.
//!
//! All types below derive `Debug` and `Clone` unless noted (DirectionPlan, LaunchPlan, State, ...).
//!
//! Binary: `dpds-controller serve|check`; `check` accepts `--sysfs-root`, `--procfs-root`,
//! `--state-dir` (missing dir = no links) and `--json`; text mode prints one line per hint
//! containing the upper-case level (`OK`/`INFO`/`WARN`/`ERROR`) and the hint title; exit 1
//! iff any hint has level error. `serve` also takes `--bind --state-dir --dpds-bin --runtime-dir
//! --sysfs-root --procfs-root`; it sets RUNTIME_DIRECTORY (DPDK's runtime dir) to the runtime dir.
//!
//! ```ignore
//! pub mod host {        // roots of the host files, so tests can point at fake trees
//!     pub struct HostRoots { pub sysfs: PathBuf, pub procfs: PathBuf }
//! }
//! pub mod model {       // stored state (B1); all: Clone + Debug + Default(where stated) + Serialize + Deserialize
//!     // spec v10 (directed links): ONE direction per link, ingress port -> egress port.
//!     // The tests build `Link` through serde (see `link()` below), so only the JSON names matter.
//!     pub struct Link { pub id: u32, pub name: String, pub ingress: String, pub egress: String,
//!                       pub config: ConfigDto, pub worker: Option<u32> }
//!     // spec v17: no `mlx5_devargs` (fixed to PREOF's `txq_mpw_en=1`); the body is `deny_unknown_fields`
//!     pub struct Settings { pub main_lcore: Option<u32>,
//!                           pub eal_extra_args: Vec<String>, pub poll_interval_ms: u64 }
//!     // Default: None, [], 500
//!     pub struct StoredState { pub links: Vec<Link>, pub settings: Settings, pub next_id: u32 } // Default
//! }
//! pub mod dto {         // ConfigDto: named-field mirror of dpds_core::Config
//!     // JSON: {buffer_capacity, reordering, delayer{distribution,mean_ns,std_ns,
//!     //   activate_correlation,half_life_period_ns,initialization_rate_pps}, limiter{mode,activate,rate_bps,limit_bit},
//!     //   loss_model: {"type":"Random","p":f} | {"type":"GilbertElliot","p","r","k","h",
//!     //   "state_is_good": bool (optional, default true)}, seed: u64|null (optional)}
//!     // distribution / mode are the dpds_core enum variant names as strings.
//!     pub struct ConfigDto;  // Serialize + Deserialize + Clone + Debug
//!     pub struct DtoError;   // Display
//!     impl ConfigDto {
//!         pub fn to_core(&self) -> Result<dpds_core::Config, DtoError>; // validates (Buffer::try_new on a ZERO-capacity copy)
//!         pub fn from_core(c: &dpds_core::Config) -> ConfigDto;
//!     }
//! }
//! pub mod store {       // B1 persistence
//!     pub const STATE_FILE_NAME: &str = "state.json"; // {"links":[..],"settings":{..},"next_id":n}
//!     pub struct StoreError;                           // Display, names the file path
//!     pub fn load(dir: &Path) -> Result<StoredState, StoreError>; // missing dir/file => Ok(default); corrupt => Err
//!     pub fn save(dir: &Path, s: &StoredState) -> Result<(), StoreError>; // creates dir, temp file + rename
//! }
//! pub mod discovery {   // B5
//!     pub struct Interface; // Serialize; JSON keys: pci, vendor_id ("0x15b3"), device_id, vendor, driver,
//!                           // netdev, mac, operstate, speed_mbps, mtu, numa_node, pcie_link{speed,width,
//!                           // max_speed,max_width}, kernel_route, dpds_usable, reason  (absent data => null)
//!     pub fn discover(roots: &HostRoots) -> Vec<Interface>;     // sorted by pci address
//! }
//! pub mod hints {       // B6
//!     pub struct Hint;      // Serialize: {id, level: "ok"|"info"|"warn"|"error", title, detail, suggestion}
//!     pub fn compute_hints(roots: &HostRoots, state: &StoredState) -> Vec<Hint>;
//!     // ALWAYS exactly one hint per id (more ids may exist):
//!     //   isolation, isolated_cores, hugepages, numa, governor, irqbalance, iommu, mtu,
//!     //   pcie_maxreadreq, mprq
//! }
//! pub mod lcores {      // B7
//!     pub struct DirectionPlan { pub dir: u32, pub link: String, pub rx: String, pub tx: String, pub lcore: u32 }
//!     //   Serialize with exactly those keys (also used in /api/emulation `directions`)
//!     pub struct Assignment { pub main_lcore: u32, pub directions: Vec<DirectionPlan> }
//!     pub struct LcoreError; // Display
//!     pub fn assign_lcores(roots: &HostRoots, state: &StoredState) -> Result<Assignment, LcoreError>;
//!     // spec v10: one direction per link, `dir` = link index in stored order, rx = ingress, tx = egress,
//!     // lcore = the link's `worker` override or a free isolated core, preferring the NUMA node of the
//!     // INGRESS NIC. Assignments are literal and never re-sorted. The links file carries them explicitly.
//! }
//! pub mod eal {         // B3 pure functions
//!     pub struct EalPort { pub name: String, pub driver: Option<String> } // driver = kernel driver of a PCI port
//!     pub struct EalInput { pub main_lcore: u32, pub worker_lcores: Vec<u32>, pub ports: Vec<EalPort>,
//!                           pub eal_extra_args: Vec<String>, pub links_file: PathBuf }
//!     // spec v17: the argv appends `,txq_mpw_en=1` to every PCI port bound to mlx5_core, nothing else
//!     pub fn eal_argv(i: &EalInput) -> Vec<String>;               // argv WITHOUT the binary name
//!     // links file v2: {"links":[{"name","ingress","egress","config":{..},"lcore":n,
//!     //   "histogram"?:{..}, "delay_stats"?:false}]}: `lcore` of link L = the `lcore` of the entry of
//!     //   `directions` whose `link == L.name`. `delay_stats` only when off, `histogram` only when set.
//!     pub fn links_file_json(links: &[Link], directions: &[DirectionPlan]) -> Result<serde_json::Value, DtoError>;
//!     pub fn drain_timeout(configs: &[dpds_core::Config]) -> Duration;
//! }
//! pub mod stats {       // B2/B4 pure maths on telemetry snapshots (raw telemetry JSON objects)
//!     #[derive(Clone, Default)]
//!     pub struct Snapshot { pub ports: Value,      // reply of /dpds/ports
//!                           pub dirs: Vec<Value>,  // dirs[d]  = reply of /dpds/stats,<d>
//!                           pub eth: Vec<Value>,   // eth[p]   = reply of /ethdev/stats,<p> (p = DPDK port id)
//!                           pub hists: Vec<Value> }// hists[d] = reply of /dpds/delay_hist,<d>
//!     pub struct StatsView;     // Serialize, see tests/stats.rs for the JSON
//!     pub fn compute_stats(prev: Option<&Snapshot>, cur: &Snapshot) -> StatsView;
//!     pub struct HistogramView; // Serialize {dir, lo_ns, bin_ns, bins, underflow, overflow, edges_ns}
//!     pub fn delay_histogram(cur: &Snapshot, dir: usize) -> Option<HistogramView>;
//! }
//! pub mod telemetry {
//!     pub struct Polled { pub prev: Option<Snapshot>, pub cur: Snapshot }   // Clone
//!     pub trait TelemetrySource: Send + Sync + 'static { fn latest(&self) -> Option<Polled>; }
//! }
//! pub mod supervisor {
//!     pub enum State { Stopped, Starting, Running, Stopping, Failed }  // Copy, Debug, PartialEq, serde lowercase
//!     pub struct LaunchPlan { pub dpds_bin: PathBuf, pub runtime_dir: PathBuf, pub links_file: PathBuf, // runtime_dir/links.json
//!                             pub links_json: Value, pub argv: Vec<String>, pub directions: Vec<DirectionPlan>,
//!                             pub main_lcore: u32, pub drain_timeout: Duration }   // Clone
//!     pub struct SupervisorStatus { pub state: State, pub pid: Option<u32>, pub started_at: Option<u64> /*unix s*/,
//!                                   pub error: Option<String>, pub log_tail: Vec<String> }   // Clone
//!     pub trait Supervisor: Send + Sync + 'static {
//!         fn start(&self, plan: LaunchPlan);   // never blocks; failures surface as state Failed
//!         fn stop(&self);                      // never blocks
//!         fn status(&self) -> SupervisorStatus;
//!     }
//!     pub struct ProcessSupervisor;            // the real one
//!     impl ProcessSupervisor { pub fn new(start_timeout: Duration) -> Self }
//! }
//! pub mod api {
//!     pub struct Deps { pub state_dir: PathBuf, pub roots: HostRoots, pub supervisor: Arc<dyn Supervisor>,
//!                       pub telemetry: Arc<dyn TelemetrySource>, pub dpds_bin: PathBuf, pub runtime_dir: PathBuf,
//!                       pub dpds_has_telemetry: bool, pub is_root: bool }
//!     pub fn router(deps: Deps) -> Result<axum::Router, StoreError>;  // routes under /api, Swagger at /api/docs,
//!                                                                    // OpenAPI JSON at /api/openapi.json
//! }
//! ```
//!
//! # Spec v12 additions
//!
//! - `delayer.initialization_rate_pps` is REQUIRED in every `ConfigDto` (no default, serde error
//!   naming the field -> 400), validated like `dpds_core` does (finite, >= 1, else 400 naming the
//!   field), stored, exported/imported, part of presets and written into the links file.
//! - `launch_plan` re-validates link names (the create rules: 1..=32 characters of `A-Za-z0-9_.-`,
//!   unique) and the dpds telemetry dict limit `3 + 5 * distinct ports + 10 * links <= 256`
//!   (the message names `256`); the same limit is checked on create, update and import.
//! - `check_version`: for a version other than 2 the message names the supported version 2; only
//!   for version 1 it keeps the hint about the old paired links.
//! - OpenAPI `worker` of `Link`, `LinkInput` and `ExportLink` has `minimum: 0`.
//!
//! # HTTP contract the API tests assume (beyond the spec text)
//!
//! - 200 for GET/PUT, 201 for POST /api/links, 204 for DELETE /api/links/{id}.
//! - POST /api/emulation/start and /stop: 202 with the `/api/emulation` body.
//!   Start with no links or a failing lcore assignment: 400. Start while
//!   starting/running/stopping, stop while stopped/failed: 409.
//! - Validation error: 400, conflict (duplicate link name, port already used as ingress (or as
//!   egress) by another link; the message names that link): 409, unknown id / unknown direction: 404. Malformed or
//!   incomplete JSON bodies: 400 or 422. Every error body is `{"message": str}`.
//! - Settings: `poll_interval_ms == 0` => 400; a body with `mlx5_devargs` (any value) => 400
//!   naming the field (spec v17).
//! - `/api/stats` with no telemetry data: 200 `{"directions":[],"ports":[]}`;
//!   histogram for a direction without data: 404.
//! - Link ids are monotonically increasing and never reused after DELETE.
//! - `pending_restart`: true after any link/settings edit while the state is
//!   starting/running; false after the next start and while stopped.
//! - A stopped controller with stored links reports the planned `directions` and
//!   `main_lcore` in `/api/emulation` (empty list when nothing is planned
//!   or the assignment fails).
//!
//! The shared fake sysfs/procfs trees are generated by [`FakeHost`] (code, not
//! checked-in files, so every test can vary one knob).

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tempfile::TempDir;

use dpds_controller::api::{self, Deps};
use dpds_controller::dto::ConfigDto;
use dpds_controller::host::HostRoots;
use dpds_controller::model::{Link, Settings, StoredState};
use dpds_controller::stats::{Polled, Snapshot};
use dpds_controller::supervisor::{LaunchPlan, State, Supervisor, SupervisorStatus};
use dpds_controller::telemetry::TelemetrySource;

// ---------------------------------------------------------------------------
// fake sysfs / procfs
// ---------------------------------------------------------------------------

/// One PCI network (or other) device.
#[derive(Clone)]
pub struct Nic {
    pub pci: String,
    pub vendor: String,
    pub device: String,
    pub class: String,
    pub driver: Option<String>,
    pub netdev: Option<String>,
    pub mac: String,
    pub operstate: String,
    pub speed: Option<String>,
    pub mtu: Option<u32>,
    pub numa: Option<String>,
    /// (current speed, current width, max speed, max width) as sysfs prints them.
    pub pcie: Option<(String, u32, String, u32)>,
    /// MaxReadReq in bytes, written into a 256 byte fake `config`.
    pub max_read_req: Option<u32>,
    pub iommu_group: bool,
}

impl Nic {
    pub fn mlx5(pci: &str, netdev: &str, numa: Option<i32>) -> Nic {
        Nic {
            pci: pci.into(),
            vendor: "0x15b3".into(),
            device: "0x1017".into(),
            class: "0x020000".into(),
            driver: Some("mlx5_core".into()),
            netdev: Some(netdev.into()),
            mac: "aa:bb:cc:00:00:01".into(),
            operstate: "up".into(),
            speed: Some("100000".into()),
            mtu: Some(1500),
            numa: numa.map(|n| n.to_string()),
            pcie: Some(("16.0 GT/s PCIe".into(), 16, "16.0 GT/s PCIe".into(), 16)),
            max_read_req: Some(4096),
            iommu_group: false,
        }
    }
}

pub struct FakeHost {
    pub tmp: TempDir,
}

impl FakeHost {
    pub fn empty() -> FakeHost {
        let h = FakeHost {
            tmp: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(h.sysfs()).unwrap();
        fs::create_dir_all(h.procfs()).unwrap();
        h
    }

    pub fn sysfs(&self) -> PathBuf {
        self.tmp.path().join("sys")
    }
    pub fn procfs(&self) -> PathBuf {
        self.tmp.path().join("proc")
    }
    pub fn roots(&self) -> HostRoots {
        HostRoots {
            sysfs: self.sysfs(),
            procfs: self.procfs(),
        }
    }

    /// Writes `content` to `rel` (relative to the tempdir, e.g. `sys/...`).
    pub fn write(&self, rel: &str, content: &str) {
        let p = self.tmp.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    pub fn write_bytes(&self, rel: &str, content: &[u8]) {
        let p = self.tmp.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    pub fn mkdir(&self, rel: &str) {
        fs::create_dir_all(self.tmp.path().join(rel)).unwrap();
    }
    /// Removes a file or directory tree; missing is fine.
    pub fn remove(&self, rel: &str) {
        let p = self.tmp.path().join(rel);
        if p.is_dir() {
            fs::remove_dir_all(p).unwrap();
        } else {
            let _ = fs::remove_file(p);
        }
    }
    pub fn symlink(&self, target: &str, rel: &str) {
        let p = self.tmp.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let _ = fs::remove_file(&p);
        std::os::unix::fs::symlink(target, p).unwrap();
    }

    /// `ncpu` online CPUs, the `isolated` file (None = file absent) and NUMA
    /// nodes `(id, cpulist)` (empty = no `node` directory at all, like a VM).
    pub fn cpus(&self, ncpu: u32, isolated: Option<&str>, nodes: &[(u32, &str)]) {
        let base = "sys/devices/system/cpu";
        self.write(&format!("{base}/online"), &format!("0-{}\n", ncpu - 1));
        self.write(&format!("{base}/possible"), &format!("0-{}\n", ncpu - 1));
        for i in 0..ncpu {
            self.mkdir(&format!("{base}/cpu{i}"));
        }
        self.remove(&format!("{base}/isolated"));
        if let Some(iso) = isolated {
            self.write(&format!("{base}/isolated"), &format!("{iso}\n"));
        }
        self.remove("sys/devices/system/node");
        for (id, list) in nodes {
            self.write(
                &format!("sys/devices/system/node/node{id}/cpulist"),
                &format!("{list}\n"),
            );
        }
        if !nodes.is_empty() {
            self.write(
                "sys/devices/system/node/online",
                &format!("0-{}\n", nodes.len() - 1),
            );
        }
    }

    pub fn cmdline(&self, line: &str) {
        self.write("proc/cmdline", &format!("{line}\n"));
    }

    pub fn governor_all(&self, ncpu: u32, governor: &str) {
        for i in 0..ncpu {
            self.write(
                &format!("sys/devices/system/cpu/cpu{i}/cpufreq/scaling_governor"),
                &format!("{governor}\n"),
            );
        }
    }

    /// 2 MiB hugepages: `total`/`free` PER NODE for each node in `nodes`
    /// (written below `node<N>/hugepages/`), the global files and
    /// `/proc/meminfo` get the sum. With no nodes, `total`/`free` are global.
    pub fn hugepages(&self, nodes: &[u32], total: u64, free: u64) {
        let n = nodes.len().max(1) as u64;
        for node in nodes {
            let d = format!("sys/devices/system/node/node{node}/hugepages/hugepages-2048kB");
            self.write(&format!("{d}/nr_hugepages"), &format!("{total}\n"));
            self.write(&format!("{d}/free_hugepages"), &format!("{free}\n"));
        }
        let (gt, gf) = (total * n, free * n);
        let d = "sys/kernel/mm/hugepages/hugepages-2048kB";
        self.write(&format!("{d}/nr_hugepages"), &format!("{gt}\n"));
        self.write(&format!("{d}/free_hugepages"), &format!("{gf}\n"));
        self.write(
            "proc/meminfo",
            &format!(
                "MemTotal:       263939432 kB\nMemFree:        200000000 kB\n\
                 HugePages_Total:    {gt}\nHugePages_Free:     {gf}\nHugePages_Rsvd:        0\n\
                 HugePages_Surp:        0\nHugepagesize:       2048 kB\n"
            ),
        );
    }

    /// `/sys/class/iommu/dmar0` present or not.
    pub fn iommu(&self, enabled: bool) {
        self.remove("sys/class/iommu");
        self.mkdir("sys/class");
        if enabled {
            self.mkdir("sys/class/iommu/dmar0");
        } else {
            self.mkdir("sys/class/iommu");
        }
    }

    /// `/proc/<pid>/comm` entries.
    pub fn processes(&self, procs: &[(u32, &str)]) {
        for (pid, comm) in procs {
            self.write(&format!("proc/{pid}/comm"), &format!("{comm}\n"));
        }
    }

    /// Complete `/proc/net/route`: `(iface, destination_hex, gateway_hex, mask_hex)`.
    pub fn routes(&self, routes: &[(&str, &str, &str, &str)]) {
        let mut s = String::from(
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n",
        );
        for (i, d, g, m) in routes {
            s.push_str(&format!("{i}\t{d}\t{g}\t0003\t0\t0\t100\t{m}\t0\t0\t0\n"));
        }
        self.write("proc/net/route", &s);
    }

    /// Complete `/proc/net/ipv6_route`: `(iface, destination_hex32, prefix_len)`, in the
    /// kernel's format (dest, plen, src, plen, next hop, metric, refcnt, use, flags, iface).
    pub fn routes6(&self, routes: &[(&str, &str, u8)]) {
        let zero = "0".repeat(32);
        let mut s = String::new();
        for (iface, dest, plen) in routes {
            s.push_str(&format!(
                "{dest} {plen:02x} {zero} 00 {zero} 00000100 00000001 00000000 00000001 {iface:>8}\n"
            ));
        }
        self.write("proc/net/ipv6_route", &s);
    }

    /// A virtual netdev (bond, VLAN, bridge) without a PCI device.
    pub fn virtual_netdev(&self, name: &str) {
        let c = format!("sys/class/net/{name}");
        self.write(&format!("{c}/address"), "aa:bb:cc:00:00:99\n");
        self.write(&format!("{c}/operstate"), "up\n");
        self.write(&format!("{c}/mtu"), "1500\n");
    }

    /// `lower` has the upper device `upper` (`upper_<upper>` link on the lower, and
    /// `lower_<lower>` on the upper), like a VLAN or a bond slave.
    pub fn upper_link(&self, lower: &str, upper: &str) {
        self.virtual_netdev(upper);
        self.symlink(
            &format!("../{upper}"),
            &format!("sys/class/net/{lower}/upper_{upper}"),
        );
        self.symlink(
            &format!("../{lower}"),
            &format!("sys/class/net/{upper}/lower_{lower}"),
        );
    }

    /// `lower` is enslaved to `master` (`master` link on the lower, like a bond or bridge port).
    pub fn master_link(&self, lower: &str, master: &str) {
        self.virtual_netdev(master);
        self.symlink(
            &format!("../{master}"),
            &format!("sys/class/net/{lower}/master"),
        );
    }

    pub fn add_nic(&self, n: &Nic) {
        let dev = format!("sys/bus/pci/devices/{}", n.pci);
        self.write(&format!("{dev}/vendor"), &format!("{}\n", n.vendor));
        self.write(&format!("{dev}/device"), &format!("{}\n", n.device));
        self.write(&format!("{dev}/class"), &format!("{}\n", n.class));
        if let Some(numa) = &n.numa {
            self.write(&format!("{dev}/numa_node"), &format!("{numa}\n"));
        }
        if let Some(d) = &n.driver {
            self.mkdir(&format!("sys/bus/pci/drivers/{d}"));
            self.symlink(&format!("../../drivers/{d}"), &format!("{dev}/driver"));
        }
        if let Some((cs, cw, ms, mw)) = &n.pcie {
            self.write(&format!("{dev}/current_link_speed"), &format!("{cs}\n"));
            self.write(&format!("{dev}/current_link_width"), &format!("{cw}\n"));
            self.write(&format!("{dev}/max_link_speed"), &format!("{ms}\n"));
            self.write(&format!("{dev}/max_link_width"), &format!("{mw}\n"));
        }
        if n.iommu_group {
            self.mkdir("sys/kernel/iommu_groups/7");
            self.symlink(
                "../../../../kernel/iommu_groups/7",
                &format!("{dev}/iommu_group"),
            );
        }
        if let Some(mrr) = n.max_read_req {
            self.write_bytes(&format!("{dev}/config"), &config_space(mrr));
        }
        if let Some(nd) = &n.netdev {
            self.mkdir(&format!("{dev}/net/{nd}"));
            let c = format!("sys/class/net/{nd}");
            self.write(&format!("{c}/address"), &format!("{}\n", n.mac));
            self.write(&format!("{c}/operstate"), &format!("{}\n", n.operstate));
            if let Some(s) = &n.speed {
                self.write(&format!("{c}/speed"), &format!("{s}\n"));
            }
            if let Some(m) = n.mtu {
                self.write(&format!("{c}/mtu"), &format!("{m}\n"));
            }
            self.symlink(
                &format!("../../../bus/pci/devices/{}", n.pci),
                &format!("{c}/device"),
            );
        }
    }

    /// Rewrites only the driver of an already added NIC (e.g. bind to vfio-pci:
    /// the netdev disappears with it).
    pub fn rebind(&self, pci: &str, driver: &str, drop_netdev: bool) {
        let dev = format!("sys/bus/pci/devices/{pci}");
        self.mkdir(&format!("sys/bus/pci/drivers/{driver}"));
        self.symlink(&format!("../../drivers/{driver}"), &format!("{dev}/driver"));
        if drop_netdev {
            self.remove(&format!("{dev}/net"));
        }
    }

    /// 8 CPUs, 2 NUMA nodes (0-3 / 4-7), isolated 2-3,6-7 with the full
    /// boot-parameter set, performance governor, 2 x 2048 x 2 MiB hugepages
    /// free, IOMMU on, no irqbalance. NICs (sorted by PCI address):
    /// `0000:03:00.0` unknown vendor 0xbeef, `tg3`, `eno1`, NUMA 0, default
    /// route (management); `0000:17:00.0` Mellanox mlx5_core `ens1f0` NUMA 0;
    /// `0000:65:00.0` Mellanox mlx5_core `ens2f0` NUMA 1, MTU 9000;
    /// `0000:b3:00.0` Intel `ice` `enp179s0` NUMA 1, MaxReadReq 512 (so
    /// tests that do not use it never see it); plus an NVMe device that must
    /// not be listed.
    pub fn bare_metal() -> FakeHost {
        let h = FakeHost::empty();
        h.cpus(8, Some("2-3,6-7"), &[(0, "0-3"), (1, "4-7")]);
        h.cmdline(
            "BOOT_IMAGE=/vmlinuz-6.8.0 root=/dev/mapper/vg-root ro quiet \
             isolcpus=2-3,6-7 nohz_full=2-3,6-7 rcu_nocbs=2-3,6-7 rcu_nocb_poll \
             intel_iommu=on iommu=pt",
        );
        h.governor_all(8, "performance");
        h.hugepages(&[0, 1], 2048, 2048);
        h.iommu(true);
        h.processes(&[(1, "systemd"), (2, "kthreadd"), (977, "sshd")]);
        h.routes(&[
            ("eno1", "00000000", "0100A8C0", "00000000"),
            ("eno1", "0000A8C0", "00000000", "00FFFFFF"),
        ]);
        h.add_nic(&Nic {
            vendor: "0xbeef".into(),
            device: "0x1521".into(),
            driver: Some("tg3".into()),
            netdev: Some("eno1".into()),
            speed: Some("1000".into()),
            max_read_req: Some(512),
            pcie: Some(("5.0 GT/s PCIe".into(), 4, "5.0 GT/s PCIe".into(), 4)),
            ..Nic::mlx5("0000:03:00.0", "eno1", Some(0))
        });
        h.add_nic(&Nic::mlx5("0000:17:00.0", "ens1f0", Some(0)));
        h.add_nic(&Nic {
            mtu: Some(9000),
            ..Nic::mlx5("0000:65:00.0", "ens2f0", Some(1))
        });
        h.add_nic(&Nic {
            vendor: "0x8086".into(),
            device: "0x1592".into(),
            driver: Some("ice".into()),
            speed: Some("25000".into()),
            max_read_req: Some(512),
            ..Nic::mlx5("0000:b3:00.0", "enp179s0", Some(1))
        });
        h.add_nic(&Nic {
            vendor: "0x144d".into(),
            device: "0xa80a".into(),
            class: "0x010802".into(),
            driver: Some("nvme".into()),
            netdev: None,
            numa: Some("0".into()),
            pcie: None,
            max_read_req: None,
            ..Nic::mlx5("0000:01:00.0", "x", None)
        });
        h
    }

    /// VM-like: 4 CPUs, isolated 2-3 (full boot parameters), NO `numa_node`,
    /// NO PCIe link files, NO `node` directory, NO cpufreq, global hugepages
    /// only (1024 free), no IOMMU. NICs: `0000:00:03.0` virtio (0x1af4,
    /// `virtio-pci`, `ens3`, default route, no speed file) and `0000:00:05.0`
    /// Mellanox VF (mlx5_core, `ens5`).
    pub fn vm() -> FakeHost {
        let h = FakeHost::empty();
        h.cpus(4, Some("2-3"), &[]);
        h.cmdline("BOOT_IMAGE=/vmlinuz ro quiet isolcpus=2-3 nohz_full=2-3 rcu_nocbs=2-3");
        h.hugepages(&[], 1024, 1024);
        h.iommu(false);
        h.processes(&[(1, "systemd")]);
        h.routes(&[("ens3", "00000000", "0202000A", "00000000")]);
        h.add_nic(&Nic {
            vendor: "0x1af4".into(),
            device: "0x1000".into(),
            driver: Some("virtio-pci".into()),
            speed: None,
            numa: None,
            pcie: None,
            max_read_req: None,
            ..Nic::mlx5("0000:00:03.0", "ens3", None)
        });
        h.add_nic(&Nic {
            device: "0x1018".into(),
            numa: None,
            pcie: None,
            max_read_req: None,
            ..Nic::mlx5("0000:00:05.0", "ens5", None)
        });
        h
    }

    /// No isolation, no hugepages: 4 CPUs, empty `isolated`, plain cmdline,
    /// 0 hugepages, powersave governor, one mlx5 NIC `0000:17:00.0` on node 0.
    pub fn bare() -> FakeHost {
        let h = FakeHost::empty();
        h.cpus(4, Some(""), &[(0, "0-3")]);
        h.cmdline("BOOT_IMAGE=/vmlinuz ro quiet");
        h.governor_all(4, "powersave");
        h.hugepages(&[0], 0, 0);
        h.iommu(false);
        h.processes(&[(1, "systemd")]);
        h.routes(&[]);
        h.add_nic(&Nic::mlx5("0000:17:00.0", "ens1f0", Some(0)));
        h
    }
}

/// 256 byte PCI config space with a PCIe capability at 0x40 whose Device
/// Control register (CAP_EXP+8 = 0x48) encodes `max_read_req` bytes in bits 14:12.
pub fn config_space(max_read_req: u32) -> Vec<u8> {
    let code: u16 = match max_read_req {
        128 => 0,
        256 => 1,
        512 => 2,
        1024 => 3,
        2048 => 4,
        4096 => 5,
        other => panic!("unsupported MaxReadReq {other}"),
    };
    let mut c = vec![0u8; 256];
    c[0x06] = 0x10; // status: capability list present
    c[0x34] = 0x40; // capabilities pointer
    c[0x40] = 0x10; // cap id: PCI Express
    c[0x41] = 0x00; // next
    let devctl: u16 = (code << 12) | 0x0020; // MaxPayload 256 B
    c[0x48..0x4a].copy_from_slice(&devctl.to_le_bytes());
    c
}

// ---------------------------------------------------------------------------
// data builders
// ---------------------------------------------------------------------------

/// A valid ConfigDto JSON: Normal delay `mean_ns`/`std_ns`, limiter off, no loss.
pub fn cfg_json(mean_ns: f64, std_ns: f64) -> Value {
    json!({
        "buffer_capacity": 100000,
        "reordering": false,
        "delayer": {
            "distribution": "NormalDistribution",
            "mean_ns": mean_ns,
            "std_ns": std_ns,
            "activate_correlation": false,
            "half_life_period_ns": 1000000.0,
            "initialization_rate_pps": 15000000.0
        },
        "limiter": { "mode": "Spacer", "activate": false, "rate_bps": 1.0e9, "limit_bit": 1.0e6 },
        "loss_model": { "type": "Random", "p": 0.0 }
    })
}

pub fn cfg_dto(mean_ns: f64, std_ns: f64) -> ConfigDto {
    serde_json::from_value(cfg_json(mean_ns, std_ns)).expect("cfg_json must deserialize")
}

pub fn dto_from(v: Value) -> ConfigDto {
    serde_json::from_value(v).expect("config JSON must deserialize")
}

/// A stored link built through serde (so tests do not depend on the Rust field types):
/// `ingress` -> `egress`, the default config (Normal 1 ms / 0.1 ms) and an optional worker lcore.
pub fn link(id: u32, name: &str, ingress: &str, egress: &str, worker: Option<u32>) -> Link {
    let mut v = json!({
        "id": id, "name": name, "ingress": ingress, "egress": egress,
        "config": cfg_json(1.0e6, 1.0e5),
    });
    if let Some(w) = worker {
        v["worker"] = json!(w);
    }
    serde_json::from_value(v).expect("a v2 link must deserialize into `Link`")
}

/// Like [`link`] with an explicit config JSON and no worker override.
pub fn link_cfg(id: u32, name: &str, ingress: &str, egress: &str, config: Value) -> Link {
    serde_json::from_value(json!({
        "id": id, "name": name, "ingress": ingress, "egress": egress, "config": config,
    }))
    .expect("a v2 link must deserialize into `Link`")
}

pub fn state_with(links: Vec<Link>) -> StoredState {
    let next_id = links.iter().map(|l| l.id).max().unwrap_or(0) + 1;
    StoredState {
        links,
        next_id,
        ..Default::default()
    }
}

pub fn state_with_main(links: Vec<Link>, main_lcore: u32) -> StoredState {
    let mut s = state_with(links);
    s.settings = Settings {
        main_lcore: Some(main_lcore),
        ..Settings::default()
    };
    s
}

pub const NIC0: &str = "0000:17:00.0"; // mlx5, NUMA 0
pub const NIC1: &str = "0000:65:00.0"; // mlx5, NUMA 1
pub const INTEL: &str = "0000:b3:00.0"; // ice, NUMA 1
pub const MGMT: &str = "0000:03:00.0"; // tg3 with the default route

/// Removes every `null` valued object member recursively (so tests do not care
/// whether `None` is serialized as `null` or skipped).
pub fn strip_nulls(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(_, x)| !x.is_null())
                .map(|(k, x)| (k.clone(), strip_nulls(x)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip_nulls).collect()),
        other => other.clone(),
    }
}

/// Missing or null.
pub fn absent(v: &Value, key: &str) -> bool {
    v.get(key).is_none_or(Value::is_null)
}

pub fn f(v: &Value, key: &str) -> f64 {
    v[key]
        .as_f64()
        .unwrap_or_else(|| panic!("`{key}` is not a number in {v}"))
}

pub fn assert_close(actual: f64, expected: f64) {
    let tol = 1e-6 * expected.abs().max(1.0);
    assert!(
        (actual - expected).abs() <= tol,
        "expected {expected}, got {actual}"
    );
}

// ---------------------------------------------------------------------------
// hints helpers
// ---------------------------------------------------------------------------

pub const HINT_IDS: [&str; 9] = [
    "isolation",
    "isolated_cores",
    "hugepages",
    "numa",
    "governor",
    "irqbalance",
    "iommu",
    "mtu",
    "pcie_maxreadreq",
];

/// Spec v17: there is no MPRQ hint, and no hint proposes or discusses mlx5 devargs.
pub fn assert_no_devargs_hint(hints: &Value) {
    for h in hints.as_array().expect("hints must be a JSON array") {
        assert_ne!(h["id"], "mprq", "the MPRQ hint is gone: {h}");
        let text = h.to_string().to_lowercase();
        for word in ["mprq", "devargs", "txq_mpw", "_en="] {
            assert!(!text.contains(word), "hint mentions {word:?}: {h}");
        }
    }
}

pub fn rank(level: &str) -> u8 {
    match level {
        "ok" => 0,
        "info" => 1,
        "warn" => 2,
        "error" => 3,
        other => panic!("unknown hint level {other:?}"),
    }
}

pub fn hints_json(host: &FakeHost, state: &StoredState) -> Value {
    serde_json::to_value(dpds_controller::hints::compute_hints(&host.roots(), state)).unwrap()
}

/// The single hint with `id` (fails if missing or duplicated).
pub fn hint<'a>(hints: &'a Value, id: &str) -> &'a Value {
    let matching: Vec<&Value> = hints
        .as_array()
        .expect("hints must be a JSON array")
        .iter()
        .filter(|h| h["id"] == id)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one hint `{id}` in {hints}"
    );
    matching[0]
}

pub fn level(hints: &Value, id: &str) -> u8 {
    rank(
        hint(hints, id)["level"]
            .as_str()
            .expect("level is a string"),
    )
}

/// `min <= level(id) <= max`.
pub fn assert_level(hints: &Value, id: &str, min: &str, max: &str) {
    let l = level(hints, id);
    assert!(
        rank(min) <= l && l <= rank(max),
        "hint `{id}`: level {} not within {min}..={max}: {}",
        hint(hints, id)["level"],
        hint(hints, id)
    );
}

// ---------------------------------------------------------------------------
// fakes for the REST tests
// ---------------------------------------------------------------------------

pub struct FakeSupervisorInner {
    pub status: SupervisorStatus,
    pub plans: Vec<LaunchPlan>,
    pub stops: usize,
    pub fail_next: Option<(String, Vec<String>)>,
    /// `start` sleeps this long (without holding the lock) before it takes effect.
    pub start_delay: Duration,
}

/// Scripted supervisor: `start` goes straight to Running (pid 4242) unless
/// `fail_next_start` was called, `stop` goes straight to Stopped.
pub struct FakeSupervisor {
    pub inner: Mutex<FakeSupervisorInner>,
}

pub fn status(state: State) -> SupervisorStatus {
    SupervisorStatus {
        state,
        pid: None,
        started_at: None,
        error: None,
        log_tail: vec![],
    }
}

impl FakeSupervisor {
    pub fn new() -> Arc<FakeSupervisor> {
        Arc::new(FakeSupervisor {
            inner: Mutex::new(FakeSupervisorInner {
                status: status(State::Stopped),
                plans: vec![],
                stops: 0,
                fail_next: None,
                start_delay: Duration::ZERO,
            }),
        })
    }
    pub fn fail_next_start(&self, error: &str, log_tail: &[&str]) {
        self.inner.lock().unwrap().fail_next = Some((
            error.into(),
            log_tail.iter().map(|s| s.to_string()).collect(),
        ));
    }
    pub fn set_start_delay(&self, d: Duration) {
        self.inner.lock().unwrap().start_delay = d;
    }
    pub fn force_status(&self, s: SupervisorStatus) {
        self.inner.lock().unwrap().status = s;
    }
    pub fn plans(&self) -> Vec<LaunchPlan> {
        self.inner.lock().unwrap().plans.clone()
    }
    pub fn stops(&self) -> usize {
        self.inner.lock().unwrap().stops
    }
}

impl Supervisor for FakeSupervisor {
    fn start(&self, plan: LaunchPlan) {
        let delay = self.inner.lock().unwrap().start_delay;
        std::thread::sleep(delay);
        let mut i = self.inner.lock().unwrap();
        i.plans.push(plan);
        i.status = match i.fail_next.take() {
            Some((error, log_tail)) => SupervisorStatus {
                error: Some(error),
                log_tail,
                ..status(State::Failed)
            },
            None => SupervisorStatus {
                pid: Some(4242),
                started_at: Some(1_700_000_000),
                log_tail: vec!["DPDS running on lcore(s) [2] with 1 port(s).".into()],
                ..status(State::Running)
            },
        };
    }
    fn stop(&self) {
        let mut i = self.inner.lock().unwrap();
        i.stops += 1;
        i.status = status(State::Stopped);
    }
    fn status(&self) -> SupervisorStatus {
        self.inner.lock().unwrap().status.clone()
    }
}

pub struct FakeTelemetry {
    pub cell: Mutex<Option<Polled>>,
}

impl FakeTelemetry {
    pub fn new() -> Arc<FakeTelemetry> {
        Arc::new(FakeTelemetry {
            cell: Mutex::new(None),
        })
    }
    pub fn set(&self, prev: Option<Snapshot>, cur: Snapshot) {
        *self.cell.lock().unwrap() = Some(Polled {
            prev,
            cur,
            age_ms: 0,
            stale: false,
        });
    }
}

impl TelemetrySource for FakeTelemetry {
    fn latest(&self) -> Option<Polled> {
        self.cell.lock().unwrap().clone()
    }
}

// ---------------------------------------------------------------------------
// in-process REST harness
// ---------------------------------------------------------------------------

pub struct Reply {
    pub status: u16,
    pub headers: axum::http::HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "body is not JSON ({e}): status {} body {:?}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    /// Asserts the `{"message": "<non-empty>"}` error shape and returns the message.
    pub fn message(&self) -> String {
        let v = self.json();
        let m = v["message"]
            .as_str()
            .unwrap_or_else(|| panic!("error body without string `message`: {v}"));
        assert!(!m.is_empty(), "empty error message");
        m.to_string()
    }
}

pub struct TestApp {
    pub router: axum::Router,
    pub host: FakeHost,
    pub supervisor: Arc<FakeSupervisor>,
    pub telemetry: Arc<FakeTelemetry>,
    pub state_dir: TempDir,
    pub runtime_dir: PathBuf,
    pub dpds_bin: PathBuf,
}

impl TestApp {
    pub fn new(host: FakeHost) -> TestApp {
        let state_dir = tempfile::tempdir().unwrap();
        let supervisor = FakeSupervisor::new();
        let telemetry = FakeTelemetry::new();
        let runtime_dir = PathBuf::from("/run/dpds-controller-test");
        let dpds_bin = PathBuf::from("/usr/local/bin/dpds");
        let router = Self::build(
            &host,
            state_dir.path(),
            &supervisor,
            &telemetry,
            &runtime_dir,
            &dpds_bin,
        );
        TestApp {
            router,
            host,
            supervisor,
            telemetry,
            state_dir,
            runtime_dir,
            dpds_bin,
        }
    }

    fn build(
        host: &FakeHost,
        state_dir: &Path,
        supervisor: &Arc<FakeSupervisor>,
        telemetry: &Arc<FakeTelemetry>,
        runtime_dir: &Path,
        dpds_bin: &Path,
    ) -> axum::Router {
        api::router(Deps {
            state_dir: state_dir.to_path_buf(),
            roots: host.roots(),
            supervisor: supervisor.clone(),
            telemetry: telemetry.clone(),
            dpds_bin: dpds_bin.to_path_buf(),
            runtime_dir: runtime_dir.to_path_buf(),
            dpds_has_telemetry: true,
            is_root: false,
        })
        .expect("router must build on an empty state dir")
    }

    /// A new router over the same state dir (a "controller restart").
    pub fn restart(&mut self) {
        self.router = Self::build(
            &self.host,
            self.state_dir.path(),
            &self.supervisor,
            &self.telemetry,
            &self.runtime_dir,
            &self.dpds_bin,
        );
    }

    pub async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.request_with(method, path, body, &[]).await
    }

    pub async fn request_with(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Reply {
        let mut req = axum::http::Request::builder().method(method).uri(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(axum::body::Body::from(b.to_string())),
            None => req.body(axum::body::Body::empty()),
        }
        .unwrap();
        send(&self.router, req).await
    }

    /// A request with an arbitrary content type (None = no header) and raw body bytes.
    pub async fn request_raw(
        &self,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> Reply {
        let mut req = axum::http::Request::builder().method(method).uri(path);
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        send(
            &self.router,
            req.body(axum::body::Body::from(body)).unwrap(),
        )
        .await
    }

    pub async fn get(&self, path: &str) -> Reply {
        self.request("GET", path, None).await
    }
    pub async fn post(&self, path: &str, body: Value) -> Reply {
        self.request("POST", path, Some(body)).await
    }
    pub async fn put(&self, path: &str, body: Value) -> Reply {
        self.request("PUT", path, Some(body)).await
    }
    pub async fn delete(&self, path: &str) -> Reply {
        self.request("DELETE", path, None).await
    }

    /// POST /api/links with `{name, ingress, egress, config}` (default config).
    pub async fn create_link(&self, name: &str, ingress: &str, egress: &str) -> Reply {
        let body = link_body(name, ingress, egress);
        self.post("/api/links", body).await
    }
}

/// Sends one request through `router` and collects the reply.
pub async fn send(router: &axum::Router, req: axum::http::Request<axum::body::Body>) -> Reply {
    use tower::ServiceExt;
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Reply {
        status,
        headers,
        body,
    }
}

/// GET `path` on a router that is not wrapped in a `TestApp`.
pub async fn router_get(router: &axum::Router, path: &str) -> Reply {
    let req = axum::http::Request::builder()
        .method("GET")
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    send(router, req).await
}

pub fn link_body(name: &str, ingress: &str, egress: &str) -> Value {
    json!({ "name": name, "ingress": ingress, "egress": egress, "config": cfg_json(1.0e6, 1.0e5) })
}

// ---------------------------------------------------------------------------
// telemetry snapshot builders (raw telemetry JSON as DPDS would answer)
// ---------------------------------------------------------------------------

pub struct DirSample {
    pub uptime_ns: u64,
    pub rx_pkts: u64,
    pub tx_sent: u64,
    pub loss: u64,
    pub limiter: u64,
    pub full: u64,
    pub tx_drops: u64,
}

pub struct PortSample {
    pub ibytes: u64,
    pub obytes: u64,
    pub imissed: u64,
    pub rx_nombuf: u64,
}

/// Two ports (0, 1), two links `a-to-b` (dir0: port 0 -> 1) and `b-to-a` (dir1: port 1 -> 0).
pub fn snapshot(dirs: [DirSample; 2], ports: [PortSample; 2]) -> Snapshot {
    let ports_reply = json!({
        "num_ports": 2, "num_links": 2, "num_directions": 2,
        "port0_name": "0000:17:00.0", "port1_name": "0000:65:00.0",
        "dir0_link": "a-to-b", "dir0_rx_name": "0000:17:00.0", "dir0_tx_name": "0000:65:00.0",
        "dir0_rx_port": 0, "dir0_tx_port": 1, "dir0_lcore": 2,
        "dir1_link": "b-to-a", "dir1_rx_name": "0000:65:00.0", "dir1_tx_name": "0000:17:00.0",
        "dir1_rx_port": 1, "dir1_tx_port": 0, "dir1_lcore": 6,
    });
    let dir_json = |d: &DirSample| {
        json!({
            "uptime_ns": d.uptime_ns, "polls": d.uptime_ns / 100, "rx_pkts": d.rx_pkts,
            "dpds_rejects": d.loss + d.limiter + d.full,
            "loss_drops": d.loss, "limiter_drops": d.limiter, "buffer_full_drops": d.full,
            "pops": d.tx_sent + d.tx_drops, "tx_pkts": d.tx_sent + d.tx_drops, "tx_sent": d.tx_sent,
            "tx_drops": d.tx_drops, "buffer_len_max": 777,
            "delay_mean_ns": "10000000.25", "delay_std_ns": "1000000.50",
            "delay_min_ns": 5000000, "delay_max_ns": 15000000,
            "lateness_mean_ns": "250.75", "lateness_max_ns": 9000,
            "first_loss_reason": "Clean", "first_loss_detail": "", "first_loss_relative_ns": 0
        })
    };
    let eth_json = |p: &PortSample| {
        json!({ "ipackets": p.ibytes / 100, "opackets": p.obytes / 100, "ibytes": p.ibytes,
                "obytes": p.obytes, "imissed": p.imissed, "ierrors": 0, "oerrors": 0,
                "rx_nombuf": p.rx_nombuf })
    };
    Snapshot {
        ports: ports_reply,
        dirs: vec![dir_json(&dirs[0]), dir_json(&dirs[1])],
        eth: vec![eth_json(&ports[0]), eth_json(&ports[1])],
        // direction 1 has a non-default number of bins, like a link with `histograms`
        hists: vec![hist_json(1000.0, 10.0, 256), hist_json(2000.0, 20.0, 100)],
    }
}

/// A `/dpds/delay_hist` reply as the real `dpds` sends it: `lo_ns` and `bin_ns` as decimal
/// strings, `num_bins`, `bins` (`num_bins` counts), `underflow`, `overflow`.
pub fn hist_json(lo_ns: f64, bin_ns: f64, num_bins: usize) -> Value {
    let hot = num_bins * 100 / 256;
    let bins: Vec<u64> = (0..num_bins)
        .map(|i| if i == hot { 5000 } else { 1 })
        .collect();
    json!({ "lo_ns": lo_ns.to_string(), "bin_ns": bin_ns.to_string(), "num_bins": num_bins,
            "bins": bins, "underflow": 3, "overflow": 4 })
}

/// The reply of a direction whose delay statistics are off.
pub fn empty_hist_json() -> Value {
    json!({ "lo_ns": "0", "bin_ns": "3.90625", "num_bins": 0, "bins": [],
            "underflow": 0, "overflow": 0 })
}

// ---------------------------------------------------------------------------
// minimal blocking HTTP client (the e2e test talks to a real socket)
// ---------------------------------------------------------------------------

pub fn http(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> std::io::Result<(u16, Value)> {
    use std::io::{Read, Write};
    let mut s = TcpStream::connect(addr)?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    s.write_all(req.as_bytes())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, rest) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| std::io::Error::other(format!("malformed response: {text:?}")))?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| std::io::Error::other(format!("no status line: {head:?}")))?;
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(rest)
    } else {
        rest.to_string()
    };
    let value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body).unwrap_or(Value::String(body))
    };
    Ok((status, value))
}

fn dechunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((len, rest)) = s.split_once("\r\n") {
        let n = usize::from_str_radix(len.trim(), 16).unwrap_or(0);
        if n == 0 || rest.len() < n {
            break;
        }
        out.push_str(&rest[..n]);
        s = rest[n..].trim_start_matches("\r\n");
    }
    out
}

/// One raw HTTP/1.1 request; returns the status, the lower-cased header lines and the body.
pub fn http_raw(addr: &str, request: &str) -> std::io::Result<(u16, Vec<String>, String)> {
    use std::io::{Read, Write};
    let mut s = TcpStream::connect(addr)?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.write_all(request.as_bytes())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| std::io::Error::other(format!("malformed response: {text:?}")))?;
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| std::io::Error::other(format!("no status line: {head:?}")))?;
    Ok((
        status,
        lines.map(str::to_lowercase).collect(),
        body.to_string(),
    ))
}

/// A GET with an `Origin` header (and `Connection: close`).
pub fn get_with_origin(addr: &str, path: &str, origin: &str) -> (u16, Vec<String>, String) {
    http_raw(
        addr,
        &format!(
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nConnection: close\r\n\r\n"
        ),
    )
    .unwrap_or_else(|e| panic!("GET {path}: {e}"))
}

/// The value of header `name` (lower case) in the lines returned by [`http_raw`].
pub fn header_value(headers: &[String], name: &str) -> Option<String> {
    headers
        .iter()
        .find_map(|l| l.strip_prefix(&format!("{name}:")))
        .map(|v| v.trim().to_string())
}

/// A running `dpds-controller` binary on a free loopback port, killed on drop.
pub struct ControllerProcess {
    pub child: std::process::Child,
    pub addr: String,
    _state: TempDir,
}

impl ControllerProcess {
    /// Starts the binary with `args` and `envs` as extra environment, plus a temporary
    /// `--state-dir`, and waits until `GET /api/online` answers. The placeholder
    /// `{addr}` in `args` is replaced by the free loopback address the process must bind.
    pub fn start(args: &[&str], envs: &[(&str, &str)]) -> ControllerProcess {
        let state = tempfile::tempdir().unwrap();
        let addr = free_addr();
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_dpds-controller"));
        for a in args {
            cmd.arg(a.replace("{addr}", &addr));
        }
        cmd.arg("--state-dir").arg(state.path().join("state"));
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let child = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("cannot spawn dpds-controller");
        let mut p = ControllerProcess {
            child,
            addr,
            _state: state,
        };
        let t0 = std::time::Instant::now();
        loop {
            if let Ok((200, _)) = http(&p.addr, "GET", "/api/online", None) {
                return p;
            }
            if let Ok(Some(st)) = p.child.try_wait() {
                panic!("dpds-controller exited early ({st}) with args {args:?}");
            }
            assert!(
                t0.elapsed() < Duration::from_secs(15),
                "dpds-controller did not answer on {} with args {args:?}",
                p.addr
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for ControllerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `127.0.0.1:<free port>`.
pub fn free_addr() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("127.0.0.1:{}", l.local_addr().unwrap().port())
}
