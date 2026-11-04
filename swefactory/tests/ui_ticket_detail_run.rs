//! Ticket detail + Run buttons endpoint tests (ticket #138, spec #135).
//!
//! Seam: HTTP status + content markers against fixture state, mirroring
//! `ui_ticket_detail.rs`. Pure functions (`ticket_acceptance_criteria`,
//! `ticket_freeze_hash`, `ticket_checks_output_for`,
//! `ticket_run_result_html`, `ticket_detail_json`) carry the markers; the
//! POST run handler is exercised over real TCP on 127.0.0.1. The live
//! seams (`gh issue view` via FACTORY_GH_BIN, ledger via
//! FACTORY_STATE_DIR, factory cache via FACTORY_CACHE_DIR) are faked so
//! proofs stay hermetic.

use std::fs;
use std::path::{Path, PathBuf};

use factory_lib::{
    ticket_acceptance_criteria, ticket_attempt_views, ticket_checks_output_for, ticket_detail,
    ticket_detail_fragment_html, ticket_detail_json, ticket_freeze_hash, ticket_run_outcome,
    ticket_run_result_html, TicketDetail,
};

/// Serialize every test that mutates process env (one process per test
/// binary, but threads share env within it). Mirrors `ui_ticket_detail.rs`.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn lock_env() -> tokio::sync::MutexGuard<'static, ()> {
    ENV_LOCK.blocking_lock()
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uiticketrun-{}-{}-{}",
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

const ISSUE_BODY: &str = "Part of #135.\n\n## Acceptance criteria\n\n- [ ] Detail shows freeze hash\n- [x] Run buttons render\n- [ ] Glossary terms exact\n";

/// Fake `gh`: answers `issue view 9` (issue fields and comments
/// separately), accepts `issue comment` posts, fails everything else.
fn write_fake_gh(dir: &Path) -> PathBuf {
    write_script(
        dir,
        "gh",
        &format!(
            r#"#!/bin/sh
case "$*" in
*"issue view 9 "*"comments"*)
cat <<'EOF'
{{"comments":[]}}
EOF
;;
*"issue view 9 "*) 
cat <<'EOF'
{{"number":9,"title":"Sprout runs","body":{body},"labels":[{{"name":"ready"}}],"state":"open"}}
EOF
;;
*"issue comment 9 "*) exit 0;;
*) exit 1;;
esac
"#,
            body = serde_json::to_string(ISSUE_BODY).unwrap(),
        ),
    )
}

/// Ledger with one passing attempt for ticket 9. Returns the state dir.
fn seed_state(root: &Path) -> PathBuf {
    let state = root.join("state");
    let logs = state.join("logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(logs.join("T9-a1.log"), "a1 line 1\n").unwrap();
    let ledger = serde_json::json!({
        "attempts": [
            {"id": 1, "ticket": 9, "attempt_no": 1, "base_commit": "base1111", "head_commit": "head1111",
             "branch": "factory/T9-x", "worktree": "/tmp/wt1", "status": "impl_done", "exit_code": 0,
             "log_path": logs.join("T9-a1.log").to_string_lossy(), "started_at": "100", "finished_at": "200"}
        ]
    });
    fs::write(
        state.join("dispatch.json"),
        serde_json::to_string_pretty(&ledger).unwrap(),
    )
    .unwrap();
    state
}

/// Seed the watched factory cache for spec 135 (the linked spec of
/// ticket 9 via `Part of #135`). Returns the cache dir.
fn seed_cache(root: &Path, hash: &str) -> PathBuf {
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(
        cache.join("SPEC-135.md"),
        format!("<!-- factory cache: issue #135 hash={hash} -->\n# SPEC-135\n\n- State: open | body-sha256: `{hash}`\n\nBody here.\n"),
    )
    .unwrap();
    cache
}

fn fixture_detail() -> TicketDetail {
    TicketDetail {
        number: 9,
        title: "Sprout runs".to_string(),
        body: ISSUE_BODY.to_string(),
        labels: vec!["ready".to_string()],
        state: "open".to_string(),
        freeze_hash: Some("abc123".to_string()),
        acceptance: vec![
            (false, "Detail shows freeze hash".to_string()),
            (true, "Run buttons render".to_string()),
        ],
        checks_output: "Ticket 9 attempt 1 finished (exit 0).".to_string(),
        attempts: Vec::new(),
        transcript: None,
    }
}

async fn request(port: u16, method: &str, path: &str) -> String {
    let (method, path) = (method.to_string(), path.to_string());
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
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
        let _ = axum::serve(listener, factory_lib::web_router()).await;
    });
    (port, handle)
}

#[test]
fn acceptance_criteria_come_from_body_checkboxes_in_order() {
    let criteria = ticket_acceptance_criteria(ISSUE_BODY);
    assert_eq!(
        criteria,
        vec![
            (false, "Detail shows freeze hash".to_string()),
            (true, "Run buttons render".to_string()),
            (false, "Glossary terms exact".to_string()),
        ],
        "checkboxes in body order, prose lines excluded"
    );
    assert!(
        ticket_acceptance_criteria("no checkboxes here\n- plain bullet\n").is_empty(),
        "non-checkbox lines are never criteria"
    );
}

