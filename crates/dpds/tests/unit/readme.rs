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

// ---- spec 22: the controller is stateless, DPDS ships no default config -------------------------
//
// The old update note about an invalid `state.json` (edit the key it names, or move the file away
// and lose the links and presets) is gone with the state file: nothing is stored on disk, a
// controller always starts, and the configuration lives in the browser's localStorage.

#[test]
fn the_readme_no_longer_documents_a_stored_state_or_the_stored_config_routes() {
    let text = readme();
    for gone in [
        "state.json",
        "--state-dir",
        "DPDS_CONTROLLER_STATE_DIR",
        "DPDS_STATE_DIR",
        "/api/links",
        "/api/presets",
        "/api/settings",
        "/api/export",
        "/api/import",
        "pending_restart",
        "Stored state",
    ] {
        assert!(
            !text.contains(gone),
            "the README still mentions `{gone}` (spec 22: no stored state, no stored-config routes)"
        );
    }
    // the old state directory may be named only where `uninstall` is said to clean it up
    for p in paragraphs(&text) {
        if p.contains("/var/lib/dpds-controller") {
            assert!(
                p.to_lowercase().contains("uninstall"),
                "`/var/lib/dpds-controller` is only mentioned as something uninstall removes: {p}"
            );
        }
    }
}

#[test]
fn the_controller_section_says_the_configuration_lives_in_the_browser() {
    let text = readme();
    let controller = controller_section(&text);
    let says = paragraphs(&controller).into_iter().any(|p| {
        let p = p.to_lowercase();
        (p.contains("localstorage") || p.contains("local storage")) && p.contains("browser")
    });
    assert!(
        says,
        "a paragraph of the controller section must say that links, presets and settings are kept \
         in the browser's localStorage (the controller stores nothing)"
    );
    // the start carries the whole configuration
    let flat = controller.to_lowercase();
    assert!(
        flat.contains("/api/emulation/start"),
        "the controller section documents the start route that carries the configuration"
    );
}

#[test]
fn the_readme_ships_and_references_no_default_config_files() {
    let text = readme();
    assert!(
        !text.contains("config.vdev.json"),
        "config.vdev.json was deleted; the examples write their config inline"
    );
    // the repository layout no longer lists a default config
    for l in text.lines() {
        assert!(
            !l.trim_start().starts_with("config.json "),
            "the repository layout still lists config.json: {l}"
        );
    }
    assert!(
        !text.contains("`config.json` in this repository"),
        "there is no config.json in the repository any more"
    );
}

#[test]
fn the_without_hardware_examples_write_their_config_inline() {
    let text = readme();
    let section = text
        .split("### Without hardware")
        .nth(1)
        .expect("a 'Without hardware' section");
    let block = section.split("```").nth(1).expect("a fenced block");
    // a heredoc writes the config to a temporary file that the dpds commands then name
    assert!(
        block.contains("<<") && block.lines().any(|l| l.trim() == "EOF"),
        "the block must write its config with a heredoc: {block}"
    );
    let configs: Vec<String> = block
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| w[0] == "--config")
        .map(|w| w[1].to_string())
        .collect();
    assert!(configs.len() >= 2, "both examples pass --config: {block}");
    for c in configs {
        assert!(
            c.starts_with("/tmp/") || c.starts_with("$"),
            "the examples read a config they wrote to a temporary file, not a repository file: {c}"
        );
        assert!(
            block.contains(&format!("> {c}")) || block.contains(&format!(">{c}")),
            "{c} is written by the block itself"
        );
    }
    // and the config it writes is a complete one: it parses as a dpds-core config
    let body: String = block
        .lines()
        .skip_while(|l| !l.contains("<<"))
        .skip(1)
        .take_while(|l| l.trim() != "EOF")
        .collect::<Vec<_>>()
        .join("\n");
    let v: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("heredoc is not JSON ({e}): {body}"));
    assert_eq!(
        v["buffer_capacity"], 4096,
        "small enough for `-m 256`: {body}"
    );
    assert!(
        v["delayer"]["initialization_rate_pps"].is_number(),
        "initialization_rate_pps is required: {body}"
    );
}

// ---- spec21: the release channel ------------------------------------------------------------------

/// The controller section of the README (the heading line, not the table of contents' link).
fn controller_section(text: &str) -> String {
    text.split("\n## ")
        .find(|s| {
            let heading = s.lines().next().unwrap_or("");
            heading.contains("Controller and web frontend") && !heading.contains("](")
        })
        .expect("controller section")
        .to_string()
}

/// Blank-line separated paragraphs with their line breaks joined to single spaces.
fn paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

#[test]
fn the_readme_documents_dpds_channel_latest_nightly_and_the_flags() {
    let section = controller_section(&readme());
    assert!(
        section.contains("DPDS_CHANNEL"),
        "the controller section never names DPDS_CHANNEL"
    );
    // one paragraph (hard-wrapped lines joined) says what the variable takes, the default and that the flags override it
    let paragraph = paragraphs(&section.to_lowercase())
        .into_iter()
        .find(|p| p.contains("dpds_channel") && p.contains("latest") && p.contains("nightly"))
        .expect("no paragraph explains DPDS_CHANNEL with latest and nightly");
    let paragraph = paragraph.as_str();
    assert!(
        paragraph.contains("default"),
        "it must say that latest is the default: {paragraph}"
    );
    assert!(
        paragraph.contains("--nightly") && paragraph.contains("--stable"),
        "it must name both flags: {paragraph}"
    );
    assert!(
        paragraph.contains("override"),
        "it must say that the flags override the variable: {paragraph}"
    );
    assert!(
        paragraph.contains("`main`") && paragraph.contains("`nightly`"),
        "it must name the branches main and nightly: {paragraph}"
    );
}

