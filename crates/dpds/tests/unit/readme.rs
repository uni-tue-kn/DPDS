//! README checks that are about behaviour, not prose: the hand-run commands use `--in-memory` like
//! PREOF (the e2e tests run the README's commands), and the Markdown links of the READMEs point at
//! files that exist.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn readme() -> String {
    fs::read_to_string(repo_root().join("README.md")).expect("README.md")
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
    assert!(commands.len() >= 2, "found only {commands:?}");
    for c in commands {
        let eal = c.split(" -- ").next().unwrap();
        assert!(
            eal.split_whitespace().any(|a| a == "--in-memory"),
            "README command without --in-memory: {c}"
        );
    }
}

/// The targets of the relative Markdown links `](target)` of `text` (no `http(s)`, `mailto`, or
/// pure `#anchor`), with any `#anchor` stripped.
fn relative_link_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("](") {
        rest = &rest[i + 2..];
        let Some(end) = rest.find(')') else { break };
        let raw = rest[..end].trim();
        // `](path "title")`: the target ends at the first space
        let target = raw.split_whitespace().next().unwrap_or("");
        let path = target.split('#').next().unwrap_or("");
        if !path.is_empty() && !path.contains("://") && !path.starts_with("mailto:") {
            out.push(path.to_string());
        }
        rest = &rest[end..];
    }
    out
}

#[test]
fn the_relative_links_of_the_readmes_point_at_existing_files() {
    let root = repo_root();
    let mut broken = Vec::new();
    let mut checked = 0;
    for doc in [
        "README.md",
        "crates/dpds-core/README.md",
        "frontend/README.md",
    ] {
        let file = root.join(doc);
        let text = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{doc}: {e}"));
        let dir = file.parent().unwrap();
        for target in relative_link_targets(&text) {
            checked += 1;
            if !dir.join(&target).exists() {
                broken.push(format!("{doc} -> {target}"));
            }
        }
    }
    assert!(checked > 0, "no relative link found in any README");
    assert!(broken.is_empty(), "broken relative links: {broken:#?}");
}
