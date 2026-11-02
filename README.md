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
crates/dpds        The data plane binary: CLI, RX/Worker/TX stages, stats
crates/dpds-core   Emulation library: delay, rate limiter, loss, packet buffer
crates/dpdk        Safe wrapper around the DPDK calls the data plane needs
crates/dpdk-sys    Raw bindgen bindings to DPDK
config.json        Default emulation configuration
```

## How it works

```
        ┌────────┐   rx_ring   ┌────────┐   tx_ring   ┌──────────┐
NIC ──► │   RX   │ ──────────► │ Worker │ ──────────► │    TX    │ ──► NIC
        └────────┘             └───┬────┘             └──────────┘
                                   │
                            ┌──────▼───────┐
                            │  dpds-core   │  delay · rate-limit · loss
                            │  (Buffer)    │
                            └──────────────┘
```

The pipeline has three stages, each able to run on its own isolated CPU core:

| Stage      | Source                       | Responsibility                                                                                    |
| ---------- | ---------------------------- | ------------------------------------------------------------------------------------------------- |
| **RX**     | `crates/dpds/src/rx.rs`      | Poll the NICs round-robin for incoming bursts and hand them to the worker.                         |
| **Worker** | `crates/dpds/src/worker.rs`  | Push each packet into the `dpds-core` buffer, then drain packets whose computed send time is due.  |
| **TX**     | `crates/dpds/src/tx.rs`      | Collect ready packets per port and transmit them in bursts.                                        |

The worker delegates *when* (or *whether*) each packet is forwarded to
`dpds-core`; see its [README](crates/dpds-core/README.md) for the delay
distributions, rate-limiter modes (Spacer / Token Bucket Filter), and loss
models (Random / Gilbert–Elliott).

### Core layouts

DPDS adapts to the number of lcores it is given (1 main/monitor core + 1–3
worker cores). Fewer cores fuse stages onto a single thread; more cores isolate
each stage for maximum throughput:

| lcores | Layout                                          |
| ------ | ----------------------------------------------- |
| 2      | RX + Worker + TX combined on one core           |
| 3      | RX on one core; Worker + TX combined on another |
| 4      | RX, Worker, and TX each on a dedicated core     |

Any other lcore count is rejected at startup.

### Topology

- **Two ports** — packets received on one port are emulated and sent out the
  other (the worker flips the port). This is the typical inline /
  bump-in-the-wire setup.
- **One port** — single-port loopback; packets are emulated and sent back out
  the same port.

Any other port count is rejected at startup.

### Shutdown

`SIGINT`/`SIGTERM` stop RX first. Every later stage keeps running until its
input is empty (the worker also until the delay buffer is drained), so packets
still in flight are transmitted rather than dropped. Then the final statistics
are printed, the CSV is written, the ports are stopped and the process exits
with status 0.

## Requirements

- **DPDK 26.07** (found through `pkg-config libdpdk`; the build needs at least 25.11),
  and for real traffic a DPDK-capable NIC (tested on Mellanox ConnectX)
- **Rust** (stable toolchain)
- **clang and lld, with the same LLVM major version as `rustc`** (see below)
- Linux; hugepages for real NICs (not needed for the virtual devices below)

## Building

```bash
cargo build --release
```

The binary is `target/release/dpds`.

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
dpds <EAL args...> -- [--config <path>] [-o <stats.csv>] [--num-mbufs <N>]
```

| Option               | Meaning                                                                                                   |
| -------------------- | --------------------------------------------------------------------------------------------------------- |
| `--config <path>`    | Emulation config file (default `config.json` in the working directory).                                    |
| `-o <stats.csv>`     | Write a final CSV stats summary on shutdown.                                                               |
| `--num-mbufs <N>`    | Packet buffers per port (default 1048575); the pool holds `N` times the port count. Lower it for tests.     |

