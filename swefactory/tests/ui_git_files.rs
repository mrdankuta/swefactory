//! Git view + files view endpoint tests (ticket #33, spec #27).
//!
//! Seam: HTTP status + content markers against fixture state.
//! Pure render functions (`git_page_html`, `git_fragment_html`,
//! `files_*_fragment_html`) carry the markers; handlers are exercised
//! directly (they are `pub async`) with a faked `git` seam
//! (FACTORY_GIT_BIN) so proofs stay hermetic. Read-only proof mirrors
//! `ui_board.rs`: byte+mtime snapshots of the state dir must not move on
//! any new GET path. The file viewer offers copy and never edit: tests
//! assert the copy button marker and the absence of edit affordances.

use axum::extract::Query;
use axum::response::IntoResponse;
use std::fs;
use std::path::{Path, PathBuf};

use factory_lib::{
    extract_ticket_refs, file_language, file_view_fragment_html, file_view_missing_fragment_html,
    files_page_html, files_search_fragment_html, files_tree_fragment_html, git_branch_data,
    git_data, git_empty_sentence, git_fragment_html, git_page_html, parse_git_branch_vv_line,
    parse_git_worktree_porcelain, sanitize_repo_path, web_router, FilesSearchQuery, FilesTreeQuery,
    FilesViewQuery, GitBranch, GitCommit, GitData, GitWorktree, GIT_POLL_MS,
};

/// Serialize every test that mutates process env (threads share env
/// within one test binary). Sync tests hold the guard across the seam.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uigitfiles-{}-{}-{}",
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

/// Fake `git` answering every read the git/files views issue: log graph,
/// per-commit stats, branches with tracking state, worktrees in porcelain,
/// ls-tree, show `<ref>:<path>`, and grep. Anything else fails.
fn write_fake_git(dir: &Path) -> PathBuf {
    write_script(
        dir,
        "git-fake",
        r#"#!/bin/sh
case "$*" in
*"branch -vv"*)
cat <<'EOF'
* factory/T33-ui-git-view-files-view abc1234 [origin/factory/T33-ui-git-view-files-view: ahead 1] git view slice
  dev def5678 [origin/dev] train land
  factory/T30-ui-ticket-detail-log-view 9012345 [origin/factory/T30-ui-ticket-detail-log-view: behind 2] ticket detail
  stale gone1111 [origin/stale: gone] abandoned branch
  local-only aaa1111 no upstream here
EOF
;;
*"worktree list"*)
cat <<'EOF'
worktree /tmp/main-wt
HEAD abc1234
branch refs/heads/dev

worktree /tmp/t33-wt
HEAD abc1234
branch refs/heads/factory/T33-ui-git-view-files-view

worktree /tmp/detached-wt
HEAD def5678
detached
EOF
;;
*"--pretty=format:"*)
cat <<'EOF'
abc1234def|abc1234|Bea|2026-10-13|feat(T33): git view slice (#33)
def5678abc|def5678|Eli|2026-10-12|land train integration (#30)
EOF
;;
*"show --stat"*)
cat <<'EOF'
 swefactory/src/lib.rs | 120 ++++++++++++++++++++++++++++++-
 1 file changed, 110 insertions(+), 10 deletions(-)
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
*"show HEAD:AGENTS.md"*)
printf '# Factory\n'
;;
*"show "*) exit 1;;
*) exit 1;;
esac
"#,
    )
}

