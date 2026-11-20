//! Application-defined telemetry commands.
//!
//! DPDK's telemetry thread (spawned by `rte_eal_init`) serves JSON over a Unix
//! socket. This module registers application commands on it, so app-level
//! counters appear in `dpdk-telemetry.py` next to `/ethdev/stats`.
//!
//! Callbacks run on the telemetry control thread, concurrently with the data
//! path: counters they read must be atomics (or otherwise thread-safe).

use std::collections::HashMap;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr::NonNull;
use std::sync::{Mutex, OnceLock};

use crate::error::DpdkError;

/// A telemetry response being filled as a dictionary (rendered as a JSON object).
pub struct Dict {
    raw: *mut dpdk_sys::rte_tel_data,
}

impl Dict {
    /// Adds one `"key": value` entry. Keys longer than DPDK's limit
    /// (`RTE_TEL_MAX_STRING_LEN`, 64 bytes) are rejected by the library and
    /// silently skipped: telemetry output is best-effort.
    pub fn add(&mut self, key: &str, value: u64) {
        let Ok(key) = CString::new(key) else { return };
        unsafe { dpdk_sys::rte_tel_data_add_dict_uint(self.raw, key.as_ptr(), value) };
    }

    /// Adds one `"key": "value"` string entry (same key-length caveat).
    pub fn add_str(&mut self, key: &str, value: &str) {
        let Ok(key) = CString::new(key) else { return };
        let Ok(value) = CString::new(value) else {
            return;
        };
        unsafe { dpdk_sys::rte_tel_data_add_dict_string(self.raw, key.as_ptr(), value.as_ptr()) };
    }

    /// Attaches `container` under `key`, handing ownership of its `rte_tel_data`
    /// to DPDK (`keep=0`: DPDK frees it after serializing the response). Same
    /// key-length caveat; if `key` is not a valid `CString` the container is
    /// dropped normally, since DPDK never took ownership.
    pub fn add_array_container(&mut self, key: &str, container: Container) {
        let Ok(key) = CString::new(key) else { return };
        let ret = unsafe {
            dpdk_sys::rte_tel_data_add_dict_container(
                self.raw,
                key.as_ptr(),
                container.raw.as_ptr(),
                0,
            )
        };
        if ret == 0 {
            // DPDK owns it now (keep=0) and frees it with the outer dict; don't
            // let `Container::drop` free it again.
            std::mem::forget(container);
        }
        // On failure DPDK did not take ownership; `container` drops normally.
    }
}

/// An owned `rte_tel_data` used as a standalone array before it is attached to a
/// [`Dict`] via [`Dict::add_array_container`].
pub struct Container {
    raw: NonNull<dpdk_sys::rte_tel_data>,
}

impl Container {
    /// A new string array (`RTE_TEL_STRING_VAL`).
    pub fn new_string_array() -> Container {
        let ptr = unsafe { dpdk_sys::rte_tel_data_alloc() };
        let raw = NonNull::new(ptr).expect("rte_tel_data_alloc returned NULL");
        unsafe {
            dpdk_sys::rte_tel_data_start_array(
                raw.as_ptr(),
                dpdk_sys::rte_tel_value_type_RTE_TEL_STRING_VAL,
            )
        };
        Container { raw }
    }

    /// Appends one string entry; silently skipped on an interior NUL, like
    /// `Dict::add_str`.
    pub fn add_string(&mut self, s: &str) {
        let Ok(s) = CString::new(s) else { return };
        unsafe { dpdk_sys::rte_tel_data_add_array_string(self.raw.as_ptr(), s.as_ptr()) };
    }

