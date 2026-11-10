//! The reorder queue of [`crate::buffer::Buffer`]: a min-queue on a `u64` key
//! built as a timing wheel, much cheaper than a binary heap when most entries
//! live far from the head (the heap pays a cache-missing sift per operation,
//! the wheel appends to and drains small slots sequentially).
//!
//! Layout, with `slot(key) = key >> SLOT_SHIFT` (a slot is [`SLOT_SHIFT`] bits
//! of key wide, 4.096 us for keys in 1/64 ns):
//!
//! - `current`: the entries of slot `cursor` and of every earlier slot, sorted
//!   by key, consumed from `head`. The head of the queue is always
//!   `current[head]`: `current` is never empty while the queue is not.
//! - `ring`: one unsorted `Vec` per slot for the slots `cursor + 1 ..
//!   cursor + ring.len()`; the slot's ring index is `slot & mask`. An
//!   occupancy bitmap finds the next non-empty slot without scanning them all.
//! - `overflow`: a binary heap for the slots beyond that horizon. It is
//!   drained into the ring whenever the cursor advances, so its entries are
//!   always `>= cursor + ring.len()`.
//!
//! The ring slots keep their capacity once used, so the steady state does not
//! allocate. Only the overflow heap is reserved up front (`capacity` entries:
//! that is what `Buffer::try_new` checks); a ring slot grows on demand like any
//! `Vec` (the queue holds at most `capacity` entries in total).

use std::{cmp::Reverse, collections::BinaryHeap, collections::TryReserveError, ffi::c_void};

/// Width of a slot in key bits: 2^18 keys = 4096 ns. Narrower slots sort less,
/// wider ones cost more ring memory for the same horizon.
const SLOT_SHIFT: u32 = 18;
/// Most ring slots (a horizon of 2^18 x 4096 ns = 1.07 s) and fewest. The ring
/// has `buffer_capacity` slots within these limits, so small buffers stay small.
const MAX_SLOTS: usize = 1 << 18;
const MIN_SLOTS: usize = 1 << 12;
/// A drained `current` that grew beyond this many entries is freed, so a burst
/// does not leave its memory behind in every ring slot it passes through.
const KEEP_CAPACITY: usize = 1 << 12;
/// A dead prefix of `current` is dropped once it is at least this long and at
/// least as long as the live part.
const COMPACT_MIN: usize = 64;

/// A queued packet: ordered by `key` only. `extra` is payload that is not part
/// of the order (`()` or the packet length).
#[derive(Copy, Clone)]
pub(crate) struct Entry<X> {
    pub key: u64,
    /// Arrival sequence number (per buffer, assigned at push).
    pub seq: u64,
    pub packet_ptr: *mut c_void,
    pub extra: X,
}

impl<X> PartialEq for Entry<X> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<X> Eq for Entry<X> {}

impl<X> PartialOrd for Entry<X> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<X> Ord for Entry<X> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}

#[inline]
fn slot_of(key: u64) -> u64 {
    key >> SLOT_SHIFT
}

pub(crate) struct Wheel<X> {
    current: Vec<Entry<X>>,
    head: usize,
    /// Absolute slot of `current`.
    cursor: u64,
    ring: Vec<Vec<Entry<X>>>,
    /// Bit `i` is set while `ring[i]` is not empty.
    occupied: Vec<u64>,
    /// Entries in `ring`.
    in_ring: usize,
    overflow: BinaryHeap<Reverse<Entry<X>>>,
}

