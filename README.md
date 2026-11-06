<div align="center">
  <img src="./docs/img/logo.png" alt="DPDS Logo" width="400"/>

  <h2>DPDS: A DPDK-Based Packet Delayer and Spacer</h2>

  ![License](https://img.shields.io/badge/licence-Apache%202.0-blue)
  ![Backend](https://img.shields.io/badge/backend-Rust-b7410e?logo=rust)
  ![Frontend](https://img.shields.io/badge/frontend-React-149eca?logo=react)
  ![DPDK](https://img.shields.io/badge/dataplane-DPDK%2026.07-1e5aa8)
  ![Version](https://img.shields.io/badge/v-1.0.0-yellow)
  [![CI](https://github.com/uni-tue-kn/DPDS/actions/workflows/ci.yml/badge.svg?branch=nightly)](https://github.com/uni-tue-kn/DPDS/actions/workflows/ci.yml)
</div>

---

## Overview

**DPDS** is a software **network-link emulator** written in Rust on top of [DPDK](https://www.dpdk.org/).
It sits between network interfaces and applies the three effects of a real link to a live packet
stream at line rate: **delay**, **rate limiting** and **packet loss**, with zero copies and no kernel
on the fast path. The emulation logic is the reusable library [`dpds-core`](crates/dpds-core/README.md).
This is the Rust port of the original C application.

Paper: [DPDS: A DPDK-Based Packet Delayer and Spacer](https://arxiv.org/abs/2606.17716) (arXiv:2606.17716)

- **Delay:** deterministic, normal, log-normal, gamma or uniform, optionally autocorrelated, with
  FIFO or send-time (reordering) packet order
- **Rate limiting:** Spacer (spreads packets out) or Token Bucket Filter (drops excess)
- **Loss:** random or Gilbert–Elliott (bursty); an optional `seed` makes runs reproducible
- **Directed links:** a link is one direction, ingress port to egress port, with its own config,
  worker lcore, mbuf pool and counters. Two links make a bump-in-the-wire; three NICs can form a
  ring; one port can loop back.
- **Observable:** counters, drop causes, delay statistics and histograms on DPDK's telemetry socket
- **Controller and web frontend** to configure, start and stop DPDS (experimental)

---

## Requirements

- **DPDK 26.07 or newer** (`pkg-config libdpdk`); for real traffic a DPDK-capable NIC (tested on
  Mellanox ConnectX) and hugepages
- **Rust** (stable) and **clang and lld with the same LLVM major version as `rustc`**
- **Node 22.22 or newer and npm** for the web frontend (controller only); Linux

## Building

```bash
cargo build --release                        # plain: no telemetry
cargo build --release --features telemetry   # with DPDK telemetry commands
```

The binary is `target/release/dpds` (`make build [FEATURES=telemetry]` builds the same).

The workspace builds with `-C target-cpu=native` and cross-language LTO (`.cargo/config.toml`):
`lld` inlines DPDK's `static inline` burst functions into the forwarding loops. This only works if
`clang` and `rustc` use the **same LLVM major version** (compare `rustc -vV` and `clang --version`).
A mismatch does not fail the build, but nothing is inlined and throughput drops;
`make check-static-link` detects it. With `rustc` on LLVM 23, put `/usr/lib/llvm-23/bin` first in
`PATH`. Without clang/lld, drop LTO and the lld requirement (`RUSTDOCFLAGS` is for doctests):

```bash
RUSTFLAGS='-Ctarget-cpu=native' RUSTDOCFLAGS='' cargo build --release
```

---

## Usage

DPDS takes DPDK's EAL arguments, then `--`, then its own options:

```
dpds <EAL args...> -- [--config <path>] [--config0 <path>] [--config1 <path>] | --links <path>
```

- `--config` is the config of every link without its own (default `config.json`); `--config0` and
  `--config1` set links `dir0` (port 0 -> port 1, or port 0 -> port 0 with one port) and `dir1`
  (port 1 -> port 0). They allow one or two ports.
- `--links` takes a links file with any number of links (below); it excludes the `--config*` flags.
- DPDS needs exactly `1 + number of links` lcores: the main lcore and one worker per link.
- DPDS locks its memory (`mlockall`) at startup. If a finite `RLIMIT_MEMLOCK` prevents that it warns
  and runs unlocked: run as root or with `ulimit -l unlimited`.
- `SIGINT`/`SIGTERM` drain the delay buffers before exit, which takes as long as the longest delay.

### Real NICs

```bash
# main lcore + one worker per link, two NICs inline, a different emulation per direction.
# txq_mpw_en=1 is an mlx5-only devarg: leave it off on other drivers.
sudo ./target/release/dpds -l 0-2 --in-memory -a <PCI0>,txq_mpw_en=1 -a <PCI1>,txq_mpw_en=1 -- \
    --config0 forward.json --config1 return.json
```

`-a <PCI>` is a NIC PCI address (one for loopback with `-l 0-1`, two for inline). `--in-memory`
skips the hugepage file setup, so a crashed process leaves no state behind.

### Without hardware

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

No root, hugepages or NIC needed. Run from the repository root. The config is a 10 ms normally
distributed delay with correlation, no limit and no loss; the second command needs an `in.pcap`.
Stop with `Ctrl-C`.

### The links file (`--links`)

```json
{ "links": [
  { "name": "a-to-b", "ingress": "<PCI0>", "egress": "<PCI1>", "config": { ... } },
  { "name": "b-to-c", "ingress": "<PCI1>", "egress": "<PCI2>", "config": { ... } },
  { "name": "c-to-a", "ingress": "<PCI2>", "egress": "<PCI0>", "config": { ... } }
] }
```

This ring over three NICs runs with `-l 0-3 -a <PCI0> -a <PCI1> -a <PCI2> -- --links ring.json`.

- Required per link: `name` (1-32 characters of `A-Za-z0-9_.-`, unique), `ingress` and `egress`
  (DPDK device names: PCI address or vdev name) and `config` (below). A port is the ingress of at
  most one link and the egress of at most one; `ingress == egress` loops back.
- Optional: `lcore` (for all links or none), `delay_stats: false` (no delay statistics or histogram
  for the link), `histogram` / `iat_histogram` (`{"bins": 1..4096, "lo_ns": .., "hi_ns": ..}`,
  every field optional). Unknown keys and inconsistencies are startup errors naming the culprit.

### Configuration

One JSON config per link: a `--config*` file, or the `config` object of a link in `--links`. There
is no default config. Example:

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
  "limiter": { "mode": "Spacer", "activate": true, "rate_bps": 10000000000.0 },
  "loss_model": { "GilbertElliot": [0.01, 0.3, 1.0, 0.5, true] }
}
```

| Field             | Values                                                                                                                                                                                                                                                                                                                                     |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `buffer_capacity` | Maximum packets held at once; more are dropped.                                                                                                                                                                                                                                                                                            |
| `reordering`      | `false` = FIFO, `true` = send-time order; not with correlation or an active `Spacer`.                                                                                                                                                                                                                                                      |
| `seed`            | Optional; reproducible delay and loss decisions. Absent: system entropy.                                                                                                                                                                                                                                                                   |
| `delayer`         | `distribution` (`DeterministicDistribution`, `NormalDistribution`, `LogNormalDistribution`, `GammaDistribution` or `UniformDistribution`), `mean_ns`, `std_ns`, `activate_correlation`, `half_life_period_ns`, and the required `initialization_rate_pps`, the link's expected packet rate (about 8.1 Mpps for 100 Gb/s of 1518 B frames). |
| `limiter`         | `mode` (`Spacer` or `TokenBucketFilter`), `activate`, `rate_bps`.                                                                                                                                                                                                                                                                          |
| `loss_model`      | `{ "Random": p }` or `{ "GilbertElliot": [p, r, k, h, state_is_good] }`.                                                                                                                                                                                                                                                                   |

The limiter's limit and each link's mbuf pool are derived from `buffer_capacity`. A buffer of
1,000,000 packets needs about 1.8 GiB of hugepages per link; shrink it if that is too much. All
fields are described in [`crates/dpds-core/README.md`](crates/dpds-core/README.md#configuration).

---

## Telemetry

Build with `--features telemetry` to serve live statistics on DPDK's telemetry socket; without it
nothing is registered and the instrumentation compiles to nothing. `<dir>` is the link's index
(0 or 1 with `--config*`, else its position in the links file).

| Command                                                            | Content                                                                                                             |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------- |
| `/dpds/stats,<dir>`                                                | Counters, drops per cause (`loss_drops`, `limiter_drops`, `buffer_full_drops`), delay statistics, first-loss result |
| `/dpds/epochs,<dir>`                                               | The same per epoch (a burst of traffic, closed after 500 ms of silence)                                             |
| `/dpds/delay_hist,<dir>[,<page>]`, `/dpds/iat_hist,<dir>[,<page>]` | Delay histogram and histogram of the gaps between scheduled send times, in pages of at most 500 bins                |
| `/dpds/ports`                                                      | Ports, ring sizes, and per link its ports, lcore, config and pool size                                              |
| `/nic/hw_first_loss,<port>`, `/nic/epochs,<port>`                  | First NIC hardware drop, per-epoch NIC counters                                                                     |

```bash
./target/release/dpds -l 0-2 --in-memory --file-prefix demo -a <PCI0> -a <PCI1> -- --config link.json &
echo '/dpds/stats,0' | dpdk-telemetry.py -f demo
```

Use the same `--file-prefix` for the client (and `XDG_RUNTIME_DIR` as non-root). The field meanings
are documented in `crates/dpds/src/counters.rs`, `stats.rs` and `first_loss.rs`.

---

## Performance tuning

For line rate, isolate DPDS's cores from the OS and tune the NIC. These settings come from preof,
the sibling project whose DPDK code DPDS reuses, not from DPDS measurements. They are runtime-only
unless noted, and applying them is up to whoever sets up the host.

- **Isolate the worker cores:** `isolcpus=<cores> nohz_full=<cores> rcu_nocbs=<cores> rcu_nocb_poll`
  at boot, pin DPDS with `-l`, keep the main lcore off them. Without `nohz_full`/`rcu_nocbs` a
  spinning worker can starve RCU and make the host unreachable.
- **PCIe MaxReadReq 4096 B** (`setpci -s <PCI> CAP_EXP+8.w=5000:7000`) and **flow control off**
  (`ethtool -A <netdev> rx off tx off`).
- **NIC firmware** (mlx5, not changeable from a VM): `MAX_ACC_OUT_READ=256` (with
  `ADVANCED_PCI_SETTINGS=1`), `CQE_COMPRESSION` aggressive.
- **Memory and interrupts:** `vm.zone_reclaim_mode=0`, `vm.swappiness=0`, interrupts off the
  isolated cores. With two ports use one port per physical card.

Don't:

- **`chrt` / `SCHED_FIFO`:** preof got an unreachable host or broken forwarding, and no gain.
- **`perf record` on a busy-polling core in a VM:** it lowered cycles and measured throughput. Use `perf stat`.
- **Stack mempool ops** (`--mbuf-pool-ops-name=stack`): preof lost millions of packets to `imissed`.

---

## Controller and web frontend

`dpds-controller` discovers the host's network interfaces, starts and stops one `dpds` child process
for all links, and serves live telemetry. It stores nothing; the configuration travels with every
start request. It embeds the React frontend, so one service on one port serves the web UI at
`http://<host>:8100/` and the REST API under `/api` (interactive documentation of every route at
`/api/docs`). For frontend development see [`frontend/README.md`](frontend/README.md).

`dpds.sh` builds and installs both as a systemd service. It checks Rust, clang/`ld.lld`, DPDK and
Node/npm first and never installs or tunes anything. The frontend is built on the host
(`npm ci`, `npm run build`), which needs the npm registry.

```bash
./dpds.sh check      # prerequisites, then the host hints
./dpds.sh install    # build dpds (telemetry) and the controller, install binaries and systemd unit
./dpds.sh start      # systemctl enable --now dpds-controller
./dpds.sh status     # also: stop, restart
./dpds.sh update     # switch to the channel's branch, pull, rebuild, reinstall
./dpds.sh uninstall  # remove binaries and unit; keeps /etc/dpds/controller.env
```

The channel is set with `DPDS_CHANNEL` or a flag: `latest` (default, branch `main`, flag `--stable`),
`nightly` (branch `nightly`, flag `--nightly`) or a release tag such as `v1.0.0`. **`main` still
holds the C version of DPDS, which has no controller and no `dpds.sh`, so use `--nightly` for now**
(`./dpds.sh --nightly update`). The header of `dpds.sh` lists its other options and variables. A
plain `cargo build -p dpds-controller` embeds whatever `frontend/dist` holds (possibly stale); set
`DPDS_FRONTEND_DIST` to embed another built directory.

**Security.** The controller runs as root and listens on all interfaces without authentication.
Anyone who reaches the port can start and stop `dpds` with their own validated configuration, but
cannot change the host. Restrict it with `DPDS_BIND=127.0.0.1:8100` before `install`, or edit
`DPDS_CONTROLLER_BIND` in `/etc/dpds/controller.env` and restart. `--allowed-host` /
`DPDS_ALLOWED_HOST` adds host names that may reach it, `--cors-origin` / `DPDS_CORS_ORIGIN` enables
CORS for the listed origins.

**Host hints.** `dpds.sh check` and the frontend's Host hints page compare the host with the tuning
above and show the commands an administrator runs as root to fix a hint. DPDS never changes the
host: neither `dpds.sh`, the controller nor the frontend writes a kernel, sysfs, NIC or firmware setting.

**Releases.** The version is in `VERSION.md`; CI tags `v<version>` and creates a GitHub release
([releases](https://github.com/uni-tue-kn/DPDS/releases)) when a merge to `main` raises it.

---

## Testing

```bash
cargo test --release                        # without telemetry
cargo test --release -p dpds --features telemetry
```

Unit tests need no DPDK runtime; the end-to-end tests (`crates/dpds/tests/e2e.rs`) run the real
binary on virtual devices without root or hugepages and need several CPUs.

---

## Cite

If you use DPDS in a publication, please cite:

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
