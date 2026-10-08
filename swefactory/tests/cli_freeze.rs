//! Freeze-protocol CLI boundary tests (ticket #9).
//!
//! Seam: argv in -> exit code + stdout/stderr out + repo/issue state changed,
//! per spec #6 Testing Decisions. `gh` is faked on PATH (no network); the
//! binary under test shells out to `gh` exactly like the bash parity surface.
//! Expected hashes are precomputed literals (`printf '%s\n' <body> | sha256sum`),
//! never recomputed the way the binary does.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

// sha256("hello spec\n")
const HELLO_BODY: &str = "hello spec";
const HELLO_HASH: &str = "0f430ed07ac54f37ca7ef9689191bf7b92072a721e8076b2c32096324536fadc";
// sha256("original spec body\n")
const ORIGINAL_BODY: &str = "original spec body";
const ORIGINAL_HASH: &str = "819bedf3fe2528deeb1e7adaf98372218d614fec5e354605b19664c320b8d781";
// sha256("changed spec body\n")
const CHANGED_BODY: &str = "changed spec body";
const CHANGED_HASH: &str = "85148ea7db395c10f1ed280c1bed22e8d4a777ec9383b208efb76df8d4c0779a";

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "factory-freeze-test-{}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst),
        name
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a fake `gh` to `dir`, driven by env vars:
/// FAKE_BODY / FAKE_TITLE / FAKE_COMMENTS_JSON / FAKE_LOG_DIR.
/// Returns the dir so callers can prepend it to PATH.
fn write_fake_gh(dir: &Path) -> PathBuf {
    let path = dir.join("gh");
    fs::write(
        &path,
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "issue" ] && [ "${2:-}" = "view" ]; then
  if [[ "$*" == *comments* ]]; then
    printf '{"comments": %s}\n' "${FAKE_COMMENTS_JSON:-[]}"
  else
    python3 -c 'import json,os; print(json.dumps({"number": int(os.environ.get("FAKE_NUMBER","6")),"title": os.environ.get("FAKE_TITLE","Spec title"),"body": os.environ.get("FAKE_BODY",""),"state": "OPEN"}))'
  fi
elif [ "${1:-}" = "issue" ] && [ "${2:-}" = "comment" ]; then
  echo "comment $*" >> "$FAKE_LOG_DIR/calls.log"
  printf '%s' "${4:-}" >> "$FAKE_LOG_DIR/calls.log" 2>/dev/null || true
  echo "" >> "$FAKE_LOG_DIR/calls.log"
  # record the --body payload verbatim (arg after --body)
  prev=""
  for a in "$@"; do
    if [ "$prev" = "--body" ]; then printf 'BODY:%s\n' "$a" >> "$FAKE_LOG_DIR/calls.log"; fi
    prev="$a"
  done
elif [ "${1:-}" = "issue" ] && [ "${2:-}" = "edit" ]; then
  echo "edit $*" >> "$FAKE_LOG_DIR/calls.log"
else
  echo "fake gh: unexpected args: $*" >&2
  exit 99
fi
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    dir.to_path_buf()
}

struct Fixture {
    workdir: PathBuf,
    fake_bin: PathBuf,
    log_dir: PathBuf,
}

fn setup(body: &str, comments_json: &str) -> Fixture {
    let root = scratch_dir("root");
    let workdir = root.join("work");
    let fake_bin = root.join("bin");
    let log_dir = root.join("log");
    fs::create_dir_all(&workdir).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(&log_dir).unwrap();
    fs::write(log_dir.join("calls.log"), "").unwrap();
    write_fake_gh(&fake_bin);
    // seed env via files the test reads back? No — env is passed per-command.
    // Stash defaults for the `cmd` helper below.
    fs::write(root.join("body"), body).unwrap();
    fs::write(root.join("comments"), comments_json).unwrap();
    Fixture {
        workdir,
        fake_bin,
        log_dir,
    }
}

fn path_with(fake_bin: &Path) -> String {
    format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap())
}

/// Build a command for the binary under test with faked PATH + gh fixtures.
fn cmd(fx: &Fixture, body: &str, comments_json: &str, args: &[&str]) -> assert_cmd::assert::Assert {
    let mut c = Command::cargo_bin("factory").unwrap();
    c.current_dir(&fx.workdir)
        .env("PATH", path_with(&fx.fake_bin))
        .env("FAKE_BODY", body)
        .env("FAKE_COMMENTS_JSON", comments_json)
        .env("FAKE_LOG_DIR", &fx.log_dir)
        .env("FAKE_NUMBER", "6")
        .env("FAKE_TITLE", "Spec title")
        .args(args);
    c.assert()
}

fn calls_log(fx: &Fixture) -> String {
    fs::read_to_string(fx.log_dir.join("calls.log")).unwrap()
}

#[test]
fn freeze_posts_approval_hash_and_label() {
    let fx = setup(HELLO_BODY, "[]");
    cmd(&fx, HELLO_BODY, "[]", &["freeze", "6"])
        .success()
        .stdout(predicates::str::contains(HELLO_HASH));
    let log = calls_log(&fx);
    assert!(
        log.contains(&format!("SPEC-APPROVED `{HELLO_HASH}`")),
        "comment must carry the approval hash, got:\n{log}"
    );
    assert!(
        log.contains("--add-label") && log.contains("spec-approved"),
        "freeze must mark the issue approved with the label, got:\n{log}"
    );
}