#[test]
fn checks_output_names_the_newest_attempt_outcome() {
    let _guard = lock_env();
    let root = tmpdir("checks");
    let state = seed_state(&root);
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
    }
    let views = ticket_attempt_views(9, 5);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
    }
    let line = ticket_checks_output_for(&views);
    assert!(line.contains("exit 0"), "names the exit, got: {line}");
    assert!(line.contains("attempt 1"), "names the attempt, got: {line}");
    assert_eq!(
        ticket_checks_output_for(&[]),
        "No checks recorded yet \u{2014} nothing has run for this ticket."
    );
}

#[test]
fn freeze_hash_comes_from_the_linked_spec_cache() {
    let _guard = lock_env();
    let root = tmpdir("freeze");
    let cache = seed_cache(&root, "deadbee");
    unsafe {
        std::env::set_var("FACTORY_CACHE_DIR", &cache);
    }
    // Ticket 9 links spec 135 (`Part of #135`), so the spec cache wins.
    let hash = ticket_freeze_hash(9, ISSUE_BODY);
    // No cache for the ticket's own number: unlinked bodies read None.
    let missing = ticket_freeze_hash(9, "no spec link here");
    unsafe {
        std::env::remove_var("FACTORY_CACHE_DIR");
    }
    assert_eq!(hash.as_deref(), Some("deadbee"));
    assert!(missing.is_none(), "no cache file means no invented hash");
}

#[test]
fn run_sync_names_the_freeze_cause_and_refreshes_the_cache() {
    let _guard = lock_env();
    let root = tmpdir("sync");
    let state = seed_state(&root);
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    let gh = write_fake_gh(&root);
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_CACHE_DIR", &cache);
        std::env::set_var("FACTORY_GH_BIN", &gh);
    }
    // No SPEC-APPROVED comment on the fake issue: sync fails and the
    // cause names the mismatch (never a bare failure).
    let (passed, cause) = ticket_run_outcome(9, "sync", None);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_CACHE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
    }
    assert!(!passed, "unapproved body must fail, cause: {cause}");
    assert!(
        cause.contains("MISMATCH"),
        "cause names mismatch, got: {cause}"
    );
    assert!(
        cache.join("SPEC-9.md").is_file(),
        "executing sync refreshes the watched factory cache"
    );
}

#[test]
fn run_check_reports_the_recorded_attempt_outcome() {
    let _guard = lock_env();
    let root = tmpdir("check");
    let state = seed_state(&root);
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
    }
    let (passed, cause) = ticket_run_outcome(9, "check", None);
    let (failed_pass, failed_cause) = ticket_run_outcome(999, "check", None);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
    }
    assert!(passed, "exit-0 attempt passes, cause: {cause}");
    assert!(
        cause.contains("exit 0"),
        "cause names the exit, got: {cause}"
    );
    assert!(
        !failed_pass && failed_cause.contains("nothing to report"),
        "ticket with no attempts fails calmly, got: {failed_cause}"
    );
}

#[test]
fn run_handoff_gates_the_signal_then_posts_via_gh() {
    let _guard = lock_env();
    let root = tmpdir("handoff");
    let gh = write_fake_gh(&root);
    unsafe {
        std::env::set_var("FACTORY_GH_BIN", &gh);
    }
    let (refused, refuse_cause) = ticket_run_outcome(9, "handoff", Some("BOGUS"));
    let (posted, post_cause) = ticket_run_outcome(9, "handoff", Some("READY_FOR_REVIEW"));
    let (unknown, unknown_cause) = ticket_run_outcome(9, "bogus-action", None);
    unsafe {
        std::env::remove_var("FACTORY_GH_BIN");
    }
    assert!(
        !refused && refuse_cause.contains("handoff refused"),
        "bad signal refused at the gate, got: {refuse_cause}"
    );
    assert!(
        posted && post_cause.contains("FACTORY:READY_FOR_REVIEW"),
        "good signal posts the handoff comment, got: {post_cause}"
    );
    assert!(
        !unknown && unknown_cause.contains("unknown run action"),
        "unknown actions fail calmly, got: {unknown_cause}"
    );
}

#[test]
fn result_snippet_marks_pass_fail_with_the_cause() {
    let ok = ticket_run_result_html("sync", true, "freeze OK: #9 hash=abc");
    assert!(ok.contains("data-run-result"), "got:\n{ok}");
    assert!(ok.contains("data-pass=\"true\""), "got:\n{ok}");
    assert!(ok.contains("freeze OK"), "cause is named, got:\n{ok}");
    let bad = ticket_run_result_html("sync", false, "freeze MISMATCH: #9");
    assert!(bad.contains("data-pass=\"false\""), "got:\n{bad}");
    assert!(bad.contains("MISMATCH"), "got:\n{bad}");
}

