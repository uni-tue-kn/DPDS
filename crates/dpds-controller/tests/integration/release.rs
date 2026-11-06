//! Spec 18: DPDS 1.0.0, `VERSION.md`, the release job in CI.
//!
//! The frontend half (version line, update check, modal) is in `frontend/src/updateCheck.test.tsx`,
//! `dpds.sh --stable` is in `packaging/tests/dpds_sh.test.sh`, and the CSP is in `frontend.rs`.
//!
//! # What these tests assume
//!
//! - `VERSION.md` at the repository root: first line `1.0.0` (`RELEASE` below; raise it together
//!   with every bump). Every crate of the workspace (as `cargo metadata` reports it, so
//!   `[workspace.package] version` with `version.workspace = true` is fine), `Cargo.lock`,
//!   `frontend/package.json`, the `packages[""]` entry and the top level of
//!   `frontend/package-lock.json`, and the README's version badge agree with it.
//! - `GET /api/online` and `GET /api/version` report that version.
//! - Workflows live in `.github/workflows/*.yml`. The release job is the only job whose script
//!   holds `gh release create`; it is in the same workflow file as the check jobs, because
//!   `needs` cannot cross workflows, and it `needs` every other job of that file.
//!   `permissions: contents: write` appears once in all workflows, in that job; every workflow
//!   keeps `contents: read` at the top level.
//! - The step that runs `gh release create` is self-contained shell: it reads `VERSION.md`,
//!   decides whether the tag exists, and creates the release, so it can be run here against stub
//!   `git` and `gh` commands. The stubs keep one set of tags. They answer `git ls-remote`,
//!   `git tag --list`, `git rev-parse`, `git show-ref`, `git cat-file -e`, `gh release view` and
//!   `gh api .../git/ref/tags/<t>`, `.../git/matching-refs/tags/<prefix>` (prefix match, like
//!   GitHub) and `.../releases/tags/<t>`. A run uses `bash -eo pipefail`, `GITHUB_SHA`,
//!   `GITHUB_REPOSITORY` and no network. The script reads those from the environment, no
//!   `${{ }}` expression in it (the job's `env:` supplies `GH_TOKEN`).
//! - A `VERSION.md` that is not `X.Y.Z` after trimming (first line only, CRLF and surrounding
//!   white space tolerated) makes the step fail, with no release created.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::common::*;

const RELEASE: &str = "1.0.0";
const CRATES: [&str; 5] = ["dpds-core", "dpdk-sys", "dpdk", "dpds", "dpds-controller"];

