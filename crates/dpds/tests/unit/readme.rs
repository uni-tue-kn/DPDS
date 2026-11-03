//! README pins of spec v17 (PREOF's EAL setup, memory locking, no devargs advice). The README is
//! documentation, but its commands are run by the e2e tests and its advice is part of the contract:
//! hand-run examples use `--in-memory` like PREOF, the memory section documents `mlockall` and the
//! unit's `LimitMEMLOCK=infinity`, and nothing proposes mlx5 devargs other than the fixed
//! `txq_mpw_en=1`.

use std::fs;
use std::path::Path;

fn readme() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"))
        .expect("README.md")
}

/// The README's lines with backslash continuations joined.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        match line.trim_end().strip_suffix('\\') {
            Some(head) => {
                cur.push_str(head);
                cur.push(' ');
            }
            None => {
                cur.push_str(line);
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    out
}

/// Every shell command of the README that starts `dpds` (with or without `sudo`).
fn dpds_invocations(text: &str) -> Vec<String> {
    logical_lines(text)
        .into_iter()
        .map(|l| l.trim().trim_start_matches("sudo ").trim().to_string())
        .filter(|l| l.starts_with("./target/release/dpds "))
        .collect()
}

#[test]
fn every_dpds_command_of_the_readme_runs_with_in_memory_like_preof() {
    let text = readme();
    let commands = dpds_invocations(&text);
    assert!(commands.len() >= 4, "found only {commands:?}");
    for c in commands {
        let eal = c.split(" -- ").next().unwrap();
        assert!(
            eal.split_whitespace().any(|a| a == "--in-memory"),
            "README command without --in-memory: {c}"
        );
    }
}

#[test]
fn the_readme_documents_the_memory_locking_and_the_unit_limit() {
    let text = readme();
    assert!(
        text.contains("mlockall"),
        "README does not mention mlockall"
    );
    assert!(
        text.contains("MCL_CURRENT") && text.contains("MCL_FUTURE"),
        "README does not name the mlockall flags"
    );
    assert!(
        text.contains("LimitMEMLOCK=infinity"),
        "README does not mention LimitMEMLOCK=infinity"
    );
}

#[test]
fn the_readme_states_the_fixed_devargs_and_proposes_no_other() {
    let text = readme();
    assert!(
        text.contains("txq_mpw_en=1"),
        "README does not name the fixed mlx5 devargs"
    );
    for gone in [
        "mlx5_devargs",
        "txq_inline",
        "rxq_cqe_comp_en",
        "rxq_pkt_pad_en",
        "rx_vec_en",
        "tx_pp",
        "Do not enable MPRQ",
        "Do not set `mprq_en`",
        "`mprq` is never set",
        "mlx5 devargs come from a fixed list",
    ] {
        assert!(!text.contains(gone), "README still has {gone:?}");
    }
}
