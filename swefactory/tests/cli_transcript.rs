//! Transcript-view CLI boundary tests (ticket #93).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (ledger with one attempt whose worktree is
//! an absolute scratch path) plus a fixture agent-sessions root behind
//! `FACTORY_PI_SESSIONS_DIR` (one dir, one session file whose recorded
//! cwd matches the worktree). No daemon, no network, no real runs.
//!
//! What these tests catch: voice rendering (assistant turns, tool calls
//! with names, results distinctly), item bounding (default 20, explicit
//! tail, 0 means full), `--json` parity, calm missing-session reads,
//! partial corrupt files, and the --fork/--transcript refusal.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-transcript-{}-{}-{}",
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
    sessions: PathBuf,
    worktree: PathBuf,
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_run(&repo, &["init", "-b", "dev"]);
    let state = root.join("state");
    let worktree = state.join("worktrees").join("T9-a1");
    fs::create_dir_all(&worktree).unwrap();
    Harness {
        repo,
        state,
        sessions: root.join("sessions"),
        worktree,
    }
}

fn write_ledger(h: &Harness) {
    fs::create_dir_all(&h.state).unwrap();
    let wt = h.worktree.display().to_string();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "attempts": [{
            "id": 1, "ticket": 9, "attempt_no": 1,
            "base_commit": "abc", "branch": "factory/T9-x",
            "worktree": wt, "status": "impl_running",
            "log_path": "", "started_at": "2026-10-19 00:00:00",
            "finished_at": ""
        }] }))
        .unwrap(),
    )
    .unwrap();
}

/// One session dir holding one file whose recorded cwd matches the
/// attempt worktree. `rows` are appended verbatim after the header.
fn write_session(h: &Harness, rows: &[&str]) {
    let dir = h.sessions.join("worktree-T9-a1");
    fs::create_dir_all(&dir).unwrap();
    let cwd = h
        .worktree
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let mut text = format!(
        "{{\"type\":\"session\",\"version\":3,\"id\":\"s1\",\"timestamp\":\"2026-10-19T00:00:00Z\",\"cwd\":\"{cwd}\"}}\n"
    );
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    fs::write(dir.join("2026-10-19T00-00-00-000Z_s1.jsonl"), text).unwrap();
}

fn transcript_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env("FACTORY_PI_SESSIONS_DIR", &h.sessions)
        .env_remove("FACTORY_SLOT");
    cmd
}

const ROWS: &[&str] = &[
    r#"{"type":"message","id":"m1","message":{"role":"user","content":[{"type":"text","text":"Fix the flaky test."}]}}"#,
    r#"{"type":"message","id":"m2","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Race, not logic. Verify first."},{"type":"text","text":"Diagnosing the timing failure."},{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"AGENTS.md"}}]}}"#,
    r#"{"type":"message","id":"m3","message":{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"file contents here"}]}}"#,
    r#"{"type":"message","id":"m4","message":{"role":"assistant","content":[{"type":"text","text":"HANDOFF written."}]}}"#,
];

#[test]
fn transcript_renders_voice_distinctly() {
    let h = setup("voice");
    write_ledger(&h);
    write_session(&h, ROWS);
    let assert = transcript_cmd(&h, &["log", "9", "--transcript"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    for marker in [
        "[user]",
        "Fix the flaky test.",
        "[agent]",
        "Diagnosing the timing failure.",
        "[thinking]",
        "Race, not logic.",
        "[tool read(",
        "AGENTS.md",
        "[result read]",
        "file contents here",
        "HANDOFF written.",
    ] {
        assert!(stdout.contains(marker), "missing {marker}:\n{stdout}");
    }
    // Transcript order, not file order scrambled: brief first, handoff last.
    assert!(
        stdout.find("Fix the flaky test.").unwrap() < stdout.find("HANDOFF written.").unwrap(),
        "transcript order:\n{stdout}"
    );
}

#[test]
fn transcript_json_carries_same_items() {
    let h = setup("json");
    write_ledger(&h);
    write_session(&h, ROWS);
    let assert = transcript_cmd(&h, &["log", "9", "--transcript", "--json"])
        .assert()
        .success();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&assert.get_output().stdout)).unwrap();
    assert_eq!(doc["ticket"], 9);
    assert_eq!(doc["attempt"], 1);
    assert_eq!(doc["slot"], "T9-a1");
    assert_eq!(doc["truncated"], false);
    let kinds: Vec<&str> = doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "user",
            "thinking",
            "assistant",
            "tool_call",
            "tool_result",
            "assistant"
        ]
    );
    assert_eq!(doc["items"][3]["name"], "read");
}

#[test]
fn transcript_tail_bounds_items() {
    let h = setup("tail");
    write_ledger(&h);
    write_session(&h, ROWS);
    let assert = transcript_cmd(&h, &["log", "9", "--transcript", "--tail", "2"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("HANDOFF written."),
        "newest kept:\n{stdout}"
    );
    assert!(
        !stdout.contains("Fix the flaky test."),
        "older dropped:\n{stdout}"
    );
    assert!(
        stdout.contains("--tail 0 for full"),
        "bound named:\n{stdout}"
    );
}

#[test]
fn transcript_missing_session_reads_calmly() {
    let h = setup("missing");
    write_ledger(&h);
    // No session dir at all.
    let assert = transcript_cmd(&h, &["log", "9", "--transcript"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("no recorded session yet"),
        "calm missing-session:\n{stdout}"
    );
    let assert = transcript_cmd(&h, &["log", "9", "--transcript", "--json"])
        .assert()
        .success();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&assert.get_output().stdout)).unwrap();
    assert_eq!(doc["session"], serde_json::Value::Null);
    assert_eq!(doc["items"], serde_json::json!([]));
}

#[test]
fn transcript_partial_file_shows_gap() {
    let h = setup("partial");
    write_ledger(&h);
    write_session(&h, &[ROWS[0], "this is not json{{{", ROWS[1]]);
    let assert = transcript_cmd(&h, &["log", "9", "--transcript"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("Fix the flaky test."),
        "good rows kept:\n{stdout}"
    );
    assert!(
        stdout.contains("partially unreadable"),
        "gap named:\n{stdout}"
    );
}

#[test]
fn transcript_with_fork_refuses_plainly() {
    let h = setup("forkcombo");
    write_ledger(&h);
    write_session(&h, ROWS);
    transcript_cmd(&h, &["log", "9", "--transcript", "--fork", "x"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--transcript with --fork"));
}

#[test]
fn transcript_no_attempts_reads_calmly() {
    let h = setup("empty");
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "attempts": [] })).unwrap(),
    )
    .unwrap();
    transcript_cmd(&h, &["log", "9", "--transcript"])
        .assert()
        .success()
        .stdout(predicates::str::contains("no attempts yet"));
}
