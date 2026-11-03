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
  default `config.json` (`buffer_capacity` 1,000,000) with 4096-descriptor rings
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

- **DPDK 26.07** (found through `pkg-config libdpdk`; the build needs at least 26.07),
  and for real traffic a DPDK-capable NIC (tested on Mellanox ConnectX)
- **Rust** (stable toolchain)
- **clang and lld, with the same LLVM major version as `rustc`** (see [Building](#building))
- **Node 18.18 or newer and npm**, plus network access to the npm registry, to build the web
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
# a different emulation per direction (link dir0 = port 0 -> port 1):
sudo ./target/release/dpds -l 0-2 -a <PCI0> -a <PCI1> -- \
    --config0 forward.json --config1 return.json
```

- `-l 0-2` - exactly three lcores for two ports: the main lcore and one worker
  per link. One port needs `-l 0-1`.
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
about a million mbufs (about 1.8 GiB) per link; to use it here, raise `-m`
accordingly. The second example needs an `in.pcap` in the working directory.
Stop with `Ctrl-C`.

### Several links in one process (`--links`)

`--links <file>` replaces the `--config*` flags and describes every link of the
process in one JSON file: one main lcore plus one worker lcore per link.
Starting or stopping DPDS applies to all links at once.

A **link is one direction**: packets that arrive on its `ingress` port leave on
its `egress` port with the link's one config applied. Each port may be the
ingress of at most one link and the egress of at most one link, but its two
partners need not be the same. NIC A -> NIC B, NIC B -> NIC C and NIC C -> NIC A
are three links (a ring); the classic bump-in-the-wire is the two links A -> B
and B -> A; `ingress == egress` is a loopback on one port.

```json
{ "links": [
  { "name": "a-to-b", "ingress": "<PCI_A>", "egress": "<PCI_B>",
    "config": { "...": "Config for A -> B" },
    "lcore": 3,
    "histogram": { "bins": 100, "lo_ns": 0, "hi_ns": 2000000 },
    "delay_stats": false },
  { "name": "b-to-c", "ingress": "<PCI_B>", "egress": "<PCI_C>",
    "config": { "...": "Config for B -> C" },
    "lcore": 2 },
  { "name": "c-to-a", "ingress": "<PCI_C>", "egress": "<PCI_A>",
    "config": { "...": "Config for C -> A" },
    "lcore": 1 }
] }
```

The bidirectional pair is the same thing with two entries, with one config per
direction (use the same config twice for a symmetric link):

```json
{ "links": [
  { "name": "forward", "ingress": "<PCI0>", "egress": "<PCI1>", "config": { "...": "Config for <PCI0> -> <PCI1>" } },
  { "name": "return",  "ingress": "<PCI1>", "egress": "<PCI0>", "config": { "...": "Config for <PCI1> -> <PCI0>" } }
] }
```

Every `config` is a complete [config](#configuration) (the `"..."` keys above
stand for its fields). `lcore`, `histogram` and `delay_stats` are optional. A key
the format does not know (a typo such as `lcores`, or the keys `ports`,
`configs`, `histograms` of the removed paired-link format) is a startup error
naming it.

- **Ports** are DPDK device names: a PCI address as DPDK reports it
  (`0000:65:00.0`) or a virtual device name (`net_null0`, `net_pcap0`). They are
  matched against the probed devices by name, never by port id, so the order of
  the `-a` / `--vdev` arguments does not matter. A port used as the ingress of two
  links, or as the egress of two links, is a startup error naming the port and
  the role. Every device DPDK probes must be used by some link (as ingress, as
  egress or both), otherwise startup fails naming it: allow only the devices you
  use. A port that is only an egress is never polled; one that is only an ingress
  never transmits. The RX queue of an egress-only port is never polled, so any
  traffic that arrives on that wire fills its ring and then shows up as NIC
  `imissed` (mlx5: `rx_out_of_buffer`) for that port, and with telemetry in
  `/nic/hw_first_loss,<port>`. Keep that wire free of traffic, or expect those
  counters to rise.
- **`name`**: 1 to 32 characters of `[A-Za-z0-9_.-]`, unique. It appears in
  `/dpds/ports` and in error messages. (With `--config*` the links are named
  `dir0` and `dir1`.)
- **Link index**: links are numbered in file order; the telemetry commands take
  this number (`/dpds/stats,<dir>`, the `dir<d>_*` keys of `/dpds/ports`).
- **`lcore`**: the EAL must have exactly `1 + number of links` lcores. Without
  `lcore` the i-th worker lcore (ascending lcore id) runs link i. With `lcore`
  the link runs exactly on the listed lcore. Either every link has `lcore` or
  none; the lcores must be distinct, must be EAL worker lcores (not the main
  lcore) and together must cover every worker lcore. `/dpds/ports`
  `dir<d>_lcore` reports the lcore actually used. This lets a caller place each
  link next to its ingress NIC's NUMA node.
- **`histogram`**: the range of the [delay histogram](docs/TELEMETRY.md#dpdsdelay_histdir).
  Every field is optional: `bins` (an integer in 1..=500, default 256), `lo_ns`
  and `hi_ns` (both >= 0, `lo_ns` < `hi_ns`). A missing field takes the
  automatic value: the range `mean +- 6 std` of the link's config, widened by the
  Spacer's queueing allowance and at least 1 us wide, clamped at 0 (the exact
  rule is in [docs/TELEMETRY.md](docs/TELEMETRY.md#dpdsdelay_histdir)). The
  histogram exists only in `--features telemetry` builds.
- **`delay_stats`** (boolean, default `true`): `false` switches the delay
  statistics and the histogram of that link off. Its worker then records neither
  the intended delay, the lateness nor the histogram (the loop is the one of the
  counters only, no per-packet and no per-burst cost); counters, drop causes and
  first-loss stay. The six `delay_*` / `lateness_*` keys of `/dpds/stats,<dir>`
  are absent, `/dpds/delay_hist,<dir>` is empty and `/dpds/ports` shows
  `dir<d>_delay_stats` 0 (see
  [docs/TELEMETRY.md](docs/TELEMETRY.md#delay-statistics-off)).
- **Telemetry limit**: `/dpds/ports` must fit DPDK's telemetry dict of 256
  entries (`3 + 5 * ports + 10 * links` keys, `ports` = distinct ports): a ring
  of 16 ports (16 links) fits, 17 does not; 12 one-way links over 24 ports fit,
  13 do not. More is a startup error naming the maximum 256.
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
CONFIG=$(tr -d '\n ' < config.vdev.json)
printf '{"links":[{"name":"a-to-b","ingress":"net_null0","egress":"net_null1","config":%s},{"name":"b-to-c","ingress":"net_null1","egress":"net_null2","config":%s},{"name":"c-to-a","ingress":"net_null2","egress":"net_null0","config":%s}]}' \
    "$CONFIG" "$CONFIG" "$CONFIG" > /tmp/links.vdev.json
./target/release/dpds --no-huge -m 512 --no-pci --in-memory -l 0-3 \
    --vdev=net_null0 --vdev=net_null1 --vdev=net_null2 -- --links /tmp/links.vdev.json
```

### Configuration

The emulation behaviour is controlled by one JSON config per link (a file for
`--config`, `--config0`, `--config1`, the `config` object of a link in `--links`). The `config.json` in this repository is a
working default (10 ms normally distributed delay, no rate limit, no loss,
`initialization_rate_pps` for 100 Gb/s with 1518 byte frames);
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
    "half_life_period_ns": 50000000.0,
    "initialization_rate_pps": 810000.0
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
| `delayer.initialization_rate_pps`  | Required, no default: expected packet rate of the link in packets per second (finite, at least 1); the correlated delay starts with it until the first 1 ms rate estimate. About 8.1 Mpps for 100 Gb/s with 1518 byte frames |
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
| `/dpds/stats,<dir>` | Packet counters of link `<dir>` (its index in the [links file](#several-links-in-one-process---links); 0 or 1 with `--config*`) since start (`rx_pkts`, `dpds_rejects` and its causes `loss_drops` / `limiter_drops` / `buffer_full_drops`, `pops`, `tx_sent`, `tx_drops`, ...), gauges, delay statistics (`delay_mean_ns`, `delay_std_ns`, `delay_min_ns`, `delay_max_ns`, `lateness_mean_ns`, `lateness_max_ns`; absent for a link with `delay_stats` off), first-loss result. |
| `/dpds/epochs,<dir>` | The same per epoch (a burst of traffic, closed after 500 ms of silence). |
| `/dpds/delay_hist,<dir>` | Cumulative histogram of the intended delay of the accepted packets (`lo_ns`, `bin_ns`, `num_bins`, `bins`, `underflow`, `overflow`; empty for a link with `delay_stats` off). |
| `/dpds/ports` | Port names, descriptor ring sizes (applied and requested), the number of links, and per link its name, ports (ids and device names), lcore, config path, pool size, free path and whether its delay statistics are on (`3 + 5 * ports + 10 * links` keys, at most 256). |
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

A rejected packet counts as exactly one of `loss_drops` (the loss model dropped
it), `limiter_drops` (the limiter rejected it) or `buffer_full_drops` (the buffer
held `buffer_capacity` packets); `dpds_rejects` is their sum. When both the loss
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

For interactive use there is the controller with a REST API and a web frontend, see
[Controller and web frontend](#️-controller-and-web-frontend).

---

## 🖥️ Controller and web frontend

`dpds-controller` stores links and their configuration, discovers the host's
network interfaces, starts and stops one `dpds` process for all links as its child process, and
serves live telemetry. The controller also serves the React web frontend: the built frontend is
embedded in the `dpds-controller` binary, so one service on one port gives the frontend at
`http://<host>:<controller port>/` (default port 8100) and the REST API under `/api`. There is no
Docker and no separate web server.

`dpds.sh` builds and installs both. It needs Rust (stable works; `--nightly` only selects the
`nightly` git branch), clang and `ld.lld` of the same LLVM major version as `rustc`, DPDK at least 26.07
(`pkg-config --atleast-version=26.07 libdpdk`), and Node at least 18.18 (the `engines.node` of
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
./dpds.sh status     # service, GET /api/online
./dpds.sh stop       # systemctl disable --now dpds-controller
./dpds.sh restart    # stop, then start
./dpds.sh update     # git pull nightly, rebuild, reinstall; restarts only if it was running
./dpds.sh uninstall  # remove binaries and unit; keeps the state dir and controller.env
```

`--yes` skips the question that `install` and `update` ask after the host check. `--nightly` is
the default; `--stable` only prints that no Rust stable release exists yet.

- **Links**: a link is one direction, `ingress` port to `egress` port, with one config
  (`{name, ingress, egress, config, worker?}`; `GET|POST /api/links`, `GET|PUT|DELETE
  /api/links/{id}`): packets received on the ingress leave on the egress with the config
  applied. `ingress` and `egress` may be the same port (hairpin). A port is the ingress of at
  most one link and the egress of at most one link (409 naming the other link and the port),
  but its ingress and egress links need not share a partner: A to B, B to C and C to A are
  three links, and the classic bidirectional pair is two links (A to B and B to A). Each link
  gets one worker lcore: `worker` if set, else a free isolated core, preferring the NUMA node of
  the ingress NIC. The links file handed to `dpds` always carries explicit lcores. The link
  index (the order of `GET /api/links`) is the direction number of the telemetry and of
  `/api/stats/{dir}/delay_histogram`; per link, rx statistics come from the ingress port and tx
  statistics from the egress port. Hugepage and isolated-core hints count one pool and one core
  per link. A state file with the older two-port link shape is refused at startup with an error
  naming the file; re-create the links. Link names are checked again when the emulation starts.
  The setup must fit `dpds`' 256-entry telemetry dict, `3 + 5 * distinct ports + 10 * links <= 256`
  (for example 16 links over 16 ports, but not 17): create, update, import and start refuse a
  setup above it with 400 naming 256.
- **Config**: every link and preset carries the full `dpds-core` config (see
  [Configuration](#configuration)), including `delayer.initialization_rate_pps`, which is required
  and has no default (400 naming the field when missing, not finite or below 1). Set it to the
  expected packet rate of the link. It is part of presets, export/import and the links file.
- **Frontend**: `http://<host>:<controller port>/`, served by the controller itself. Pages for the dashboard (start/stop, per-link rates, drop
  shares per cause, delay statistics and histogram), links, interfaces, host hints and settings.
- **REST API**: served by the controller on port 8100 (`/api/...`), the same port as the frontend. The
  interactive OpenAPI documentation is at `http://<host>:8100/api/docs`.
- **Stored state** lives in `/var/lib/dpds-controller` and survives `stop` and `update`.
- **Presets**: named configs (one per link) stored beside the links (`GET|POST /api/presets`,
  `GET|PUT|DELETE /api/presets/{id}`; body `{name, description?, config}`). The name has 1 to 64
  characters and is unique (409); the config is validated like a link config (400 naming the
  field). Presets are not part of a run, so editing them while `dpds` runs does not set
  `pending_restart`. Preset ids are never reused.
- **Export and import**: `GET /api/export` returns the whole setup (`format`
  `dpds-controller-export`, `version` 2, `settings`, `links`, `presets`; no ids, no runtime state).
  `POST /api/import` (`?mode=replace`, the only mode) replaces links, presets and settings with such a
  document. It is refused with 409 unless the emulation is `stopped`, checks the whole document
  first (format and version, every link, preset and setting by the rules of the single endpoints,
  unique names, no port reused as ingress or egress, unknown keys such as `id` are 400) and applies it
  all or nothing in one state write. New ids continue the counters. The reply is
  `{"links": n, "presets": m}`. A version 1 document (links with `ports` and `configs`) is
  refused with 400.
- **Delay histogram extras** (`GET /api/stats/{dir}/delay_histogram`): `?window_s=10|60|300` returns
  only the samples of the last n seconds (difference to the poll that is at least n seconds old;
  `window_s` absent or 0 and runs younger than n seconds give the counts since start; `span_ms` is
  the time the counts cover). The controller keeps the polls of the last 300 s for this (at most
  6000, so a poll interval below 50 ms shortens the window) and drops them when the emulation
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
  for LogNormal/Gamma) is treated as a point mass.
- **Stopping** the service stops `dpds` cleanly: it drains the delay buffers first, which can
  take as long as the longest configured delay. The unit's `TimeoutStopSec` is 600 s; after
  that systemd kills `dpds` mid-drain. The controller adds a `drain_timeout` hint when a stored
  link needs more than 590 s (delay mean + 6 std + limiter queue `limit_bit / rate_bps`); shorten
  the delay or the limiter queue for such a link.
- **Input allowlists**: the controller starts `dpds` as root, so it only accepts values it can
  vet. Ports are a PCI address or `net_null<N>` / `net_ring<N>` (no devargs, no `net_pcap`). Extra
  EAL arguments are limited to `--no-huge`, `--no-pci`, `-m <n>`, `-n <n>`, `--socket-mem=<list>`
  and `--log-level=<spec>`. mlx5 devargs come from a fixed list of keys; `mprq` is never set. The
  management-NIC guard refuses to start a link on an interface that carries an IP route (409).
- **Host check**: `dpds.sh check` and `install` run `dpds-controller check`, which prints one
  `OK`/`INFO`/`WARN`/`ERROR` line per hint: isolated cores (`isolcpus`, `nohz_full`,
  `rcu_nocbs`), hugepages, NIC driver, NUMA locality, CPU governor, irqbalance, IOMMU, MTU, PCIe
  MaxReadReq. The same hints are served at `GET /api/host/hints` and shown in the frontend. Warnings do
  not block the installation.
- **Tuning is never applied.** Neither `dpds.sh`, the controller nor the frontend changes kernel
  parameters, hugepages, NIC settings or governors; they only report hints and suggestions.
- **Security**: the controller runs as root and listens on all interfaces without
  authentication. Anyone who can reach the port can start `dpds` with their own (allowlisted)
  configuration. Set `DPDS_BIND` before `install` (for example `DPDS_BIND=127.0.0.1:8100`) or edit
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

`dpds.sh` reads `DPDS_PREFIX`, `DPDS_ETC_DIR`, `DPDS_STATE_DIR`, `DPDS_SYSTEMD_DIR` and
`DPDS_BIND` (defaults `/usr/local`, `/etc/dpds`, `/var/lib/dpds-controller`,
`/etc/systemd/system`, `0.0.0.0:8100`); `DPDS_BIND` only applies when `controller.env` is
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
