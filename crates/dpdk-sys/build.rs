use std::{env, path::PathBuf, process::Command};

fn pkg_config_cflags() -> Vec<String> {
    let out = Command::new("pkg-config")
        .args(["--cflags", "libdpdk"])
        .output()
        .expect("pkg-config not found; is DPDK installed?");
    assert!(out.status.success(), "pkg-config --cflags libdpdk failed");
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .map(String::from)
        .collect()
}

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");

    // Regenerate bindings when DPDK itself is updated: every `ninja install`
    // rewrites libdpdk.pc (it contains the version), so watching it makes a
    // plain `cargo build` pick up a new DPDK without needing `cargo clean`.
    if let Ok(out) = Command::new("pkg-config")
        .args(["--variable=pcfiledir", "libdpdk"])
        .output()
    {
        if out.status.success() {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !dir.is_empty() {
                println!("cargo:rerun-if-changed={dir}/libdpdk.pc");
            }
        }
    }

    // Emit link directives for all DPDK shared libraries.
    let lib = pkg_config::Config::new()
        .atleast_version("26.07")
        .probe("libdpdk")
        .expect("libdpdk not found via pkg-config; is DPDK installed?");

    let cflags = pkg_config_cflags();

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Generate Rust bindings.  wrap_static_fns(true) makes bindgen emit an
    // additional `extern.c` file (in OUT_DIR) containing non-inline wrappers
    // for every static-inline function, so the linker can find a real symbol.
    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .wrap_static_fns(true)
        .wrap_static_fns_path(out.join("extern.c"))
        // Include everything defined in DPDK's own headers (rte_*.h) but
        // nothing from transitively-included system headers.  This means we
        // never need to touch build.rs when adding new DPDK calls.
        .allowlist_file(".*rte_.*\\.h")
        // Also pick up the helpers defined in wrapper.h itself
        // (e.g. rte_errno_value).
        .allowlist_file(".*wrapper\\.h")
        // These types have C packed+aligned combinations that Rust's type
        // system rejects (E0588) or anonymous unions without Debug (E0277).
        // Marking them opaque (byte-array representation) is the standard fix.
        .opaque_type("rte_arp_ipv4")
        .opaque_type("rte_arp_hdr")
        .opaque_type("rte_l2tpv2_combined_msg_hdr")
        .opaque_type("rte_gtp_psc_generic_hdr")
        // RTM (TSX) helpers and the lock-elision variants built on them.
        // Their wrappers would need -mrtm, which the wrapper compile below
        // drops (see there); nothing in this workspace calls them.
        .blocklist_function("rte_x(begin|end|test)|rte_try_tm|rte_.*_tm")
        // Carry the doxygen comments from DPDK's headers into bindings.rs.
        // generate_comments is on by default, but libclang only surfaces
        // recognized documentation comments unless we ask it to parse all of
        // them via -fparse-all-comments below.
        .generate_comments(true)
        .clang_arg("-fparse-all-comments")
        .clang_arg("-fretain-comments-from-system-headers")
        .derive_default(true);

    for flag in &cflags {
        builder = builder.clang_arg(flag);
    }

    let bindings = builder
        .generate()
        .expect("Unable to generate DPDK bindings");

    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("Couldn't write bindings.rs");

    // Compile the extern.c that bindgen just wrote.  It contains the actual
    // non-inline definitions that the Rust linker resolves against.
    // The generated file opens with `#include "wrapper.h"`, so we must add
    // the crate source directory to the include search path.
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut cc_build = cc::Build::new();
    cc_build.file(out.join("extern.c"));
    cc_build.include(&manifest_dir);
    // Cross-language LTO: when clang is available, compile the wrappers to
    // LLVM bitcode so rust-lld can inline rte_eth_rx/tx_burst into the Rust
    // forwarding loop (see the repository README.md).  clang's LLVM major must match
    // rustc's — with an older clang, lld silently uses the machine-code
    // half of the fat objects and nothing is inlined.  -ffat-lto-objects
    // keeps machine code alongside the bitcode so the archive links with
    // or without linker-plugin-lto.
    // Setting CC (or lacking clang) falls back to the system compiler with
    // no LTO objects (gcc's -flto GIMPLE is useless to lld) — the build
    // still works, just without the cross-language inlining.
    let have_clang = Command::new("clang")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if env::var_os("CC").is_none() && have_clang {
        cc_build.compiler("clang");
        // Must be thin LTO: rustc's -Clinker-plugin-lto emits bitcode with
        // ThinLTO summaries, and lld never inlines between its ThinLTO and
        // regular-LTO partitions — plain -flto (full) bitcode would land in
        // the regular partition and none of the wrappers would be inlined.
        cc_build.flag("-flto=thin");
        cc_build.flag_if_supported("-ffat-lto-objects");
    }
    for path in &lib.include_paths {
        cc_build.include(path);
    }
    // DPDK's x86 build adds -mrtm unconditionally (config/x86/meson.build),
    // even on CPUs without TSX (e.g. AMD EPYC). A wrapper
    // compiled with rtm has a target feature its -Ctarget-cpu=native Rust
    // callers lack, so LLVM refuses to inline it ("conflicting target
    // features") and every burst becomes an out-of-line call. The
    // -march=native added below already enables RTM where the CPU has it.
    for flag in &cflags {
        if !flag.starts_with("-I") && flag != "-mrtm" {
            cc_build.flag(flag);
        }
    }
    // wrap_static_fns wraps every allowlisted static-inline function, some of
    // which DPDK marks experimental; the deprecation warnings are noise here.
    cc_build.flag_if_supported("-Wno-deprecated-declarations");
    // bindgen occasionally emits `const const` in wrapper signatures.
    cc_build.flag_if_supported("-Wno-duplicate-decl-specifier");
    // The extern.c wrappers contain the rx/tx burst fast path.  DPDK itself
    // is built for the host CPU by default; compile the wrappers the same way
    // so the burst code keeps its SIMD level.  x86 spells it -march=native;
    // clang on AArch64 rejects that and wants -mcpu=native (on x86 -mcpu is
    // only a deprecated alias, so prefer -march and fall back).
    if cc_build.is_flag_supported("-march=native").unwrap_or(false) {
        cc_build.flag("-march=native");
    } else {
        cc_build.flag_if_supported("-mcpu=native");
    }
    cc_build.compile("dpdk_static_fns");
}
