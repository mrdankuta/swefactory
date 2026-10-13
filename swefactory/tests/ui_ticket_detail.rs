//! Ticket detail + log view endpoint tests (ticket #30, spec #27).
//!
//! Seam: HTTP status + content markers against fixture state.
//! Pure render functions (`ticket_detail_page_html`,
//! `ticket_detail_fragment_html`, `ticket_detail_json`) carry the markers;
//! the Axum handlers are exercised over real TCP on 127.0.0.1 (no browser,
//! no network beyond loopback). The live seams (`gh issue view` via
//! FACTORY_GH_BIN, ledger via FACTORY_STATE_DIR) are faked so proofs stay
//! hermetic. Read-only proof mirrors `ui_board.rs`: byte+mtime snapshots
//! must not move.

use std::fs;
use std::path::{Path, PathBuf};

use factory_lib::{
    ticket_attempt_views, ticket_detail, ticket_detail_fragment_html, ticket_detail_json,
    ticket_detail_page_html, ticket_missing_sentence, ticket_no_attempts_sentence, web_router,
    TicketAttemptView, TicketDetail,
};

/// Serialize every test that mutates process env (one process per test
/// binary, but threads share env within it). A tokio mutex: async tests
/// `.await` it, sync tests `blocking_lock` it — never a std guard across
/// an await point (clippy `await_holding_lock`).
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn lock_env() -> tokio::sync::MutexGuard<'static, ()> {
    ENV_LOCK.blocking_lock()
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uiticket-{}-{}-{}",
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

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// Fake `gh`: answers `issue view 9` with the canned issue, fails
/// everything else (missing tickets read as calm 404s, never 500s).
fn write_fake_gh(dir: &Path) -> PathBuf {
    write_script(
        dir,
        "gh",
        r#"#!/bin/sh
case "$*" in
*"issue view 9 "*)
cat <<'EOF'
{"number":9,"title":"Sprout detail","body":"Grow the sprout.\nCost: 2 points","labels":[{"name":"ready"}],"state":"open"}
EOF
;;
*) exit 1;;
esac
"#,
    )
}

/// Ledger with two attempts for ticket 9 (a1 failed implement, a2
/// reviewing) plus logs on disk. Returns the state dir.
fn seed_state(root: &Path) -> PathBuf {
    let state = root.join("state");
    let logs = state.join("logs");
    fs::create_dir_all(&logs).unwrap();
    let a1_log = logs.join("T9-a1.log");
    let a1_review = logs.join("T9-a1-review.log");
    let a2_log = logs.join("T9-a2.log");
    let a1_lines: Vec<String> = (1..=10).map(|i| format!("a1 implement line {i}")).collect();
    fs::write(&a1_log, a1_lines.join("\n") + "\n").unwrap();
    fs::write(&a1_review, "reviewer: changes requested\n").unwrap();
    fs::write(&a2_log, "a2 implement line 1\na2 implement line 2\n").unwrap();
    let ledger = serde_json::json!({
        "attempts": [
            {"id": 1, "ticket": 9, "attempt_no": 1, "base_commit": "base1111", "head_commit": "head1111",
             "branch": "factory/T9-x", "worktree": "/tmp/wt1", "status": "impl_failed", "exit_code": 1,
             "log_path": a1_log.to_string_lossy(), "started_at": "100", "finished_at": "200"},
            {"id": 2, "ticket": 9, "attempt_no": 2, "base_commit": "base2222", "head_commit": "",
             "branch": "factory/T9-y", "worktree": "/tmp/wt2", "status": "review_running", "exit_code": 0,
             "log_path": a2_log.to_string_lossy(), "started_at": "300", "finished_at": ""}
        ]
    });
    fs::write(
        state.join("dispatch.json"),
        serde_json::to_string_pretty(&ledger).unwrap(),
    )
    .unwrap();
    state
}

fn fixture_detail() -> TicketDetail {
    TicketDetail {
        number: 9,
        title: "Sprout detail".to_string(),
        body: "Grow the sprout.".to_string(),
        labels: vec!["ready".to_string()],
        state: "open".to_string(),
        attempts: vec![
            TicketAttemptView {
                attempt_no: 2,
                status: "review_running".to_string(),
                summary: "Ticket 9 attempt 2 is being reviewed (slot T9-a2, started 300)."
                    .to_string(),
                base: "base2222".to_string(),
                head: String::new(),
                started: "300".to_string(),
                finished: String::new(),
                exit_code: 0,
                transcript: "a2 implement line 1\na2 implement line 2\n".to_string(),
                truncated: false,
            },
            TicketAttemptView {
                attempt_no: 1,
                status: "impl_failed".to_string(),
                summary: "Ticket 9 attempt 1 failed during implement (exit 1).".to_string(),
                base: "base1111".to_string(),
                head: "head1111".to_string(),
                started: "100".to_string(),
                finished: "200".to_string(),
                exit_code: 1,
                transcript: "a1 implement line 10\n".to_string(),
                truncated: true,
            },
        ],
    }
}

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

