//! Live event streams (tickets #83-#85, spec #66): transport, per-scope
//! streams, reconnects, slow viewers, and the read-only boundary.
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

/// Grow the attempt log to roughly `lines` x `line_len` bytes: the
/// dispatcher fragment embeds a bounded tail of UNBOUNDED lines, so long
/// lines make large snapshots cheaply (no extra subprocesses per tick).
fn grow_log(state: &Path, lines: usize, line_len: usize) {
    let path = state.join("logs").join("T9-a1.log");
    let mut out = String::new();
    for i in 0..lines {
        out.push_str(&format!("bulk line {i:04} "));
        while out.len() < (i + 1) * (line_len + 20) {
            out.push('x');
        }
        out.push('\n');
    }
    fs::write(path, out).unwrap();
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

/// Blocking std HTTP/1.1 request with any method over loopback (the
/// read-only-boundary tests attempt mutations over the stream surface).
/// Same blocking-thread discipline as `get`: never inline on the
/// single-threaded `#[tokio::test]` runtime.
async fn request(port: u16, method: &str, path: &str) -> String {
    let (method, path) = (method.to_string(), path.to_string());
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
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

/// Byte snapshot of a state dir: every file's relative path plus bytes,
/// sorted. Rendering or refusing must never mutate: before and after
/// must compare equal, so no ledger, log, worktree, or issue state moved.
fn snapshot_dir(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(top) = stack.pop() {
        for entry in fs::read_dir(&top).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}
/// One live stream connection: a plain OS thread owns the socket,
/// splits the SSE byte flow into `\n\n`-terminated blocks, and forwards
/// each block over a channel. The async test pulls blocks with timeouts,
/// so a seeded ledger rewrite lands mid-connection and the delta is
/// observed on the same connection that saw the snapshot.
///
/// The thread also honors a pause flag (stalled-viewer tests stop the
/// socket reads so TCP backpressure is real, not simulated) and keeps a
/// killer handle so a test can kill the connection mid-stream.
struct LiveTap {
    blocks: tokio::sync::mpsc::UnboundedReceiver<String>,
    killer: std::net::TcpStream,
    paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl LiveTap {
    async fn attach(port: u16, path: &str) -> Self {
        let path = path.to_string();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_paused = paused.clone();
        // Killer handle: shutting it down kills the connection mid-stream
        // (the reconnect tests' external kill switch).
        let probe = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let killer = probe.try_clone().unwrap();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut s = probe;
            s.set_read_timeout(Some(std::time::Duration::from_millis(300)))
                .unwrap();
            s.write_all(
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n\r\n").as_bytes(),
            )
            .unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                // Stalled-viewer tests pause the socket reads: unread
                // bytes pile up in kernel buffers until the server feels
                // real TCP backpressure.
                if thread_paused.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
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
        Self {
            blocks: rx,
            killer,
            paused,
        }
    }

    /// Kill the connection mid-stream from the outside (the server sees
    /// the socket die, exactly like a dropped viewer).
    fn kill(&self) {
        let _ = self.killer.shutdown(std::net::Shutdown::Both);
    }

    /// Pause (true) or resume (false) the socket reads. Paused, the test
    /// stops draining TCP, so the server's next deltas meet real
    /// backpressure past the kernel buffers.
    fn set_paused(&self, paused: bool) {
        self.paused
            .store(paused, std::sync::atomic::Ordering::Relaxed);
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

/// Documented live rules for one test: silence bound (ms) plus buffer
/// bound (events). The hello on every new connection advertises the
/// effective values, so viewers and tests read the rule from the wire.
fn set_stream_rule_env(silence_after_ms: &str, buffer_bound: &str) {
    unsafe {
        std::env::set_var("FACTORY_STREAM_SILENCE_AFTER_MS", silence_after_ms);
        std::env::set_var("FACTORY_STREAM_BUFFER_BOUND", buffer_bound);
    }
}

fn clear_stream_env() {
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
        std::env::remove_var("FACTORY_GIT_BIN");
        std::env::remove_var("FACTORY_STREAM_KEEPALIVE_MS");
        std::env::remove_var("FACTORY_STREAM_SILENCE_AFTER_MS");
        std::env::remove_var("FACTORY_STREAM_BUFFER_BOUND");
    }
}

#[tokio::test]
async fn stream_contract_defaults_are_documented() {
    let _guard = ENV_LOCK.lock().await;
    clear_stream_env();
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
        event, "commit",
        "transitions announce as one commit entry beside the delta"
    );
    let entry: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert_eq!(entry["scope"], "dispatcher-live", "got: {entry}");
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
    // keepalive interval: two viewers never disagree past it. The
    // transition announces as one commit entry beside its delta.
    let (event, _) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "commit");
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

// --- ticket #84 (spec #66): reconnects + slow viewers ---------------------
//
// Fresh-snapshot-always: a new connection gets hello, then the whole
// current state, then deltas. Streams never replay, so duplicates are
// impossible by construction and a viewer that was offline simply missed
// the middle. A documented silence rule declares half-open sockets dead;
// viewers past the documented buffer bound are disconnected with a
// re-attach instruction instead of buffering forever.

#[tokio::test]
async fn stream_silence_and_buffer_bounds_are_documented() {
    let _guard = ENV_LOCK.lock().await;
    clear_stream_env();
    assert_eq!(
        factory_lib::STREAM_SILENCE_AFTER_MS,
        3 * factory_lib::STREAM_KEEPALIVE_MS,
        "silence bound is documented as three quiet keepalives"
    );
    assert!(
        factory_lib::stream_buffer_bound() >= 2,
        "the effective bound always fits hello, snapshot, and a delta"
    );
    // The hello advertises the live rules so viewers (and tests) read
    // them from the wire instead of the source.
    let hello = factory_lib::stream_hello_json("board", None);
    for field in [
        "keepalive_ms",
        "silence_after_ms",
        "buffer_bound",
        "partial_ms",
    ] {
        assert!(
            hello
                .get(field)
                .and_then(|v| v.as_u64())
                .is_some_and(|v| v > 0),
            "hello advertises the {field} rule, got: {hello}"
        );
    }
}

#[tokio::test]
async fn board_stream_hello_advertises_the_effective_live_rules() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("hello-rules");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "");
    set_stream_env(&state, &gh, &git);
    set_stream_rule_env("777", "9");
    let (port, server) = serve_port().await;

    let mut tap = LiveTap::attach(port, "/streams/board").await;
    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let hello = hello_payload(&data);
    assert_eq!(
        hello["keepalive_ms"], 200,
        "effective keepalive, got: {hello}"
    );
    assert_eq!(
        hello["silence_after_ms"], 777,
        "effective silence bound, got: {hello}"
    );
    assert_eq!(
        hello["buffer_bound"], 9,
        "effective buffer bound, got: {hello}"
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn kill_midstream_then_reconnect_lands_on_current_with_no_duplicates() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("kill-reconnect");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    set_stream_rule_env("2000", "64");
    let (port, server) = serve_port().await;

    // First connection sees the old state, then dies mid-stream.
    let mut first = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let (event, data) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"].as_str().unwrap().contains("impl_running"),
        "first snapshot shows the old state"
    );
    first.kill();
    drop(first);

    // The loop moves while nobody watches.
    set_attempt_status(&state, "impl_done");

    // Reconnect: hello, then ONE fresh snapshot of now — never a replay.
    let mut second = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello", "reconnect opens with hello, not a replay");
    let (event, data) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot", "reconnect lands on a fresh snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"]
            .as_str()
            .unwrap()
            .contains("data-status=\"impl_done\""),
        "fresh snapshot shows current state, got: {}",
        snapshot["html"]
    );

    // Nothing moves after the reconnect, so no delta may repeat anything:
    // the middle was missed, not queued.
    let quiet_until = std::time::Instant::now() + std::time::Duration::from_millis(600);
    while std::time::Instant::now() < quiet_until {
        let remaining = quiet_until.saturating_duration_since(std::time::Instant::now());
        if let Some(block) = second.next_block(remaining).await {
            let (event, _) = parse_block(&block);
            assert!(
                event.as_deref() != Some("delta") && event.as_deref() != Some("snapshot"),
                "no replayed events after a fresh snapshot, got: {event:?}"
            );
        }
    }

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn quiet_connection_past_silence_bound_counts_as_dead() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("silence");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "");
    set_stream_env(&state, &gh, &git);
    unsafe {
        // No keepalive beats during the test: the connection is quiet.
        std::env::set_var("FACTORY_STREAM_KEEPALIVE_MS", "3600000");
    }
    set_stream_rule_env("300", "64");
    let (port, server) = serve_port().await;

    let mut tap = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let hello = hello_payload(&data);
    assert_eq!(
        hello["silence_after_ms"], 300,
        "hello names the silence bound, got: {hello}"
    );
    let (event, _) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");

    // Past the documented bound with no bytes at all, the viewer counts
    // the connection dead (the server never fakes liveness) and
    // re-attaches to current state.
    assert!(
        tap.next_block(std::time::Duration::from_millis(900))
            .await
            .is_none(),
        "a quiet connection stays quiet past the silence bound"
    );
    drop(tap);
    let mut reattached = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut reattached, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello", "re-attach opens with hello");
    let (event, data) = next_event(&mut reattached, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot", "re-attach lands on current state");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"].as_str().unwrap().contains("data-dispatch"),
        "re-attached snapshot is live state, got: {}",
        snapshot["html"]
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
// Ticket #89 (flake quarantine): this test drives the bounded-queue
// disconnect through REAL TCP backpressure, which is timing-dependent
// and failed intermittently on timing-neutral commits (local #84 attempt
// 1, CI on the rc.7 tag which touched only version/changelog, CI on the
// #88 landing). Mechanism: the collapse needs 3 detected changes queued
// while the socket is stalled, but each 40ms producer tick re-renders a
// ~1.5MB fragment (ticks alias against the 60ms toggle beat under load)
// and kernel loopback buffers absorb whole deltas before backpressure
// bites — so whether the bound trips depends on machine scheduling, not
// the logic. 15 consecutive local runs (9 idle + 6 under synthetic CPU
// load, plus the full binary) all passed, which is itself the signature:
// the trigger is environmental, not reproducible on demand. The collapse
// logic itself is pinned deterministically by
// `stream_backpressure_tests` in src/lib.rs (no sockets, no clock).
// RE-ENABLE when the stall is driven deterministically — e.g. a test-only
// hook capping the connection's socket buffers or a virtual producer
// clock — so the disconnect no longer depends on real TCP timing.
#[ignore = "ticket #89: real-TCP-backpressure timing flake; re-enable on a deterministic stall hook"]
async fn stalled_consumer_past_buffer_bound_gets_reattach_then_disconnects() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("stall");
    let state = seed_state(&root, "impl_running");
    // Long transcript lines make ~1.5MB snapshots cheaply: the fragment
    // embeds a bounded tail of unbounded lines, with no per-line fan-out.
    grow_log(&state, 50, 30_000);
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    set_stream_rule_env("30000", "4");
    let (port, server) = serve_port().await;

    let mut tap = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut tap, std::time::Duration::from_secs(15)).await;
    assert_eq!(event, "hello");
    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(15)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"].as_str().unwrap().len() > 1_000_000,
        "snapshots are large enough to fill kernel buffers"
    );

    // Stall: stop draining TCP while the loop keeps moving. Each toggle
    // is a ~1.5MB delta, so the kernel buffers fill and the server must
    // disconnect instead of buffering forever.
    tap.set_paused(true);
    let state_clone = state.clone();
    // Blocking sleeps must NOT run on the test runtime thread:
    // `#[tokio::test]` is single-threaded, so sleeping inline would
    // starve the server task (and its producer ticks) for the whole
    // toggle run. A blocking thread keeps the loop moving while the
    // server stays schedulable.
    tokio::task::spawn_blocking(move || {
        for i in 0..40 {
            set_attempt_status(
                &state_clone,
                if i % 2 == 0 {
                    "impl_done"
                } else {
                    "impl_running"
                },
            );
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
    })
    .await
    .unwrap();
    tap.set_paused(false);

    // Drain: stale deltas, then the terminal re-attach instruction,
    // then the server ends the stream.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut saw_reattach = false;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "stalled consumer was never disconnected past the buffer bound"
        );
        match tap.next_block(std::time::Duration::from_secs(3)).await {
            None => break,
            Some(block) => {
                let (event, data) = parse_block(&block);
                if event.as_deref() == Some("reattach") {
                    assert_eq!(data.len(), 1, "reattach carries one JSON line");
                    let payload: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
                    assert!(
                        payload["message"].as_str().unwrap().contains("fell behind"),
                        "calm copy names what happened, got: {payload}"
                    );
                    assert_eq!(
                        payload["action"], "Re-attach",
                        "one re-attach action, got: {payload}"
                    );
                    assert_eq!(
                        payload["stream"], "/streams/dispatch",
                        "instruction names the way back, got: {payload}"
                    );
                    saw_reattach = true;
                }
            }
        }
    }
    assert!(
        saw_reattach,
        "the stalled viewer is disconnected WITH a re-attach instruction"
    );

    // The instruction works: re-attaching lands on current state.
    set_attempt_status(&state, "impl_done");
    let mut reattached = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut reattached, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let (event, data) = next_event(&mut reattached, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"]
            .as_str()
            .unwrap()
            .contains("data-status=\"impl_done\""),
        "re-attach after disconnect shows current state"
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn dropped_and_reopened_viewer_returns_to_current_with_place_kept() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("reopen");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "");
    set_stream_env(&state, &gh, &git);
    set_stream_rule_env("2000", "64");
    let (port, server) = serve_port().await;

    // Bea watches ticket 9, then drops the browser.
    let mut first = LiveTap::attach(port, "/streams/ticket/9").await;
    let (event, data) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    assert_eq!(hello_payload(&data)["ticket"], 9);
    let (event, data) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"]
            .as_str()
            .unwrap()
            .contains("data-ticket=\"9\""),
        "first view holds its place on ticket 9"
    );
    first.kill();
    drop(first);

    // The loop moves while she is gone; the middle is missed, not queued.
    set_attempt_status(&state, "impl_done");

    // Reopening the same place lands on current state, place kept.
    let mut second = LiveTap::attach(port, "/streams/ticket/9").await;
    let (event, data) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    assert_eq!(
        hello_payload(&data)["ticket"],
        9,
        "reopened viewer keeps its place on ticket 9"
    );
    let (event, data) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot", "reopen lands on a fresh snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let html = snapshot["html"].as_str().unwrap();
    assert!(
        html.contains("data-ticket=\"9\""),
        "place kept on ticket 9, got: {html:.200}"
    );
    assert!(
        html.contains("finished implement, awaiting review"),
        "reopened view shows current state, got: {html:.200}"
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn live_pages_carry_the_stream_hook_and_calm_reattach_copy() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("page-hook");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "");
    set_stream_env(&state, &gh, &git);
    set_stream_rule_env("2000", "64");
    let (port, server) = serve_port().await;

    for (page, scope, stream) in [
        ("/", "board", "/streams/board"),
        ("/tickets/9", "ticket", "/streams/ticket/9"),
        ("/dispatch", "dispatcher-live", "/streams/dispatch"),
    ] {
        let raw = get(port, page).await;
        assert!(raw.contains("200"), "page {page} serves, got:\n{raw}");
        let body = body_of(&raw).to_string();
        assert!(
            body.contains(&format!("data-stream-scope=\"{scope}\"")),
            "page {page} names its stream scope, got:\n{body:.500}"
        );
        assert!(
            body.contains(&format!("data-stream-url=\"{stream}\"")),
            "page {page} names its stream URL, got:\n{body:.500}"
        );
        assert!(
            body.contains("data-stream-silence-after-ms="),
            "page {page} documents the silence bound, got:\n{body:.500}"
        );
        assert!(
            body.contains("data-stream-banner"),
            "page {page} carries the disconnected banner, got:\n{body:.500}"
        );
        assert!(
            body.contains("disconnected") && body.contains("Re-attach"),
            "page {page} reads as calm copy with one action, got:\n{body:.500}"
        );
    }

    server.abort();
    clear_stream_env();
}