fn repo() -> PathBuf {
    // DPDS_TEST_REPO points the file checks at a copy of the tree (used to try these tests against a
    // sample workflow without touching the repository).
    if let Some(p) = std::env::var_os("DPDS_TEST_REPO") {
        return PathBuf::from(p);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn read(rel: &str) -> String {
    let p = repo().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

fn version_md() -> String {
    read("VERSION.md")
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn is_semver(v: &str) -> bool {
    let p: Vec<&str> = v.split('.').collect();
    p.len() == 3
        && p.iter()
            .all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()))
}

// ---- version in one place ----------------------------------------------------------------------

#[test]
fn version_md_holds_the_release_version_on_its_first_line() {
    let text = read("VERSION.md");
    let first = text.lines().next().unwrap_or("");
    assert!(
        !first.ends_with('\r'),
        "VERSION.md must use LF line endings: {first:?}"
    );
    assert!(is_semver(first), "first line {first:?} is not X.Y.Z");
    assert_eq!(first, RELEASE);
}

#[test]
fn every_workspace_crate_has_the_version_of_version_md() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .current_dir(repo())
        .output()
        .expect("cargo metadata");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: Value = serde_json::from_slice(&out.stdout).unwrap();
    let wanted = version_md();
    let mut seen = BTreeSet::new();
    for p in meta["packages"].as_array().unwrap() {
        let name = p["name"].as_str().unwrap().to_string();
        assert_eq!(
            p["version"].as_str().unwrap(),
            wanted,
            "crate {name} disagrees with VERSION.md"
        );
        seen.insert(name);
    }
    for c in CRATES {
        assert!(seen.contains(c), "crate {c} is not a workspace member");
    }
}

#[test]
fn cargo_lock_lists_the_workspace_crates_with_the_version_of_version_md() {
    let lock = read("Cargo.lock");
    let wanted = version_md();
    let lines: Vec<&str> = lock.lines().collect();
    for c in CRATES {
        let at = lines
            .iter()
            .position(|l| *l == format!("name = \"{c}\""))
            .unwrap_or_else(|| panic!("{c} not in Cargo.lock"));
        assert_eq!(
            lines[at + 1],
            format!("version = \"{wanted}\""),
            "Cargo.lock entry of {c} is stale (run cargo update -w or any cargo build)"
        );
    }
}

#[test]
fn the_frontend_package_files_have_the_version_of_version_md() {
    let wanted = version_md();
    let pkg: Value = serde_json::from_str(&read("frontend/package.json")).unwrap();
    assert_eq!(pkg["version"], wanted.as_str(), "frontend/package.json");
    let lock: Value = serde_json::from_str(&read("frontend/package-lock.json")).unwrap();
    assert_eq!(
        lock["version"],
        wanted.as_str(),
        "package-lock.json top level"
    );
    assert_eq!(
        lock["packages"][""]["version"],
        wanted.as_str(),
        "package-lock.json packages[\"\"]"
    );
}

#[test]
fn the_readme_version_badge_shows_the_version_of_version_md() {
    let readme = read("README.md");
    let wanted = version_md();
    assert!(
        readme.contains(&format!("img.shields.io/badge/v-{wanted}-")),
        "README badge does not show {wanted}"
    );
    assert!(
        !readme.contains("badge/v-0.1.0-"),
        "stale 0.1.0 badge in the README"
    );
}

// ---- README badges (spec 23) ---------------------------------------------------------------------

const BADGE_BACKEND: &str =
    "![Backend](https://img.shields.io/badge/backend-Rust-b7410e?logo=rust)";
const BADGE_FRONTEND: &str =
    "![Frontend](https://img.shields.io/badge/frontend-React-149eca?logo=react)";
const BADGE_CI: &str = "[![CI](https://github.com/uni-tue-kn/DPDS/actions/workflows/ci.yml/badge.svg?branch=nightly)](https://github.com/uni-tue-kn/DPDS/actions/workflows/ci.yml)";
const BADGE_DPDK: &str = "![DPDK](https://img.shields.io/badge/dataplane-DPDK%2026.07-1e5aa8)";

/// The badge block at the top of the README: everything before the first `---` line.
fn readme_badge_block() -> String {
    let readme = read("README.md");
    let end = readme
        .lines()
        .position(|l| l.trim() == "---")
        .expect("a `---` line after the badges");
    readme.lines().take(end).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_readme_has_the_backend_and_frontend_badges_with_the_exact_urls() {
    let top = readme_badge_block();
    assert!(top.contains(BADGE_BACKEND), "backend badge missing");
    assert!(top.contains(BADGE_FRONTEND), "frontend badge missing");
}

#[test]
fn the_readme_has_the_ci_badge_for_nightly_linking_to_the_workflow() {
    let top = readme_badge_block();
    assert!(top.contains(BADGE_CI), "CI badge missing or not exact");
    assert_eq!(
        top.matches("actions/workflows/ci.yml").count(),
        2,
        "the CI badge is the image and the link to the workflow, once"
    );
}

#[test]
fn the_readme_dpdk_badge_is_the_text_badge_without_a_logo() {
    let top = readme_badge_block();
    assert!(top.contains(BADGE_DPDK), "DPDK badge missing or not exact");
    assert!(
        !top.contains("built%20with-DPDK"),
        "the old DPDK badge is still there"
    );
    assert_eq!(
        top.lines()
            .filter(|l| l.contains("shields.io") && l.contains("DPDK"))
            .count(),
        1,
        "exactly one DPDK badge in the header"
    );
}

#[test]
fn the_readme_has_no_lang_rust_badge_any_more() {
    let readme = read("README.md");
    assert!(
        !readme.contains("lang-rust") && !readme.contains("badge/lang-"),
        "the old Language badge is still in the README"
    );
    assert!(!readme.contains("![Language]"));
}

#[test]
fn the_readme_keeps_the_license_and_version_badges() {
    let top = readme_badge_block();
    assert!(
        top.contains("![License](https://img.shields.io/badge/licence-Apache%202.0-blue)"),
        "license badge changed or gone"
    );
    assert!(
        top.contains(&format!(
            "![Version](https://img.shields.io/badge/v-{}-yellow)",
            version_md()
        )),
        "version badge changed or gone"
    );
}

#[tokio::test]
async fn the_controller_reports_the_release_version_on_online_and_version() {
    let a = TestApp::new(FakeHost::bare_metal());
    let online = a.get("/api/online").await;
    assert_eq!(online.status, 200, "{}", online.text());
    assert_eq!(online.json()["version"], RELEASE);
    let version = a.get("/api/version").await;
    assert_eq!(version.status, 200, "{}", version.text());
    assert_eq!(version.json()["version"], RELEASE);
}

// ---- workflows --------------------------------------------------------------------------------

struct Workflow {
    file: String,
    text: String,
}

fn workflows() -> Vec<Workflow> {
    let dir = repo().join(".github/workflows");
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir).expect("workflows dir") {
        let p = e.unwrap().path();
        if matches!(p.extension().and_then(|x| x.to_str()), Some("yml" | "yaml")) {
            out.push(Workflow {
                file: p.file_name().unwrap().to_string_lossy().into_owned(),
                text: std::fs::read_to_string(&p).unwrap(),
            });
        }
    }
    out
}

struct Job {
    name: String,
    lines: Vec<String>,
}

impl Job {
    fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Value of a job-level key (4 spaces of indent), continuation lines included.
    fn key(&self, key: &str) -> Option<String> {
        let head = format!("    {key}:");
        let at = self.lines.iter().position(|l| l.starts_with(&head))?;
        let mut v = self.lines[at][head.len()..].trim().to_string();
        for l in &self.lines[at + 1..] {
            if l.trim().is_empty() || l.trim_start().starts_with('#') {
                continue;
            }
            if indent(l) <= 4 {
                break;
            }
            v.push(' ');
            v.push_str(l.trim());
        }
        Some(v)
    }
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

/// `(jobs, text outside the jobs block)`.
fn split_jobs(text: &str) -> (Vec<Job>, String) {
    let mut jobs: Vec<Job> = Vec::new();
    let mut outside = Vec::new();
    let mut in_jobs = false;
    for l in text.lines() {
        if l.starts_with("jobs:") {
            in_jobs = true;
            continue;
        }
        if in_jobs && !l.trim().is_empty() && indent(l) == 0 && !l.starts_with('#') {
            in_jobs = false;
        }
        if !in_jobs {
            outside.push(l.to_string());
            continue;
        }
        let t = l.trim_end();
        let is_job_head = indent(t) == 2
            && t.trim_start().ends_with(':')
            && !t.trim_start().starts_with('#')
            && !t.trim_start().starts_with("- ");
        if is_job_head {
            jobs.push(Job {
                name: t.trim().trim_end_matches(':').to_string(),
                lines: Vec::new(),
            });
        }
        if let Some(j) = jobs.last_mut() {
            j.lines.push(l.to_string());
        }
    }
    (jobs, outside.join("\n"))
}

/// The workflow file and the job holding `gh release create`; panics unless there is exactly one.
fn release_job() -> (Workflow, Job, Vec<Job>) {
    let mut found = Vec::new();
    for w in workflows() {
        let (jobs, _) = split_jobs(&w.text);
        let (rel, others): (Vec<Job>, Vec<Job>) = jobs
            .into_iter()
            .partition(|j| j.text().contains("gh release create"));
        for r in rel {
            found.push((
                Workflow {
                    file: w.file.clone(),
                    text: w.text.clone(),
                },
                r,
                others
                    .iter()
                    .map(|o| Job {
                        name: o.name.clone(),
                        lines: o.lines.clone(),
                    })
                    .collect::<Vec<_>>(),
            ));
        }
    }
    assert_eq!(
        found.len(),
        1,
        "exactly one job must run `gh release create`, found {}",
        found.len()
    );
    found.pop().unwrap()
}

fn words(s: &str) -> BTreeSet<String> {
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .filter(|w| !w.is_empty() && *w != "-")
        .map(str::to_string)
        .collect()
}

#[test]
fn a_release_job_exists_next_to_the_check_jobs() {
    let (w, job, others) = release_job();
    let names: BTreeSet<String> = others.iter().map(|j| j.name.clone()).collect();
    for check in CHECK_JOBS {
        assert!(
            names.contains(check),
            "{}: the workflow with the release job lacks the check job {check} ({names:?})",
            w.file
        );
    }
    assert!(
        !names.contains(&job.name),
        "the release job's name is taken twice"
    );
}

/// Spec 23: `rust` became `backend` (with the fmt step), `packaging` became `installer`.
const CHECK_JOBS: [&str; 3] = ["backend", "frontend", "installer"];

#[test]
fn the_check_jobs_are_exactly_backend_frontend_and_installer() {
    let (w, job, others) = release_job();
    let names: BTreeSet<String> = others.iter().map(|j| j.name.clone()).collect();
    let want: BTreeSet<String> = CHECK_JOBS.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        names, want,
        "{}: jobs next to `{}` (no fmt, rust or packaging job any more)",
        w.file, job.name
    );
}

#[test]
fn the_installer_job_runs_the_dpds_sh_tests() {
    let (_, _, others) = release_job();
    let installer = others
        .iter()
        .find(|j| j.name == "installer")
        .expect("an `installer` job");
    assert!(
        installer
            .text()
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .any(|l| l.contains("packaging/tests/dpds_sh.test.sh")),
        "the installer job must run packaging/tests/dpds_sh.test.sh"
    );
}

#[test]
fn the_release_job_needs_every_check_job() {
    let (w, job, others) = release_job();
    let needs = job
        .key("needs")
        .unwrap_or_else(|| panic!("{}: the release job has no `needs`", w.file));
    let got = words(&needs);
    for o in &others {
        assert!(
            got.contains(&o.name),
            "{}: release job does not need {} (needs: {needs})",
            w.file,
            o.name
        );
    }
}

#[test]
fn the_release_job_needs_exactly_backend_frontend_and_installer() {
    let (w, job, _) = release_job();
    let needs = job
        .key("needs")
        .unwrap_or_else(|| panic!("{}: the release job has no `needs`", w.file));
    let want: BTreeSet<String> = CHECK_JOBS.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        words(&needs),
        want,
        "{}: release job needs: {needs}",
        w.file
    );
}