impl<X: Copy> Wheel<X> {
    /// An empty queue for up to `capacity` entries; fails if the overflow heap
    /// (reserved for all of them) cannot be allocated.
    pub fn try_new(capacity: usize) -> Result<Self, TryReserveError> {
        let mut overflow = BinaryHeap::new();
        overflow.try_reserve_exact(capacity)?;
        let slots = capacity.clamp(MIN_SLOTS, MAX_SLOTS).next_power_of_two();
        let mut ring = Vec::new();
        ring.try_reserve_exact(slots)?;
        ring.resize_with(slots, Vec::new);
        Ok(Wheel {
            current: Vec::new(),
            head: 0,
            cursor: 0,
            ring,
            occupied: vec![0; slots / 64],
            in_ring: 0,
            overflow,
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.current.len() - self.head + self.in_ring + self.overflow.len()
    }

    /// The entry with the smallest key.
    #[inline]
    pub fn peek(&self) -> Option<&Entry<X>> {
        self.current.get(self.head)
    }

    /// Removes and returns the entry with the smallest key.
    #[inline]
    pub fn pop(&mut self) -> Option<Entry<X>> {
        let entry = *self.current.get(self.head)?;
        self.head += 1;
        if self.head == self.current.len() {
            self.current.clear();
            if self.current.capacity() > KEEP_CAPACITY {
                self.current = Vec::new();
            }
            self.head = 0;
            self.advance();
        }
        Some(entry)
    }

    #[inline]
    pub fn push(&mut self, entry: Entry<X>) {
        let slot = slot_of(entry.key);
        if self.current.len() == self.head {
            // Empty (`current` is never empty otherwise): start over at `slot`.
            self.current.clear();
            self.head = 0;
            self.cursor = slot;
            self.current.push(entry);
        } else if slot <= self.cursor {
            self.insert_current(entry);
        } else if slot - self.cursor < self.ring.len() as u64 {
            self.push_ring(slot, entry);
        } else {
            self.overflow.push(Reverse(entry));
        }
    }

    /// Inserts into the sorted live part of `current`.
    fn insert_current(&mut self, entry: Entry<X>) {
        if self.head >= COMPACT_MIN && self.head * 2 >= self.current.len() {
            self.current.drain(..self.head);
            self.head = 0;
        }
        let live = &self.current[self.head..];
        let pos = self.head + live.partition_point(|e| e.key <= entry.key);
        self.current.insert(pos, entry);
    }

    #[inline]
    fn push_ring(&mut self, slot: u64, entry: Entry<X>) {
        let index = slot as usize & (self.ring.len() - 1);
        self.ring[index].push(entry);
        self.occupied[index / 64] |= 1 << (index % 64);
        self.in_ring += 1;
    }

    /// `current` was just emptied: moves the cursor to the next slot that holds
    /// entries and makes that slot `current`.
    fn advance(&mut self) {
        let slot = if self.in_ring > 0 {
            self.next_ring_slot()
        } else if let Some(Reverse(next)) = self.overflow.peek() {
            slot_of(next.key)
        } else {
            return;
        };
        self.cursor = slot;
        let end = slot + self.ring.len() as u64;
        while let Some(Reverse(next)) = self.overflow.peek() {
            let next_slot = slot_of(next.key);
            if next_slot >= end {
                break;
            }
            let Some(Reverse(next)) = self.overflow.pop() else {
                break;
            };
            self.push_ring(next_slot, next);
        }
        let index = slot as usize & (self.ring.len() - 1);
        std::mem::swap(&mut self.current, &mut self.ring[index]);
        self.occupied[index / 64] &= !(1 << (index % 64));
        self.in_ring -= self.current.len();
        self.current.sort_unstable_by_key(|e| e.key);
    }

    /// The absolute slot of the first non-empty ring slot after the cursor.
    fn next_ring_slot(&self) -> u64 {
        let mask = self.ring.len() - 1;
        let start = (self.cursor as usize + 1) & mask;
        let index = self
            .first_set(start, self.ring.len())
            .or_else(|| self.first_set(0, start))
            .expect("in_ring > 0 implies an occupied slot");
        self.cursor + 1 + (index.wrapping_sub(start) & mask) as u64
    }

    /// Index of the first set bit of `occupied` in `from..to`.
    fn first_set(&self, from: usize, to: usize) -> Option<usize> {
        let mut word = from / 64;
        let mut bits = self.occupied[word] & (!0u64 << (from % 64));
        loop {
            if bits != 0 {
                let index = word * 64 + bits.trailing_zeros() as usize;
                return (index < to).then_some(index);
            }
            word += 1;
            if word * 64 >= to {
                return None;
            }
            bits = self.occupied[word];
        }
    }
}
