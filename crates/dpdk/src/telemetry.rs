//! Application-defined telemetry commands.
//!
//! DPDK's telemetry thread (spawned by `rte_eal_init`, see README) serves
//! JSON over a Unix socket; every library registers read-only commands on
//! it.  This module lets the application register its own, so app-level
//! counters appear in `dpdk-telemetry.py` next to `/ethdev/stats`.
//!
//! The callback runs on the telemetry control thread, concurrently with the
//! data path — counters it reads must be atomics (or otherwise thread-safe).

use std::collections::HashMap;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr::NonNull;
use std::sync::{Mutex, OnceLock};

use crate::error::DpdkError;

/// A telemetry response being filled as a dictionary of `u64` values
/// (rendered as a JSON object by the telemetry library).
pub struct Dict {
    raw: *mut dpdk_sys::rte_tel_data,
}

impl Dict {
    /// Add one `"key": value` entry.  Keys longer than DPDK's limit
    /// (`RTE_TEL_MAX_STRING_LEN`, 64 bytes) are rejected by the library and
    /// silently skipped here — telemetry output is best-effort by design.
    pub fn add(&mut self, key: &str, value: u64) {
        let Ok(key) = CString::new(key) else { return };
        unsafe { dpdk_sys::rte_tel_data_add_dict_uint(self.raw, key.as_ptr(), value) };
    }

    /// Add one `"key": "value"` string entry (same key-length caveat as `add`).
    pub fn add_str(&mut self, key: &str, value: &str) {
        let Ok(key) = CString::new(key) else { return };
        let Ok(value) = CString::new(value) else {
            return;
        };
        unsafe { dpdk_sys::rte_tel_data_add_dict_string(self.raw, key.as_ptr(), value.as_ptr()) };
    }

    /// Attach `container` (e.g. built via [`Container::new_string_array`])
    /// under `key`, handing ownership of its underlying `rte_tel_data` to
    /// DPDK's telemetry library (`keep=0`: DPDK frees it once it has
    /// finished serializing this response).  Same key-length caveat as
    /// `add`/`add_str`; on a `CString::new` failure for `key`, `container`
    /// is dropped normally (freeing itself) rather than leaked, since the
    /// attach call never happened and DPDK never took ownership.
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
            // DPDK took ownership (keep=0): its own free of the outer dict
            // will free this container too. Don't let `Container::drop`
            // free it a second time.
            std::mem::forget(container);
        }
        // On failure DPDK did not take ownership; let `container` drop
        // normally here, freeing itself via `Container::drop`.
    }
}

/// An owned `rte_tel_data` used as a standalone array (e.g. of strings),
/// prior to being attached to a [`Dict`] via
/// [`Dict::add_array_container`]. Mirrors `Ring`'s (`ring.rs`)
/// alloc/`NonNull`/`Drop` ownership idiom.
pub struct Container {
    raw: NonNull<dpdk_sys::rte_tel_data>,
}

impl Container {
    /// Allocates a new `rte_tel_data` and starts it as a string array
    /// (`RTE_TEL_STRING_VAL`).
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

    /// Append one string entry. Silently skipped on a `CString::new`
    /// failure (interior NUL), matching `Dict::add_str`'s existing
    /// best-effort convention.
    pub fn add_string(&mut self, s: &str) {
        let Ok(s) = CString::new(s) else { return };
        unsafe { dpdk_sys::rte_tel_data_add_array_string(self.raw.as_ptr(), s.as_ptr()) };
    }

    /// Allocates a new `rte_tel_data` and starts it as a uint64 array
    /// (`RTE_TEL_UINT_VAL`) -- the columnar per-epoch counter/timestamp
    /// arrays the `/dpds/epochs` command needs (one `uint64` array per
    /// counter, indexed by epoch;
    /// `rte_tel_data_add_array_container`'s own contract requires a
    /// container to be an array of `uint64_t`/`int`/`string`, so a
    /// per-epoch JSON *object* is not representable this way -- see
    /// `crates/dpds/src/counters.rs` for the design this exists to
    /// support).
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

