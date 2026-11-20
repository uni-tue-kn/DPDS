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

    // Every `ninja install` rewrites libdpdk.pc (it contains the version), so
    // watching it makes a plain `cargo build` pick up a new DPDK.
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

    let lib = pkg_config::Config::new()
        .atleast_version("26.07")
        .probe("libdpdk")
        .expect("libdpdk not found via pkg-config; is DPDK installed?");

    let cflags = pkg_config_cflags();

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // wrap_static_fns(true) makes bindgen emit `extern.c` (in OUT_DIR) with
    // non-inline wrappers for every static-inline function, so the linker finds
    // a real symbol.
    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .wrap_static_fns(true)
        .wrap_static_fns_path(out.join("extern.c"))
        // Everything from DPDK's own headers (rte_*.h), nothing from system headers.
        .allowlist_file(".*rte_.*\\.h")
        // Also the helpers defined in wrapper.h (e.g. rte_errno_value).
        .allowlist_file(".*wrapper\\.h")
        // These types have packed+aligned combinations Rust rejects (E0588) or
        // anonymous unions without Debug (E0277); opaque (byte array) fixes both.
        .opaque_type("rte_arp_ipv4")
        .opaque_type("rte_arp_hdr")
        .opaque_type("rte_l2tpv2_combined_msg_hdr")
        .opaque_type("rte_gtp_psc_generic_hdr")
        // RTM (TSX) helpers and the lock-elision variants built on them: their
        // wrappers would need -mrtm, which the wrapper compile below drops.
        .blocklist_function("rte_x(begin|end|test)|rte_try_tm|rte_.*_tm")
        // Carry DPDK's doxygen comments into bindings.rs; libclang only surfaces
        // recognized documentation comments unless -fparse-all-comments is set.
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

    // Compile the `extern.c` bindgen wrote. It opens with `#include "wrapper.h"`,
    // so the crate directory goes on the include path.
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut cc_build = cc::Build::new();
    cc_build.file(out.join("extern.c"));
    cc_build.include(&manifest_dir);
    // Cross-language LTO: with clang, compile the wrappers to LLVM bitcode so
    // rust-lld can inline rte_eth_rx/tx_burst into the forwarding loop. clang's
    // LLVM major must match rustc's: with an older clang, lld silently uses the
    // machine-code half of the fat objects and nothing is inlined.
    // -ffat-lto-objects keeps machine code alongside the bitcode so the archive
    // links with or without linker-plugin-lto. Setting CC (or lacking clang)
    // falls back to the system compiler without LTO objects (gcc's -flto GIMPLE
    // is useless to lld): it builds, just without the inlining.
    let have_clang = Command::new("clang")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if env::var_os("CC").is_none() && have_clang {
        cc_build.compiler("clang");
        // Must be thin LTO: rustc's -Clinker-plugin-lto emits ThinLTO bitcode, and
        // lld never inlines between its ThinLTO and regular-LTO partitions, so
        // full -flto bitcode would leave every wrapper out of line.
        cc_build.flag("-flto=thin");
        cc_build.flag_if_supported("-ffat-lto-objects");
    }
    for path in &lib.include_paths {
        cc_build.include(path);
    }
    // DPDK's x86 build adds -mrtm unconditionally (config/x86/meson.build), even
    // on CPUs without TSX (e.g. AMD EPYC). A wrapper compiled with rtm has a
    // target feature its -Ctarget-cpu=native Rust callers lack, so LLVM refuses
    // to inline it ("conflicting target features") and every burst becomes an
    // out-of-line call. The -march=native below enables RTM where the CPU has it.
    for flag in &cflags {
        if !flag.starts_with("-I") && flag != "-mrtm" {
            cc_build.flag(flag);
        }
    }
    // Some wrapped static-inline functions are marked experimental; the warnings are noise.
    cc_build.flag_if_supported("-Wno-deprecated-declarations");
    // bindgen occasionally emits `const const` in wrapper signatures.
    cc_build.flag_if_supported("-Wno-duplicate-decl-specifier");
    // The wrappers contain the rx/tx burst fast path. DPDK is built for the host
    // CPU, so compile them the same way to keep the SIMD level: -march=native on
    // x86, -mcpu=native on AArch64 clang (which rejects -march=native).
    if cc_build.is_flag_supported("-march=native").unwrap_or(false) {
        cc_build.flag("-march=native");
    } else {
        cc_build.flag_if_supported("-mcpu=native");
    }
    cc_build.compile("dpdk_static_fns");
}
