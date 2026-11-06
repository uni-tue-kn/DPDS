//! First-loss telemetry: the first packet loss of a link, with reason and detail.

use std::fmt;
#[cfg(feature = "telemetry")]
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

/// Why a packet was first lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FirstLossReason {
    /// `Buffer::push_outcome` rejected a packet (`cause=` in the detail says why).
    DpdsReject,
    /// A partial `tx_burst`: the NIC's TX descriptor ring was full.
    DescriptorFull,
    /// NIC `imissed` rose (wire spelling `IMissed`).
    RxMissed,
    /// NIC `rx_nombuf` rose (`RX_NoMbuf`).
    RxNoMbuf,
    /// NIC `ierrors` rose (`IErrors`).
    RxError,
    /// NIC `oerrors` rose (`OErrors`).
    TxError,
}

impl FirstLossReason {
    /// Inverse of `as u8`; only fed bytes this module wrote.
    #[cfg(feature = "telemetry")]
    fn from_u8(v: u8) -> Self {
        match v {
            0 => FirstLossReason::DpdsReject,
            1 => FirstLossReason::DescriptorFull,
            2 => FirstLossReason::RxMissed,
            3 => FirstLossReason::RxNoMbuf,
            4 => FirstLossReason::RxError,
            5 => FirstLossReason::TxError,
            _ => unreachable!("invalid FirstLossReason byte {v}"),
        }
    }
}

/// Wire spelling of `first_loss_reason`; consumers parse these strings. The
/// software reasons use their identifier, the NIC ones the DPDK counter name.
impl fmt::Display for FirstLossReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FirstLossReason::DpdsReject => "DpdsReject",
            FirstLossReason::DescriptorFull => "DescriptorFull",
            FirstLossReason::RxMissed => "IMissed",
            FirstLossReason::RxNoMbuf => "RX_NoMbuf",
            FirstLossReason::RxError => "IErrors",
            FirstLossReason::TxError => "OErrors",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    /// The data-plane worker (`dpds`).
    Dpds,
    /// The NIC hardware as a `to=` destination (`nic`).
    Nic,
}

impl Stage {
    /// Inverse of `as u8`; only fed bytes this module wrote.
    #[cfg(feature = "telemetry")]
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Stage::Dpds,
            1 => Stage::Nic,
            _ => unreachable!("invalid Stage byte {v}"),
        }
    }
}

/// Lowercase wire spelling (part of `first_loss_detail`).
impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stage::Dpds => "dpds",
            Stage::Nic => "nic",
        })
    }
}

/// Why the buffer rejected a packet; precedence: loss model, limiter, capacity
/// (see `dpds_core::buffer::PushOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropCause {
    Loss,
    Limiter,
    BufferFull,
}

impl DropCause {
    #[cfg(feature = "telemetry")]
    fn from_u32(v: u32) -> Self {
        match v {
            0 => DropCause::Loss,
            1 => DropCause::Limiter,
            2 => DropCause::BufferFull,
            _ => unreachable!("invalid DropCause code {v}"),
        }
    }
}

