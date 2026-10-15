//! Reconcile metrics-section CLI boundary tests (ticket #58, spec #56).
//!
//! Seam: argv in -> exit code + stdout out. State comes from a seeded
//! `FACTORY_STATE_DIR` (dispatch ledger + canned transcripts), per-ticket
//! `gh issue view` fixtures via the `FACTORY_GH_BIN` seam, and a scratch
//! git repo standing in for merge history. No network anywhere here.
//!
//! What these tests catch: the metrics section renders exact figures in
//! human mode, unlanded tickets show blanks, missing logs show gaps, cost
//! reads as not-yet-wired (never zero), and `--json` mode carries the
//! identical figure set.
//!
//! What they miss: real `gh` timestamp shapes (fixtures replay the
//! ISO-8601 `createdAt` shape `gh` actually emits).

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Fake-`gh` seam: `issue list` replays $GH_LIST_JSON; `issue view <n>`
/// replays $GH_VIEW_<n>_JSON so each ticket gets its own timeline fixture.
fn write_fake_gh(dir: &Path) -> PathBuf {
    let path = dir.join("gh");
    fs::write(
        &path,
        r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done >> "$GH_LOG"
if [ "$1" = "issue" ] && [ "$2" = "list" ]; then printf '%s' "$GH_LIST_JSON"; exit 0; fi
if [ "$1" = "issue" ] && [ "$2" = "view" ]; then var="GH_VIEW_$3_JSON"; eval "printf '%s' \"\$$var\""; exit 0; fi
exit 0
"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-reconcile-metrics-{}-{}-{}",
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
    git_run_env(repo, args, &[]);
}

fn git_run_env(repo: &Path, args: &[&str], extra_env: &[(&str, &str)]) {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let st = cmd.status().unwrap();
    assert!(st.success(), "git {args:?} failed in {}", repo.display());
}

struct Harness {
    repo: PathBuf,
    state: PathBuf,
    gh: PathBuf,
    log: PathBuf,
}

// Signal: 2026-10-01T00:00:00Z == 1790812800 (verified with date(1)).
const SIGNAL_ISO: &str = "2026-10-01T00:00:00Z";
const SIGNAL_EPOCH: i64 = 1790812800;
// Ticket #7 lands 3 days later; ticket #9 lands 1 day later.
const LANDED_7: i64 = SIGNAL_EPOCH + 3 * 86_400;
const LANDED_9: i64 = SIGNAL_EPOCH + 86_400;

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
    // Merge history: landed tickets get a dated commit mentioning them.
    for (ticket, title, landed) in [(7u64, "widget", LANDED_7), (9u64, "gadget", LANDED_9)] {
        fs::write(repo.join(format!("t{ticket}.txt")), "y\n").unwrap();
        git_run(&repo, &["add", "."]);
        let when = format!("@{landed} +0000");
        git_run_env(
            &repo,
            &["commit", "-m", &format!("land #{ticket} {title}")],
            &[
                ("GIT_AUTHOR_DATE", when.as_str()),
                ("GIT_COMMITTER_DATE", when.as_str()),
            ],
        );
    }
    let state = root.join("state");
    fs::create_dir_all(state.join("logs")).unwrap();

    // Dispatch ledger: #7 has one 45-minute accepted attempt; #9 has one
    // attempt whose log is missing (gap, never a guess); #43 has none.
    let log7 = state.join("logs").join("T7-a1-impl.log");
    fs::write(&log7, "implemented the widget\nVERDICT: accept\n").unwrap();
    let ledger = serde_json::json!({ "attempts": [
        {
            "id": 1, "ticket": 7, "attempt_no": 1,
            "base_commit": "base7", "head_commit": "head7",
            "branch": "factory/T7-widget", "worktree": "/tmp/wt-T7-a1",
            "status": "review_done", "exit_code": 0,
            "log_path": log7.to_string_lossy(),
            "started_at": SIGNAL_EPOCH.to_string(),
            "finished_at": (SIGNAL_EPOCH + 2_700).to_string(),
            "train_branch": ""
        },
        {
            "id": 2, "ticket": 9, "attempt_no": 1,
            "base_commit": "base9", "head_commit": "head9",
            "branch": "factory/T9-gadget", "worktree": "/tmp/wt-T9-a1",
            "status": "impl_failed", "exit_code": 1,
            "log_path": state.join("logs").join("T9-a1-impl.log").to_string_lossy(),
            "started_at": "", "finished_at": "",
            "train_branch": ""
        }
    ] });
    fs::write(
        state.join("dispatch.json"),
        serde_json::to_string_pretty(&ledger).unwrap(),
    )
    .unwrap();

    let gh = write_fake_gh(&root);
    let log = root.join("gh.log");
    Harness {
        repo,
        state,
        gh,
        log,
    }
}

fn reconcile_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_GH_BIN", &h.gh)
        .env("GH_LOG", &h.log)
        .env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT")
        .env(
            "GH_LIST_JSON",
            r#"[{"number":7,"title":"widget"},{"number":43,"title":"unlanded"},{"number":9,"title":"gadget"}]"#,
        )
        .env("GH_VIEW_7_JSON", serde_json::json!({
            "number": 7, "createdAt": SIGNAL_ISO, "labels": [], "comments": []
        }).to_string())
        .env("GH_VIEW_43_JSON", serde_json::json!({
            "number": 43, "createdAt": SIGNAL_ISO, "labels": [], "comments": []
        }).to_string())
        .env("GH_VIEW_9_JSON", serde_json::json!({
            "number": 9, "createdAt": SIGNAL_ISO, "labels": [],
            "comments": [{"body": "HANDOFF `FACTORY:NEEDS_CLARIFICATION`"}]
        }).to_string());
    cmd
}

