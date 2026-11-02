# dpds-core

`dpds-core` is the core of **DPDS**, a software **network-link emulator**. It
models the three effects a real network link imposes on packets — **delay**,
**rate limiting**, and **packet loss** — and exposes them through a small **Rust
API**. It is used by the packet-processing data plane (the `dpds` crate, which
runs on [DPDK](https://www.dpdk.org/)); the data plane owns the packets and
calls into this library to decide *when* (or *whether*) each packet should be
forwarded.

The library never copies or interprets packet payloads. It only stores an opaque
packet pointer together with a computed send time, so it stays agnostic to the
data plane's packet representation.

## Pipeline

Each packet pushed into a `Buffer` passes through three stages:

| Stage       | Source           | Responsibility                                                                                                                                       |
| ----------- | ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Delayer** | `src/delayer.rs` | Samples a forwarding delay from a configurable distribution, optionally autocorrelated over time.                                                    |
| **Limiter** | `src/limiter.rs` | Enforces a rate limit, either as a **Spacer** (spreads packets out in time) or a **Token Bucket Filter** (drops packets that exceed a burst budget). |
| **Dropper** | `src/dropper.rs` | Decides whether a packet is lost, using a **Random** (Bernoulli) or **Gilbert–Elliott** (bursty) loss model.                                         |

The `Buffer` (`src/buffer.rs`) stores admitted packets ordered by their send
time. With `reordering = false` it is a FIFO `VecDeque`; with `reordering = true`
it is a min-heap keyed on send time, allowing packets whose sampled delays
reorder them to be emitted in send-time order.

### Ordering rule for equal send times

In reordering mode, packets that enter at the same instant keep their entry
order. There is no sequence number; instead every packet gets a tiny time offset
so that it sorts the way it entered. The heap key is the send time in fixed
point with 1/64 ns resolution:

```text
key = floor(t * 64) + tie
```

where `t` is the send time in ns (now + delay + limiter offset, before
truncation) and `tie` counts the packets accepted since the last push whose
`now_ns` differed (0 for the first packet of an instant, 1 for the next one with
the same `now_ns`, ...). Each packet entering at the same instant therefore
sorts 1/64 ns after the previous one. Rejected pushes do not advance `tie`, and
the key saturates instead of overflowing. `front_time()` and `pop_due()` use
`key >> 6`, i.e. whole nanoseconds, so the offsets are invisible to the host.
Packets entering at *different* instants whose keys happen to be exactly equal
may leave in either order. FIFO mode is unaffected: it keeps push order and truncates the send time
to whole ns.

Caveat across instants: a burst of `n` packets pushed at one instant shifts its
last keys by up to `(n - 1)/64` ns, so a packet pushed at a later instant that is
less than that apart can sort between them. With the 32-packet burst of `dpds`
that is at most about 0.5 ns; a library user pushing large batches at one
instant sees correspondingly more.

### Delay correlation

When `activate_correlation` is enabled, the delayer smooths successive samples
with a rate adaptive exponential moving average (EMA). The smoothing weight
is derived from the configured `half_life_period_ns` and the measured packet
rate, so the *effective* correlation half-life stays constant in wall-clock time
regardless of how fast packets arrive. The rate is re-estimated every 1 ms. The
smoothed delay starts at `mean_ns`, and the delay spread uses the weight of the
15 Mpps initialisation rate from the first packet on.

`try_new` validates the initial parameters: `half_life_period_ns` must be
finite and not negative, and with correlation active it must not be so long
that the weight at the initialisation rate reaches 1. A huge finite half-life
combined with a measured rate far above the initialisation rate can still make
a later rate update fail, which panics in `delayer_sample_ns`.

## Building and features

Requires a recent stable Rust toolchain (`rust-version = "1.99"`, the version
the crate is developed and tested with). The crate is not on crates.io; add it
as a git dependency (Cargo finds the package inside the workspace by name):

```toml
dpds-core = { git = "https://github.com/uni-tue-kn/DPDS", branch = "nightly" }
```

or, inside this workspace, `dpds-core = { path = "../dpds-core" }`.

| Feature            | Default | Enables                                                                                      |
| ------------------ | ------- | -------------------------------------------------------------------------------------------- |
| `json`             | yes     | `Config::from_file` and `ConfigError` (implies `serde`, pulls in `serde_json`).              |
| `serde`            | via `json` | `Serialize`/`Deserialize` for `Config`, `DelayerConfig`, `LimiterConfig`, `LossModel`, ... |

Without any feature (`--no-default-features`) the crate depends only on `rand`
and `rand_distr`; construct `Config` in code. With
`--no-default-features --features serde` you get the derives but no file
loading. `crates/dpds` uses the default features.

## Using the API

The host loads (or builds) a configuration, creates a buffer, then pushes and
pops packets. Everything is in `dpds_core::buffer` and the crate root:

| Item                                                                  | Purpose                                                                          |
| --------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| `Config::from_file(path) -> Result<Config, ConfigError>` (`json`)     | Parse a JSON config file (see below). I/O and parse errors are distinct variants. |
| `Buffer::new(config) -> Buffer`                                       | Create a buffer for the given config; panics on invalid parameters.              |
| `Buffer::try_new(config) -> Result<Buffer, BuildError>`               | Same, but invalid delay distribution parameters, a negative/NaN/infinite (or, with correlation, absurdly long) `half_life_period_ns` or an unallocatable `buffer_capacity` are an error naming the parameter. |
| `Buffer::push(&mut self, pkt: *mut c_void, len_byte: u32, now_ns: f64) -> bool` | Offer a packet. Returns `false` if it was dropped or the buffer was full.  |
| `Buffer::pop_due(&mut self, now_ns: f64) -> Option<*mut c_void>`      | Pop the head packet iff its send time (as `front_time` reports it) is `<= now_ns`. |
| `Buffer::peek_nth(&self, n: usize) -> Option<*mut c_void>`            | The packet that would be popped `n`-th from now, without popping (e.g. to prefetch). FIFO mode: the `n`-th queued entry. Reordering mode: only `n == 0` (the head) is answered, `n > 0` is `None`. |
| `Buffer::front_time(&self) -> Option<f64>`                            | Send time (ns) of the head packet, `None` if empty.                              |
| `Buffer::pop(&mut self) -> Option<*mut c_void>`                       | Remove and return the head packet pointer, `None` if empty.                      |
| `Buffer::is_empty(&self)`, `len(&self)`, `capacity(&self)`            | Occupancy and configured capacity.                                               |
| `Delayer::try_new(..)`, `Delayer::try_new_seeded(.., seed)`, `Dropper::new(loss_model)`, `Dropper::with_seed(loss_model, seed)` | The individual stages, usable on their own. |

`BuildError` implements `std::error::Error`; its message names the offending
parameter (`mean_ns`, `std_ns`, `half_life_period_ns`, `buffer_capacity`).
`BuildError` gained the `HalfLifePeriod` variant (API change for code that
matches it exhaustively).

A typical host loop reads the clock once per iteration, pushes the arriving
packets with that timestamp, then forwards everything that is due:

```rust
use dpds_core::{buffer::Buffer, Config};

let config = Config::from_file("config.json")?;
let mut buf = Buffer::try_new(config)?;

// per iteration: one clock read
let now_ns = clock_ns();

// on packet arrival:
if !buf.push(pkt, pkt_len, now_ns) {
    // packet was dropped or buffer full -- free it
}

// forward everything that is due (bounded by a burst budget):
for _ in 0..32 {
    let Some(pkt) = buf.pop_due(now_ns) else { break };
    // transmit pkt
}
```

> **Safety:** the buffer stores raw packet pointers and does not own or
> dereference them. The caller must keep each pushed pointer valid until it is
> popped. `Buffer` is not `Send` because of those pointers; a host that moves
> it to another thread must vouch that it owns the packets.

## Seeding

**API change:** `Config.seed` is a new public field and `Config` is not
`#[non_exhaustive]`, so every `Config { .. }` struct literal needs `seed: None`
(JSON files need no change).

`Config.seed` is an optional `u64` (`"seed": 42` in JSON; absent or `null` means
`None`). With `None` the delay and loss random number generators are seeded from
the OS entropy source, as before. With `Some(s)` both are seeded
deterministically and differently from each other (`SmallRng::seed_from_u64(s)`
for the delayer and `seed_from_u64(s ^ 0x9E37_79B9_7F4A_7C15)` for the dropper),
so the same seed and the same sequence of `push` calls give identical results
and send times. This is meant for tests and reproducible experiments. Seeded
streams are reproducible only for the same `rand` version and the same target
word size (`SmallRng` is not portable across them).

## Benchmark

`cargo bench -p dpds-core` runs a small hot-path microbenchmark
(`benches/hot_path.rs`: 32-packet bursts, push then `pop_due`, per delay, limiter,
loss and ordering variant) and prints the median ns per packet. Plain
`cargo test --all-targets` only runs a tiny smoke pass of it. The file documents
how to profile it with callgrind.

## Configuration

`Config::from_file` (feature `json`) parses a JSON file into the `Config` struct. A full example lives
in [`config.example.json`](config.example.json):

```json
{
  "buffer_capacity": 100000,
  "reordering": false,
  "delayer": {
    "distribution": "LogNormalDistribution",
    "mean_ns": 5000000.0,
    "std_ns": 500000.0,
    "activate_correlation": true,
    "half_life_period_ns": 50000000.0
  },
  "limiter": {
    "mode": "TokenBucketFilter",
    "activate": true,
    "rate_bps": 1000000000.0,
    "limit_bit": 2000000.0
  },
  "loss_model": {
    "GilbertElliot": [0.01, 0.3, 1.0, 0.5, true]
  }
}
```

### Fields

**Top level**

| Field             | Type    | Meaning                                                                 |
| ----------------- | ------- | ----------------------------------------------------------------------- |
| `buffer_capacity` | integer | Maximum number of packets held at once; pushes beyond this are dropped. |
| `reordering`      | bool    | `false` = FIFO; `true` = emit in send-time order (min-heap), equal send times keep entry order (see above). |
| `seed`            | integer or `null` | Optional. Seeds the delay and loss RNGs deterministically; absent = entropy (see Seeding). |

**`delayer`**

| Field                  | Type   | Meaning                                                                                                                        |
| ---------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------ |
| `distribution`         | string | One of `DeterministicDistribution`, `NormalDistribution`, `LogNormalDistribution`, `GammaDistribution`, `UniformDistribution`. |
| `mean_ns`              | float  | Mean delay in nanoseconds.                                                                                                     |
| `std_ns`               | float  | Standard deviation of the delay in nanoseconds (ignored for `DeterministicDistribution`).                                      |
| `activate_correlation` | bool   | Enable UTEMA autocorrelation of successive delays.                                                                             |
| `half_life_period_ns`  | float  | Correlation half-life in nanoseconds (only used when correlation is active). Must be finite and not negative (0 is valid), whether or not correlation is active. With correlation, a half-life so long that its smoothing weight is not below 1 is rejected. |

**`limiter`**

| Field       | Type   | Meaning                                                                         |
| ----------- | ------ | ------------------------------------------------------------------------------- |
| `mode`      | string | `Spacer` or `TokenBucketFilter`.                                                |
| `activate`  | bool   | Enable rate limiting. When `false` the limiter is a no-op.                      |
| `rate_bps`  | float  | Target rate in bits per second.                                                 |
| `limit_bit` | float  | Spacer: max in-flight backlog in bits. TBF: bucket size (burst budget) in bits. |

**`loss_model`** — a tagged union, one of:

- `{ "Random": p }` — each packet is dropped independently with probability `p`.
- `{ "GilbertElliot": [p, r, k, h, state_is_good] }` — bursty loss with a Good/Bad
  state machine:
  - `p` — Good → Bad transition probability
  - `r` — Bad → Good transition probability
  - `k` — probability of correct transmission while in the Good state
  - `h` — probability of correct transmission while in the Bad state
  - `state_is_good` — initial state flag (always reset to `true`/Good on startup)

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