/// Wire spelling (part of `first_loss_detail`).
impl fmt::Display for DropCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DropCause::Loss => "loss",
            DropCause::Limiter => "limiter",
            DropCause::BufferFull => "buffer_full",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageLossDetail {
    /// `DpdsReject` without a known cause: `from=<stage>`.
    Single { from: Stage },
    /// `DpdsReject`: `from=<stage>,cause=<loss|limiter|buffer_full>`.
    Reject { from: Stage, cause: DropCause },
    /// `DescriptorFull`: `from=<stage>,to=<stage>` (`to` is `Stage::Nic`).
    RingFull { from: Stage, to: Stage },
    /// `RxMissed`/`RxNoMbuf`/`RxError`/`TxError`: `from=<stage>,count=<n>`.
    HardwareDrop { from: Stage, count: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageLossEvent {
    pub reason: FirstLossReason,
    pub detail: StageLossDetail,
    pub relative_ns: f64,
}

/// At most one [`StageLossEvent`] for its lifetime, first writer wins. With the
/// `telemetry` feature: lock-free atomics, cache-line aligned so trackers of
/// different lcores never share a line. Without it: a zero-sized struct whose
/// `try_record` is always `false` and `current` always `None`.
///
/// `claimed` is the first-writer-wins CAS gate; `ready` is set (`Release`) after
/// every payload field is written and checked (`Acquire`) before any is read,
/// so the payload fields are `Relaxed`.
#[repr(align(64))]
#[cfg(feature = "telemetry")]
pub struct StageLossTracker {
    claimed: AtomicBool,
    ready: AtomicBool,
    reason: AtomicU8,
    // The recorded `StageLossDetail` variant, independent of `reason`.
    shape: AtomicU8,
    from: AtomicU8,
    to: AtomicU8,
    count: AtomicU32,
    relative_ns_bits: AtomicU64,
}

#[cfg(feature = "telemetry")]
mod shape_tag {
    pub const SINGLE: u8 = 0;
    pub const RING_FULL: u8 = 1;
    pub const HARDWARE_DROP: u8 = 2;
    pub const REJECT: u8 = 3;
}

#[repr(align(64))]
#[cfg(not(feature = "telemetry"))]
pub struct StageLossTracker;

#[cfg(feature = "telemetry")]
impl StageLossTracker {
    pub fn new() -> Self {
        StageLossTracker {
            claimed: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            reason: AtomicU8::new(0),
            shape: AtomicU8::new(0),
            from: AtomicU8::new(0),
            to: AtomicU8::new(0),
            count: AtomicU32::new(0),
            relative_ns_bits: AtomicU64::new(0),
        }
    }

    /// `true` iff this call won the slot (a loser does not retry).
    pub fn try_record(
        &self,
        reason: FirstLossReason,
        detail: StageLossDetail,
        relative_ns: f64,
    ) -> bool {
        if self
            .claimed
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return false;
        }

        // Unused fields get a default; `current()` reads only the ones the shape uses.
        let (shape, from, to, count) = match detail {
            StageLossDetail::Single { from } => (shape_tag::SINGLE, from, Stage::Dpds, 0),
            // the cause code travels in `count`
            StageLossDetail::Reject { from, cause } => {
                (shape_tag::REJECT, from, Stage::Dpds, cause as u32)
            }
            StageLossDetail::RingFull { from, to } => (shape_tag::RING_FULL, from, to, 0),
            StageLossDetail::HardwareDrop { from, count } => {
                (shape_tag::HARDWARE_DROP, from, Stage::Dpds, count)
            }
        };

        self.reason.store(reason as u8, Ordering::Relaxed);
        self.shape.store(shape, Ordering::Relaxed);
        self.from.store(from as u8, Ordering::Relaxed);
        self.to.store(to as u8, Ordering::Relaxed);
        self.count.store(count, Ordering::Relaxed);
        self.relative_ns_bits
            .store(relative_ns.to_bits(), Ordering::Relaxed);

        self.ready.store(true, Ordering::Release);
        true
    }

    pub fn current(&self) -> Option<StageLossEvent> {
        if !self.ready.load(Ordering::Acquire) {
            return None;
        }

        let reason = FirstLossReason::from_u8(self.reason.load(Ordering::Relaxed));
        let from = Stage::from_u8(self.from.load(Ordering::Relaxed));
        let detail = match self.shape.load(Ordering::Relaxed) {
            shape_tag::SINGLE => StageLossDetail::Single { from },
            shape_tag::RING_FULL => StageLossDetail::RingFull {
                from,
                to: Stage::from_u8(self.to.load(Ordering::Relaxed)),
            },
            shape_tag::REJECT => StageLossDetail::Reject {
                from,
                cause: DropCause::from_u32(self.count.load(Ordering::Relaxed)),
            },
            shape_tag::HARDWARE_DROP => StageLossDetail::HardwareDrop {
                from,
                count: self.count.load(Ordering::Relaxed),
            },
            other => unreachable!("invalid StageLossDetail shape tag {other}"),
        };

        Some(StageLossEvent {
            reason,
            detail,
            relative_ns: f64::from_bits(self.relative_ns_bits.load(Ordering::Relaxed)),
        })
    }
}

#[cfg(not(feature = "telemetry"))]
impl StageLossTracker {
    pub fn new() -> Self {
        StageLossTracker
    }

    #[inline(always)]
    pub fn try_record(
        &self,
        _reason: FirstLossReason,
        _detail: StageLossDetail,
        _relative_ns: f64,
    ) -> bool {
        false
    }

    #[inline(always)]
    pub fn current(&self) -> Option<StageLossEvent> {
        None
    }
}

impl Default for StageLossTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Wire fields of one first-loss result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstLossFields {
    pub reason: String,
    pub detail: String,
    pub relative_ns: u64,
}

/// `None` -> `("Clean", "", 0)`; `relative_ns` truncates. The one place the wire
/// format of a first-loss result is built.
pub fn format_first_loss(event: Option<StageLossEvent>) -> FirstLossFields {
    match event {
        Some(event) => {
            let detail = match event.detail {
                StageLossDetail::Single { from } => format!("from={from}"),
                StageLossDetail::Reject { from, cause } => format!("from={from},cause={cause}"),
                StageLossDetail::RingFull { from, to } => format!("from={from},to={to}"),
                StageLossDetail::HardwareDrop { from, count } => {
                    format!("from={from},count={count}")
                }
            };
            FirstLossFields {
                reason: event.reason.to_string(),
                detail,
                relative_ns: event.relative_ns as u64,
            }
        }
        None => FirstLossFields {
            reason: "Clean".to_string(),
            detail: String::new(),
            relative_ns: 0,
        },
    }
}

/// First counter that rose between two polls, priority `imissed` > `rx_nombuf` >
/// `ierrors` > `oerrors`; decreases count as no change.
pub fn detect_hardware_drop(
    prev: &dpdk::port::PortStats,
    curr: &dpdk::port::PortStats,
) -> Option<(FirstLossReason, u32)> {
    let imissed = curr.imissed.saturating_sub(prev.imissed);
    let rx_nombuf = curr.rx_nombuf.saturating_sub(prev.rx_nombuf);
    let ierrors = curr.ierrors.saturating_sub(prev.ierrors);
    let oerrors = curr.oerrors.saturating_sub(prev.oerrors);

    if imissed > 0 {
        Some((FirstLossReason::RxMissed, imissed as u32))
    } else if rx_nombuf > 0 {
        Some((FirstLossReason::RxNoMbuf, rx_nombuf as u32))
    } else if ierrors > 0 {
        Some((FirstLossReason::RxError, ierrors as u32))
    } else if oerrors > 0 {
        Some((FirstLossReason::TxError, oerrors as u32))
    } else {
        None
    }
}