    /// A new uint64 array (`RTE_TEL_UINT_VAL`), for the columnar per-epoch
    /// arrays of `/dpds/epochs`: `rte_tel_data_add_array_container` only accepts
    /// arrays of `uint64_t`/`int`/`string`, so a per-epoch JSON object is not
    /// representable.
    pub fn new_uint_array() -> Container {
        let ptr = unsafe { dpdk_sys::rte_tel_data_alloc() };
        let raw = NonNull::new(ptr).expect("rte_tel_data_alloc returned NULL");
        unsafe {
            dpdk_sys::rte_tel_data_start_array(
                raw.as_ptr(),
                dpdk_sys::rte_tel_value_type_RTE_TEL_UINT_VAL,
            )
        };
        Container { raw }
    }

    /// Appends one uint64 entry; the container must come from
    /// [`new_uint_array`](Self::new_uint_array).
    ///
    /// An array holds at most `RTE_TEL_MAX_ARRAY_ENTRIES` (512) entries; from
    /// entry 513 on DPDK drops the value and this returns its error (`-ENOSPC`),
    /// so a caller cannot send a cut array by accident.
    pub fn add_uint(&mut self, v: u64) -> Result<(), DpdkError> {
        let ret = unsafe { dpdk_sys::rte_tel_data_add_array_uint(self.raw.as_ptr(), v) };
        if ret < 0 {
            Err(DpdkError(ret))
        } else {
            Ok(())
        }
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        unsafe { dpdk_sys::rte_tel_data_free(self.raw.as_ptr()) };
    }
}

type Callback = Box<dyn Fn(&str, &mut Dict) + Send + Sync + 'static>;

/// Registered callbacks by command name. DPDK keeps the name pointer, hence the
/// leaked `&'static CStr`.
static CALLBACKS: OnceLock<Mutex<HashMap<&'static CStr, Callback>>> = OnceLock::new();

extern "C" fn trampoline(
    cmd: *const c_char,
    params: *const c_char,
    data: *mut dpdk_sys::rte_tel_data,
) -> c_int {
    // Safety: DPDK passes the same NUL-terminated command string it was
    // registered with.
    let cmd = unsafe { CStr::from_ptr(cmd) };
    // Safety: `params` is either NULL or a NUL-terminated string owned by DPDK
    // for the duration of this call. Client-supplied, so never panic on it: NULL
    // or invalid UTF-8 fall back to "".
    let params = if params.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(params) }.to_str().unwrap_or("")
    };
    let callbacks = CALLBACKS.get().expect("callback fired before registration");
    let callbacks = callbacks.lock().expect("telemetry callback map poisoned");
    let Some(cb) = callbacks.get(cmd) else {
        return -22; // -EINVAL: not one of ours
    };
    unsafe { dpdk_sys::rte_tel_data_start_dict(data) };
    cb(params, &mut Dict { raw: data });
    0
}

