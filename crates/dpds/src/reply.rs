//! A telemetry reply as plain data, so the wire fields can be built and tested
//! without an EAL. The registration shims copy a [`Reply`] into the
//! `dpdk::telemetry::Dict`, entry by entry, in order.

/// One value of a telemetry dict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Uint(u64),
    Str(String),
    /// A uint64 array container.
    UintArray(Vec<u64>),
    /// A string array container.
    StrArray(Vec<String>),
}

/// Ordered `(key, value)` entries of one reply dict.
pub type Reply = Vec<(String, Value)>;
