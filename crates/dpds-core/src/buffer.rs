use std::{
    cmp::{Ordering, Reverse},
    collections::{BinaryHeap, VecDeque},
    ffi::c_void,
};

use crate::{delayer::Delayer, dropper::Dropper, limiter::Limiter, Config};

enum InnerBuffer {
    Queue(VecDeque<TimeToPacket>),
    Heap(BinaryHeap<Reverse<TimeToPacket>>),
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

pub struct Buffer {
    inner: InnerBuffer,
    capacity: usize,
    delayer: Delayer,
    limiter: Limiter,
    dropper: Dropper,
}

impl Buffer {
    /// Pushes a packet into the buffer, applying the delayer, limiter and
    /// dropper. Returns `false` if the packet was dropped or the buffer is full.
    ///
    /// # Safety
    /// `self` must be a valid `Buffer` pointer returned from `buffer_new`, and
    /// `packet_ptr` must remain valid until the packet is popped.
    #[no_mangle]
    pub extern "C" fn buffer_push(
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

        let time_to_send = (time_to_send_ns + time_offset_ns) as u64;
        let time_to_packet = TimeToPacket {
            time_to_send_ns: time_to_send,
            packet_ptr,
        };

        match &mut self.inner {
            InnerBuffer::Queue(q) => {
                if q.len() >= self.capacity {
                    return false;
                }
                q.push_back(time_to_packet);
            }
            InnerBuffer::Heap(h) => {
                if h.len() >= self.capacity {
                    return false;
                }
                h.push(Reverse(time_to_packet));
            }
        }
        true
    }

    /// Removes and returns the head packet pointer.
    ///
    /// # Safety
    /// `self` must be a valid `Buffer` pointer returned from `buffer_new`, and
    /// the caller must ensure the buffer is non-empty.
    #[no_mangle]
    pub extern "C" fn buffer_pop(&mut self) -> *mut c_void {
        match &mut self.inner {
            InnerBuffer::Queue(q) => {
                q.pop_front()
                    .expect("buffer_pop called on empty buffer")
                    .packet_ptr
            }
            InnerBuffer::Heap(h) => {
                h.pop()
                    .expect("buffer_pop called on empty buffer")
                    .0
                    .packet_ptr
            }
        }
    }

    /// Returns the send time (in ns) of the head packet.
    ///
    /// # Safety
    /// `self` must be a valid `Buffer` pointer returned from `buffer_new`, and
    /// the caller must ensure the buffer is non-empty.
    #[no_mangle]
    pub extern "C" fn buffer_front_time(&self) -> f64 {
        let ns: u64 = match &self.inner {
            InnerBuffer::Queue(q) => {
                q.front()
                    .expect("buffer_front_time called on empty buffer")
                    .time_to_send_ns
            }
            InnerBuffer::Heap(h) => {
                h.peek()
                    .expect("buffer_front_time called on empty buffer")
                    .0
                    .time_to_send_ns
            }
        };
        ns as f64
    }

    /// Returns `true` if the buffer holds no packets.
    ///
    /// # Safety
    /// `self` must be a valid `Buffer` pointer returned from `buffer_new`.
    #[no_mangle]
    pub extern "C" fn buffer_is_empty(&self) -> bool {
        match &self.inner {
            InnerBuffer::Queue(q) => q.is_empty(),
            InnerBuffer::Heap(h) => h.is_empty(),
        }
    }

    /// Returns the number of packets currently in the buffer.
    ///
    /// # Safety
    /// `self` must be a valid `Buffer` pointer returned from `buffer_new`.
    #[no_mangle]
    pub extern "C" fn buffer_len(&self) -> usize {
        match &self.inner {
            InnerBuffer::Queue(q) => q.len(),
            InnerBuffer::Heap(h) => h.len(),
        }
    }

    /// Returns the maximum number of packets the buffer can hold.
    ///
    /// # Safety
    /// `self` must be a valid `Buffer` pointer returned from `buffer_new`.
    #[no_mangle]
    pub extern "C" fn buffer_capacity(&self) -> usize {
        self.capacity
    }
}

#[no_mangle]
pub extern "C" fn buffer_new(config: Config) -> *mut Buffer {
    // ~15 Mpps is current max rate of the emulator
    let initialization_rate_pps = 15_000_000.;

    let inner = if config.reordering {
        InnerBuffer::Heap(BinaryHeap::with_capacity(config.buffer_capacity))
    } else {
        InnerBuffer::Queue(VecDeque::with_capacity(config.buffer_capacity))
    };

    let delayer = Delayer::new(
        config.delayer.distribution,
        config.delayer.mean_ns,
        config.delayer.std_ns,
        config.delayer.activate_correlation,
        config.delayer.half_life_period_ns,
        initialization_rate_pps,
    );

    let limiter = Limiter::new(
        config.limiter.mode,
        config.limiter.activate,
        config.limiter.rate_bps,
        config.limiter.limit_bit,
    );

    let dropper = Dropper::new(config.loss_model);

    Box::into_raw(Box::new(Buffer {
        inner,
        capacity: config.buffer_capacity,
        delayer,
        limiter,
        dropper,
    }))
}

/// # Safety
/// `ptr` must be a pointer returned from `buffer_new` and must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn buffer_destroy(ptr: *mut Buffer) {
    if !ptr.is_null() {
        drop(Box::from_raw(ptr));
    }
}