#[test]
fn the_release_job_runs_only_for_pushes_to_main() {
    let (w, job, _) = release_job();
    let cond = job
        .key("if")
        .unwrap_or_else(|| panic!("{}: the release job has no `if`", w.file));
    assert!(
        cond.contains("refs/heads/main") || cond.contains("ref_name == 'main'"),
        "the condition must name main: {cond}"
    );
    for bad in ["nightly", "master", "pull_request", "!="] {
        assert!(
            !cond.contains(bad),
            "the condition must not allow other refs ({bad}): {cond}"
        );
    }
    // the workflow itself still runs on nightly and pull requests: the job must be what limits it
    assert!(
        w.text.contains("pull_request"),
        "the check workflow was changed: it no longer runs on pull requests"
    );
}

#[test]
fn contents_write_exists_once_and_only_in_the_release_job() {
    let (_, job, others) = release_job();
    let perms = job.key("permissions").expect("job-level permissions");
    assert!(
        perms.contains("contents: write"),
        "release job permissions: {perms}"
    );
    let mut writes = 0;
    for w in workflows() {
        writes += w.text.matches(": write").count();
        assert!(
            !w.text.contains("write-all"),
            "{}: write-all permissions",
            w.file
        );
        let (_, outside) = split_jobs(&w.text);
        assert!(
            !outside.contains(": write"),
            "{}: workflow-level permissions must stay read-only",
            w.file
        );
        assert!(
            outside.contains("contents: read"),
            "{}: top-level `permissions: contents: read` is gone",
            w.file
        );
    }
    assert_eq!(writes, 1, "write permissions across all workflows");
    for o in &others {
        if let Some(p) = o.key("permissions") {
            assert!(!p.contains("write"), "job {} permissions: {p}", o.name);
        }
    }
}

