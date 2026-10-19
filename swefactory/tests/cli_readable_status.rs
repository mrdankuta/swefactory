//! Readable status views (ticket #90).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (ledger + train + STOP fixtures); the
//! working directory is a scratch git repo standing in for the project.
//! No daemon, no network, no real runs anywhere in this suite.
//!
//! What these tests catch: `status` keeps its contract (version, project
//! files, open train, live attempts, loop state) with terminal history
//! collapsed into a counts + most-recent-8 summary; `dispatch status`
//! renders an aligned table without internal ids, bounded to live plus
//! recent with a `--all` opt-in, plus a totals line; the STOP line reads
//! as loop state in both; `--json` keeps full fidelity for machines.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-readable-{}-{}-{}",
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

fn seed_project_files(h: &Harness) {
    fs::write(h.repo.join("AGENTS.md"), "factory\n").unwrap();
    fs::write(h.repo.join("GLOSSARY.md"), "terms\n").unwrap();
    let scripts = h.repo.join("scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(scripts.join("factory-check"), "#!/bin/sh\n").unwrap();
}

fn attempt_full(ticket: u64, id: u64, no: u64, status: &str, finished: &str) -> serde_json::Value {
    let phase = if status.starts_with("impl") {
        "implement"
    } else if status.starts_with("review") {
        "review"
    } else {
        ""
    };
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
        "finished_at": finished,
        "train_branch": "",
        "phase": phase
    })
}

fn write_ledger(h: &Harness, attempts: serde_json::Value) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "attempts": attempts })).unwrap(),
    )
    .unwrap();
}

fn factory_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

