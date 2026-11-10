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
//! - `heads` and `nodes`: the slots `cursor + 1 .. cursor + heads.len()`; the
//!   slot's ring index is `slot & mask`. A ring slot is an unsorted singly
//!   linked list of nodes in the shared pool `nodes`, `heads[index]` being its
//!   first node. An occupancy bitmap finds the next non-empty slot without
//!   scanning them all. Freed nodes go on a free list, which is reused before
//!   the never-used part of the pool, so nodes freed together (one drained
//!   slot, in the order they were pushed) are reused together.
//! - `overflow`: a binary heap for the slots beyond that horizon. It is
//!   drained into the ring whenever the cursor advances, so its entries are
//!   always `>= cursor + heads.len()`.
//!
//! All memory is allocated in [`Wheel::try_new`]: `capacity` pool nodes,
//! `capacity` entries for `current`, `capacity` for `overflow`, and the ring
//! heads and bitmap. The queue holds at most `capacity` entries in total (the
//! caller checks), so none of them can outgrow its reservation and nothing is
//! allocated, reallocated or freed afterwards. That is `3 * 32` bytes per
//! entry of capacity for `Wheel<u32>` and `32 + 2 * 24` for `Wheel<()>`, plus
//! 4.125 bytes per ring slot (16.5 KiB to 1.03 MiB, whatever the capacity).

use std::{cmp::Reverse, collections::BinaryHeap, collections::TryReserveError, ffi::c_void};

/// Width of a slot in key bits: 2^18 keys = 4096 ns. Narrower slots sort less,
/// wider ones cost more ring memory for the same horizon.
const SLOT_SHIFT: u32 = 18;
/// Most ring slots (a horizon of 2^18 x 4096 ns = 1.07 s) and fewest. The ring
/// has `buffer_capacity` slots within these limits, so small buffers stay small.
const MAX_SLOTS: usize = 1 << 18;
const MIN_SLOTS: usize = 1 << 12;
/// "No node": end of a list. Node indices are `u32`, so a pool holds at most
/// `NONE` nodes (indices `0..NONE`).
const NONE: u32 = u32::MAX;
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

/// A pool node: an [`Entry`] spelled out field by field (a nested `Entry`
/// would pad `next` to 8 bytes) and the next node of its list.
#[derive(Copy, Clone)]
struct Node<X> {
    key: u64,
    seq: u64,
    packet_ptr: *mut c_void,
    extra: X,
    next: u32,
}

impl<X: Copy> Node<X> {
    fn new(entry: Entry<X>, next: u32) -> Self {
        Node {
            key: entry.key,
            seq: entry.seq,
            packet_ptr: entry.packet_ptr,
            extra: entry.extra,
            next,
        }
    }

