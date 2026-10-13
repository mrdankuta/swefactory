//! Dispatcher live screen endpoint tests (ticket #31, spec #27).
//!
//! Seam: HTTP status + content markers against fixture state.
//! Pure render functions (`dispatcher_page_html`, `dispatcher_fragment_html`)
//! carry the markers; handler responses are asserted via `IntoResponse`
//! status + body bytes (no browser, no network). The live seams (ledger +
//! logs via FACTORY_STATE_DIR, git worktrees via FACTORY_GIT_BIN) are faked
//! so rendering proofs stay hermetic. Read-only proof mirrors
//! `ui_board.rs`: byte+mtime snapshots must not move on GET paths.
//! STOP/Resume acts through the loop controls (`cmd_dispatch_stop` /
//! `cmd_dispatch_resume`): the test toggles the STOP file through those
//! functions and observes `dispatcher_data().stopped` flip.

use axum::response::IntoResponse;
use std::fs;
use std::path::{Path, PathBuf};

use factory_lib::{
    dispatcher_data, dispatcher_empty_sentence, dispatcher_fragment_html, dispatcher_page_html,
    web_router, DISPATCHER_POLL_MS,
};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uidispatch-{}-{}-{}",
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

fn write_fake_git(dir: &Path, worktree_list: &str) -> PathBuf {
    let path = dir.join("git-fake");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nif echo \"$*\" | grep -q \"worktree list\"; then\ncat <<'EOF'\n{worktree_list}\nEOF\nelse\nexit 1\nfi\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn seed_state(root: &Path, ledger_json: &str, logs: &[(&str, &str)]) -> PathBuf {
    let state = root.join("state");
    fs::create_dir_all(state.join("logs")).unwrap();
    fs::write(state.join("dispatch.json"), ledger_json).unwrap();
    for (name, body) in logs {
        fs::write(state.join("logs").join(name), body).unwrap();
    }
    state
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap()
}

fn with_state_env<T>(state: &Path, git: &Path, f: impl FnOnce() -> T) -> T {
    let _guard = env_lock();
    // SAFETY: the static lock serializes every seam user in this test
    // binary, so no two tests observe each other's FACTORY_* values.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", state);
        std::env::set_var("FACTORY_GIT_BIN", git);
    }
    let out = f();
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GIT_BIN");
    }
    out
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

async fn response_body_text(resp: axum::response::Response) -> (axum::http::StatusCode, String) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

const LEDGER_TWO: &str = r#"{"attempts":[
{"id":1,"ticket":11,"attempt_no":1,"base_commit":"base1111","head_commit":"head1111","branch":"factory/T11-x","worktree":"/tmp/wt1","status":"impl_running","exit_code":0,"log_path":"","started_at":"100","finished_at":""},
{"id":2,"ticket":12,"attempt_no":2,"base_commit":"base2222","head_commit":"","branch":"factory/T12-y","worktree":"/tmp/wt2","status":"impl_failed","exit_code":1,"log_path":"","started_at":"200","finished_at":"201"}
]}"#;

#[test]
fn attempts_slots_and_stop_state_render_live() {
    let root = tmpdir("live");
    let git = write_fake_git(
        &root,
        "/tmp/wt1 abc123 [factory/T11-x]\n/tmp/wt2 def456 [factory/T12-y]",
    );
    let log11 = "line-one\nline-two\nline-three\n";
    let state = seed_state(
        &root,
        LEDGER_TWO,
        &[
            ("T11-a1-impl.log", log11),
            ("T12-a2-impl.log", "boom failed\n"),
        ],
    );
    // Point the first attempt at its log file via ledger rewrite.
    let ledger_path = state.join("dispatch.json");
    let mut ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ledger_path).unwrap()).unwrap();
    ledger["attempts"][0]["log_path"] = serde_json::Value::String(
        state
            .join("logs")
            .join("T11-a1-impl.log")
            .to_string_lossy()
            .into_owned(),
    );
    ledger["attempts"][1]["log_path"] = serde_json::Value::String(
        state
            .join("logs")
            .join("T12-a2-impl.log")
            .to_string_lossy()
            .into_owned(),
    );
    fs::write(&ledger_path, serde_json::to_string(&ledger).unwrap()).unwrap();

    let data = with_state_env(&state, &git, dispatcher_data);
    assert_eq!(data.attempts.len(), 2);
    assert!(!data.stopped);

    let frag = dispatcher_fragment_html(&data);
    assert!(frag.contains("data-dispatch"), "got:\n{frag}");
    assert!(
        frag.contains("data-attempt=\"T11-a1\""),
        "slot lane marker, got:\n{frag}"
    );
    assert!(
        frag.contains("data-slot=\"T11-a1\""),
        "slot marker, got:\n{frag}"
    );
    assert!(
        frag.contains("data-attempt=\"T12-a2\""),
        "second attempt, got:\n{frag}"
    );
    assert!(
        frag.contains("data-stop"),
        "STOP state marker, got:\n{frag}"
    );
    assert!(
        frag.contains("data-log-tail"),
        "per-attempt log tail marker, got:\n{frag}"
    );
    assert!(
        frag.contains("line-three"),
        "tail streams log lines, got:\n{frag}"
    );
    assert!(
        frag.contains("data-worktrees"),
        "worktree ritual marker, got:\n{frag}"
    );
    assert!(
        frag.contains("factory/T11-x"),
        "worktree list streams, got:\n{frag}"
    );
    // STOP/Resume controls act through the loop controls: the fragment
    // names the POST endpoints whose handlers call those functions.
    assert!(
        frag.contains("/dispatch/stop"),
        "STOP control endpoint, got:\n{frag}"
    );
    assert!(
        frag.contains("/dispatch/resume"),
        "Resume control endpoint, got:\n{frag}"
    );
    // CLI correspondence is on the screen.
    assert!(
        frag.contains("factory dispatch status"),
        "status ritual correspondence, got:\n{frag}"
    );
    assert!(
        frag.contains("factory log"),
        "tail ritual correspondence, got:\n{frag}"
    );
    assert!(
        frag.contains("worktree list"),
        "worktree ritual correspondence, got:\n{frag}"
    );
}