fn human(h: &Harness, args: &[&str]) -> String {
    let out = factory_cmd(h, args).output().unwrap();
    assert!(
        out.status.success(),
        "args {args:?} failed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn machine(h: &Harness, args: &[&str]) -> serde_json::Value {
    let out = factory_cmd(h, args).output().unwrap();
    assert!(
        out.status.success(),
        "args {args:?} failed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap()
}

#[test]
fn status_empty_stays_calm_with_loop_line() {
    let h = setup("empty");
    seed_project_files(&h);
    write_ledger(&h, serde_json::json!([]));

    let text = human(&h, &["status"]);
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "version, got:\n{text}"
    );
    assert!(text.contains("present"), "project files, got:\n{text}");
    assert!(text.contains("no open train"), "train, got:\n{text}");
    assert!(
        text.contains("attempts: no attempts yet."),
        "calm empty attempts, got:\n{text}"
    );
    assert!(
        text.contains("history: (none)"),
        "calm empty history, got:\n{text}"
    );
    assert!(
        text.contains("loop: running"),
        "loop state when no STOP file, got:\n{text}"
    );
    assert!(!text.contains("STOP:"), "no legacy STOP line, got:\n{text}");
    assert!(
        !text.contains("no STOP file"),
        "no double-negative STOP copy, got:\n{text}"
    );

    let v = machine(&h, &["status", "--json"]);
    assert_eq!(v["version"], serde_json::json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(v["project"]["present"], serde_json::json!(true));
    assert_eq!(v["train"]["open"], serde_json::json!(false));
    assert_eq!(v["attempts"], serde_json::json!([]));
    assert_eq!(v["stopped"], serde_json::json!(false));
}

#[test]
fn status_lists_live_and_summarizes_terminal_history() {
    let h = setup("mixed");
    seed_project_files(&h);
    write_ledger(
        &h,
        serde_json::json!([
            attempt_full(97, 1, 1, "review_done", "1700000001"),
            attempt_full(96, 2, 1, "implement_interrupted", ""),
            attempt_full(98, 3, 2, "impl_running", ""),
        ]),
    );
    git_run(&h.repo, &["branch", "factory/integration-train"]);
    fs::write(
        h.state.join("train.json"),
        r#"{"branch":"factory/integration-train","base_commit":"abc123def456","status":"open","created_at":"1700000000"}"#,
    )
    .unwrap();
    fs::write(h.state.join("STOP"), "stopped\n").unwrap();

    let text = human(&h, &["status"]);
    // Live attempts keep their Bea sentences with slots.
    assert!(
        text.contains("being implemented") && text.contains("T98-a2"),
        "live running row, got:\n{text}"
    );
    assert!(
        text.contains("interrupted during implement") && text.contains("resumable"),
        "live interrupted row, got:\n{text}"
    );
    // Terminal history is summarized, never dumped per-attempt.
    assert!(
        !text.contains("Ticket 97 attempt 1 finished: review_done"),
        "no full per-attempt dump, got:\n{text}"
    );
    assert!(
        text.contains("review_done x1"),
        "counts by terminal state, got:\n{text}"
    );
    assert!(
        text.lines()
            .any(|l| l.contains("T97-a1") && l.contains("review_done")),
        "recent compact row, got:\n{text}"
    );
    assert!(
        text.contains("factory log <ticket>"),
        "per-ticket history hint, got:\n{text}"
    );
    assert!(
        text.contains("loop: STOPPED"),
        "halted loop reads as loop state, got:\n{text}"
    );
    assert!(!text.contains("STOP:"), "no legacy STOP line, got:\n{text}");

    // Machines see zero change: every row, every field.
    let v = machine(&h, &["status", "--json"]);
    let attempts = v["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 3);
    assert_eq!(v["stopped"], serde_json::json!(true));
    assert_eq!(attempts[0]["ticket"], serde_json::json!(97));
    assert_eq!(attempts[0]["status"], serde_json::json!("review_done"));
    assert_eq!(attempts[2]["slot"], serde_json::json!("T98-a2"));
}

#[test]
fn status_history_bounds_recent_to_eight() {
    let h = setup("many-terminal");
    seed_project_files(&h);
    let mut rows = Vec::new();
    for i in 1..=12u64 {
        let status = match i % 3 {
            0 => "review_done",
            1 => "impl_failed",
            _ => "review_failed",
        };
        rows.push(attempt_full(90 + i, i, 1, status, "1700000001"));
    }
    write_ledger(&h, serde_json::Value::Array(rows));

    let text = human(&h, &["status"]);
    assert!(
        text.contains("12 finished"),
        "summary covers all terminal attempts, got:\n{text}"
    );
    assert!(text.contains("review_done x4"), "counts, got:\n{text}");
    let recent: Vec<&str> = text.lines().filter(|l| l.starts_with("  T")).collect();
    assert_eq!(recent.len(), 8, "recent bounded to 8, got:\n{text}");
    assert!(
        recent[0].contains("T102-a1"),
        "newest first, got:\n{recent:?}"
    );
    assert!(
        !text.contains("T91-a1"),
        "oldest terminal row bounded out, got:\n{text}"
    );
    assert!(text.contains("loop: running"), "got:\n{text}");

    let v = machine(&h, &["status", "--json"]);
    assert_eq!(v["attempts"].as_array().unwrap().len(), 12);
}

#[test]
fn dispatch_status_table_bounded_with_all_opt_in() {
    let h = setup("dispatch-bounded");
    seed_project_files(&h);
    let mut rows = Vec::new();
    rows.push(attempt_full(98, 11, 1, "impl_running", ""));
    rows.push(attempt_full(99, 12, 1, "review_running", ""));
    for i in 1..=10u64 {
        rows.push(attempt_full(80 + i, 20 + i, 1, "review_done", "1700000001"));
    }
    write_ledger(&h, serde_json::Value::Array(rows));

    let text = human(&h, &["dispatch", "status"]);
    assert!(
        text.contains("TICKET") && text.contains("STATUS"),
        "aligned-column header, got:\n{text}"
    );
    assert!(
        !text.lines().any(|l| l.trim_start().starts_with('#')),
        "no internal ledger ids, got:\n{text}"
    );
    assert!(!text.contains("base="), "no unaligned dump, got:\n{text}");
    assert!(!text.contains("head="), "no unaligned dump, got:\n{text}");
    let data_rows: Vec<&str> = text
        .lines()
        .filter(|l| {
            l.split_whitespace()
                .next()
                .is_some_and(|w| w.parse::<u64>().is_ok())
        })
        .collect();
    assert_eq!(
        data_rows.len(),
        2 + 8,
        "live plus most recent 8, got:\n{text}"
    );
    assert!(text.contains("total:"), "totals line, got:\n{text}");
    assert!(
        text.contains("loop: running"),
        "loop state on the totals line, got:\n{text}"
    );
    assert!(
        text.contains("--all"),
        "full-history opt-in hint, got:\n{text}"
    );
    assert!(
        !text.contains("STOPPED (") && !text.contains("no STOP file"),
        "renamed STOP line, got:\n{text}"
    );

    let full = human(&h, &["dispatch", "status", "--all"]);
    let full_rows: Vec<&str> = full
        .lines()
        .filter(|l| {
            l.split_whitespace()
                .next()
                .is_some_and(|w| w.parse::<u64>().is_ok())
        })
        .collect();
    assert_eq!(full_rows.len(), 12, "opt-in shows everything, got:\n{full}");
    assert!(
        !full.lines().any(|l| l.trim_start().starts_with('#')),
        "no ids even with --all, got:\n{full}"
    );

    // Machines see zero change with or without --all.
    for args in [
        vec!["--json", "dispatch", "status"],
        vec!["--json", "dispatch", "status", "--all"],
    ] {
        let v = machine(&h, &args);
        let attempts = v["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 12, "json keeps all rows: {args:?}");
        for field in [
            "id",
            "ticket",
            "attempt_no",
            "status",
            "base_commit",
            "head_commit",
        ] {
            assert!(
                attempts[0].get(field).is_some(),
                "row keeps {field}: {args:?}"
            );
        }
        assert_eq!(v["stopped"], serde_json::json!(false));
    }
}

#[test]
fn dispatch_status_empty_and_stopped_read_calm() {
    let h = setup("dispatch-empty");
    seed_project_files(&h);
    write_ledger(&h, serde_json::json!([]));

    let text = human(&h, &["dispatch", "status"]);
    assert!(text.contains("(none)"), "calm empty, got:\n{text}");
    assert!(text.contains("total:"), "totals line, got:\n{text}");
    assert!(text.contains("loop: running"), "got:\n{text}");

    fs::write(h.state.join("STOP"), "stopped\n").unwrap();
    let text = human(&h, &["dispatch", "status"]);
    assert!(text.contains("loop: STOPPED"), "got:\n{text}");
    assert!(!text.contains("STOP:"), "got:\n{text}");
    let v = machine(&h, &["--json", "dispatch", "status"]);
    assert_eq!(v["attempts"], serde_json::json!([]));
    assert_eq!(v["stopped"], serde_json::json!(true));
}

#[test]
fn dispatch_status_keeps_interrupted_and_memo_copy() {
    let h = setup("dispatch-copy");
    seed_project_files(&h);
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        r#"{"attempts":[
{"id":1,"ticket":67,"attempt_no":1,"base_commit":"b","head_commit":"","branch":"factory/T67-x","worktree":"","status":"implement_interrupted","exit_code":0,"log_path":"","started_at":"s","finished_at":"","phase":"implement","resumed":1}],
"decision_memos":[{"ticket":67,"attempt":1,"key":"approve-deploy","decision":"allow","stored_at":"s"}]}"#,
    )
    .unwrap();

    let text = human(&h, &["dispatch", "status"]);
    assert!(
        text.contains("implement_interrupted") || text.contains("interrupted during implement"),
        "names the interrupted phase, got:\n{text}"
    );
    assert!(
        !text.contains("failed"),
        "distinct from failed, got:\n{text}"
    );
    assert!(text.contains("resumed"), "resume count, got:\n{text}");
    assert!(
        text.contains("approve-deploy=allow"),
        "stored decision, got:\n{text}"
    );
}
