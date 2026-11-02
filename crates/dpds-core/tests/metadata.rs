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