    fn entry(&self) -> Entry<X> {
        Entry {
            key: self.key,
            seq: self.seq,
            packet_ptr: self.packet_ptr,
            extra: self.extra,
        }
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
    /// First node of each ring slot, or `NONE`.
    heads: Vec<u32>,
    /// The node pool: `0..nodes.len()` have been used at least once, the rest
    /// of the reservation never.
    nodes: Vec<Node<X>>,
    /// First node of the free list, or `NONE`.
    free: u32,
    /// Bit `i` is set while ring slot `i` is not empty.
    occupied: Vec<u64>,
    /// Entries in the ring.
    in_ring: usize,
    overflow: BinaryHeap<Reverse<Entry<X>>>,
}

impl<X: Copy> Wheel<X> {
    /// An empty queue for up to `capacity` entries; fails if any of its memory
    /// (see the module doc) cannot be allocated, including when `capacity` is
    /// beyond what a `u32` node index addresses.
    pub fn try_new(capacity: usize) -> Result<Self, TryReserveError> {
        if u32::try_from(capacity).is_err() {
            // No constructor for the error: a reservation beyond `isize::MAX`
            // bytes always reports a capacity overflow.
            Vec::<u8>::new().try_reserve_exact(usize::MAX)?;
        }
        let slots = capacity.clamp(MIN_SLOTS, MAX_SLOTS).next_power_of_two();
        let mut overflow = BinaryHeap::new();
        overflow.try_reserve_exact(capacity)?;
        let mut current = Vec::new();
        current.try_reserve_exact(capacity)?;
        let mut nodes = Vec::new();
        nodes.try_reserve_exact(capacity)?;
        let mut heads = Vec::new();
        heads.try_reserve_exact(slots)?;
        heads.resize(slots, NONE);
        let mut occupied = Vec::new();
        occupied.try_reserve_exact(slots / 64)?;
        occupied.resize(slots / 64, 0);
        Ok(Wheel {
            current,
            head: 0,
            cursor: 0,
            heads,
            nodes,
            free: NONE,
            occupied,
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

    /// The entry the `n`-th pop from now returns (`n == 0`: [`Wheel::peek`]) if
    /// nothing is pushed in between, or `None` if it is not in `current`: only
    /// that sorted array is looked into, so the ring and the overflow, which
    /// are not in order yet, are never seen.
    #[inline]
    pub fn peek_nth(&self, n: usize) -> Option<&Entry<X>> {
        self.current.get(self.head.checked_add(n)?)
    }

    /// Removes and returns the entry with the smallest key.
    #[inline]
    pub fn pop(&mut self) -> Option<Entry<X>> {
        let entry = *self.current.get(self.head)?;
        self.head += 1;
        if self.head == self.current.len() {
            self.current.clear();
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
        } else if slot - self.cursor < self.heads.len() as u64 {
            self.push_ring(slot, entry);
        } else {
            self.overflow.push(Reverse(entry));
        }
    }

    /// Inserts into the sorted live part of `current`. The queue is below its
    /// capacity (the caller checks), so dropping the dead prefix always makes
    /// room within the reservation.
    fn insert_current(&mut self, entry: Entry<X>) {
        let full = self.current.len() == self.current.capacity();
        if full || (self.head >= COMPACT_MIN && self.head * 2 >= self.current.len()) {
            self.current.drain(..self.head);
            self.head = 0;
        }
        let live = &self.current[self.head..];
        let pos = self.head + live.partition_point(|e| e.key <= entry.key);
        debug_assert!(self.current.len() < self.current.capacity());
        self.current.insert(pos, entry);
    }

    /// Links `entry` into ring slot `slot`, in a node of the free list or else
    /// the first never-used one. Ring entries never exceed the capacity the
    /// pool was reserved for, so the pool never grows.
    #[inline]
    fn push_ring(&mut self, slot: u64, entry: Entry<X>) {
        let index = slot as usize & (self.heads.len() - 1);
        let node_index = self.free;
        if node_index == NONE {
            debug_assert!(self.nodes.len() < self.nodes.capacity());
            // `len < capacity <= u32::MAX`, see `try_new`.
            let new = self.nodes.len() as u32;
            self.nodes.push(Node::new(entry, self.heads[index]));
            self.heads[index] = new;
        } else {
            let node = &mut self.nodes[node_index as usize];
            self.free = node.next;
            *node = Node::new(entry, self.heads[index]);
            self.heads[index] = node_index;
        }
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
        let end = slot + self.heads.len() as u64;
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
        let index = slot as usize & (self.heads.len() - 1);
        let mut node_index = std::mem::replace(&mut self.heads[index], NONE);
        while node_index != NONE {
            let node = &mut self.nodes[node_index as usize];
            self.current.push(node.entry());
            let next = std::mem::replace(&mut node.next, self.free);
            self.free = node_index;
            node_index = next;
        }
        self.occupied[index / 64] &= !(1 << (index % 64));
        self.in_ring -= self.current.len();
        self.current.sort_unstable_by_key(|e| e.key);
    }

    /// The absolute slot of the first non-empty ring slot after the cursor.
    fn next_ring_slot(&self) -> u64 {
        let mask = self.heads.len() - 1;
        let start = (self.cursor as usize + 1) & mask;
        let index = self
            .first_set(start, self.heads.len())
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
