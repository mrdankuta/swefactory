//! Dispatch-loop CLI boundary tests (ticket #13).
//!
//! Seam: argv in -> exit code + stdout/stderr out + state-dir files and
//! subprocess argv logs out. `gh` is a fake script (no network), `git` is a
//! logging shim over the real binary, and both the implementer (`pi`) and
//! the check adapter are fakes. Mirrors the `cli_issue_ops.rs` patterns.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-dispatch-{}-{}-{}",
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

fn git_repo(dir: &Path) {
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.email", "t@t"]);
    run(&["config", "user.name", "t"]);
    fs::write(dir.join("f.txt"), "x").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "seed"]);
}

/// Fake `gh`: serves `issue list` from $GH_LIST_JSON and `issue view <n>`
/// from $GH_VIEWS_DIR/<n>.json; logs every argv to $GH_LOG. Comment/edit
/// calls succeed without side effects.
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

/// Logging shim over real git: records argv to $GIT_LOG, then delegates.
fn write_git_shim(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "git-shim",
        r#"#!/bin/sh
for a in "$@"; do printf '%s ' "$a"; done >> "$GIT_LOG"
printf '\n' >> "$GIT_LOG"
exec /usr/bin/git "$@"
"#,
    )
}

/// Fake implementer/reviewer: logs cwd + slot + tmpdir, drops a file change
/// so the dispatcher has something to commit, and prints the verdict line
/// (only the review log is parsed for it, so printing it always is safe).
fn write_fake_pi(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi",
        r#"#!/bin/sh
echo "pi cwd=$(pwd) slot:${FACTORY_SLOT:-unset} tmpdir:${TMPDIR:-unset}" >> "$PI_LOG"
echo "change from pi" >> change.txt
echo "notes"
echo "VERDICT: ${PI_VERDICT:-accept}"
"#,
    )
}

/// Fake check adapter: records the observed slot, then passes/fails on cue.
fn write_fake_adapter(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "adapter.sh",
        r#"#!/bin/sh
echo "adapter slot:${FACTORY_SLOT:-unset}" >> "$ADAPTER_LOG"
echo "FACTORY-CHECK PASSED"
exit "${ADAPTER_FAIL:-0}"
"#,
    )
}

