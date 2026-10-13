//! Draft-approve loop CLI boundary tests (ticket #43, spec #41 slice).
//!
//! Seam: argv in -> exit code + stdout/stderr out + state-dir files out.
//! `gh` is the same PATH-sandbox fake as `cli_onboard.rs` (no network).
//! Idea capture, draft generation, and approve/change round-trips live
//! here; the T42 backbone (install/auth/repo/seed) stays covered by
//! `cli_onboard.rs` and is never re-asserted except as pass-through.
//!
//! What these tests catch: drafts rendered in Bea's own words (idea
//! verbatim), versioned approve/change round-trips (v1 kept when v2
//! lands), approval freezing the approved version with a content hash,
//! the approved draft handing a board pointer, and --plain equivalence.
//! What they miss: real browser UI, real `gh issue create`, and the
//! T42 backbone states.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-onboard-draft-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_exe(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Fake `gh`: logs every argv line, then answers from env knobs.
/// GH_AUTH=expired fails `auth status`.
fn write_fake_gh(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "gh",
        r#"#!/bin/sh
for a in "$@"; do printf '%s ' "$a"; done >> "$GH_LOG"
printf '\n' >> "$GH_LOG"
if [ "$1" = "--version" ]; then echo "gh version 2.102.0"; exit 0; fi
if [ "$1" = "auth" ]; then
  if [ "$GH_AUTH" = "expired" ]; then echo "authentication expired" >&2; exit 1; fi
  echo "Logged in to github.com"; exit 0
fi
if [ "$1" = "repo" ] && [ "$2" = "view" ]; then printf '{"nameWithOwner":"%s"}' "$3"; exit 0; fi
if [ "$1" = "repo" ] && [ "$2" = "create" ]; then echo "created"; exit 0; fi
exit 0
"#,
    )
}

struct Harness {
    #[allow(dead_code)]
    root: PathBuf,
    work: PathBuf,
    state: PathBuf,
    gh_log: PathBuf,
    gh: PathBuf,
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    let h = Harness {
        state: root.join("state"),
        gh_log: root.join("gh.log"),
        gh: write_fake_gh(&root),
        root,
        work,
    };
    fs::write(&h.gh_log, "").unwrap();
    h
}

const IDEA: &str = "Groceries remind me what to cook. Dinner plans itself on Sunday.";

fn onboard_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.work);
    cmd.env("FACTORY_GH_BIN", &h.gh)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("GH_LOG", &h.gh_log)
        .env("GH_AUTH", "valid")
        .env_remove("GH_CREATE_FAIL")
        .env_remove("FACTORY_ONBOARD_REPO")
        .env_remove("FACTORY_ONBOARD_NEW")
        .env_remove("FACTORY_ONBOARD_VISIBILITY")
        .env_remove("FACTORY_ONBOARD_IDEA")
        .env_remove("FACTORY_ONBOARD_APPROVE")
        .env_remove("FACTORY_ONBOARD_CHANGE");
    cmd
}

