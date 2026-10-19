//! Transcript-pane UI tests (ticket #93).
//!
//! Seam: fixture state (ledger + worktree) behind `FACTORY_STATE_DIR` and
//! a fixture agent-sessions root behind `FACTORY_PI_SESSIONS_DIR`.
//! Pure render functions (`ticket_transcript_pane_html`,
//! `ticket_detail_fragment_html`, `ticket_detail_json`) plus the read-only
//! `ticket_transcript_pane` assembly carry the markers; the CLI parity
//! check shells out to the built binary like `cli_transcript.rs`.
//! No daemon, no network, no live runs anywhere here.
//!
//! What these tests catch: the UI pane renders the same normalized items
//! the CLI renders (one reducer, no double jsonl parse), with the same
//! distinctions (assistant prose, thinking, tool calls with names,
//! results), calm missing/partial/corrupt reads, item bounding, and a
//! read-only proof (bytes + mtime never move).

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

use factory_lib::{
    ticket_detail_fragment_html, ticket_detail_json, ticket_transcript_pane,
    ticket_transcript_pane_html, TicketAttemptView, TicketDetail, TicketTranscriptPane,
    TranscriptItem,
};

/// Serialize every test that mutates process env (one process per test
/// binary, but threads share env within it).
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uitranscript-{}-{}-{}",
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

fn with_env(h: &Harness) -> tokio::sync::MutexGuard<'static, ()> {
    let guard = ENV_LOCK.blocking_lock();
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &h.state);
        std::env::set_var("FACTORY_PI_SESSIONS_DIR", &h.sessions);
    }
    guard
}

fn clear_env() {
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_PI_SESSIONS_DIR");
    }
}

const ROWS: &[&str] = &[
    r#"{"type":"message","id":"m1","message":{"role":"user","content":[{"type":"text","text":"Fix the flaky test."}]}}"#,
    r#"{"type":"message","id":"m2","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Race, not logic. Verify first."},{"type":"text","text":"Diagnosing the timing failure."},{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"AGENTS.md"}}]}}"#,
    r#"{"type":"message","id":"m3","message":{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"file contents here"}]}}"#,
    r#"{"type":"message","id":"m4","message":{"role":"assistant","content":[{"type":"text","text":"HANDOFF written."}]}}"#,
];

fn pane_kinds(pane: &TicketTranscriptPane) -> Vec<&str> {
    pane.items.iter().map(|i| i.kind.as_str()).collect()
}

#[test]
fn pane_reduces_the_same_items_once() {
    let h = setup("voice");
    write_ledger(&h);
    write_session(&h, ROWS);
    let _guard = with_env(&h);
    let pane = ticket_transcript_pane(9).expect("latest attempt has a pane");
    assert_eq!(pane.ticket, 9);
    assert_eq!(pane.attempt_no, 1);
    assert_eq!(pane.slot, "T9-a1");
    assert!(pane.session.is_some(), "session file resolved");
    assert_eq!(
        pane_kinds(&pane),
        [
            "user",
            "thinking",
            "assistant",
            "tool_call",
            "tool_result",
            "assistant"
        ],
        "one reducer, transcript order"
    );
    assert_eq!(pane.items[3].name, "read");
    assert!(!pane.truncated);
    assert_eq!(pane.partial, 0);
    assert_eq!(pane.total, 6);
    clear_env();
}

#[test]
fn pane_html_renders_each_kind_distinctly() {
    let h = setup("html");
    write_ledger(&h);
    write_session(&h, ROWS);
    let _guard = with_env(&h);
    let pane = ticket_transcript_pane(9).expect("pane");
    let html = ticket_transcript_pane_html(&pane);
    for marker in [
        "data-transcript-pane",
        "data-slot=\"T9-a1\"",
        "data-transcript-item=\"user\"",
        "data-transcript-item=\"thinking\"",
        "data-transcript-item=\"assistant\"",
        "data-transcript-item=\"tool_call\"",
        "data-transcript-item=\"tool_result\"",
        "data-tool-name>read<",
        "data-transcript-text",
        "Fix the flaky test.",
        "Diagnosing the timing failure.",
        "Race, not logic.",
        "AGENTS.md",
        "file contents here",
        "HANDOFF written.",
        "factory log 9 --transcript",
    ] {
        assert!(html.contains(marker), "missing {marker} in:\n{html}");
    }
    // Assistant prose reads as the agent's voice, thinking as thinking.
    let agent = html.find(">agent<").expect("agent kind label");
    let thinking = html.find(">thinking<").expect("thinking kind label");
    assert!(agent < thinking || thinking < agent, "both labels render");
    clear_env();
}