fn ticket_view(number: u64, title: &str, body: &str, labels: &[&str], state: &str) -> String {
    let labels_json = labels
        .iter()
        .map(|l| format!(r#"{{"name":"{l}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let body_esc = body
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!(
        r#"{{"number":{number},"title":"{title}","body":"{body_esc}","labels":[{labels_json}],"state":"{state}","comments":[]}}"#
    )
}

struct Harness {
    repo: PathBuf,
    state: PathBuf,
    views: PathBuf,
    gh_log: PathBuf,
    git_log: PathBuf,
    pi_log: PathBuf,
    adapter_log: PathBuf,
    gh: PathBuf,
    git_shim: PathBuf,
    pi: PathBuf,
    adapter: PathBuf,
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_repo(&repo);
    let views = root.join("views");
    fs::create_dir_all(&views).unwrap();
    let h = Harness {
        state: root.join("state"),
        gh_log: root.join("gh.log"),
        git_log: root.join("git.log"),
        pi_log: root.join("pi.log"),
        adapter_log: root.join("adapter.log"),
        gh: write_fake_gh(&root),
        git_shim: write_git_shim(&root),
        pi: write_fake_pi(&root),
        adapter: write_fake_adapter(&root),
        repo,
        views,
    };
    fs::write(&h.gh_log, "").unwrap();
    fs::write(&h.git_log, "").unwrap();
    fs::write(&h.pi_log, "").unwrap();
    fs::write(&h.adapter_log, "").unwrap();
    h
}

fn write_view(h: &Harness, number: u64, title: &str, body: &str, labels: &[&str], state: &str) {
    fs::write(
        h.views.join(format!("{number}.json")),
        ticket_view(number, title, body, labels, state),
    )
    .unwrap();
}

/// Base env for a `dispatch` run: all subprocess seams faked, state isolated.
fn dispatch_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_GH_BIN", &h.gh)
        .env("FACTORY_GIT_BIN", &h.git_shim)
        .env("FACTORY_PI_BIN", &h.pi)
        .env("FACTORY_CHECK_ADAPTER", &h.adapter)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("GH_LOG", &h.gh_log)
        .env("GIT_LOG", &h.git_log)
        .env("PI_LOG", &h.pi_log)
        .env("ADAPTER_LOG", &h.adapter_log)
        .env("GH_VIEWS_DIR", &h.views)
        .env("GH_LIST_JSON", "[]")
        .env_remove("FACTORY_SLOT");
    cmd
}

fn gh_log(h: &Harness) -> String {
    fs::read_to_string(&h.gh_log).unwrap()
}

fn git_log(h: &Harness) -> String {
    fs::read_to_string(&h.git_log).unwrap()
}

/// The safety invariant: no git argv may push, merge into, checkout-and-use,
/// or force-update dev/main; no GitHub write may close an issue.
fn assert_never_touches_dev_or_lands(git: &str, gh: &str) {
    for line in git.lines() {
        let l = line.trim();
        assert!(
            !l.starts_with("push"),
            "dispatcher must never push:\n{line}"
        );
        assert!(
            !(l.starts_with("merge") && (l.contains("dev") || l.contains("main"))),
            "dispatcher must never merge into dev/main:\n{line}"
        );
        assert!(
            !(l.starts_with("checkout") && (l.contains(" dev") || l.contains(" main"))),
            "dispatcher must never checkout dev/main for work:\n{line}"
        );
        assert!(
            !(l.contains("branch")
                && l.contains("-f")
                && (l.contains("dev") || l.contains("main"))),
            "dispatcher must never force-update dev/main:\n{line}"
        );
    }
    assert!(
        !gh.lines().any(|l| l.trim() == "close"),
        "dispatcher must never close issues:\n{gh}"
    );
}

#[test]
fn dispatch_once_moves_ready_ticket_through_review_without_landing() {
    let h = setup("once-happy");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cmd = dispatch_cmd(&h, &["dispatch", "once"]);
    cmd.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cmd.assert().success();

    // Ledger: one attempt, carried to review_done with a head commit.
    let ledger = fs::read_to_string(h.state.join("dispatch.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&ledger).unwrap();
    let attempts = parsed.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1, "ledger:\n{ledger}");
    let a = &attempts[0];
    assert_eq!(a.get("ticket").and_then(|v| v.as_u64()), Some(11));
    assert_eq!(
        a.get("status").and_then(|v| v.as_str()),
        Some("review_done"),
        "ledger:\n{ledger}"
    );
    let head = a.get("head_commit").and_then(|v| v.as_str()).unwrap_or("");
    assert!(!head.is_empty(), "head commit must be recorded");
    assert_ne!(
        head,
        a.get("base_commit").and_then(|v| v.as_str()).unwrap_or(""),
        "head must advance past base"
    );

    // Labels moved ready -> review -> accepted; evidence posted as comments.
    let gh = gh_log(&h);
    assert!(gh.contains("comment"), "must post progress comments:\n{gh}");
    assert!(gh.contains("accepted"), "must apply accepted label:\n{gh}");

    // Worktree cleaned up: dir gone and unlisted.
    let wt_dir = h.state.join("worktrees");
    let leftover: Vec<_> = fs::read_dir(&wt_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).collect())
        .unwrap_or_default();
    assert!(
        leftover.is_empty(),
        "worktrees must be cleaned up: {leftover:?}"
    );

    // Structured log left behind for the attempt.
    let logs: Vec<_> = fs::read_dir(h.state.join("logs"))
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!logs.is_empty(), "per-attempt logs must exist");
    let mut saw_json = false;
    for entry in &logs {
        let text = fs::read_to_string(entry.path()).unwrap();
        assert!(text.contains("T11"), "log must name the attempt:\n{text}");
        for line in text.lines() {
            if line.trim_start().starts_with('{')
                && serde_json::from_str::<serde_json::Value>(line).is_ok()
            {
                saw_json = true;
            }
        }
    }
    assert!(saw_json, "logs must include JSON-lines entries");

    assert_never_touches_dev_or_lands(&git_log(&h), &gh);
}

