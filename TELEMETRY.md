# Telemetry

Everything DPDS exposes over DPDK's telemetry socket. It follows preof's
telemetry design (same counters, epochs, first-loss reasons, wire shapes), scoped
to DPDS's single data-plane worker. The source of truth for the exact wire format
is `crates/dpds/src/counters.rs`, `first_loss.rs` and `stats.rs`; this file
explains what each field means.

Telemetry is behind the `telemetry` Cargo feature, **off by default**:

```bash
cargo build --release --features telemetry
```

## Querying

DPDK's telemetry thread (started by `rte_eal_init`) serves JSON on a Unix
socket. Use the client that ships with DPDK:

```bash
dpdk-telemetry.py -f <file-prefix>        # interactive prompt, then e.g.:
--> /dpds/stats
--> /nic/epochs,0
```

or non-interactively:

```bash
echo '/dpds/stats' | dpdk-telemetry.py -f <file-prefix>
```

- `-f` / `--file-prefix` must equal the `--file-prefix` EAL argument DPDS was
  started with (DPDK's default prefix is `rte`, so `-f` can be omitted then).
  `dpdk-telemetry.py -l` lists the prefixes it can see.
- A command with a parameter is written `/cmd,<param>`, e.g. `/nic/hw_first_loss,1`.
  The reply is keyed by the command **without** its `,<param>`.
- The socket is `$XDG_RUNTIME_DIR/dpdk/<file-prefix>/dpdk_telemetry.v2` (as
  non-root; `/tmp/dpdk/<file-prefix>/...` if `XDG_RUNTIME_DIR` is unset; root
  uses DPDK's default `/var/run/dpdk/...`). It also exists with `--in-memory`,
  and is removed when DPDS exits. Run the client with the same
  `XDG_RUNTIME_DIR` as DPDS.
- **The socket path must fit `sockaddr_un.sun_path` (108 bytes).** A longer path
  makes `rte_eal_init` fail with `TELEMETRY: Error with socket binding, path too
  long`, so DPDS exits with status 1. Point `XDG_RUNTIME_DIR` at a short
  directory (e.g. `/tmp/dpds-x1`) or use a short `--file-prefix`.
- Replies are limited to 16384 bytes. `/dpds/epochs` stays far below that (at most
  32 epochs).
- DPDK's own `/ethdev/stats,<port>`, `/ethdev/xstats,<port>` and `/` (list all
  commands) are available next to these.

## Commands

| command | what |
| --- | --- |
| `/dpds/stats` | Worker counters, cumulative since start, plus the whole-run first-loss result. |
| `/dpds/epochs` | The same counters per epoch (deltas) and gauges per epoch (maxima), plus per-epoch first loss. |
| `/dpds/ports` | Static: device name and descriptor ring sizes of each port. |
| `/nic/hw_first_loss,<port>` | First NIC hardware drop seen by the main lcore on port `<port>` (`0` or `1`). |
| `/nic/epochs,<port>` | Per-epoch NIC `rx_packets` / `imissed` / `rx_port_discards` of port `<port>`. |

A `<port>` that is not the index of an existing port returns an empty reply.

## `/dpds/stats` and epochs

The single worker lcore owns one counter set. Every counter has exactly one
writer (the worker) and is read by the telemetry thread without locks (relaxed
atomics, cache-line aligned).

| field | meaning |
| --- | --- |
| `uptime_ns` | Nanoseconds since DPDS started its telemetry (the shared zero point of every time in these replies). Computed when the reply is built. |
| `polls` | Loop iterations of the worker. Compare with `rx_pkts`: many polls and few packets means the lcore is spinning idle. |
| `rx_pkts` | Packets received via `rx_burst`, all ports. |
| `rx_full_bursts` | `rx_burst` calls that returned a full `MAX_BURST_SIZE` (16) batch: the NIC had at least that much waiting, so intake was the binding constraint. |
| `emulator_rejects` | Packets the `dpds-core` buffer refused: the configured loss model, the rate limiter's `limit_bit`, or the buffer being full (`buffer_capacity`). Undifferentiated by design. They are freed, never sent. |
| `pops` | Packets that left the emulator buffer (their send time had arrived) and went on to TX. |
| `tx_calls` | `tx_burst` calls with at least one packet. TX collects per-port tables of up to 16 packets, so one call is one table flush; `tx_pkts / tx_calls` is the mean batch size. |
| `tx_pkts` | Packets handed to `tx_burst`. |
| `tx_sent` | Packets the NIC accepted. |
| `tx_drops` | `tx_pkts - tx_sent`, derived when the reply is built (not a counter). Real loss at the NIC's TX descriptor ring. |
| `tx_short_calls` | `tx_burst` calls that accepted fewer packets than offered: the TX descriptor ring was momentarily full. The unsent tail is freed. |
| `tx_batch_max` (gauge) | Largest single `tx_burst` call (at most 16). |
| `tx_pkts_per_call` | `tx_pkts / tx_calls` with two decimals, as a string (telemetry dicts have no float type); `"0.00"` with no calls. |
| `buffer_len_max` (gauge) | The emulator buffer's depth, sampled once per loop iteration after the pop (as in preof), so a zero-delay pass-through reads about 0. At steady state roughly `arrival_rate x delay`. Pinned at `buffer_capacity` means the buffer is the bottleneck. |
| `first_loss_reason` / `first_loss_detail` / `first_loss_relative_ns` | The first loss of the whole run (see below); `"Clean"` / `""` / `0` if none yet. |

A packet that is neither delayed nor lost satisfies
`rx_pkts = emulator_rejects + pops + (in buffer)` and `pops = tx_pkts` once TX
has flushed, which makes `tx_sent == N` and `emulator_rejects == 0` a good
exactness check for a run of `N` packets.

Gauges in `/dpds/stats` are the maximum over all epochs of the run, including
the open one.

### Epochs

The cumulative counters blend a whole run together. `/dpds/epochs` segments it
into **epochs**, one per burst of traffic. The worker detects boundaries itself
from its own arrival pattern: an iteration with no arrivals, more than 500 ms
(`EPOCH_IDLE_GAP_NS`, strictly greater) after the last one, closes the current
epoch, provided it saw at least one arrival. Idle time before the first packet
or after an epoch closes never produces empty epochs. The next epoch starts
exactly where the previous one ended.

Each epoch's counters are **deltas** (that epoch's own count) and each gauge is
that epoch's own high-water mark. The reply is columnar (DPDK telemetry arrays
can hold uint64 or strings, not objects): every array has one entry per closed
epoch, oldest first.

