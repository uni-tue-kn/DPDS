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

// ---- spec19 items 4 and 5 ------------------------------------------------------------------------

/// The text of the `### <heading>` section, up to the next `### ` or `## ` heading.
fn section(text: &str, heading: &str) -> String {
    let start = text
        .find(&format!("### {heading}"))
        .unwrap_or_else(|| panic!("README has no section {heading:?}"));
    let rest = &text[start + 4..];
    let end = rest
        .find("\n### ")
        .into_iter()
        .chain(rest.find("\n## "))
        .min()
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn the_hand_run_example_says_that_txq_mpw_en_is_for_mlx5_ports_only() {
    // `txq_mpw_en` is an mlx5 devarg: on another driver (ice, i40e, ...) EAL refuses or ignores it, so the example's
    // `<PCI0>,txq_mpw_en=1` must say next to itself that it applies to mlx5 ports only.
    let text = readme();
    let real_nics = section(&text, "Real NICs");
    assert!(
        real_nics.contains("txq_mpw_en=1"),
        "the hand-run example no longer shows the devarg"
    );
    let flat = real_nics
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let says_mlx5_only = flat.match_indices("mlx5").any(|(i, _)| {
        let from = flat[..i].char_indices().rev().nth(39).map_or(0, |(b, _)| b);
        let to = flat[i..]
            .char_indices()
            .nth(40)
            .map_or(flat.len(), |(b, _)| i + b);
        flat[from..to].contains("only")
    });
    assert!(
        says_mlx5_only,
        "the Real NICs example must say that txq_mpw_en=1 is for mlx5 ports only (\"mlx5 only\" or \"only on mlx5 ports\"): {real_nics}"
    );
}

#[test]
fn the_update_notes_explain_an_invalid_state_file_and_how_to_fix_it() {
    // A format change can make an old `state.json` unusable (the controller then refuses to start). The README says so in
    // one paragraph of the controller section, with both remedies: edit the file to remove the key it names, or move it
    // away and start empty (which loses the links and presets).
    let text = readme();
    let controller = text
        .split("\n## ")
        // the section whose heading line is the title; the table of contents links to it as `[..](#..)`
        .find(|s| {
            let heading = s.lines().next().unwrap_or("");
            heading.contains("Controller and web frontend") && !heading.contains("](")
        })
        .expect("controller section");
    let paragraph = controller
        .split("\n\n")
        .map(str::to_lowercase)
        .find(|p| p.contains("state.json") && p.contains("invalid"))
        .unwrap_or_else(|| {
            panic!("no paragraph of the controller section mentions state.json becoming invalid")
        });
    assert!(
        paragraph.contains("update"),
        "the paragraph belongs to the update notes: {paragraph}"
    );
    assert!(
        paragraph.contains("remove"),
        "it says to remove the offending key: {paragraph}"
    );
    assert!(
        paragraph.contains("move"),
        "it says to move the file away: {paragraph}"
    );
    assert!(
        paragraph.contains("links") && paragraph.contains("presets"),
        "it says what moving the file away loses: {paragraph}"
    );
}