#[test]
fn reconcile_human_mode_pins_exact_figures_per_ticket() {
    let h = setup("human");
    let out = reconcile_cmd(&h, &["reconcile"]).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("== outcome metrics =="),
        "missing metrics section:\n{stdout}"
    );
    assert!(
        stdout.contains("Ticket #7: signal-to-production cycle 3.0 days, attempt machine minutes 45.0 minutes. Autonomy: autonomous"),
        "wrong #7 figures:\n{stdout}"
    );
    assert!(
        stdout.contains("spend tracking not yet wired"),
        "cost must read as not-yet-wired, never zero:\n{stdout}"
    );
    assert!(
        stdout.contains("manual work outside the loop is invisible to the rule"),
        "autonomy caveat must ship in the output copy:\n{stdout}"
    );
}

#[test]
fn reconcile_human_mode_blanks_unlanded_and_gaps_missing_logs() {
    let h = setup("gaps");
    let out = reconcile_cmd(&h, &["reconcile"]).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    // #43 never landed and has no attempts: blanks for landed figures.
    assert!(
        stdout.contains("Ticket #43: signal-to-production cycle not yet landed, attempt machine minutes logs missing. Autonomy: not autonomous"),
        "unlanded #43 must show blanks, never guesses:\n{stdout}"
    );
    // #9 landed but its log is missing, and its timeline carries a
    // clarification signal: gap for minutes, not autonomous.
    assert!(
        stdout.contains("Ticket #9: signal-to-production cycle 1.0 days, attempt machine minutes logs missing. Autonomy: not autonomous"),
        "missing logs must read as gaps:\n{stdout}"
    );
}

#[test]
fn reconcile_json_carries_the_identical_figure_set() {
    let h = setup("json");
    let out = reconcile_cmd(&h, &["--json", "reconcile"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    let metrics = parsed.get("metrics").expect("JSON must carry metrics");
    let by_ticket = |n: u64| {
        metrics
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m.get("ticket").and_then(|t| t.as_u64()) == Some(n))
            .unwrap()
            .clone()
    };
    let m7 = by_ticket(7);
    assert_eq!(
        m7.get("cycle_days"),
        Some(&serde_json::json!(3.0)),
        "m7: {m7}"
    );
    assert_eq!(
        m7.get("total_attempt_minutes"),
        Some(&serde_json::json!(45.0)),
        "m7: {m7}"
    );
    assert_eq!(
        m7.get("autonomous"),
        Some(&serde_json::json!(true)),
        "m7: {m7}"
    );
    assert_eq!(
        m7.get("autonomy_verdict"),
        Some(&serde_json::json!("autonomous")),
        "m7: {m7}"
    );
    assert_eq!(
        m7.get("cost_text"),
        Some(&serde_json::json!("spend tracking not yet wired")),
        "m7: {m7}"
    );
    assert_eq!(
        m7.get("autonomy_caveat"),
        Some(&serde_json::json!(
            "manual work outside the loop is invisible to the rule"
        )),
        "m7: {m7}"
    );
    assert_eq!(
        m7.get("attempt_minutes"),
        Some(&serde_json::json!([{"attempt_no": 1, "minutes": 45.0}])),
        "m7: {m7}"
    );
    // Unlanded #43: blanks (null), never guesses.
    let m43 = by_ticket(43);
    assert_eq!(
        m43.get("cycle_days"),
        Some(&serde_json::Value::Null),
        "m43: {m43}"
    );
    assert_eq!(
        m43.get("total_attempt_minutes"),
        Some(&serde_json::Value::Null),
        "m43: {m43}"
    );
    assert_eq!(
        m43.get("autonomous"),
        Some(&serde_json::json!(false)),
        "m43: {m43}"
    );
    assert_eq!(
        m43.get("attempt_minutes"),
        Some(&serde_json::json!([])),
        "m43: {m43}"
    );
    // #9: landed cycle pinned, missing logs blank the minutes.
    let m9 = by_ticket(9);
    assert_eq!(
        m9.get("cycle_days"),
        Some(&serde_json::json!(1.0)),
        "m9: {m9}"
    );
    assert_eq!(
        m9.get("total_attempt_minutes"),
        Some(&serde_json::Value::Null),
        "m9: {m9}"
    );
    assert_eq!(
        m9.get("autonomous"),
        Some(&serde_json::json!(false)),
        "m9: {m9}"
    );
    assert_eq!(
        m9.get("attempt_minutes"),
        Some(&serde_json::json!([{"attempt_no": 1, "minutes": serde_json::Value::Null}])),
        "m9: {m9}"
    );
}

#[test]
fn reconcile_stays_read_only_with_metrics_enabled() {
    let h = setup("readonly");
    let before = fs::read_to_string(h.state.join("dispatch.json")).unwrap();
    let out = reconcile_cmd(&h, &["reconcile"]).output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        fs::read_to_string(h.state.join("dispatch.json")).unwrap(),
        before,
        "ledger mutated"
    );
    let logged = fs::read_to_string(&h.log).unwrap();
    assert!(
        logged.contains("list"),
        "reconcile must list issues:\n{logged}"
    );
    for banned in ["comment", "edit", "create", "close"] {
        assert!(
            !logged.lines().any(|l| l.trim() == banned),
            "reconcile is read-only, must not call gh issue {banned}:\n{logged}"
        );
    }
    // Read-only proof extends to metrics assembly: per-ticket views only.
    assert!(
        logged.contains("view"),
        "metrics must read issue timelines:\n{logged}"
    );
}
