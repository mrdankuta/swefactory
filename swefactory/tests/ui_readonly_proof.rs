//! Hardening read-only proof (ticket #34, spec #27).
//!
//! Every GET endpoint in `web_router()` is exercised over real loopback
//! HTTP against fixture state (seeded ledger, canned logs, fake `gh` +
//! `git`), with a byte+mtime snapshot of the state dir taken before and
//! after. Rendering must never mutate: the snapshots must be identical
//! and no STOP flag may appear. Per-view snapshot tests live next to
//! their views (`ui_board.rs`, `ui_dispatch.rs`, `ui_git_files.rs`,
//! `ui_ticket_detail.rs`, `ui_train_review.rs`); this file is the single
//! checklist that no GET route is missing from the proof.

use std::fs;
use std::path::{Path, PathBuf};

use factory_lib::web_router;

/// All 17 GET routes in `web_router()` (ticket #34: the proof must cover
/// every one; POSTs — land, STOP/Resume — are the mutation path and are
/// tested with the CLI-land harness instead).
const ALL_GETS: &[&str] = &[
    "/",
    "/fragments/board",
    "/tickets/9",
    "/fragments/ticket/9",
    "/api/tickets/9",
    "/dispatch",
    "/fragments/dispatch",
    "/git",
    "/fragments/git",
    "/api/git",
    "/files?ref=HEAD",
    "/fragments/files/tree?ref=HEAD",
    "/fragments/files/view?ref=HEAD&path=swefactory/src/lib.rs",
    "/fragments/files/search?ref=HEAD&q=factory",
    "/train",
    "/fragments/train",
    "/health",
];

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uireadonly-{}-{}-{}",
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

/// Fake `gh`: one open issue for the board list and the ticket detail.
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
*"issue list"*)
cat <<'EOF'
[{"number":9,"title":"Sprout detail","body":"Grow the sprout.","labels":[{"name":"ready"}],"state":"open"}]
EOF
;;
*) exit 1;;
esac
"#,
    )
}

/// Fake `git` answering every read the views issue (mirrors the
/// `ui_git_files.rs` fake so files search/view resolve).
fn write_fake_git(dir: &Path) -> PathBuf {
    write_script(
        dir,
        "git-fake",
        r#"#!/bin/sh
case "$*" in
*"branch -vv"*)
cat <<'EOF'
* factory/T34-ui-hardening-release abc1234 [origin/factory/T34-ui-hardening-release: ahead 1] hardening slice
  dev def5678 [origin/dev] train land
EOF
;;
*"worktree list"*)
cat <<'EOF'
worktree /tmp/main-wt
HEAD abc1234
branch refs/heads/dev

worktree /tmp/t34-wt
HEAD abc1234
branch refs/heads/factory/T34-ui-hardening-release
EOF
;;
*"--pretty=format:"*)
cat <<'EOF'
abc1234def|abc1234|Bea|2026-10-13|feat(T34): hardening slice (#34)
def5678abc|def5678|Eli|2026-10-12|land train integration (#30)
EOF
;;
*"show --stat"*)
cat <<'EOF'
 swefactory/src/lib.rs | 10 ++++++++--
 1 file changed, 8 insertions(+), 2 deletions(-)
EOF
;;
*"ls-tree"*)
cat <<'EOF'
AGENTS.md
swefactory/src/lib.rs
swefactory/src/main.rs
EOF
;;
*"grep -n"*)
cat <<'EOF'
HEAD:swefactory/src/lib.rs:10:factory ui serving
HEAD:swefactory/src/lib.rs:42:Bea-first board
EOF
;;
*"show HEAD:swefactory/src/lib.rs"*)
printf 'pub fn web_router() {}\n// Bea-first\n'
;;
*"show "*) exit 1;;
*) exit 1;;
esac
"#,
    )
}

/// Ledger with one running attempt plus its log on disk. The log path is
/// built from the real state dir so no rewrite pass is needed.
fn seed_state(root: &Path) -> PathBuf {
    let state = root.join("state");
    let logs = state.join("logs");
    fs::create_dir_all(&logs).unwrap();
    let log = logs.join("T9-a1.log");
    fs::write(&log, "a1 implement line 1\na1 implement line 2\n").unwrap();
    let ledger = format!(
        "{{\"attempts\":[{{\"id\":1,\"ticket\":9,\"attempt_no\":1,\"base_commit\":\"b\",\"head_commit\":\"\",\"branch\":\"factory/T9-x\",\"worktree\":\"\",\"status\":\"impl_running\",\"exit_code\":0,\"log_path\":\"{}\",\"started_at\":\"1\",\"finished_at\":\"\"}}]}}",
        log.to_string_lossy()
    );
    fs::write(state.join("dispatch.json"), ledger).unwrap();
    state
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

/// Blocking std HTTP/1.1 GET over loopback (same pattern as
/// `ui_ticket_detail.rs`: std on a blocking thread so the single-threaded
/// test runtime keeps serving while we read).
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

/// Hitting every GET must leave every byte and mtime under the state dir
/// untouched, and every route must answer 200 (the smoke path doubles as
/// the read-only proof transport).
#[tokio::test]
async fn all_get_endpoints_never_mutate_state() {
    let root = tmpdir("all-gets");
    let state = seed_state(&root);
    let gh = write_fake_gh(&root);
    let git = write_fake_git(&root);

    let before = snapshot(&state);
    assert!(
        !before.is_empty(),
        "seeded state must exist to prove against"
    );

    // SAFETY: this test binary holds no other env-mutating test while it
    // runs (single proof test in this file).
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GH_BIN", &gh);
        std::env::set_var("FACTORY_GIT_BIN", &git);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, web_router()).await;
    });

    for path in ALL_GETS {
        let raw = get(port, path).await;
        assert!(
            raw.starts_with("HTTP/1.1 200"),
            "GET {path} must answer 200, got status line: {}",
            raw.lines().next().unwrap_or("<empty response>")
        );
        let body = raw.split("\r\n\r\n").nth(1).unwrap_or("");
        assert!(
            !body.is_empty(),
            "GET {path} must render a body, not an empty 200"
        );
    }
    server.abort();
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
        std::env::remove_var("FACTORY_GIT_BIN");
    }

    // STOP-toggle writes are the POST path's job; GETs leave STOP alone.
    assert!(
        !state.join("STOP").exists(),
        "no GET may create the STOP flag"
    );
    let after = snapshot(&state);
    assert_eq!(
        before.len(),
        after.len(),
        "no GET may add or drop state files"
    );
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}
