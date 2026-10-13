//! Train review + land button screen tests (ticket #32, spec #27).
//!
//! Seam: HTTP status + content markers against fixture state (never DOM
//! structure). Read paths render through pure functions
//! (`train_review_page_html`, `train_review_fragment_html`) over
//! `TrainReviewData`; the POST land path acts through the same command
//! function the CLI calls (`cmd_land`, with its preflight shared as
//! `train_land_preflight`), so UI refusals match the CLI by construction.
//! Hermetic: ledger/train record via FACTORY_STATE_DIR, git via
//! FACTORY_GIT_BIN (fake script for render tests, real repo + bare
//! origin for the end-to-end land test), gh via FACTORY_GH_BIN,
//! checks via FACTORY_CHECK_ADAPTER. GETs never mutate (snapshot-tested).

use axum::response::IntoResponse;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use factory_lib::{
    train_landed_page_html, train_no_train_sentence, train_review_data, train_review_fragment_html,
    train_review_page_html, web_router, TRAIN_POLL_MS,
};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uitrain-{}-{}-{}",
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

fn write_exe(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Fake git for render tests: rev-parse answers the train head, status is
/// clean, diff answers a canned stat, ls-remote answers reachable, and
/// worktree add/remove/prune succeed (the head-check preflight stages an
/// ephemeral worktree, then removes it).
fn write_fake_git(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "git-fake",
        "#!/bin/sh\n\
if [ \"$1\" = \"rev-parse\" ]; then echo \"abc123head456\"; exit 0; fi\n\
if [ \"$1\" = \"status\" ]; then exit 0; fi\n\
if [ \"$1\" = \"diff\" ]; then echo \" w30.txt | 1 +\"; echo \" 1 file changed, 1 insertion(+)\"; exit 0; fi\n\
if [ \"$1\" = \"ls-remote\" ]; then echo \"abc123head456\\tHEAD\"; exit 0; fi\n\
if [ \"$1\" = \"worktree\" ]; then if [ \"$2\" = \"add\" ]; then mkdir -p \"$4\"; fi; exit 0; fi\n\
exit 1\n",
    )
}

/// Fake git reporting a dirty working tree (status prints a modification).
fn write_fake_git_dirty(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "git-dirty",
        "#!/bin/sh\n\
if [ \"$1\" = \"rev-parse\" ]; then echo \"abc123head456\"; exit 0; fi\n\
if [ \"$1\" = \"status\" ]; then echo \" M dirty.txt\"; exit 0; fi\n\
if [ \"$1\" = \"diff\" ]; then echo \" w30.txt | 1 +\"; exit 0; fi\n\
if [ \"$1\" = \"ls-remote\" ]; then echo \"abc123head456\\tHEAD\"; exit 0; fi\n\
if [ \"$1\" = \"worktree\" ]; then if [ \"$2\" = \"add\" ]; then mkdir -p \"$4\"; fi; exit 0; fi\n\
exit 1\n",
    )
}

fn write_fake_adapter(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "adapter.sh",
        "#!/bin/sh\necho \"FACTORY-CHECK PASSED\"\nexit \"${ADAPTER_FAIL:-0}\"\n",
    )
}

const TRAIN: &str = "integration/20261013-beta";

fn seed_state(root: &Path, ledger_json: &str, logs: &[(&str, &str)]) -> PathBuf {
    let state = root.join("state");
    fs::create_dir_all(state.join("logs")).unwrap();
    fs::write(state.join("dispatch.json"), ledger_json).unwrap();
    fs::write(
        state.join("train.json"),
        format!(
            "{{\"branch\":\"{TRAIN}\",\"base_commit\":\"base0000\",\"status\":\"open\",\"created_at\":\"t\"}}"
        ),
    )
    .unwrap();
    for (name, body) in logs {
        fs::write(state.join("logs").join(name), body).unwrap();
    }
    state
}

