//! A telemetry reply as plain data, so the exact wire fields can be built and
//! tested without an EAL. The registration shims copy a [`Reply`] into the
//! `dpdk::telemetry::Dict` the telemetry thread hands them, entry by entry,
//! in order.

/// One value of a telemetry dict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// `rte_tel_data_add_dict_uint`
    Uint(u64),
    /// `rte_tel_data_add_dict_string`
    Str(String),
    /// A uint64 array container (`Container::new_uint_array`)
    UintArray(Vec<u64>),
    /// A string array container (`Container::new_string_array`)
    StrArray(Vec<String>),
}

/// Ordered `(key, value)` entries of one reply dict.
pub type Reply = Vec<(String, Value)>;