| field | meaning |
| --- | --- |
| `epochs_closed` | Number of closed epochs recorded (at most 32, `EPOCH_CAPACITY`). The still-open epoch is not included. |
| `epochs_dropped` | Closes beyond the 32nd. The log keeps the **oldest** 32 and drops later ones (it never overwrites). Treat counters past epoch 32 as missing, not zero. |
| `epoch_start_uptime_ns` / `epoch_end_uptime_ns` | Array. Epoch window in nanoseconds since start, the same clock as `uptime_ns` and `first_loss_relative_ns`. |
| `first_loss_reason` / `first_loss_detail` / `first_loss_relative_ns` | Arrays. The first loss **within that epoch** (`"Clean"` / `""` / `0` if none). |
| `polls`, `rx_pkts`, `rx_full_bursts`, `emulator_rejects`, `pops`, `tx_calls`, `tx_pkts`, `tx_sent`, `tx_short_calls` | Arrays of per-epoch counter deltas. |
| `tx_batch_max`, `buffer_len_max` | Arrays of per-epoch gauge maxima. |

A gauge peak of an epoch dropped past capacity is not lost: it is folded into
a persistent high-water mark that `/dpds/stats` still reflects. There is no
such survivor for counters or first-loss reasons.

Every field is an instantaneous read of a relaxed atomic, not a consistent
snapshot (same as preof): a read that lands between two related updates can,
for example, briefly show `tx_drops` > 0 (`tx_pkts` already raised, `tx_sent`
not yet) or a gauge just reset at an epoch close. Treat single values as
approximate while traffic flows; they are exact once the worker is idle.

## First-loss reasons