// --- ticket #85 (spec #66): read-only boundary + single-scope lock-in ---
//
// Streams are structurally read-only: the stream surface admits no
// mutation (no writes, stops, lands, or dispatches), closing every
// client leaves the running loop untouched, every scope pushes exactly
// the bytes its polling fragment already shows, and one connection
// serves one scope (multiplexing stays deferred). These tests pin that
// boundary against the loopback server; the hermetic seams (seeded
// ledger, fake `gh`/`git`, no model calls in the stream path) mean the
// suite holds with no external network and no spend.

#[tokio::test]
async fn stream_surface_admits_no_mutation() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("readonly-boundary");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    let before = snapshot_dir(&state);

    // Every mutating method on every stream scope: refused, never opened.
    for (method, path) in [
        ("POST", "/streams/board"),
        ("PUT", "/streams/board"),
        ("DELETE", "/streams/board"),
        ("PATCH", "/streams/board"),
        ("POST", "/streams/ticket/9"),
        ("PUT", "/streams/ticket/9"),
        ("DELETE", "/streams/ticket/9"),
        ("POST", "/streams/dispatch"),
        ("PUT", "/streams/dispatch"),
        ("DELETE", "/streams/dispatch"),
    ] {
        let resp = request(port, method, path).await;
        let status = resp.lines().next().unwrap_or("").to_string();
        assert!(
            status.contains("405"),
            "{method} {path} is refused at the method boundary, got: {status}"
        );
        assert!(
            !resp.contains("event:"),
            "{method} {path} opens no stream, got:\n{resp:.500}"
        );
    }

    // The refusal changed no ledger, log, worktree, or issue state: the
    // whole state dir compares byte-equal, so no new file (STOP flag or
    // otherwise) appeared and no byte moved.
    let after = snapshot_dir(&state);
    assert_eq!(
        before, after,
        "refused mutations leave every ledger and log byte untouched"
    );
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state.join("dispatch.json")).unwrap()).unwrap();
    assert_eq!(
        ledger["attempts"][0]["status"], "impl_running",
        "the seeded attempt is exactly where the loop left it"
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn stream_snapshots_push_only_what_polling_shows() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("no-new-facts");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    // Structural, pinned by test: each scope's producer re-reads the same
    // render call its polling fragment serves, so a stream can only ever
    // push facts the views already show — no second pipeline exists to
    // drift. Quiet ledger throughout: no rewrite between the snapshot
    // and the fragment GET, so byte-equality is exact, not eventual.
    for (stream_path, fragment_path) in [
        ("/streams/board", "/fragments/board"),
        ("/streams/ticket/9", "/fragments/ticket/9"),
        ("/streams/dispatch", "/fragments/dispatch"),
    ] {
        let mut tap = LiveTap::attach(port, stream_path).await;
        let (event, _) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(event, "hello", "{stream_path} opens with hello");
        let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(event, "snapshot", "{stream_path} snapshots before deltas");
        let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
        let pushed = snapshot["html"].as_str().unwrap().to_string();

        let raw = get(port, fragment_path).await;
        assert!(
            raw.contains("200"),
            "polling fragment {fragment_path} serves, got:\n{raw:.300}"
        );
        assert_eq!(
            pushed,
            body_of(&raw),
            "{stream_path} pushes byte-identical bytes to {fragment_path}"
        );
    }

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn one_connection_serves_one_scope() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("single-scope");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    // Each scope is independently verifiable: its snapshot carries its
    // own section root and none of the other scopes' roots, so no
    // connection multiplexes several scopes (deferred, not forbidden).
    // Roots are the full `<section ...>` open tags: bare attribute
    // substrings (e.g. `data-ticket="9"`) legitimately nest inside the
    // dispatch cards, so only section roots prove isolation.
    for (stream_path, own, others) in [
        (
            "/streams/board",
            "<section data-board-columns>",
            vec!["<section data-dispatch>", "<section data-ticket="],
        ),
        (
            "/streams/ticket/9",
            "<section data-ticket=\"9\">",
            vec!["<section data-board-columns>", "<section data-dispatch>"],
        ),
        (
            "/streams/dispatch",
            "<section data-dispatch>",
            vec!["<section data-board-columns>", "<section data-ticket="],
        ),
    ] {
        let mut tap = LiveTap::attach(port, stream_path).await;
        let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(event, "hello");
        let hello = hello_payload(&data);
        let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(event, "snapshot");
        let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
        let html = snapshot["html"].as_str().unwrap().to_string();
        assert!(
            html.contains(own),
            "{stream_path} serves its own scope ({own}), got:\n{html:.300}"
        );
        for other in others {
            assert!(
                !html.contains(other),
                "{stream_path} carries no {other} facts (hello scope: {})",
                hello["scope"]
            );
        }
    }

    // No scope-combining surface exists: the bare path 404s, and junk
    // query params never switch a connection off its route's scope.
    let bare = get(port, "/streams").await;
    assert!(
        bare.contains("404"),
        "no multiplex endpoint at /streams, got:\n{bare:.300}"
    );
    let mut tap = LiveTap::attach(port, "/streams/board?scope=ticket&foo=bar").await;
    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    assert_eq!(
        hello_payload(&data)["scope"],
        "board",
        "the route fixes the scope, not the query string"
    );
    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"]
            .as_str()
            .unwrap()
            .contains("data-board-columns"),
        "junk params leave the board scope on the board stream"
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn closing_every_client_leaves_the_loop_running() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("close-all");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    let (port, server) = serve_port().await;

    // One viewer per scope, every one provably live (hello received).
    let mut board = LiveTap::attach(port, "/streams/board").await;
    let mut ticket = LiveTap::attach(port, "/streams/ticket/9").await;
    let mut live = LiveTap::attach(port, "/streams/dispatch").await;
    for tap in [&mut board, &mut ticket, &mut live] {
        let (event, _) = next_event(tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(event, "hello", "every client is attached and live");
    }

    // Mid-attempt, every client closes at once: the running loop is not
    // consulted, warned, or touched — viewers are observers, never owners.
    board.kill();
    ticket.kill();
    live.kill();
    drop(board);
    drop(ticket);
    drop(live);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // The loop completes normally with nobody watching: the seeded
    // transition lands exactly as it would with viewers attached.
    set_attempt_status(&state, "impl_done");
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state.join("dispatch.json")).unwrap()).unwrap();
    assert_eq!(
        ledger["attempts"][0]["status"], "impl_done",
        "the attempt completes with zero clients attached"
    );

    // Polling shows the completion, and a fresh attach lands on it: the
    // server kept serving the whole time nobody watched.
    let raw = get(port, "/fragments/dispatch").await;
    assert!(
        body_of(&raw).contains("data-status=\"impl_done\""),
        "polling shows the completion, got:\n{}",
        body_of(&raw)
    );
    let mut reattached = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut reattached, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let (event, data) = next_event(&mut reattached, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"]
            .as_str()
            .unwrap()
            .contains("data-status=\"impl_done\""),
        "fresh snapshot after close-all shows the completed loop"
    );

    server.abort();
    clear_stream_env();
}

