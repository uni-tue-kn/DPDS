//! Checks on the files themselves:
//! crate metadata, LICENSE, README content, crate docs, the bench target and the
//! `#[inline]` attributes. Cheap text checks, deliberately lenient about
//! wording; they only pin the essentials.

use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: impl AsRef<Path>) -> String {
    let p = path.as_ref();
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

/// `key = value` line inside the `[package]` table (naive but enough for these
/// manifests).
fn package_field(manifest: &str, key: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

#[test]
fn cargo_toml_has_the_publishing_metadata() {
    let m = read(crate_dir().join("Cargo.toml"));
    assert_eq!(
        package_field(&m, "repository").as_deref(),
        Some("\"https://github.com/uni-tue-kn/DPDS\"")
    );
    assert!(package_field(&m, "keywords").is_some(), "keywords");
    assert!(package_field(&m, "categories").is_some(), "categories");
    assert!(package_field(&m, "include").is_some(), "include list");
    let msrv = package_field(&m, "rust-version").expect("rust-version");
    let digits = msrv.trim_matches('"');
    assert!(
        digits.split('.').count() >= 2 && digits.split('.').all(|p| p.parse::<u32>().is_ok()),
        "rust-version {msrv} must look like 1.NN[.P]"
    );
}

#[test]
fn cargo_toml_declares_the_features_and_the_bench_target() {
    let m = read(crate_dir().join("Cargo.toml"));
    assert!(m.contains("[features]"));
    let compact: String = m.split_whitespace().collect();
    assert!(
        compact.contains("default=[\"json\"]"),
        "default = [\"json\"]"
    );
    assert!(compact.contains("serde=[\"dep:serde\"]"));
    assert!(compact.contains("json=[\"serde\",\"dep:serde_json\"]"));
    assert!(m.contains("[[bench]]"));
    assert!(compact.contains("name=\"hot_path\""));
    assert!(compact.contains("harness=false"));
    assert!(
        crate_dir().join("benches/hot_path.rs").is_file(),
        "benches/hot_path.rs"
    );
}

#[test]
fn license_file_is_a_copy_of_the_root_apache_license() {
    let ours = read(crate_dir().join("LICENSE"));
    let root = read(crate_dir().join("../../LICENSE"));
    assert_eq!(ours, root);
    assert!(ours.contains("Apache License"));
}

#[test]
fn other_workspace_crates_are_not_publishable() {
    for krate in ["dpds", "dpdk", "dpdk-sys"] {
        let m = read(crate_dir().join("..").join(krate).join("Cargo.toml"));
        assert_eq!(
            package_field(&m, "publish").as_deref(),
            Some("false"),
            "{krate}"
        );
    }
}

#[test]
fn readme_documents_the_new_api_features_seeding_ordering_and_git_usage() {
    let r = read(crate_dir().join("README.md"));
    for needle in [
        "pop_due",
        "peek_nth",
        "try_new",
        "seed",
        "json",
        "--no-default-features",
        "1/64",
        "dpds-core = { git = \"https://github.com/uni-tue-kn/DPDS\", branch = \"nightly\" }",
    ] {
        assert!(r.contains(needle), "README.md should mention `{needle}`");
    }
}

#[test]
fn crate_level_docs_cover_the_loop_ownership_seeding_and_ordering() {
    let lib = read(crate_dir().join("src/lib.rs"));
    let docs: String = lib
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("//!"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!docs.is_empty(), "lib.rs needs `//!` crate docs");
    for needle in ["push", "pop_due", "c_void", "Send", "seed", "1/64"] {
        assert!(
            docs.contains(needle),
            "crate docs should mention `{needle}`"
        );
    }
    assert!(docs.contains("```"), "crate docs need an example (doctest)");
}

#[test]
fn buffer_docs_state_the_tie_rule() {
    let buf = read(crate_dir().join("src/buffer.rs"));
    assert!(
        buf.contains("1/64"),
        "buffer.rs doc comment should explain the 1/64 ns tie offset"
    );
}

/// `true` if the `fn name(` definition is directly preceded (through other
/// attributes and doc comments) by an `#[inline...]` attribute.
fn has_inline(src: &str, name: &str) -> bool {
    let lines: Vec<&str> = src.lines().collect();
    let needle = format!("fn {name}(");
    let Some(idx) = lines
        .iter()
        .position(|l| l.contains(&needle) && !l.trim_start().starts_with("//"))
    else {
        panic!("fn {name} not found");
    };
    lines[..idx]
        .iter()
        .rev()
        .take_while(|l| {
            let t = l.trim_start();
            t.starts_with("#[") || t.starts_with("///")
        })
        .any(|l| l.trim_start().starts_with("#[inline"))
}

#[test]
fn hot_path_methods_are_inline_and_push_is_not() {
    let src = crate_dir().join("src");
    let buffer = read(src.join("buffer.rs"));
    for f in [
        "pop",
        "pop_due",
        "front_time",
        "len",
        "is_empty",
        "capacity",
        "peek_nth",
    ] {
        assert!(has_inline(&buffer, f), "Buffer::{f} needs #[inline]");
    }
    assert!(
        !has_inline(&buffer, "push"),
        "Buffer::push must not be #[inline]"
    );
    assert!(has_inline(&read(src.join("dropper.rs")), "dropper_drop"));
    assert!(has_inline(
        &read(src.join("limiter.rs")),
        "limiter_calculate_offset_ns"
    ));
}

// ---- initialization_rate_pps: required config field, no built-in rate ---------

#[test]
fn readme_documents_the_required_initialization_rate_and_has_no_15_mpps_preset() {
    let r = read(crate_dir().join("README.md"));
    assert!(
        r.contains("| `initialization_rate_pps`"),
        "README.md needs a row for `initialization_rate_pps` in the config table"
    );
    let json_examples = r.matches("\"initialization_rate_pps\"").count();
    assert!(
        json_examples >= 1,
        "the README config example must carry `initialization_rate_pps`"
    );
    assert!(
        !r.contains("15 Mpps"),
        "the delay correlation section must not describe a built-in 15 Mpps rate"
    );
}

#[test]
fn crate_docs_note_the_api_change_for_the_initialization_rate() {
    let lib = read(crate_dir().join("src/lib.rs"));
    let docs: String = lib
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("//!"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        docs.contains("initialization_rate_pps"),
        "the crate docs need the `initialization_rate_pps` API-change note"
    );
}

#[test]
fn no_fallback_initialization_rate_constant_is_left_in_the_buffer() {
    let src = read(crate_dir().join("src/buffer.rs"));
    for needle in ["15_000_000", "15e6", "15_000_000.", "Mpps"] {
        assert!(
            !src.contains(needle),
            "src/buffer.rs must not hard-code a rate (`{needle}`)"
        );
    }
}

// ── spec20: the limiter limit is derived, `limit_bit` is gone ────────

fn crate_docs() -> String {
    let lib = read(crate_dir().join("src/lib.rs"));
    lib.lines()
        .filter_map(|l| l.trim_start().strip_prefix("//!"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn max_frame_bytes_is_a_public_constant_of_1542() {
    let src = ["lib.rs", "buffer.rs", "limiter.rs"]
        .iter()
        .map(|f| read(crate_dir().join("src").join(f)))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains("pub const MAX_FRAME_WIRE_BYTES: u32 = 1542;"),
        "src needs `pub const MAX_FRAME_WIRE_BYTES: u32 = 1542;`"
    );
    assert!(
        !src.contains("MAX_FRAME_BYTES"),
        "the first-draft name MAX_FRAME_BYTES (1522) must be gone"
    );
}

#[test]
fn readme_documents_the_derived_limit_the_order_and_the_api_change() {
    let r = read(crate_dir().join("README.md"));
    assert!(
        !r.contains("| `limit_bit`"),
        "README.md must not list `limit_bit` as a config field any more"
    );
    assert!(
        !r.contains("\"limit_bit\":"),
        "README.md config examples must not carry `limit_bit`"
    );
    for needle in [
        "1542",
        "MAX_FRAME_WIRE_BYTES",
        "buffer_capacity",
        "API change",
    ] {
        assert!(r.contains(needle), "README.md should mention `{needle}`");
    }
    // the removal is announced as an API change next to the field's name
    assert!(
        r.lines()
            .any(|l| l.contains("limit_bit") && !l.trim_start().starts_with('|')),
        "README.md should name the removed `limit_bit` in its API-change note"
    );
}

#[test]
fn crate_docs_note_the_removed_limit_bit_the_derived_limit_and_the_order() {
    let docs = crate_docs();
    for needle in [
        "limit_bit",
        "MAX_FRAME_WIRE_BYTES",
        "1542",
        "API change",
        "BufferFull",
    ] {
        assert!(
            docs.contains(needle),
            "crate docs should mention `{needle}`"
        );
    }
    assert!(
        !docs.contains("limit_bit: 1e6"),
        "the crate-doc example must not set `limit_bit`"
    );
}

#[test]
fn push_outcome_docs_state_that_the_capacity_check_comes_first() {
    let buf = read(crate_dir().join("src/buffer.rs"));
    assert!(
        !buf.contains("then the capacity is checked"),
        "the PushOutcome docs still describe the old order (capacity last)"
    );
    assert!(
        !buf.contains("The packet was accepted by loss model and limiter, but the buffer"),
        "BufferFull docs still describe the old order"
    );
}

#[test]
fn no_workspace_doc_or_config_example_carries_a_limit_bit_json_key_or_table_row() {
    let root = crate_dir().join("../..");
    for rel in [
        "README.md",
        "docs/TELEMETRY.md",
        "crates/dpds-core/README.md",
    ] {
        let text = read(root.join(rel));
        assert!(
            !text.contains("\"limit_bit\""),
            "{rel}: a JSON example still has `limit_bit`"
        );
        assert!(
            !text.contains("| `limit_bit`") && !text.contains("| `limiter.limit_bit`"),
            "{rel}: a field table still lists `limit_bit`"
        );
        assert!(
            !text.contains("limit_bit / rate_bps"),
            "{rel}: a formula still uses `limit_bit`"
        );
    }
    // the telemetry reference has no reason to mention the removed field at all
    let t = read(root.join("docs/TELEMETRY.md"));
    assert!(
        !t.contains("limit_bit"),
        "docs/TELEMETRY.md still mentions `limit_bit`"
    );
}

#[test]
fn the_root_readme_documents_the_derived_limit_and_the_order_of_operations() {
    let r = read(crate_dir().join("../../README.md"));
    assert!(
        r.contains("1542"),
        "README.md should state the 1542 B frame size of the derived limit"
    );
    assert!(
        r.contains("buffer_capacity") && r.contains("limit"),
        "README.md should explain that the limit comes from the buffer capacity"
    );
}