#[test]
fn the_release_job_uses_the_documented_gh_call_and_token() {
    let (_, job, _) = release_job();
    let t = job.text();
    assert!(t.contains("actions/checkout"), "the job needs the sources");
    assert!(
        t.contains("GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}"),
        "GH_TOKEN must come from secrets.GITHUB_TOKEN"
    );
    assert!(t.contains("VERSION.md"), "the job must read VERSION.md");
    assert!(t.contains("--generate-notes"));
    assert!(t.contains("--repo \"$GITHUB_REPOSITORY\""), "--repo");
    assert!(t.contains("--target \"$GITHUB_SHA\""), "--target");
    assert!(t.contains("--title \"DPDS "), "--title \"DPDS v<version>\"");
}

#[test]
fn the_release_job_publishes_no_assets() {
    let (_, job, _) = release_job();
    let t = job.text();
    for bad in [
        "release upload",
        "upload-artifact",
        "upload-release-asset",
        "action-gh-release",
        "gh-release",
        "ghcr.io",
    ] {
        assert!(!t.contains(bad), "release job mentions {bad}");
    }
    assert!(
        !t.contains("cargo build") && !t.contains("npm "),
        "the release job builds nothing"
    );
}

#[test]
fn no_workflow_uses_docker() {
    for w in workflows() {
        assert!(
            !w.text.to_lowercase().contains("docker"),
            "{} mentions docker",
            w.file
        );
    }
}