// --- ticket #74 (spec #66 follow-up): task-graph watch + compact feed + partial output ---
//
// Three stream-transport gaps as one delivery, all riding the existing
// transport and scopes (board, ticket, dispatcher-live): no new scopes,
// no new auth, no replay. Tests pin external behavior only against a
// loopback server: event order plus parsed payloads plus view-state
// markers, never wire bytes beyond ordering or store internals.

/// Rewrite the ledger's live-task nodes in place (the graph producers'
/// write path; streams only ever read). Fields mirror LiveTask's serde
/// shape; absent optionals parse by default.
fn set_ledger_tasks(state: &Path, tasks: serde_json::Value) {
    let path = state.join("dispatch.json");
    let mut ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    ledger["tasks"] = tasks;
    fs::write(path, serde_json::to_string_pretty(&ledger).unwrap()).unwrap();
}

/// Append raw lines to the seeded attempt's log without touching its
/// status: the call is still going, only its output grows.
fn append_log_lines(state: &Path, extra: &str) {
    let path = state.join("logs").join("T9-a1.log");
    let mut cur = fs::read_to_string(&path).unwrap();
    cur.push_str(extra);
    fs::write(path, cur).unwrap();
}

/// Count literal occurrences of a view-state marker.
fn count_occurrences(hay: &str, needle: &str) -> usize {
    hay.match_indices(needle).count()
}

