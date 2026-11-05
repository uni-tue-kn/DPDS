use std::{
    cmp::{Ordering, Reverse},
    collections::{binary_heap::PeekMut, BinaryHeap, VecDeque},
    ffi::c_void,
};

use crate::{
    delayer::Delayer,
    derived_limit_bit,
    dropper::Dropper,
    limiter::{Limiter, LimiterMode},
    BuildError, Config,
};

/// Fractional bits of the reordering heap key: send times are kept in 1/64 ns.
const TIE_FRAC_BITS: u32 = 6;
const TIE_SCALE: f64 = (1u64 << TIE_FRAC_BITS) as f64;
/// XOR-ed into the seed for the dropper so that it differs from the delayer's.
const DROPPER_SEED_XOR: u64 = 0x9E37_79B9_7F4A_7C15;

/// What [`Buffer::push_outcome`] did with a packet.
///
/// The cause of a rejection follows the evaluation order of the push: the
/// capacity is checked first (`BufferFull`, nothing else is touched), then the
/// limiter accounts the packet (also when the packet is rejected afterwards)
/// and the loss model decides. The reported cause is the first that rejects the
/// packet: the capacity (`BufferFull`), else the loss model (`Lost`), else the
/// limiter (`LimiterDrop`). A packet that both the loss model and the limiter reject therefore counts as
/// `Lost`, so with a loss model active `LimiterDrop` can under-count the
/// packets the limiter alone would have rejected.
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
    Heap(BinaryHeap<Reverse<HeapEntry>>),
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