// ---- the release script, run against stub git and gh --------------------------------------------

/// The shell of the step that runs `gh release create`.
fn release_script() -> String {
    let (_, job, _) = release_job();
    let lines = &job.lines;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start().trim_start_matches("- ").trim_start();
        let Some(rest) = t.strip_prefix("run:") else {
            continue;
        };
        let rest = rest.trim();
        let key_indent = indent(l)
            + if l.trim_start().starts_with("- ") {
                2
            } else {
                0
            };
        let script = if rest.starts_with('|') || rest.starts_with('>') {
            let mut block: Vec<&str> = Vec::new();
            for b in &lines[i + 1..] {
                if !b.trim().is_empty() && indent(b) <= key_indent {
                    break;
                }
                block.push(b);
            }
            let base = block
                .iter()
                .filter(|b| !b.trim().is_empty())
                .map(|b| indent(b))
                .min()
                .unwrap_or(0);
            block
                .iter()
                .map(|b| b.get(base..).unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            rest.to_string()
        };
        if script.contains("gh release create") {
            return script;
        }
    }
    panic!("no `run:` step of the release job holds `gh release create`");
}

#[test]
fn the_release_step_is_self_contained_and_reads_version_md_itself() {
    let s = release_script();
    assert!(
        s.contains("VERSION.md"),
        "the step must read VERSION.md:\n{s}"
    );
    assert!(
        !s.contains("${{"),
        "no ${{{{ }}}} expression inside the script, use the environment:\n{s}"
    );
}

const STUB_GIT: &str = r#"#!/bin/bash
{ printf 'git'; for a in "$@"; do printf '\037%s' "$a"; done; echo; } >> "$STUB_LOG"
has() { grep -qxF -- "$1" "$STUB_TAGS"; }
strip() { local t="${1#refs/tags/}"; t="${t%%^*}"; echo "$t"; }
sub="${1:-}"; shift || true
case "$sub" in
  ls-remote)
    exitcode=0; pats=(); remote=""
    for a in "$@"; do
      case "$a" in
        --exit-code) exitcode=1;;
        -*) ;;
        *) if [ -z "$remote" ]; then remote=$a; else pats+=("$a"); fi;;
      esac
    done
    out=0
    while IFS= read -r t; do
      [ -n "$t" ] || continue
      if [ ${#pats[@]} -eq 0 ]; then printf 'abc123\trefs/tags/%s\n' "$t"; out=1; continue; fi
      for p in "${pats[@]}"; do
        pp="${p#refs/tags/}"
        # git matches a pattern against the tail of the ref name
        # shellcheck disable=SC2053
        if [[ "$t" == $pp ]]; then printf 'abc123\trefs/tags/%s\n' "$t"; out=1; fi
      done
    done < "$STUB_TAGS"
    [ "$exitcode" = 1 ] && [ "$out" = 0 ] && exit 2
    exit 0;;
  tag)
    list=0; pos=()
    for a in "$@"; do
      case "$a" in
        -l|--list) list=1;;
        -*) ;;
        *) pos+=("$a");;
      esac
    done
    if [ "$list" = 1 ] || [ ${#pos[@]} -eq 0 ]; then
      while IFS= read -r t; do
        [ -n "$t" ] || continue
        if [ ${#pos[@]} -eq 0 ]; then echo "$t"; continue; fi
        # shellcheck disable=SC2053
        [[ "$t" == ${pos[0]} ]] && echo "$t"
      done < "$STUB_TAGS"
      exit 0
    fi
    echo "${pos[0]}" >> "$STUB_TAGS"; exit 0;;
  rev-parse)
    last=""
    for a in "$@"; do case "$a" in -*) ;; *) last=$a;; esac; done
    [ "$last" = HEAD ] && { echo "$GITHUB_SHA"; exit 0; }
    t=$(strip "$last")
    if has "$t"; then echo abc123; exit 0; fi
    echo "fatal: Needed a single revision" >&2; exit 128;;
  show-ref)
    last=""
    for a in "$@"; do case "$a" in -*) ;; *) last=$a;; esac; done
    t=$(strip "$last")
    if has "$t"; then echo "abc123 refs/tags/$t"; exit 0; fi
    exit 1;;
  cat-file)
    last=""
    for a in "$@"; do case "$a" in -*) ;; *) last=$a;; esac; done
    has "$(strip "$last")" && exit 0
    exit 128;;