fn live_tasks_fixture() -> serde_json::Value {
    serde_json::json!([
        {"id": "t1", "ticket": 9, "attempt": 1, "parent": null, "kind": "parent", "status": "running", "waits_on": []},
        {"id": "t2", "ticket": 9, "attempt": 1, "parent": "t1", "kind": "child", "status": "waiting", "waits_on": ["t3"]}
    ])
}

#[tokio::test]
async fn task_graph_streams_live_and_reconnects_never_duplicate_nodes() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("task-graph");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    // The graph exists before any viewer attaches: t1 runs, t2 waits on t3.
    set_ledger_tasks(&state, live_tasks_fixture());
    set_stream_env(&state, &gh, &git);
    set_stream_rule_env("2000", "64");
    let (port, server) = serve_port().await;

    // The late joiner starts from the current graph: one node per live id.
    let mut first = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let (event, data) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let html = snapshot["html"].as_str().unwrap().to_string();
    assert!(
        html.contains("data-task-graph"),
        "dispatcher snapshot carries the live graph, got:\n{html:.500}"
    );
    assert_eq!(count_occurrences(&html, "data-task-node=\"t1\""), 1);
    assert_eq!(count_occurrences(&html, "data-task-node=\"t2\""), 1);

    // The loop moves while viewers watch: t2 finishes (leaves the graph
    // on the commit that marks it terminal) and t3 joins.
    set_ledger_tasks(
        &state,
        serde_json::json!([
            {"id": "t1", "ticket": 9, "attempt": 1, "parent": null, "kind": "parent", "status": "running", "waits_on": []},
            {"id": "t2", "ticket": 9, "attempt": 1, "parent": "t1", "kind": "child", "status": "done", "waits_on": []},
            {"id": "t3", "ticket": 9, "attempt": 1, "parent": "t1", "kind": "child", "status": "running", "waits_on": []}
        ]),
    );

    // A second viewer joins late, after the move: its snapshot is the
    // current graph, never a replay of the middle.
    let before = fs::read(state.join("dispatch.json")).unwrap();
    let mut second = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let (event, data) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let html = snapshot["html"].as_str().unwrap().to_string();
    assert_eq!(
        count_occurrences(&html, "data-task-node=\"t1\""),
        1,
        "current graph keeps t1 exactly once, got:\n{html:.500}"
    );
    assert_eq!(
        count_occurrences(&html, "data-task-node=\"t3\""),
        1,
        "current graph carries the joined node exactly once, got:\n{html:.500}"
    );
    assert_eq!(
        count_occurrences(&html, "data-task-node=\"t2\""),
        0,
        "the terminal node left the graph, got:\n{html:.500}"
    );
    // Viewers are observers: attaching changed no ledger byte.
    assert_eq!(
        fs::read(state.join("dispatch.json")).unwrap(),
        before,
        "late attach is read-only"
    );

    // Reconnect after a kill: hello, then ONE fresh snapshot of now —
    // the middle was missed, nothing is duplicated.
    second.kill();
    drop(second);
    let mut reattached = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut reattached, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello", "reconnect opens with hello, not a replay");
    let (event, data) = next_event(&mut reattached, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot", "reconnect lands on a fresh snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let html = snapshot["html"].as_str().unwrap().to_string();
    for id in ["t1", "t3"] {
        assert_eq!(
            count_occurrences(&html, &format!("data-task-node=\"{id}\"")),
            1,
            "reconnected graph carries {id} exactly once, got:\n{html:.500}"
        );
    }
    assert_eq!(count_occurrences(&html, "data-task-node=\"t2\""), 0);

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn compact_feed_carries_one_entry_per_commit_on_every_scope() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("commit-feed");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    set_stream_rule_env("2000", "64");
    let (port, server) = serve_port().await;

    // One viewer per scope, every one past its snapshot.
    let mut board = LiveTap::attach(port, "/streams/board").await;
    let mut ticket = LiveTap::attach(port, "/streams/ticket/9").await;
    let mut live = LiveTap::attach(port, "/streams/dispatch").await;
    for tap in [&mut board, &mut ticket, &mut live] {
        let (event, _) = next_event(tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(event, "hello");
        let (event, _) = next_event(tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(event, "snapshot");
    }

    // One ledger commit: the running attempt finishes implement.
    set_attempt_status(&state, "impl_done");

    // Every scope carries exactly one compact entry beside its delta:
    // commit first, then the full state it derives from.
    for (tap, scope) in [
        (&mut board, "board"),
        (&mut ticket, "ticket"),
        (&mut live, "dispatcher-live"),
    ] {
        let (event, data) = next_event(tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(
            event, "commit",
            "{scope} announces the commit before its delta"
        );
        assert_eq!(data.len(), 1, "commit carries one JSON line");
        let entry: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
        assert_eq!(entry["scope"], scope, "got: {entry}");
        assert_eq!(
            entry["seq"], 1,
            "first commit on a fresh connection is seq 1, got: {entry}"
        );
        assert!(
            entry["commit"].as_str().is_some_and(|c| !c.is_empty()),
            "one entry names its commit, got: {entry}"
        );
        assert!(
            entry["server_time"].as_u64().is_some_and(|t| t > 0),
            "commit names server time, got: {entry}"
        );
        let changes = entry["changes"].as_array().cloned().unwrap_or_default();
        assert!(
            !changes.is_empty(),
            "the entry is sufficient without full state, got: {entry}"
        );
        let text = serde_json::to_string(&changes).unwrap();
        let want = match scope {
            "board" => "#9",
            "ticket" => "attempt 1",
            _ => "T9-a1",
        };
        assert!(
            text.contains(want) && text.contains("impl_done"),
            "a consumer that never wants full state still learns what moved, got: {entry}"
        );
        if scope == "ticket" {
            assert_eq!(entry["ticket"], 9, "got: {entry}");
        }

        let (event, data) = next_event(tap, std::time::Duration::from_secs(10)).await;
        assert_eq!(event, "delta", "{scope} follows with full state");
        let delta: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
        let delta_html = delta["html"].as_str().unwrap();
        let delta_want = match scope {
            "ticket" => "finished implement, awaiting review",
            _ => "impl_done",
        };
        assert!(
            delta_html.contains(delta_want),
            "{scope} delta carries the finished attempt"
        );
    }

    // Exactly one entry per commit: nothing else was committed, so no
    // scope may repeat or invent another entry.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
    for (tap, scope) in [
        (&mut board, "board"),
        (&mut ticket, "ticket"),
        (&mut live, "dispatcher-live"),
    ] {
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            if let Some(block) = tap.next_block(remaining).await {
                let (event, _) = parse_block(&block);
                assert!(
                    event.as_deref() != Some("commit") && event.as_deref() != Some("delta"),
                    "{scope} emits exactly one entry per commit, got extra: {event:?}"
                );
            }
        }
    }

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn running_output_surfaces_within_partial_interval_while_call_continues() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("partial-output");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    unsafe {
        // An hour-long keepalive: any fast arrival below is the partial
        // beat's doing, never the keepalive's.
        std::env::set_var("FACTORY_STREAM_KEEPALIVE_MS", "3600000");
        std::env::set_var("FACTORY_STREAM_PARTIAL_MS", "100");
    }
    set_stream_rule_env("30000", "64");
    let (port, server) = serve_port().await;

    let mut tap = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    assert_eq!(
        hello_payload(&data)["partial_ms"],
        100,
        "hello advertises the documented sub-second interval"
    );
    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"]
            .as_str()
            .unwrap()
            .contains("a1 implement line 2"),
        "snapshot shows the seeded output"
    );

    // The call is still going (status untouched): only output grows.
    let started = std::time::Instant::now();
    append_log_lines(
        &state,
        "a1 implement line 3\npartial line 4 still running\n",
    );

    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "commit", "running output commits while going");
    let entry: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let text = serde_json::to_string(&entry["changes"]).unwrap();
    assert!(
        text.contains("T9-a1") && text.contains("+2 lines"),
        "the entry names the growing output, got: {entry}"
    );
    let (event, data) = next_event(&mut tap, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "delta");
    let delta: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    let html = delta["html"].as_str().unwrap().to_string();
    assert!(
        html.contains("partial line 4 still running"),
        "running output is visible mid-call, got:\n{html:.500}"
    );
    assert!(
        html.contains("data-status=\"impl_running\""),
        "the call is still going in the same delta, got:\n{html:.500}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "visible well inside any slow beat (keepalive is an hour)"
    );

    server.abort();
    clear_stream_env();
}

