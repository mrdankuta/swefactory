//! Status-command CLI boundary tests (ticket #24, spec #23 slice).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (ledger + train + STOP fixtures); the
//! working directory is a scratch git repo standing in for the project.
//! No daemon, no network, no real runs anywhere in this suite.
//!
//! What these tests catch: all five aggregates in prose, stable --json
//! shapes, sentence-style empty states (exit 0), clear nonzero errors
//! for missing/corrupt ledgers, and byte+mtime proof that observing
//! changes nothing.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-status-{}-{}-{}",
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

/// Seed the three project files `status` treats as project presence.
fn seed_project_files(h: &Harness) {
    fs::write(h.repo.join("AGENTS.md"), "factory\n").unwrap();
    fs::write(h.repo.join("GLOSSARY.md"), "terms\n").unwrap();
    let scripts = h.repo.join("scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(scripts.join("factory-check"), "#!/bin/sh\n").unwrap();
}

fn write_ledger(h: &Harness, attempts: serde_json::Value) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "attempts": attempts })).unwrap(),
    )
    .unwrap();
}

fn attempt(ticket: u64, id: u64, no: u64, status: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "ticket": ticket,
        "attempt_no": no,
        "base_commit": "abc123def456",
        "head_commit": "def456abc123",
        "branch": format!("factory/T{ticket}-demo"),
        "worktree": format!("/tmp/wt-T{ticket}-a{no}"),
        "status": status,
        "exit_code": 0,
        "log_path": format!("/tmp/log-T{ticket}-a{no}.log"),
        "started_at": "1700000000",
        "finished_at": "",
        "train_branch": ""
    })
}

fn status_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

/// (relative path, bytes, mtime) for every file under the state dir.
fn snapshot(state: &Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut out = Vec::new();
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
fn status_empty_state_reads_as_sentences() {
    let h = setup("empty");
    seed_project_files(&h);
    write_ledger(&h, serde_json::json!([]));

    let out = status_cmd(&h, &["status"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "prose carries version, got:\n{text}"
    );
    assert!(
        text.contains("present"),
        "prose reports project files, got:\n{text}"
    );
    assert!(
        text.contains("no open train"),
        "empty train reads as a sentence, got:\n{text}"
    );
    assert!(
        text.contains("no attempts yet"),
        "empty attempts read as a sentence, got:\n{text}"
    );
    assert!(
        text.contains("no STOP file"),
        "STOP state reported, got:\n{text}"
    );

    let out = status_cmd(&h, &["status", "--json"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["version"], serde_json::json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(v["project"]["present"], serde_json::json!(true));
    assert_eq!(v["train"]["open"], serde_json::json!(false));
    assert_eq!(v["attempts"], serde_json::json!([]));
    assert_eq!(v["stopped"], serde_json::json!(false));
}

#[test]
fn status_live_mixture_renders_all_five_aggregates() {
    let h = setup("live");
    seed_project_files(&h);
    write_ledger(
        &h,
        serde_json::json!([
            attempt(98, 2, 2, "impl_running"),
            attempt(97, 1, 1, "review_done")
        ]),
    );
    // Open train whose branch exists locally so head resolves (read-only).
    git_run(&h.repo, &["branch", "factory/integration-train"]);
    fs::write(
        h.state.join("train.json"),
        r#"{"branch":"factory/integration-train","base_commit":"abc123def456","status":"open","created_at":"1700000000"}"#,
    )
    .unwrap();
    fs::write(h.state.join("STOP"), "stopped\n").unwrap();

    let out = status_cmd(&h, &["status"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "version aggregate, got:\n{text}"
    );
    assert!(text.contains("present"), "project aggregate, got:\n{text}");
    assert!(
        text.contains("factory/integration-train"),
        "train aggregate, got:\n{text}"
    );
    assert!(
        text.contains("98") && text.contains("T98-a2"),
        "live attempt w/ slot, got:\n{text}"
    );
    assert!(
        text.contains("being implemented"),
        "Bea-wording for running attempt, got:\n{text}"
    );
    assert!(text.contains("97"), "finished attempt listed, got:\n{text}");
    assert!(text.contains("STOP"), "STOP aggregate, got:\n{text}");

    let out = status_cmd(&h, &["status", "--json"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["train"]["open"], serde_json::json!(true));
    assert_eq!(
        v["train"]["train"],
        serde_json::json!("factory/integration-train")
    );
    assert!(
        !v["train"]["head"].as_str().unwrap_or("").is_empty(),
        "train head resolves"
    );
    assert_eq!(v["stopped"], serde_json::json!(true));
    let attempts = v["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    for a in attempts {
        for field in [
            "ticket", "attempt", "slot", "status", "base", "head", "started",
        ] {
            assert!(a.get(field).is_some(), "attempt carries {field}, got: {a}");
        }
    }
    assert_eq!(attempts[0]["ticket"], serde_json::json!(98));
    assert_eq!(attempts[0]["slot"], serde_json::json!("T98-a2"));
    assert_eq!(attempts[0]["status"], serde_json::json!("impl_running"));
}

#[test]
fn status_missing_ledger_errors_nonzero() {
    let h = setup("missing");
    seed_project_files(&h);
    fs::create_dir_all(&h.state).unwrap();
    // No dispatch.json at all.

    for args in [&["status"][..], &["status", "--json"][..]] {
        let out = status_cmd(&h, args).output().unwrap();
        assert!(
            !out.status.success(),
            "missing ledger must fail, args: {args:?}"
        );
        assert_ne!(out.status.code(), Some(0));
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.to_lowercase().contains("ledger"),
            "error names the ledger, got:\n{err}"
        );
    }
}

#[test]
fn status_corrupt_ledger_errors_nonzero() {
    let h = setup("corrupt");
    seed_project_files(&h);
    fs::create_dir_all(&h.state).unwrap();
    fs::write(h.state.join("dispatch.json"), "this is not json{{{\n").unwrap();

    for args in [&["status"][..], &["status", "--json"][..]] {
        let out = status_cmd(&h, args).output().unwrap();
        assert!(
            !out.status.success(),
            "corrupt ledger must fail, args: {args:?}"
        );
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.to_lowercase().contains("ledger"),
            "error names the ledger, got:\n{err}"
        );
    }
}

#[test]
fn status_observing_changes_nothing() {
    let h = setup("readonly");
    seed_project_files(&h);
    write_ledger(
        &h,
        serde_json::json!([
            attempt(98, 2, 2, "impl_running"),
            attempt(97, 1, 1, "review_done")
        ]),
    );
    git_run(&h.repo, &["branch", "factory/integration-train"]);
    fs::write(
        h.state.join("train.json"),
        r#"{"branch":"factory/integration-train","base_commit":"abc123def456","status":"open","created_at":"1700000000"}"#,
    )
    .unwrap();
    fs::write(h.state.join("STOP"), "stopped\n").unwrap();

    let before = snapshot(&h.state);
    assert!(!before.is_empty());

    for args in [&["status"][..], &["status", "--json"][..]] {
        let out = status_cmd(&h, args).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let after = snapshot(&h.state);
    assert_eq!(
        before.len(),
        after.len(),
        "no files added or removed by status"
    );
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}