An unknown option, a missing value, a bad config file, an unsupported lcore or
port count, or a port whose link is down makes `dpds` exit with a non-zero
status and a message on stderr.

### Real NICs

```bash
# 4 lcores (RX, Worker, TX + monitor), two NICs inline, CSV summary on exit:
sudo ./target/release/dpds -l 0-3 -a 0000:17:00.0 -a 0000:17:00.1 -- \
    --config config.json -o stats.csv
```

- `-l 0-3` — lcores to use (here cores 0–3, the 4-lcore layout).
- `-a <PCI>` — NIC PCI address; pass one for loopback, two for inline.

### Without hardware (virtual devices)

No root, hugepages or NIC needed, e.g. to try the setup or in CI:

```bash
# One port that receives nothing and discards everything, 2 lcores:
./target/release/dpds --no-huge -m 256 --no-pci --in-memory -l 0-1 \
    --vdev=net_null0 -- --config config.json --num-mbufs 8192

# Replay a pcap through the emulator, capture the result (1 port, 3 lcores):
./target/release/dpds --no-huge -m 256 --no-pci --in-memory -l 0-2 \
    --vdev=net_pcap0,rx_pcap=in.pcap,tx_pcap=out.pcap -- --num-mbufs 8192 -o stats.csv
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

## Statistics

Once per second DPDS redraws an in-place dashboard on stdout:

```
--- Emulator Stats (1s interval) ---
RX:     500 pkt/s | Total: 1500 | Ring drops: 3
  RX burst/s: [1]:5 [2-3]:10 [4-7]:20 [8-15]:30 [16]:40 | avg: 4.8
Worker: recv 1400 | sent 1300 | emulator drops: 70 | TX ring drops: 30
TX:     490 pkt/s | Total: 1290 | TX drops: 10
  TX burst/s: [1]:1 [2-3]:2 [4-7]:3 [8-15]:4 [16]:5 | avg: 32.7
NIC 0:  imissed=1 ierr=2 oerr=3 rx_nombuf=4
```

- **RX / TX** — packets per second, totals, and the number of bursts of each size
  per second with the average burst size.
- **Ring drops** — packets RX had to free because the RX-to-worker ring was full.
- **emulator drops** — packets the `dpds-core` buffer refused (loss model, rate
  limiter limit, or buffer full).
- **TX ring drops / TX drops** — packets freed because the TX ring or the NIC
  could not take them.
- **NIC** — hardware counters per port (`imissed`: dropped by the NIC because RX
  was too slow; `rx_nombuf`: no free mbuf, the pool is too small).

At shutdown it prints `=== Final Stats ===` and
`=== Cumulative Burst Histograms ===` (histograms over the whole run).

With `-o`, a CSV with a header line and one values line is written at exit:

```
rx_received,rx_ring_drops,rx_burst_1,rx_burst_2_3,rx_burst_4_7,rx_burst_8_15,rx_burst_16,
worker_received,worker_sent,worker_emulator_drops,worker_tx_ring_drops,
tx_transmitted,tx_drops,tx_burst_1,tx_burst_2_3,tx_burst_4_7,tx_burst_8_15,tx_burst_16,
nic_<port>_imissed,nic_<port>_ierrors,nic_<port>_oerrors,nic_<port>_rx_nombuf  (per port)
```

(Shown wrapped; it is a single comma-separated line.)

## Testing

```bash
cargo test --release
```

Unit tests need no DPDK runtime. The end-to-end tests (`crates/dpds/tests/e2e.rs`)
start the real binary on `net_null` and `net_pcap` virtual devices without root
or hugepages and need at least 4 CPUs for the full set.

## Performance tuning

For line-rate operation, isolate the emulator's cores from the OS and tune the
NIC. These steps are host-specific and re-applied per boot.

### CPU isolation

Isolate the cores the emulator uses from the OS scheduler with the kernel boot
parameter, e.g. for cores 1–4:

```
isolcpus=1-4
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
