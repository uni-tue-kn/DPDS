# Running `dpds` directly, building and development

This document covers everything beside the controller: building the `dpds` binary, running it
directly with its command line, the links file and configuration reference, DPDK telemetry,
performance tuning and testing. To use DPDS through the controller (web frontend or REST API),
see the [README](../README.md).

## Requirements

- **DPDK 26.07 or newer** (`pkg-config libdpdk`); for real traffic a DPDK-capable NIC (tested on
  Mellanox ConnectX) and hugepages
- **Rust** (stable) and **clang and lld with the same LLVM major version as `rustc`**
- **Node 26 or newer and npm** for the web frontend (controller only); Linux

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
  `--config1` set the link of direction 0 (port 0 -> port 1, or port 0 -> port 0 with one port) and
  of direction 1 (port 1 -> port 0). They allow one or two ports.
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
# A config with a small buffer: its derived pool of 4096 + 4096 + 32768 + 768 + 64
# = 41,792 mbufs (about 163 MiB at one 4 KiB page per mbuf) fits into -m 256.
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

### In a virtual network (`net_af_packet`)

The controller's virtual network mode (`dpds.sh install --network virtual`, see the README) runs
`dpds` on kernel interfaces such as the veth ends of containerlab nodes. By hand it is:

```bash
./target/release/dpds -l 0-2 --no-huge -m 80 --no-pci --in-memory --file-prefix dpds \
    --vdev net_af_packet0,iface=eth1 --vdev net_af_packet1,iface=eth2 -- --links links.json
```

The controller maps the interfaces once, in `launch_plan`: the distinct ports of the links in link
order become `net_af_packet0,iface=eth1`, `net_af_packet1,iface=eth2`, and so on, passed as
`--vdev`. The `--links` file and the planned directions (`rx`, `tx` of `GET /api/emulation`) carry
the DPDK names `net_af_packet<i>`, which are also what the `dir<d>_rx_name` telemetry reports; the
run config, the history and `GET /api/stats` keep the interface names. The lcores are the lowest
online core as main and the next `links` online cores as workers.

`-m` is the memory of the mbuf pools. Without hugepages every mbuf takes a whole 4 KiB page (a
2432 B mbuf does not pack into one), so

```
m_mib = ceil(sum over links of (buffer_capacity + 4096 + 32768 + 768 + 64) * 4096 / 2^20) + 64
```

where the sum is the pool of each link and 64 MiB covers EAL, the rings and headroom. A start is
refused (400) when `m_mib` MiB are more than `MemAvailable`.

### The links file (`--links`)

```json
{ "links": [
  { "ingress": "<PCI0>", "egress": "<PCI1>", "config": { ... } },
  { "ingress": "<PCI1>", "egress": "<PCI2>", "config": { ... } },
  { "ingress": "<PCI2>", "egress": "<PCI0>", "config": { ... } }
] }
```

This ring over three NICs runs with `-l 0-3 -a <PCI0> -a <PCI1> -a <PCI2> -- --links ring.json`.

- Required per link: `ingress` and `egress` (DPDK device names: PCI address or vdev name) and
  `config` (below). A link has no name: it is identified by its ports, `link <ingress> -> <egress>`
  in messages. A port is the ingress of at most one link and the egress of at most one;
  `ingress == egress` loops back.
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
| `reordering`      | `false` = FIFO, `true` = send-time order; not with correlation; with an active `Spacer`: reorder heap, then Spacer FIFO (shared capacity, double allocation). |
| `seed`            | Optional; reproducible delay and loss decisions. Absent: system entropy.                                                                                                                                                                                                                                                                   |
| `delayer`         | `distribution` (`DeterministicDistribution`, `NormalDistribution`, `LogNormalDistribution`, `GammaDistribution` or `UniformDistribution`), `mean_ns`, `std_ns`, `activate_correlation`, `half_life_period_ns`, and the required `initialization_rate_pps`, the link's expected packet rate (about 8.1 Mpps for 100 Gb/s of 1518 B frames). |
| `limiter`         | `mode` (`Spacer` or `TokenBucketFilter`), `activate`, `rate_bps`.                                                                                                                                                                                                                                                                          |
| `loss_model`      | `{ "Random": p }` or `{ "GilbertElliot": [p, r, k, h, state_is_good] }`.                                                                                                                                                                                                                                                                   |