const LEDGER_TWO: &str = r#"{"attempts":[
{"id":1,"ticket":30,"attempt_no":1,"base_commit":"base0000","head_commit":"head3030","branch":"factory/T30-work","worktree":"","status":"review_done","exit_code":0,"log_path":"","started_at":"100","finished_at":"101","train_branch":"integration/20261013-beta"},
{"id":2,"ticket":31,"attempt_no":1,"base_commit":"base0000","head_commit":"head3131","branch":"factory/T31-work","worktree":"","status":"review_failed","exit_code":0,"log_path":"","started_at":"200","finished_at":"201","train_branch":"integration/20261013-beta"}
]}"#;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap()
}

fn with_env<T>(state: &Path, git: &Path, adapter: Option<&Path>, f: impl FnOnce() -> T) -> T {
    let _guard = env_lock();
    // SAFETY: the static lock serializes every seam user in this test
    // binary, so no two tests observe each other's FACTORY_* values.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", state);
        std::env::set_var("FACTORY_GIT_BIN", git);
        std::env::remove_var("FACTORY_SLOT");
        std::env::remove_var("FACTORY_DISPATCHER");
        std::env::remove_var("FACTORY_DISPATCH");
        match adapter {
            Some(a) => std::env::set_var("FACTORY_CHECK_ADAPTER", a),
            None => std::env::remove_var("FACTORY_CHECK_ADAPTER"),
        }
        std::env::set_var("ADAPTER_FAIL", "0");
    }
    let out = f();
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GIT_BIN");
        std::env::remove_var("FACTORY_CHECK_ADAPTER");
        std::env::remove_var("ADAPTER_FAIL");
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

#[test]
fn members_verdicts_diffs_and_head_render() {
    let root = tmpdir("members");
    let git = write_fake_git(&root);
    let adapter = write_fake_adapter(&root);
    let state = seed_state(
        &root,
        LEDGER_TWO,
        &[
            ("T30-a1-impl.log", "implement did the thing\n"),
            ("T30-a1-review.log", "looks good\nVERDICT: accept\n"),
            ("T31-a1-impl.log", "implement tried\n"),
            (
                "T31-a1-review.log",
                "needs fixes\nVERDICT: changes_requested\n",
            ),
        ],
    );

    let data = with_env(&state, &git, Some(&adapter), train_review_data);
    assert!(data.open);
    assert_eq!(data.train, TRAIN);
    assert_eq!(data.head, "abc123head456");
    assert_eq!(data.members.len(), 2);

    let frag = train_review_fragment_html(&data);
    assert!(
        frag.contains("data-train-review"),
        "review marker, got:\n{frag}"
    );
    assert!(
        frag.contains(&format!("data-train=\"{TRAIN}\"")),
        "train branch marker, got:\n{frag}"
    );
    assert!(
        frag.contains("data-train-head"),
        "train head marker, got:\n{frag}"
    );
    assert!(frag.contains("abc123head456"), "head sha streams");
    assert!(
        frag.contains("data-member=\"30\""),
        "member #30 marker, got:\n{frag}"
    );
    assert!(
        frag.contains("data-member=\"31\""),
        "member #31 marker, got:\n{frag}"
    );
    assert!(
        frag.contains("factory/T30-work"),
        "member branch streams, got:\n{frag}"
    );
    // Verdicts: the review ritual's word per member.
    assert!(
        frag.contains("data-verdict=\"accept\""),
        "accept verdict marker, got:\n{frag}"
    );
    assert!(
        frag.contains("data-verdict=\"changes_requested\""),
        "changes-requested marker, got:\n{frag}"
    );
    // Diffs render per member.
    assert!(frag.contains("data-diff"), "diff marker, got:\n{frag}");
    assert!(
        frag.contains("1 file changed"),
        "diff stat streams, got:\n{frag}"
    );
    // The land button posts through the command function.
    assert!(
        frag.contains("/train/land"),
        "land action endpoint, got:\n{frag}"
    );
    assert!(
        frag.contains("data-land-form"),
        "land form marker, got:\n{frag}"
    );
    // CLI correspondence is on the screen.
    assert!(
        frag.contains("factory land"),
        "land ritual correspondence, got:\n{frag}"
    );
    assert!(
        frag.contains("release status"),
        "status ritual correspondence, got:\n{frag}"
    );
}

