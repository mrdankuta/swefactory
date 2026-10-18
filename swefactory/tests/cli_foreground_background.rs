//! Follow-up (#59, ticket #86 rework of #69): foreground/background tasks + bottom-up abort.
//!
//! Seam: argv in -> exit code + stdout out + state-dir files out, over a
//! fixture ledger (owned_tasks seeded as JSON, no migration: old ledgers without
//! `owned_tasks` still parse). `gh` is a fake script (no network). Tests assert
//! only invoked-arguments to exit code plus output plus observable state —
//! never store internals, function shapes, or module layout.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-fgbg-{}-{}-{}",
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

fn write_fake_gh(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "gh",
        r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done >> "$GH_LOG"
printf -- '---\n' >> "$GH_LOG"
if [ "$1" = "issue" ] && [ "$2" = "list" ]; then printf '%s' "$GH_LIST_JSON"; exit 0; fi
if [ "$1" = "issue" ] && [ "$2" = "view" ]; then cat "$GH_VIEWS_DIR/$3.json"; exit 0; fi
exit 0
"#,
    )
}

struct Harness {
    repo: PathBuf,
    state: PathBuf,
    views: PathBuf,
    gh_log: PathBuf,
    gh: PathBuf,
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    let views = root.join("views");
    fs::create_dir_all(&views).unwrap();
    let h = Harness {
        state: root.join("state"),
        gh_log: root.join("gh.log"),
        gh: write_fake_gh(&root),
        repo,
        views,
    };
    fs::write(&h.gh_log, "").unwrap();
    fs::create_dir_all(&h.state).unwrap();
    h
}

fn factory_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_GH_BIN", &h.gh)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("GH_LOG", &h.gh_log)
        .env("GH_VIEWS_DIR", &h.views)
        .env("GH_LIST_JSON", "[]")
        .env_remove("FACTORY_SLOT");
    cmd
}

fn ledger_json(h: &Harness) -> serde_json::Value {
    let text =
        fs::read_to_string(h.state.join("dispatch.json")).expect("ledger must exist as fixture");
    serde_json::from_str(&text).expect("ledger must stay valid JSON")
}

fn seed_ledger(h: &Harness, json: &str) {
    fs::write(h.state.join("dispatch.json"), json).unwrap();
}

fn run_json(h: &Harness, args: &[&str]) -> serde_json::Value {
    let out = factory_cmd(h, args).assert().success().get_output().clone();
    serde_json::from_str(&String::from_utf8(out.stdout).unwrap())
        .expect("machine output must be JSON")
}

fn attempt_row(ledger: &serde_json::Value, ticket: u64) -> &serde_json::Value {
    ledger
        .get("attempts")
        .and_then(|a| a.as_array())
        .unwrap()
        .iter()
        .find(|a| a.get("ticket").and_then(|t| t.as_u64()) == Some(ticket))
        .expect("attempt row must exist")
}

#[allow(clippy::needless_lifetimes)]
fn task_row<'a>(ledger: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    ledger
        .get("owned_tasks")
        .and_then(|t| t.as_array())
        .unwrap()
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
        .unwrap_or_else(|| panic!("task {name} must exist:\n{ledger}"))
}

