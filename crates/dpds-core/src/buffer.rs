use std::{
    cmp::{Ordering, Reverse},
    collections::{binary_heap::PeekMut, BinaryHeap, VecDeque},
    ffi::c_void,
};

use crate::{delayer::Delayer, dropper::Dropper, limiter::Limiter, BuildError, Config};

/// Fractional bits of the reordering heap key: send times are kept in 1/64 ns.
const TIE_FRAC_BITS: u32 = 6;
const TIE_SCALE: f64 = (1u64 << TIE_FRAC_BITS) as f64;
/// XOR-ed into the seed for the dropper so that it differs from the delayer's.
const DROPPER_SEED_XOR: u64 = 0x9E37_79B9_7F4A_7C15;

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
/// enter at the same instant keep their entry order. There is no sequence
/// number: every entry's send time is kept in fixed point with 1/64 ns
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
    /// smoothing weight is not below 1, or an unallocatable `buffer_capacity`
    /// instead of panicking. Only the initial parameters are validated: the
    /// correlated delay re-derives its distribution from the measured packet
    /// rate, which a huge finite half-life can still push out of range later.
    pub fn try_new(config: Config) -> Result<Buffer, BuildError> {
        // ~15 Mpps is current max rate of DPDS
        let initialization_rate_pps = 15_000_000.;

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

        let limiter = Limiter::new(
            config.limiter.mode,
            config.limiter.activate,
            config.limiter.rate_bps,
            config.limiter.limit_bit,
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
        })
    }

    /// Pushes a packet into the buffer, applying the delayer, limiter and
    /// dropper. Returns `false` if the packet was dropped or the buffer is full.
    ///
    /// The buffer does not own `packet_ptr` and never dereferences it; the
    /// caller must keep it valid until it is popped.
    pub fn push(
        &mut self,
        packet_ptr: *mut c_void,
        packet_length_byte: u32,
        current_time_ns: f64,
    ) -> bool {
        let time_to_send_ns = current_time_ns + self.delayer.delayer_sample_ns(current_time_ns);

        // Increase by 4 because L2 FEC is removed
        let packet_length_byte = packet_length_byte + 4;

        let limiter_result = self
            .limiter
            .limiter_calculate_offset_ns(time_to_send_ns, packet_length_byte);

        // Drop packets after they are accounted for the limiter
        if self.dropper.dropper_drop() {
            return false;
        }

        let Some(time_offset_ns) = limiter_result else {
            return false;
        };

        let send_time_ns = time_to_send_ns + time_offset_ns;

        match &mut self.inner {
            InnerBuffer::Queue(q) => {
                if q.len() >= self.capacity {
                    return false;
                }
                q.push_back(TimeToPacket {
                    time_to_send_ns: send_time_ns as u64,
                    packet_ptr,
                });
            }
            InnerBuffer::Heap(h) => {
                if h.len() >= self.capacity {
                    return false;
                }
                if current_time_ns != self.tie_instant_ns {
                    self.tie_instant_ns = current_time_ns;
                    self.tie = 0;
                }
                // `as u64` truncates (= floor for t >= 0) and saturates.
                let key = ((send_time_ns * TIE_SCALE) as u64).saturating_add(self.tie);
                self.tie = self.tie.saturating_add(1);
                h.push(Reverse(HeapEntry { key, packet_ptr }));
            }
        }
        true
    }

    /// Removes and returns the head packet pointer, or `None` if the buffer is empty.
    #[inline]
    pub fn pop(&mut self) -> Option<*mut c_void> {
        match &mut self.inner {
            InnerBuffer::Queue(q) => q.pop_front().map(|t| t.packet_ptr),
            InnerBuffer::Heap(h) => h.pop().map(|t| t.0.packet_ptr),
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
                    Some(PeekMut::pop(head).0.packet_ptr)
                } else {
                    None
                }
            }
        }
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