#[test]
fn log_tails_truncate_with_a_calm_note() {
    let root = tmpdir("tail");
    let git = write_fake_git(&root, "");
    let many: String = (1..=120).map(|i| format!("log-line-{i}\n")).collect();
    let state = seed_state(
        &root,
        r#"{"attempts":[{"id":1,"ticket":7,"attempt_no":1,"base_commit":"b","head_commit":"","branch":"factory/T7-x","worktree":"","status":"impl_running","exit_code":0,"log_path":"","started_at":"1","finished_at":""}]}"#,
        &[("T7-a1-impl.log", &many)],
    );
    let ledger_path = state.join("dispatch.json");
    let mut ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ledger_path).unwrap()).unwrap();
    ledger["attempts"][0]["log_path"] = serde_json::Value::String(
        state
            .join("logs")
            .join("T7-a1-impl.log")
            .to_string_lossy()
            .into_owned(),
    );
    fs::write(&ledger_path, serde_json::to_string(&ledger).unwrap()).unwrap();

    let data = with_state_env(&state, &git, dispatcher_data);
    assert_eq!(data.attempts.len(), 1);
    assert!(data.attempts[0].truncated);
    let frag = dispatcher_fragment_html(&data);
    assert!(
        frag.contains("log-line-120"),
        "newest lines stream, got tail missing"
    );
    assert!(
        !frag.contains("log-line-1\n"),
        "older lines truncate beyond the tail"
    );
    assert!(
        frag.contains("last 50 lines"),
        "truncation says so calmly, got:\n{frag}"
    );
}