#[test]
fn pane_missing_session_reads_calmly() {
    let h = setup("missing");
    write_ledger(&h);
    // No sessions dir at all.
    let _guard = with_env(&h);
    let pane = ticket_transcript_pane(9).expect("missing session still panes");
    assert_eq!(pane.session, None);
    assert!(pane.items.is_empty());
    let html = ticket_transcript_pane_html(&pane);
    assert!(
        html.contains("data-transcript-pane"),
        "pane section still renders, got:\n{html}"
    );
    assert!(
        html.contains("data-no-transcript-session"),
        "calm missing marker, got:\n{html}"
    );
    assert!(
        html.contains("no recorded session yet"),
        "calm sentence, got:\n{html}"
    );

    // JSON twin: session null, items empty, never an error shape.
    let detail = TicketDetail {
        number: 9,
        title: "t".to_string(),
        body: "b".to_string(),
        labels: vec![],
        state: "open".to_string(),
        attempts: vec![],
        transcript: Some(pane),
    };
    let doc = ticket_detail_json(&detail);
    assert_eq!(doc["transcript_pane"]["session"], serde_json::Value::Null);
    assert_eq!(doc["transcript_pane"]["items"], serde_json::json!([]));
    clear_env();
}

#[test]
fn pane_partial_file_shows_named_gap() {
    let h = setup("partial");
    write_ledger(&h);
    write_session(&h, &[ROWS[0], "this is not json{{{", ROWS[1]]);
    let _guard = with_env(&h);
    let pane = ticket_transcript_pane(9).expect("pane");
    assert_eq!(pane.partial, 1, "one corrupt line counted");
    assert!(
        pane.items
            .iter()
            .any(|i| i.text.contains("Fix the flaky test.")),
        "good rows kept, got: {:?}",
        pane.items
    );
    let html = ticket_transcript_pane_html(&pane);
    assert!(
        html.contains("data-transcript-gap"),
        "gap marker, got:\n{html}"
    );
    assert!(
        html.contains("partially unreadable"),
        "gap named, got:\n{html}"
    );
    clear_env();
}

#[test]
fn pane_bounds_items_with_a_named_truncation() {
    let h = setup("bound");
    write_ledger(&h);
    let rows: Vec<String> = (1..=25)
        .map(|i| {
            format!(
                "{{\"type\":\"message\",\"id\":\"m{i}\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"turn {i}\"}}]}}}}"
            )
        })
        .collect();
    let refs: Vec<&str> = rows.iter().map(|s| s.as_str()).collect();
    write_session(&h, &refs);
    let _guard = with_env(&h);
    let pane = ticket_transcript_pane(9).expect("pane");
    assert_eq!(pane.items.len(), 20, "bounded to the newest items");
    assert_eq!(pane.total, 25);
    assert!(pane.truncated);
    assert!(
        pane.items.iter().any(|i| i.text == "turn 25"),
        "newest kept"
    );
    assert!(
        !pane.items.iter().any(|i| i.text == "turn 1"),
        "oldest dropped"
    );
    let html = ticket_transcript_pane_html(&pane);
    assert!(
        html.contains("data-transcript-truncated"),
        "truncation named, got:\n{html}"
    );
    assert!(
        html.contains("--tail 0"),
        "full-conversation path named, got:\n{html}"
    );
    clear_env();
}

#[test]
fn pane_empty_ticket_is_none_and_fragment_omits_pane() {
    let h = setup("empty");
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "attempts": [] })).unwrap(),
    )
    .unwrap();
    let _guard = with_env(&h);
    assert!(
        ticket_transcript_pane(9).is_none(),
        "no attempts means no pane"
    );
    let detail = TicketDetail {
        number: 9,
        title: "t".to_string(),
        body: "b".to_string(),
        labels: vec![],
        state: "open".to_string(),
        attempts: vec![],
        transcript: None,
    };
    let frag = ticket_detail_fragment_html(&detail);
    assert!(
        !frag.contains("data-transcript-pane"),
        "no pane without attempts, got:\n{frag}"
    );
    clear_env();
}

