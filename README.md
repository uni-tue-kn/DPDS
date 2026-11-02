# DPDS

**DPDS** is a high-performance software **network-link emulator**. It sits
between two network interfaces and reproduces the three effects a real link
imposes on traffic — **delay**, **rate limiting**, and **packet loss** — on a
live packet stream at line rate.

The data plane is built on [DPDK](https://www.dpdk.org/): packets are received,
passed through the emulation pipeline, and transmitted again with zero copies
and no kernel involvement on the fast path. The emulation logic itself lives in
[`dpds-core`](crates/dpds-core/README.md), a Rust library used directly by the
`dpds` binary. Everything is Rust; this is the port of the original C
application.

## Repository layout

```
crates/dpds        The data plane binary: CLI, RX/Worker/TX steps, telemetry
crates/dpds-core   Emulation library: delay, rate limiter, loss, packet buffer
crates/dpdk        Safe wrapper around the DPDK calls the data plane needs
crates/dpdk-sys    Raw bindgen bindings to DPDK
config.json        Default emulation configuration
TELEMETRY.md       The telemetry commands and what every field means
```

## How it works

```
                  one worker lcore
        ┌──────────────────────────────────────┐
        │  ┌────────┐  ┌────────┐  ┌────────┐  │
NIC ──► │  │   RX   │─►│ Worker │─►│   TX   │  │ ──► NIC
        │  └────────┘  └───┬────┘  └────────┘  │
        └──────────────────┼───────────────────┘
                    ┌──────▼───────┐
                    │  dpds-core   │  delay · rate-limit · loss
                    │  (Buffer)    │
                    └──────────────┘
```

DPDS is a **single data-plane core** application: it needs exactly two lcores
(`-l 0-1`, i.e. the EAL main lcore plus one worker; any other lcore count is
rejected at startup). The worker runs one fused loop over three steps:

| Step       | Source                       | Responsibility                                                                                    |
| ---------- | ---------------------------- | ------------------------------------------------------------------------------------------------- |
| **RX**     | `crates/dpds/src/rx.rs`      | Poll the NICs round-robin for incoming bursts.                                                     |
| **Worker** | `crates/dpds/src/worker.rs`  | Push each packet into the `dpds-core` buffer, then drain packets whose computed send time is due.  |
| **TX**     | `crates/dpds/src/tx.rs`      | Collect ready packets per port and transmit them in bursts.                                        |

The main lcore stays free for signal handling and for polling the NICs'
hardware counters every 250 ms (`/nic/*` telemetry).

The worker delegates *when* (or *whether*) each packet is forwarded to
`dpds-core`; see its [README](crates/dpds-core/README.md) for the delay
distributions, rate-limiter modes (Spacer / Token Bucket Filter), and loss
models (Random / Gilbert–Elliott).

### Topology

- **Two ports** — packets received on one port are emulated and sent out the
  other (the worker flips the port). This is the typical inline /
  bump-in-the-wire setup.
- **One port** — single-port loopback; packets are emulated and sent back out
  the same port.

Any other port count is rejected at startup.

### Shutdown

`SIGINT`/`SIGTERM` make the worker stop receiving, drain the delay buffer
completely (packets still leave at their send time) and flush TX. Then the
main lcore stops the ports, cleans up the EAL and the process exits with
status 0.

## Requirements

- **DPDK 26.07** (found through `pkg-config libdpdk`; the build needs at least 25.11),
  and for real traffic a DPDK-capable NIC (tested on Mellanox ConnectX)
- **Rust** (stable toolchain)
- **clang and lld, with the same LLVM major version as `rustc`** (see below)
- Linux; hugepages for real NICs (not needed for the virtual devices below)

## Building

```bash
cargo build --release                        # plain: no telemetry
cargo build --release --features telemetry   # with DPDK telemetry commands
```

The binary is `target/release/dpds`. The `telemetry` feature is off by default
and costs nothing when off, see [Telemetry](#telemetry).

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

## Running

DPDS takes DPDK's EAL arguments, then `--`, then its own options:

```
dpds <EAL args...> -- [--config <path>] [--num-mbufs <N>]
```

| Option               | Meaning                                                                                                   |
| -------------------- | --------------------------------------------------------------------------------------------------------- |
| `--config <path>`    | Emulation config file (default `config.json` in the working directory).                                    |
| `--num-mbufs <N>`    | Packet buffers per port (default 1048575); the pool holds `N` times the port count. Lower it for tests.     |

An unknown option, a missing value, a bad config file, an unsupported lcore or
port count, or a port whose link is down makes `dpds` exit with a non-zero
status and a message on stderr.

### Real NICs

```bash
# 2 lcores (main + one worker), two NICs inline:
sudo ./target/release/dpds -l 0-1 -a 0000:17:00.0 -a 0000:17:00.1 -- \
    --config config.json
```

- `-l 0-1` — exactly two lcores: the main lcore and the data-plane worker.
- `-a <PCI>` — NIC PCI address; pass one for loopback, two for inline.

### Without hardware (virtual devices)

No root, hugepages or NIC needed, e.g. to try the setup or in CI:

```bash
# One port that receives nothing and discards everything:
./target/release/dpds --no-huge -m 256 --no-pci --in-memory -l 0-1 \
    --vdev=net_null0 -- --config config.json --num-mbufs 8192

# Replay a pcap through the emulator, capture the result (1 port):
./target/release/dpds --no-huge -m 256 --no-pci --in-memory -l 0-1 \
    --vdev=net_pcap0,rx_pcap=in.pcap,tx_pcap=out.pcap -- --num-mbufs 8192
```

Stop with `Ctrl-C`.

## Configuration

The emulation behaviour is controlled by a JSON file (`--config`). The full
schema — delay distributions, limiter modes, and loss models — is documented in
[`crates/dpds-core/README.md`](crates/dpds-core/README.md#configuration), with a
complete example in
[`crates/dpds-core/config.example.json`](crates/dpds-core/config.example.json).
The `config.json` in this repository is a working default (10 ms normally
distributed delay, no rate limit, no loss).

## Telemetry

DPDS prints a few lines once at startup (ports, descriptor ring sizes, the
worker lcore) and then nothing: live statistics are served over DPDK's telemetry
socket, which is the machine-readable interface (a controller can poll it).
Build with `--features telemetry` to register the commands:

| command | what |
| --- | --- |
| `/dpds/stats` | Packet counters since start (`rx_pkts`, `emulator_rejects`, `pops`, `tx_sent`, `tx_drops`, ...), gauges, first-loss result. |
| `/dpds/epochs` | The same per epoch (a burst of traffic, closed after 500 ms of silence). |
| `/dpds/ports` | Device names and descriptor ring sizes (applied and requested). |
| `/nic/hw_first_loss,<port>` | First NIC hardware drop (`imissed`, `rx_nombuf`, ...) seen by the main lcore. |
| `/nic/epochs,<port>` | Per-epoch NIC `rx_packets` / `imissed` / `rx_port_discards`. |

Query with the client that ships with DPDK (`--file-prefix` as given to DPDS,
and the same `XDG_RUNTIME_DIR` when running as non-root):

```bash
./target/release/dpds -l 0-1 --file-prefix demo -a 0000:17:00.0 -- --config config.json &
echo '/dpds/stats' | dpdk-telemetry.py -f demo
```

The socket path is limited to 108 bytes; a longer `XDG_RUNTIME_DIR` /
`--file-prefix` makes DPDS fail at startup. Without the feature nothing is
registered and the instrumentation compiles to nothing.
Every field, the epoch semantics, the first-loss reasons and the socket details
are in [TELEMETRY.md](TELEMETRY.md).

## Testing

```bash
cargo test --release                        # without telemetry
cargo test --release -p dpds --features telemetry
```

Unit tests need no DPDK runtime. The end-to-end tests (`crates/dpds/tests/e2e.rs`)
start the real binary on `net_null` and `net_pcap` virtual devices without root
or hugepages and need at least 4 CPUs for the full set (they fail, not skip,
on a smaller host). With the feature they also check the exact packet counts
through the telemetry socket.

## Performance tuning

For line-rate operation, isolate the emulator's cores from the OS and tune the
NIC. These steps are host-specific and re-applied per boot.

### CPU isolation

Isolate the cores the emulator uses from the OS scheduler with the kernel boot
parameter, e.g. for the worker core 1:

```
isolcpus=1
```

### NIC tuning (one-time, per host)

On Mellanox ConnectX NICs, increasing the PCIe Maximum Read Request Size to
1024 B reduces the round-trips needed to fetch TX packet data:

```bash
# Read the current value (requires root; returns 4 hex digits ABCD)
sudo setpci -s <PCI_ADDRESS> 68.w

# Set to 1024 B: replace the first digit with 3, keep the rest (BCD)
sudo setpci -s <PCI_ADDRESS> 68.w=3BCD
```

Example — address `0000:17:00.0` with original value `2934`:

```bash
sudo setpci -s 0000:17:00.0 68.w=3934
```

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
