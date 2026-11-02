//! Shared test doubles: fake mbufs and mock NIC/ring/free implementations.
//!
//! A "fake mbuf" is a heap-allocated, zeroed `dpdk_sys::rte_mbuf` with `port`
//! and `pkt_len` set. It is only ever handed to code that must not call DPDK
//! on it (the mock `MbufFree` never dereferences it).

use std::collections::VecDeque;

use dpdk::RawMbuf;
use dpds::io::{Enqueue, MbufFree, RxBurst, TxBurst};

pub struct FakeMbufs {
    mbufs: Box<[dpdk_sys::rte_mbuf]>,
}

impl FakeMbufs {
    /// `n` fake mbufs, all on `port`, all `pkt_len` bytes long.
    pub fn new(n: usize, port: u16, pkt_len: u32) -> FakeMbufs {
        // A boxed slice: the mbufs never move, so raw pointers to them stay valid.
        let mbufs = (0..n)
            .map(|_| {
                let mut m: dpdk_sys::rte_mbuf = unsafe { std::mem::zeroed() };
                m.__bindgen_anon_1.__bindgen_anon_1.port = port;
                m.__bindgen_anon_2.__bindgen_anon_1.pkt_len = pkt_len;
                m
            })
            .collect();
        FakeMbufs { mbufs }
    }

    pub fn ptr(&mut self, i: usize) -> RawMbuf {
        &mut self.mbufs[i] as *mut _
    }

    pub fn ptrs(&mut self) -> Vec<RawMbuf> {
        (0..self.mbufs.len()).map(|i| self.ptr(i)).collect()
    }

    pub fn set_port(&mut self, i: usize, port: u16) {
        self.mbufs[i].__bindgen_anon_1.__bindgen_anon_1.port = port;
    }

    pub fn port(&self, i: usize) -> u16 {
        port_of(&self.mbufs[i] as *const _ as RawMbuf)
    }
}

pub fn port_of(m: RawMbuf) -> u16 {
    unsafe { (*m).__bindgen_anon_1.__bindgen_anon_1.port }
}

/// Mock RX: every `rx_burst` call pops the next scripted batch (an empty
/// script means "no packets"). Records `(port, bufs.len())` of every call.
#[derive(Default)]
pub struct MockRx {
    pub script: VecDeque<Vec<RawMbuf>>,
    pub calls: Vec<(u16, usize)>,
}

impl MockRx {
    pub fn with_script(script: Vec<Vec<RawMbuf>>) -> MockRx {
        MockRx {
            script: script.into(),
            calls: Vec::new(),
        }
    }
}

impl RxBurst for MockRx {
    fn rx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 {
        self.calls.push((port, bufs.len()));
        let Some(next) = self.script.pop_front() else {
            return 0;
        };
        assert!(next.len() <= bufs.len(), "script batch exceeds buffer");
        bufs[..next.len()].copy_from_slice(&next);
        next.len() as u16
    }
}

/// Mock TX: accepts at most `accept_limit` mbufs per call (default: all).
/// Records `(port, mbufs passed)` for every call, in order.
pub struct MockTx {
    pub accept_limit: usize,
    pub calls: Vec<(u16, Vec<RawMbuf>)>,
}

impl Default for MockTx {
    fn default() -> Self {
        MockTx {
            accept_limit: usize::MAX,
            calls: Vec::new(),
        }
    }
}

impl TxBurst for MockTx {
    fn tx_burst(&mut self, port: u16, bufs: &mut [RawMbuf]) -> u16 {
        self.calls.push((port, bufs.to_vec()));
        bufs.len().min(self.accept_limit) as u16
    }
}

/// Mock free: records freed mbufs in call order.
#[derive(Default)]
pub struct MockFree {
    pub freed: Vec<RawMbuf>,
}

impl MbufFree for MockFree {
    fn free(&mut self, mbuf: RawMbuf) {
        self.freed.push(mbuf);
    }
}

/// Mock ring with a fixed number of free slots; enqueue is all-or-prefix
/// (like `rte_ring_enqueue_burst`: as many as fit, in order).
pub struct MockRing {
    pub free_slots: usize,
    pub enqueued: Vec<RawMbuf>,
}

impl MockRing {
    pub fn with_space(free_slots: usize) -> MockRing {
        MockRing {
            free_slots,
            enqueued: Vec::new(),
        }
    }
}

impl Enqueue for MockRing {
    fn enqueue_burst(&mut self, bufs: &[RawMbuf]) -> u16 {
        let n = bufs.len().min(self.free_slots);
        self.enqueued.extend_from_slice(&bufs[..n]);
        self.free_slots -= n;
        n as u16
    }
}
