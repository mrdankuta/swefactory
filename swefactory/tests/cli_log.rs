//! Log-command CLI boundary tests (ticket #25, spec #23 slice).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (ledger + canned transcripts under
//! state/logs/); the working directory is a scratch git repo standing in
//! for the project. No daemon, no network, no real runs anywhere here.
//!
//! What these tests catch: newest-first ordering with outcome summaries
//! before transcripts, --tail bounding (default 50, 0 means full),
//! stable --json shapes, Bea-readable failure cause before evidence, and
//! byte+mtime proof that observing changes nothing.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-log-{}-{}-{}",
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

fn write_ledger(h: &Harness, attempts: serde_json::Value) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "attempts": attempts })).unwrap(),
    )
    .unwrap();
}

/// Ledger attempt whose log_path points at a real file under the state dir.
fn attempt_with_log(
    h: &Harness,
    ticket: u64,
    id: u64,
    no: u64,
    status: &str,
    exit_code: i32,
) -> serde_json::Value {
    let log_path = h
        .state
        .join("logs")
        .join(format!("T{ticket}-a{no}-impl.log"));
    serde_json::json!({
        "id": id,
        "ticket": ticket,
        "attempt_no": no,
        "base_commit": "abc123def456",
        "head_commit": "def456abc123",
        "branch": format!("factory/T{ticket}-demo"),
        "worktree": format!("/tmp/wt-T{ticket}-a{no}"),
        "status": status,
        "exit_code": exit_code,
        "log_path": log_path.to_string_lossy(),
        "started_at": "1700000000",
        "finished_at": "1700000600",
        "train_branch": ""
    })
}