/// Registers `cmd` (must start with `/`, e.g. `"/myapp/drops"`); a query over the
/// telemetry socket invokes `callback` to fill the response dictionary.
///
/// `callback`'s first argument is the raw text after the comma in a `/cmd,param`
/// query (`/dpds/stats,1` gives `"1"`), or `""` without a parameter.
///
/// Registration is permanent (DPDK has no unregister) and the callback runs on
/// the telemetry control thread, hence the `Send + Sync + 'static` bounds.
pub fn register_command(
    cmd: &str,
    help: &str,
    callback: impl Fn(&str, &mut Dict) + Send + Sync + 'static,
) -> Result<(), DpdkError> {
    let cmd = CString::new(cmd).map_err(|_| DpdkError(-22))?; // -EINVAL
    let help = CString::new(help).map_err(|_| DpdkError(-22))?;
    // DPDK keeps both pointers for the process lifetime.
    let cmd: &'static CStr = Box::leak(cmd.into_boxed_c_str());
    let help: &'static CStr = Box::leak(help.into_boxed_c_str());

    // Insert before registering: the command is queryable once
    // rte_telemetry_register_cmd returns.
    CALLBACKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("telemetry callback map poisoned")
        .insert(cmd, Box::new(callback));

    let ret = unsafe {
        dpdk_sys::rte_telemetry_register_cmd(cmd.as_ptr(), Some(trampoline), help.as_ptr())
    };
    if ret < 0 {
        CALLBACKS
            .get()
            .unwrap()
            .lock()
            .expect("telemetry callback map poisoned")
            .remove(cmd);
        // DPDK only returns -EINVAL, so name the limits it checks
        // (lib/telemetry/telemetry.c): cmd < 56 chars of [A-Za-z0-9_/] starting
        // with '/', help < RTE_TEL_MAX_STRING_LEN (128) chars.
        eprintln!(
            "rte_telemetry_register_cmd({cmd:?}) failed ({ret}); limits: cmd < 56 chars of \
             [A-Za-z0-9_/] starting with '/', help < 128 chars (help is {} chars)",
            help.to_bytes().len()
        );
        return Err(DpdkError(ret));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! DPDK's telemetry data structures have no public read-back accessor, so
    //! these tests only assert that construction, population and the ownership
    //! handoff do not panic or double-free, not that the JSON is right. The
    //! `rte_tel_data_*` calls used need no `rte_eal_init()`.

    use super::*;

    #[test]
    fn container_new_string_array_and_add_string_do_not_panic() {
        let mut container = Container::new_string_array();
        container.add_string("flow=3,reason=Distance,seq=12345,link=A,ts_ns=98765432100");
        container.add_string("flow=7,reason=Duplicate,seq=42,link=B,ts_ns=1000");
        container.add_string("flow=1,reason=CapacityExceeded,seq=99,link=A,ts_ns=5000");
        // Dropping an un-attached Container (rte_tel_data_free) must not crash.
        drop(container);
    }

    #[test]
    fn dict_add_array_container_attaches_and_transfers_ownership_without_double_free() {
        // A root dict built against the private `raw` field (only the telemetry
        // trampoline constructs a `Dict` otherwise).
        let root = unsafe { dpdk_sys::rte_tel_data_alloc() };
        assert!(
            !root.is_null(),
            "rte_tel_data_alloc must succeed with no EAL init"
        );
        let start_ret = unsafe { dpdk_sys::rte_tel_data_start_dict(root) };
        assert_eq!(
            start_ret, 0,
            "rte_tel_data_start_dict must succeed with no EAL init"
        );
        let mut dict = Dict { raw: root };

        let mut container = Container::new_string_array();
        container.add_string("flow=3,reason=Distance,seq=12345,link=A,ts_ns=98765432100");

        // keep=0: DPDK owns the container's rte_tel_data and frees it with the
        // outer dict, so add_array_container must forget its own handle, or
        // freeing `dict` below would double-free.
        dict.add_array_container("first_loss_history", container);

        // Freeing the outer dict must not crash: the only signal here (no
        // ASan/miri) that ownership was transferred rather than double-freed.
        unsafe { dpdk_sys::rte_tel_data_free(dict.raw) };
    }

    // uint64 Container: the same two tests as above.

    #[test]
    fn container_new_uint_array_and_add_uint_do_not_panic() {
        let mut container = Container::new_uint_array();
        container.add_uint(0).unwrap();
        container.add_uint(1_000_000_000).unwrap();
        container.add_uint(u64::MAX).unwrap();
        drop(container);
    }

    #[test]
    fn dict_add_array_container_attaches_uint_container_and_transfers_ownership_without_double_free(
    ) {
        let root = unsafe { dpdk_sys::rte_tel_data_alloc() };
        assert!(
            !root.is_null(),
            "rte_tel_data_alloc must succeed with no EAL init"
        );
        let start_ret = unsafe { dpdk_sys::rte_tel_data_start_dict(root) };
        assert_eq!(
            start_ret, 0,
            "rte_tel_data_start_dict must succeed with no EAL init"
        );
        let mut dict = Dict { raw: root };

        let mut container = Container::new_uint_array();
        container.add_uint(42).unwrap();
        container.add_uint(1_234_567_890).unwrap();

        // keep=0: same handoff as for the string container.
        dict.add_array_container("epoch_polls", container);

        unsafe { dpdk_sys::rte_tel_data_free(dict.raw) };
    }
}