fn onboard_state(h: &Harness) -> serde_json::Value {
    let text = fs::read_to_string(h.state.join("onboard.json"))
        .expect("onboard.json must exist after a run");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn draft_v1_reads_as_own_words_and_asks_for_a_decision() {
    let h = setup("v1");
    onboard_cmd(
        &h,
        &["onboard", "--plain", "--new", "bea-cook", "--idea", IDEA],
    )
    .assert()
    .failure()
    .stdout(predicates::str::contains("[onboard:draft]"))
    .stdout(predicates::str::contains("v1"))
    .stdout(predicates::str::contains(
        "Groceries remind me what to cook.",
    ))
    .stdout(predicates::str::contains("Dinner plans itself on Sunday."))
    .stdout(predicates::str::contains("## Epic"))
    .stdout(predicates::str::contains("## Spec"))
    .stdout(predicates::str::contains("## Tickets"))
    .stderr(predicates::str::contains("--approve"))
    .stderr(predicates::str::contains("--change"));

    let state = onboard_state(&h);
    assert_eq!(
        state.get("draft_version").and_then(|v| v.as_u64()),
        Some(1),
        "first draft is version 1: {state}"
    );
    assert_eq!(
        state.get("approved_version").and_then(|v| v.as_u64()),
        Some(0),
        "nothing is approved yet: {state}"
    );
    let v1 =
        fs::read_to_string(h.state.join("onboard-draft-v1.md")).expect("v1 draft file must exist");
    assert!(
        v1.contains(IDEA),
        "the draft carries the idea verbatim (her own words):\n{v1}"
    );
}

#[test]
fn rerun_without_decision_replays_v1_without_new_version() {
    let h = setup("replay");
    onboard_cmd(
        &h,
        &["onboard", "--plain", "--new", "bea-replay", "--idea", IDEA],
    )
    .assert()
    .failure();
    onboard_cmd(
        &h,
        &["onboard", "--plain", "--new", "bea-replay", "--idea", IDEA],
    )
    .assert()
    .failure()
    .stdout(predicates::str::contains("v1"));

    assert!(
        !h.state.join("onboard-draft-v2.md").is_file(),
        "replaying the decision must not mint a new version"
    );
    assert_eq!(
        onboard_state(&h)
            .get("draft_version")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
}

#[test]
fn change_bumps_version_and_keeps_v1_intact() {
    let h = setup("change");
    onboard_cmd(
        &h,
        &["onboard", "--plain", "--new", "bea-change", "--idea", IDEA],
    )
    .assert()
    .failure();
    let v1_before = fs::read_to_string(h.state.join("onboard-draft-v1.md")).expect("v1 must exist");

    onboard_cmd(
        &h,
        &[
            "onboard",
            "--plain",
            "--new",
            "bea-change",
            "--idea",
            IDEA,
            "--change",
            "Simpler words, please.",
        ],
    )
    .assert()
    .failure()
    .stdout(predicates::str::contains("v2"))
    .stdout(predicates::str::contains("Simpler words, please."))
    .stdout(predicates::str::contains(
        "Groceries remind me what to cook.",
    ));

    let v1_after =
        fs::read_to_string(h.state.join("onboard-draft-v1.md")).expect("v1 must survive");
    assert_eq!(v1_before, v1_after, "v1 is immutable once written");
    let v2 = fs::read_to_string(h.state.join("onboard-draft-v2.md")).expect("v2 must exist");
    assert!(
        v2.contains("Simpler words, please."),
        "v2 records what she asked to change:\n{v2}"
    );
    assert!(v2.contains(IDEA), "v2 still reads as her own words:\n{v2}");
    assert_eq!(
        onboard_state(&h)
            .get("draft_version")
            .and_then(|v| v.as_u64()),
        Some(2),
        "change round-trips the version forward"
    );
}

#[test]
fn approve_freezes_the_version_and_hands_the_board() {
    let h = setup("approve");
    onboard_cmd(
        &h,
        &["onboard", "--plain", "--new", "bea-approve", "--idea", IDEA],
    )
    .assert()
    .failure();
    onboard_cmd(
        &h,
        &[
            "onboard",
            "--plain",
            "--new",
            "bea-approve",
            "--idea",
            IDEA,
            "--approve",
        ],
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("[onboard:approve]"))
    .stdout(predicates::str::contains("v1"))
    .stdout(predicates::str::contains("[onboard:ready]"))
    .stdout(predicates::str::contains("factory ui"));

    let state = onboard_state(&h);
    assert_eq!(
        state.get("approved_version").and_then(|v| v.as_u64()),
        Some(1),
        "approval freezes v1: {state}"
    );
    let hash = state
        .get("draft_hash")
        .and_then(|s| s.as_str())
        .expect("approval records the content hash: {state}");
    assert_eq!(hash.len(), 64, "sha256 hex of what she approved: {state}");
    assert!(
        state
            .get("steps")
            .and_then(|s| s.as_array())
            .map(|steps| steps.iter().any(|s| s == "approve"))
            .unwrap_or(false),
        "the approve round is a recorded step: {state}"
    );
}

#[test]
fn change_then_approve_freezes_v2() {
    let h = setup("change-approve");
    onboard_cmd(
        &h,
        &["onboard", "--plain", "--new", "bea-ca", "--idea", IDEA],
    )
    .assert()
    .failure();
    onboard_cmd(
        &h,
        &[
            "onboard",
            "--plain",
            "--new",
            "bea-ca",
            "--idea",
            IDEA,
            "--change",
            "Shorter tickets.",
        ],
    )
    .assert()
    .failure();
    onboard_cmd(
        &h,
        &[
            "onboard",
            "--plain",
            "--new",
            "bea-ca",
            "--idea",
            IDEA,
            "--approve",
        ],
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("v2"))
    .stdout(predicates::str::contains("factory ui"));

    let state = onboard_state(&h);
    assert_eq!(
        state.get("approved_version").and_then(|v| v.as_u64()),
        Some(2),
        "approval freezes the latest round: {state}"
    );
}

#[test]
fn default_path_without_approve_or_change_keeps_backbone_ready() {
    // No --idea: the T42 backbone contract — ready without a decision.
    let h = setup("legacy");
    onboard_cmd(&h, &["onboard", "--plain", "--new", "bea-legacy"])
        .assert()
        .success()
        .stdout(predicates::str::contains("[onboard:ready]"));
}

#[test]
fn plain_and_default_transcripts_carry_the_same_draft() {
    let h_plain = setup("t-plain");
    onboard_cmd(
        &h_plain,
        &["onboard", "--plain", "--new", "bea-tp", "--idea", IDEA],
    )
    .assert()
    .failure()
    .stdout(predicates::str::contains("[onboard:draft]"))
    .stdout(predicates::str::contains("## Epic"));

    let h_default = setup("t-default");
    onboard_cmd(&h_default, &["onboard", "--new", "bea-td", "--idea", IDEA])
        .assert()
        .failure()
        .stdout(predicates::str::contains("[onboard:draft]"))
        .stdout(predicates::str::contains("## Epic"));
}