fn write_impl_log(h: &Harness, ticket: u64, no: u64, body: &str) {
    let dir = h.state.join("logs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("T{ticket}-a{no}-impl.log")), body).unwrap();
}

fn numbered_lines(prefix: &str, n: usize) -> String {
    (1..=n)
        .map(|i| format!("{prefix} line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn log_cmd(h: &Harness, args: &[&str]) -> Command {
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

fn seed_two_attempts(h: &Harness) {
    // Stored oldest-first on purpose: output must still be newest-first.
    write_ledger(
        h,
        serde_json::json!([
            attempt_with_log(h, 98, 1, 1, "review_done", 0),
            attempt_with_log(h, 98, 2, 2, "impl_failed", 1),
        ]),
    );
    write_impl_log(h, 98, 1, "a1 line 1\na1 line 2\na1 line 3\n");
    write_impl_log(h, 98, 2, "a2 line 1\na2 line 2\na2 line 3\n");
}

#[test]
fn log_orders_newest_first_with_summaries_before_transcripts() {
    let h = setup("ordering");
    seed_two_attempts(&h);

    let out = log_cmd(&h, &["log", "98"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let pos_a2 = text.find("Attempt 2").expect("attempt 2 header");
    let pos_a1 = text.find("Attempt 1").expect("attempt 1 header");
    assert!(pos_a2 < pos_a1, "newest first, got:\n{text}");
    // Outcome summary precedes the transcript within attempt 2's section.
    let section = &text[pos_a2..];
    let summary = section.find("failed").expect("outcome summary");
    let evidence = section.find("a2 line 1").expect("transcript evidence");
    assert!(
        summary < evidence,
        "summary before transcript, got:\n{text}"
    );
    // Base/head + timestamps travel with each entry.
    assert!(text.contains("abc123def456"), "base shown, got:\n{text}");
    assert!(text.contains("def456abc123"), "head shown, got:\n{text}");
}

#[test]
fn log_tail_bounds_output_and_zero_means_full() {
    let h = setup("tail");
    write_ledger(
        &h,
        serde_json::json!([attempt_with_log(&h, 98, 1, 1, "review_done", 0)]),
    );
    write_impl_log(&h, 98, 1, &numbered_lines("tail", 60));

    // Default bounds to DEFAULT_TAIL lines.
    let out = log_cmd(&h, &["log", "98"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !text.contains("tail line 1\n"),
        "default tail drops the oldest line, got tail head:\n{}",
        text.lines().take(8).collect::<Vec<_>>().join("\n")
    );
    assert!(text.contains("tail line 60"), "newest line kept");

    // Explicit small tail keeps only the last N lines.
    let out = log_cmd(&h, &["log", "98", "--tail", "2"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("tail line 60"), "newest kept, got:\n{text}");
    assert!(
        text.contains("tail line 59"),
        "second-newest kept, got:\n{text}"
    );
    assert!(
        !text.contains("tail line 58\n"),
        "third-newest dropped, got:\n{text}"
    );

    // --tail 0 means the full transcript.
    let out = log_cmd(&h, &["log", "98", "--tail", "0"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("tail line 1"),
        "full transcript, got head only"
    );
    assert!(text.contains("tail line 60"), "full transcript tail");

    // --json honours the same bound.
    let out = log_cmd(&h, &["log", "98", "--tail", "2", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let transcript = v["attempts"][0]["transcript"].as_str().unwrap();
    assert!(transcript.contains("tail line 60"), "json tail newest");
    assert!(!transcript.contains("tail line 58\n"), "json tail bounded");
    assert_eq!(v["attempts"][0]["truncated"], serde_json::json!(true));
}

#[test]
fn log_json_stable_shapes() {
    let h = setup("json");
    seed_two_attempts(&h);

    let out = log_cmd(&h, &["log", "98", "--json"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["ticket"], serde_json::json!(98));
    let attempts = v["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["attempt"], serde_json::json!(2));
    assert_eq!(attempts[1]["attempt"], serde_json::json!(1));
    for a in attempts {
        for field in [
            "ticket",
            "attempt",
            "slot",
            "status",
            "summary",
            "base",
            "head",
            "started",
            "finished",
            "exit_code",
            "transcript",
            "truncated",
        ] {
            assert!(a.get(field).is_some(), "attempt carries {field}, got: {a}");
        }
    }
    assert_eq!(attempts[0]["slot"], serde_json::json!("T98-a2"));
    assert_eq!(attempts[0]["status"], serde_json::json!("impl_failed"));
}

#[test]
fn log_failure_entries_summarize_cause_before_evidence() {
    let h = setup("failures");
    write_ledger(
        &h,
        serde_json::json!([
            attempt_with_log(&h, 98, 1, 1, "review_failed", 0),
            attempt_with_log(&h, 98, 2, 2, "impl_failed", 1),
        ]),
    );
    write_impl_log(&h, 98, 1, "review transcript evidence\n");
    write_impl_log(&h, 98, 2, "implement transcript evidence\n");

    let out = log_cmd(&h, &["log", "98"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let pos_a2 = text.find("Attempt 2").expect("attempt 2");
    let cause_a2 = text[pos_a2..].find("failed").expect("cause summary");
    let evidence_a2 = text[pos_a2..]
        .find("implement transcript evidence")
        .expect("evidence");
    assert!(
        cause_a2 < evidence_a2,
        "cause before evidence, got:\n{text}"
    );
    assert!(
        text.to_lowercase().contains("failed"),
        "Bea-readable failure words, got:\n{text}"
    );

    let out = log_cmd(&h, &["log", "98", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    for a in v["attempts"].as_array().unwrap() {
        let summary = a["summary"].as_str().unwrap();
        assert!(
            summary.to_lowercase().contains("fail"),
            "json summary names the failure, got: {summary}"
        );
    }
}

#[test]
fn log_unknown_ticket_reads_as_sentence() {
    let h = setup("empty-ticket");
    write_ledger(
        &h,
        serde_json::json!([attempt_with_log(&h, 97, 1, 1, "review_done", 0)]),
    );
    write_impl_log(&h, 97, 1, "other ticket\n");

    let out = log_cmd(&h, &["log", "98"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "empty ticket still exits 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("98") && text.to_lowercase().contains("no attempts"),
        "empty state reads as a sentence, got:\n{text}"
    );

    let out = log_cmd(&h, &["log", "98", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["ticket"], serde_json::json!(98));
    assert_eq!(v["attempts"], serde_json::json!([]));
}

#[test]
fn log_missing_and_corrupt_ledger_error_nonzero() {
    let h = setup("missing");
    fs::create_dir_all(&h.state).unwrap();
    // No dispatch.json at all.
    for args in [&["log", "98"][..], &["log", "98", "--json"][..]] {
        let out = log_cmd(&h, args).output().unwrap();
        assert!(!out.status.success(), "missing ledger must fail: {args:?}");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.to_lowercase().contains("ledger"),
            "error names the ledger, got:\n{err}"
        );
    }

    let h = setup("corrupt");
    fs::create_dir_all(&h.state).unwrap();
    fs::write(h.state.join("dispatch.json"), "this is not json{{{\n").unwrap();
    for args in [&["log", "98"][..], &["log", "98", "--json"][..]] {
        let out = log_cmd(&h, args).output().unwrap();
        assert!(!out.status.success(), "corrupt ledger must fail: {args:?}");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.to_lowercase().contains("ledger"),
            "error names the ledger, got:\n{err}"
        );
    }
}

#[test]
fn log_observing_changes_nothing_in_all_modes() {
    let h = setup("readonly");
    seed_two_attempts(&h);

    let before = snapshot(&h.state);
    assert!(!before.is_empty());

    for args in [
        vec!["log", "98"],
        vec!["log", "98", "--tail", "2"],
        vec!["log", "98", "--json"],
        vec!["log", "98", "--tail", "0", "--json"],
    ] {
        let out = log_cmd(&h, &args).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "args {args:?} stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // Filenames with a `#` prefix resolve the same ticket.
    let out = log_cmd(&h, &["log", "#98"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));

    let after = snapshot(&h.state);
    assert_eq!(
        before.len(),
        after.len(),
        "no files added or removed by log"
    );
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}