The limiter's limit and each link's mbuf pool are derived from `buffer_capacity`. A buffer of
1,000,000 packets needs about 2.3 GiB of hugepages per link; shrink it if that is too much. All
fields are described in [`crates/dpds-core/README.md`](../crates/dpds-core/README.md#configuration).

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

**Throughput ceiling: one core per link.** A link (one direction) is served by one worker lcore
with one RX queue and one TX queue; there is no RSS and no multi-queue. Its packet rate is
therefore bounded by what a single core forwards. 100 Gb/s of 1518 B frames is 8.1 Mpps and fits
one core; 100 Gb/s of 64 B frames is 148.8 Mpps and does not, whatever the tuning. The delay
buffer, the limiter and the loss model of a link are single-threaded state, so more cores per link
would mean splitting that state per flow or feeding one emulation core from several RX cores.

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

## Controller and frontend builds

`dpds.sh` builds the frontend on the host (`npm ci`, `npm run build`). A plain
`cargo build -p dpds-controller` embeds whatever `frontend/dist` holds (possibly stale); set
`DPDS_FRONTEND_DIST` to embed another built directory. For frontend development see
[`frontend/README.md`](../frontend/README.md).

---

## Testing

```bash
cargo test --release                        # without telemetry
cargo test --release -p dpds --features telemetry
```

Unit tests need no DPDK runtime; the end-to-end tests (`crates/dpds/tests/e2e.rs`) run the real
binary on virtual devices without root or hugepages and need several CPUs.

The controller's end-to-end tests (`crates/dpds-controller/tests/integration/e2e.rs`) are
`#[ignore]`d. They run the real `dpds`, built first with `--features telemetry`, one at a time, and
the physical ones need hugepages (`sudo sysctl -w vm.nr_hugepages=512`):

```bash
cargo build --release -p dpds --features telemetry
cargo test --release -p dpds-controller --test integration e2e -- --ignored --test-threads=1
```

The two `virtual_network_*` tests run the controller in virtual mode over veth pairs and need root,
so build the test binary as your user and run it with the environment kept:

```bash
cargo test --release -p dpds-controller --test integration --no-run   # prints the binary
sudo env "PATH=$PATH" "LD_LIBRARY_PATH=$LD_LIBRARY_PATH" target/release/deps/integration-<hash> e2e::virtual_network --ignored --test-threads=1
```

The lab has the veth ends `vdpds0` and `vdpds1` in the root namespace, their peers in the network
namespaces `vdpds-a` (10.77.0.1/24) and `vdpds-b` (10.77.0.2/24). Two links `vdpds0` -> `vdpds1`
and back, each with a 20 ms deterministic delay, carry `ip netns exec vdpds-a ping 10.77.0.2`, whose
round trip must be at least 40 ms. A second test starts one link with `buffer_capacity` 2^18 to pin
the `-m` formula. It needs the capabilities `CAP_NET_ADMIN` and `CAP_NET_RAW` (veth and
`AF_PACKET` sockets), `CAP_SYS_ADMIN` for the network namespaces (`ip netns`), a DPDK built with
`net/af_packet` and no hugepages. Without permission for `ip link add` the tests print SKIP; where
only `ip netns` is refused, the ping test skips and the memory test runs. CI runs both as root in
the backend job (its own step after the physical tests), on a DPDK built with `net/af_packet`
(`.github/scripts/install-dpdk.sh`).

With the environment variable `CI` set (GitHub Actions sets it), every test that would skip for
lack of something in its environment fails instead: the virtual network tests, the `dpds` memlock
tests (CI lifts the memlock limit with `prlimit`) and the root cases of
`packaging/tests/dpds_sh.test.sh`. A green CI run therefore means they all ran.

