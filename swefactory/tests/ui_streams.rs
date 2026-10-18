//! Live event streams (ticket #83, spec #66): transport + per-scope streams.
//!
//! Seam: external behavior only against a loopback server — HTTP status
//! codes, SSE event ordering (hello before snapshot before any delta),
//! parsed JSON payload fields, and rendered view-state markers. Tests never
//! assert wire bytes beyond ordering, store internals, or module layout.
//! The live seams (ledger via FACTORY_STATE_DIR, `gh` via FACTORY_GH_BIN,
//! git via FACTORY_GIT_BIN, keepalive via FACTORY_STREAM_KEEPALIVE_MS) are
//! faked so proofs stay hermetic. Polling fragments are byte-pinned with
//! streams on and off.

use std::fs;
use std::path::{Path, PathBuf};

use factory_lib::{web_router, STREAM_CONTRACT_VERSION, STREAM_KEEPALIVE_MS};

/// Serialize every test that mutates process env (one process per test
/// binary, but threads share env within it). A tokio mutex: async tests
/// `.await` it — never a std guard across an await point (clippy
/// `await_holding_lock`).
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uistreams-{}-{}-{}",
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

/// Fake `gh`: answers `issue list` with the board JSON and `issue view 9`
/// with the canned issue, fails everything else (missing tickets read as
/// calm 404s, never 500s).
fn write_fake_gh(dir: &Path, board_json: &str) -> PathBuf {
    write_script(
        dir,
        "gh",
        &format!(
            r#"#!/bin/sh
case "$*" in
*"issue list"*)
cat <<'EOF'
{board_json}
EOF
;;
*"issue view 9 "*)
cat <<'EOF'
{{"number":9,"title":"Sprout detail","body":"Grow the sprout.\nCost: 2 points","labels":[{{"name":"ready"}}],"state":"open"}}
EOF
;;
*) exit 1;;
esac
"#
        ),
    )
}

/// Fake `git`: answers `worktree list` with the canned lines, fails
/// everything else (no history hits, no crashes).
fn write_fake_git(dir: &Path, worktree_list: &str) -> PathBuf {
    write_script(
        dir,
        "git-fake",
        &format!(
            "#!/bin/sh\nif echo \"$*\" | grep -q \"worktree list\"; then\ncat <<'EOF'\n{worktree_list}\nEOF\nelse\nexit 1\nfi\n"
        ),
    )
}

const BOARD_JSON: &str = r#"[{"number":9,"title":"Sprout detail","body":"Grow the sprout.\nCost: 2 points","labels":[{"name":"ready"}],"state":"open"}]"#;

/// Ledger with one running attempt for ticket 9 plus its log. Returns the
/// state dir. The status can be rewritten mid-test to seed an attempt
/// transition (the delta trigger).
fn seed_state(root: &Path, status: &str) -> PathBuf {
    let state = root.join("state");
    let logs = state.join("logs");
    fs::create_dir_all(&logs).unwrap();
    let log = logs.join("T9-a1.log");
    fs::write(&log, "a1 implement line 1\na1 implement line 2\n").unwrap();
    let ledger = serde_json::json!({
        "attempts": [
            {"id": 1, "ticket": 9, "attempt_no": 1, "base_commit": "base1111", "head_commit": "",
             "branch": "factory/T9-x", "worktree": "/tmp/wt1", "status": status, "exit_code": 0,
             "log_path": log.to_string_lossy(), "started_at": "100", "finished_at": ""}
        ]
    });
    fs::write(
        state.join("dispatch.json"),
        serde_json::to_string_pretty(&ledger).unwrap(),
    )
    .unwrap();
    state
}

/// Rewrite the single attempt's status in place (a seeded transition).
fn set_attempt_status(state: &Path, status: &str) {
    let path = state.join("dispatch.json");
    let mut ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    ledger["attempts"][0]["status"] = serde_json::Value::String(status.to_string());
    if status != "impl_running" {
        ledger["attempts"][0]["finished_at"] = serde_json::Value::String("200".to_string());
    }
    fs::write(path, serde_json::to_string_pretty(&ledger).unwrap()).unwrap();
}

async fn serve_port() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, web_router()).await;
    });
    (port, handle)
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

/// Body of a raw HTTP/1.1 response (headers carry the clock, so
/// byte-identity is a body property, never a `Date:` property).
fn body_of(raw: &str) -> &str {
    match raw.find("\r\n\r\n") {
        Some(pos) => &raw[pos + 4..],
        None => raw,
    }
}
/// One live stream connection: a plain OS thread owns the socket,
/// splits the SSE byte flow into `\n\n`-terminated blocks, and forwards
/// each block over a channel. The async test pulls blocks with timeouts,
/// so a seeded ledger rewrite lands mid-connection and the delta is
/// observed on the same connection that saw the snapshot.
struct LiveTap {
    blocks: tokio::sync::mpsc::UnboundedReceiver<String>,
}

