use std::{cmp::Ordering, collections::VecDeque, ffi::c_void};

use crate::{
    delayer::Delayer,
    derived_limit_bit,
    dropper::Dropper,
    limiter::{Limiter, LimiterMode},
    wheel::{Entry, Wheel},
    BuildError, Config,
};

/// Fractional bits of the reorder queue key: send times are kept in 1/64 ns.
const TIE_FRAC_BITS: u32 = 6;
const TIE_SCALE: f64 = (1u64 << TIE_FRAC_BITS) as f64;
/// XOR-ed into the seed for the dropper so that it differs from the delayer's.
const DROPPER_SEED_XOR: u64 = 0x9E37_79B9_7F4A_7C15;

/// What [`Buffer::push_outcome`] did with a packet.
///
/// The reported cause is the first that rejects the packet: the capacity
/// (`BufferFull`, checked first, nothing else is touched), else the loss model
/// (`Lost`), else the limiter (`LimiterDrop`). The limiter accounts the packet
/// even when the loss model drops it afterwards, so with a loss model active
/// `LimiterDrop` can under-count the packets the limiter alone would have
/// rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// The packet is queued and leaves at `send_time_ns` (whole ns, exactly
    /// what [`Buffer::front_time`] reports for its entry; in reordering mode
    /// this includes the 1/64 ns tie offsets).
    Accepted { send_time_ns: u64 },
    /// The loss model dropped the packet.
    Lost,
    /// The limiter rejected the packet (and the loss model did not drop it).
    LimiterDrop,
    /// The buffer already holds `buffer_capacity` packets. Checked first: the
    /// delayer, limiter and dropper are left untouched.
    BufferFull,
}

enum InnerBuffer {
    Queue(VecDeque<TimeToPacket>),
    Reorder(Wheel<()>),
    /// Reordering with an active Spacer: see [`Buffer`].
    ReorderThenQueue(ReorderThenQueue),
}

#[derive(Debug, Copy, Clone)]
pub struct TimeToPacket {
    pub time_to_send_ns: u64,
    pub packet_ptr: *mut c_void,
}

impl PartialEq for TimeToPacket {
    fn eq(&self, other: &Self) -> bool {
        self.time_to_send_ns == other.time_to_send_ns
    }
}

impl Eq for TimeToPacket {}

impl PartialOrd for TimeToPacket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimeToPacket {
    fn cmp(&self, other: &Self) -> Ordering {
        self.time_to_send_ns.cmp(&other.time_to_send_ns)
    }
}

/// Tie offset and arrival sequence state of the reordering pushes.
struct Tie {
    /// `current_time_ns` of the last accepted push (NaN: none yet).
    instant_ns: f64,
    /// Number of packets accepted at `instant_ns` so far.
    count: u64,
    /// Arrival sequence number of the next accepted push.
    next_seq: u64,
}

impl Tie {
    /// The queue entry of a packet accepted now with the given send time.
    fn entry<X>(
        &mut self,
        current_time_ns: f64,
        send_time_ns: f64,
        packet_ptr: *mut c_void,
        extra: X,
    ) -> Entry<X> {
        if current_time_ns != self.instant_ns {
            self.instant_ns = current_time_ns;
            self.count = 0;
        }
        // `as u64` truncates (= floor for t >= 0) and saturates.
        let key = ((send_time_ns * TIE_SCALE) as u64).saturating_add(self.count);
        self.count = self.count.saturating_add(1);
        let seq = self.next_seq;
        self.next_seq += 1;
        Entry {
            key,
            seq,
            packet_ptr,
            extra,
        }
    }
}

/// The send time of a queue entry in whole ns, as reported by
/// [`Buffer::front_time`].
#[inline]
fn time_ns<X>(entry: &Entry<X>) -> f64 {
    (entry.key >> TIE_FRAC_BITS) as f64
}

/// A packet in the FIFO of [`ReorderThenQueue`].
#[derive(Copy, Clone)]
struct SpacedPacket {
    time_to_send_ns: u64,
    seq: u64,
    packet_ptr: *mut c_void,
}