#[test]
fn detail_fragment_embeds_pane_beside_attempts() {
    let h = setup("embed");
    write_ledger(&h);
    write_session(&h, ROWS);
    let _guard = with_env(&h);
    let pane = ticket_transcript_pane(9).expect("pane");
    let detail = TicketDetail {
        number: 9,
        title: "Sprout".to_string(),
        body: "Grow.".to_string(),
        labels: vec!["ready".to_string()],
        state: "open".to_string(),
        attempts: vec![TicketAttemptView {
            attempt_no: 1,
            status: "impl_running".to_string(),
            summary: "Ticket 9 attempt 1 is being implemented.".to_string(),
            base: "abc".to_string(),
            head: String::new(),
            started: "100".to_string(),
            finished: String::new(),
            exit_code: 0,
            transcript: "raw log".to_string(),
            truncated: false,
        }],
        transcript: Some(pane),
    };
    // The polling unit carries both: the raw attempt transcript for the
    // log view plus the normalized pane for the conversation view.
    let frag = ticket_detail_fragment_html(&detail);
    assert!(frag.contains("data-attempt=\"1\""), "got:\n{frag}");
    assert!(frag.contains("data-transcript-pane"), "got:\n{frag}");
    assert!(
        frag.find("data-attempt=\"1\"").unwrap() < frag.find("data-transcript-pane").unwrap(),
        "pane rides after the attempt history, got:\n{frag}"
    );
    // JSON carries the same items structurally.
    let doc = ticket_detail_json(&detail);
    let kinds: Vec<&str> = doc["transcript_pane"]["items"]
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
    clear_env();
}

#[test]
fn cli_and_ui_render_the_same_items() {
    let h = setup("parity");
    write_ledger(&h);
    write_session(&h, ROWS);
    let _guard = with_env(&h);
    let pane = ticket_transcript_pane(9).expect("pane");
    let pane_json: Vec<serde_json::Value> = pane
        .items
        .iter()
        .map(|i| serde_json::json!({"kind": i.kind, "name": i.name, "text": i.text}))
        .collect();

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(["log", "9", "--transcript", "--json"])
        .current_dir(&h.repo)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("FACTORY_PI_SESSIONS_DIR", &h.sessions)
        .env_remove("FACTORY_SLOT");
    let out = cmd.output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let cli_items = doc["items"].as_array().unwrap();
    let cli_json: Vec<serde_json::Value> = cli_items.to_vec();
    // TranscriptItem serializes exactly {kind, name, text}: the pane facts
    // and the CLI facts are structurally identical (one reducer).
    let expected: Vec<serde_json::Value> = pane_json
        .iter()
        .map(|v| {
            let item: TranscriptItem = serde_json::from_value(v.clone()).unwrap();
            serde_json::to_value(&item).unwrap()
        })
        .collect();
    assert_eq!(cli_json, expected, "CLI --json parity with the UI pane");
    clear_env();
}

/// (relative path, bytes, mtime) for every file under the given roots.
fn snapshot(roots: &[&Path]) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut out = Vec::new();
    for root in roots {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries: Vec<_> = fs::read_dir(&dir).map(|r| r.collect()).unwrap_or_default();
            for e in entries {
                let e = e.unwrap();
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let bytes = fs::read(&p).unwrap();
                    let mtime = fs::metadata(&p).unwrap().modified().unwrap();
                    out.push((p.display().to_string(), bytes, mtime));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn transcript_read_path_never_mutates_state() {
    let h = setup("readonly");
    write_ledger(&h);
    write_session(&h, ROWS);
    let before = snapshot(&[&h.state, &h.sessions]);
    assert!(!before.is_empty());

    let _guard = with_env(&h);
    let pane = ticket_transcript_pane(9).expect("pane");
    let _ = ticket_transcript_pane_html(&pane);
    let detail = TicketDetail {
        number: 9,
        title: "t".to_string(),
        body: "b".to_string(),
        labels: vec![],
        state: "open".to_string(),
        attempts: vec![],
        transcript: Some(pane),
    };
    let _ = ticket_detail_fragment_html(&detail);
    let _ = ticket_detail_json(&detail);
    clear_env();
    drop(_guard);

    // The CLI twin over the same fixtures.
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(["log", "9", "--transcript"])
        .current_dir(&h.repo)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("FACTORY_PI_SESSIONS_DIR", &h.sessions)
        .env_remove("FACTORY_SLOT");
    assert!(cmd.output().unwrap().status.success());

    let after = snapshot(&[&h.state, &h.sessions]);
    assert_eq!(before.len(), after.len(), "no files added or removed");
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}