/// A background task keeps running while its attempt reads idle, and an
/// ordinary abort of the attempt's current work leaves it alone.
#[test]
fn background_task_runs_while_attempt_idle_and_survives_ordinary_abort() {
    let h = setup("bg-survives");
    seed_ledger(
        &h,
        r#"{"attempts":[
{"id":1,"ticket":11,"attempt_no":1,"base_commit":"b","head_commit":"","branch":"factory/T11-x","worktree":"","status":"impl_running","exit_code":0,"log_path":"","started_at":"s","finished_at":"","phase":"implement","resumed":0}],
"owned_tasks":[
{"id":1,"ticket":11,"attempt":1,"name":"foreground-work","scope":"foreground","status":"running","parent":null,"effects":["fg-effect"],"undone":[]},
{"id":2,"ticket":11,"attempt":1,"name":"background-watch","scope":"background","status":"running","parent":null,"effects":["bg-effect"],"undone":[]}],
"abort_log":[]}"#,
    );

    // Ordinary abort stops the foreground work only.
    let out = factory_cmd(&h, &["dispatch", "abort", "11"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(out).unwrap();
    assert!(
        human.contains("foreground-work"),
        "ordinary abort must name the foreground work it stops:\n{human}"
    );
    assert!(
        !human.contains("background-watch"),
        "ordinary abort must leave background work alone:\n{human}"
    );

    // Observable state: foreground aborted, background still running.
    let ledger = ledger_json(&h);
    assert_eq!(
        task_row(&ledger, "foreground-work")
            .get("status")
            .and_then(|v| v.as_str()),
        Some("aborted"),
        "foreground work must be aborted:\n{ledger}"
    );
    assert_eq!(
        task_row(&ledger, "background-watch")
            .get("status")
            .and_then(|v| v.as_str()),
        Some("running"),
        "background work must survive an ordinary abort:\n{ledger}"
    );

    // The attempt reads idle while only background work runs — human and
    // machine carry the same idle fact.
    let status_human = factory_cmd(&h, &["dispatch", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_human = String::from_utf8(status_human).unwrap();
    assert!(
        status_human.to_lowercase().contains("idle") && status_human.contains("background"),
        "attempt must read idle while background tasks run:\n{status_human}"
    );
    let machine = run_json(&h, &["dispatch", "status", "--json"]);
    let idle = machine
        .get("attempts")
        .and_then(|a| a.as_array())
        .unwrap()
        .iter()
        .find(|a| a.get("ticket").and_then(|t| t.as_u64()) == Some(11))
        .and_then(|a| a.get("idle_background").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    assert!(
        idle,
        "machine status must carry the same idle fact:\n{machine}"
    );
    let _ = attempt_row(&ledger, 11);
}

/// Aborting with background scope stops background tasks and everything
/// they own, bottom-up (owned work first).
#[test]
fn abort_with_background_scope_stops_background_and_owned_work_bottom_up() {
    let h = setup("bg-scope");
    seed_ledger(
        &h,
        r#"{"attempts":[
{"id":1,"ticket":12,"attempt_no":1,"base_commit":"b","head_commit":"","branch":"factory/T12-x","worktree":"","status":"impl_running","exit_code":0,"log_path":"","started_at":"s","finished_at":"","phase":"implement","resumed":0}],
"owned_tasks":[
{"id":1,"ticket":12,"attempt":1,"name":"foreground-work","scope":"foreground","status":"running","parent":null,"effects":["fg-effect"],"undone":[]},
{"id":2,"ticket":12,"attempt":1,"name":"background-parent","scope":"background","status":"running","parent":null,"effects":["parent-effect"],"undone":[]},
{"id":3,"ticket":12,"attempt":1,"name":"background-child","scope":"background","status":"running","parent":2,"effects":["child-effect"],"undone":[]}],
"abort_log":[]}"#,
    );

    let out = factory_cmd(&h, &["dispatch", "abort", "12", "--background"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(out).unwrap();
    assert!(
        human.contains("background-parent") && human.contains("background-child"),
        "background-scope abort must name the background work it stops:\n{human}"
    );
    let child_pos = human.find("background-child").unwrap();
    let parent_pos = human.find("background-parent").unwrap();
    assert!(
        child_pos < parent_pos,
        "owned work aborts before its owner (bottom-up):\n{human}"
    );

    // Observable state: the background tree is fully stopped.
    let ledger = ledger_json(&h);
    for name in ["background-parent", "background-child", "foreground-work"] {
        assert_eq!(
            task_row(&ledger, name)
                .get("status")
                .and_then(|v| v.as_str()),
            Some("aborted"),
            "{name} must be aborted:\n{ledger}"
        );
    }
    let log = ledger
        .get("abort_log")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect::<Vec<_>>();
    let child_idx = log
        .iter()
        .position(|e| e == "child-effect")
        .expect("child undone");
    let parent_idx = log
        .iter()
        .position(|e| e == "parent-effect")
        .expect("parent undone");
    assert!(
        child_idx < parent_idx,
        "persisted unwind order is bottom-up:\n{ledger}"
    );
}

/// A simulated kill mid-abort resumes the unwind in the same bottom-up
/// order: no effect undone twice, none skipped.
#[test]
fn kill_mid_abort_resumes_same_bottom_up_order_without_double_undo() {
    let h = setup("kill-abort");
    seed_ledger(
        &h,
        r#"{"attempts":[
{"id":1,"ticket":13,"attempt_no":1,"base_commit":"b","head_commit":"","branch":"factory/T13-x","worktree":"","status":"impl_running","exit_code":0,"log_path":"","started_at":"s","finished_at":"","phase":"implement","resumed":0}],
"owned_tasks":[
{"id":1,"ticket":13,"attempt":1,"name":"top","scope":"background","status":"running","parent":null,"effects":["top-effect"],"undone":[]},
{"id":2,"ticket":13,"attempt":1,"name":"mid","scope":"background","status":"running","parent":1,"effects":["mid-effect"],"undone":[]},
{"id":3,"ticket":13,"attempt":1,"name":"leaf","scope":"background","status":"running","parent":2,"effects":["leaf-effect"],"undone":[]}],
"abort_log":[]}"#,
    );

    // Simulated kill after the first unwind step (leaf-effect).
    factory_cmd(&h, &["dispatch", "abort", "13", "--background"])
        .env("FACTORY_KILL_ABORT_AT", "leaf-effect")
        .assert()
        .failure();
    let partial = ledger_json(&h);
    let partial_log = partial
        .get("abort_log")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect::<Vec<_>>();
    assert_eq!(
        partial_log,
        vec!["leaf-effect"],
        "kill must leave the persisted prefix intact:\n{partial}"
    );

    // Resume finishes the same bottom-up unwind.
    let out = factory_cmd(&h, &["dispatch", "abort", "13", "--background"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(out).unwrap();
    assert!(
        human.contains("mid") && human.contains("top"),
        "resume must finish the remaining unwind:\n{human}"
    );
    let ledger = ledger_json(&h);
    let full_log = ledger
        .get("abort_log")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect::<Vec<_>>();
    assert_eq!(
        full_log,
        vec!["leaf-effect", "mid-effect", "top-effect"],
        "resume continues the same bottom-up order with nothing repeated or skipped:\n{ledger}"
    );
    for name in ["top", "mid", "leaf"] {
        let undone = task_row(&ledger, name)
            .get("undone")
            .and_then(|v| v.as_array())
            .unwrap()
            .len();
        assert_eq!(
            undone, 1,
            "{name} undoes its own effect exactly once:\n{ledger}"
        );
    }
}