impl LiveTap {
    async fn attach(port: u16, path: &str) -> Self {
        let path = path.to_string();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(std::time::Duration::from_millis(300)))
                .unwrap();
            s.write_all(
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n\r\n").as_bytes(),
            )
            .unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match s.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        // Split off complete `\n\n`-terminated blocks on the
                        // raw bytes (chunked framing never matters: only
                        // `event:`/`data:`/`:` lines are parsed later).
                        while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
                            let block = String::from_utf8_lossy(&buf[..pos]).into_owned();
                            buf = buf[pos + 2..].to_vec();
                            if tx.send(block).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                    Err(_) => break,
                }
            }
        });
        Self { blocks: rx }
    }

    /// Next `\n\n`-terminated block within `timeout`, or `None`.
    async fn next_block(&mut self, timeout: std::time::Duration) -> Option<String> {
        tokio::time::timeout(timeout, self.blocks.recv())
            .await
            .ok()
            .flatten()
    }
}

/// Split a raw SSE block into its event name (if any) and data lines.
/// Comment/keepalive blocks (`: ...`) yield `(None, vec![])`. Chunked
/// framing lines never start with these prefixes, so they are ignored.
fn parse_block(block: &str) -> (Option<String>, Vec<String>) {
    let mut event = None;
    let mut data = Vec::new();
    for line in block.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix("event:") {
            event = Some(name.trim().to_string());
        } else if let Some(payload) = line.strip_prefix("data:") {
            data.push(payload.trim().to_string());
        }
    }
    (event, data)
}

fn hello_payload(data: &[String]) -> serde_json::Value {
    assert_eq!(data.len(), 1, "hello carries one data line, got: {data:?}");
    serde_json::from_str(&data[0]).expect("hello data is JSON")
}

/// Skip keepalive/comment blocks until the next real event arrives.
async fn next_event(tap: &mut LiveTap, timeout: std::time::Duration) -> (String, Vec<String>) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for the next stream event"
        );
        let block = tap
            .next_block(remaining)
            .await
            .expect("stream stays open while waiting for an event");
        let (event, data) = parse_block(&block);
        if let Some(name) = event {
            return (name, data);
        }
    }
}

fn set_stream_env(state: &Path, gh: &Path, git: &Path) {
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", state);
        std::env::set_var("FACTORY_GH_BIN", gh);
        std::env::set_var("FACTORY_GIT_BIN", git);
        std::env::set_var("FACTORY_STREAM_KEEPALIVE_MS", "200");
    }
}

fn clear_stream_env() {
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
        std::env::remove_var("FACTORY_GIT_BIN");
        std::env::remove_var("FACTORY_STREAM_KEEPALIVE_MS");
    }
}

#[test]
fn stream_contract_defaults_are_documented() {
    assert_eq!(STREAM_CONTRACT_VERSION, 1, "contract starts at v1");
    assert_eq!(
        STREAM_KEEPALIVE_MS, 15_000,
        "documented keepalive matches the board poll beat"
    );
    assert_eq!(
        factory_lib::stream_keepalive_ms(),
        15_000,
        "unset env reads as the default"
    );
}