#[test]
fn empty_states_read_as_sentences() {
    let root = tmpdir("empty");
    let git = write_fake_git(&root, "");
    let state = seed_state(&root, r#"{"attempts":[]}"#, &[]);
    let data = with_state_env(&state, &git, dispatcher_data);
    assert!(data.attempts.is_empty());
    let frag = dispatcher_fragment_html(&data);
    assert!(
        frag.contains(dispatcher_empty_sentence()),
        "empty attempts read as a sentence, got:\n{frag}"
    );
    assert!(
        frag.contains("No worktrees"),
        "empty worktrees read calmly, got:\n{frag}"
    );
    let page = dispatcher_page_html(&data);
    assert!(
        page.contains(dispatcher_empty_sentence()),
        "page carries the calm empty state"
    );
    // An attempt with no log lines yet reads calmly, not blank.
    let state2 = seed_state(
        &root,
        r#"{"attempts":[{"id":1,"ticket":9,"attempt_no":1,"base_commit":"b","head_commit":"","branch":"","worktree":"","status":"impl_running","exit_code":0,"log_path":"","started_at":"1","finished_at":""}]}"#,
        &[],
    );
    let data2 = with_state_env(&state2, &git, dispatcher_data);
    let frag2 = dispatcher_fragment_html(&data2);
    assert!(
        frag2.contains("No transcript yet"),
        "missing transcripts read calmly, got:\n{frag2}"
    );
}

#[test]
fn stop_state_flips_through_loop_controls() {
    let root = tmpdir("stop");
    let git = write_fake_git(&root, "");
    let state = seed_state(&root, r#"{"attempts":[]}"#, &[]);
    assert!(!state.join("STOP").exists());
    with_state_env(&state, &git, || {
        assert_eq!(factory_lib::cmd_dispatch_stop(false), 0);
    });
    assert!(state.join("STOP").is_file());
    let stopped = with_state_env(&state, &git, dispatcher_data).stopped;
    assert!(stopped);
    let frag = dispatcher_fragment_html(&with_state_env(&state, &git, dispatcher_data));
    assert!(
        frag.contains("halted"),
        "halted STOP reads on screen, got:\n{frag}"
    );
    with_state_env(&state, &git, || {
        assert_eq!(factory_lib::cmd_dispatch_resume(false), 0);
    });
    assert!(!state.join("STOP").exists());
    let running = with_state_env(&state, &git, dispatcher_data).stopped;
    assert!(!running);
}

#[test]
fn dispatcher_page_polls_fragment_without_full_reload() {
    let root = tmpdir("poll");
    let git = write_fake_git(&root, "");
    let state = seed_state(&root, r#"{"attempts":[]}"#, &[]);
    let data = with_state_env(&state, &git, dispatcher_data);
    let page = dispatcher_page_html(&data);
    assert!(
        page.contains("data-dispatch-fragment=\"/fragments/dispatch\""),
        "page names the fragment endpoint"
    );
    assert!(
        page.contains(&format!("data-poll-ms=\"{DISPATCHER_POLL_MS}\"")),
        "page carries the ~3s poll interval"
    );
    assert_eq!(DISPATCHER_POLL_MS, 3000);
    assert!(page.contains("fetch("), "page refreshes via fetch");
    assert!(
        page.contains("factory dispatch status"),
        "page shows its CLI correspondence"
    );
    assert!(page.contains("/dispatch/stop"), "page carries STOP control");
}

#[tokio::test]
async fn endpoints_return_200_with_markers() {
    let _ = web_router();
    let root = tmpdir("endpoints");
    let git = write_fake_git(&root, "");
    let state = seed_state(&root, r#"{"attempts":[]}"#, &[]);
    let data = with_state_env(&state, &git, dispatcher_data);
    let page = dispatcher_page_html(&data);
    let resp = axum::response::Html(page).into_response();
    let (status, body) = response_body_text(resp).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("Factory dispatcher"));
    assert!(body.contains("data-dispatch"));

    let frag = dispatcher_fragment_html(&data);
    let resp = axum::response::Html(frag).into_response();
    let (status, body) = response_body_text(resp).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-dispatch"));

    // Handlers serve the same markers through the router's code path.
    let resp = factory_lib::dispatcher_page_handler().await.into_response();
    let (status, _) = response_body_text(resp).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let resp = factory_lib::dispatcher_fragment_handler()
        .await
        .into_response();
    let (status, _) = response_body_text(resp).await;
    assert_eq!(status, axum::http::StatusCode::OK);
}

#[test]
fn stop_resume_handlers_act_through_loop_controls() {
    let _guard = env_lock();
    let root = tmpdir("handlers");
    let git = write_fake_git(&root, "");
    let state = seed_state(&root, r#"{"attempts":[]}"#, &[]);
    // SAFETY: lock held for the whole body: no sibling test can
    // observe these FACTORY_* values while the handlers run. A private
    // runtime drives the handler futures so no guard crosses an await.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GIT_BIN", &git);
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert!(!factory_lib::dispatcher_data().stopped);
    let resp = rt
        .block_on(factory_lib::dispatcher_stop_handler())
        .into_response();
    // Redirect back to the live screen after acting.
    assert!(
        resp.status().is_redirection(),
        "stop redirects, got: {}",
        resp.status()
    );
    assert!(state.join("STOP").is_file());
    let resp = rt
        .block_on(factory_lib::dispatcher_resume_handler())
        .into_response();
    assert!(
        resp.status().is_redirection(),
        "resume redirects, got: {}",
        resp.status()
    );
    assert!(!state.join("STOP").exists());
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GIT_BIN");
    }
}

#[test]
fn dispatcher_get_path_never_mutates_state() {
    let root = tmpdir("readonly");
    let git = write_fake_git(&root, "/tmp/wt1 abc [b]");
    let state = seed_state(
        &root,
        r#"{"attempts":[{"id":1,"ticket":9,"attempt_no":1,"base_commit":"b","head_commit":"","branch":"factory/T9-x","worktree":"","status":"impl_running","exit_code":0,"log_path":"","started_at":"1","finished_at":""}]}"#,
        &[("T9-a1-impl.log", "hello\n")],
    );
    let ledger_path = state.join("dispatch.json");
    let mut ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&ledger_path).unwrap()).unwrap();
    ledger["attempts"][0]["log_path"] = serde_json::Value::String(
        state
            .join("logs")
            .join("T9-a1-impl.log")
            .to_string_lossy()
            .into_owned(),
    );
    fs::write(&ledger_path, serde_json::to_string(&ledger).unwrap()).unwrap();

    let before = snapshot(&state);
    assert!(!before.is_empty());
    with_state_env(&state, &git, || {
        let data = dispatcher_data();
        let _ = dispatcher_page_html(&data);
        let _ = dispatcher_fragment_html(&data);
    });
    // STOP-toggle writes are the POST path's job; GETs leave STOP alone.
    assert!(!state.join("STOP").exists());
    let after = snapshot(&state);
    assert_eq!(before.len(), after.len());
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}