fn seed_state(root: &Path) -> PathBuf {
    let state = root.join("state");
    fs::create_dir_all(state.join("logs")).unwrap();
    fs::write(state.join("dispatch.json"), r#"{"attempts":[]}"#).unwrap();
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

async fn response_body_text(resp: axum::response::Response) -> (axum::http::StatusCode, String) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn fixture_git_data() -> GitData {
    GitData {
        commits: vec![GitCommit {
            hash: "abc1234def".to_string(),
            short: "abc1234".to_string(),
            author: "Bea".to_string(),
            date: "2026-10-13".to_string(),
            subject: "feat(T33): git view slice (#33)".to_string(),
            ticket_refs: vec![33],
            stat: "1 file changed, 110 insertions(+)".to_string(),
        }],
        branches: vec![
            GitBranch {
                name: "factory/T33-ui-git-view-files-view".to_string(),
                upstream: "origin/factory/T33-ui-git-view-files-view".to_string(),
                tracking: "ahead 1".to_string(),
                is_current: true,
                ticket_ref: Some(33),
            },
            GitBranch {
                name: "dev".to_string(),
                upstream: "origin/dev".to_string(),
                tracking: "in sync".to_string(),
                is_current: false,
                ticket_ref: None,
            },
        ],
        worktrees: vec![GitWorktree {
            path: "/tmp/t33-wt".to_string(),
            branch: "factory/T33-ui-git-view-files-view".to_string(),
            head: "abc1234".to_string(),
            ticket_ref: Some(33),
        }],
    }
}

#[test]
fn ticket_refs_parse_hash_and_branch_forms() {
    assert_eq!(extract_ticket_refs("land #11 into train"), vec![11]);
    assert_eq!(
        extract_ticket_refs("factory/T33-ui-git-view-files-view"),
        vec![33]
    );
    assert_eq!(
        extract_ticket_refs("feat(T33): slice (#33) fixes #30"),
        vec![30, 33]
    );
    assert_eq!(extract_ticket_refs("no tickets here"), Vec::<u64>::new());
    // Duplicates collapse, output sorted.
    assert_eq!(extract_ticket_refs("#9 and #9 plus #7"), vec![7, 9]);
}

#[test]
fn branch_vv_parser_reads_tracking_state() {
    let cur = parse_git_branch_vv_line(
        "* factory/T33-ui-git-view-files-view abc1234 [origin/factory/T33-ui-git-view-files-view: ahead 1] slice",
    )
    .unwrap();
    assert!(cur.is_current);
    assert_eq!(cur.name, "factory/T33-ui-git-view-files-view");
    assert_eq!(cur.tracking, "ahead 1");
    assert_eq!(cur.ticket_ref, Some(33));

    let sync = parse_git_branch_vv_line("  dev def5678 [origin/dev] train land").unwrap();
    assert!(!sync.is_current);
    assert_eq!(sync.tracking, "in sync");
    assert_eq!(sync.ticket_ref, None);

    let behind =
        parse_git_branch_vv_line("  factory/T30-x 9012345 [origin/factory/T30-x: behind 2] detail")
            .unwrap();
    assert_eq!(behind.tracking, "behind 2");
    assert_eq!(behind.ticket_ref, Some(30));

    let gone = parse_git_branch_vv_line("  stale gone1111 [origin/stale: gone] abandoned").unwrap();
    assert_eq!(gone.tracking, "upstream gone");

    let local = parse_git_branch_vv_line("  local-only aaa1111 no upstream here").unwrap();
    assert_eq!(local.upstream, "");
    assert_eq!(local.tracking, "no upstream");
}

#[test]
fn worktree_porcelain_parser_links_tickets() {
    let text = "worktree /tmp/main-wt\nHEAD abc1234\nbranch refs/heads/dev\n\nworktree /tmp/t33-wt\nHEAD abc1234\nbranch refs/heads/factory/T33-ui-git-view-files-view\n";
    let wts = parse_git_worktree_porcelain(text);
    assert_eq!(wts.len(), 2);
    assert_eq!(wts[0].branch, "dev");
    assert_eq!(wts[0].ticket_ref, None);
    assert_eq!(wts[1].path, "/tmp/t33-wt");
    assert_eq!(wts[1].branch, "factory/T33-ui-git-view-files-view");
    assert_eq!(wts[1].ticket_ref, Some(33));
}

#[test]
fn git_fragment_renders_log_branches_worktrees_with_tickets_and_stats() {
    let html = git_fragment_html(&fixture_git_data());
    assert!(html.contains("data-git-log"), "got:\n{html}");
    assert!(html.contains("data-commit=\"abc1234\""), "got:\n{html}");
    assert!(
        html.contains("data-stat"),
        "diff stats present, got:\n{html}"
    );
    assert!(
        html.contains("110 insertions"),
        "stat body present, got:\n{html}"
    );
    assert!(html.contains("data-branches"), "got:\n{html}");
    assert!(
        html.contains("data-branch=\"factory/T33-ui-git-view-files-view\""),
        "got:\n{html}"
    );
    assert!(html.contains("data-tracking"), "got:\n{html}");
    assert!(html.contains("ahead 1"), "got:\n{html}");
    assert!(html.contains("data-worktrees"), "got:\n{html}");
    assert!(
        html.contains("data-worktree=\"/tmp/t33-wt\""),
        "got:\n{html}"
    );
    // Ticket associations render as links to the ticket detail.
    assert!(html.contains("data-ticket=\"33\""), "got:\n{html}");
    assert!(html.contains("/tickets/33"), "got:\n{html}");
    assert!(
        html.contains("git log"),
        "CLI correspondence shown, got:\n{html}"
    );
}

#[test]
fn git_empty_state_reads_as_sentence() {
    let empty = GitData {
        commits: Vec::new(),
        branches: Vec::new(),
        worktrees: Vec::new(),
    };
    let html = git_fragment_html(&empty);
    assert!(
        html.contains(git_empty_sentence()),
        "empty git view reads as a sentence, got:\n{html}"
    );
}

#[test]
fn git_page_polls_fragment_without_full_reload() {
    let page = git_page_html(&fixture_git_data());
    assert!(
        page.contains("data-git-fragment=\"/fragments/git\""),
        "page names the fragment endpoint"
    );
    assert!(
        page.contains(&format!("data-poll-ms=\"{GIT_POLL_MS}\"")),
        "page carries the poll interval"
    );
    assert!(page.contains("fetch("), "page refreshes via fetch");
    assert!(page.contains("Factory git"), "Bea-readable title");
}

#[test]
fn sanitize_repo_path_rejects_traversal() {
    assert_eq!(
        sanitize_repo_path("swefactory/src/lib.rs").unwrap(),
        "swefactory/src/lib.rs"
    );
    assert!(sanitize_repo_path("/etc/passwd").is_err());
    assert!(sanitize_repo_path("../outside").is_err());
    assert!(sanitize_repo_path("a/../../b").is_err());
    assert!(sanitize_repo_path("").is_err());
    assert!(sanitize_repo_path("a\\b").is_err());
}

#[test]
fn file_language_maps_extensions() {
    assert_eq!(file_language("src/lib.rs"), "rust");
    assert_eq!(file_language("README.md"), "markdown");
    assert_eq!(file_language("Cargo.toml"), "toml");
    assert_eq!(file_language("Makefile"), "text");
}

#[test]
fn files_tree_fragment_lists_files_with_ref_echo() {
    let tree = vec!["AGENTS.md".to_string(), "swefactory/src/lib.rs".to_string()];
    let html = files_tree_fragment_html(&tree, "dev");
    assert!(html.contains("data-files-tree"), "got:\n{html}");
    assert!(
        html.contains("data-file=\"swefactory/src/lib.rs\""),
        "got:\n{html}"
    );
    assert!(html.contains("data-ref=\"dev\""), "ref context echoed");
    let empty = files_tree_fragment_html(&[], "dev");
    assert!(
        empty.contains(factory_lib::files_empty_sentence()),
        "empty tree reads as a sentence, got:\n{empty}"
    );
}

#[test]
fn file_view_fragment_offers_copy_never_edit() {
    let html = file_view_fragment_html("HEAD", "swefactory/src/lib.rs", "pub fn x() {}", false);
    assert!(html.contains("data-file-view"), "got:\n{html}");
    assert!(
        html.contains("data-copy-button"),
        "copy offered, got:\n{html}"
    );
    assert!(html.contains("data-file-body"), "got:\n{html}");
    assert!(html.contains("pub fn x()"), "body shown, got:\n{html}");
    assert!(!html.contains("<form"), "viewer never edits: no forms");
    assert!(
        !html.contains("<textarea"),
        "viewer never edits: no textarea"
    );
    assert!(
        !html.contains("contenteditable"),
        "viewer never edits: no contenteditable"
    );
    let truncated = file_view_fragment_html("HEAD", "big.log", "x", true);
    assert!(
        truncated.contains("data-truncated"),
        "truncation is named, got:\n{truncated}"
    );
}

#[test]
fn file_view_missing_and_traversal_are_calm() {
    let missing = file_view_missing_fragment_html("HEAD", "nope.txt", "not found in HEAD");
    assert!(missing.contains("data-file-missing"), "got:\n{missing}");
    assert!(missing.contains("nope.txt"), "got:\n{missing}");
    let bad = file_view_missing_fragment_html("HEAD", "../evil", "invalid path");
    assert!(!bad.contains("../evil") || bad.contains("invalid path"));
}

#[test]
fn search_fragment_renders_hits_with_ref_context() {
    let hits = vec!["HEAD:swefactory/src/lib.rs:10:factory ui serving".to_string()];
    let html = files_search_fragment_html("HEAD", "serving", &hits);
    assert!(html.contains("data-files-search"), "got:\n{html}");
    assert!(html.contains("data-search-hit"), "got:\n{html}");
    assert!(html.contains("factory ui serving"), "got:\n{html}");
    assert!(html.contains("data-ref=\"HEAD\""), "got:\n{html}");
    let empty = files_search_fragment_html("HEAD", "zzz-no-match", &[]);
    assert!(
        empty.contains("No matches"),
        "empty search reads as a sentence, got:\n{empty}"
    );
}

#[test]
fn files_page_carries_ref_tree_search_and_viewer_hooks() {
    let tree = vec!["AGENTS.md".to_string()];
    let page = files_page_html(&tree, "factory/T33-x");
    assert!(page.contains("data-ref=\"factory/T33-x\""), "ref echoed");
    assert!(page.contains("data-files-tree"), "tree present");
    assert!(page.contains("data-files-search"), "search present");
    assert!(
        page.contains("/fragments/files/view"),
        "viewer fragment endpoint named"
    );
    assert!(page.contains("git show"), "CLI correspondence shown");
}

#[tokio::test]
async fn git_and_files_handlers_return_200_with_markers() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("handlers");
    let state = seed_state(&root);
    let git = write_fake_git(&root);
    // SAFETY: ENV_LOCK serializes every seam user in this test binary.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GIT_BIN", &git);
    }

    let data = git_data();
    assert_eq!(data.commits.len(), 2, "log seam parsed");
    assert_eq!(data.branches.len(), 5, "branch seam parsed");
    assert_eq!(data.worktrees.len(), 3, "worktree seam parsed");
    assert!(
        data.commits[0].ticket_refs.contains(&33),
        "log tickets associated"
    );
    assert!(
        !data.commits[0].stat.is_empty(),
        "diff stats attached to commits"
    );

    let (status, body) =
        response_body_text(factory_lib::git_page_handler().await.into_response()).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("Factory git"), "got markers missing");

    let (status, body) =
        response_body_text(factory_lib::git_fragment_handler().await.into_response()).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-git-log"));

    let (status, json) = factory_lib::git_api_handler().await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(json.get("commits").is_some(), "JSON carries commits");
    assert!(json.get("branches").is_some(), "JSON carries branches");
    assert!(json.get("worktrees").is_some(), "JSON carries worktrees");

    let _ = web_router();

    let (status, body) = response_body_text(
        factory_lib::files_page_handler(Query(FilesTreeQuery {
            gitref: Some("HEAD".to_string()),
        }))
        .await
        .into_response(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-files-tree"));
    assert!(body.contains("AGENTS.md"));

    let (status, body) = response_body_text(
        factory_lib::files_tree_fragment_handler(Query(FilesTreeQuery {
            gitref: Some("HEAD".to_string()),
        }))
        .await
        .into_response(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-file="));

    let (status, body) = response_body_text(
        factory_lib::file_view_fragment_handler(Query(FilesViewQuery {
            gitref: Some("HEAD".to_string()),
            path: "swefactory/src/lib.rs".to_string(),
        }))
        .await
        .into_response(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-copy-button"));
    assert!(body.contains("web_router"));

    // Any branch context works: an unknown ref reads calm, never 500.
    let (status, _) = response_body_text(
        factory_lib::files_tree_fragment_handler(Query(FilesTreeQuery {
            gitref: Some("no-such-branch".to_string()),
        }))
        .await
        .into_response(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let (status, body) = response_body_text(
        factory_lib::files_search_fragment_handler(Query(FilesSearchQuery {
            gitref: Some("HEAD".to_string()),
            q: "serving".to_string(),
        }))
        .await
        .into_response(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-search-hit"));

    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GIT_BIN");
    }
}

#[test]
fn git_branches_parse_through_live_seam() {
    let _guard = ENV_LOCK.blocking_lock();
    let root = tmpdir("branches");
    let state = seed_state(&root);
    let git = write_fake_git(&root);
    // SAFETY: ENV_LOCK serializes every seam user in this test binary.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GIT_BIN", &git);
    }
    let branches = git_branch_data();
    assert!(branches.iter().any(|b| b.is_current));
    assert!(branches.iter().any(|b| b.tracking == "ahead 1"));
    assert!(branches.iter().any(|b| b.tracking == "upstream gone"));
    assert!(branches.iter().any(|b| b.ticket_ref == Some(33)));
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GIT_BIN");
    }
}

#[tokio::test]
async fn git_and_files_reads_never_mutate_state() {
    let _guard = ENV_LOCK.lock().await;
    let root = tmpdir("readonly");
    let state = seed_state(&root);
    let git = write_fake_git(&root);
    let before = snapshot(&state);
    assert!(!before.is_empty());
    // SAFETY: ENV_LOCK serializes every seam user in this test binary.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GIT_BIN", &git);
    }
    let data = git_data();
    let _ = git_page_html(&data);
    let _ = git_fragment_html(&data);
    let _ = factory_lib::git_json(&data);
    let _ = files_page_html(&factory_lib::files_tree("HEAD"), "HEAD");
    let _ = files_tree_fragment_html(&factory_lib::files_tree("HEAD"), "HEAD");
    let _ = factory_lib::files_search("HEAD", "serving");
    let _ = factory_lib::git_page_handler().await;
    let _ = factory_lib::git_fragment_handler().await;
    let _ = factory_lib::git_api_handler().await;
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
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

/// Regression for review finding [spec-drift] (ticket #33): `?ref=` on the
/// wire must bind to the files handlers. Every files URL the UI generates
/// uses `ref=` (tree poll, viewer links, search form), so decoding must go
/// through Axum's real query codec (`Query::try_from_uri`), not through
/// directly-constructed structs which bypass deserialization.
#[tokio::test]
async fn files_ref_query_binds_on_the_wire() {
    // Tree: `?ref=dev` binds, and the fragment echoes it.
    let uri: axum::http::Uri = "/fragments/files/tree?ref=dev".parse().unwrap();
    let Query(tree_q) = Query::<FilesTreeQuery>::try_from_uri(&uri).unwrap();
    assert_eq!(tree_q.gitref.as_deref(), Some("dev"));
    let html = files_tree_fragment_html(&[], tree_q.gitref.as_deref().unwrap_or("HEAD"));
    assert!(html.contains("data-ref=\"dev\""), "got:\n{html}");

    // Viewer: `?ref=` + `path=` bind together.
    let uri: axum::http::Uri = "/fragments/files/view?ref=dev&path=AGENTS.md"
        .parse()
        .unwrap();
    let Query(view_q) = Query::<FilesViewQuery>::try_from_uri(&uri).unwrap();
    assert_eq!(view_q.gitref.as_deref(), Some("dev"));
    assert_eq!(view_q.path, "AGENTS.md");

    // Search: `?ref=` + `q=` bind together.
    let uri: axum::http::Uri = "/fragments/files/search?ref=dev&q=serving".parse().unwrap();
    let Query(search_q) = Query::<FilesSearchQuery>::try_from_uri(&uri).unwrap();
    assert_eq!(search_q.gitref.as_deref(), Some("dev"));
    assert_eq!(search_q.q, "serving");

    // Traversal through the real wire stays calm: 200 + missing marker.
    let uri: axum::http::Uri = "/fragments/files/view?ref=HEAD&path=../evil"
        .parse()
        .unwrap();
    let query = Query::<FilesViewQuery>::try_from_uri(&uri).unwrap();
    let (status, body) = response_body_text(
        factory_lib::file_view_fragment_handler(query)
            .await
            .into_response(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body.contains("data-file-missing"),
        "traversal reads calm, got:\n{body}"
    );
}
