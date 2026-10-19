//! Live terminal watch (ticket #91).
//!
//! Seam: argv in -> exit code + stdout/stderr out, plus the pure
//! `factory_lib` seams (`watch_snapshot`, `watch_render_text`,
//! `watch_frame_changed`, `watch_scripted_transition`). State comes from
//! a seeded `FACTORY_STATE_DIR` (ledger + optional train + STOP); the
//! working directory is a scratch git repo standing in for the project.
//! No daemon, no network (a closed loopback port pins the refused
//! fallback), no spend, no prior command run anywhere in this suite.
//!
//! What these tests catch: one pane (lanes + health + transitions) from
//! fixture state, seeded deltas moving the display, clean and interrupt
//! quits restoring the terminal, streams-refused falling back to polling
//! with a calm indicator (never an error), byte+mtime proof that
//! watching writes nothing, calm empty/stopped states, and piped output
//! degrading to a static snapshot with no escape bytes.
//!
//! Style rule for this suite: assert rendered CONTENT (substrings) and
//! terminal STATE (booleans), never exact escape-byte sequences.

use assert_cmd::Command;
use factory_lib::{
    watch_frame_changed, watch_render_text, watch_scripted_transition, watch_snapshot,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// `watch_snapshot` reads `FACTORY_STATE_DIR` from the process env, so the
// in-process tests serialize env access. (Child-process tests set env on
// the child and need no lock.)
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-watch-{}-{}-{}",
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
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_run(&repo, &["init", "-b", "dev"]);
    git_run(&repo, &["config", "user.email", "t@t"]);
    git_run(&repo, &["config", "user.name", "t"]);
    fs::write(repo.join("f.txt"), "x\n").unwrap();
    git_run(&repo, &["add", "."]);
    git_run(&repo, &["commit", "-m", "seed"]);
    Harness {
        state: root.join("state"),
        repo,
    }
}

fn write_ledger(h: &Harness, ledger: &serde_json::Value) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(ledger).unwrap(),
    )
    .unwrap();
}

fn write_train(h: &Harness, branch: &str) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("train.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "branch": branch,
            "base_commit": "abc123def456",
            "status": "open",
            "created_at": "1700000000",
        }))
        .unwrap(),
    )
    .unwrap();
}

/// One live attempt plus one finished attempt with an outcome marker.
fn fixture_ledger() -> serde_json::Value {
    serde_json::json!({
        "attempts": [
            {
                "id": 1, "ticket": 70, "attempt_no": 1,
                "base_commit": "abc123def456", "head_commit": "",
                "branch": "factory/T70-demo", "worktree": "/tmp/wt-T70-a1",
                "status": "impl_running", "exit_code": 0,
                "log_path": "", "started_at": "1700000000", "finished_at": "",
                "train_branch": "", "phase": "implement", "resumed": 0
            },
            {
                "id": 2, "ticket": 69, "attempt_no": 2,
                "base_commit": "abc123def456", "head_commit": "def456abc123",
                "branch": "factory/T69-demo", "worktree": "/tmp/wt-T69-a2",
                "status": "review_done", "exit_code": 0,
                "log_path": "", "started_at": "1699990000", "finished_at": "1699999900",
                "train_branch": "", "phase": "review", "resumed": 0
            }
        ]
    })
}

fn watch_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT")
        .env_remove("FACTORY_WATCH_STREAM_URL");
    cmd
}