esac
exit 0
"#;

const STUB_GH: &str = r#"#!/bin/bash
{ printf 'gh'; for a in "$@"; do printf '\037%s' "$a"; done; echo; } >> "$STUB_LOG"
has() { grep -qxF -- "$1" "$STUB_TAGS"; }
# first positional argument after the subcommand words, skipping flags and their values
first_pos() {
  local skip=0 a
  for a in "$@"; do
    if [ $skip = 1 ]; then skip=0; continue; fi
    case "$a" in
      --repo|-R|--json|--jq|-q|--template|-t|--target|--title|--notes|-n|--notes-file|-F|--notes-start-tag|--discussion-category|--method|-X|-H|--header|-f|-F|--field|--raw-field)
        skip=1;;
      -*) ;;
      *) echo "$a"; return;;
    esac
  done
}
case "${1:-} ${2:-}" in
  "release view")
    t=$(first_pos "${@:3}")
    if has "$t"; then echo "{\"tagName\":\"$t\"}"; exit 0; fi
    echo "release not found" >&2; exit 1;;
  "release create")
    t=$(first_pos "${@:3}")
    echo "$t" >> "$STUB_TAGS"
    echo "https://github.com/uni-tue-kn/DPDS/releases/tag/$t"; exit 0;;
  "release list")
    while IFS= read -r t; do [ -n "$t" ] && printf '%s\t%s\n' "$t" "Latest"; done < "$STUB_TAGS"
    exit 0;;
esac
if [ "${1:-}" = api ]; then
  path=$(first_pos "${@:2}")
  case "$path" in
    */git/ref/tags/*|*/git/refs/tags/*|*/releases/tags/*)
      t="${path##*/tags/}"
      if has "$t"; then echo "{\"ref\":\"refs/tags/$t\",\"object\":{\"sha\":\"abc123\"}}"; exit 0; fi
      echo '{"message":"Not Found","status":"404"}'; echo "gh: Not Found (HTTP 404)" >&2; exit 1;;
    */git/matching-refs/tags/*)
      pre="${path##*/tags/}"; first=1; printf '['
      while IFS= read -r t; do
        [ -n "$t" ] || continue
        case "$t" in "$pre"*)
          [ $first = 1 ] || printf ','
          first=0; printf '{"ref":"refs/tags/%s"}' "$t";;
        esac
      done < "$STUB_TAGS"
      echo ']'; exit 0;;
  esac
fi
exit 0
"#;

struct Run {
    rc: i32,
    stderr: String,
    /// Every stub call, arguments split.
    calls: Vec<Vec<String>>,
    tags_after: Vec<String>,
}

impl Run {
    fn creates(&self) -> Vec<&Vec<String>> {
        self.calls
            .iter()
            .filter(|c| {
                c[0] == "gh"
                    && c.get(1).map(String::as_str) == Some("release")
                    && c.get(2).map(String::as_str) == Some("create")
            })
            .collect()
    }
}

