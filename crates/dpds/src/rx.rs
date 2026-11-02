//! RX step: poll the ports round-robin.

use dpdk::RawMbuf;

use crate::consts::MAX_BURST_SIZE;
use crate::io::RxBurst;
use crate::stats::Telemetry;

pub struct Rx {
    port: u16,
    num_ports: u16,
}

impl Rx {
    pub fn new(num_ports: u16) -> Rx {
        Rx { port: 0, num_ports }
    }

    /// The port the next [`step`](Rx::step) polls.
    pub fn next_port(&self) -> u16 {
        self.port
    }

    /// One `rx_burst` of `MAX_BURST_SIZE` on the current port, then on to
    /// the next port (even if nothing was received). Counts `rx_pkts` and,
    /// for a full burst, `rx_full_bursts`. Returns the number received.
    pub fn step(
        &mut self,
        q: &mut impl RxBurst,
        t: &mut Telemetry<'_>,
        batch: &mut [RawMbuf; MAX_BURST_SIZE],
    ) -> usize {
        let n = q.rx_burst(self.port, &mut batch[..]) as usize;
        let c = &t.stats.counters;
        c.rx_pkts.add(n as u64);
        if n == MAX_BURST_SIZE {
            c.rx_full_bursts.inc();
        }
        self.port += 1;
        if self.port == self.num_ports {
            self.port = 0;
        }
        n
    }
}