#[test]
fn the_readme_warns_that_main_holds_the_c_version_until_nightly_is_merged() {
    let section = controller_section(&readme());
    let paragraph = paragraphs(&section)
        .into_iter()
        .find(|p| p.contains("C version") && p.contains("main"))
        .expect("no paragraph says that main still holds the C version");
    assert!(
        paragraph.contains("--nightly") || paragraph.contains("DPDS_CHANNEL=nightly"),
        "it must tell to use the nightly channel for now: {paragraph}"
    );
}

#[test]
fn the_readme_no_longer_describes_the_tag_selection_or_nightly_as_the_default() {
    let text = readme();
    // (spec25: `git fetch --tags` is back, for the release-tag channel, so it is no longer in this list)
    for gone in [
        "highest `v*` tag",
        "git pull nightly",
        "instead of nightly",
        "`--nightly` is\nthe default",
        "`--nightly` is the default",
    ] {
        assert!(!text.contains(gone), "README still has {gone:?}");
    }
    // the command list shows the channel instead
    let section = controller_section(&text);
    assert!(
        section.contains("DPDS_CHANNEL=nightly ./dpds.sh")
            || section.contains("./dpds.sh --nightly update"),
        "the command block shows no nightly update example"
    );
}

// ---- spec25: DPDS_CHANNEL accepts a release tag -----------------------------------------------------

#[test]
fn the_readme_documents_dpds_channel_with_a_release_tag() {
    let section = controller_section(&readme());
    // the example of the spec, in the command block or in the text
    assert!(
        section.contains("DPDS_CHANNEL=v1.0.0 ./dpds.sh update"),
        "no `DPDS_CHANNEL=v1.0.0 ./dpds.sh update` example in the controller section"
    );
    let lower = section.to_lowercase();
    let tag_paragraphs: Vec<String> = paragraphs(&lower)
        .into_iter()
        .filter(|p| p.contains("v1.0.0") && p.contains("dpds_channel"))
        .collect();
    assert!(
        !tag_paragraphs.is_empty(),
        "no paragraph explains DPDS_CHANNEL with a release tag like v1.0.0"
    );
    // the rest may be spread over the paragraphs of the section that talk about tags
    let all = paragraphs(&lower)
        .into_iter()
        .filter(|p| p.contains("tag"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        all.contains("tag") && all.contains("release"),
        "it must say that the value is a release tag: {all}"
    );
    assert!(
        all.contains("update"),
        "it must say what `update` does with a tag: {all}"
    );
    // staying on a tag keeps that version until the channel changes
    assert!(
        (all.contains("keep") || all.contains("stay") || all.contains("remain"))
            && all.contains("until")
            && all.contains("channel"),
        "it must say that later updates keep that version until the channel changes: {all}"
    );
    // tags exist only once a version has reached main
    assert!(
        all.contains("main") && (all.contains("only") || all.contains("once")),
        "it must say that tags exist only once a version has reached main: {all}"
    );
    // the pattern of a valid tag is stated, so a wrong value is no surprise
    assert!(
        all.contains("vx.y.z") || all.contains("x.y.z") || all.contains("v<"),
        "it must show the form of a tag (vX.Y.Z): {all}"
    );
}

// ---- spec 24: mlockall and a finite RLIMIT_MEMLOCK -------------------------------------------------
//
// With a finite hard limit `dpds` does not call `mlockall` at all (the lock would succeed while the
// process is small and then make EAL's later allocations fail). The README's "Memory locking"
// section documents the four cases.

#[test]
fn the_memory_locking_section_documents_the_four_rlimit_cases() {
    let text = readme();
    let sec = section(&text, "Memory locking");
    let flat = sec.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    // the limit is named, with the shell command that lifts it
    assert!(flat.contains("RLIMIT_MEMLOCK"), "no RLIMIT_MEMLOCK: {flat}");
    assert!(
        flat.contains("ulimit -l unlimited"),
        "no `ulimit -l unlimited`: {flat}"
    );
    // case 4: the warning, with its distinctive wording
    assert!(
        flat.contains("not unlimited: continuing unlocked"),
        "the skip warning (`RLIMIT_MEMLOCK is <n> KiB, not unlimited: continuing unlocked`) is not shown: {flat}"
    );
    assert!(
        flat.contains("KiB"),
        "the warning names the limit in KiB: {flat}"
    );
    // cases 1-3 by their conditions: root, unlimited soft limit, soft raised under an unlimited hard limit
    assert!(lower.contains("root"), "case root is missing: {flat}");
    assert!(
        lower.contains("soft") && lower.contains("hard"),
        "the soft and the hard limit must both be distinguished: {flat}"
    );
    assert!(
        lower.contains("raise"),
        "case 'soft finite, hard unlimited: dpds raises the soft limit' is missing: {flat}"
    );
    // a lock that still fails keeps today's warning
    assert!(
        flat.contains("mlockall failed, continuing unlocked"),
        "a failing mlockall keeps its own warning: {flat}"
    );
    // the old claim is wrong now: a too low limit is no longer 'mlockall failed' in general
    assert!(
        !flat.contains("with too low a `RLIMIT_MEMLOCK` `dpds` prints"),
        "the old sentence says every low limit gives the mlockall warning: {flat}"
    );
    // the unit's setting stays documented
    assert!(
        flat.contains("LimitMEMLOCK=infinity"),
        "LimitMEMLOCK=infinity: {flat}"
    );
}
