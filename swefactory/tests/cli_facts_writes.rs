//! Guarded-writes CLI boundary tests (ticket #78, spec #64).
//!
//! Seam: argv in -> exit code + stdout/stderr out + observable
//! state-directory contents. State comes from a seeded
//! `FACTORY_STATE_DIR`; the working directory is a scratch git repo
//! standing in for the project. No daemon, no network, no issue-tracker
//! calls.
//!
//! What these tests catch: create/set/delete round-trips in both output
//! modes with identical value-plus-revision facts; create-on-existing
//! refused with the stored value intact; stale-revision writes rejected
//! naming presented and current revisions with the prior value intact;
//! fresh-read retry after a conflict succeeding with revision advancing
//! by exactly one; and a crash simulated mid-write leaving the last
//! fully committed revision intact.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-facts-writes-{}-{}-{}",
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

fn git_run(repo: &Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?} failed in {}", repo.display());
}

struct Harness {
    repo: PathBuf,
    state: PathBuf,
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_run(&repo, &["init", "-b", "dev"]);
    git_run(&repo, &["config", "user.email", "t@t"]);
    git_run(&repo, &["config", "user.name", "t"]);
    fs::write(repo.join("f.txt"), "x\n").unwrap();
    git_run(&repo, &["add", "."]);
    git_run(&repo, &["commit", "-m", "seed"]);
    Harness {
        state: root.join("state"),
        repo,
    }
}

fn facts_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

/// Read the raw facts document from the state dir.
fn read_doc(h: &Harness) -> serde_json::Value {
    let text = fs::read_to_string(h.state.join("facts.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn show_json(h: &Harness, path: &str) -> serde_json::Value {
    let out = facts_cmd(h, &["facts", "show", path, "--json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "show {path} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap()
}

#[test]
fn facts_create_set_delete_round_trip_both_modes() {
    let h = setup("roundtrip");
    fs::create_dir_all(&h.state).unwrap();

    // Create at revision zero: human names path, revision 1, and value.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "42",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "create stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("costs/total"),
        "human names path, got:\n{text}"
    );
    assert!(text.contains("revision 1"), "human revision, got:\n{text}");
    assert!(text.contains("42"), "human value, got:\n{text}");

    // Same create facts in machine mode (fresh path, so create again on
    // a second harness path to compare modes on the same operation).
    let h2 = setup("roundtrip-json");
    fs::create_dir_all(&h2.state).unwrap();
    let out = facts_cmd(
        &h2,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "42",
            "--json",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(1));
    assert_eq!(v["path"], serde_json::json!("costs/total"));
    assert_eq!(v["value"], serde_json::json!(42));

    // Stored state agrees: revision 1 with the value committed.
    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(1));
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(42));
    assert_eq!(show_json(&h, "costs/total")["value"], serde_json::json!(42));
    assert_eq!(
        show_json(&h, "costs/total")["revision"],
        serde_json::json!(1)
    );

    // Set (upsert) over the created path: revision 1 -> 2, both modes.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "1",
            "--value",
            "43",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "set stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("revision 2"), "human revision, got:\n{text}");
    assert!(text.contains("43"), "human value, got:\n{text}");

    let out = facts_cmd(
        &h2,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "1",
            "--value",
            "43",
            "--json",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(2));
    assert_eq!(v["value"], serde_json::json!(43));

    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(2));
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(43));

    // Delete: revision 2 -> 3, both modes carry the same revision.
    let out = facts_cmd(&h, &["facts", "delete", "costs/total", "--revision", "2"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "delete stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("costs/total"),
        "human names path, got:\n{text}"
    );
    assert!(text.contains("revision 3"), "human revision, got:\n{text}");

    let out = facts_cmd(
        &h2,
        &[
            "facts",
            "delete",
            "costs/total",
            "--revision",
            "2",
            "--json",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["path"], serde_json::json!("costs/total"));

    // After delete the path reads missing, still at revision 3.
    let v = show_json(&h, "costs/total");
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["value"], serde_json::Value::Null);
    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(3));
    assert!(
        doc["facts"]["costs"].get("total").is_none(),
        "deleted leaf is gone, got: {doc}"
    );
}