/// Blocking std HTTP/1.1 GET over loopback (tokio here has no io-util,
/// and std is plenty for a status-line + marker proof). Runs on a
/// blocking thread: `#[tokio::test]` is single-threaded, so calling this
/// inline would starve the server task and hang.
async fn get(port: u16, path: &str) -> String {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    })
    .await
    .unwrap()
}

async fn serve_port() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, web_router()).await;
    });
    (port, handle)
}

#[test]
fn attempts_render_newest_first_with_summaries_before_transcripts() {
    let _guard = lock_env();
    let root = tmpdir("order");
    let state = seed_state(&root);
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
    }
    let views = ticket_attempt_views(9, 50);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
    }

    assert_eq!(views.len(), 2);
    assert_eq!(views[0].attempt_no, 2, "newest attempt first");
    assert_eq!(views[1].attempt_no, 1);
    // Failure cause reads as the story: the summary names the failure.
    assert!(
        views[1].summary.contains("failed during implement"),
        "failure summary first, got: {}",
        views[1].summary
    );
    // Combined transcript: implement log plus the sibling review log.
    assert!(
        views[0].transcript.contains("a2 implement line 2"),
        "got: {}",
        views[0].transcript
    );
    assert!(
        views[1].transcript.contains("a1 implement line 10"),
        "got: {}",
        views[1].transcript
    );
    assert!(
        views[1].transcript.contains("changes requested"),
        "review tail joins the attempt transcript, got: {}",
        views[1].transcript
    );

    let detail = fixture_detail();
    let html = ticket_detail_fragment_html(&detail);
    for a in &detail.attempts {
        let attempt_marker = format!("data-attempt=\"{}\"", a.attempt_no);
        let pos = html.find(&attempt_marker).unwrap_or_else(|| {
            panic!("missing {attempt_marker} in:\n{html}");
        });
        let tail = &html[pos..];
        let summary = tail.find("data-summary").unwrap_or_else(|| {
            panic!(
                "missing data-summary for attempt {} in:\n{html}",
                a.attempt_no
            );
        });
        let transcript = tail.find("data-transcript").unwrap_or_else(|| {
            panic!(
                "missing data-transcript for attempt {} in:\n{html}",
                a.attempt_no
            );
        });
        assert!(
            summary < transcript,
            "attempt {} summary must precede its transcript",
            a.attempt_no
        );
    }
}

#[test]
fn tail_bounds_hold_in_prose_and_json_shapes() {
    let _guard = lock_env();
    let root = tmpdir("tails");
    let state = seed_state(&root);
    // Extra-long transcript: 100 lines, bound to the last 5.
    let long = state.join("logs").join("T9-a2.log");
    let lines: Vec<String> = (1..=100).map(|i| format!("long line {i:03}")).collect();
    fs::write(&long, lines.join("\n") + "\n").unwrap();
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
    }
    let views = ticket_attempt_views(9, 5);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
    }

    let newest = &views[0];
    assert_eq!(
        newest.transcript.lines().count(),
        5,
        "got:\n{}",
        newest.transcript
    );
    assert!(
        newest.transcript.contains("long line 100"),
        "tail keeps the newest lines, got:\n{}",
        newest.transcript
    );
    assert!(
        !newest.transcript.contains("long line 001"),
        "tail drops the oldest lines, got:\n{}",
        newest.transcript
    );
    assert!(newest.truncated, "dropped lines must be flagged");

    // tail 0 opts into the full transcript (same contract as `factory log`).
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
    }
    let full = ticket_attempt_views(9, 0);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
    }
    assert_eq!(full[0].transcript.lines().count(), 100);
    assert!(!full[0].truncated);

    // Prose shape honors the same bound.
    let detail = fixture_detail();
    let html = ticket_detail_fragment_html(&detail);
    assert!(
        html.contains("a1 implement line 10"),
        "bounded tail present in prose, got:\n{html}"
    );
    assert!(
        html.contains("data-truncated"),
        "truncation is named in prose, got:\n{html}"
    );
    // JSON shape honors the same bound.
    let json = ticket_detail_json(&detail);
    let attempts = json.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[1]
            .get("transcript")
            .and_then(|t| t.as_str())
            .unwrap(),
        "a1 implement line 10\n"
    );
    assert!(attempts[1]
        .get("truncated")
        .and_then(|t| t.as_bool())
        .unwrap());
    assert_eq!(
        attempts[1].get("summary").and_then(|s| s.as_str()).unwrap(),
        "Ticket 9 attempt 1 failed during implement (exit 1)."
    );
}