#[test]
fn dispatch_once_skips_tickets_with_open_dependencies() {
    let h = setup("once-deps");
    write_view(
        &h,
        11,
        "Blocked widget",
        "Depends on: #12\nBuild after the other one.",
        &["ticket", "ready"],
        "OPEN",
    );
    write_view(&h, 12, "Other", "First.", &["ticket", "ready"], "OPEN");
    let mut cmd = dispatch_cmd(&h, &["dispatch", "once"]);
    cmd.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("waiting on open deps"));

    let gh = gh_log(&h);
    assert!(
        !gh.lines().any(|l| l.trim() == "comment"),
        "skipped ticket must get no comments:\n{gh}"
    );
    assert!(
        !h.state.join("dispatch.json").is_file(),
        "skipped ticket must leave no attempt"
    );
}

#[test]
fn dispatch_once_marks_budget_exhausted_ticket_blocked() {
    let h = setup("once-budget");
    write_view(
        &h,
        11,
        "Flaky widget",
        "Keeps failing.",
        &["ticket", "ready"],
        "OPEN",
    );
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        r#"{"attempts":[
{"id":1,"ticket":11,"attempt_no":1,"base_commit":"b","head_commit":"","branch":"factory/T11-x","worktree":"","status":"impl_failed","exit_code":1,"log_path":"","started_at":"s","finished_at":"f"},
{"id":2,"ticket":11,"attempt_no":2,"base_commit":"b","head_commit":"","branch":"factory/T11-x","worktree":"","status":"review_failed","exit_code":1,"log_path":"","started_at":"s","finished_at":"f"}]}"#,
    )
    .unwrap();
    let mut cmd = dispatch_cmd(&h, &["dispatch", "once"]);
    cmd.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("exhausted"));

    let gh = gh_log(&h);
    assert!(gh.contains("comment"), "must explain exhaustion:\n{gh}");
    assert!(gh.contains("blocked"), "must apply blocked label:\n{gh}");
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    assert_eq!(
        ledger
            .get("attempts")
            .and_then(|a| a.as_array())
            .map(|a| a.len()),
        Some(2),
        "exhausted ticket must gain no new attempt"
    );
}

