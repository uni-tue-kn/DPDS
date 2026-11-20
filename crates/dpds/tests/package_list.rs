//! The published `dpds-core` crate must not ship `tests/metadata.rs`: it
//! reads files outside the crate (the sibling crates' manifests), so it cannot
//! pass from the unpacked package. The test stays in the repository; only the
//! crate's `include` list must leave it out.
//!
//! Lives in the `dpds` crate (not published) so that this test itself is not
//! part of the `dpds-core` package it inspects. `cargo package --list` is
//! offline and does not build anything.

use std::path::PathBuf;
use std::process::Command;

fn package_list() -> Vec<String> {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new(cargo)
        .args([
            "package",
            "-p",
            "dpds-core",
            "--allow-dirty",
            "--offline",
            "--list",
        ])
        .current_dir(workspace)
        .output()
        .expect("failed to run cargo package --list");
    assert!(
        out.status.success(),
        "cargo package --list failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("package list is UTF-8")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn the_dpds_core_package_does_not_ship_tests_metadata_rs() {
    let list = package_list();
    assert!(
        !list.iter().any(|f| f == "tests/metadata.rs"),
        "tests/metadata.rs reads workspace files and must not be packaged:\n{}",
        list.join("\n")
    );
}

#[test]
fn the_dpds_core_package_still_ships_sources_docs_and_the_other_tests() {
    let list = package_list();
    for needed in [
        "Cargo.toml",
        "README.md",
        "LICENSE",
        "src/lib.rs",
        "src/buffer.rs",
        "benches/hot_path.rs",
        "config.example.json",
        "tests/buffer.rs",
        "tests/try_new.rs",
    ] {
        assert!(
            list.iter().any(|f| f == needed),
            "{needed} missing from the package:\n{}",
            list.join("\n")
        );
    }
}
