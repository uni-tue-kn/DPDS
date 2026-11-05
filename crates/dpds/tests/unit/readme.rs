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

// ---- spec 30: docs/TELEMETRY.md describes the paged histogram ----------------------------------

fn telemetry_doc() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/TELEMETRY.md"))
        .expect("docs/TELEMETRY.md")
}

/// The heading of the paged histogram section (spec 30b: it names the optional page).
const DELAY_HIST_HEADING: &str = "## `/dpds/delay_hist,<dir>[,<page>]`";

/// The lines of the `## /dpds/delay_hist,<dir>[,<page>]` section (the heading line itself picks
/// it, not a substring: the table of contents and the command table mention the command too).
fn delay_hist_section(text: &str) -> String {
    let mut lines = text
        .lines()
        .skip_while(|l| l.trim_end() != DELAY_HIST_HEADING);
    assert!(lines.next().is_some(), "the delay_hist section heading");
    lines
        .take_while(|l| !l.starts_with("## "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_telemetry_doc_describes_paging_and_the_4096_cap() {
    let section = delay_hist_section(&telemetry_doc());
    for needle in [
        "4096",
        "/dpds/delay_hist,<dir>,<page>",
        "`page`",
        "`first_bin`",
        "500",
    ] {
        assert!(
            section.contains(needle),
            "the section names {needle:?}:\n{section}"
        );
    }
    let lower = section.to_lowercase();
    assert!(
        lower.contains("page 0"),
        "`,<dir>` alone is page 0:\n{section}"
    );
}

#[test]
fn the_telemetry_doc_heading_names_the_optional_page() {
    let doc = telemetry_doc();
    assert!(
        doc.lines().any(|l| l.trim_end() == DELAY_HIST_HEADING),
        "a heading line {DELAY_HIST_HEADING:?}"
    );
    assert!(
        !doc.lines()
            .any(|l| l.trim_end() == "## `/dpds/delay_hist,<dir>`"),
        "the old heading without `[,<page>]` is gone"
    );
}

#[test]
fn the_telemetry_doc_says_a_bad_page_is_an_empty_reply_not_null() {
    // spec 30b item 5: the real `dpds` answers a page past the end, a malformed page and a bad
    // `<dir>` with an empty dict
    let section = delay_hist_section(&telemetry_doc());
    let flat = section.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("empty reply (`{}`)"),
        "a bad page is an empty reply (`{{}}`):\n{section}"
    );
    assert!(
        !flat.contains("`null`") && !flat.contains("error reply"),
        "a bad page is not a `null` or error reply:\n{section}"
    );
}

#[test]
fn the_telemetry_doc_no_longer_gives_500_as_the_bins_limit() {
    let doc = telemetry_doc();
    for stale in ["1..=500", "the reply holds up to 500 counts"] {
        assert!(
            !doc.contains(stale),
            "stale text {stale:?} in docs/TELEMETRY.md"
        );
    }
    // the command table lists the page parameter too
    let row = doc
        .lines()
        .find(|l| l.starts_with("| `/dpds/delay_hist,<dir>"))
        .expect("the command table row");
    assert!(row.contains("<page>"), "{row}");
}

// ---- spec 29b item 3: the controller section documents the stats and history routes ------------

/// The top-level list items of the controller section that mention `needle`, each with its line
/// breaks and indentation joined to single spaces.
fn controller_items_with(needle: &str) -> Vec<String> {
    let section = controller_section(&readme());
    let mut items: Vec<String> = Vec::new();
    for line in section.lines() {
        if line.starts_with("- ") || items.is_empty() {
            items.push(String::new());
        }
        let last = items.last_mut().unwrap();
        last.push_str(line);
        last.push(' ');
    }
    items
        .into_iter()
        .map(|i| i.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|i| i.contains(needle))
        .collect()
}

/// The list items that document `route` (spelled `GET <path>` in backticks), joined; at least one.
fn route_item(route: &str) -> String {
    let items = controller_items_with(route);
    assert!(
        !items.is_empty(),
        "no item of the controller section documents {route}"
    );
    items.join(" ")
}

#[test]
fn the_readme_documents_get_api_stats_with_elapsed_ms() {
    let item = route_item("`GET /api/stats`");
    assert!(
        item.contains("`elapsed_ms`"),
        "`GET /api/stats` documents `elapsed_ms`: {item}"
    );
    // what elapsed_ms is: time since the run started, frozen at the stop, absent before any run
    let lower = item.to_lowercase();
    assert!(
        lower.contains("since the run started")
            || lower.contains("since the run's start")
            || lower.contains("since the start of the run"),
        "elapsed_ms counts from the start of the run: {item}"
    );
    assert!(
        lower.contains("monotonic"),
        "from a monotonic clock: {item}"
    );
    assert!(
        lower.contains("frozen") || lower.contains("stops counting"),
        "elapsed_ms stops at the stop: {item}"
    );
}

#[test]
fn the_readme_documents_get_api_stats_history() {
    let item = route_item("`GET /api/stats/history`");
    let lower = item.to_lowercase();
    // the query: since_ms, digits only, anything else is a 400
    assert!(item.contains("`since_ms`"), "{item}");
    assert!(
        lower.contains("digits only"),
        "`since_ms` takes digits only: {item}"
    );
    assert!(item.contains("400"), "a bad `since_ms` is a 400: {item}");
    // what the series holds
    assert!(
        lower.contains("one sample per link per second"),
        "one sample per link per second: {item}"
    );
    assert!(
        item.contains("86400"),
        "the cap of 86400 samples per link: {item}"
    );
    assert!(
        lower.contains("monotonic"),
        "times come from the controller's monotonic clock: {item}"
    );
    // its life: kept after the stop, cleared at the next start, lost with the controller
    assert!(
        lower.contains("after the stop") || lower.contains("after a stop"),
        "kept after the stop: {item}"
    );
    assert!(
        lower.contains("next start"),
        "cleared at the next start: {item}"
    );
    assert!(
        lower.contains("lost") && lower.contains("controller restart"),
        "lost when the controller restarts: {item}"
    );
    // the identity fields of the answer (spec 29b item 1)
    for field in ["`run_id`", "`links`", "`t_ms`"] {
        assert!(item.contains(field), "the answer's {field}: {item}");
    }
}

// ---- spec 35: `totals` of the history and the receiving port's counters of `/api/stats` ----------

const TOTALS_FIELDS: [&str; 7] = [
    "rx_pkts",
    "loss_drops",
    "limiter_drops",
    "buffer_full_drops",
    "nic_tx_drops",
    "nic_rx_imissed",
    "nic_rx_nombuf",
];

/// The item that documents the stats route (not the history route, which also names
/// `GET /api/stats` when it says where its numbers come from), lower-cased copy second.
fn stats_route_item() -> String {
    let items: Vec<String> = controller_items_with("`GET /api/stats`")
        .into_iter()
        .filter(|i| !i.contains("`GET /api/stats/history`"))
        .collect();
    assert!(
        !items.is_empty(),
        "no item of the controller section documents `GET /api/stats` on its own"
    );
    items.join(" ")
}

#[test]
fn the_readme_documents_totals_on_the_history() {
    let item = route_item("`GET /api/stats/history`");
    let lower = item.to_lowercase();
    assert!(item.contains("`totals`"), "the answer's `totals`: {item}");
    for field in TOTALS_FIELDS {
        assert!(
            item.contains(&format!("`{field}`")),
            "totals has `{field}`: {item}"
        );
    }
    // what they are: cumulative counters of the newest poll, per link, kept like the samples
    assert!(
        lower.contains("cumulative"),
        "totals are cumulative counters: {item}"
    );
    assert!(
        lower.contains("last poll")
            || lower.contains("newest poll")
            || lower.contains("latest poll"),
        "totals come from the last poll of the run: {item}"
    );
    // `totals` is an object per link index that is empty before any run
    assert!(
        lower.contains("`{}`") || lower.contains("empty object"),
        "totals is `{{}}` before any run: {item}"
    );
}

#[test]
fn the_readme_documents_the_port_counters_of_a_direction_on_the_stats() {
    let item = stats_route_item();
    let lower = item.to_lowercase();
    for field in ["`nic_rx_imissed`", "`nic_rx_nombuf`"] {
        assert!(
            item.contains(field),
            "`GET /api/stats` documents {field}: {item}"
        );
    }
    assert!(
        lower.contains("receiving port"),
        "they are the receiving port's counters: {item}"
    );
    assert!(
        lower.contains("cumulative"),
        "they are cumulative counters: {item}"
    );
    // links that receive on one port report the same numbers
    assert!(
        lower.contains("share") || lower.contains("hairpin"),
        "two links on one receiving port both report its counters: {item}"
    );
}

// ---- spec 33: DPDS never changes the host; hints show the commands for an administrator --------
//
// Spec 32's Apply (route, button, `action`) is gone. The controller section says that neither
// `dpds.sh`, the controller nor the frontend changes the host, that a hint can carry `commands`
// which an administrator runs as root, what `dpds-controller check` prints under a hint, and the
// Security item keeps saying what reaching the port allows: starting and stopping `dpds`.

fn items_with_all(needles: &[&str]) -> Vec<String> {
    let mut items = controller_items_with(needles[0]);
    items.retain(|i| needles.iter().all(|n| i.contains(n)));
    items
}

#[test]
fn the_readme_has_no_apply_route_button_or_action_left() {
    let text = readme();
    for gone in [
        "/api/host/actions",
        "Host action route",
        "Host actions**",
        "Apply in the frontend",
        "Apply button",
        "carries an `action`",
        "allow-listed",
        "never restores an old value",
        "Tuning is never applied",
        "they only report hints and suggestions",
    ] {
        assert!(!text.contains(gone), "README still says {gone:?}");
    }
}

#[test]
fn the_readme_says_dpds_never_changes_the_host() {
    let items = controller_items_with("never changes the host");
    assert!(
        !items.is_empty(),
        "no item of the controller section says that DPDS never changes the host"
    );
    let item = items.join(" ").to_lowercase();
    for who in ["`dpds.sh`", "controller", "frontend"] {
        assert!(
            item.contains(who),
            "the item names {who} among those that never change the host: {item}"
        );
    }
    assert!(
        item.contains("administrator") && item.contains("as root"),
        "an administrator runs the commands as root: {item}"
    );
}

#[test]
fn the_readme_documents_the_commands_of_a_hint() {
    let items = controller_items_with("`commands`");
    assert!(
        !items.is_empty(),
        "no item documents the `commands` of a hint"
    );
    let item = items.join(" ").to_lowercase();
    assert!(
        item.contains("exact") && item.contains("as root") && item.contains("administrator"),
        "the exact commands an administrator runs as root: {item}"
    );
    assert!(
        item.contains("apt install"),
        "a missing tool gets an `apt install` command: {item}"
    );
    assert!(
        item.contains("memavailable") && item.contains("half"),
        "hugepage counts are capped at half of MemAvailable: {item}"
    );
    assert!(
        item.contains("read-only") && item.contains("30"),
        "the tools are only queried, read-only, with a 30 s timeout: {item}"
    );
    assert!(
        item.contains("not checked while an emulation is running"),
        "no tool runs while an emulation runs: {item}"
    );
}

#[test]
fn the_readme_says_what_check_prints_under_a_hint() {
    let items = items_with_all(&["`dpds-controller check`", "`# run as root:`"]);
    assert!(
        !items.is_empty(),
        "no item says that `dpds-controller check` prints `# run as root:` and the commands"
    );
    let item = items.join(" ").to_lowercase();
    assert!(
        item.contains("indented"),
        "the commands are indented: {item}"
    );
    assert!(item.contains("commands"), "{item}");
}

#[test]
fn the_security_item_says_what_reaching_the_port_allows_and_that_the_host_stays_unchanged() {
    let items = controller_items_with("**Security**");
    assert!(!items.is_empty(), "no Security item");
    let item = items.join(" ").to_lowercase();
    assert!(
        item.contains("start") && item.contains("stop") && item.contains("`dpds`"),
        "reaching the port allows starting and stopping `dpds`: {item}"
    );
    assert!(
        [
            "cannot change the host",
            "not change the host",
            "no host change",
            "never changes the host"
        ]
        .iter()
        .any(|p| item.contains(p)),
        "and nothing on the host: {item}"
    );
}

#[test]
fn the_readme_lists_the_checks_the_tools_and_the_hint_only_command_line_checks() {
    let section = controller_section(&readme()).to_lowercase();
    for name in [
        "vm.swappiness",
        "zone_reclaim_mode",
        "irq_affinity",
        "flow control",
        "maxreadreq",
        "port buffer",
        "governor",
        "hugepages",
        "max_acc_out_read",
        "cqe_compression",
    ] {
        assert!(
            section.contains(name),
            "the controller section never names {name}"
        );
    }
    for tool in ["ethtool", "`dcb`", "iproute2", "mstconfig", "mstflint"] {
        assert!(
            section.contains(tool),
            "the controller section never names {tool}"
        );
    }
    let items = controller_items_with("`rcu_nocb_poll`")
        .join(" ")
        .to_lowercase();
    assert!(
        items.contains("kernel command line")
            && (items.contains("hint-only") || items.contains("hint only")),
        "the kernel command line checks are hint-only: {items}"
    );
    assert!(
        !section.contains("irqbalance` hint"),
        "the irqbalance hint is replaced by irq_affinity"
    );
}

#[test]
fn the_readme_says_dpds_sh_tunes_nothing() {
    let tunes_nothing = controller_items_with("`dpds.sh`")
        .into_iter()
        .map(|i| i.to_lowercase())
        .any(|i| i.contains("never tunes") || i.contains("does not tune"));
    assert!(
        tunes_nothing,
        "no item says that `dpds.sh` never tunes the host"
    );
    assert!(
        controller_section(&readme()).contains("Host hints"),
        "the Host hints page is named"
    );
}

// ---- spec 33b item 8 (and the 409 of item 4): what an administrator needs to run the commands ---
//
// `sudo echo 2 > /proc/...` does not work: the redirection is opened by the caller's own shell, not
// by sudo. The README says so and names the two ways that work. It also lists `irq_affinity` with
// the link-dependent hints, since it needs the links to know the NICs.

#[test]
fn the_readme_says_how_to_run_commands_that_write_below_proc_or_sys() {
    let items = controller_items_with("`commands`");
    assert!(
        !items.is_empty(),
        "no item documents the `commands` of a hint"
    );
    let item = items.join(" ");
    assert!(
        item.contains("`sudo -i`"),
        "a root shell (`sudo -i`) runs the commands that write to /proc or /sys: {item}"
    );
    assert!(
        item.contains("sudo tee"),
        "or the value is piped to `sudo tee`: {item}"
    );
    let lower = item.to_lowercase();
    assert!(
        item.contains("sudo echo")
            && (lower.contains("does not work") || lower.contains("doesn't work")),
        "and `sudo echo ... >` is said not to work: {item}"
    );
    assert!(
        lower.contains("/proc") && lower.contains("/sys"),
        "the advice is tied to /proc and /sys: {item}"
    );
}

#[test]
fn irq_affinity_is_listed_as_link_dependent_not_as_host_wide() {
    let items = controller_items_with("**Host check**");
    assert_eq!(items.len(), 1, "one Host check item: {items:?}");
    let item = items[0].to_lowercase();
    let at: Vec<usize> = item.match_indices("irq_affinity").map(|(i, _)| i).collect();
    assert!(!at.is_empty(), "the Host check item names `irq_affinity`");
    for i in at {
        // the closest heading before the mention: the host-wide list or the link-dependent one
        let before = &item[..i];
        let host_wide = before.rfind("host-wide hints");
        let linked = before.rfind("link-dependent");
        let near = &item[i.saturating_sub(100)..(i + 100).min(item.len())];
        let says_linked = near.contains("link-dependent")
            || near.contains("depends on the links")
            || near.contains("needs the links");
        assert!(
            linked > host_wide || says_linked,
            "`irq_affinity` is listed with the host-wide hints, but it needs the links: {near}"
        );
    }
    assert!(
        item.contains("link-dependent"),
        "the item says which hints are link-dependent: {item}"
    );
}

#[test]
fn the_readme_documents_the_400_and_the_409_of_the_hints_post() {
    let items = controller_items_with("`POST /api/host/hints`");
    assert!(
        !items.is_empty(),
        "no item documents `POST /api/host/hints`"
    );
    let item = items.join(" ");
    assert!(
        item.contains("400 naming the field, 409 for a duplicate name or reused port"),
        "the answers of the hints post: {item}"
    );
}

// ---- spec 33c items 1 and 6: no lock around hint queries, and the touched paragraphs wrapped ----

/// The top-level list items of the controller section that mention `needle`, as written (line
/// breaks and indentation kept).
fn controller_raw_items_with(needle: &str) -> Vec<String> {
    let section = controller_section(&readme());
    let mut items: Vec<String> = Vec::new();
    for line in section.lines() {
        if line.starts_with("- ") || items.is_empty() {
            items.push(String::new());
        }
        let last = items.last_mut().unwrap();
        last.push_str(line);
        last.push('\n');
    }
    items.into_iter().filter(|i| i.contains(needle)).collect()
}

/// The two items spec 33b / commit d02768b edited are the "Host check" item (host hints, the
/// hints post) and the "Hint commands" item. The rest of the controller section has older lines
/// longer than 120 characters, so only these two are pinned.
#[test]
fn the_host_check_and_hint_commands_items_have_no_line_over_120_characters() {
    for marker in ["- **Host check**", "- **Hint commands**"] {
        let items = controller_raw_items_with(marker);
        assert_eq!(items.len(), 1, "one item starts with {marker}: {items:?}");
        let too_long: Vec<String> = items[0]
            .lines()
            .filter(|l| l.chars().count() > 120)
            .map(|l| format!("{} characters: {l}", l.chars().count()))
            .collect();
        assert!(
            too_long.is_empty(),
            "{marker} has lines over 120 characters (rewrap the paragraph):\n{}",
            too_long.join("\n")
        );
    }
}

/// Rewrapping must not change what the item says: these facts stay.
#[test]
fn the_rewrapped_items_still_say_what_they_said() {
    let host_check = controller_items_with("- **Host check**").join(" ");
    for needle in [
        "`irq_affinity`",
        "400 naming the field, 409 for a duplicate name or reused port",
        "link-dependent",
    ] {
        assert!(host_check.contains(needle), "Host check lost {needle:?}");
    }
    let commands = controller_items_with("- **Hint commands**").join(" ");
    for needle in [
        "`sudo -i`",
        "sudo tee",
        "sudo echo",
        "budget of 60 s",
        "not checked (time budget of 60 s used up)",
        "not checked while an emulation is running",
        "4 MiB",
    ] {
        assert!(commands.contains(needle), "Hint commands lost {needle:?}");
    }
}

/// Spec 33c item 1: hint queries take no lock, so the README no longer says a start waits for a
/// running query (or that the queries share the lock of start and stop).
#[test]
fn the_readme_no_longer_says_a_start_waits_for_a_running_query() {
    let text = readme();
    let flat = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    for stale in [
        "a start waits",
        "start waits for a running query",
        "same lock as start and stop",
        "under the lock of start and stop",
    ] {
        assert!(!flat.contains(stale), "the README still says {stale:?}");
    }
}

// ---- spec 34: the firmware query is cached until the controller restarts or a recheck -----------

/// The controller section with all whitespace runs joined to single spaces.
fn controller_flat() -> String {
    controller_section(&readme())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The sentences of the controller section that talk about a cache.
fn cache_sentences() -> Vec<String> {
    controller_flat()
        .split(". ")
        .filter(|s| s.to_lowercase().contains("cache"))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_readme_says_the_firmware_query_is_cached_until_the_controller_restarts_or_a_recheck() {
    let sentences = cache_sentences();
    assert!(
        !sentences.is_empty(),
        "no sentence of the controller section mentions the cache"
    );
    let text = sentences.join(". ").to_lowercase();
    for needle in ["firmware", "until the controller restarts", "recheck"] {
        assert!(
            text.contains(needle),
            "the cache sentences lack {needle:?}: {text}"
        );
    }
    // one sentence says all of it
    assert!(
        sentences.iter().any(|s| {
            let s = s.to_lowercase();
            s.contains("firmware")
                && s.contains("until the controller restarts")
                && s.contains("recheck")
        }),
        "{sentences:?}"
    );
}

#[test]
fn the_readme_documents_recheck_true_on_the_hints_post() {
    let items = controller_items_with("`POST /api/host/hints");
    assert!(!items.is_empty(), "no item documents the hints post");
    let item = items.join(" ");
    assert!(
        item.contains("recheck=true"),
        "the hints post documents `recheck=true`: {item}"
    );
    let flat = controller_flat();
    assert!(
        flat.contains("`recheck`") && flat.contains("`recheck=false`"),
        "the query values are documented"
    );
    // a bad value is a 400 naming `recheck`
    assert!(
        flat.contains("400 naming `recheck`"),
        "the README says a bad `recheck` is a 400 naming `recheck`"
    );
}

#[test]
fn the_readme_says_a_cached_result_is_used_while_an_emulation_runs_and_that_check_has_no_cache() {
    let text = cache_sentences().join(". ").to_lowercase();
    assert!(
        text.contains("emulation"),
        "a cached result is used while an emulation is starting, running or stopping: {text}"
    );
    let flat = controller_flat();
    assert!(
        flat.contains("`dpds-controller check`")
            && flat
                .split(". ")
                .any(|s| s.contains("`dpds-controller check`")
                    && s.to_lowercase().contains("no cache")),
        "a sentence names `dpds-controller check` and says it has no cache"
    );
}
