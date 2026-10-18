//! Facts-read CLI boundary tests (ticket #77, spec #64).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (a `facts.json` beside the dispatch
//! ledger, or no document at all); the working directory is a scratch
//! git repo standing in for the project. No daemon, no network, no
//! writes anywhere here.
//!
//! What these tests catch: absent-document reads as empty at revision
//! zero in both modes with identical facts; whole-document and per-path
//! reads agreeing across human and machine modes for seeded content;
//! missing paths stated plainly (never zero-filled, never error-shaped);
//! and byte+mtime proof that reading changes no ledger, log, worktree,
//! or facts file.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-facts-{}-{}-{}",
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

/// Seed a facts document beside the dispatch ledger.
fn write_facts(h: &Harness, revision: u64, facts: serde_json::Value) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("facts.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "revision": revision,
            "facts": facts,
        }))
        .unwrap(),
    )
    .unwrap();
}

fn seed_ledger_and_logs(h: &Harness) {
    fs::create_dir_all(h.state.join("logs")).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "attempts": [] })).unwrap(),
    )
    .unwrap();
    fs::write(h.state.join("logs").join("T98-a1-impl.log"), "evidence\n").unwrap();
}

fn seed_fixture_content(h: &Harness) {
    write_facts(
        h,
        3,
        serde_json::json!({
            "costs": { "total": 42, "currency": "USD" },
            "verdicts": { "T77": "accept" },
        }),
    );
}

fn facts_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

/// (relative path, bytes, mtime) for every file under the state dir.
fn snapshot(state: &Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut out = Vec::new();
    if !state.is_dir() {
        return out;
    }
    let mut stack = vec![state.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries: Vec<_> = fs::read_dir(&dir).map(|r| r.collect()).unwrap_or_default();
        for e in entries {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p
                    .strip_prefix(state)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let bytes = fs::read(&p).unwrap();
                let mtime = fs::metadata(&p).unwrap().modified().unwrap();
                out.push((rel, bytes, mtime));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn facts_absent_document_reads_empty_at_revision_zero() {
    let h = setup("absent");
    fs::create_dir_all(&h.state).unwrap();
    // No facts.json at all (and no ledger either: reads need no ledger).

    let out = facts_cmd(&h, &["facts", "show"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "absent document still exits 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("revision 0"),
        "human names revision zero, got:\n{text}"
    );
    assert!(
        text.to_lowercase().contains("empty"),
        "human states empty, got:\n{text}"
    );

    let out = facts_cmd(&h, &["facts", "show", "--json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(0));
    assert_eq!(v["facts"], serde_json::json!({}));

    // Identical facts in both modes: same revision, same (empty) value.
    assert!(
        text.contains("revision 0"),
        "human and machine agree on revision 0"
    );

    // Reading an absent document creates nothing (no migration).
    assert!(
        !h.state.join("facts.json").is_file(),
        "absent document stays absent after reads"
    );
}

#[test]
fn facts_whole_and_path_reads_agree_across_modes() {
    let h = setup("parity");
    seed_ledger_and_logs(&h);
    seed_fixture_content(&h);

    // Whole document: human carries the same revision and values.
    let out = facts_cmd(&h, &["facts", "show"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("revision 3"), "human revision, got:\n{text}");
    assert!(text.contains("42"), "human value, got:\n{text}");
    assert!(text.contains("accept"), "human value, got:\n{text}");

    let out = facts_cmd(&h, &["facts", "show", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["facts"]["costs"]["total"], serde_json::json!(42));
    assert_eq!(v["facts"]["verdicts"]["T77"], serde_json::json!("accept"));

    // Per-path read: same value plus same revision in both modes.
    let out = facts_cmd(&h, &["facts", "show", "costs/total"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("costs/total"),
        "human names the path, got:\n{text}"
    );
    assert!(text.contains("revision 3"), "human revision, got:\n{text}");
    assert!(text.contains("42"), "human value, got:\n{text}");

    let out = facts_cmd(&h, &["facts", "show", "costs/total", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["path"], serde_json::json!("costs/total"));
    assert_eq!(v["value"], serde_json::json!(42));

    // Nested-object path reads the subtree, not a zero-fill.
    let out = facts_cmd(&h, &["facts", "show", "verdicts", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["value"], serde_json::json!({"T77": "accept"}));
}

#[test]
fn facts_missing_path_stated_plainly_never_zero_filled() {
    let h = setup("missing");
    seed_fixture_content(&h);

    let out = facts_cmd(&h, &["facts", "show", "costs/missing"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "missing path is not error-shaped (exit 0), stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "missing path writes nothing to stderr, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("costs/missing"),
        "human names the missing path, got:\n{text}"
    );
    assert!(
        text.to_lowercase().contains("no value set"),
        "human states the miss plainly, got:\n{text}"
    );
    assert!(text.contains("revision 3"), "human revision, got:\n{text}");

    let out = facts_cmd(&h, &["facts", "show", "costs/missing", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "machine miss writes nothing to stderr, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["path"], serde_json::json!("costs/missing"));
    assert_eq!(
        v["value"],
        serde_json::Value::Null,
        "missing reads as null, never zero-filled, got: {v}"
    );
    assert_ne!(
        v["value"],
        serde_json::json!(0),
        "missing is never the number zero, got: {v}"
    );

    // A missing top-level path on an absent document behaves the same.
    let h2 = setup("missing-absent");
    fs::create_dir_all(&h2.state).unwrap();
    let out = facts_cmd(&h2, &["facts", "show", "costs/total", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(0));
    assert_eq!(v["value"], serde_json::Value::Null);
}

#[test]
fn facts_corrupt_document_errors_naming_facts() {
    let h = setup("corrupt");
    fs::create_dir_all(&h.state).unwrap();
    fs::write(h.state.join("facts.json"), "this is not json{{{\n").unwrap();

    for args in [
        &["facts", "show"][..],
        &["facts", "show", "--json"][..],
        &["facts", "show", "costs/total"][..],
    ] {
        let out = facts_cmd(&h, args).output().unwrap();
        assert!(
            !out.status.success(),
            "corrupt facts document must fail: {args:?}"
        );
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.to_lowercase().contains("facts"),
            "error names the facts document, got:\n{err}"
        );
    }
}

#[test]
fn facts_reading_changes_nothing() {
    let h = setup("readonly");
    seed_ledger_and_logs(&h);
    seed_fixture_content(&h);

    let before = snapshot(&h.state);
    assert!(before.len() >= 3, "ledger + log + facts seeded");

    for args in [
        vec!["facts", "show"],
        vec!["facts", "show", "--json"],
        vec!["facts", "show", "costs/total"],
        vec!["facts", "show", "costs/total", "--json"],
        vec!["facts", "show", "verdicts"],
        vec!["facts", "show", "nope/missing"],
        vec!["facts", "show", "nope/missing", "--json"],
    ] {
        let out = facts_cmd(&h, &args).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "args {args:?} stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let after = snapshot(&h.state);
    assert_eq!(
        before.len(),
        after.len(),
        "no files added or removed by facts reads"
    );
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}