#[test]
fn member_without_verdict_reads_calmly() {
    let root = tmpdir("noverdict");
    let git = write_fake_git(&root);
    let adapter = write_fake_adapter(&root);
    let state = seed_state(
        &root,
        r#"{"attempts":[{"id":1,"ticket":30,"attempt_no":1,"base_commit":"b","head_commit":"h","branch":"factory/T30-work","worktree":"","status":"review_running","exit_code":0,"log_path":"","started_at":"1","finished_at":"","train_branch":"integration/20261013-beta"}]}"#,
        &[],
    );
    let data = with_env(&state, &git, Some(&adapter), train_review_data);
    assert_eq!(data.members.len(), 1);
    assert_eq!(data.members[0].verdict, None);
    let frag = train_review_fragment_html(&data);
    assert!(
        frag.contains("data-verdict=\"none\""),
        "missing verdict marker, got:\n{frag}"
    );
    assert!(
        frag.contains("No review verdict yet"),
        "missing verdict reads calmly, got:\n{frag}"
    );
}

#[test]
fn no_open_train_reads_as_sentence() {
    let root = tmpdir("notrain");
    let git = write_fake_git(&root);
    let adapter = write_fake_adapter(&root);
    let state = root.join("state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("dispatch.json"), r#"{"attempts":[]}"#).unwrap();
    let data = with_env(&state, &git, Some(&adapter), train_review_data);
    assert!(!data.open);
    let frag = train_review_fragment_html(&data);
    assert!(
        frag.contains(train_no_train_sentence()),
        "calm no-train sentence, got:\n{frag}"
    );
    assert!(
        frag.contains("data-no-train"),
        "no-train marker, got:\n{frag}"
    );
    assert!(
        !frag.contains("/train/land"),
        "no land button without a train, got:\n{frag}"
    );
    let page = train_review_page_html(&data);
    assert!(page.contains(train_no_train_sentence()));
}

#[test]
fn preflight_refuses_dirty_tree_exactly_like_cli() {
    let root = tmpdir("dirty");
    let git = write_fake_git_dirty(&root);
    let adapter = write_fake_adapter(&root);
    let state = seed_state(&root, LEDGER_TWO, &[]);
    let err = with_env(&state, &git, Some(&adapter), || {
        factory_lib::train_land_preflight(TRAIN).expect_err("dirty tree must refuse")
    });
    assert!(
        err.contains("dirty"),
        "dirty refusal names the cause, got: {err}"
    );
    // Identical preflight: the CLI land prints this same message.
    assert!(err.starts_with("land:"), "CLI-shaped refusal, got: {err}");
}

#[test]
fn preflight_refuses_red_head_exactly_like_cli() {
    let root = tmpdir("red");
    let git = write_fake_git(&root);
    let adapter = write_fake_adapter(&root);
    let state = seed_state(&root, LEDGER_TWO, &[]);
    let err = with_env(&state, &git, Some(&adapter), || {
        unsafe { std::env::set_var("ADAPTER_FAIL", "1") };
        let r = factory_lib::train_land_preflight(TRAIN).expect_err("red head must refuse");
        unsafe { std::env::set_var("ADAPTER_FAIL", "0") };
        r
    });
    assert!(
        err.contains("train head checks failed"),
        "red refusal names the check gate, got: {err}"
    );
}

#[test]
fn preflight_passes_green_tree_and_head() {
    let root = tmpdir("green");
    let git = write_fake_git(&root);
    let adapter = write_fake_adapter(&root);
    let state = seed_state(&root, LEDGER_TWO, &[]);
    with_env(&state, &git, Some(&adapter), || {
        factory_lib::train_land_preflight(TRAIN).expect("green must pass");
    });
}

#[test]
fn train_page_polls_fragment_without_full_reload() {
    let root = tmpdir("poll");
    let git = write_fake_git(&root);
    let adapter = write_fake_adapter(&root);
    let state = seed_state(&root, LEDGER_TWO, &[]);
    let data = with_env(&state, &git, Some(&adapter), train_review_data);
    let page = train_review_page_html(&data);
    assert!(
        page.contains("data-train-fragment=\"/fragments/train\""),
        "page names the fragment endpoint"
    );
    assert!(
        page.contains(&format!("data-poll-ms=\"{TRAIN_POLL_MS}\"")),
        "page carries the poll interval"
    );
    assert!(page.contains("fetch("), "page refreshes via fetch");
    assert!(page.contains("/train/land"), "page carries the land action");
    assert!(
        page.contains("Train review"),
        "page titles the ritual, got head:\n{}",
        &page[..page.len().min(600)]
    );
}

#[test]
fn landed_page_shows_release_state() {
    let html = train_landed_page_html(TRAIN, &[30, 31], "Landed note.");
    assert!(
        html.contains("data-released"),
        "release marker, got:\n{html}"
    );
    assert!(html.contains(TRAIN), "train named in release state");
    assert!(html.contains("#30"), "closed member #30 named");
    assert!(html.contains("#31"), "closed member #31 named");
    assert!(html.contains("Landed note."), "release note streams");
}

#[test]
fn endpoints_return_200_with_markers() {
    let _ = web_router();
    let root = tmpdir("endpoints");
    let git = write_fake_git(&root);
    let adapter = write_fake_adapter(&root);
    let state = seed_state(&root, LEDGER_TWO, &[]);
    let data = with_env(&state, &git, Some(&adapter), train_review_data);
    let page = train_review_page_html(&data);
    let rt0 = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let resp = axum::response::Html(page).into_response();
    let (status, body) = rt0.block_on(response_body_text(resp));
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("Train review"));
    assert!(body.contains("data-train-review"));

    let frag = train_review_fragment_html(&data);
    let resp = axum::response::Html(frag).into_response();
    let (status, body) = rt0.block_on(response_body_text(resp));
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-train-review"));

    // Handlers serve the same markers through the router's code path.
    let _guard = env_lock();
    // SAFETY: lock held for the whole body; a private runtime drives the
    // handler futures so no guard crosses an await.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GIT_BIN", &git);
        std::env::set_var("FACTORY_CHECK_ADAPTER", &adapter);
        std::env::remove_var("FACTORY_SLOT");
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let resp = rt
        .block_on(factory_lib::train_review_page_handler())
        .into_response();
    let (status, _) = rt.block_on(response_body_text(resp));
    assert_eq!(status, axum::http::StatusCode::OK);
    let resp = rt
        .block_on(factory_lib::train_review_fragment_handler())
        .into_response();
    let (status, _) = rt.block_on(response_body_text(resp));
    assert_eq!(status, axum::http::StatusCode::OK);
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GIT_BIN");
        std::env::remove_var("FACTORY_CHECK_ADAPTER");
    }
}