/// (relative path, bytes, mtime) for every file under the state dir.
fn snapshot(state: &Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut out = Vec::new();
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
                let bytes = fs::read(&p).unwrap_or_default();
                let mtime = fs::metadata(&p).and_then(|m| m.modified()).unwrap();
                out.push((rel, bytes, mtime));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn with_state_env<T>(h: &Harness, f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("FACTORY_STATE_DIR", &h.state);
    std::env::remove_var("FACTORY_WATCH_STREAM_URL");
    let out = f();
    std::env::remove_var("FACTORY_STATE_DIR");
    out
}

// One pane shows live attempt lanes, health, and recent transitions from
// fixture state without any prior command run (no init, no status first:
// the seeded ledger is the only input).
#[test]
fn watch_pane_reads_fixture_without_prior_command() {
    let h = setup("pane");
    write_ledger(&h, &fixture_ledger());
    write_train(&h, "integration/20261019-train");

    let out = watch_cmd(&h, &["watch"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    // Health: version, train, loop state.
    assert!(stdout.contains("factory "), "version line: {stdout:?}");
    assert!(
        stdout.contains("integration/20261019-train"),
        "train line: {stdout:?}"
    );
    assert!(stdout.contains("running"), "loop state: {stdout:?}");
    // Live attempt lane with its slot.
    assert!(stdout.contains("T70-a1"), "live lane: {stdout:?}");
    assert!(stdout.contains("impl_running"), "live status: {stdout:?}");
    // Recent finished attempt with its outcome marker.
    assert!(stdout.contains("T69-a2"), "recent lane: {stdout:?}");
    assert!(stdout.contains("review_done"), "outcome marker: {stdout:?}");
    // Recent-transitions tail names both attempts.
    assert!(stdout.contains("T70-a1"), "transitions: {stdout:?}");
    // Polling fallback reads calmly when streams are unavailable.
    assert!(stdout.contains("polling"), "source indicator: {stdout:?}");

    // The pure seams agree with the CLI over the same fixture.
    let (snap, text) = with_state_env(&h, || {
        (watch_snapshot(), watch_render_text(&watch_snapshot()))
    });
    assert!(text.contains("T70-a1"));
    assert!(text.contains("review_done"));
    assert!(snap.live_lanes.iter().any(|l| l.contains("T70-a1")));
    assert!(snap.recent_lanes.iter().any(|l| l.contains("T69-a2")));
    assert!(!snap.transitions.is_empty());
}

// Seeded deltas move the display: a new attempt appears, a phase change
// rewrites the lane, and a completion lands in recent — each visible
// without restarting anything (successive reads, no setup rerun).
#[test]
fn watch_seeded_deltas_move_display() {
    let h = setup("deltas");
    write_ledger(&h, &fixture_ledger());

    let before = watch_cmd(&h, &["watch"]).output().unwrap();
    let before_text = String::from_utf8_lossy(&before.stdout).into_owned();
    assert!(!before_text.contains("T71-a1"));

    // Delta 1: a new attempt arrives.
    let mut ledger = fixture_ledger();
    ledger["attempts"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": 3, "ticket": 71, "attempt_no": 1,
            "base_commit": "abc123def456", "head_commit": "",
            "branch": "factory/T71-demo", "worktree": "/tmp/wt-T71-a1",
            "status": "impl_running", "exit_code": 0,
            "log_path": "", "started_at": "1700000100", "finished_at": "",
            "train_branch": "", "phase": "plan", "resumed": 0
        }));
    write_ledger(&h, &ledger);
    let after_new = watch_cmd(&h, &["watch"]).output().unwrap();
    let after_new_text = String::from_utf8_lossy(&after_new.stdout).into_owned();
    assert!(
        after_new_text.contains("T71-a1"),
        "new lane: {after_new_text:?}"
    );

    // Delta 2: the new attempt changes phase; the lane follows.
    ledger["attempts"][2]["phase"] = serde_json::Value::String("verify".to_string());
    write_ledger(&h, &ledger);
    let after_phase = watch_cmd(&h, &["watch"]).output().unwrap();
    let after_phase_text = String::from_utf8_lossy(&after_phase.stdout).into_owned();
    assert!(
        after_phase_text.contains("T71-a1") && after_phase_text.contains("verify"),
        "phase change: {after_phase_text:?}"
    );

    // Delta 3: the attempt completes; its outcome marker lands in recent.
    ledger["attempts"][2]["status"] = serde_json::Value::String("review_done".to_string());
    ledger["attempts"][2]["phase"] = serde_json::Value::String("review".to_string());
    ledger["attempts"][2]["finished_at"] = serde_json::Value::String("1700000200".to_string());
    write_ledger(&h, &ledger);
    let after_done = watch_cmd(&h, &["watch"]).output().unwrap();
    let after_done_text = String::from_utf8_lossy(&after_done.stdout).into_owned();
    assert!(
        after_done_text.contains("T71-a1") && after_done_text.contains("review_done"),
        "completion: {after_done_text:?}"
    );

    // The change detector names only the moved sections, so a delta
    // refreshes in place instead of restarting the pane.
    with_state_env(&h, || {
        let prev = watch_snapshot();
        let changed = watch_frame_changed(&prev, &prev);
        assert!(changed.is_empty(), "identical frames move nothing");
        let mut moved = prev.clone();
        moved
            .live_lanes
            .push("T99-a9 impl_running (implement)".to_string());
        let changed = watch_frame_changed(&prev, &moved);
        assert!(changed.contains(&"lanes".to_string()), "{changed:?}");
        assert!(!changed.contains(&"health".to_string()), "{changed:?}");
        assert!(!changed.contains(&"source".to_string()), "{changed:?}");
    });
}

// Quitting restores the terminal exactly on both the clean path (q) and
// the interrupt path (Ctrl-C byte): scripted inputs over scripted frames,
// no real terminal needed.
#[test]
fn watch_quit_restores_terminal_on_clean_and_interrupt() {
    let h = setup("quit");
    write_ledger(&h, &fixture_ledger());
    let frames = with_state_env(&h, || vec![watch_snapshot(), watch_snapshot()]);

    let clean = watch_scripted_transition(&frames, b"q");
    assert!(clean.quit, "q quits");
    assert!(clean.restored, "q restores the terminal");
    assert!(clean.alternate_screen_left, "q leaves the alternate screen");
    assert!(clean.clean, "q is a clean quit");

    let interrupt = watch_scripted_transition(&frames, &[0x03]);
    assert!(interrupt.quit, "Ctrl-C quits");
    assert!(interrupt.restored, "Ctrl-C restores the terminal");
    assert!(
        interrupt.alternate_screen_left,
        "Ctrl-C leaves the alternate screen"
    );
    assert!(interrupt.clean, "Ctrl-C is a clean quit");

    let idle = watch_scripted_transition(&frames, &[]);
    assert!(!idle.quit, "no input keeps watching");

    // Non-interactive output never enters the alternate screen at all:
    // piped stdout carries zero escape bytes (no residue possible).
    let out = watch_cmd(&h, &["watch"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        !out.stdout.contains(&0x1b),
        "piped output must carry no escape bytes"
    );
}

// Streams unavailable (here: a refused loopback port) falls back to
// polling with a visible-but-calm indicator — exit 0, never an error.
#[test]
fn watch_streams_refused_falls_back_to_polling_calmly() {
    let h = setup("fallback");
    write_ledger(&h, &fixture_ledger());

    let mut cmd = watch_cmd(&h, &["watch"]);
    cmd.env("FACTORY_WATCH_STREAM_URL", "http://127.0.0.1:1/");
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "fallback never errors");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("polling"), "calm indicator: {stdout:?}");
    assert!(stdout.contains("T70-a1"), "pane still renders: {stdout:?}");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !stderr.to_lowercase().contains("error"),
        "no error text: {stderr:?}"
    );
}