#[test]
fn dispatch_status_shows_attempts_and_worktrees() {
    let h = setup("status");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build it.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut once = dispatch_cmd(&h, &["dispatch", "once"]);
    once.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    once.assert().success();

    dispatch_cmd(&h, &["dispatch", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("11"))
        .stdout(predicates::str::contains("review_done"));

    // Machine-parseable shape carries attempts + worktrees + stop flag.
    let out = dispatch_cmd(&h, &["--json", "dispatch", "status"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    assert!(
        parsed
            .get("attempts")
            .and_then(|a| a.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "status JSON must list attempts: {parsed}"
    );
    assert!(
        parsed.get("worktrees").is_some(),
        "missing worktrees: {parsed}"
    );
    assert_eq!(
        parsed.get("stopped").and_then(|v| v.as_bool()),
        Some(false),
        "fresh loop is running: {parsed}"
    );
}

#[test]
fn dispatch_stop_halts_and_resume_restarts() {
    let h = setup("stop-resume");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build it.",
        &["ticket", "ready"],
        "OPEN",
    );
    dispatch_cmd(&h, &["dispatch", "stop"])
        .assert()
        .success()
        .stdout(predicates::str::contains("stopped"));
    assert!(h.state.join("STOP").is_file(), "stop must create STOP");

    fs::write(&h.gh_log, "").unwrap();
    let mut stalled = dispatch_cmd(&h, &["dispatch", "once"]);
    stalled.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    stalled
        .assert()
        .success()
        .stdout(predicates::str::contains("STOP"));
    assert!(
        !gh_log(&h).contains("view"),
        "halted loop must not even read tickets:\n{}",
        gh_log(&h)
    );
    assert!(
        !h.state.join("dispatch.json").is_file(),
        "halted loop records nothing"
    );

    dispatch_cmd(&h, &["dispatch", "resume"])
        .assert()
        .success()
        .stdout(predicates::str::contains("resumed"));
    assert!(!h.state.join("STOP").is_file(), "resume removes STOP");

    let mut go = dispatch_cmd(&h, &["dispatch", "once"]);
    go.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    go.assert().success();
    assert!(
        h.state.join("dispatch.json").is_file(),
        "resumed loop processes tickets"
    );
}

#[test]
fn concurrent_workers_are_isolated() {
    let h = setup("isolation");
    write_view(
        &h,
        11,
        "First widget",
        "Build one.",
        &["ticket", "ready"],
        "OPEN",
    );
    write_view(
        &h,
        12,
        "Second widget",
        "Build two.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cmd = dispatch_cmd(&h, &["dispatch", "once"]);
    cmd.env("MAX_CONCURRENT", "2")
        .env("GH_LIST_JSON", r#"[{"number":11},{"number":12}]"#);
    cmd.assert().success();

    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 2, "both tickets attempted:\n{ledger}");

    // Distinct slots observed by the implementer and the check adapter.
    let pi = fs::read_to_string(&h.pi_log).unwrap();
    let slots: Vec<String> = pi
        .lines()
        .filter_map(|l| {
            l.split("slot:")
                .nth(1)
                .and_then(|s| s.split_whitespace().next().map(|s| s.to_string()))
        })
        .collect();
    assert_eq!(slots.len(), 4, "implement + review per ticket:\n{pi}");
    let mut uniq = slots.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), 2, "each worker owns its slot:\n{pi}");

    let adapter = fs::read_to_string(&h.adapter_log).unwrap();
    assert!(adapter.contains("T11") && adapter.contains("T12"));
    let tmpdirs: Vec<String> = pi
        .lines()
        .filter_map(|l| {
            l.split("tmpdir:")
                .nth(1)
                .and_then(|s| s.split_whitespace().next().map(|s| s.to_string()))
        })
        .collect();
    assert_eq!(tmpdirs.len(), 4);
    let mut uniq_tmp = tmpdirs.clone();
    uniq_tmp.sort();
    uniq_tmp.dedup();
    assert_eq!(
        uniq_tmp.len(),
        2,
        "no shared temp dirs between workers:\n{pi}"
    );

    // Distinct per-attempt logs; no shared worktrees left behind.
    let logs: Vec<_> = fs::read_dir(h.state.join("logs"))
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(logs.len() >= 4, "per-attempt logs: {logs:?}");
    let leftover: Vec<_> = fs::read_dir(h.state.join("worktrees"))
        .map(|rd| rd.filter_map(|e| e.ok()).collect())
        .unwrap_or_default();
    assert!(leftover.is_empty(), "worktrees cleaned: {leftover:?}");

    assert_never_touches_dev_or_lands(&git_log(&h), &gh_log(&h));
}

#[test]
fn dispatch_once_dry_run_touches_nothing() {
    let h = setup("dry-run");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build it.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cmd = dispatch_cmd(&h, &["dispatch", "once", "--dry-run"]);
    cmd.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("dry-run"));

    let gh = gh_log(&h);
    assert!(gh.contains("list"), "dry-run still surveys tickets");
    for banned in ["comment", "edit"] {
        assert!(
            !gh.lines().any(|l| l.trim() == banned),
            "dry-run must not write to GitHub (gh issue {banned}):\n{gh}"
        );
    }
    assert!(
        git_log(&h).lines().all(|l| !l.contains("worktree add")),
        "dry-run creates no worktrees"
    );
    assert!(
        !h.state.join("dispatch.json").is_file(),
        "dry-run records no attempts"
    );
}

#[test]
fn dispatch_routes_changes_requested_back_to_ready() {
    let h = setup("changes-requested");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build it.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cmd = dispatch_cmd(&h, &["dispatch", "once"]);
    cmd.env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("PI_VERDICT", "changes_requested");
    cmd.assert().success();

    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let a = &ledger.get("attempts").and_then(|v| v.as_array()).unwrap()[0];
    assert_eq!(
        a.get("status").and_then(|v| v.as_str()),
        Some("review_failed"),
        "ledger:\n{ledger}"
    );
    let gh = gh_log(&h);
    assert!(gh.contains("ready"), "ticket routes back to ready:\n{gh}");
    let leftover: Vec<_> = fs::read_dir(h.state.join("worktrees"))
        .map(|rd| rd.filter_map(|e| e.ok()).collect())
        .unwrap_or_default();
    assert!(leftover.is_empty(), "review worktree cleaned: {leftover:?}");
    assert_never_touches_dev_or_lands(&git_log(&h), &gh);
}

#[test]
fn no_api_client_dependency_for_dispatch() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let content = fs::read_to_string(&manifest).unwrap().to_lowercase();
    for banned in ["rusqlite", "reqwest", "octocrab", "ureq", "hyper", "tokio"] {
        assert!(
            !content.contains(banned),
            "Cargo.toml must not gain {banned} for dispatch"
        );
    }
}