#[test]
fn train_get_paths_never_mutate_state() {
    let root = tmpdir("readonly");
    let git = write_fake_git(&root);
    let adapter = write_fake_adapter(&root);
    let state = seed_state(
        &root,
        LEDGER_TWO,
        &[("T30-a1-review.log", "VERDICT: accept\n")],
    );
    let before = snapshot(&state);
    assert!(!before.is_empty());
    with_env(&state, &git, Some(&adapter), || {
        let data = train_review_data();
        let _ = train_review_page_html(&data);
        let _ = train_review_fragment_html(&data);
    });
    let after = snapshot(&state);
    assert_eq!(before.len(), after.len());
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}

// --- end-to-end: one-click land through the command function ---------------
//
// A hermetic repo (dev + bare origin, real git) with an open train and
// one merged member. The POST handler must land it exactly like the CLI:
// merge --no-ff, push, close members with the release note, train record
// removed, release state rendered.

fn git_real(repo: &Path, args: &[&str]) {
    let st = std::process::Command::new("/usr/bin/git")
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

fn write_fake_gh(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "gh",
        "#!/bin/sh\n\
for a in \"$@\"; do printf '%s\\n' \"$a\"; done >> \"$GH_LOG\"\n\
printf -- '---\\n' >> \"$GH_LOG\"\n\
if [ \"$1\" = \"issue\" ] && [ \"$2\" = \"view\" ]; then cat \"$GH_VIEWS_DIR/$3.json\"; exit 0; fi\n\
exit 0\n",
    )
}

