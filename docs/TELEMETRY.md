# Telemetry

Everything DPDS exposes over DPDK's telemetry socket. It follows preof's
telemetry design (same counters, epochs, first-loss reasons, wire shapes), kept
per link: a link is one direction (ingress port -> egress port) and has its own
worker lcore, its own counter set and a name. The source of truth for the exact wire format
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
--> /dpds/stats,0
--> /nic/epochs,0
```

or non-interactively:

```bash
echo '/dpds/stats,0' | dpdk-telemetry.py -f <file-prefix>
```

- `-f` / `--file-prefix` must equal the `--file-prefix` EAL argument DPDS was
  started with (DPDK's default prefix is `rte`, so `-f` can be omitted then).
  `dpdk-telemetry.py -l` lists the prefixes it can see.
- A command with a parameter is written `/cmd,<param>`, e.g. `/dpds/stats,1` or `/nic/hw_first_loss,1`.
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
- Replies are limited to 16384 bytes. `/dpds/epochs,<dir>` stays far below that (at most
  32 epochs).
- DPDK's own `/ethdev/stats,<port>`, `/ethdev/xstats,<port>` and `/` (list all
  commands) are available next to these.

## Commands

| command | what |
| --- | --- |
| `/dpds/stats,<dir>` | Counters of link `<dir>`, cumulative since start, the delay statistics, plus that link's whole-run first-loss result. |
| `/dpds/epochs,<dir>` | The same counters per epoch (deltas) and gauges per epoch (maxima), plus per-epoch first loss, for link `<dir>`. |
| `/dpds/delay_hist,<dir>[,<page>]` | One page (500 bins) of the cumulative histogram of the delay (arrival until enqueued into the TX descriptors) of link `<dir>`; `,<dir>` alone is page 0. |
| `/dpds/iat_hist,<dir>[,<page>]` | Cumulative histogram of the gaps between the send times of consecutive packets of link `<dir>` (256 bins by default, one page), same fields as `/dpds/delay_hist`. |
| `/dpds/ports` | Static: device name and descriptor ring sizes of each port, the number of links, and the layout of each link. |
| `/nic/hw_first_loss,<port>` | First NIC hardware drop seen by the main lcore on port `<port>` (a DPDK port id, any port of the links). |
| `/nic/epochs,<port>` | Per-epoch NIC `rx_packets` / `imissed` / `rx_port_discards` of port `<port>`. |

`<dir>` is the index of a link, its position in the `--links` file; `/dpds/ports`
gives each link's name, ports and lcore. With `--config*` the links are `dir0`
(port 0 -> port 1, or port 0 -> port 0 with one port) and, with two ports, `dir1`
(port 1 -> port 0). `<dir>` must be the index of an existing link. A
missing, non-numeric or out-of-range `<dir>` or `<port>` returns an empty reply.
`<port>` is a DPDK port id (as in `/dpds/ports`), not a link index.

## `/dpds/stats,<dir>` and epochs

Each direction's worker lcore owns one counter set. Every counter has exactly one
writer (that worker) and is read by the telemetry thread without locks (relaxed
atomics, cache-line aligned).

| field | meaning |
| --- | --- |
| `uptime_ns` | Nanoseconds since DPDS started its telemetry (the shared zero point of every time in these replies). Computed when the reply is built. |
| `polls` | Loop iterations of this direction's worker. Compare with `rx_pkts`: many polls and few packets means the lcore is spinning idle. |
| `rx_pkts` | Packets received via `rx_burst` on this direction's RX port. |
| `rx_full_bursts` | `rx_burst` calls that returned a full `BURST_SIZE` (32) batch: the NIC had at least that much waiting, so intake was the binding constraint. |
| `dpds_rejects` | Packets the `dpds-core` buffer refused; always `loss_drops + limiter_drops + buffer_full_drops`. They are freed, never sent. |
| `loss_drops` | Rejected packets dropped by the configured loss model. |
| `limiter_drops` | Rejected packets the rate limiter refused (the Spacer's backlog would exceed the limiter's limit, or the Token Bucket Filter's budget is used up; the limit is derived from the buffer capacity, `buffer_capacity` x 1542 B x 8). |
| `buffer_full_drops` | Rejected packets that found the buffer holding `buffer_capacity` packets (checked first: loss model and limiter never saw them). |
| `pops` | Packets that left the DPDS buffer (their send time had arrived) and went on to TX. |
| `tx_calls` | `tx_burst` calls with at least one packet. A worker stages the packets it pops and sends the staging array with one call when it is full (`TX_BURST_SIZE` = 64 packets) or when its oldest packet has been due for `TX_MAX_WAIT_NS` = 10 us (TX staging, see the README), so there is at most one call per iteration and normally far fewer; `tx_pkts / tx_calls` is the mean batch size. |
| `tx_pkts` | Packets handed to `tx_burst`. |
| `tx_sent` | Packets the NIC accepted. |
| `tx_drops` | `tx_pkts - tx_sent`, derived when the reply is built (not a counter). Real loss at the NIC's TX descriptor ring. |
| `tx_short_calls` | `tx_burst` calls that accepted fewer packets than offered: the TX descriptor ring was momentarily full. The unsent tail is freed. |
| `tx_batch_max` (gauge) | Largest single `tx_burst` call (at most 64). |
| `tx_pkts_per_call` | `tx_pkts / tx_calls` with two decimals, as a string (telemetry dicts have no float type); `"0.00"` with no calls. |
| `buffer_len_max` (gauge) | The DPDS buffer's depth, sampled once per loop iteration after the pop (as in preof), so a zero-delay pass-through reads about 0. At steady state roughly `arrival_rate x delay`. Pinned at `buffer_capacity` means the buffer is the bottleneck. |
| `frame_sizes` | Array of 8 cumulative counts of the frames received (`pkt_len` of every mbuf `rx_burst` returned, before any drop) in P4TG's buckets: 0-63, 64, 65-127, 128-255, 256-511, 512-1023, 1024-1518, 1519 and larger bytes. In every link, also with `delay_stats` off. |
| `out_of_order` | Packets popped with an arrival sequence number below the highest one popped before (P4TG's notion); only a `reordering` link counts, always 0 otherwise. |
| `first_loss_reason` / `first_loss_detail` / `first_loss_relative_ns` | The first loss of the whole run (see below); `"Clean"` / `""` / `0` if none yet. |
| `delay_mean_ns`, `delay_std_ns` | Mean and (population) standard deviation of the delay of every packet the NIC accepted (arrival until it is enqueued into the TX descriptors), 2-decimal strings. See [Delay statistics](#delay-statistics). |
| `delay_min_ns`, `delay_max_ns` | Smallest and largest delay, uints (truncated ns). |
| `lateness_mean_ns`, `lateness_max_ns` | Mean (string) and maximum (uint) of the TX enqueue time minus the send time of the packets the NIC accepted. |

The six delay and lateness keys are absent for a link with
`"delay_stats": false` ([Delay statistics off](#delay-statistics-off)).

A packet that is neither delayed nor lost satisfies
`rx_pkts = dpds_rejects + pops + (in buffer)` and `pops = tx_pkts` (every popped
packet is staged and handed to `tx_burst` within about 10 us), which makes `tx_sent == N` and `dpds_rejects == 0` a good
exactness check for a run of `N` packets through one direction.

Gauges in `/dpds/stats,<dir>` are the maximum over all epochs of the run, including
the open one.

### Drop causes

**API change (spec20):** the room check moved from last to first (see below), so a
buffer-full packet is no longer accounted by the limiter or the loss model.

The buffer evaluates a pushed packet in a fixed order: first it checks for room, and
a buffer that already holds `buffer_capacity` packets rejects the packet at once
(`buffer_full_drops`) without touching the delayer, the limiter or the loss model: the
packet never takes part in the emulation, so nothing is accounted for it. Otherwise
the delay is sampled, the limiter accounts for the packet (also when it is rejected
afterwards), the loss model decides, and the limiter's verdict follows. The cause that
is reported is the first that rejects the packet, in the order **buffer full, loss
model, limiter**:

- if the buffer is full, `buffer_full_drops`;
- else, if the loss model drops the packet it counts as `loss_drops`, whatever the
  limiter would have done. A packet that both the loss model and the limiter reject
  is therefore a loss, and with a loss model active `limiter_drops` can under-count
  what the limiter alone would have rejected;
- else, if the limiter rejects it, `limiter_drops`.

The limiter's state advances for lost packets too (a lost packet still occupies its
spacing slot), but not for buffer-full packets.

### Epochs

The cumulative counters blend a whole run together. `/dpds/epochs,<dir>` segments it
into **epochs**, one per burst of traffic. Each worker detects boundaries itself
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
| `polls`, `rx_pkts`, `rx_full_bursts`, `dpds_rejects`, `pops`, `tx_calls`, `tx_pkts`, `tx_sent`, `tx_short_calls`, `loss_drops`, `limiter_drops`, `buffer_full_drops` | Arrays of per-epoch counter deltas. |
| `tx_batch_max`, `buffer_len_max` | Arrays of per-epoch gauge maxima. |

A gauge peak of an epoch dropped past capacity is not lost: it is folded into
a persistent high-water mark that `/dpds/stats,<dir>` still reflects. There is no
such survivor for counters or first-loss reasons.

Every field is an instantaneous read of a relaxed atomic, not a consistent
snapshot (same as preof): a read that lands between two related updates can,
for example, briefly show `tx_drops` > 0 (`tx_pkts` already raised, `tx_sent`
not yet) or a gauge just reset at an epoch close. Treat single values as
approximate while traffic flows; they are exact once the worker is idle.

## Delay statistics

Per direction, behind the `telemetry` feature, written by the direction's worker
only. Two quantities are tracked:

- the **delay** of every packet the NIC accepted: `tx_ns - arrival`, with `arrival`
  the iteration's single clock read at the push and `tx_ns` the extra clock read
  right before the `tx_burst` call that took the packet, i.e. the time it is
  enqueued into the TX descriptors (no NIC hardware timestamps). It includes the
  delay distribution's sample, the Spacer's queueing offset, the poll
  granularity, head-of-line blocking (FIFO mode: a packet waits for the one in
  front of it), loop latency and the TX staging wait (at most about 10 us).
  Rejected packets and packets the NIC refused are not counted. The arrival
  travels in the 8-byte `hash` field of the mbuf (unused with one RX queue and no
  RSS) while the packet is in the buffer;
- the **lateness** of the same packets: `tx_ns - send_time`, never below 0, the
  delay beyond the scheduled send time (staging wait included); its mean is over
  the same packets as the delay.

Both are clamped at 0 and recorded at send time. With the statistics on, the worker
reads the clock once more before each `tx_burst` (only the instance with the
statistics does).

`delay_std_ns` is the population standard deviation (divide by `n`). It doubles
as a check that DPDS reproduces the configured distribution. Everything is
cumulative since start (take differences between two polls for a window).

Numerics: the mean and the variance are kept as count, mean and sum of squared
deviations (Welford, merged burst by burst with Chan's update). Each burst is first
summed shifted by its own first delay, so the result stays exact when the delay
is large and its spread tiny (for `1e10 ns +- 1 us` a plain sum of squares in
`f64` would lose the whole variance). The only divisions are two per loop
iteration, none per packet. The worker stores `count`, `mean`, `M2` separately, so
a poll can see them from two different iterations; it is not a consistent
snapshot (like every other field here).

Cost: without the `telemetry` feature the code is absent and the loop is
unchanged; with `"delay_stats": false` it is the loop of the counters only (see
[Delay statistics off](#delay-statistics-off)). With it, the delay statistics and the histogram add about 1.6 to
2.2 ns per packet on top of the other telemetry counters, measured with
`cargo bench -p dpds --bench forward --features telemetry` (it drives
`forward::run` with mocks; aarch64 dev container, so relative numbers only, not
valid for an x86 host): 4.2 -> 6.4 ns/packet for a pass-through, 6.7 -> 8.4 for a
normal delay. To keep that low the loop only stores each accepted and each popped
send time (one store per packet) and folds them in tight loops after the push and
the pop loop; keeping the accumulators inside the push loop, which calls into the
buffer, cost several times as much.

### Delay statistics off

Each link can switch the delay statistics and the histogram off with its
`"delay_stats": true|false` key in the `--links` file (default `true`; see the
README). For a link with `false`:

- the worker does not record the delay, the lateness or the histogram (and does not read the clock before `tx_burst`).
  `forward::run` picks the loop without them once, before it starts, so the
  push and pop loops carry no statistics code at all (no per-packet and no
  per-burst work, same cost as a build without the `telemetry` feature);
- `/dpds/stats,<dir>` **does not contain** the six keys `delay_mean_ns`,
  `delay_std_ns`, `delay_min_ns`, `delay_max_ns`, `lateness_mean_ns`,
  `lateness_max_ns` (22 keys instead of 28; they are absent, not `null` and not
  0, which dicts cannot express anyway). Everything else is unchanged: the
  counters, the drop causes, the gauges, the first-loss result and the epochs;
- `/dpds/delay_hist,<dir>` (page 0) returns an empty reply and has no page 1;
- `/dpds/ports` reports `dir<d>_delay_stats` `0` (and `1` for the others).

A consumer checks `dir<d>_delay_stats` once, or tests for the absent keys.

## `/dpds/delay_hist,<dir>[,<page>]`

A cumulative linear histogram of the delay (as defined above: arrival until enqueued
into the TX descriptors) of every packet the NIC accepted on link `<dir>`, since start, with up to 4096 bins. One DPDK
telemetry array holds at most 512 entries, so the histogram is read in pages of at
most 500 bins: `/dpds/delay_hist,<dir>,<page>`, where `<page>` is a 0-based plain
decimal number. `/dpds/delay_hist,<dir>` alone is page 0. A histogram of `num_bins`
bins has `ceil(num_bins / 500)` pages (4096 bins: eight pages of 500 and one of 96).
A page that does not exist (`<page>` >= the number of pages), a `<page>` that is not
a plain decimal number and a bad `<dir>` give an empty reply (`{}`), never a short or
empty histogram. Every page has exactly these fields:

| field | meaning |
| --- | --- |
| `lo_ns` | Lower edge (inclusive) of bin 0, a decimal string (telemetry has no float). |
| `bin_ns` | Width of each bin, a decimal string. |
| `num_bins` | Number of bins of the whole histogram (uint), the same on every page. |
| `bins` | The page's counts: bins `first_bin` to `first_bin + len - 1`, at most 500 of them; bin `i` counts delays in `[lo_ns + i * bin_ns, lo_ns + (i + 1) * bin_ns)`. |
| `underflow` | Delays below `lo_ns`, cumulative; on every page. |
| `overflow` | Delays at or above `lo_ns + num_bins * bin_ns`, cumulative; on every page. |
| `page` | The 0-based index of this page (uint). |
| `first_bin` | The index of the page's first bin, `500 * page` (uint). |

A reader checks `page` and `first_bin` (replies are keyed without the `,<param>`
suffix, so a client matches them to requests by send order), that `bins` has
`min(500, num_bins - first_bin)` entries, and that the pages together hold
`num_bins` bins. The pages are read one after another, not as one atomic
snapshot: packets that arrive between two reads show up in the pages read later,
and `underflow` / `overflow` differ slightly between pages. The controller takes
them from page 0.

Every accepted packet is in exactly one of the bins, `underflow` or `overflow`.
The bin index is one subtraction and one multiplication by the reciprocal of
`bin_ns` computed at startup (no division per packet).

**Range.** The range is fixed at startup, from the link's config unless the
`--links` file overrides it with the link's `histogram` (`bins`, `lo_ns`, `hi_ns`, each
optional). The automatic rule, from `delayer.mean_ns`, `delayer.std_ns` (for any
distribution) and, for an active Spacer, its queueing allowance
`extra = buffer_capacity * 12336 / rate_bps * 1e9` ns (a packet can wait up to a full limit's
worth of bits behind the others, and the delay includes that wait) and the TX staging
wait `TX_MAX_WAIT_NS` = 10 us, twice (the wait, plus as much again for loop latency: a
timed-out batch's oldest packet is already 10 us late, so one wait alone would put it
exactly on the exclusive upper edge):

```
lo = max(0, mean - 6 * std)
hi = mean + 6 * std + extra + 2 * 10000 ns
if hi - lo < 1000 ns:      mid = (lo + hi) / 2;  lo = max(0, mid - 500);  hi = lo + 1000
bin_ns = (hi - lo) / 256
```

So a deterministic delay gets the window `[mean, mean + 20 us)` (bin width 78.125 ns), and a
Normal delay `mean +- 6 sigma` plus the 20 us. The 1 us minimum width below can no longer
trigger (the window is always at least 20 us wide). A Token Bucket Filter adds no delay and no
allowance. With explicit values: a missing `bins` is 256, a missing `lo_ns` /
`hi_ns` is the automatic one, `bin_ns = (hi - lo) / bins`, and explicit values are
used as given (the 1 us minimum applies to the automatic range only). Startup
fails, naming the link, if `bins` is not in 1..=4096, `lo_ns` or `hi_ns` is
negative, or the effective `lo_ns` is not below the effective `hi_ns`, also when
only one of them is given and it lies beyond the automatic other one.

Keep the histogram out of frequent polls: a full page holds 500 counts, about
5 KB with small counts and up to about 10.5 KB for 500 bins whose counts have
reached 20 digits (500 x 21 characters with the comma), below the 16384-byte reply limit
of DPDK's telemetry socket; a histogram of 4096 bins takes nine such replies.

**Switched off.** A link with `"delay_stats": false` has no histogram:
`/dpds/delay_hist,<dir>` (page 0) returns an empty reply, and there is no page 1,
like for a bad `<dir>` (see [Delay statistics off](#delay-statistics-off)).

## `/dpds/iat_hist,<dir>[,<page>]`

The inter-arrival time (IAT) histogram of link `<dir>`: for every packet popped after the first of the run,
`send_time - previous send_time` in ns (the send times of `dpds`' own schedule, the same ones
the pop path collects; the previous one is carried across bursts; a negative gap, possible
with reordering, counts as 0). Linear bins over `[0, 4 * 1e9 / initialization_rate_pps)`, 256
of them (the defaults; the link's `iat_histogram` in the `--links` file, `bins`, `lo_ns`, `hi_ns`, each
optional, overrides any of them, with the validation of `histogram`), plus `underflow` (never) and `overflow`. Fields, paging, bad-param and delay-statistics-off behaviour are those of
`/dpds/delay_hist`; there is only page 0.

## First-loss reasons

`first_loss_reason` is the first loss ever recorded (cumulative) or per epoch.
`first_loss_detail` says where; `first_loss_relative_ns` is the time since start.

| reason | detail | meaning |
| --- | --- | --- |
| `DpdsReject` | `from=dpds,cause=<cause>` | `Buffer::push_outcome` rejected a packet; `<cause>` is `loss` (loss model), `limiter` or `buffer_full`, with the [precedence](#drop-causes) buffer full, loss, limiter. |
| `DescriptorFull` | `from=dpds,to=nic` | A partial `tx_burst`: the NIC's TX descriptor ring was full. |
| `IMissed` | `from=dpds,count=<n>` | NIC hardware: `imissed` rose, i.e. the NIC's RX ring had no room and dropped packets before software saw them. |
| `RX_NoMbuf` | `from=dpds,count=<n>` | NIC hardware: `rx_nombuf` rose, no free mbuf in the pool (pool exhausted, which the derived pool size rules out unless mbufs leak). |
| `IErrors` | `from=dpds,count=<n>` | NIC hardware: `ierrors` rose. |
| `OErrors` | `from=dpds,count=<n>` | NIC hardware: `oerrors` rose. |

The four hardware reasons are spelled like the DPDK counters they come
from. The latch is first writer wins: later losses never replace it. The
cumulative one therefore usually reflects the start of the run, the per-epoch
ones answer "which traffic burst broke first".

## `/dpds/ports`

Static, read once at startup (also printed once to stdout at startup). Port keys:
`num_ports` (the distinct ports of the links, see the size limit below), then per port index `i`:
`port<i>_name` (device name, e.g.
a PCI address or `net_pcap0`), `port<i>_rx_ring` and `port<i>_tx_ring` (descriptor
ring sizes the driver **applied**), `port<i>_rx_ring_requested` and
`port<i>_tx_ring_requested` (what DPDS asked for, 4096; the driver may clamp it
or round it down, so compare the two). Port indices are DPDK's port ids.

Link keys: `num_directions` and `num_links` (both the number of links, the file
order; the key is kept twice), then per link `d` (its index, see
[Commands](#commands)):

| key | meaning |
| --- | --- |
| `dir<d>_rx_port` | DPDK port id the direction receives on (the link's ingress). |
| `dir<d>_tx_port` | Port id it transmits on (its egress; the same one for a loopback link). |
| `dir<d>_lcore` | EAL lcore id of the direction's worker (the link's `lcore`, or the i-th worker lcore). |
| `dir<d>_config` | Where the link's config comes from (string): its config file, or the links file. |
| `dir<d>_pool_size` | Mbufs in the link's pool, derived from `buffer_capacity` and the applied RX/TX descriptor counts (plus the egress port's RX ring if that port is no link's ingress). |
| `dir<d>_raw_free` | `1` if the worker frees with `rte_mbuf_raw_free_bulk` (both ports report `MBUF_FAST_FREE`), `0` if with `rte_pktmbuf_free_bulk`. |
| `dir<d>_link` | Name of the link (string; `dir<d>` itself with `--config*`). |
| `dir<d>_rx_name` | DPDK device name of the receiving (ingress) port (string), the entry of the links file. |
| `dir<d>_tx_name` | DPDK device name of the transmitting (egress) port (string). |
| `dir<d>_delay_stats` | `1` if the link records [delay statistics](#delay-statistics-off) and the histogram, `0` if its link switched them off. |

**Examples.** A ring of three NICs A, B, C in this order (links `a-to-b`,
`b-to-c`, `c-to-a`, port ids 0, 1, 2) reports `num_ports` 3, `num_links` 3 and
`dir0_link` `a-to-b` (`dir0_rx_name` A, `dir0_tx_name` B, `dir0_rx_port` 0,
`dir0_tx_port` 1), `dir1_link` `b-to-c` (1 -> 2), `dir2_link` `c-to-a`
(2 -> 0): 3 + 15 + 30 = 48 keys. The classic bidirectional pair is two links over
the same two ports, `dir0` 0 -> 1 and `dir1` 1 -> 0 (3 + 10 + 20 = 33 keys); a
loopback on one port is one link with `dir0_rx_port` = `dir0_tx_port`.

**Size limit.** DPDK's telemetry dict holds at most 256 entries
(`RTE_TEL_MAX_DICT_ENTRIES`), and this reply has `3 + 5 * ports + 10 *
links` of them (`ports` = distinct ports; the 3 are `num_ports`, `num_directions`,
`num_links`). A ring (one link per port) costs 15 keys per port, so a ring of 16
ports (243 keys) fits and 17 would need 258; 12 one-way links over 24 ports fit
(243 keys), 13 over 26 ports need 263; 16 loopback links fit, 17 do not. DPDS
checks this when it reads the links file and exits with an error naming the
maximum (256), before anything is probed.

## `/nic/hw_first_loss,<port>`

Not tied to the worker. The **main lcore** polls every port's hardware counters
(`eth_stats_get`) every 250 ms and records the first rise in `imissed`,
`rx_nombuf`, `ierrors` or `oerrors` (fixed priority in that order if several rose
in one poll). Fields: `hw_first_loss_reason`, `hw_first_loss_detail`,
`hw_first_loss_relative_ns`, same format as above (`"Clean"` / `""` / `0` if
nothing yet). The main lcore writes only its own state, never the worker's, which
is why this is separate from `/dpds/stats,<dir>`. A poll can lag the event by up to
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