#[test]
fn sync_passes_on_unchanged_spec() {
    let comments = format!(r#"[{{"body": "SPEC-APPROVED `{ORIGINAL_HASH}`"}}]"#);
    let fx = setup(ORIGINAL_BODY, &comments);
    cmd(&fx, ORIGINAL_BODY, &comments, &["sync", "6"])
        .success()
        .stdout(predicates::str::contains(ORIGINAL_HASH));
    let cached = fs::read_to_string(fx.workdir.join("factory/.cache/SPEC-6.md")).unwrap();
    assert!(cached.contains(ORIGINAL_BODY), "cache must hold the body");
    assert!(cached.contains(ORIGINAL_HASH), "cache must record the hash");
}

#[test]
fn verify_freeze_ok_on_unchanged_spec() {
    let comments = format!(r#"[{{"body": "SPEC-APPROVED `{ORIGINAL_HASH}`"}}]"#);
    let fx = setup(ORIGINAL_BODY, &comments);
    cmd(&fx, ORIGINAL_BODY, &comments, &["verify-freeze", "6"])
        .success()
        .stdout(predicates::str::contains(ORIGINAL_HASH));
}

#[test]
fn verify_freeze_fails_loudly_on_drift_showing_what_changed() {
    // Approved the ORIGINAL body, but the issue now shows CHANGED.
    let comments = format!(r#"[{{"body": "SPEC-APPROVED `{ORIGINAL_HASH}`"}}]"#);
    let fx = setup(CHANGED_BODY, &comments);
    // Seed a previous cache of the approved body so the diff has a "before".
    fs::create_dir_all(fx.workdir.join("factory/.cache")).unwrap();
    fs::write(
        fx.workdir.join("factory/.cache/SPEC-6.md"),
        format!("<!-- factory cache: issue #6 hash={ORIGINAL_HASH} -->\n# SPEC-6: Spec title\n\n{ORIGINAL_BODY}\n"),
    )
    .unwrap();
    let assert = cmd(&fx, CHANGED_BODY, &comments, &["verify-freeze", "6"]).failure();
    let output = String::from_utf8(assert.get_output().stdout.clone()).unwrap()
        + &String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        output.contains(ORIGINAL_HASH) && output.contains(CHANGED_HASH),
        "drift report must show both hashes, got:\n{output}"
    );
    assert!(
        output.contains(ORIGINAL_BODY) && output.contains(CHANGED_BODY),
        "drift report must show what changed (old vs new lines), got:\n{output}"
    );
}

#[test]
fn sync_shows_what_changed_on_drift() {
    let comments = format!(r#"[{{"body": "SPEC-APPROVED `{ORIGINAL_HASH}`"}}]"#);
    let fx = setup(CHANGED_BODY, &comments);
    fs::create_dir_all(fx.workdir.join("factory/.cache")).unwrap();
    fs::write(
        fx.workdir.join("factory/.cache/SPEC-6.md"),
        format!("<!-- factory cache: issue #6 hash={ORIGINAL_HASH} -->\n# SPEC-6: Spec title\n\n{ORIGINAL_BODY}\n"),
    )
    .unwrap();
    let assert = cmd(&fx, CHANGED_BODY, &comments, &["sync", "6"]).failure();
    let output = String::from_utf8(assert.get_output().stdout.clone()).unwrap()
        + &String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        output.contains(ORIGINAL_HASH) && output.contains(CHANGED_HASH),
        "sync drift must show both hashes, got:\n{output}"
    );
    assert!(
        output.contains(ORIGINAL_BODY) && output.contains(CHANGED_BODY),
        "sync drift must show what changed, got:\n{output}"
    );
    // Cache still advances to the current body so the next sync has a baseline.
    let cached = fs::read_to_string(fx.workdir.join("factory/.cache/SPEC-6.md")).unwrap();
    assert!(cached.contains(CHANGED_BODY));
}

#[test]
fn freeze_json_is_machine_parseable() {
    let fx = setup(HELLO_BODY, "[]");
    let mut c = Command::cargo_bin("factory").unwrap();
    c.current_dir(&fx.workdir)
        .env("PATH", path_with(&fx.fake_bin))
        .env("FAKE_BODY", HELLO_BODY)
        .env("FAKE_COMMENTS_JSON", "[]")
        .env("FAKE_LOG_DIR", &fx.log_dir)
        .env("FAKE_NUMBER", "6")
        .env("FAKE_TITLE", "Spec title")
        .args(["--json", "freeze", "6"]);
    let out = c.output().unwrap();
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("freeze --json must be JSON");
    assert_eq!(
        parsed.get("hash").and_then(|v| v.as_str()),
        Some(HELLO_HASH)
    );
}

#[test]
fn verify_freeze_json_reports_ok_false_on_drift() {
    let comments = format!(r#"[{{"body": "SPEC-APPROVED `{ORIGINAL_HASH}`"}}]"#);
    let fx = setup(CHANGED_BODY, &comments);
    let mut c = Command::cargo_bin("factory").unwrap();
    c.current_dir(&fx.workdir)
        .env("PATH", path_with(&fx.fake_bin))
        .env("FAKE_BODY", CHANGED_BODY)
        .env("FAKE_COMMENTS_JSON", comments.clone())
        .env("FAKE_LOG_DIR", &fx.log_dir)
        .env("FAKE_NUMBER", "6")
        .env("FAKE_TITLE", "Spec title")
        .args(["verify-freeze", "--json", "6"]);
    let out = c.output().unwrap();
    assert!(!out.status.success(), "drift must exit nonzero");
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("verify-freeze --json must be JSON");
    assert_eq!(parsed.get("ok").and_then(|v| v.as_bool()), Some(false));
}