/// Runs the release step with `version_md` as `VERSION.md` and `tags` as the existing tags;
/// `rerun` runs it a second time on the resulting state and returns that run.
fn run_release(version_md: &str, tags: &[&str], reruns: usize) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let stubs = root.join("stubs");
    let base = root.join("base");
    let work = root.join("work");
    for d in [&stubs, &base, &work] {
        std::fs::create_dir_all(d).unwrap();
    }
    for (n, body) in [("git", STUB_GIT), ("gh", STUB_GH)] {
        let p = stubs.join(n);
        std::fs::write(&p, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for u in [
        "bash", "sh", "env", "cat", "sed", "grep", "egrep", "awk", "tr", "head", "tail", "cut",
        "sort", "uniq", "mktemp", "rm", "mkdir", "wc", "xargs", "date", "true", "false", "tee",
        "ls", "stat", "id", "cmp", "diff", "readlink", "realpath", "touch", "basename", "dirname",
        "jq", "printf", "echo", "test", "expr",
    ] {
        for d in ["/usr/bin", "/bin", "/usr/local/bin"] {
            let src = Path::new(d).join(u);
            if src.exists() {
                let _ = std::os::unix::fs::symlink(&src, base.join(u));
                break;
            }
        }
    }
    std::fs::write(work.join("VERSION.md"), version_md).unwrap();
    let tags_file = root.join("tags");
    std::fs::write(
        &tags_file,
        tags.iter().map(|t| format!("{t}\n")).collect::<String>(),
    )
    .unwrap();
    let log = root.join("calls.log");
    std::fs::write(&log, "").unwrap();
    let script = root.join("step.sh");
    std::fs::write(&script, release_script()).unwrap();

    let mut last_rc = 0;
    let mut last_err = String::new();
    let mut calls_before = 0;
    let mut last_calls: Vec<Vec<String>> = Vec::new();
    for _ in 0..=reruns {
        let out = Command::new(base.join("bash"))
            .args(["-eo", "pipefail"])
            .arg(&script)
            .current_dir(&work)
            .env_clear()
            .env("PATH", format!("{}:{}", stubs.display(), base.display()))
            .env("HOME", root)
            .env("STUB_LOG", &log)
            .env("STUB_TAGS", &tags_file)
            .env("GITHUB_SHA", "0123456789abcdef0123456789abcdef01234567")
            .env("GITHUB_REPOSITORY", "uni-tue-kn/DPDS")
            .env("GITHUB_REF", "refs/heads/main")
            .env("GITHUB_REF_NAME", "main")
            .env("GITHUB_EVENT_NAME", "push")
            .env("GITHUB_OUTPUT", root.join("output"))
            .env("GITHUB_ENV", root.join("env"))
            .env("GITHUB_STEP_SUMMARY", root.join("summary"))
            .env("GH_TOKEN", "test-token")
            .output()
            .expect("run the release step");
        last_rc = out.status.code().unwrap_or(-1);
        last_err = String::from_utf8_lossy(&out.stderr).into_owned();
        let all: Vec<Vec<String>> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| l.split('\u{1f}').map(str::to_string).collect())
            .collect();
        last_calls = all[calls_before..].to_vec();
        calls_before = all.len();
    }
    let tags_after = std::fs::read_to_string(&tags_file)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    Run {
        rc: last_rc,
        stderr: last_err,
        calls: last_calls,
        tags_after,
    }
}

/// The value after `flag` in a call, if any.
fn flag_value<'a>(call: &'a [String], flag: &str) -> Option<&'a str> {
    call.iter()
        .position(|a| a == flag)
        .and_then(|i| call.get(i + 1))
        .map(String::as_str)
}