`first_loss_reason` is the first loss ever recorded (cumulative) or per epoch.
`first_loss_detail` says where; `first_loss_relative_ns` is the time since start.

| reason | detail | meaning |
| --- | --- | --- |
| `EmulatorReject` | `from=dpds` | `Buffer::push` returned false: loss model, limiter or buffer full (not distinguished). |
| `DescriptorFull` | `from=dpds,to=nic` | A partial `tx_burst`: the NIC's TX descriptor ring was full. |
| `IMissed` | `from=dpds,count=<n>` | NIC hardware: `imissed` rose, i.e. the NIC's RX ring had no room and dropped packets before software saw them. |
| `RX_NoMbuf` | `from=dpds,count=<n>` | NIC hardware: `rx_nombuf` rose, no free mbuf in the pool (`--num-mbufs` too small, or an mbuf leak). |
| `IErrors` | `from=dpds,count=<n>` | NIC hardware: `ierrors` rose. |
| `OErrors` | `from=dpds,count=<n>` | NIC hardware: `oerrors` rose. |

The four hardware reasons are spelled like the DPDK counters they come
from. The latch is first writer wins: later losses never replace it. The
cumulative one therefore usually reflects the start of the run, the per-epoch
ones answer "which traffic burst broke first".

## `/dpds/ports`

Static, read once at startup (also printed once to stdout at startup):
`num_ports` (1 or 2), then per port index `i`: `port<i>_name` (device name, e.g.
a PCI address or `net_pcap0`), `port<i>_rx_ring` and `port<i>_tx_ring` (descriptor
ring sizes the driver **applied**), `port<i>_rx_ring_requested` and
`port<i>_tx_ring_requested` (what DPDS asked for, 65535; the driver may clamp it
or round it down, so compare the two). Port indices are DPDK's port ids.

## `/nic/hw_first_loss,<port>`

Not tied to the worker. The **main lcore** polls every port's hardware counters
(`eth_stats_get`) every 250 ms and records the first rise in `imissed`,
`rx_nombuf`, `ierrors` or `oerrors` (fixed priority in that order if several rose
in one poll). Fields: `hw_first_loss_reason`, `hw_first_loss_detail`,
`hw_first_loss_relative_ns`, same format as above (`"Clean"` / `""` / `0` if
nothing yet). The main lcore writes only its own state, never the worker's, which
is why this is separate from `/dpds/stats`. A poll can lag the event by up to
250 ms. `/ethdev/stats,<port>` gives the raw cumulative NIC counters.

## `/nic/epochs,<port>`

The main lcore also segments each port's own arrival stream (`ipackets` between
polls) into epochs with the same rule as the worker (500 ms idle gap after at
least one arrival). Same wire shape as `/dpds/epochs`
(`epochs_closed`, `epochs_dropped`, `epoch_start_uptime_ns`/`epoch_end_uptime_ns`, and
the `first_loss_*` arrays, which are always `"Clean"` / `""` / `0` here since a NIC
port has no software loss tracker), with these counter arrays (deltas per epoch):

| field | meaning |
| --- | --- |
| `rx_packets` | The port's `ipackets` delta, the stream that defines the epoch. |
| `imissed` | The port's `imissed` delta (RX packets the NIC dropped). |
| `rx_port_discards` | Delta of the sum of every `rx_prio[0-7]_buf_discard_packets` / `rx_prio[0-7]_cong_discard_packets` xstat the port has (mlx5 port-buffer discards). The xstat ids are resolved once at startup and the sum is read **only on the poll that closes an epoch**, because frequent firmware xstats queries are suspected of amplifying NIC stalls. `0` for ports (e.g. virtual devices) without those xstats. |

Epoch times here are polled, so they are accurate to the 250 ms poll interval.

## Everything is free when you don't ask for it

Without `--features telemetry` every counter, gauge, epoch and first-loss type is
a zero-sized no-op, nothing is registered on the telemetry socket (the main lcore
still polls the NIC counters, and the `rx_port_discards` xstats are never
queried), and `/dpds/*` and `/nic/*` do not exist: querying them returns
`null`. DPDK's own commands (`/ethdev/stats`, ...) are unaffected.