/// Reordering with an active Spacer in two stages: the reorder queue, then a
/// FIFO behind the Spacer. A reorder entry carries the packet length (with the
/// FEC bytes); the length is not part of the order.
struct ReorderThenQueue {
    reorder: Wheel<u32>,
    queue: VecDeque<SpacedPacket>,
    /// Stage 2: unlimited, so it only delays and never drops.
    spacer: Limiter,
}

impl ReorderThenQueue {
    fn held(&self) -> usize {
        self.reorder.len() + self.queue.len()
    }

    /// Moves the reorder queue head through the Spacer, at its queue time, into
    /// the FIFO.
    fn move_head(&mut self) {
        let Some(entry) = self.reorder.pop() else {
            return;
        };
        let time_ns = time_ns(&entry);
        let offset_ns = self
            .spacer
            .limiter_calculate_offset_ns(time_ns, entry.extra)
            .unwrap_or(0.); // no limit: never `None`
        self.queue.push_back(SpacedPacket {
            time_to_send_ns: (time_ns + offset_ns) as u64,
            seq: entry.seq,
            packet_ptr: entry.packet_ptr,
        });
    }

    /// Next packet in departure order, due or not.
    fn pop_next(&mut self) -> Option<SpacedPacket> {
        if self.queue.is_empty() {
            self.move_head();
        }
        self.queue.pop_front()
    }

    /// Moves every reorder entry due at `now_ns`, then pops the FIFO head if due.
    fn pop_ready(&mut self, now_ns: f64) -> Option<SpacedPacket> {
        while self.reorder.peek().is_some_and(|e| time_ns(e) <= now_ns) {
            self.move_head();
        }
        if self.queue.front()?.time_to_send_ns as f64 <= now_ns {
            self.queue.pop_front()
        } else {
            None
        }
    }

    /// Send time of the next packet: the FIFO head's, else the reorder queue
    /// head's after the Spacer, which is looked at without changing it.
    fn head_time(&self) -> Option<f64> {
        if let Some(head) = self.queue.front() {
            return Some(head.time_to_send_ns as f64);
        }
        let entry = self.reorder.peek()?;
        let time_ns = time_ns(entry);
        let offset_ns = self.spacer.limiter_peek_offset_ns(time_ns, entry.extra)?;
        Some((time_ns + offset_ns) as u64 as f64)
    }
}

/// The packet buffer of one link: applies delay, rate limit and loss to every
/// pushed packet and releases them at their send time.
///
/// # Ordering
///
/// With `reordering: false` the buffer is a FIFO: packets leave in push order,
/// whatever their send times. The send time is truncated to whole ns.
///
/// With `reordering: true` packets leave in send-time order, and packets that
/// enter at the same instant keep their entry order. The order does not use
/// the arrival sequence number (kept only for [`Buffer::out_of_order`]): every
/// entry's send time is kept in fixed point with 1/64 ns resolution
/// (`key = floor(t * 64) + tie`, `t` the send time in ns) and `tie` counts the
/// packets accepted since the last push with a different `current_time_ns`, so
/// each packet entering at the same instant sorts 1/64 ns after the previous
/// one. Rejected pushes do not advance `tie`. The key saturates at `u64::MAX`.
/// [`Buffer::front_time`] reports `key >> 6` (whole ns). Packets that enter at
/// *different* instants and end up with exactly equal keys may leave in either
/// order.
///
/// A burst of `n` packets at one instant shifts its last keys by up to
/// `(n - 1)/64` ns, so a packet pushed at a later instant that is less than
/// that apart can sort between them (at most about 0.5 ns for the 32-packet
/// burst of `dpds`).
///
/// With `reordering: true` and an active Spacer limiter the buffer has two
/// stages and reserves `buffer_capacity` entries for each, but the capacity is
/// shared (`len()` counts both). A push applies only the delayer and the loss
/// model and enters the reorder queue above;
/// [`PushOutcome::Accepted::send_time_ns`] is its queue time. An entry due at its
/// queue time `t` goes through the Spacer *at `t`* (not when it is popped) into a
/// FIFO with send time `t + offset`. The Spacer has no limit: an accepted packet
/// is never dropped later. [`Buffer::peek_nth`] looks into the FIFO only.
pub struct Buffer {
    inner: InnerBuffer,
    capacity: usize,
    delayer: Delayer,
    limiter: Limiter,
    dropper: Dropper,
    tie: Tie,
    /// Highest arrival sequence number popped so far plus one (0: none yet).
    popped_seq_end: u64,
    /// Departures with a lower arrival sequence number than an earlier one.
    out_of_order: u64,
}

