<div align="center">
  <img src="./docs/img/logo.png" alt="DPDS Logo" width="400"/>

  <h2>DPDS: A DPDK-Based Packet Delayer and Spacer</h2>

  ![License](https://img.shields.io/badge/licence-Apache%202.0-blue)
  ![Language](https://img.shields.io/badge/lang-rust-darkred)
  ![Built with DPDK](https://img.shields.io/badge/built%20with-DPDK-orange)
  ![Version](https://img.shields.io/badge/v-0.1.0-yellow)
</div>

---

## 📑 Table of Contents
- [📑 Table of Contents](#-table-of-contents)
- [📝 Overview](#-overview)
  - [Highlights](#highlights)
  - [Features](#features)
- [🧩 Architecture](#-architecture)
  - [Topology and lcores](#topology-and-lcores)
  - [Budgets, rings and pools](#budgets-rings-and-pools)
  - [Packet free path](#packet-free-path)
  - [Shutdown](#shutdown)
- [🚀 Installation \& Quick Start](#-installation--quick-start)
  - [Requirements](#requirements)
  - [Building](#building)
  - [Quick Start](#quick-start)
  - [Real NICs](#real-nics)
  - [Without hardware (virtual devices)](#without-hardware-virtual-devices)
  - [Configuration](#configuration)
    - [Configuration Options](#configuration-options)
    - [Reproducible Runs](#reproducible-runs)
- [📊 Telemetry](#-telemetry)
- [🔧 Performance Tuning](#-performance-tuning)
  - [Host and NIC settings](#host-and-nic-settings)
  - [What not to do](#what-not-to-do)
- [🤖 Testing \& Automation](#-testing--automation)
- [📚 Documentation](#-documentation)
- [🛠️ Development](#️-development)
- [📖 Cite](#-cite)
- [License](#license)

---

## 📝 Overview

**DPDS (DPDK-based Packet Delayer and Spacer)** is a high-performance software **network-link emulator** written in Rust.
It sits between two network interfaces and reproduces the three effects a real link imposes on traffic — **delay**, **rate limiting**, and **packet loss** — on a live packet stream at line rate.
Packets are received, passed through the emulation pipeline, and transmitted again with zero copies and no kernel involvement on the fast path, built on [DPDK](https://www.dpdk.org/).
The emulation logic lives in the reusable library [`dpds-core`](crates/dpds-core/README.md), which the `dpds` binary uses directly. This is the Rust port of the original C application.

📖 Related publication:
- [DPDS: A DPDK-Based Packet Delayer and Spacer](https://arxiv.org/abs/2606.17716) (arXiv:2606.17716)

### Highlights

- **Delay:** deterministic, normal, log-normal, gamma or uniform delay distributions, optionally autocorrelated over time (half-life based), with FIFO or send-time (reordering) packet order
- **Rate limiting:** **Spacer** (spreads packets out in time) or **Token Bucket Filter** (drops packets that exceed a burst budget)
- **Packet loss:** **Random** (Bernoulli) or **Gilbert–Elliott** (bursty) loss model
- **Inline or loopback:** two ports (bump-in-the-wire, one emulated link per direction) or one port (loopback)
- **Independent directions:** one worker lcore, one config file, one mbuf pool and one set of telemetry counters per direction
- **Reproducible:** optional `seed` for the delay and loss random number generators
- **Observable:** live statistics over DPDK's telemetry socket (off by default, no cost when off)

### Features

| Feature                                                          | Status         |
| ---------------------------------------------------------------- | -------------- |
| Deterministic, normal, log-normal, gamma, uniform delay          | ✅ Available    |
| Adaptive delay correlation (half-life period)                    | ✅ Available    |
| Packet reordering mode (send-time order)                         | ✅ Available    |
| Spacer rate limiter                                              | ✅ Available    |
| Token Bucket Filter rate limiter                                 | ✅ Available    |
| Random and Gilbert–Elliott packet loss                           | ✅ Available    |
| Inline (2 ports) and loopback (1 port) operation                 | ✅ Available    |
| Separate emulation config per direction                          | ✅ Available    |
| Deterministic runs (`seed`)                                      | ✅ Available    |
| Zero-copy DPDK forwarding, bulk free, cross-language LTO         | ✅ Available    |
| DPDK telemetry (`telemetry` cargo feature)                       | ✅ Available    |
| Reusable Rust library (`dpds-core`)                              | ✅ Available    |
| Virtual-device operation without hardware (`net_null`, `net_pcap`) | ✅ Available  |
| Automated end-to-end tests                                       | ✅ Available    |
| REST API for starting and configuring DPDS                       | ⏳ Planned      |
| React web UI                                                     | ⏳ Planned      |

---

## 🧩 Architecture

DPDS runs **one worker lcore per traffic direction**. Direction `d` is the
traffic received on port `d`; with two ports it leaves on the other port
(`d ^ 1`), with one port it loops back out of port 0.

```
                    worker lcore 1 (direction 0)
              ┌─────────────────────────────────────┐
port 0 RX ──► │ rx_burst ─► dpds-core Buffer ─► tx  │ ──► port 1 TX
              └─────────────────────────────────────┘
                    worker lcore 2 (direction 1)
              ┌─────────────────────────────────────┐
port 1 RX ──► │ rx_burst ─► dpds-core Buffer ─► tx  │ ──► port 0 TX
              └─────────────────────────────────────┘
                      main lcore: signals, NIC poll, telemetry
```

Each direction owns everything on its path: its `dpds-core` `Buffer` (built
from its **own config file**), its mbuf pool, its telemetry counters and
epochs. RX queue 0 of port `d` is used only by worker `d`, and TX queue 0 of the
egress port only by that worker, so the workers share no data-plane state.
The i-th EAL worker lcore (in EAL order) runs direction i.

One loop iteration of a worker:

1. Read the quit flag.
2. Read the clock **once**; that `now` is used for every time in the iteration.
3. Unless quitting, `rx_burst` up to `BURST_SIZE` (32) packets.
4. Push each packet into the buffer. Packets the buffer refuses (loss model,
   rate limiter, buffer full) are freed with one bulk free.
5. Pop up to `BURST_SIZE` (32) packets whose send time is due.
6. Prefetch the popped packets' mbuf headers and those of the next queued ones.
7. `tx_burst` the popped packets **in the same iteration**; whatever the NIC
   did not accept is freed with one bulk free.

The main lcore stays free for signal handling and for polling the NICs'
hardware counters every 250 ms (`/nic/*` telemetry).

The worker delegates *when* (or *whether*) each packet is forwarded to
`dpds-core`; see its [README](crates/dpds-core/README.md) for the delay
distributions, rate-limiter modes (Spacer / Token Bucket Filter), and loss
models (Random / Gilbert-Elliott).

### Topology and lcores

| ports | directions | lcores needed | example EAL arg |
| ----- | ---------- | ------------- | --------------- |
| 2 (inline, bump-in-the-wire) | 2: port 0 -> port 1 and port 1 -> port 0 | 3 (main + 2 workers) | `-l 0-2` |
| 1 (loopback) | 1: port 0 -> port 0 | 2 (main + 1 worker) | `-l 0-1` |

DPDS needs exactly `1 + number of ports` lcores and exits at startup with an
error naming the required count otherwise. Any other port count is rejected.
`--config1` with only one port is an error.

### Budgets, rings and pools

- `BURST_SIZE` is 32: the RX burst size and the pop budget per iteration.
- `NIC_DESCRIPTORS` is 4096: the RX and TX ring size requested per port (the
  driver may clamp or round it; the applied sizes are logged at startup and
  shown by `/dpds/ports`).
- Each direction has its **own mbuf pool** (`dpds_pool_<d>`), created on the
  NUMA socket of port `d`. DPDS warns on stderr if the worker lcore of a
  direction sits on a different socket than its port. The data room is sized for
  `MTU + 14 + 4` bytes (MTU 1500).
- **Pool size is derived, not configured.** For every direction,

  ```
  pool size = buffer_capacity + rx_desc(port d) + tx_desc(egress port)
              + MBUF_CACHE_SIZE * 3 / 2 + 2 * BURST_SIZE
  ```

  `buffer_capacity` is the direction's own config value; `rx_desc` and `tx_desc`
  are the ring sizes the drivers apply (`rte_eth_dev_adjust_nb_rx_tx_desc`,
  queried before any pool or port is created); `MBUF_CACHE_SIZE` is 512, so the
  last two terms are 768 (the worker's mempool cache) and 64 (two bursts). The
  pool therefore always holds a full delay buffer plus both rings, so the NIC
  never runs out of mbufs before the DPDS buffer rejects a packet. Example: the
  default `config.json` (`buffer_capacity` 1,000,000) with 4096-descriptor rings
  gives 1,000,000 + 4096 + 4096 + 768 + 64 = 1,009,024 mbufs per direction. If a
  pool cannot be created (e.g. not enough hugepage memory), DPDS exits non-zero
  naming the direction and the size it tried. The constants are in
  `crates/dpds/src/consts.rs`.
- **Hugepage memory.** One mbuf takes about 1.9 KB in the pool (1664 B data
  room + 128 B `rte_mbuf` + the mempool object header; 1920 B measured with
  128-byte cache lines, slightly less with 64-byte lines). The default config
  therefore needs about 1.8 GiB of hugepages per direction, twice that with two
  ports. Shrink `buffer_capacity` if that is too much.

### Packet free path

Dropped packets are freed in bulk. If **both** ports a direction uses (RX port
`d`, TX egress port) report `MBUF_FAST_FREE` active, the worker frees with the raw
`rte_mbuf_raw_free_bulk` on the direction's pool, otherwise with
`rte_pktmbuf_free_bulk`. Raw free is safe because the TX port's fast-free flag
covers the mbufs the NIC did not accept (direct, refcount 1, from the direction's
pool), and the mbufs the buffer rejected on RX are single-segment: RX scatter is
off and the data room fits the largest frame. The choice is logged per direction
at startup (and shown as `dir<d>_raw_free` by `/dpds/ports`). mlx5 withdraws
`MBUF_FAST_FREE` when MPRQ is enabled (`mprq_en=1`), so keep MPRQ off, see
[Performance tuning](#-performance-tuning).

### Shutdown

`SIGINT`/`SIGTERM` make every worker stop receiving, drain its delay buffer
completely (packets still leave at their send time) and return once its buffer is
empty. Then the main lcore waits for all workers, stops the ports, cleans up the
EAL and the process exits with status 0.

---

## 🚀 Installation & Quick Start

### Requirements

- **DPDK 26.07** (found through `pkg-config libdpdk`; the build needs at least 25.11),
  and for real traffic a DPDK-capable NIC (tested on Mellanox ConnectX)
- **Rust** (stable toolchain)
- **clang and lld, with the same LLVM major version as `rustc`** (see [Building](#building))
- Linux; hugepages for real NICs (not needed for the virtual devices below)

### Building

```bash
cargo build --release                        # plain: no telemetry
cargo build --release --features telemetry   # with DPDK telemetry commands
```

The binary is `target/release/dpds`. The `telemetry` feature is off by default
and costs nothing when off, see [Telemetry](#-telemetry).

The workspace is configured for maximum performance (`.cargo/config.toml`):
`-C target-cpu=native` plus cross-language LTO, where `lld` inlines DPDK's
`static inline` burst functions (`rte_eth_rx_burst`, `rte_eth_tx_burst`, ...)
into the Rust forwarding loops. This only works if the `clang` that compiles
the DPDK wrappers and `rustc` use the **same LLVM major version**. Check with
`rustc -vV` (`LLVM version:`) and `clang --version`. For example with
`rustc` on LLVM 23:

```bash
export PATH=/usr/lib/llvm-23/bin:$PATH   # clang-23 and lld-23 first in PATH
cargo build --release
```

A mismatch does not fail the build; lld silently falls back to the machine
code and nothing is inlined, which costs throughput.

Without clang/lld, override the flags to drop LTO and the lld requirement:

```bash
RUSTFLAGS='-Ctarget-cpu=native' cargo build --release
```

### Quick Start

DPDS takes DPDK's EAL arguments, then `--`, then its own options:

```
dpds <EAL args...> -- [--config <path>] [--config0 <path>] [--config1 <path>]
```

| Option               | Meaning                                                                                                   |
| -------------------- | --------------------------------------------------------------------------------------------------------- |
| `--config <path>`    | Emulation config file for every direction without its own (default `config.json` in the working directory). |
| `--config0 <path>`   | Config for direction 0 (traffic received on port 0). Overrides `--config`.                                  |
| `--config1 <path>`   | Config for direction 1 (port 1 -> port 0). Overrides `--config`. Error with only one port.                  |

The config of direction `d` is `--config<d>` if given, else `--config`, else
`config.json`. A flag given twice, an unknown flag, a missing value, a bad
config file (invalid distribution or half-life parameters are reported at startup, not as a panic), a
wrong lcore or port count, a pool that cannot be created, or a port whose link is down
makes `dpds` exit with a non-zero status and a message on stderr.

At startup DPDS prints, per port: name, applied and requested ring sizes,
socket, RX/TX burst mode and link; per direction: lcore, ports, config path,
pool size and socket, and the free path (raw or pktmbuf).

### Real NICs

```bash
# 3 lcores (main + one worker per direction), two NICs inline,
# a different emulation per direction:
sudo ./target/release/dpds -l 0-2 -a <PCI0> -a <PCI1> -- \
    --config0 forward.json --config1 return.json
```

- `-l 0-2` - exactly three lcores for two ports: the main lcore and one worker
  per direction. One port needs `-l 0-1`.
- `-a <PCI>` - NIC PCI address (e.g. `0000:65:00.0`); pass one for loopback,
  two for inline. For mlx5 NICs append driver arguments to it, e.g.
  `-a <PCI0>,txq_mpw_en=1` (Enhanced Multi-Packet Write on TX; already the
  default on ConnectX-5 and newer, so this is a safety net). Do not set
  `mprq_en`, see [Performance tuning](#-performance-tuning).
- `--in-memory` is a sensible EAL flag for real runs too: it skips the hugepage
  file setup, so a crashed process leaves no state behind.

Tuning a testbed host (isolated cores, NIC settings) and launching, stopping and
syncing DPDS there is done by the separate evaluation automation, not by this
repository; the repository only provides the binary, the config files and the
telemetry socket. The tuning advice DPDS depends on is under
[Performance tuning](#-performance-tuning).

### Without hardware (virtual devices)

No root, hugepages or NIC needed, e.g. to try the setup or in CI. Run them from
the repository root:

```bash
# One port that receives nothing and discards everything:
./target/release/dpds --no-huge -m 256 --no-pci --in-memory -l 0-1 \
    --vdev=net_null0 -- --config config.vdev.json

# Replay a pcap through DPDS, capture the result (1 port):
./target/release/dpds --no-huge -m 256 --no-pci --in-memory -l 0-1 \
    --vdev=net_pcap0,rx_pcap=in.pcap,tx_pcap=out.pcap -- --config config.vdev.json
```

`config.vdev.json` is `config.json` with `buffer_capacity` 4096, so its
[derived pool](#budgets-rings-and-pools) of 4096 + 4096 + 4096 + 768 + 64 =
13,120 mbufs (about 25 MB) fits into `-m 256`. The default `config.json` derives
about a million mbufs (about 1.8 GiB) per direction; to use it here, raise `-m`
accordingly. The second example needs an `in.pcap` in the working directory.
Stop with `Ctrl-C`.

### Configuration

The emulation behaviour is controlled by one JSON file per direction
(`--config`, `--config0`, `--config1`). The `config.json` in this repository is a
working default (10 ms normally distributed delay, no rate limit, no loss);
`config.vdev.json` is the same with a small `buffer_capacity` for the
[virtual-device examples](#without-hardware-virtual-devices). Example with
the Spacer enabled, bursty loss and a fixed seed:

```json
{
  "buffer_capacity": 100000,
  "reordering": false,
  "seed": 42,
  "delayer": {
    "distribution": "LogNormalDistribution",
    "mean_ns": 5000000.0,
    "std_ns": 500000.0,
    "activate_correlation": true,
    "half_life_period_ns": 50000000.0
  },
  "limiter": {
    "mode": "Spacer",
    "activate": true,
    "rate_bps": 10000000000.0,
    "limit_bit": 12000000.0
  },
  "loss_model": {
    "GilbertElliot": [0.01, 0.3, 1.0, 0.5, true]
  }
}
```

The full schema is documented in
[`crates/dpds-core/README.md`](crates/dpds-core/README.md#configuration), with a
complete example in
[`crates/dpds-core/config.example.json`](crates/dpds-core/config.example.json).

#### Configuration Options

| Option                             | Valid Values                                                                                                                  |
| ---------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `buffer_capacity`                  | Integer: maximum number of packets held at once; pushes beyond this are dropped                                               |
| `reordering`                       | `false` (FIFO) · `true` (send-time order)                                                                                     |
| `seed`                             | Optional integer; absent: seeded from system entropy                                                                          |
| `delayer.distribution`             | `DeterministicDistribution` · `NormalDistribution` · `LogNormalDistribution` · `GammaDistribution` · `UniformDistribution`    |
| `delayer.mean_ns`, `delayer.std_ns` | Mean and standard deviation of the delay in ns (`std_ns` is ignored for `DeterministicDistribution`)                          |
| `delayer.activate_correlation`     | `true` · `false`: autocorrelation of successive delays                                                                        |
| `delayer.half_life_period_ns`      | Correlation half-life in ns (finite, not negative; not absurdly long)                                                                        |
| `limiter.mode`                     | `Spacer` · `TokenBucketFilter`                                                                                                |
| `limiter.activate`                 | `true` · `false`: when `false` the limiter is a no-op                                                                         |
| `limiter.rate_bps`                 | Target rate in bits per second                                                                                                |
| `limiter.limit_bit`                | Spacer: maximum in-flight backlog in bits; Token Bucket Filter: bucket size (burst budget) in bits                            |
| `loss_model`                       | `{ "Random": p }` · `{ "GilbertElliot": [p, r, k, h, state_is_good] }`                                                        |

#### Reproducible Runs

Two fields matter for reproducible runs:

- **`reordering`**: with `false` packets leave in arrival order (FIFO); with
  `true` they leave in send-time order. In reordering mode packets with equal
  send times keep their entry order: every packet that enters at the same
  instant sorts 1/64 ns after the previous one, so a constant delay never
  reorders. Caveat across instants: a burst of `n` packets at one instant
  shifts its keys by up to `(n - 1)/64` ns (about 0.5 ns at the 32-packet
  burst of `dpds`), so packets that entered at different instants whose send
  times are closer than that may leave in either order.
- **`seed`** (optional, integer): seeds the delay and loss random number
  generators deterministically. Absent: seeded from system entropy as before.
  With the same seed and the same input the emulated decisions repeat.

---

## 📊 Telemetry

DPDS prints a few lines once at startup (ports, descriptor ring sizes, the
worker lcores) and then nothing: live statistics are served over DPDK's telemetry
socket, which is the machine-readable interface (a controller can poll it).
Build with `--features telemetry` to register the commands:

| command | what |
| --- | --- |
| `/dpds/stats,<dir>` | Packet counters of direction `<dir>` (0 or 1) since start (`rx_pkts`, `dpds_rejects`, `pops`, `tx_sent`, `tx_drops`, ...), gauges, first-loss result. |
| `/dpds/epochs,<dir>` | The same per epoch (a burst of traffic, closed after 500 ms of silence). |
| `/dpds/ports` | Port names, descriptor ring sizes (applied and requested), and per direction its ports, lcore, config path, pool size and free path. |
| `/nic/hw_first_loss,<port>` | First NIC hardware drop (`imissed`, `rx_nombuf`, ...) seen by the main lcore. |
| `/nic/epochs,<port>` | Per-epoch NIC `rx_packets` / `imissed` / `rx_port_discards`. |

Query with the client that ships with DPDK (`--file-prefix` as given to DPDS,
and the same `XDG_RUNTIME_DIR` when running as non-root):

```bash
./target/release/dpds -l 0-2 --file-prefix demo -a <PCI0> -a <PCI1> -- --config config.json &
echo '/dpds/stats,0' | dpdk-telemetry.py -f demo
echo '/dpds/ports' | dpdk-telemetry.py -f demo
```

The socket path is limited to 108 bytes; a longer `XDG_RUNTIME_DIR` /
`--file-prefix` makes DPDS fail at startup. Without the feature nothing is
registered and the instrumentation compiles to nothing.
Every field, the epoch semantics, the first-loss reasons and the socket details
are in [docs/TELEMETRY.md](docs/TELEMETRY.md).

---

## 🔧 Performance Tuning

For line-rate operation, isolate DPDS's cores from the OS and tune the NIC. The
facts below come from preof (the sibling project whose DPDK code DPDS reuses,
`dpdk-peof`) and apply to any similar host. They are not DPDS measurements, and
applying them is the job of whoever sets up the host (on a testbed: the
evaluation automation).

### Host and NIC settings

All of these are runtime-only (they revert on reboot or NIC reset) unless noted.

- **Isolate the worker cores** (boot arguments `isolcpus=<cores> nohz_full=<cores>
  rcu_nocbs=<cores> rcu_nocb_poll`) and pin DPDS to them with `-l`; keep the
  main lcore off them. Without `nohz_full`/`rcu_nocbs`, a spinning worker can
  starve RCU system-wide and make the host unreachable.
- **PCIe MaxReadReq 4096 B** on each NIC, with
  `setpci -s <PCI> CAP_EXP+8.w=5000:7000` (a masked write of bits 14:12 of the
  PCIe Device Control register, valid for any device because it is relative to
  the PCIe capability). In a VM whether the write reaches the physical device
  through passthrough is unverified.
- **Ethernet flow control off** (`ethtool -A <netdev> rx off tx off`).
- **NIC firmware** (cannot be changed from inside a VM): `MAX_ACC_OUT_READ`
  (with `ADVANCED_PCI_SETTINGS=1`) set to 256 (the firmware may clamp it) and
  `CQE_COMPRESSION` aggressive, as preof does.
- **Memory and interrupts**: `vm.zone_reclaim_mode=0`, `vm.swappiness=0`, and
  interrupts kept off the isolated cores (`irqbalance` with those cores banned).
- **One port per physical card** when using two ports: a card-level firmware
  reset takes down both functions of a card.

### What not to do

- **No `chrt` / `SCHED_FIFO`.** preof tried real-time priority 99 (made the host
  unreachable, needed a hardware reset) and 98 (broke forwarding) with no
  throughput gain over plain scheduling on isolated cores. Run under normal
  scheduling, pinned by `-l`.
- **Do not `perf record` a busy-polling core in a VM.** preof saw the core's
  cycles per second drop by about a third a few seconds after
  `perf record -C <core> -F 999` was started, which also lowered the measured
  throughput; the control run with counting-mode `perf stat` alone was not done.
  Use `perf stat` (counting) for throughput, IPC and cycles, and take sampled
  profiles only for relative shares.
- **Do not enable MPRQ** (`mprq_en=1`). preof ran with it and turned it off after
  its zero-loss sweep showed an RX queue running out of buffers
  (`rx_out_of_buffer`) in most runs, suspected because held packets pin whole
  MPRQ buffers. DPDS holds packets for the emulated delay too, and mlx5 also
  withdraws `MBUF_FAST_FREE` under MPRQ, which the raw bulk free path needs.
- **Do not use the stack mempool ops** (`--mbuf-pool-ops-name=stack`). preof
  removed it after every 10 ms zero-loss run lost millions of packets to
  `imissed`.

---

## 🤖 Testing & Automation

```bash
cargo test --release                        # without telemetry
cargo test --release -p dpds --features telemetry
```

Unit tests need no DPDK runtime. The end-to-end tests (`crates/dpds/tests/e2e.rs`)
start the real binary on `net_null` and `net_pcap` virtual devices without root
or hugepages; they need several CPUs (they fail, not skip, on a host that is too
small; see the file's header for the count). With the feature they also check the
exact packet counts through the telemetry socket. One of them runs the commands
of the [Without hardware](#without-hardware-virtual-devices) examples above as
written, so the README stays runnable.

Tuning a testbed host and launching, stopping and syncing DPDS on it is done by a
separate evaluation automation, which starts the binary itself with its own EAL
arguments and configs. The binary, the CLI flags, the JSON config files and the
telemetry socket are non-interactive and meant to be driven from another process.

⏳ Planned: a REST API to start and configure DPDS, and a React web UI on top of it.

---

## 📚 Documentation

- Emulation library, delay/limiter/loss configuration and Rust API: [`crates/dpds-core/README.md`](crates/dpds-core/README.md)
- Telemetry commands and every field: [docs/TELEMETRY.md](docs/TELEMETRY.md)
- Paper: [DPDS: A DPDK-Based Packet Delayer and Spacer](https://arxiv.org/abs/2606.17716)

---

## 🛠️ Development

```
crates/dpds        The data plane binary: CLI, per-direction worker loop, telemetry
crates/dpds-core   Emulation library: delay, rate limiter, loss, packet buffer
crates/dpdk        Safe wrapper around the DPDK calls the data plane needs
crates/dpdk-sys    Raw bindgen bindings to DPDK
Makefile           build / check-static-link (see below)
config.json        Default emulation configuration
config.vdev.json   Small configuration for the virtual-device examples
docs/TELEMETRY.md  The telemetry commands and what every field means
```

The root `Makefile` is deliberately small and knows nothing about any host:

| target | what |
| --- | --- |
| `make build [FEATURES=telemetry]` | `cargo build --release -p dpds`, optionally with the `telemetry` feature (empty by default: no instrumentation). |
| `make check-static-link` | Disassemble `target/release/dpds` and fail if `rx_burst` / `tx_burst` / `rdtsc` `__extern` wrappers are still called out of line, i.e. cross-language LTO did not inline them (see [Building](#building)). The bulk-free wrappers are only listed. |

Both are non-interactive. Run the tests as described under
[Testing & Automation](#-testing--automation).

---

## 📖 Cite

If you use DPDS in any of your publications, please cite the following paper:

E. Zink, F. Ihle, and M. Menth: [DPDS: A DPDK-Based Packet Delayer and Spacer](https://arxiv.org/abs/2606.17716), arXiv:2606.17716, 2026

```tex
@misc{ZiIh26,
  author    = {Zink, Etienne and Ihle, Fabian and Menth, Michael},
  title     = {{DPDS: A DPDK-Based Packet Delayer and Spacer}},
  publisher = {arXiv},
  year      = {2026},
  month     = jun
}
```

---

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
