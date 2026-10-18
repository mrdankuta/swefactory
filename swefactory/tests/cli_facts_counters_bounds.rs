//! Counters, bounds, and hardening CLI boundary tests (ticket #79,
//! spec #64).
//!
//! Seam: argv in -> exit code + stdout/stderr out + observable
//! state-directory contents. State comes from a seeded
//! `FACTORY_STATE_DIR`; the working directory is a scratch git repo
//! standing in for the project. No daemon, no network, no issue-tracker
//! calls.
//!
//! What these tests catch: increment accumulating across successive
//! writes with exact totals in both modes; increment on a non-numeric
//! path refused with the stored value intact; oversized writes refused
//! with the documented cap stated and nothing written; malformed paths
//! rejected showing the accepted shape with no write landing anywhere;
//! conflicts rendering as calm sentences in human output while machine
//! output keeps the exact revisions.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-facts-bounds-{}-{}-{}",
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
fn facts_increment_accumulates_exact_totals_both_modes() {
    let h = setup("increment");
    fs::create_dir_all(&h.state).unwrap();

    // Missing path starts from zero: 0 + 5 at revision zero -> 1.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "increment",
            "costs/total",
            "--revision",
            "0",
            "--by",
            "5",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "increment stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("costs/total"), "human names path:\n{text}");
    assert!(text.contains("revision 1"), "human revision:\n{text}");
    assert!(text.contains('5'), "human total:\n{text}");

    // Second increment accumulates: 5 + 7 -> 12 at revision 2.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "increment",
            "costs/total",
            "--revision",
            "1",
            "--by",
            "7",
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
    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(2));
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(12));

    // Machine mode carries the identical total and revision.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "increment",
            "costs/total",
            "--revision",
            "2",
            "--by",
            "1",
            "--json",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["path"], serde_json::json!("costs/total"));
    assert_eq!(v["value"], serde_json::json!(13));

    // Reads agree in both modes: exact total, same revision.
    assert_eq!(show_json(&h, "costs/total")["value"], serde_json::json!(13));
    assert_eq!(
        show_json(&h, "costs/total")["revision"],
        serde_json::json!(3)
    );
}

#[test]
fn facts_increment_on_non_numeric_refused_value_intact() {
    let h = setup("increment-nonnumeric");
    fs::create_dir_all(&h.state).unwrap();

    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "verdicts/T79",
            "--revision",
            "0",
            "--value",
            "\"accept\"",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));

    let out = facts_cmd(
        &h,
        &[
            "facts",
            "increment",
            "verdicts/T79",
            "--revision",
            "1",
            "--by",
            "1",
        ],
    )
    .output()
    .unwrap();
    assert!(!out.status.success(), "increment on text must fail");
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("verdicts/T79"),
        "refusal names the path, got:\n{err}"
    );
    assert!(
        err.to_lowercase().contains("not a number"),
        "refusal says why, got:\n{err}"
    );

    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(1), "revision unchanged");
    assert_eq!(
        doc["facts"]["verdicts"]["T79"],
        serde_json::json!("accept"),
        "stored value intact"
    );
}

#[test]
fn facts_oversized_write_refused_cap_stated_never_truncated() {
    let h = setup("oversized");
    fs::create_dir_all(&h.state).unwrap();

    // A value just over the 65536-byte documented cap.
    let big = format!("\"{}\"", "x".repeat(70_000));
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "big/blob",
            "--revision",
            "0",
            "--value",
            &big,
        ],
    )
    .output()
    .unwrap();
    assert!(!out.status.success(), "oversized write must fail");
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("65536"),
        "refusal states the documented cap, got:\n{err}"
    );
    assert!(
        err.to_lowercase().contains("nothing changed"),
        "refusal promises nothing changed, got:\n{err}"
    );

    // Nothing landed and nothing was truncated into place.
    let doc_text = fs::read_to_string(h.state.join("facts.json")).unwrap_or_default();
    assert!(
        !h.state.join("facts.json").is_file() || !doc_text.contains('x'),
        "no truncated value may land, got {doc_text:.120}"
    );

    // Machine mode states the cap structurally too.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "big/blob",
            "--revision",
            "0",
            "--value",
            &big,
            "--json",
        ],
    )
    .output()
    .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("65536"), "human refusal states cap:\n{err}");
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["cap"], serde_json::json!(65536));

    // The document still accepts a normal write at revision zero.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "create",
            "ok/path",
            "--revision",
            "0",
            "--value",
            "1",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "post-refusal write stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(read_doc(&h)["revision"], serde_json::json!(1));
}

#[test]
fn facts_malformed_paths_rejected_showing_accepted_shape() {
    let h = setup("badpath");
    fs::create_dir_all(&h.state).unwrap();

    for bad in [
        "/leading",
        "trailing/",
        "doubled//slash",
        "bad chars",
        "bang!",
        "dot/./seg",
        "",
    ] {
        let out = facts_cmd(
            &h,
            &["facts", "create", bad, "--revision", "0", "--value", "1"],
        )
        .output()
        .unwrap();
        assert!(!out.status.success(), "malformed path {bad:?} must fail");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.contains("costs/total"),
            "rejection shows the accepted shape for {bad:?}, got:\n{err}"
        );
    }

    // No write landed at any surprise location: no document at all.
    assert!(
        !h.state.join("facts.json").is_file(),
        "malformed writes create nothing"
    );

    // Reads of malformed paths are rejected too, never treated as missing.
    let out = facts_cmd(&h, &["facts", "show", "doubled//slash"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "malformed read must fail");
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("costs/total"),
        "read rejection shows the accepted shape, got:\n{err}"
    );
}

#[test]
fn facts_conflict_calm_human_exact_machine() {
    let h = setup("conflict-tone");
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

    // Human conflict: a calm sentence naming the path and both revisions.
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
        err.contains("someone else saved"),
        "conflict reads calm, got:\n{err}"
    );
    assert!(
        err.contains("costs/total"),
        "conflict names the path, got:\n{err}"
    );
    assert!(
        err.contains('0') && err.contains('2'),
        "conflict names presented (0) and current (2), got:\n{err}"
    );

    // Machine conflict: the exact revisions structurally.
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
            "--json",
        ],
    )
    .output()
    .unwrap();
    assert!(!out.status.success());
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["path"], serde_json::json!("costs/total"));
    assert_eq!(v["presented"], serde_json::json!(0));
    assert_eq!(v["current"], serde_json::json!(2));

    // Nothing changed by either refusal.
    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(2));
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(43));
}

#[test]
fn facts_increment_under_stale_revision_refused_calmly() {
    let h = setup("increment-stale");
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
            "10",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));

    // Presenting revision 0 after the create moved the document to
    // revision 1 is stale: refused calmly, value intact.
    let out = facts_cmd(
        &h,
        &[
            "facts",
            "increment",
            "costs/total",
            "--revision",
            "0",
            "--by",
            "5",
        ],
    )
    .output()
    .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("someone else saved"),
        "stale increment reads calm, got:\n{err}"
    );
    let doc = read_doc(&h);
    assert_eq!(doc["revision"], serde_json::json!(1));
    assert_eq!(doc["facts"]["costs"]["total"], serde_json::json!(10));
}