impl Buffer {
    /// Creates an empty buffer from `config`.
    ///
    /// # Panics
    ///
    /// If `config` is invalid (see [`Buffer::try_new`]).
    pub fn new(config: Config) -> Buffer {
        Buffer::try_new(config).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Like [`Buffer::new`], but returns a [`BuildError`] instead of panicking:
    /// invalid delay distribution parameters, a bad `half_life_period_ns`, an
    /// `initialization_rate_pps` that is not finite or below 1, an
    /// unallocatable `buffer_capacity`, or `reordering` combined with
    /// correlation. Only the initial
    /// parameters are validated: the correlated delay re-derives its
    /// distribution from the measured packet rate, which a huge finite
    /// half-life can still push out of range later.
    pub fn try_new(config: Config) -> Result<Buffer, BuildError> {
        if config.reordering && config.delayer.activate_correlation {
            return Err(BuildError::ReorderingWithCorrelation);
        }
        let two_stage = config.reordering
            && config.limiter.activate
            && matches!(config.limiter.mode, LimiterMode::Spacer);
        let initialization_rate_pps = config.delayer.initialization_rate_pps;

        let capacity_error =
            |reason: std::collections::TryReserveError| BuildError::BufferCapacity {
                capacity: config.buffer_capacity,
                reason: reason.to_string(),
            };
        let inner = if two_stage {
            let reorder = Wheel::try_new(config.buffer_capacity).map_err(capacity_error)?;
            let mut queue = VecDeque::new();
            queue
                .try_reserve_exact(config.buffer_capacity)
                .map_err(capacity_error)?;
            InnerBuffer::ReorderThenQueue(ReorderThenQueue {
                reorder,
                queue,
                spacer: Limiter::new(LimiterMode::Spacer, true, config.limiter.rate_bps, f64::MAX),
            })
        } else if config.reordering {
            InnerBuffer::Reorder(Wheel::try_new(config.buffer_capacity).map_err(capacity_error)?)
        } else {
            let mut queue = VecDeque::new();
            queue
                .try_reserve_exact(config.buffer_capacity)
                .map_err(capacity_error)?;
            InnerBuffer::Queue(queue)
        };

        let delayer = match config.seed {
            None => Delayer::try_new(
                config.delayer.distribution,
                config.delayer.mean_ns,
                config.delayer.std_ns,
                config.delayer.activate_correlation,
                config.delayer.half_life_period_ns,
                initialization_rate_pps,
            ),
            Some(seed) => Delayer::try_new_seeded(
                config.delayer.distribution,
                config.delayer.mean_ns,
                config.delayer.std_ns,
                config.delayer.activate_correlation,
                config.delayer.half_life_period_ns,
                initialization_rate_pps,
                seed,
            ),
        }?;

        let limit_bit = derived_limit_bit(config.buffer_capacity);
        let limiter = Limiter::new(
            config.limiter.mode,
            // The two-stage mode applies its Spacer when leaving the reorder queue instead.
            config.limiter.activate && !two_stage,
            config.limiter.rate_bps,
            limit_bit,
        );

        let dropper = match config.seed {
            None => Dropper::new(config.loss_model),
            Some(seed) => Dropper::with_seed(config.loss_model, seed ^ DROPPER_SEED_XOR),
        };

        Ok(Buffer {
            inner,
            capacity: config.buffer_capacity,
            delayer,
            limiter,
            dropper,
            tie: Tie {
                instant_ns: f64::NAN,
                count: 0,
                next_seq: 0,
            },
            popped_seq_end: 0,
            out_of_order: 0,
        })
    }

    /// Pushes a packet, applying the delayer, limiter and dropper. Returns
    /// `false` if the packet was dropped or the buffer is full.
    ///
    /// The buffer does not own `packet_ptr` and never dereferences it; the
    /// caller must keep it valid until it is popped.
    pub fn push(
        &mut self,
        packet_ptr: *mut c_void,
        packet_length_byte: u32,
        current_time_ns: f64,
    ) -> bool {
        matches!(
            self.push_outcome(packet_ptr, packet_length_byte, current_time_ns),
            PushOutcome::Accepted { .. }
        )
    }

    /// Like [`Buffer::push`], but tells why a packet was rejected and, for an
    /// accepted one, when it leaves. See [`PushOutcome`] for the precedence of
    /// the causes.
    pub fn push_outcome(
        &mut self,
        packet_ptr: *mut c_void,
        packet_length_byte: u32,
        current_time_ns: f64,
    ) -> PushOutcome {
        // Room first: a packet that cannot be enqueued must not drift the
        // limiter or consume random draws.
        if self.len() >= self.capacity {
            return PushOutcome::BufferFull;
        }

        let time_to_send_ns = current_time_ns + self.delayer.delayer_sample_ns(current_time_ns);

        // Add the 4 L2 FEC bytes that were removed from the length.
        let packet_length_byte = packet_length_byte.saturating_add(4);

        let limiter_result = self
            .limiter
            .limiter_calculate_offset_ns(time_to_send_ns, packet_length_byte);

        // The limiter has accounted the packet; the loss model decides after.
        if self.dropper.dropper_drop() {
            return PushOutcome::Lost;
        }

        let Some(time_offset_ns) = limiter_result else {
            return PushOutcome::LimiterDrop;
        };

        let send_time_ns = time_to_send_ns + time_offset_ns;

        match &mut self.inner {
            InnerBuffer::Queue(q) => {
                let time_to_send_ns = send_time_ns as u64;
                q.push_back(TimeToPacket {
                    time_to_send_ns,
                    packet_ptr,
                });
                PushOutcome::Accepted {
                    send_time_ns: time_to_send_ns,
                }
            }
            InnerBuffer::Reorder(w) => {
                let entry = self
                    .tie
                    .entry(current_time_ns, send_time_ns, packet_ptr, ());
                w.push(entry);
                PushOutcome::Accepted {
                    send_time_ns: entry.key >> TIE_FRAC_BITS,
                }
            }
            InnerBuffer::ReorderThenQueue(s) => {
                let entry = self.tie.entry(
                    current_time_ns,
                    send_time_ns,
                    packet_ptr,
                    packet_length_byte,
                );
                s.reorder.push(entry);
                PushOutcome::Accepted {
                    send_time_ns: entry.key >> TIE_FRAC_BITS,
                }
            }
        }
    }

    /// Removes and returns the head packet pointer.
    #[inline]
    pub fn pop(&mut self) -> Option<*mut c_void> {
        match &mut self.inner {
            InnerBuffer::Queue(q) => q.pop_front().map(|t| t.packet_ptr),
            InnerBuffer::Reorder(w) => {
                let entry = w.pop()?;
                self.count_departure(entry.seq);
                Some(entry.packet_ptr)
            }
            InnerBuffer::ReorderThenQueue(s) => {
                let p = s.pop_next()?;
                self.count_departure(p.seq);
                Some(p.packet_ptr)
            }
        }
    }

    /// Removes and returns the head packet pointer if its send time (as
    /// [`Buffer::front_time`] reports it) is `<= now_ns`; otherwise, or if the
    /// buffer is empty, returns `None` and leaves the buffer unchanged.
    #[inline]
    pub fn pop_due(&mut self, now_ns: f64) -> Option<*mut c_void> {
        match &mut self.inner {
            InnerBuffer::Queue(q) => {
                if q.front()?.time_to_send_ns as f64 <= now_ns {
                    q.pop_front().map(|t| t.packet_ptr)
                } else {
                    None
                }
            }
            InnerBuffer::Reorder(w) => {
                if time_ns(w.peek()?) <= now_ns {
                    let entry = w.pop()?;
                    self.count_departure(entry.seq);
                    Some(entry.packet_ptr)
                } else {
                    None
                }
            }
            InnerBuffer::ReorderThenQueue(s) => {
                let p = s.pop_ready(now_ns)?;
                self.count_departure(p.seq);
                Some(p.packet_ptr)
            }
        }
    }

    /// Like [`Buffer::pop_due`], but also returns the send time (whole ns, as
    /// [`Buffer::front_time`] reported it before the pop) of the popped packet.
    #[inline]
    pub fn pop_due_with_time(&mut self, now_ns: f64) -> Option<(*mut c_void, u64)> {
        match &mut self.inner {
            InnerBuffer::Queue(q) => {
                let send_time_ns = q.front()?.time_to_send_ns;
                if send_time_ns as f64 <= now_ns {
                    q.pop_front().map(|t| (t.packet_ptr, send_time_ns))
                } else {
                    None
                }
            }
            InnerBuffer::Reorder(w) => {
                if time_ns(w.peek()?) <= now_ns {
                    let entry = w.pop()?;
                    self.count_departure(entry.seq);
                    Some((entry.packet_ptr, entry.key >> TIE_FRAC_BITS))
                } else {
                    None
                }
            }
            InnerBuffer::ReorderThenQueue(s) => {
                let p = s.pop_ready(now_ns)?;
                self.count_departure(p.seq);
                Some((p.packet_ptr, p.time_to_send_ns))
            }
        }
    }

    /// Counts a departing reordering packet: out of order when its arrival
    /// sequence number is below the highest one popped before it.
    #[inline]
    fn count_departure(&mut self, seq: u64) {
        if seq < self.popped_seq_end {
            self.out_of_order += 1;
        } else {
            self.popped_seq_end = seq + 1;
        }
    }

    /// Cumulative number of out-of-order departures: popped packets whose
    /// arrival sequence number (0, 1, 2, ... in push order, accepted pushes
    /// only) is lower than the highest one popped before them. Always 0 without
    /// `reordering` (a FIFO never reorders, and counts nothing per packet).
    #[inline]
    pub fn out_of_order(&self) -> u64 {
        self.out_of_order
    }

    /// Returns the send time (ns) of the head packet.
    #[inline]
    pub fn front_time(&self) -> Option<f64> {
        match &self.inner {
            InnerBuffer::Queue(q) => Some(q.front()?.time_to_send_ns as f64),
            InnerBuffer::Reorder(w) => Some(time_ns(w.peek()?)),
            InnerBuffer::ReorderThenQueue(s) => s.head_time(),
        }
    }

    /// Returns the packet that would be popped `n`-th from now (`n == 0`: the
    /// head) without removing it, or `None` if there is none.
    ///
    /// In FIFO mode this is the `n`-th queued entry. In reordering mode only
    /// `n == 0` (the head) is answered; for `n > 0` this returns `None`,
    /// because only the FIFO order is cheap to look ahead in. With reordering
    /// and a Spacer it is the `n`-th entry of the Spacer FIFO (entries still in
    /// the reorder queue are not seen). Send times are ignored: the packet need
    /// not be due.
    #[inline]
    pub fn peek_nth(&self, n: usize) -> Option<*mut c_void> {
        match &self.inner {
            InnerBuffer::Queue(q) => q.get(n).map(|t| t.packet_ptr),
            InnerBuffer::Reorder(w) => {
                if n == 0 {
                    w.peek().map(|e| e.packet_ptr)
                } else {
                    None
                }
            }
            InnerBuffer::ReorderThenQueue(s) => s.queue.get(n).map(|p| p.packet_ptr),
        }
    }

    /// Returns `true` if the buffer holds no packets.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of packets currently in the buffer.
    #[inline]
    pub fn len(&self) -> usize {
        match &self.inner {
            InnerBuffer::Queue(q) => q.len(),
            InnerBuffer::Reorder(w) => w.len(),
            InnerBuffer::ReorderThenQueue(s) => s.held(),
        }
    }

    /// Returns the maximum number of packets the buffer can hold.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}