/// Entry of the reordering heap: the send time in 1/64 ns plus the tie offset
/// (see [`Buffer`]), ordered by `key` only.
#[derive(Copy, Clone)]
struct HeapEntry {
    key: u64,
    /// Arrival sequence number (per buffer, assigned at push).
    seq: u64,
    packet_ptr: *mut c_void,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

impl HeapEntry {
    /// The send time in whole ns, as reported by [`Buffer::front_time`].
    #[inline]
    fn time_ns(&self) -> f64 {
        (self.key >> TIE_FRAC_BITS) as f64
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
/// entry's send time is kept in fixed point with 1/64 ns
/// resolution (`key = floor(t * 64) + tie`, `t` the send time in ns) and `tie`
/// counts the packets accepted since the last push with a different
/// `current_time_ns`, so each packet entering at the same instant sorts 1/64 ns
/// after the previous one. Rejected pushes do not advance `tie`. The key
/// saturates at `u64::MAX` instead of overflowing. [`Buffer::front_time`]
/// reports `key >> 6` (whole ns). Packets that enter at *different* instants
/// and end up with exactly equal keys may leave in either order.
///
/// The offsets also reach across instants: a burst of `n` packets at one
/// instant shifts its last keys by up to `(n - 1)/64` ns, so a packet pushed at
/// a later instant that is less than that apart can sort between them. With
/// the 32-packet burst of `dpds` that is at most about 0.5 ns.
pub struct Buffer {
    inner: InnerBuffer,
    capacity: usize,
    delayer: Delayer,
    limiter: Limiter,
    dropper: Dropper,
    /// `current_time_ns` of the last accepted reordering push (NaN: none yet).
    tie_instant_ns: f64,
    /// Number of packets accepted at `tie_instant_ns` so far.
    tie: u64,
    /// Arrival sequence number of the next accepted reordering push.
    next_seq: u64,
    /// Highest arrival sequence number popped so far plus one (0: none yet).
    popped_seq_end: u64,
    /// Departures with a lower arrival sequence number than an earlier one.
    out_of_order: u64,
}

impl Buffer {
    /// Creates an empty buffer (and its delayer, limiter and dropper) from `config`.
    ///
    /// # Panics
    ///
    /// If `config` is invalid (see [`Buffer::try_new`]).
    pub fn new(config: Config) -> Buffer {
        Buffer::try_new(config).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Like [`Buffer::new`], but returns an error for invalid delay
    /// distribution parameters, a `half_life_period_ns` that is negative, NaN
    /// or infinite (0 is valid) or, with correlation, so long that its
    /// smoothing weight is not below 1, an `initialization_rate_pps` that is
    /// not finite or below 1, or an unallocatable `buffer_capacity`
    /// instead of panicking. Also rejects `reordering` combined with
    /// correlation or with an active Spacer limiter. Only the initial parameters are validated: the
    /// correlated delay re-derives its distribution from the measured packet
    /// rate, which a huge finite half-life can still push out of range later.
    pub fn try_new(config: Config) -> Result<Buffer, BuildError> {
        if config.reordering && config.delayer.activate_correlation {
            return Err(BuildError::ReorderingWithCorrelation);
        }
        if config.reordering
            && config.limiter.activate
            && matches!(config.limiter.mode, LimiterMode::Spacer)
        {
            return Err(BuildError::ReorderingWithSpacer);
        }
        let initialization_rate_pps = config.delayer.initialization_rate_pps;

        let capacity_error =
            |reason: std::collections::TryReserveError| BuildError::BufferCapacity {
                capacity: config.buffer_capacity,
                reason: reason.to_string(),
            };
        let inner = if config.reordering {
            let mut heap = BinaryHeap::new();
            heap.try_reserve_exact(config.buffer_capacity)
                .map_err(capacity_error)?;
            InnerBuffer::Heap(heap)
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
            config.limiter.activate,
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
            tie_instant_ns: f64::NAN,
            tie: 0,
            next_seq: 0,
            popped_seq_end: 0,
            out_of_order: 0,
        })
    }

    /// Pushes a packet into the buffer, applying the delayer, limiter and
    /// dropper. Returns `false` if the packet was dropped or the buffer is full.
    ///
    /// Same as [`Buffer::push_outcome`] reduced to "accepted or not".
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
    ///
    /// **API change (addition):** new method; `push` is a wrapper around it.
    pub fn push_outcome(
        &mut self,
        packet_ptr: *mut c_void,
        packet_length_byte: u32,
        current_time_ns: f64,
    ) -> PushOutcome {
        // Room first: a packet that cannot be enqueued must not be accounted
        // anywhere (it would drift the limiter and consume random draws).
        if self.len() >= self.capacity {
            return PushOutcome::BufferFull;
        }

        let time_to_send_ns = current_time_ns + self.delayer.delayer_sample_ns(current_time_ns);

        // Increase by 4 because L2 FEC is removed
        let packet_length_byte = packet_length_byte + 4;

        let limiter_result = self
            .limiter
            .limiter_calculate_offset_ns(time_to_send_ns, packet_length_byte);

        // Drop packets after they are accounted for the limiter
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
            InnerBuffer::Heap(h) => {
                if current_time_ns != self.tie_instant_ns {
                    self.tie_instant_ns = current_time_ns;
                    self.tie = 0;
                }
                // `as u64` truncates (= floor for t >= 0) and saturates.
                let key = ((send_time_ns * TIE_SCALE) as u64).saturating_add(self.tie);
                self.tie = self.tie.saturating_add(1);
                let seq = self.next_seq;
                self.next_seq += 1;
                h.push(Reverse(HeapEntry {
                    key,
                    seq,
                    packet_ptr,
                }));
                PushOutcome::Accepted {
                    send_time_ns: key >> TIE_FRAC_BITS,
                }
            }
        }
    }

    /// Removes and returns the head packet pointer, or `None` if the buffer is empty.
    #[inline]
    pub fn pop(&mut self) -> Option<*mut c_void> {
        match &mut self.inner {
            InnerBuffer::Queue(q) => q.pop_front().map(|t| t.packet_ptr),
            InnerBuffer::Heap(h) => {
                let entry = h.pop()?.0;
                self.count_departure(entry.seq);
                Some(entry.packet_ptr)
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
            InnerBuffer::Heap(h) => {
                let head = h.peek_mut()?;
                if head.0.time_ns() <= now_ns {
                    let entry = PeekMut::pop(head).0;
                    self.count_departure(entry.seq);
                    Some(entry.packet_ptr)
                } else {
                    None
                }
            }
        }
    }

    /// Like [`Buffer::pop_due`], but also returns the send time (whole ns, as
    /// [`Buffer::front_time`] reported it before the pop) of the popped packet.
    ///
    /// **API change (addition):** new method.
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
            InnerBuffer::Heap(h) => {
                let head = h.peek_mut()?;
                if head.0.time_ns() <= now_ns {
                    let entry = PeekMut::pop(head).0;
                    self.count_departure(entry.seq);
                    Some((entry.packet_ptr, entry.key >> TIE_FRAC_BITS))
                } else {
                    None
                }
            }
        }
    }

    /// Counts a departing reordering-heap packet: out of order when its arrival
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

    /// Returns the send time (in ns) of the head packet, or `None` if the buffer is empty.
    #[inline]
    pub fn front_time(&self) -> Option<f64> {
        match &self.inner {
            InnerBuffer::Queue(q) => Some(q.front()?.time_to_send_ns as f64),
            InnerBuffer::Heap(h) => Some(h.peek()?.0.time_ns()),
        }
    }

    /// Returns the packet that would be popped `n`-th from now (`n == 0`: the
    /// head) without removing it, or `None` if there is none.
    ///
    /// In FIFO mode this is the `n`-th queued entry. In reordering mode only
    /// `n == 0` (the heap head) is answered; for `n > 0` this returns `None`,
    /// because only the FIFO order is cheap to look ahead in. Send times are
    /// ignored: the packet need not be due.
    #[inline]
    pub fn peek_nth(&self, n: usize) -> Option<*mut c_void> {
        match &self.inner {
            InnerBuffer::Queue(q) => q.get(n).map(|t| t.packet_ptr),
            InnerBuffer::Heap(h) => {
                if n == 0 {
                    h.peek().map(|t| t.0.packet_ptr)
                } else {
                    None
                }
            }
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
            InnerBuffer::Heap(h) => h.len(),
        }
    }

    /// Returns the maximum number of packets the buffer can hold.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}