fn assert_created(run: &Run, tag: &str) {
    assert_eq!(run.rc, 0, "step failed: {}", run.stderr);
    let creates = run.creates();
    assert_eq!(creates.len(), 1, "gh release create calls: {:?}", run.calls);
    let c = creates[0];
    // the tag is the one positional argument; any other positional would be an asset file
    let value_flags = [
        "--target",
        "--title",
        "--repo",
        "-R",
        "--notes",
        "-n",
        "--notes-file",
        "-F",
        "--notes-start-tag",
        "--discussion-category",
        "-t",
    ];
    let mut positional = Vec::new();
    let mut skip = false;
    for a in &c[3..] {
        if skip {
            skip = false;
            continue;
        }
        if value_flags.contains(&a.as_str()) {
            skip = true;
        } else if !a.starts_with('-') {
            positional.push(a.as_str());
        }
    }
    assert_eq!(positional, [tag], "positional arguments of {c:?}");
    assert_eq!(
        flag_value(c, "--target"),
        Some("0123456789abcdef0123456789abcdef01234567"),
        "{c:?}"
    );
    assert!(c.iter().any(|a| a == "--generate-notes"), "{c:?}");
    assert_eq!(
        flag_value(c, "--title"),
        Some(format!("DPDS {tag}").as_str()),
        "{c:?}"
    );
    assert_eq!(flag_value(c, "--repo"), Some("uni-tue-kn/DPDS"), "{c:?}");
    assert!(c.iter().all(|a| !a.contains('\r')), "stray CR in {c:?}");
}

fn assert_nothing_changed(run: &Run, tags: &[&str]) {
    assert_eq!(run.rc, 0, "step failed: {}", run.stderr);
    assert!(
        run.creates().is_empty(),
        "created a release: {:?}",
        run.calls
    );
    for c in &run.calls {
        let joined = c.join(" ");
        for bad in [
            "release create",
            "release edit",
            "release delete",
            "release upload",
            "git push",
        ] {
            assert!(!joined.contains(bad), "unexpected {bad}: {joined}");
        }
    }
    assert_eq!(run.tags_after, tags, "the tag list changed");
}

#[test]
fn a_missing_tag_gets_a_release_with_generated_notes_and_no_assets() {
    let run = run_release("1.2.3\n", &["v0.9.0", "v1.2.2"], 0);
    assert_created(&run, "v1.2.3");
}

#[test]
fn an_existing_tag_means_nothing_is_done() {
    let tags = ["v0.9.0", "v1.2.3"];
    let run = run_release("1.2.3\n", &tags, 0);
    assert_nothing_changed(&run, &tags);
}

#[test]
fn only_the_exact_tag_counts_as_existing_not_a_prefix_or_a_similar_one() {
    let run = run_release(
        "1.2.3\n",
        &["v1.2.30", "v11.2.3", "v1.2.3-rc1", "v1.2.2", "1.2.3"],
        0,
    );
    assert_created(&run, "v1.2.3");
}

#[test]
fn the_release_is_created_once_when_the_job_runs_again() {
    let run = run_release("1.2.3\n", &[], 1);
    assert_nothing_changed(&run, &["v1.2.3"]);
}

#[test]
fn only_the_first_line_of_version_md_is_the_version() {
    let run = run_release("1.2.3\n\nNotes for 9.9.9\n", &[], 0);
    assert_created(&run, "v1.2.3");
}

#[test]
fn a_version_md_without_a_trailing_newline_works() {
    let run = run_release("1.2.3", &[], 0);
    assert_created(&run, "v1.2.3");
}

#[test]
fn crlf_and_surrounding_white_space_do_not_end_up_in_the_tag() {
    for body in ["1.2.3\r\n", " 1.2.3 \t\n", "1.2.3  \n"] {
        let run = run_release(body, &["v1.2.2"], 0);
        assert_created(&run, "v1.2.3");
    }
}

#[test]
fn a_malformed_version_md_fails_the_step_and_creates_nothing() {
    for body in ["", "\n", "banana\n", "1.2\n", "1.2.x\n", "version 1.2.3\n"] {
        let run = run_release(body, &["v1.0.0"], 0);
        assert_ne!(run.rc, 0, "VERSION.md {body:?} must fail the step");
        assert!(
            run.creates().is_empty(),
            "VERSION.md {body:?} created a release: {:?}",
            run.calls
        );
        assert_eq!(run.tags_after, ["v1.0.0"], "VERSION.md {body:?}");
    }
}

#[test]
fn the_release_for_a_new_major_version_uses_that_version() {
    let run = run_release("12.0.7\n", &["v1.0.0", "v2.0.0"], 0);
    assert_created(&run, "v12.0.7");
}