#[test]
fn facts_create_on_existing_path_refused_value_intact() {
    let h = setup("create-existing");
    fs::create_dir_all(&h.state).unwrap();

    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "42",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));

    // Second create at the fresh revision is refused: exit nonzero,
    // names the path, and changes nothing.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "1",
            "--value",
            "99",
        ],
    )
    .output()
    .unwrap();
    assert!(
        !out.status.success(),
        "create on an existing path must fail"
    );
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("costs/total"),
        "refusal names the path, got:\n{err}"
    );

    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(1), "revision unchanged");
    assert_eq!(
        doc["facts"]["costs"]["total"],
        serde_json::json!(42),
        "stored value intact"
    );
}

#[test]
fn facts_stale_revision_rejected_naming_revisions_value_intact() {
    let h = setup("stale");
    fs::create_dir_all(&h.state).unwrap();

    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "42",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));
    // Advance to revision 2 so a presentation of 0 is stale by two.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "1",
            "--value",
            "43",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));

    // Stale set presenting revision 0 against current 2.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "99",
        ],
    )
    .output()
    .unwrap();
    assert!(!out.status.success(), "stale write must fail");
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("0") && err.contains('2'),
        "rejection names presented (0) and current (2) revisions, got:\n{err}"
    );
    assert!(
        err.to_lowercase().contains("presented") && err.to_lowercase().contains("current"),
        "rejection labels which revision is which, got:\n{err}"
    );

    // Nothing changed: prior value intact at the current revision.
    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(2));
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(43));
    assert_eq!(show_json(&h, "costs/total")["value"], serde_json::json!(43));

    // Stale delete is rejected the same way.
    let out = facts_cmd(&h, &["facts", "delete", "costs/total", "--revision", "1"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "stale delete must fail");
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains('1') && err.contains('2'),
        "delete rejection names presented (1) and current (2), got:\n{err}"
    );
    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(2));
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(43));
}

#[test]
fn facts_fresh_retry_after_conflict_advances_exactly_one() {
    let h = setup("retry");
    fs::create_dir_all(&h.state).unwrap();

    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "42",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));

    // Loser presents a stale revision and is rejected.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "99",
        ],
    )
    .output()
    .unwrap();
    assert!(!out.status.success());

    // Winner moves the document to revision 2 (simulating the
    // concurrent writer that saved first).
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "1",
            "--value",
            "43",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));

    // Loser rereads, observes revision 2, and retries: succeeds,
    // advancing the revision by exactly one to 3.
    let v = show_json(&h, "costs/total");
    assert_eq!(v["revision"], serde_json::json!(2));
    let current: u64 = v["revision"].as_u64().unwrap();
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            &current.to_string(),
            "--value",
            "100",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "fresh retry stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc = read_doc(&h);
    assert_eq!(
        doc["revision"],
        serde_json::json!(current + 1),
        "retry advances the revision by exactly one"
    );
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(100));
}

#[test]
fn facts_crash_mid_write_leaves_committed_revision_intact() {
    let h = setup("crash");
    fs::create_dir_all(&h.state).unwrap();

    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "42",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let before = fs::read(h.state.join("facts.json")).unwrap();

    // Simulated crash: the write stages its tmp file then dies before
    // the swap, exactly like a kill -9 at that instant.
    let mut cmd = facts_cmd(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "1",
            "--value",
            "99",
        ],
    );
    cmd.env("FACTORY_FACTS_CRASH", "1");
    let out = cmd.output().unwrap();
    assert!(
        !out.status.success(),
        "crashed write must not report success"
    );

    // The last fully committed revision is intact: same bytes, same
    // revision, same value; a reread agrees.
    let after = fs::read(h.state.join("facts.json")).unwrap();
    assert_eq!(
        before, after,
        "committed document byte-identical after crash"
    );
    let doc: serde_json::Value = serde_json::from_slice(&after).unwrap();
    assert_eq!(doc["revision"], serde_json::json!(1));
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(42));
    assert_eq!(show_json(&h, "costs/total")["value"], serde_json::json!(42));

    // Recovery: a fresh write at the intact revision still commits.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "1",
            "--value",
            "43",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "post-crash write stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(2));
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(43));
}
