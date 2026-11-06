# dpds-core

`dpds-core` is the emulation library of **DPDS**, a software network-link
emulator. It models the three effects a real link imposes on packets (**delay**,
**rate limiting** and **packet loss**) and decides *when*, or *whether*, each
packet is forwarded. The `dpds` data plane, built on [DPDK](https://www.dpdk.org/),
owns the packets and calls this library.

The library never copies or reads packet payloads. It stores an opaque packet
pointer together with a computed send time.

## Pipeline

Each packet pushed into a `Buffer` passes through three stages:

| Stage       | Source           | Job                                                                                          |
| ----------- | ---------------- | -------------------------------------------------------------------------------------------- |
| **Delayer** | `src/delayer.rs` | Samples a delay from a distribution, optionally autocorrelated over time.                    |
| **Limiter** | `src/limiter.rs` | Rate limit: **Spacer** (spreads packets out) or **Token Bucket Filter** (drops excess).      |
| **Dropper** | `src/dropper.rs` | Loss model: **Random** (Bernoulli) or **Gilbert–Elliott** (bursty).                          |

The `Buffer` (`src/buffer.rs`) keeps the admitted packets ordered by send time.
With `reordering = false` it is a FIFO, so a packet with a late send time blocks
the ones behind it. With `reordering = true` it is a min-heap on the send time and
packets leave in send-time order (packets pushed at the same instant keep their
entry order). Reordering is a way to reproduce the configured delay more
accurately, not a model of network reordering.

A push is evaluated in this order: capacity check first (a full buffer rejects the
packet and touches nothing else), then delay, limiter accounting, loss model and
limiter verdict. A packet that both the loss model and the limiter reject counts
as lost.

## Use it

The crate is not on crates.io. Add it as a git dependency (Cargo finds the package
in the workspace by name), or by path inside this workspace:

```toml
dpds-core = { git = "https://github.com/uni-tue-kn/DPDS", branch = "nightly" }
```

| Feature | Default | Enables                                                                  |
| ------- | ------- | ------------------------------------------------------------------------ |
| `json`  | yes     | `Config::from_file` and `ConfigError` (implies `serde`, uses `serde_json`). |
| `serde` | via `json` | `Serialize` / `Deserialize` for `Config` and the types in it.         |

Without features the crate depends only on `rand` and `rand_distr`; build `Config`
in code.

## API essentials

| Item                                         | Purpose                                                                         |
| -------------------------------------------- | ------------------------------------------------------------------------------- |
| `Config::from_file(path)`                    | Parse a JSON config (`json` feature).                                           |
| `Buffer::try_new(config)`                    | Build a buffer; invalid parameters are a `BuildError` naming the parameter. `Buffer::new` panics instead. |
| `Buffer::push(pkt, len_byte, now_ns) -> bool` | Offer a packet; `false` if it was rejected. `push_outcome` says why (`Accepted { send_time_ns }`, `Lost`, `LimiterDrop`, `BufferFull`). |
| `Buffer::pop_due(now_ns)`                    | Pop the head packet if its send time is `<= now_ns`. `pop_due_with_time` also returns that time. |
| `Buffer::front_time()`, `peek_nth(n)`        | Send time of the head, or the n-th packet to pop (FIFO only for `n > 0`).        |
| `Buffer::len()`, `capacity()`, `is_empty()`  | Occupancy.                                                                      |

A host reads the clock once per iteration, pushes the arrivals, then forwards what is due:

```rust
use dpds_core::{buffer::Buffer, Config};

let mut buf = Buffer::try_new(Config::from_file("config.json")?)?;

let now_ns = clock_ns(); // one clock read per iteration

if !buf.push(pkt, pkt_len, now_ns) {
    // rejected (loss, limiter or buffer full): free the packet
}

for _ in 0..32 {
    let Some(pkt) = buf.pop_due(now_ns) else { break };
    // transmit pkt
}
```

> **Safety:** the buffer stores raw packet pointers and neither owns nor
> dereferences them. The caller keeps each pushed pointer valid until it is popped.
> `Buffer` is not `Send` because of those pointers; a host that moves it to another
> thread must vouch that it owns the packets.

`cargo bench -p dpds-core` runs a hot-path microbenchmark (`benches/hot_path.rs`).

## Configuration

A JSON file per link (full example: [`config.example.json`](config.example.json)):

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
    "initialization_rate_pps": 81000.0
  },
  "limiter": {
    "mode": "TokenBucketFilter",
    "activate": true,
    "rate_bps": 1000000000.0
  },
  "loss_model": {
    "GilbertElliot": [0.01, 0.3, 1.0, 0.5, true]
  }
}
```

**Top level**

| Field             | Meaning                                                                                  |
| ----------------- | ---------------------------------------------------------------------------------------- |
| `buffer_capacity` | Maximum number of packets held at once; further pushes are rejected. |
| `reordering`      | `false` = FIFO, `true` = send-time order. Not with `activate_correlation` and not with an active `Spacer` (use `TokenBucketFilter`); `try_new` rejects both. |
| `seed`            | Optional integer. Seeds the delay and loss generators, so the same seed and the same pushes give the same decisions (same `rand` version and word size). Absent: OS entropy. |

The limiter's limit is not configured. Both modes derive it from the capacity:
`buffer_capacity * 1542 * 8` bit (1542 B = a 1518 B frame, 4 FEC bytes and 20 B of
line overhead, so `buffer_capacity` maximum-size frames fit). It is the Spacer's
maximum backlog and the Token Bucket Filter's bucket depth (which starts full).

**`delayer`**

| Field                     | Meaning                                                                                  |
| ------------------------- | ---------------------------------------------------------------------------------------- |
| `distribution`            | `DeterministicDistribution`, `NormalDistribution`, `LogNormalDistribution`, `GammaDistribution` or `UniformDistribution`. |
| `mean_ns`, `std_ns`       | Mean and standard deviation of the delay in ns (`std_ns` is ignored for `Deterministic`). |
| `activate_correlation`    | Smooth successive delays with a rate-adaptive exponential moving average.                 |
| `half_life_period_ns`     | Correlation half-life in ns, finite and not negative.                                     |
| `initialization_rate_pps` | Required, no default: the expected packet rate of the link (finite, at least 1), used until the first 1 ms rate estimate. About 8.1 Mpps for 100 Gb/s of 1518 B frames. |

**`limiter`**

| Field      | Meaning                                              |
| ---------- | ---------------------------------------------------- |
| `mode`     | `Spacer` or `TokenBucketFilter`.                     |
| `activate` | `false` makes the limiter a no-op.                   |
| `rate_bps` | Target rate in bits per second.                      |

**`loss_model`**: `{ "Random": p }` drops each packet independently with probability
`p`. `{ "GilbertElliot": [p, r, k, h, state_is_good] }` is a two-state model: `p` is
the Good to Bad and `r` the Bad to Good transition probability, `k` and `h` the
probability of correct transmission in Good and Bad, `state_is_good` the initial state.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