#[tokio::test]
async fn commit_feed_reconnects_from_snapshot_with_no_replay_and_changes_nothing() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("feed-reconnect");
    let state = seed_state(&root, "impl_running");
    let gh = write_fake_gh(&root, BOARD_JSON);
    let git = write_fake_git(&root, "/tmp/wt1 abc123 [factory/T9-x]");
    set_stream_env(&state, &gh, &git);
    set_stream_rule_env("2000", "64");
    let (port, server) = serve_port().await;

    let mut first = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello");
    let (event, _) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot");
    set_attempt_status(&state, "impl_done");
    let (event, data) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "commit");
    let entry: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert_eq!(entry["seq"], 1);
    let (event, _) = next_event(&mut first, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "delta");
    first.kill();
    drop(first);

    // The loop moves while nobody watches: output grows, no viewer queued.
    append_log_lines(&state, "unwatched line while dead\n");
    let before = fs::read(state.join("dispatch.json")).unwrap();

    // Reconnect: hello, then ONE fresh snapshot of now — the missed
    // commit is never replayed as an entry.
    let mut second = LiveTap::attach(port, "/streams/dispatch").await;
    let (event, _) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "hello", "reconnect opens with hello, not a replay");
    let (event, data) = next_event(&mut second, std::time::Duration::from_secs(10)).await;
    assert_eq!(event, "snapshot", "reconnect lands on a fresh snapshot");
    let snapshot: serde_json::Value = serde_json::from_str(&data[0]).unwrap();
    assert!(
        snapshot["html"]
            .as_str()
            .unwrap()
            .contains("unwatched line while dead"),
        "fresh snapshot shows what happened while dead"
    );

    // Quiet: no replayed commit, delta, or second snapshot follows.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Some(block) = second.next_block(remaining).await {
            let (event, _) = parse_block(&block);
            assert!(
                event.as_deref() != Some("commit")
                    && event.as_deref() != Some("delta")
                    && event.as_deref() != Some("snapshot"),
                "no replayed entries after a fresh snapshot, got: {event:?}"
            );
        }
    }
    // Viewers are observers: the whole attach/kill/reconnect cycle moved
    // no ledger byte.
    assert_eq!(
        fs::read(state.join("dispatch.json")).unwrap(),
        before,
        "streaming changed nothing about the running loop"
    );

    server.abort();
    clear_stream_env();
}