#[test]
fn fragment_shows_freeze_acceptance_checks_run_buttons_and_glossary() {
    let detail = fixture_detail();
    let html = ticket_detail_fragment_html(&detail);
    assert!(html.contains("data-freeze-hash"), "got:\n{html}");
    assert!(html.contains("abc123"), "freeze hash renders, got:\n{html}");
    assert!(html.contains("data-acceptance-criteria"), "got:\n{html}");
    assert!(
        html.contains("Run buttons render"),
        "criteria text renders, got:\n{html}"
    );
    assert!(html.contains("data-checks-output"), "got:\n{html}");
    assert!(html.contains("data-run-buttons"), "got:\n{html}");
    for action in ["sync", "check", "handoff"] {
        assert!(
            html.contains(&format!("data-run=\"{action}\"")),
            "missing Run {action} button, got:\n{html}"
        );
        assert!(
            html.contains(&format!("/tickets/9/run/{action}")),
            "button posts to the run endpoint, got:\n{html}"
        );
    }
    // UI copy uses the domain glossary terms exactly (spec #135).
    for term in ["AgentRunner", "session identity", "verdict tag"] {
        assert!(
            html.contains(term),
            "glossary term {term:?} missing from run copy, got:\n{html}"
        );
    }

    let json = ticket_detail_json(&detail);
    assert_eq!(
        json.get("freeze_hash").and_then(|h| h.as_str()),
        Some("abc123")
    );
    assert_eq!(
        json.get("checks_output").and_then(|c| c.as_str()),
        Some("Ticket 9 attempt 1 finished (exit 0).")
    );
    let acceptance = json.get("acceptance").and_then(|a| a.as_array()).unwrap();
    assert_eq!(acceptance.len(), 2);
}

#[test]
fn detail_assembly_joins_cache_tracker_and_ledger() {
    let _guard = lock_env();
    let root = tmpdir("assembly");
    let state = seed_state(&root);
    let cache = seed_cache(&root, "feedface");
    let gh = write_fake_gh(&root);
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_CACHE_DIR", &cache);
        std::env::set_var("FACTORY_GH_BIN", &gh);
    }
    let detail = ticket_detail(9, factory_lib::TICKET_DETAIL_TAIL);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_CACHE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
    }
    let detail = detail.expect("ticket 9 exists in the fake gh");
    assert_eq!(
        detail.freeze_hash.as_deref(),
        Some("feedface"),
        "freeze comes from the linked-spec cache join"
    );
    assert_eq!(detail.acceptance.len(), 3, "criteria parsed from the body");
    assert!(
        detail.checks_output.contains("exit 0"),
        "checks joined from the ledger, got: {}",
        detail.checks_output
    );
}

#[tokio::test]
async fn run_buttons_post_inline_pass_fail_with_cause() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("endpoints");
    let state = seed_state(&root);
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    let gh = write_fake_gh(&root);
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_CACHE_DIR", &cache);
        std::env::set_var("FACTORY_GH_BIN", &gh);
    }
    let (port, server) = serve_port().await;

    // Sync: unapproved body fails with the mismatch named inline.
    let sync = request(port, "POST", "/tickets/9/run/sync").await;
    assert!(sync.contains("422"), "failing run is 422, got:\n{sync}");
    assert!(sync.contains("data-run-result"), "got:\n{sync}");
    assert!(sync.contains("data-pass=\"false\""), "got:\n{sync}");
    assert!(
        sync.contains("MISMATCH"),
        "cause named inline, got:\n{sync}"
    );

    // Check: exit-0 attempt passes inline.
    let check = request(port, "POST", "/tickets/9/run/check").await;
    assert!(
        check.contains("200 OK"),
        "passing run is 200, got:\n{check}"
    );
    assert!(check.contains("data-pass=\"true\""), "got:\n{check}");
    assert!(check.contains("exit 0"), "got:\n{check}");

    // Handoff with a good signal posts via the gh seam.
    let handoff = request(
        port,
        "POST",
        "/tickets/9/run/handoff?signal=READY_FOR_REVIEW",
    )
    .await;
    assert!(handoff.contains("200 OK"), "got:\n{handoff}");
    assert!(
        handoff.contains("FACTORY:READY_FOR_REVIEW"),
        "got:\n{handoff}"
    );

    // Handoff with a bad signal is refused, never posted.
    let refused = request(port, "POST", "/tickets/9/run/handoff?signal=BOGUS").await;
    assert!(refused.contains("422"), "got:\n{refused}");
    assert!(refused.contains("handoff refused"), "got:\n{refused}");

    // Unknown tickets fail calmly with the missing-ticket sentence, never 500.
    let missing = request(port, "POST", "/tickets/999/run/sync").await;
    assert!(missing.contains("422"), "got:\n{missing}");
    assert!(missing.contains("no ticket 999"), "got:\n{missing}");

    server.abort();
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_CACHE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
    }
}