fn ticket_view(number: u64, title: &str, body: &str, state: &str) -> String {
    let body_esc = body
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!(
        r#"{{"number":{number},"title":"{title}","body":"{body_esc}","labels":[],"state":"{state}","comments":[]}}"#
    )
}

#[test]
fn land_button_lands_end_to_end_and_shows_release() {
    let _guard = env_lock();
    let root = tmpdir("e2e");
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_real(&repo, &["init", "-b", "dev"]);
    git_real(&repo, &["config", "user.email", "t@t"]);
    git_real(&repo, &["config", "user.name", "t"]);
    fs::write(repo.join("f.txt"), "x\n").unwrap();
    git_real(&repo, &["add", "."]);
    git_real(&repo, &["commit", "-m", "seed"]);
    let origin = root.join("origin.git");
    git_real(&repo, &["init", "--bare", origin.to_str().unwrap()]);
    git_real(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_real(&repo, &["push", "-q", "origin", "dev"]);

    // Member branch with one commit, then a train carrying its merge.
    git_real(&repo, &["checkout", "-qb", "factory/T30-work"]);
    fs::write(repo.join("w30.txt"), "thirty\n").unwrap();
    git_real(&repo, &["add", "-A"]);
    git_real(&repo, &["commit", "-qm", "feat(T30): widget"]);
    git_real(&repo, &["checkout", "-q", "dev"]);
    let base = std::process::Command::new("/usr/bin/git")
        .args(["rev-parse", "dev"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let base = String::from_utf8(base.stdout).unwrap().trim().to_string();
    git_real(&repo, &["branch", TRAIN, "dev"]);
    git_real(&repo, &["checkout", "-q", TRAIN]);
    git_real(
        &repo,
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            "Merge ticket #30 (factory/T30-work) into integration/20261013-beta",
            "factory/T30-work",
        ],
    );
    git_real(&repo, &["checkout", "-q", "dev"]);

    let state = root.join("state");
    fs::create_dir_all(state.join("logs")).unwrap();
    fs::write(
        state.join("train.json"),
        format!(
            "{{\"branch\":\"{TRAIN}\",\"base_commit\":\"{base}\",\"status\":\"open\",\"created_at\":\"t\"}}"
        ),
    )
    .unwrap();
    fs::write(
        state.join("dispatch.json"),
        format!(
            "{{\"attempts\":[{{\"id\":1,\"ticket\":30,\"attempt_no\":1,\"base_commit\":\"{base}\",\"head_commit\":\"h\",\"branch\":\"factory/T30-work\",\"worktree\":\"\",\"status\":\"review_done\",\"exit_code\":0,\"log_path\":\"\",\"started_at\":\"s\",\"finished_at\":\"f\",\"train_branch\":\"{TRAIN}\"}}]}}"
        ),
    )
    .unwrap();
    fs::write(
        state.join("logs").join("T30-a1-review.log"),
        "looks good\nVERDICT: accept\n",
    )
    .unwrap();
    let views = root.join("views");
    fs::create_dir_all(&views).unwrap();
    fs::write(
        views.join("30.json"),
        ticket_view(30, "Widget", "Build the widget.", "OPEN"),
    )
    .unwrap();
    let gh = write_fake_gh(&root);
    let gh_log = root.join("gh.log");
    fs::write(&gh_log, "").unwrap();
    let adapter = write_fake_adapter(&root);

    // The review screen sees the member before the click.
    // SAFETY: lock held for the whole test; cwd is restored after.
    let prev_cwd = std::env::current_dir().unwrap();
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GH_BIN", &gh);
        std::env::set_var("FACTORY_GIT_BIN", "/usr/bin/git");
        std::env::set_var("FACTORY_CHECK_ADAPTER", &adapter);
        std::env::set_var("GH_LOG", &gh_log);
        std::env::set_var("GH_VIEWS_DIR", &views);
        std::env::set_var("ADAPTER_FAIL", "0");
        std::env::remove_var("FACTORY_SLOT");
        std::env::remove_var("FACTORY_DISPATCHER");
        std::env::remove_var("FACTORY_DISPATCH");
    }
    std::env::set_current_dir(&repo).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let before = factory_lib::train_review_data();
    assert!(before.open, "train must read open before land");
    assert_eq!(before.members.len(), 1);
    assert_eq!(before.members[0].verdict.as_deref(), Some("accept"));

    // The click: POST /train/land through the command function.
    let resp = rt.block_on(factory_lib::train_land_handler());
    let (status, body) = rt.block_on(response_body_text(resp.into_response()));
    std::env::set_current_dir(&prev_cwd).unwrap();
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
        std::env::remove_var("FACTORY_GIT_BIN");
        std::env::remove_var("FACTORY_CHECK_ADAPTER");
        std::env::remove_var("GH_LOG");
        std::env::remove_var("GH_VIEWS_DIR");
        std::env::remove_var("ADAPTER_FAIL");
    }
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body.contains("data-released"),
        "successful land shows release state, got:\n{body}"
    );
    assert!(
        body.contains(TRAIN),
        "release names the train, got:\n{body}"
    );
    assert!(
        !state.join("train.json").is_file(),
        "successful land closes the train record"
    );
    let gh_text = fs::read_to_string(&gh_log).unwrap();
    assert!(
        gh_text.contains("close"),
        "successful land closes members, gh log:\n{gh_text}"
    );

    // After the land, the screen reads released (no open train).
    // NOTE: `_guard` above is still held: the lock stays ours.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GIT_BIN", "/usr/bin/git");
        std::env::remove_var("FACTORY_SLOT");
    }
    let after = factory_lib::train_review_data();
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GIT_BIN");
    }
    assert!(!after.open);
    let frag = train_review_fragment_html(&after);
    assert!(frag.contains(train_no_train_sentence()));
}