// Read-only proof: watching issues no writes — the state dir is
// byte+mtime identical after observation, and no STOP flag appears.
#[test]
fn watch_issues_no_writes() {
    let h = setup("readonly");
    write_ledger(&h, &fixture_ledger());
    write_train(&h, "integration/20261019-train");

    let before = snapshot(&h.state);
    let out = watch_cmd(&h, &["watch"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    with_state_env(&h, || {
        let _ = watch_snapshot();
        let _ = watch_render_text(&watch_snapshot());
    });
    let after = snapshot(&h.state);

    let before_bytes: Vec<_> = before.iter().map(|(p, b, _)| (p, b)).collect();
    let after_bytes: Vec<_> = after.iter().map(|(p, b, _)| (p, b)).collect();
    assert_eq!(before_bytes, after_bytes, "watching changed state bytes");
    assert_eq!(before, after, "watching touched state mtimes");
    assert!(
        !h.state.join("STOP").exists(),
        "watching must not create a STOP file"
    );
}

// Empty and stopped states read calmly, and piped output degrades to a
// static snapshot: exit 0 fast, no screen clearing, no hang.
#[test]
fn watch_empty_stopped_and_piped_degrade_calmly() {
    let h = setup("empty");
    write_ledger(&h, &serde_json::json!({"attempts": []}));

    let out = watch_cmd(&h, &["watch"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("no attempts yet"),
        "calm empty state: {stdout:?}"
    );
    assert!(
        !stdout.contains('\x1b'),
        "piped output never clears the screen"
    );

    fs::write(h.state.join("STOP"), "stopped\n").unwrap();
    let out = watch_cmd(&h, &["watch"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("STOPPED"),
        "stopped reads calmly: {stdout:?}"
    );
    assert!(
        stdout.contains("no attempts yet"),
        "empty + stopped together: {stdout:?}"
    );
    assert!(!stdout.contains('\x1b'), "still no escape bytes");
}

// Ticket #96: one formatting rule for both timestamp eras. Pre-cutover
// rows carry UTC civil strings (the bash dispatcher wrote SQLite
// `datetime('now')`, which is UTC); post-cutover rows carry raw epoch
// seconds (`now_stamp`). Human output renders both identically; every
// machine timestamp field in `--json` keeps its raw ledger string.
#[test]
fn watch_transitions_render_both_timestamp_eras_identically() {
    let h = setup("watch-eras");
    write_ledger(
        &h,
        &serde_json::json!({
            "attempts": [
                {
                    "id": 1, "ticket": 11, "attempt_no": 1,
                    "base_commit": "abc123def456", "head_commit": "def456abc123",
                    "branch": "factory/T11-demo", "worktree": "/tmp/wt-T11-a1",
                    "status": "review_done", "exit_code": 0,
                    "log_path": "",
                    "started_at": "2026-10-08 13:01:30",
                    "finished_at": "2026-10-08 13:16:25",
                    "train_branch": "", "phase": "review", "resumed": 0
                },
                {
                    "id": 2, "ticket": 12, "attempt_no": 1,
                    "base_commit": "abc123def456", "head_commit": "def456abc123",
                    "branch": "factory/T12-demo", "worktree": "/tmp/wt-T12-a1",
                    "status": "review_done", "exit_code": 0,
                    "log_path": "",
                    "started_at": "1792399637",
                    "finished_at": "1792403894",
                    "train_branch": "", "phase": "review", "resumed": 0
                }
            ]
        }),
    );
    let out = watch_cmd(&h, &["watch"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    // 1792399637 = 2026-10-19 08:47:17 UTC; 1792403894 = 2026-10-19
    // 09:58:14 UTC (independent `date -u` truth, not the formatter).
    assert!(
        text.contains("started 2026-10-08 13:01:30, finished 2026-10-08 13:16:25"),
        "pre-cutover row keeps its UTC civil shape:\n{text}"
    );
    assert!(
        text.contains("started 2026-10-19 08:47:17, finished 2026-10-19 09:58:14"),
        "epoch row renders the identical shape:\n{text}"
    );
    assert!(
        !text.contains("1792399637") && !text.contains("1792403894"),
        "no raw epoch leaks into human output:\n{text}"
    );
}
