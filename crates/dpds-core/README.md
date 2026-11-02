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

### Delay correlation

When `activate_correlation` is enabled, the delayer smooths successive samples
with a rate adaptive exponential moving average (EMA). The smoothing weight
is derived from the configured `half_life_period_ns` and the measured packet
rate, so the *effective* correlation half-life stays constant in wall-clock time
regardless of how fast packets arrive.

## Building

Requires a stable Rust toolchain. Add it as a dependency and build with Cargo:

```toml
dpds-core = { path = "crates/dpds-core" }
```

## Using the API

The host loads a configuration, creates a buffer, then pushes and pops packets.
Everything is in `dpds_core::buffer` and the crate root:

| Item                                                                  | Purpose                                                                          |
| --------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| `Config::from_file(path) -> Result<Config, ConfigError>`              | Parse a JSON config file (see below). I/O and parse errors are distinct variants. |
| `Buffer::new(config) -> Buffer`                                       | Create a buffer for the given config.                                            |
| `Buffer::push(&mut self, pkt: *mut c_void, len_byte: u32, now_ns: f64) -> bool` | Offer a packet. Returns `false` if it was dropped or the buffer was full.  |
| `Buffer::front_time(&self) -> Option<f64>`                            | Send time (ns) of the head packet, `None` if empty.                              |
| `Buffer::pop(&mut self) -> Option<*mut c_void>`                       | Remove and return the head packet pointer, `None` if empty.                      |
| `Buffer::is_empty(&self)`, `len(&self)`, `capacity(&self)`            | Occupancy and configured capacity.                                               |

A typical host loop pushes arriving packets with the current timestamp, then
forwards a packet whenever `front_time` has passed:

```rust
use dpds_core::{buffer::Buffer, Config};

let config = Config::from_file("config.json")?;
let mut buf = Buffer::new(config);

// on packet arrival:
if !buf.push(pkt, pkt_len, now_ns) {
    // packet was dropped or buffer full -- free it
}

// in the forwarding loop:
while buf.front_time().is_some_and(|t| t <= now_ns) {
    let pkt = buf.pop().unwrap();
    // transmit pkt
}
```

> **Safety:** the buffer stores raw packet pointers and does not own or
> dereference them. The caller must keep each pushed pointer valid until it is
> popped. `Buffer` is not `Send` because of those pointers; a host that moves
> it to another thread must vouch that it owns the packets.

## Configuration

`Config::from_file` parses a JSON file into the `Config` struct. A full example lives
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
| `reordering`      | bool    | `false` = FIFO; `true` = emit in send-time order (min-heap).            |

**`delayer`**

| Field                  | Type   | Meaning                                                                                                                        |
| ---------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------ |
| `distribution`         | string | One of `DeterministicDistribution`, `NormalDistribution`, `LogNormalDistribution`, `GammaDistribution`, `UniformDistribution`. |
| `mean_ns`              | float  | Mean delay in nanoseconds.                                                                                                     |
| `std_ns`               | float  | Standard deviation of the delay in nanoseconds (ignored for `DeterministicDistribution`).                                      |
| `activate_correlation` | bool   | Enable UTEMA autocorrelation of successive delays.                                                                             |
| `half_life_period_ns`  | float  | Correlation half-life in nanoseconds (only used when correlation is active).                                                   |

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