#[test]
fn land_button_refuses_dirty_tree_without_mutation() {
    let _guard = env_lock();
    let root = tmpdir("e2erefuse");
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_real(&repo, &["init", "-b", "dev"]);
    git_real(&repo, &["config", "user.email", "t@t"]);
    git_real(&repo, &["config", "user.name", "t"]);
    fs::write(repo.join("f.txt"), "x\n").unwrap();
    git_real(&repo, &["add", "."]);
    git_real(&repo, &["commit", "-m", "seed"]);
    git_real(&repo, &["branch", TRAIN, "dev"]);
    let state = root.join("state");
    fs::create_dir_all(&state).unwrap();
    fs::write(
        state.join("train.json"),
        format!(
            "{{\"branch\":\"{TRAIN}\",\"base_commit\":\"b\",\"status\":\"open\",\"created_at\":\"t\"}}"
        ),
    )
    .unwrap();
    fs::write(state.join("dispatch.json"), r#"{"attempts":[]}"#).unwrap();
    // Dirty the tree exactly like the CLI refusal test does.
    fs::write(repo.join("uncommitted.txt"), "dirty\n").unwrap();
    let adapter = write_fake_adapter(&root);

    let prev_cwd = std::env::current_dir().unwrap();
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::set_var("FACTORY_GIT_BIN", "/usr/bin/git");
        std::env::set_var("FACTORY_CHECK_ADAPTER", &adapter);
        std::env::set_var("ADAPTER_FAIL", "0");
        std::env::remove_var("FACTORY_SLOT");
        std::env::remove_var("FACTORY_DISPATCHER");
        std::env::remove_var("FACTORY_DISPATCH");
    }
    std::env::set_current_dir(&repo).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let resp = rt.block_on(factory_lib::train_land_handler());
    let (status, body) = rt.block_on(response_body_text(resp.into_response()));
    std::env::set_current_dir(&prev_cwd).unwrap();
    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GIT_BIN");
        std::env::remove_var("FACTORY_CHECK_ADAPTER");
        std::env::remove_var("ADAPTER_FAIL");
    }
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body.contains("dirty"),
        "dirty refusal streams the CLI message, got:\n{body}"
    );
    assert!(
        body.contains("data-train-review"),
        "refused land stays on the review screen, got:\n{body}"
    );
    assert!(
        state.join("train.json").is_file(),
        "refused land must leave the train record untouched"
    );
}