#[tokio::test]
async fn board_stream_opens_with_versioned_hello_then_snapshot() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("hello-board");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    let mut tap = LiveTap::attach(port, "/streams/board").await;
    let (first, first_data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(first, "hello", "every stream opens with hello");
    let hello = hello_payload(&first_data);
    assert_eq!(hello["version"], STREAM_CONTRACT_VERSION);
    assert_eq!(hello["scope"], "board");
    assert!(
        hello["server_time"].as_u64().unwrap_or(0) > 0,
        "hello names server time, got: {hello}"
    );

    let (second, second_data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(second, "snapshot", "snapshot precedes any delta");
    assert_eq!(second_data.len(), 1);
    let snapshot: serde_json::Value = serde_json::from_str(&second_data[0]).unwrap();
    let html = snapshot["html"].as_str().unwrap();
    assert!(
        html.contains("data-board-columns"),
        "snapshot pushes exactly what the board fragment shows, got:\n{html}"
    );
    assert!(
        html.contains("Sprout detail"),
        "snapshot carries the seeded issue, got:\n{html}"
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn ticket_and_dispatch_scopes_open_the_same_way() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("hello-scopes");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    let mut ticket = LiveTap::attach(port, "/streams/ticket/9").await;
    let (event, data) = next_event(&mut ticket, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let hello = hello_payload(&data);
    assert_eq!(hello["version"], STREAM_CONTRACT_VERSION);
    assert_eq!(hello["scope"], "ticket");
    assert_eq!(hello["ticket"], 9);
    let (event, data) = next_event(&mut ticket, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"]
            .as_str()
            .unwrap()
            .contains("data-ticket=\"9\""),
        "ticket snapshot mirrors the detail fragment, got: {}",
        snapshot["html"]
    );

    let mut live = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, data) = next_event(&mut live, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let hello = hello_payload(&data);
    assert_eq!(hello["scope"], "dispatcher-live");
    let (event, data) = next_event(&mut live, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let html = snapshot["html"].as_str().unwrap();
    assert!(html.contains("data-dispatch"), "got:\n{html}");
    assert!(
        html.contains("data-attempt=\"T9-a1\""),
        "live snapshot mirrors the dispatch fragment, got:\n{html}"
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn version_mismatch_fails_loudly_on_every_scope() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("version");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    for path in [
        "/streams/board?version=9999",
        "/streams/ticket/9?version=9999",
        "/streams/dispatch?version=9999",
    ] {
        let resp = get(port, path).await;
        assert!(
            resp.contains("400"),
            "mismatched client fails loudly at hello, got:\n{resp}"
        );
        assert!(
            resp.contains(&STREAM_CONTRACT_VERSION.to_string()),
            "the refusal names the expected contract version, got:\n{resp}"
        );
    }

    // The matching version streams fine (hello still leads).
    let mut tap = LiveTap::attach(
        port,
        &format!("/streams/board?version={STREAM_CONTRACT_VERSION}"),
    )
    .await;
    let (event, _) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn unknown_ticket_stream_404s_calmly() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("missing");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    for path in ["/streams/ticket/999", "/streams/ticket/abc"] {
        let resp = get(port, path).await;
        assert!(
            resp.contains("404"),
            "no stream for unknown tickets, got:\n{resp}"
        );
    }

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn seeded_attempt_transition_arrives_as_delta_after_snapshot() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("delta");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    let mut tap = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let before = snapshot["html"].as_str().unwrap().to_string();
    assert!(
        before.contains("data-status=\"impl_running\""),
        "got:\n{before}"
    );

    // Seed the transition the matching scope already shows.
    set_attempt_status(&state, "impl_done");

    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(
        event, "delta",
        "transitions ride as deltas, never a second snapshot"
    );
    let delta: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let after = delta["html"].as_str().unwrap();
    assert!(
        after.contains("data-status=\"impl_done\""),
        "delta carries the seeded transition, got:\n{after}"
    );
    assert_ne!(before, after, "delta bytes move with the transition");

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn second_client_sees_live_state_immediately_and_viewers_agree() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("second-client");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    // First viewer attaches at the old state.
    let mut first = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let (event, data) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let first_snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(first_snapshot["html"]
        .as_str()
        .unwrap()
        .contains("impl_running"));

    // The loop moves while the first viewer watches.
    set_attempt_status(&state, "impl_done");

    // A second viewer attaches after the move: its snapshot is live now,
    // with no wait for a delta.
    let mut second = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let (event, data) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let second_snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let second_html = second_snapshot["html"].as_str().unwrap().to_string();
    assert!(
        second_html.contains("data-status=\"impl_done\""),
        "late attach lands on current state, got:\n{second_html}"
    );

    // The first viewer converges on the same bytes within the documented
    // keepalive interval: two viewers never disagree past it.
    let (event, data) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "delta");
    let converged: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert_eq!(
        converged["html"].as_str().unwrap(),
        second_html,
        "both viewers read identical live state"
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn polling_views_render_byte_identically_with_streams_on_and_off() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("poll-identical");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    let paths = [
        "/",
        "/fragments/board",
        "/tickets/9",
        "/fragments/ticket/9",
        "/dispatch",
        "/fragments/dispatch",
    ];
    let mut before = Vec::new();
    for path in paths {
        before.push(body_of(&get(port, path).await).to_string());
    }

    // Attach one stream per scope; each must be alive (hello received)
    // so the "streams on" side is real.
    let mut taps = Vec::new();
    for path in ["/streams/board", "/streams/ticket/9", "/streams/dispatch"] {
        let mut tap = LiveTap::attach(port, path).await;
        let (event, _) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(event, "hello", "stream {path} is live");
        taps.push(tap);
    }

    for (path, body_before) in paths.iter().zip(before.iter()) {
        let raw_after = get(port, path).await;
        let body_after = body_of(&raw_after);
        assert_eq!(
            body_after,
            body_before.as_str(),
            "polling view {path} is byte-identical with streams attached"
        );
    }

    drop(taps);
    server.abort();
    clear_stream_env();
}
