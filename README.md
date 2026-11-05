<div align="center">
  <img src="./docs/img/logo.png" alt="DPDS Logo" width="400"/>

  <h2>DPDS: A DPDK-Based Packet Delayer and Spacer</h2>

  [![CI](https://github.com/uni-tue-kn/DPDS/actions/workflows/ci.yml/badge.svg?branch=nightly)](https://github.com/uni-tue-kn/DPDS/actions/workflows/ci.yml)
  ![License](https://img.shields.io/badge/licence-Apache%202.0-blue)
  ![Backend](https://img.shields.io/badge/backend-Rust-b7410e?logo=rust)
  ![Frontend](https://img.shields.io/badge/frontend-React-149eca?logo=react)
  ![DPDK](https://img.shields.io/badge/dataplane-DPDK%2026.07-1e5aa8)
  ![Version](https://img.shields.io/badge/v-1.0.0-yellow)
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
  - [Memory locking](#memory-locking)
  - [Shutdown](#shutdown)
- [🚀 Installation \& Quick Start](#-installation--quick-start)
  - [Requirements](#requirements)
  - [Building](#building)
  - [Quick Start](#quick-start)
  - [Real NICs](#real-nics)
  - [Without hardware (virtual devices)](#without-hardware-virtual-devices)
  - [Several links in one process (`--links`)](#several-links-in-one-process---links)
  - [Configuration](#configuration)
    - [Configuration Options](#configuration-options)
    - [Reproducible Runs](#reproducible-runs)
- [📊 Telemetry](#-telemetry)
- [🔧 Performance Tuning](#-performance-tuning)
  - [Host and NIC settings](#host-and-nic-settings)
  - [What not to do](#what-not-to-do)
- [🖥️ Controller and web frontend](#️-controller-and-web-frontend)
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
- **Directed links:** a link is one direction, ingress port to egress port, with its own emulation: NIC A -> NIC B, NIC B -> NIC C and NIC C -> NIC A are three links; the classic bump-in-the-wire is two links (A -> B and B -> A); one port can loop back (ingress = egress)
- **Independent links:** one worker lcore, one config, one mbuf pool and one set of telemetry counters per link
- **Several links in one process:** a `--links` file describes any number of links, ports named by PCI address or virtual-device name
- **Reproducible:** optional `seed` for the delay and loss random number generators
- **Observable:** live statistics over DPDK's telemetry socket (off by default, no cost when off): counters, drops per cause, delay statistics and a delay histogram

### Features

| Feature                                                          | Status         |
| ---------------------------------------------------------------- | -------------- |
| Deterministic, normal, log-normal, gamma, uniform delay          | ✅ Available    |
| Adaptive delay correlation (half-life period)                    | ✅ Available    |
| Packet reordering mode (send-time order)                         | ✅ Available    |
| Spacer rate limiter                                              | ✅ Available    |
| Token Bucket Filter rate limiter                                 | ✅ Available    |
| Random and Gilbert–Elliott packet loss                           | ✅ Available    |
| Directed links (ingress -> egress), ring and hairpin topologies   | ✅ Available    |
| Separate emulation config per link                               | ✅ Available    |
| Several links in one process (`--links`), explicit lcores        | ✅ Available    |
| Drop counters per cause (loss model, limiter, buffer full)       | ✅ Available    |
| Delay statistics and configurable delay histogram (telemetry)    | ✅ Available    |
| Deterministic runs (`seed`)                                      | ✅ Available    |
| Zero-copy DPDK forwarding, bulk free, cross-language LTO         | ✅ Available    |
| DPDK telemetry (`telemetry` cargo feature)                       | ✅ Available    |
| Reusable Rust library (`dpds-core`)                              | ✅ Available    |
| Virtual-device operation without hardware (`net_null`, `net_pcap`) | ✅ Available  |
| Automated end-to-end tests                                       | ✅ Available    |
| REST controller for configuring, starting and stopping DPDS      | ⚠️ Experimental |
| React web frontend (served by the controller)                    | ⚠️ Experimental |

---

## 🧩 Architecture

DPDS runs **one worker lcore per link**. A link is one direction: the traffic
received on its ingress port leaves on its egress port with the link's
emulation applied. The picture shows the classic pair, two links (`dir0`: port 0
-> port 1, `dir1`: port 1 -> port 0); the
[links file](#several-links-in-one-process---links) allows any directed
topology, e.g. a ring A -> B -> C -> A over three NICs.

```
                    worker lcore 1 (link dir0)
              ┌─────────────────────────────────────┐
port 0 RX ──► │ rx_burst ─► dpds-core Buffer ─► tx  │ ──► port 1 TX
              └─────────────────────────────────────┘
                    worker lcore 2 (link dir1)
              ┌─────────────────────────────────────┐
port 1 RX ──► │ rx_burst ─► dpds-core Buffer ─► tx  │ ──► port 0 TX
              └─────────────────────────────────────┘
                      main lcore: signals, NIC poll, telemetry
```

Each link owns everything on its path: its `dpds-core` `Buffer` (built
from its **own config**), its mbuf pool, its telemetry counters and epochs.
Every port is the ingress of at most one link and the egress of at most one
link, so RX queue 0 of a port is used only by the worker of the link that
receives on it and TX queue 0 only by the worker of the link that transmits on
it: the workers share no data-plane state. A port that is only an egress is
never polled. Without explicit lcores the i-th EAL worker lcore (in EAL order)
runs link i.

One loop iteration of a worker:

1. Read the quit flag.
2. Read the clock **once**; that `now` is used for every time in the iteration.
3. Unless quitting, `rx_burst` up to `BURST_SIZE` (32) packets.
4. Push each packet into the buffer. Packets the buffer refuses (buffer full,
   loss model, rate limiter) are freed with one bulk free.
5. Pop the packets whose send time is due into the TX staging array (at most
   `BURST_SIZE` (32) per iteration, at most as many as the array has free slots).
6. Prefetch the popped packets' mbuf headers and those of the next queued ones.
7. If the staging array is full (64 packets) or its oldest packet has been due
   for 10 µs, `tx_burst` everything staged; whatever the NIC did not accept is
   freed with one bulk free.

**TX staging.** At high rates one iteration pops only a packet or two, and a
`tx_burst` per iteration then rings the NIC's doorbell for tiny batches, with a high
per-packet cost in PCIe transactions (at 80 Gb/s with 1518 B frames this showed as
RX and TX overflow). A worker therefore collects due packets in a staging array of
`TX_BURST_SIZE` = 64 and sends them with one `tx_burst` when the array is full or
when its oldest packet has been due for `TX_MAX_WAIT_NS` = 10 µs, whichever comes
first (both are fixed, not configurable). A packet thus leaves up to about 10 µs
after its send time (a bounded extra delay, included in the delay statistics and
the delay histogram) and the output is burstier, in batches of up to 64 packets
instead of the individual spacing of the model. The last partial batch of a run is
not flushed early; it leaves when its 10 µs are over.

The main lcore stays free for signal handling and for polling the NICs'
hardware counters every 250 ms (`/nic/*` telemetry).

The worker delegates *when* (or *whether*) each packet is forwarded to
`dpds-core`; see its [README](crates/dpds-core/README.md) for the delay
distributions, rate-limiter modes (Spacer / Token Bucket Filter), and loss
models (Random / Gilbert-Elliott).

### Topology and lcores

| ports | links | lcores needed | example EAL arg |
| ----- | ----- | ------------- | --------------- |
| 2 (inline, bump-in-the-wire) | 2: `dir0` port 0 -> port 1 and `dir1` port 1 -> port 0 | 3 (main + 2 workers) | `-l 0-2` |
| 1 (loopback) | 1: `dir0` port 0 -> port 0 | 2 (main + 1 worker) | `-l 0-1` |

DPDS needs exactly `1 + number of links` lcores and exits at startup with an
error naming the required count otherwise. With `--config*` any port count other
than 1 or 2 is rejected and `--config1` with only one port is an error. With
[`--links`](#several-links-in-one-process---links) any number of links is
allowed, every link gets a worker lcore.

### Budgets, rings and pools

- `BURST_SIZE` is 32: the RX burst size and the pop budget per iteration.
- `NIC_DESCRIPTORS` is 4096: the RX and TX ring size requested per port (the
  driver may clamp or round it; the applied sizes are logged at startup and
  shown by `/dpds/ports`).
- Each link has its **own mbuf pool** (`dpds_pool_<d>`), created on the
  NUMA socket of its ingress port. DPDS warns on stderr if the worker lcore of a
  link sits on a different socket than its ingress port. The data room is sized for
  `MTU + 14 + 4` bytes (MTU 1500).
- **Pool size is derived, not configured.** For every link,

  ```
  pool size = buffer_capacity + rx_desc(ingress port) + tx_desc(egress port)
              + MBUF_CACHE_SIZE * 3 / 2 + 2 * BURST_SIZE
  ```

  `buffer_capacity` is the link's own config value; `rx_desc` and `tx_desc`
  are the ring sizes the drivers apply (`rte_eth_dev_adjust_nb_rx_tx_desc`,
  queried before any pool or port is created); `MBUF_CACHE_SIZE` is 512, so the
  last two terms are 768 (the worker's mempool cache) and 64 (two bursts). The
  pool therefore always holds a full delay buffer plus both rings, so the NIC
  never runs out of mbufs before the DPDS buffer rejects a packet. Example: the
  config with `buffer_capacity` 1,000,000 and 4096-descriptor rings
  gives 1,000,000 + 4096 + 4096 + 768 + 64 = 1,009,024 mbufs per link. If a
  pool cannot be created (e.g. not enough hugepage memory), DPDS exits non-zero
  naming the link's index and the size it tried. A port that is only an
  egress adds its own `rx_desc` to the pool of the link that transmits on it
  (its RX ring is filled but never polled). The constants are in
  `crates/dpds/src/consts.rs`.
- **Hugepage memory.** One mbuf takes about 1.9 KB in the pool (1664 B data
  room + 128 B `rte_mbuf` + the mempool object header; 1920 B measured with
  128-byte cache lines, slightly less with 64-byte lines). The default config
  therefore needs about 1.8 GiB of hugepages per link, twice that with two
  links. Shrink `buffer_capacity` if that is too much.

### Packet free path

Dropped packets are freed in bulk. If **both** ports a link uses (ingress and
egress port) report `MBUF_FAST_FREE` active, the worker frees with the raw
`rte_mbuf_raw_free_bulk` on the link's pool, otherwise with
`rte_pktmbuf_free_bulk`. Raw free is safe because the TX port's fast-free flag
covers the mbufs the NIC did not accept (direct, refcount 1, from the link's
pool), and the mbufs the buffer rejected on RX are single-segment: RX scatter is
off and the data room fits the largest frame. The choice is logged per link
at startup (and shown as `dir<d>_raw_free` by `/dpds/ports`).

### Memory locking

Like preof, `dpds` locks its memory before it does anything else: its first call, before EAL
initialisation, is `mlockall(MCL_CURRENT | MCL_FUTURE)`. With `MCL_FUTURE` every later mapping is
locked and populated when it is created: the EAL heap and mbuf pools, each link's delay buffer
(reserved up front at its full `buffer_capacity`) and the telemetry structures. All of it is
allocated and resident before the first packet is forwarded, so no worker lcore takes a page
fault while it polls. The forwarding loops allocate nothing; the only allocations after startup
are the small telemetry replies, built on DPDK's telemetry thread.

Locked memory counts against `RLIMIT_MEMLOCK`, and the pools and buffers are large (see
[Budgets, rings and pools](#budgets-rings-and-pools)). `mlockall` would even succeed while the
process is still small and then make every later allocation count against a finite limit, so EAL
initialisation would fail. Before locking, `dpds` therefore looks at the effective user and at the
soft and the hard limit:

1. Root (it has `CAP_IPC_LOCK`, which ignores the limit): lock.
2. The soft limit is unlimited: lock.
3. The soft limit is finite but the hard limit is unlimited: `dpds` raises the soft limit to
   unlimited, then locks.
4. The hard limit is finite: `dpds` does not call `mlockall`, prints
   `WARNING: RLIMIT_MEMLOCK is <n> KiB, not unlimited: continuing unlocked (locking would make later allocations fail; run as root or set LimitMEMLOCK=infinity / ulimit -l unlimited)`
   (`<n>` is the soft limit) and runs unlocked.

If the lock itself still fails in cases 1 to 3, `dpds` prints
`WARNING: mlockall failed, continuing unlocked: <error>` and runs unlocked. Run by hand as another
user, raise the limits (`ulimit -l unlimited`, or `memlock` in `/etc/security/limits.conf`). The
unit `packaging/dpds-controller.service` sets `LimitMEMLOCK=infinity` explicitly, so the `dpds`
child of the controller can lock whatever it needs.

### Shutdown

`SIGINT`/`SIGTERM` make every worker stop receiving, drain its delay buffer
completely (packets still leave at their send time) and return once its buffer is
empty. Then the main lcore waits for all workers, stops the ports, cleans up the
EAL and the process exits with status 0.

---

## 🚀 Installation & Quick Start

### Requirements

- **DPDK 26.07** (found through `pkg-config libdpdk`; the build needs at least 26.07),
  and for real traffic a DPDK-capable NIC (tested on Mellanox ConnectX)
- **Rust** (stable toolchain)
- **clang and lld, with the same LLVM major version as `rustc`** (see [Building](#building))
- **Node 22.22 or newer and npm**, plus network access to the npm registry, to build the web
  frontend (only for the controller; see [Controller and web frontend](#️-controller-and-web-frontend))
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
dpds <EAL args...> -- [--config <path>] [--config0 <path>] [--config1 <path>] | --links <path>
```

| Option               | Meaning                                                                                                   |
| -------------------- | --------------------------------------------------------------------------------------------------------- |
| `--config <path>`    | Emulation config file for every link without its own (default `config.json` in the working directory).      |
| `--config0 <path>`   | Config for link `dir0` (port 0 -> port 1, with one port port 0 -> port 0). Overrides `--config`.            |
| `--config1 <path>`   | Config for link `dir1` (port 1 -> port 0). Overrides `--config`. Error with only one port.                  |
| `--links <path>`     | Links file: any number of links, see [Several links](#several-links-in-one-process---links). Cannot be combined with the three `--config*` flags. |

The config of link `dir<d>` is `--config<d>` if given, else `--config`, else
`config.json`. With `--config*` the probed ports form the links `dir0` and, with
two ports, `dir1` (the shorthand names; `dir<d>` is also the link's index in
`/dpds/ports`). A flag given twice, an unknown flag, a missing value, a bad
config file (invalid distribution or half-life parameters are reported at startup, not as a panic), a
wrong lcore or port count, a pool that cannot be created, or a port whose link is down
makes `dpds` exit with a non-zero status and a message on stderr.

At startup DPDS prints, per port: name, applied and requested ring sizes,
socket, RX/TX burst mode and link; per link: lcore, ports, config path,
pool size and socket, and the free path (raw or pktmbuf).

### Real NICs

```bash
# 3 lcores (main + one worker per link), two NICs inline,
# a different emulation per direction (link dir0 = port 0 -> port 1).
# `,txq_mpw_en=1` is an mlx5 devarg: add it only on mlx5 ports (mlx5 only), not on other drivers.
sudo ./target/release/dpds -l 0-2 --in-memory -a <PCI0>,txq_mpw_en=1 -a <PCI1>,txq_mpw_en=1 -- \
    --config0 forward.json --config1 return.json
```

- `-l 0-2` - exactly three lcores for two ports: the main lcore and one worker
  per link. One port needs `-l 0-1`.
- `-a <PCI>` - NIC PCI address (e.g. `0000:65:00.0`); pass one for loopback,
  two for inline. The controller appends `,txq_mpw_en=1` (Enhanced Multi-Packet Write on TX,
  preof's setting) to every port bound to mlx5_core, e.g. `-a <PCI0>,txq_mpw_en=1`; when you
  run `dpds` by hand, use the same.
- `--in-memory` - like preof, always: it skips the hugepage file setup, so a crashed process
  leaves no state behind. The controller passes it too.

Tuning a testbed host (isolated cores, NIC settings) and launching, stopping and
syncing DPDS there is done by the separate evaluation automation, not by this
repository; the repository only provides the binary and the telemetry socket.
The tuning advice DPDS depends on is under
[Performance tuning](#-performance-tuning).

### Without hardware (virtual devices)

No root, hugepages or NIC needed, e.g. to try the setup or in CI. DPDS ships no
default config: the examples write theirs to a temporary file first. Run them from
the repository root:

```bash
# A config with a small buffer: its derived pool of 4096 + 4096 + 4096 + 768 + 64
# = 13,120 mbufs (about 25 MB) fits into -m 256.
cat > /tmp/dpds-demo.json <<'EOF'
{
  "buffer_capacity": 4096,
  "reordering": false,
  "delayer": {
    "distribution": "NormalDistribution",
    "mean_ns": 10000000,
    "std_ns": 3000000,
    "activate_correlation": true,
    "half_life_period_ns": 30000000,
    "initialization_rate_pps": 8100000
  },
  "limiter": {
    "mode": "Spacer",
    "activate": false,
    "rate_bps": 100000000000
  },
  "loss_model": {
    "Random": 0.0
  }
}
EOF

# One port that receives nothing and discards everything:
./target/release/dpds --no-huge -m 256 --no-pci --in-memory -l 0-1 \
    --vdev=net_null0 -- --config /tmp/dpds-demo.json

# Replay a pcap through DPDS, capture the result (1 port):
./target/release/dpds --no-huge -m 256 --no-pci --in-memory -l 0-1 \
    --vdev=net_pcap0,rx_pcap=in.pcap,tx_pcap=out.pcap -- --config /tmp/dpds-demo.json
```

The config is a 10 ms normally distributed delay with correlation, no rate limit and no loss;
`initialization_rate_pps` is 8.1 Mpps, 100 Gb/s of 1518 byte frames. The second example needs an
`in.pcap` in the working directory. A `buffer_capacity` of 1,000,000 derives about a million
mbufs (about 1.8 GiB) per link; to use such a config here, raise `-m` accordingly. Stop with
`Ctrl-C`.

- **Histograms**: a link entry may have `"histogram"` (delay) and `"iat_histogram"` (IAT), each
  `{"bins": 1..4096, "lo_ns": ..., "hi_ns": ...}` with every field optional (`lo_ns` >= 0, `lo_ns` < `hi_ns`).
  An omitted field takes the default: for the delay histogram the automatic range of the config, for the
  IAT histogram `lo_ns` 0, `hi_ns` `4 * 1e9 / initialization_rate_pps`, 256 bins. The controller sets both
  from the optional `histogram` / `iat_histogram` of a link's config.
- **Pools**: every link has its own mbuf pool on the socket of its ingress port.
  A port that is only an egress still has an RX ring that is filled, so its
  descriptors are added to the pool of the link that transmits on it.

Any problem in the file (a bad name, a duplicate name, a port twice in the same
role, an empty port name, an unknown key, a mix of links with and without `lcore`,
too many ports and links, a config with invalid parameters, a port that is not
probed, a probed port that no link uses, a wrong lcore count) is a clean startup
error naming the link, port or lcore concerned.

Example without hardware, a ring of three `net_null` devices (`-l 0-3`: the main
lcore and one worker per link), run from the repository root:

```bash
CONFIG='{"buffer_capacity":4096,"reordering":false,"delayer":{"distribution":"NormalDistribution","mean_ns":10000000,"std_ns":3000000,"activate_correlation":true,"half_life_period_ns":30000000,"initialization_rate_pps":8100000},"limiter":{"mode":"Spacer","activate":false,"rate_bps":100000000000},"loss_model":{"Random":0.0}}'
printf '{"links":[{"name":"a-to-b","ingress":"net_null0","egress":"net_null1","config":%s},{"name":"b-to-c","ingress":"net_null1","egress":"net_null2","config":%s},{"name":"c-to-a","ingress":"net_null2","egress":"net_null0","config":%s}]}' \
    "$CONFIG" "$CONFIG" "$CONFIG" > /tmp/links.vdev.json
./target/release/dpds --no-huge -m 512 --no-pci --in-memory -l 0-3 \
    --vdev=net_null0 --vdev=net_null1 --vdev=net_null2 -- --links /tmp/links.vdev.json
```

### Configuration

The emulation behaviour is controlled by one JSON config per link (a file for
`--config`, `--config0`, `--config1`, the `config` object of a link in `--links`). DPDS ships no
default config; the [virtual-device examples](#without-hardware-virtual-devices) write a small
one inline. Example with the Spacer enabled, bursty loss and a fixed seed:

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
    "half_life_period_ns": 50000000.0,
    "initialization_rate_pps": 810000.0
  },
  "limiter": {
    "mode": "Spacer",
    "activate": true,
    "rate_bps": 10000000000.0
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
| `reordering`                       | `false` (FIFO) · `true` (send-time order); not with `delayer.activate_correlation` and not with an active `Spacer` limiter (use `TokenBucketFilter`) |
| `seed`                             | Optional integer; absent: seeded from system entropy                                                                          |
| `delayer.distribution`             | `DeterministicDistribution` · `NormalDistribution` · `LogNormalDistribution` · `GammaDistribution` · `UniformDistribution`    |
| `delayer.mean_ns`, `delayer.std_ns` | Mean and standard deviation of the delay in ns (`std_ns` is ignored for `DeterministicDistribution`)                          |
| `delayer.activate_correlation`     | `true` · `false`: autocorrelation of successive delays; not with `reordering`                                                |
| `delayer.half_life_period_ns`      | Correlation half-life in ns (finite, not negative; not absurdly long)                                                                        |
| `delayer.initialization_rate_pps`  | Required, no default: expected packet rate of the link in packets per second (finite, at least 1); the correlated delay starts with it until the first 1 ms rate estimate. About 8.1 Mpps for 100 Gb/s with 1518 byte frames |
| `limiter.mode`                     | `Spacer` · `TokenBucketFilter`                                                                                                |
| `limiter.activate`                 | `true` · `false`: when `false` the limiter is a no-op                                                                         |
| `limiter.rate_bps`                 | Target rate in bits per second. The limiter's limit is not configured, see [The limiter's limit](#the-limiters-limit)         |
| `loss_model`                       | `{ "Random": p }` · `{ "GilbertElliot": [p, r, k, h, state_is_good] }`                                                        |

#### The limiter's limit

**API change:** `limiter.limit_bit` is removed; a config that still has it is
rejected (the error names the field). Both limiter modes use one derived limit,
`buffer_capacity x MAX_FRAME_WIRE_BYTES x 8` bit with `MAX_FRAME_WIRE_BYTES = 1542`
(1518 B maximum Ethernet frame + 4 FEC bytes + 20 B L1 overhead, as the limiter
counts a packet), i.e. `buffer_capacity x 12336` bit: exactly `buffer_capacity`
maximum-size frames fit. It is the Spacer's maximum in-flight backlog and the Token
Bucket Filter's bucket depth (the bucket starts full). The `dpds-core` buffer
evaluates a pushed packet in this order: **room check first** (a full buffer
rejects the packet and touches nothing else, so no packet is accounted that the
emulation never uses), then delay, limiter accounting, loss model, limiter verdict
and enqueue.

#### Reproducible Runs

Two fields matter for reproducible runs:

- **`reordering`**: with `false` packets leave in arrival order (FIFO); with
  `true` they leave in send-time order. Reordering and correlation are both
  settings for how accurately the configured mean delay and jitter are
  reproduced; reordering does not emulate a reordering characteristic of a
  network. It cannot be combined with `delayer.activate_correlation` and works
  only with the `TokenBucketFilter` limiter, not with an active `Spacer`; such a
  config is rejected ("reordering cannot be combined with correlation",
  "reordering works only with the TokenBucketFilter limiter, not Spacer"). In reordering mode packets with equal
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
| `/dpds/stats,<dir>` | Packet counters of link `<dir>` (its index in the [links file](#several-links-in-one-process---links); 0 or 1 with `--config*`) since start (`rx_pkts`, `dpds_rejects` and its causes `loss_drops` / `limiter_drops` / `buffer_full_drops`, `pops`, `tx_sent`, `tx_drops`, `out_of_order`, `frame_sizes` ...), gauges, delay statistics (`delay_mean_ns`, `delay_std_ns`, `delay_min_ns`, `delay_max_ns`: arrival until enqueued into the TX descriptors; `lateness_mean_ns`, `lateness_max_ns`: TX enqueue minus send time; absent for a link with `delay_stats` off), first-loss result. |
| `/dpds/epochs,<dir>` | The same per epoch (a burst of traffic, closed after 500 ms of silence). |
| `/dpds/delay_hist,<dir>[,<page>]` | Cumulative histogram of the delay of the packets handed to the NIC (arrival until enqueued into the TX descriptors, the TX staging wait included), up to 4096 bins, read in pages of at most 500 bins (`<page>` from 0, default 0; each page has `lo_ns`, `bin_ns`, `num_bins`, `underflow`, `overflow`, `page`, `first_bin` and its `bins`; empty page 0 for a link with `delay_stats` off). See [docs/TELEMETRY.md](docs/TELEMETRY.md). |
| `/dpds/iat_hist,<dir>[,<page>]` | Cumulative histogram of the IAT of link `<dir>`: the gap between the send times of consecutive packets that left the buffer (the departure gap `dpds` schedules, not a measured RX gap), exactly one `lo_ns` ... `bins` page like `/dpds/delay_hist`, by default 256 bins over `[0, 4 / initialization_rate_pps)` (4 mean gaps at the link's expected rate), or the link's `iat_histogram` (`bins`, `lo_ns`, `hi_ns`, each optional); empty for a link with `delay_stats` off. See [docs/TELEMETRY.md](docs/TELEMETRY.md). |
| `/dpds/ports` | Port names, descriptor ring sizes (applied and requested), the number of links, and per link its name, ports (ids and device names), lcore, config path, pool size, free path and whether its delay statistics are on (`3 + 5 * ports + 10 * links` keys, at most 256). |
| `/nic/hw_first_loss,<port>` | First NIC hardware drop (`imissed`, `rx_nombuf`, ...) seen by the main lcore. |
| `/nic/epochs,<port>` | Per-epoch NIC `rx_packets` / `imissed` / `rx_port_discards`. |

Query with the client that ships with DPDK (`--file-prefix` as given to DPDS,
and the same `XDG_RUNTIME_DIR` when running as non-root):

```bash
./target/release/dpds -l 0-2 --in-memory --file-prefix demo -a <PCI0> -a <PCI1> -- --config link.json &
echo '/dpds/stats,0' | dpdk-telemetry.py -f demo
echo '/dpds/ports' | dpdk-telemetry.py -f demo
```

The socket path is limited to 108 bytes; a longer `XDG_RUNTIME_DIR` /
`--file-prefix` makes DPDS fail at startup. Without the feature nothing is
registered and the instrumentation compiles to nothing.

A rejected packet counts as exactly one of `loss_drops` (the loss model dropped
it), `limiter_drops` (the limiter rejected it) or `buffer_full_drops` (the buffer
held `buffer_capacity` packets, checked first); `dpds_rejects` is their sum. When both the loss
model and the limiter would reject a packet it counts as a loss, so
`limiter_drops` can under-count what the limiter alone would reject while a loss
model is active.
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

For interactive use there is the controller with a REST API and a web frontend, see
[Controller and web frontend](#️-controller-and-web-frontend).

---

## 🖥️ Controller and web frontend

`dpds-controller` discovers the host's network interfaces, starts and stops one `dpds` process for
all links as its child process, and serves live telemetry. It stores nothing: it always starts
empty, and the configuration travels with every start request. The controller also serves the React web frontend: the built frontend is
embedded in the `dpds-controller` binary, so one service on one port gives the frontend at
`http://<host>:<controller port>/` (default port 8100) and the REST API under `/api`. There is no
Docker and no separate web server.

`dpds.sh` builds and installs both. It needs Rust (stable works; the channel below only
selects the git branch), clang and `ld.lld` of the same LLVM major version as `rustc`, DPDK at least 26.07
(`pkg-config --atleast-version=26.07 libdpdk`), and Node at least 22.22 (the `engines.node` of
`frontend/package.json`) with npm. `check`, `install` and `update` verify all of them first, list every
missing or too old tool at once and never install anything. `update` refuses a working tree with
uncommitted changes, warns when it switches from another branch, and pulls with `--ff-only`.

`install` and `update` build the frontend on the host: `dpds.sh` runs `npm ci` and then
`npm run build` in `frontend/` (this needs network access to the npm registry), checks that
`frontend/dist/index.html` exists and builds the controller with `DPDS_FRONTEND_DIST` pointing at
`frontend/dist`, so that the controller embeds the result. A failing npm step stops `dpds.sh` before
anything is built or installed. In `update` the frontend is built after the `git pull`. The controller
serves the frontend at `http://<host>:8100/` (the port of its bind address).

A plain `cargo build -p dpds-controller` embeds whatever `frontend/dist` holds, which may be stale
(an older `npm run build`), and otherwise a small placeholder page when `frontend/dist` has no
`index.html`. Set `DPDS_FRONTEND_DIST` to a built directory to embed that one instead (a directory without
an `index.html` is an error). `dpds.sh` never relies on that: it ignores a `DPDS_FRONTEND_DIST` from
the caller's environment and always passes the `frontend/dist` it has just built.

For frontend development, run `npm run dev` in `frontend/` (see `frontend/README.md`): the Vite dev
server on port 5173 proxies `/api` to a controller on `127.0.0.1:8100` (`DPDS_CONTROLLER` changes that).

```bash
./dpds.sh check      # prerequisites (Rust, clang/lld, DPDK, Node/npm), then the host hints (see below)
./dpds.sh install    # build dpds (telemetry) and the controller, host check, install binaries,
                     # systemd unit, /etc/dpds/controller.env; builds the frontend (npm) first
./dpds.sh start      # systemctl enable --now dpds-controller
./dpds.sh status     # service, GET /api/online and the installed version
./dpds.sh stop       # systemctl disable --now dpds-controller
./dpds.sh restart    # stop, then start
./dpds.sh update     # switch to the channel's branch and pull it, rebuild, reinstall; restarts only if it was running;
                     # does nothing when the installed commit is already the channel's head
./dpds.sh --nightly update # the same on the nightly channel (or DPDS_CHANNEL=nightly ./dpds.sh update)
DPDS_CHANNEL=v1.0.0 ./dpds.sh update # install exactly the release v1.0.0 (see the channel paragraph below)
./dpds.sh uninstall  # remove binaries and unit (and the /var/lib/dpds-controller of an older version); keeps controller.env
```

`install` records the installed commit in `/etc/dpds/installed-commit`; `update` does nothing (no build, no
reinstall, no restart) when that commit is already the channel's head, and `install` always rebuilds and
reinstalls, e.g. after a toolchain or DPDK upgrade. `uninstall` removes the record.

`--yes` skips the question that `install` and `update` ask after the host check. `status` prints the
channel, the branch of the checkout and the installed version, as reported by `/api/online`.

The release channel is set with the environment variable `DPDS_CHANNEL`, like P4TG's
`P4TG_CHANNEL`: `latest` (the default) follows the branch `main`, `nightly` follows the branch
`nightly`, and a release tag such as `v1.0.0` pins that release. Any other value is an error. The flags `--nightly` (channel `nightly`) and `--stable`
(channel `latest`) override the variable; giving both is an error. `update` refuses a working tree
with uncommitted changes, then runs `git fetch`, `git checkout <branch>` and `git pull --ff-only`
for the channel's branch before it builds anything, and warns when it switches from another branch
or from a detached HEAD. `install` makes no git call: it builds the tree as it is and warns when
the checkout is not on the channel's branch (run `update` to switch).

`DPDS_CHANNEL=v1.0.0 ./dpds.sh update` installs exactly that release. The value is a release tag of the
form `vX.Y.Z` (digits only, so `v1.0.0-rc1` and `1.0.0` are errors). `update` then runs `git fetch --tags`
and `git checkout --detach v1.0.0` (no pull; the checkout is detached) before it builds anything, and
refuses a tag that does not exist. Later `update`s keep that version until the channel changes, so use
`DPDS_CHANNEL=latest` or `--nightly` to leave it. Tags exist only once a version has reached `main`: CI
creates `vX.Y.Z` when a merge to `main` raises the version. `install` warns when the checkout is not at
the tag's commit (run `update`).

Until `nightly` is merged into `main`, `main` still holds the C version of DPDS, which has no
controller and no `dpds.sh`. For now use `--nightly` or `DPDS_CHANNEL=nightly` (for example
`DPDS_CHANNEL=nightly ./dpds.sh update`); the default channel `latest` only makes sense after that
merge.

### Versions and releases

The version of DPDS is in `VERSION.md` (first line, `X.Y.Z`). Every crate, `frontend/package.json`,
`frontend/package-lock.json`, `Cargo.lock` and the version badge above carry the same number; a test
fails when they disagree. The controller reports it as `version` in `GET /api/online` and
`GET /api/version`, and the web frontend shows it at the bottom of the sidebar.

Releases are made by CI. To publish one, raise the version in `VERSION.md`, the crates (the
`[workspace.package] version` in `Cargo.toml`, then `Cargo.lock`), `frontend/package.json`,
`frontend/package-lock.json` and the badge on the `nightly` branch, then merge `nightly` into `main`. When all
check jobs pass on `main`, the release job creates the tag `v<version>` on that commit and a GitHub
release with generated notes and no build assets. It does nothing when the tag exists, so merges
without a version bump release nothing. The releases are listed at
<https://github.com/uni-tue-kn/DPDS/releases>.

The web frontend checks once per page load whether `VERSION.md` on `main` is newer than the running
version. It asks `raw.githubusercontent.com` from the browser (the DPDS host makes no outbound
request) and, when a newer release exists, shows a red arrow next to the version and an "Update
available" notice. Update with `./dpds.sh update --stable`. Any failure of the check, for example
without internet access, means no notice.

- **Configuration lives in the browser.** The frontend keeps the links only in the
  browser's localStorage, as P4TG does, so another or a cleared browser starts empty. The
  Settings page, which lists and edits the links, has Import and Export buttons: Export downloads
  them as `dpds-links-<date>.json`, Import replaces all links with those of such a file. The
  "Duplicate" button of a link on the Settings page opens the new-link form pre-filled with a
  copy; you pick its ports. The controller reads and writes no configuration file, so an update can never leave it with a file it
  cannot load. While the emulation runs, `GET /api/emulation` reports the configuration it was
  started with in `config`, as the client sent it. The frontend compares it with its own to show
  "pending restart" and offers "Load running configuration" to a browser without links.
- **Start**: `POST /api/emulation/start` takes the links as its JSON body,
  `{"links": [{name, ingress, egress, config}]}`; unknown fields (such as a `settings` member or a
  link's core) are a 400 (`deny_unknown_fields`). It answers 202 and starts
  `dpds`, 400 when a rule is broken or `links` is empty (`nothing to start`), and 409 while the
  emulation is starting, running or stopping. Nothing is spawned on an error. Start also refuses
  (400) when the links' mbuf pools need more hugepage memory than is free, naming the need, the free
  memory and the link with the largest need (the estimate of the hugepage hint); a host that reports
  no hugepages is not refused (`dpds` reports it), and validate skips this check. When `dpds` exits
  with a non-zero status, the emulation's `error` is `dpds exited with exit status N: <reason>` (or
  `signal S`), the reason being the last output line starting with `dpds: ` that is not a
  `dpds: warning:` line, without the prefix; without such a line the error has no reason.
  `POST /api/emulation/validate` takes the same body, applies the same rules and answers 200
  `{main_lcore, directions: [{dir, link, rx, tx, lcore}]}`, the lcores a start would use, or the same
  400/409 errors. It starts nothing and ignores the state of the emulation; the frontend uses it
  to show problems before Start.
- **Links**: a link is one direction, `ingress` port to `egress` port, with one config
  (`{name, ingress, egress, config}` in the start body): packets received on the ingress
  leave on the egress with the config applied. `ingress` and `egress` may be the same port
  (hairpin). A port is the ingress of at most one link and the egress of at most one link (409
  naming the other link and the port), but its ingress and egress links need not share a partner:
  A to B, B to C and C to A are three links, and the classic bidirectional pair is two links (A to
  B and B to A). The cores are assigned automatically: the main lcore is the lowest online core that is
  not isolated, and each link gets its own isolated core as worker lcore, the lowest id first,
  preferring the NUMA node of the ingress NIC. Start fails when fewer isolated cores than links
  exist (set `isolcpus`) or when every online core is isolated. The links file handed to `dpds` always carries
  explicit lcores. The link index (the order of the start body) is the direction number of the
  telemetry and of `/api/stats/{dir}/delay_histogram`; per link, rx statistics come from the
  ingress port and tx statistics from the egress port. Hugepage and isolated-core hints count one
  pool and one core per link. The setup must fit `dpds`' 256-entry telemetry dict,
  `3 + 5 * distinct ports + 10 * links <= 256` (for example 16 links over 16 ports, but not 17):
  start and validate refuse a setup above it with 400 naming 256.
- **Telemetry poll**: the controller polls `dpds` every 1000 ms, a fixed interval that no request
  changes; the chart history keeps 1 sample per link per second.
- **Config**: every link carries the full `dpds-core` config (see
  [Configuration](#configuration)). The stages `delayer`, `limiter` and `loss_model` are optional:
  `null` or absent means the stage is not added, and the link runs with a stage that does nothing
  (no delay, no rate limit, no loss). The limiter has no `activate` flag; a present limiter is
  active. A present stage needs all its fields, including `delayer.initialization_rate_pps`, which
  has no default (400 naming the field when missing, not finite or below 1). Set it to the
  expected packet rate of the link.
- **Stats** (`GET /api/stats`): the rates and drop shares of the last poll per link,
  with `age_ms` and `stale`. `elapsed_ms` is the time since the run started in whole milliseconds,
  read from the controller's monotonic clock, so a client never compares its own clock with the
  controller's. It keeps counting while the run goes on, is frozen at the stop and is absent before
  any run; a new run restarts it near 0. A direction's `counters` also carry the cumulative NIC loss counts, read from the
  ports' xstats (`/ethdev/xstats`, polled once a second; a name the NIC does not report is 0; the NIC counters are reset
  when `dpds` starts, so they count from its start like the others). From the link's receiving (ingress) port:
  `nic_rx_missed` (`rx_missed_errors`, the same counter as `imissed` and mlx5 `rx_out_of_buffer`),
  `nic_rx_nombuf` (`rx_mbuf_allocation_errors`), `nic_rx_port_buffer_discards` (the sum of
  `rx_prio0..7_buf_discard_packets` and `rx_prio0..7_cong_discard_packets`) and `nic_rx_phy_discards`
  (`rx_phy_discard_packets`); from its transmitting (egress) port `nic_tx_phy_discards`
  (`tx_phy_discard_packets`, frames the NIC took from `tx_burst` but dropped before the wire). The diagnostics
  `nic_rx_crc_errors`, `nic_rx_symbol_errors` and `nic_rx_length_errors` (`rx_phy_crc_errors`, `rx_phy_symbol_errors`,
  `rx_phy_in_range_len_errors`) are shown but counted in no loss total. Two links that share a port (a hairpin of one
  port) both report that port's counters, unsplit. `nic_rx_pct` is the share of the window's received packets lost to the
  four RX-side NIC causes, `nic_tx_pct` the share lost to `tx_drops` plus `nic_tx_phy_discards`.
  `counters.frame_sizes` is an array of 8 cumulative counts of the frames the link received (their
  packet length, before any drop) in P4TG's buckets 0-63, 64, 65-127, 128-255, 256-511, 512-1023,
  1024-1518 and 1519 and larger bytes; `counters.out_of_order` is the cumulative number of packets
  that left before an earlier arrival (P4TG's notion, relative to arrival order; always 0 on a link
  without `reordering`). Both are cumulative passthroughs of `dpds` (0 when it reports none).
- **Chart history** (`GET /api/stats/history`, optionally with `?since_ms=<t>`): the answer is `{started_at, run_id,
  links, samples}`. `samples` has, per link index (the order of the start body), the series of the
  run, oldest first. Each sample is `{t_ms, rx_gbps, tx_gbps, rx_mpps, tx_mpps, loss_drops,
  limiter_drops, buffer_full_drops, nic_tx_drops, nic_rx_missed, nic_rx_nombuf, nic_rx_port_buffer_discards,
  nic_rx_phy_discards, nic_tx_phy_discards}`: the rates of `GET
  /api/stats` for the poll window ending at `t_ms` (whole milliseconds since the run started on
  the same monotonic clock as `elapsed_ms`) and the link's cumulative drop counters at that poll. The controller takes one sample per link per second
  (all links at the same `t_ms`) and keeps at most 86400 per link (24 hours), dropping the oldest.
  `since_ms` returns only the samples with `t_ms` above it; it takes digits only (a whole number
  of milliseconds), and a bad value or any other query parameter is a 400. `run_id` is one more
  with every new run the controller sees and null before any run; `links` is
  `[{name, ingress, egress}]` of that run in link order, `[]` before any run, so a client can title
  the charts of a finished run. The answer also has `totals`, per link index (like `samples`) an object
  of `rx_pkts`, `tx_sent` (the frames forwarded, `counters.tx_sent`), `frame_sizes`, `out_of_order`,
  `loss_drops`, `limiter_drops`, `buffer_full_drops`, `nic_tx_drops`,
  `nic_rx_missed`, `nic_rx_nombuf`, `nic_rx_port_buffer_discards`, `nic_rx_phy_discards` and
  `nic_tx_phy_discards`: the cumulative counters since the `dpds` process started, taken from the last
  poll of the run (not from the newest once-a-second sample), so a finished run's drops by cause
  stay readable. `nic_tx_drops` is `counters.tx_drops` of `GET /api/stats`. `totals` is in every
  answer, also with `since_ms`, and is the empty object `{}` before any run. A client that sees a new `run_id` starts over. The history is
  kept after the stop (the final charts stay readable), cleared at the next start and lost when
  the controller restarts, as it lives in memory only.
- **Frontend**: `http://<host>:<controller port>/`, served by the controller itself. Pages for the
  dashboard (start/stop, per-link rates, drop shares per cause, delay statistics and histogram),
  interfaces, host hints and settings (the links, with import and export).
- **Connected users** (`GET /api/online`): besides `version`, `dpds_bin`, `telemetry` and `root` it answers
  `connected_clients`, the number of OTHER sessions that polled `/api/online` in the last 15 s, like P4TG. A
  session is the value of the `X-Session-Id` request header (cut to 64 bytes; the frontend sends a random id per
  browser tab on every request), else the client IP. Only `GET /api/online` tracks. While `connected_clients` is
  above 0 the frontend shows a warning above the page ("Currently, n other users are connected to this DPDS
  instance").
- **REST API**: served by the controller on port 8100 (`/api/...`), the same port as the frontend.
  The interactive OpenAPI documentation is at `http://<host>:8100/api/docs`.
- **IAT histogram** (`GET /api/stats/{dir}/iat_histogram`): the histogram of the gap between the
  send times `dpds` schedules for consecutive packets of the link (the departure gap, exact per
  packet; recorded only for a link with delay statistics), 256 bins over `[0, hi)` with
  `hi = 4 * 1e9 / initialization_rate_pps` ns by default; the optional `iat_histogram` of a link
  config (`{bins, lo_ns, hi_ns}`, the same shape and validation as `histogram`, every field optional,
  an omitted one keeps this default; errors say "IAT histogram") changes that. The reply has the shape of the delay histogram's
  (`lo_ns`, `bin_ns`, `bins`, `underflow`, `overflow`, `edges_ns`, `count`, `percentiles`) since the
  start, without `expected*` and `window_s`; 404 without data or for a link without delay
  statistics. Like the delay histogram it is answered from a copy after the stop (refreshed while
  served and right before the stop, cleared at the next start).
- **Delay histogram extras** (`GET /api/stats/{dir}/delay_histogram`): `?window_s=10|60|300` returns
  only the samples of the last n seconds (difference to the poll that is at least n seconds old;
  `window_s` absent or 0 and runs younger than n seconds give the counts since start; `span_ms` is
  the time the counts cover). The controller keeps the polls of the last 300 s for this (at most
  6000) and drops them when the emulation
  stops. The reply also has `count` (samples including under- and overflow) and `percentiles`
  `p50`, `p90`, `p99`, `p999` in ns (linear interpolation inside the bin; a percentile in the
  under- or overflow is the range edge and its `p..._clamped` says `underflow` or `overflow`;
  all null without samples). `expected`, `expected_underflow` and `expected_overflow` are the
  counts a perfect sampler of the direction's configured delay distribution would put in each bin
  for `count` samples (negative delays are clamped to 0 by `dpds`, so their mass sits in the bin
  of 0). `expected_model` is `exact`, or `normal_approximation` with `activate_correlation`
  (the stationary distribution is approximated as Normal(mean, std)). With an active limiter
  `expected_note` says `excludes limiter queueing delay`: the expectation covers the delay
  distribution only, the limiter's queueing adds to the measured delay. The CDFs are a small
  self-contained implementation (regularised incomplete gamma; shapes above 1e5 use the
  Wilson-Hilferty approximation) with unit tests instead of `statrs`, which would pull in
  `nalgebra` and `rand`. They are not tuned for extreme parameters: a zero std (or a zero mean
  for LogNormal/Gamma) is treated as a point mass. The reply also has `mean_ns` and `std_ns`
  (the link's `delay_mean_ns` / `delay_std_ns` of the latest stats poll, null when unknown).
  The controller keeps a copy of the last since-start histogram of each link of the run
  (refreshed whenever one is served while running, and once more right before `dpds` is
  stopped); after the run ended (stopped or failed) the route answers that copy for any valid
  `window_s`, with `window_s: 0`, or 404 for a link without one. The copies are cleared at the
  next start.
- **Stopping** the service stops `dpds` cleanly: it drains the delay buffers first, which can
  take as long as the longest configured delay. The unit's `TimeoutStopSec` is 600 s; after
  that systemd kills `dpds` mid-drain. The controller adds a `drain_timeout` hint when a
  link of the configuration needs more than 590 s (delay mean + 6 std + limiter queue `buffer_capacity * 12336 bit / rate_bps`); shorten
  the delay, the buffer or the limiter queue for such a link.
- **Input allowlists**: the controller starts `dpds` as root, so it only accepts values it can
  vet. Ports are a PCI address or `net_null<N>` / `net_ring<N>` (no devargs, no `net_pcap`). The
  controller passes no extra EAL arguments. The mlx5 devargs are fixed to `txq_mpw_en=1` and not configurable. The
  uplink guard refuses to start a link on an interface the host uses (409, naming the link, the
  port and the reason). An interface is an uplink when it, or a bond, VLAN or bridge above it,
  (1) carries a route in `/proc/net/route` or `/proc/net/ipv6_route`, (2) has an IP address
  according to `getifaddrs` (any IPv4 address, including 169.254/16, or an IPv6 address that is
  not link-local; a static address counts while the interface is down), or (3) is a port of Open
  vSwitch. The interface list shows uplinks disabled with the reason, and the `uplink` host hint is
  an error for a configured link on one. Blind spots: an interface without any address whose only
  routes are in a non-main policy-routing table, and a DHCP uplink that is down and has lost its
  lease, look free.
- **DPDS never changes the host.** Neither `dpds.sh` (which never tunes anything), the controller
  nor the frontend writes a kernel parameter, a sysfs or procfs file, a hugepage count, a NIC
  setting or a firmware value, and the API has no route that does. A hint that can be fixed shows
  the commands instead (see **Hint commands**); an administrator with a shell runs them as root.
- **Host check**: `dpds.sh check` and `install` run `dpds-controller check`, which prints one
  `OK`/`INFO`/`WARN`/`ERROR` line per hint. Under every hint that has commands it prints an indented
  `# run as root:` line and then one indented line per command, exactly as in the hint's `commands`;
  `dpds-controller check --json` carries the `commands` member. Host-wide hints: core isolation
  (`isolcpus`, `nohz_full`, `rcu_nocbs`), isolated cores, hugepages, NUMA locality, CPU governor,
  IOMMU, MTU, `vm.swappiness` and `vm.zone_reclaim_mode`. For each PCI NIC of the configured links
  there are further hints: Ethernet flow control (`flow_control:<pci>`, needs `ethtool`), PCIe
  MaxReadReq (`pcie_maxreadreq:<pci>`) and, for Mellanox NICs on `mlx5_core`, the port buffer
  (`port_buffer:<pci>`, needs `dcb` from the `iproute2` package) and the firmware settings
  (`firmware:<pci>`, needs `mstconfig` from the `mstflint` package; `mlxconfig` of the NVIDIA MFT
  works too). `GET /api/host/hints` serves the hints that depend on the host only and the frontend
  shows them on its Host hints page. The controller computes them in a background task right
  after it starts serving (startup is not delayed), together with the firmware settings of every
  mlx5 NIC of the host; the GET answers that stored result without recomputing, and waits for the
  task while it still runs. `GET /api/host/hints?recheck=true` computes the host hints and queries
  the firmware of all NICs again, replaces the stored result and answers it (same query rules as
  the POST's `recheck`). `POST /api/host/hints` takes a start body, validated like the
  start body (400 naming the field, 409 for a duplicate name or reused port), and adds the
  link-dependent hints, which need the links to know the NICs (NUMA locality, hugepages for the
  links, NIC interrupts (`irq_affinity`), host uplink, drain timeout, MTU and the per-NIC hints of
  the used NICs); the frontend calls it with its current configuration. The firmware query is slow,
  so its successful result is kept in a cache per card (function 0) until the controller restarts,
  or until a recheck drops it with `POST /api/host/hints?recheck=true` (the frontend's Recheck
  button, which also sends `GET ?recheck=true`; the first load on opening the page sends neither);
  `recheck=false` or no query is the normal behaviour, and any other value or query key is
  a 400 naming `recheck`, before anything runs. A cached result runs no tool and costs no budget
  time. It is used while an emulation is starting, running or stopping, also with `recheck=true`,
  and a NIC without a cached result stays "not checked while an emulation is running". The detail
  of a cached hint ends with "(firmware values read <N> s ago; Recheck to read them again)". A
  failure, a timeout, a missing tool and a used-up budget are never cached, and `ethtool` and `dcb`
  are not cached at all. `dpds-controller check` is another process with no cache and no recheck.
  Warnings do not block the installation.
- **Hint commands**: a hint that can be fixed has an optional `commands` array (omitted when empty):
  the exact shell commands an administrator runs as root to enable the tuning on this host, computed
  by the controller from what it read (netdev names, PCI addresses, interrupt numbers, core lists,
  page counts), with the real `/proc` and `/sys` paths. DPDS runs none of them. A command that
  writes to a file below `/proc` or `/sys` (the `echo ... >` ones) needs a root shell (`sudo -i`) or
  the value piped to `sudo tee <file>`: `sudo echo ... > <file>` does not work, because the calling
  shell opens the file, not sudo. They are `sysctl -w vm.swappiness=0` and `sysctl -w
  vm.zone_reclaim_mode=0`; one `echo <cores> > /proc/irq/<n>/smp_affinity_list` per NIC interrupt on
  an isolated core, moving it to the non-isolated online cores of the NIC's NUMA node (all
  non-isolated online cores when that node has none or is unknown; a NIC the host uses or DPDS
  cannot use is skipped and named); `ethtool -A <netdev> rx off tx off` (plus `autoneg off` while
  autonegotiation is on); `setpci -s <pci> CAP_EXP+8.w=5000:7000` for MaxReadReq 4096 B; `dcb buffer
  set dev <netdev> buffer-size all:0 <i>:<total>` for the port buffer, only when all priorities
  share one buffer and flow control is already off; `echo performance >
  /sys/devices/system/cpu/cpu<n>/cpufreq/scaling_governor` per core; `echo <n> >
  .../hugepages-<size>kB/nr_hugepages` (per NUMA node of the NICs when known), where the pages added
  never exceed half of `MemAvailable` (the detail says when that cap applies); and `mstconfig -d
  <function 0> -y set ADVANCED_PCI_SETTINGS=1`, then `... set MAX_ACC_OUT_READ=256
  CQE_COMPRESSION=1` (`mlxconfig` when only that is installed). The firmware values are written to
  the NVRAM of the whole card, persist and take effect after a firmware reset that DPDS never runs
  (on a VM, from the hypervisor host); MaxReadReq read inside a VM is the hypervisor's copy of the
  register, so verify it on the hypervisor host. A missing tool is an `INFO` hint that names its
  package and has the command `apt install <package>`. No command is suggested for a NIC the host
  uses or that DPDS cannot use (the detail says why; the `apt install` of a missing tool stays), for
  MaxReadReq of a virtual function, or for a netdev whose name is not safe to put into a shell
  command (only letters, digits, `_`, `.`, `@` and `-`, not starting with `-`; no tool runs for it
  either). The controller only runs read-only queries (`ethtool --json -a`, `dcb -j buffer show`,
  `mstconfig -j ... q`), each in its own process group with a timeout of at most 30 s and at most 4
  MiB of output per stream. The queries of one request share a budget of 60 s; once it is used up
  the remaining hints say "not checked (time budget of 60 s used up)". No query runs while an
  emulation is starting, running or stopping: those hints say "not checked while an emulation is
  running".
- **Kernel command line** (`rcu_nocb_poll` and boot-time 1 GiB hugepages, `default_hugepagesz=1G
  hugepagesz=1G hugepages=<n>`) is checked next to the core isolation parameters. These are
  hint-only: the detail names the parameters to add, and there are no commands.
- **Security**: the controller runs as root and listens on all interfaces without
  authentication. Anyone who can reach the port can start and stop `dpds` with their own
  (validated) configuration and read the host hints, but cannot change the host. Set `DPDS_BIND` before `install` (for example `DPDS_BIND=127.0.0.1:8100`) or edit
  `DPDS_CONTROLLER_BIND` in `/etc/dpds/controller.env` and restart to restrict it. The frontend
  has no listener of its own: the controller's port serves both the frontend and the full API
  (start/stop included), so the controller's bind restricts everything. Further controller
  options, set as flags of `dpds-controller serve` or as environment variables in `controller.env`:
  - `--allowed-host` / `DPDS_ALLOWED_HOST` (repeatable; comma-separated in the variable): the
    `Host` header of every request, the frontend's files included, is checked against DNS rebinding. IP literals,
    `localhost` and the machine's hostname are always allowed; add any other name you use to reach
    the controller (and so the frontend), or requests get 421/403.
  - `--cors-origin` / `DPDS_CORS_ORIGIN` (repeatable): CORS is off by default. Only the listed
    origins are echoed. `*` and origins with a path or trailing slash are rejected at startup.

`dpds.sh status` probes the controller at the bind address from `controller.env`.

`dpds.sh` reads `DPDS_PREFIX`, `DPDS_ETC_DIR`, `DPDS_SYSTEMD_DIR` and
`DPDS_BIND` (defaults `/usr/local`, `/etc/dpds`, `/etc/systemd/system`, `0.0.0.0:8100`); `DPDS_BIND` only applies when `controller.env` is
created, which `install` never overwrites.

---

## 📚 Documentation

- Emulation library, delay/limiter/loss configuration and Rust API: [`crates/dpds-core/README.md`](crates/dpds-core/README.md)
- Telemetry commands and every field: [docs/TELEMETRY.md](docs/TELEMETRY.md)
- Paper: [DPDS: A DPDK-Based Packet Delayer and Spacer](https://arxiv.org/abs/2606.17716)

---

## 🛠️ Development

```
crates/dpds        The data plane binary: CLI, per-link worker loop, telemetry
crates/dpds-core   Emulation library: delay, rate limiter, loss, packet buffer
crates/dpdk        Safe wrapper around the DPDK calls the data plane needs
crates/dpdk-sys    Raw bindgen bindings to DPDK
Makefile           build / check-static-link (see below)
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
