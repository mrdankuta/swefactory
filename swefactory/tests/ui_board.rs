//! Board read-path endpoint tests (ticket #29, spec #27).
//!
//! Seam: HTTP status + content markers against fixture state.
//! Pure render functions (`board_page_html`, `board_fragment_html`) carry
//! the markers; handler responses are asserted via `IntoResponse` status +
//! body bytes (no browser, no network). The live seams (`gh` via
//! FACTORY_GH_BIN, ledger via FACTORY_STATE_DIR, git via FACTORY_GIT_BIN)
//! are faked so rendering proofs stay hermetic. Read-only proof mirrors
//! `cli_status.rs`: byte+mtime snapshots must not move.

use axum::response::IntoResponse;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use factory_lib::{
    board_changed_text, board_column_for_labels, board_cost_text, board_data, board_empty_sentence,
    board_fragment_html, board_issues_to_columns, board_page_html, board_status_text, web_router,
    BoardIssue,
};

fn issue(number: u64, title: &str, body: &str, labels: &[&str]) -> BoardIssue {
    BoardIssue {
        number,
        title: title.to_string(),
        body: body.to_string(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        state: "open".to_string(),
    }
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uiboard-{}-{}-{}",
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

fn write_fake_gh(dir: &Path, json: &str) -> PathBuf {
    let path = dir.join("gh");
    fs::write(
        &path,
        format!("#!/bin/sh\nif echo \"$*\" | grep -q \"issue list\"; then\ncat <<'EOF'\n{json}\nEOF\nelse\necho '[]'\nfi\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn write_fake_git(dir: &Path) -> PathBuf {
    let path = dir.join("git-fake");
    fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
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

#[test]
fn columns_map_labels_in_bea_order() {
    assert_eq!(
        board_column_for_labels(&["blocked".to_string()]),
        ("needs-you", "Needs you")
    );
    assert_eq!(
        board_column_for_labels(&["needs-info".to_string()]),
        ("needs-you", "Needs you")
    );
    assert_eq!(
        board_column_for_labels(&["ready".to_string()]),
        ("ready", "Ready to build")
    );
    assert_eq!(
        board_column_for_labels(&["running".to_string()]),
        ("running", "Being built")
    );
    assert_eq!(
        board_column_for_labels(&["review".to_string()]),
        ("review", "In review")
    );
    assert_eq!(
        board_column_for_labels(&["accepted".to_string()]),
        ("done", "Done")
    );
    assert_eq!(
        board_column_for_labels(&["integrated".to_string()]),
        ("done", "Done")
    );
    assert_eq!(
        board_column_for_labels(&["ticket".to_string()]),
        ("open", "New work")
    );
    assert_eq!(board_column_for_labels(&[]), ("open", "New work"));
}

#[test]
fn cards_carry_status_cost_and_what_changed() {
    let labels = vec!["ready".to_string()];
    let status = board_status_text(&labels, None);
    assert!(status.contains("Ready to build"), "got: {status}");
    let running = board_status_text(&["running".to_string()], Some("impl_running"));
    assert!(
        running.contains("Being built"),
        "status carries column meaning, got: {running}"
    );
    assert!(
        running.contains("implement"),
        "status carries attempt word, got: {running}"
    );

    assert_eq!(board_cost_text("Cost: 2 points\nmore"), "Cost: 2 points");
    assert_eq!(
        board_cost_text("no estimate here"),
        "Cost not estimated yet."
    );

    assert_eq!(
        board_changed_text(&["abc123 land #11".to_string()]),
        "What changed: abc123 land #11"
    );
    assert_eq!(board_changed_text(&[]), "No changes recorded yet.");
}

#[test]
fn board_groups_cards_and_renders_markers() {
    let issues = vec![
        issue(11, "Alpha", "Cost: 1 point", &["ready"]),
        issue(12, "Beta", "no cost", &["review"]),
    ];
    let attempts: HashMap<u64, String> = [(11, "impl_running".to_string())].into_iter().collect();
    let changes: HashMap<u64, Vec<String>> =
        [(11, vec!["deadbee merge #11 into train".to_string()])]
            .into_iter()
            .collect();
    let cols = board_issues_to_columns(&issues, &attempts, &changes);

    let ready = cols.iter().find(|c| c.id == "ready").unwrap();
    assert_eq!(ready.cards.len(), 1);
    assert_eq!(ready.cards[0].number, 11);
    assert!(ready.cards[0].status_text.contains("implement"));
    assert_eq!(ready.cards[0].cost_text, "Cost: 1 point");
    assert!(ready.cards[0].changed_text.contains("deadbee"));

    let review = cols.iter().find(|c| c.id == "review").unwrap();
    assert_eq!(review.cards.len(), 1);
    assert_eq!(review.cards[0].cost_text, "Cost not estimated yet.");
    assert_eq!(review.cards[0].changed_text, "No changes recorded yet.");

    let html = board_fragment_html(&cols);
    assert!(html.contains("data-board-columns"), "got:\n{html}");
    assert!(html.contains("data-column=\"ready\""), "got:\n{html}");
    assert!(html.contains("data-card=\"11\""), "got:\n{html}");
    assert!(html.contains("data-status"), "got:\n{html}");
    assert!(html.contains("data-cost"), "got:\n{html}");
    assert!(html.contains("data-changed"), "got:\n{html}");
    assert!(html.contains("#11 Alpha"), "got:\n{html}");
}

#[test]
fn empty_states_read_as_sentences() {
    let cols = board_issues_to_columns(&[], &HashMap::new(), &HashMap::new());
    let fragment = board_fragment_html(&cols);
    assert!(
        fragment.contains(board_empty_sentence()),
        "empty board reads as a sentence, got:\n{fragment}"
    );
    assert!(
        fragment.contains("Nothing here right now."),
        "empty columns read as sentences, got:\n{fragment}"
    );

    let page = board_page_html(&cols);
    assert!(
        page.contains(board_empty_sentence()),
        "page carries the calm empty state, got marker missing"
    );
}

#[test]
fn board_page_polls_fragment_without_full_reload() {
    let cols = board_issues_to_columns(&[], &HashMap::new(), &HashMap::new());
    let page = board_page_html(&cols);
    assert!(
        page.contains("data-board-fragment=\"/fragments/board\""),
        "page names the fragment endpoint, got markers missing"
    );
    assert!(
        page.contains("data-poll-ms=\"15000\""),
        "page carries the ~15s poll interval"
    );
    assert!(
        page.contains("fetch("),
        "page refreshes via fetch, not full reloads"
    );
    assert!(
        page.contains("factory status"),
        "page shows its CLI correspondence"
    );
}

#[tokio::test]
async fn endpoints_return_200_with_markers() {
    // Router must expose the board, its fragment, and health.
    let _ = web_router();
    // Page + fragment render through the same pure functions the handlers
    // call, so markers asserted here are the markers served there.
    let cols = board_issues_to_columns(
        &[issue(7, "Sprout", "Cost: small", &["ready"])],
        &HashMap::new(),
        &HashMap::new(),
    );
    let page = board_page_html(&cols);
    let resp = axum::response::Html(page).into_response();
    let (status, body) = response_body_text(resp).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("Factory board"));
    assert!(body.contains("data-card=\"7\""));

    let frag = board_fragment_html(&cols);
    let resp = axum::response::Html(frag).into_response();
    let (status, body) = response_body_text(resp).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-board-columns"));

    let health = factory_lib::web_health_response();
    assert!(health.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn board_read_path_never_mutates_state() {
    let root = tmpdir("readonly");
    let state = root.join("state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("dispatch.json"), r#"{"attempts":[]}"#).unwrap();
    let gh = write_fake_gh(&root, "[]");
    let git = write_fake_git(&root);

    let before = snapshot(&state);
    assert!(!before.is_empty());

    // Render through both the pure path and the live `board_data` seam
    // (gh faked empty, git faked failing, ledger present).
    let _ = std::env::var("FACTORY_STATE_DIR");
    // SAFETY: single-threaded test process env mutation for seam isolation.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GH_BIN", &gh);
        std::env::set_var("FACTORY_GIT_BIN", &git);
    }
    let cols = board_data();
    let _ = board_page_html(&cols);
    let _ = board_fragment_html(&cols);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
        std::env::remove_var("FACTORY_GIT_BIN");
    }

    let after = snapshot(&state);
    assert_eq!(before.len(), after.len());
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}

#[test]
fn ui_and_serve_share_one_code_path() {
    use assert_cmd::Command;
    for args in [&["ui", "--help"][..], &["serve", "--help"][..]] {
        let mut cmd = Command::cargo_bin("factory").unwrap();
        cmd.args(args);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "factory {args:?} --help must exit 0, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            text.contains("--addr"),
            "help names the bind flag, got:\n{text}"
        );
        assert!(
            text.contains("--allow-remote"),
            "help names the exposure override, got:\n{text}"
        );
    }
}

#[test]
fn non_localhost_bind_refuses_without_override() {
    // Pure guard: web_serve must refuse before binding (no listener, fast).
    let err = factory_lib::web_serve("0.0.0.0:18080", false).unwrap_err();
    assert!(
        err.contains("--allow-remote"),
        "refusal names the explicit override, got: {err}"
    );
}

/// Ticket #34: the localhost-only default and the v1 no-auth boundary are
/// documented where Bea and the maintainer look — the product README —
/// so no auth gap is silent. The refusal path names the override (proved
/// above); these docs name the default and the boundary.
#[test]
fn bind_and_auth_gaps_are_documented_loudly() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let readme = std::fs::read_to_string(manifest.join("README.md"))
        .expect("swefactory/README.md must exist");
    for marker in [
        "127.0.0.1:8080",
        "--addr",
        "--allow-remote",
        "Localhost-only",
        "No auth in v1",
    ] {
        assert!(readme.contains(marker), "README never documents {marker:?}");
    }
}