    /// Append one uint64 entry (`rte_tel_data_add_array_uint`). Must have
    /// been started via [`new_uint_array`](Self::new_uint_array), not
    /// [`new_string_array`](Self::new_string_array).
    ///
    /// An array holds at most `RTE_TEL_MAX_ARRAY_ENTRIES` (512) entries; from
    /// entry 513 on DPDK drops the value and this returns its error
    /// (`-ENOSPC`), so a caller cannot send a cut array by accident.
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

/// Registered callbacks, keyed by command name.  Command names must outlive
/// the process (DPDK keeps the pointer), hence the leaked `&'static CStr`.
static CALLBACKS: OnceLock<Mutex<HashMap<&'static CStr, Callback>>> = OnceLock::new();

extern "C" fn trampoline(
    cmd: *const c_char,
    params: *const c_char,
    data: *mut dpdk_sys::rte_tel_data,
) -> c_int {
    // Safety: DPDK passes the same NUL-terminated command string it was
    // registered with.
    let cmd = unsafe { CStr::from_ptr(cmd) };
    // Safety: `params` is either NULL or a NUL-terminated string owned by
    // DPDK for the duration of this call. Untrusted (client-supplied), so
    // never panic on it: NULL or invalid UTF-8 both fall back to "".
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

/// Register `cmd` (must start with `/`, e.g. `"/myapp/drops"`) so that
/// querying it over the telemetry socket invokes `callback` to fill the
/// response dictionary.
///
/// `callback`'s first argument is the raw text after the comma in a
/// `/cmd,param` query (e.g. querying `/dpds/stats,1` invokes the callback
/// with `"1"`), or `""` if the query had no parameter.
///
/// Registration is permanent for the lifetime of the process (DPDK offers
/// no unregister), and the callback runs on the telemetry control thread —
/// hence the `Send + Sync + 'static` bounds.
pub fn register_command(
    cmd: &str,
    help: &str,
    callback: impl Fn(&str, &mut Dict) + Send + Sync + 'static,
) -> Result<(), DpdkError> {
    let cmd = CString::new(cmd).map_err(|_| DpdkError(-22))?; // -EINVAL
    let help = CString::new(help).map_err(|_| DpdkError(-22))?;
    // DPDK stores both pointers for the process lifetime.
    let cmd: &'static CStr = Box::leak(cmd.into_boxed_c_str());
    let help: &'static CStr = Box::leak(help.into_boxed_c_str());

    // Insert before registering: the command is queryable as soon as
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
        // DPDK only returns -EINVAL, so name the command and the limits it
        // checks (lib/telemetry/telemetry.c): cmd < 56 chars of [A-Za-z0-9_/]
        // starting with '/', help < RTE_TEL_MAX_STRING_LEN (128) chars.
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
    //! Tests for `Container` and `Dict::add_array_container`: a named
    //! string-array field attached to a telemetry dict response via
    //! `rte_tel_data_add_dict_container(d, name, val, keep=0)`, mirroring
    //! `Ring`'s (`ring.rs`) alloc/free ownership pattern.
    //!
    //! `Dict`'s `raw` field is private outside this module; these tests
    //! reach it directly via `use super::*;`, the usual idiom for reaching
    //! a type's private fields from its own `mod tests`.
    //!
    //! Testability finding: `rte_tel_data_alloc`, `_start_array`,
    //! `_add_array_string`, `_start_dict`, `_add_dict_container`, and
    //! `_free` were confirmed (via a throwaway probe test calling them
    //! directly through `dpdk_sys`, run in this sandbox with no
    //! `rte_eal_init()` call anywhere in the process) to succeed --
    //! return 0 / non-null, no crash -- with no EAL/hugepage/mempool
    //! state touched. So this module needs no special build/run recipe
    //! beyond the usual `CC=gcc RUSTFLAGS='-Ctarget-cpu=native' cargo test
    //! -p dpdk`. DPDK's telemetry data structures have no public
    //! read-back accessor outside of the JSON the live telemetry-socket
    //! handler serializes internally, so these tests can only assert that
    //! the calls succeed and that ownership handoff doesn't crash
    //! (double-free/UAF) -- not that the resulting JSON actually contains
    //! the right key/values. Full round-trip verification needs a live
    //! `rte_eal_init()` + telemetry socket + a real client query (e.g.
    //! `dpdk-telemetry.py`), out of scope for a unit test.

    use super::*;

    #[test]
    fn container_new_string_array_and_add_string_do_not_panic() {
        let mut container = Container::new_string_array();
        container.add_string("flow=3,reason=Distance,seq=12345,link=A,ts_ns=98765432100");
        container.add_string("flow=7,reason=Duplicate,seq=42,link=B,ts_ns=1000");
        container.add_string("flow=1,reason=CapacityExceeded,seq=99,link=A,ts_ns=5000");
        // No public accessor exists to read the array's contents back;
        // the most this test can assert is that construction and
        // population didn't panic, and that dropping an un-attached
        // Container (its Drop impl calling rte_tel_data_free) doesn't
        // crash either.
        drop(container);
    }

    #[test]
    fn dict_add_array_container_attaches_and_transfers_ownership_without_double_free() {
        // A root dict, built directly against the private `raw` field
        // (no public `Dict` constructor exists outside the telemetry
        // trampoline) -- the usual idiom for reaching private fields from
        // `mod tests`.
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

        // keep=0 (per rte_tel_data_add_dict_container's contract): DPDK
        // now owns the container's rte_tel_data and frees it itself when
        // the outer dict is freed. add_array_container must therefore
        // std::mem::forget its own Container handle -- if it instead let
        // Container's Drop run (calling rte_tel_data_free a second time),
        // freeing `dict` below would double-free/use-after-free.
        dict.add_array_container("first_loss_history", container);

        // Freeing the outer dict must not crash. This is the only signal
        // available (no ASan/miri harness in this sandbox) that ownership
        // was transferred correctly rather than double-freed.
        unsafe { dpdk_sys::rte_tel_data_free(dict.raw) };
    }

    // ── uint64 Container (`new_uint_array`/`add_uint`) ──────────────────
    //
    // Mirrors the two string-array tests above exactly, for the columnar
    // per-epoch arrays `/dpds/epochs,<n>` etc. need (see `new_uint_array`'s
    // doc comment). Same testability finding applies: `rte_tel_data_alloc`,
    // `_start_array` with `RTE_TEL_UINT_VAL`, `_add_array_uint`,
    // `_start_dict`, `_add_dict_container`, and `_free` need no
    // `rte_eal_init()` call, so no special build/run recipe beyond the
    // usual `CC=gcc RUSTFLAGS='-Ctarget-cpu=native' cargo test -p dpdk`.
    // Same caveat too: no public accessor exists to read the array's
    // contents back, so these tests can only assert construction/
    // population/ownership-transfer don't panic or double-free, not that
    // the resulting JSON actually contains the right values.

    #[test]
    fn container_new_uint_array_and_add_uint_do_not_panic() {
        let mut container = Container::new_uint_array();
        container.add_uint(0).unwrap();
        container.add_uint(1_000_000_000).unwrap();
        container.add_uint(u64::MAX).unwrap();
        // No public accessor exists to read the array's contents back;
        // the most this test can assert is that construction and
        // population didn't panic, and that dropping an un-attached
        // Container (its Drop impl calling rte_tel_data_free) doesn't
        // crash either.
        drop(container);
    }

    #[test]
    fn dict_add_array_container_attaches_uint_container_and_transfers_ownership_without_double_free(
    ) {
        // Same shape as
        // `dict_add_array_container_attaches_and_transfers_ownership_without_double_free`
        // above, just with a uint64 container instead of a string one --
        // `add_array_container` itself is generic over `Container`, so
        // this pins that the ownership-handoff path works identically
        // regardless of which `rte_tel_value_type` the container was
        // started with.
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

        // keep=0: DPDK now owns the container's rte_tel_data and frees it
        // itself when the outer dict is freed -- `add_array_container`
        // must `std::mem::forget` its own `Container` handle regardless of
        // which value type it was started with, or freeing `dict` below
        // would double-free/use-after-free.
        dict.add_array_container("epoch_polls", container);

        // Freeing the outer dict must not crash.
        unsafe { dpdk_sys::rte_tel_data_free(dict.raw) };
    }
}