#[test]
fn detail_page_reads_as_sentences_with_cli_correspondence() {
    let detail = fixture_detail();
    let page = ticket_detail_page_html(&detail);
    assert!(page.contains("Ticket #9"), "got markers missing");
    assert!(page.contains("data-ticket=\"9\""), "got markers missing");
    assert!(page.contains("data-ticket-body"), "got markers missing");
    assert!(
        page.contains("Grow the sprout."),
        "body renders, got:\n{page}"
    );
    assert!(
        page.contains("factory log 9"),
        "page shows its CLI correspondence, got markers missing"
    );
    assert!(
        page.contains("data-ticket-fragment=\"/fragments/ticket/9\""),
        "page names the fragment endpoint, got markers missing"
    );
    assert!(
        page.contains("data-poll-ms="),
        "page carries the poll interval, got markers missing"
    );
    assert!(
        page.contains("width=device-width"),
        "narrow screens must not break, got markers missing"
    );

    let empty = TicketDetail {
        attempts: Vec::new(),
        ..fixture_detail()
    };
    let frag = ticket_detail_fragment_html(&empty);
    assert!(
        frag.contains(ticket_no_attempts_sentence()),
        "empty history reads as a sentence, got:\n{frag}"
    );

    assert!(
        !ticket_missing_sentence("999").is_empty(),
        "missing tickets get a calm sentence, never a bare 500"
    );
}

#[tokio::test]
async fn endpoints_serve_detail_fragment_and_json() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("endpoints");
    let state = seed_state(&root);
    let gh = write_fake_gh(&root);
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GH_BIN", &gh);
    }
    let (port, server) = serve_port().await;

    let page = get(port, "/tickets/9").await;
    assert!(page.contains("200 OK"), "got:\n{page}");
    assert!(page.contains("data-ticket=\"9\""), "got:\n{page}");
    assert!(page.contains("data-summary"), "got:\n{page}");
    assert!(page.contains("data-transcript"), "got:\n{page}");
    assert!(page.contains("factory log 9"), "got:\n{page}");

    let frag = get(port, "/fragments/ticket/9").await;
    assert!(frag.contains("200 OK"), "got:\n{frag}");
    assert!(frag.contains("data-attempt=\"2\""), "got:\n{frag}");
    assert!(frag.contains("data-attempt=\"1\""), "got:\n{frag}");
    let first = frag.find("data-attempt=\"2\"").unwrap();
    let second = frag.find("data-attempt=\"1\"").unwrap();
    assert!(
        first < second,
        "fragment renders newest first, got:\n{frag}"
    );

    let api = get(port, "/api/tickets/9").await;
    assert!(api.contains("200 OK"), "got:\n{api}");
    assert!(api.contains("\"transcript\""), "got:\n{api}");
    assert!(api.contains("\"truncated\""), "got:\n{api}");
    assert!(api.contains("failed during implement"), "got:\n{api}");

    let missing = get(port, "/tickets/999").await;
    assert!(
        missing.contains("404"),
        "missing tickets 404 calmly, got:\n{missing}"
    );

    server.abort();
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
    }
}

#[tokio::test]
async fn ticket_detail_assembly_reads_issue_and_attempts() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("assembly");
    let state = seed_state(&root);
    let gh = write_fake_gh(&root);
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GH_BIN", &gh);
    }
    let detail = ticket_detail(9, factory_lib::TICKET_DETAIL_TAIL);
    let missing = ticket_detail(999, factory_lib::TICKET_DETAIL_TAIL);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
    }

    let detail = detail.expect("ticket 9 exists in the fake gh");
    assert_eq!(detail.title, "Sprout detail");
    assert!(detail.body.contains("Grow the sprout."));
    assert_eq!(detail.attempts.len(), 2);
    assert_eq!(detail.attempts[0].attempt_no, 2);
    assert!(missing.is_none(), "unknown tickets assemble to None");
}

#[tokio::test]
async fn ticket_read_path_never_mutates_state() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("readonly");
    let state = seed_state(&root);
    let gh = write_fake_gh(&root);
    let before = snapshot(&state);
    assert!(!before.is_empty());

    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GH_BIN", &gh);
    }
    let (port, server) = serve_port().await;
    let detail = ticket_detail(9, factory_lib::TICKET_DETAIL_TAIL).unwrap();
    let _ = ticket_detail_fragment_html(&detail);
    let _ = ticket_detail_page_html(&detail);
    let _ = ticket_detail_json(&detail);
    let _ = get(port, "/tickets/9").await;
    let _ = get(port, "/fragments/ticket/9").await;
    let _ = get(port, "/api/tickets/9").await;
    server.abort();
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
    }

    let after = snapshot(&state);
    assert_eq!(before.len(), after.len());
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}
