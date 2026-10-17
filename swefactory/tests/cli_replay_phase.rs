//! Replayable attempts, slice 1: phase checkpoints + interrupted (ticket #60,
//! spec #59).
//!
//! Seam: argv in -> exit code + stdout out + state-dir files out, over a
//! fixture ledger plus a scratch repo. `gh` is a fake script (no network),
//! `git` is a logging shim over the real binary, and both the implementer
//! (`pi`) and the check adapter are fakes. Tests assert only
//! invoked-arguments to exit code plus output plus observable state — never
//! store internals, function shapes, or module layout.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-replay-{}-{}-{}",
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

fn write_fake_pi(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi",
        r#"#!/bin/sh
echo "pi cwd=$(pwd) slot:${FACTORY_SLOT:-unset}" >> "$PI_LOG"
echo "change from pi" >> change.txt
echo "notes"
echo "VERDICT: ${PI_VERDICT:-accept}"
"#,
    )
}

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

fn factory_cmd(h: &Harness, args: &[&str]) -> Command {
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

fn ledger_json(h: &Harness) -> serde_json::Value {
    let text =
        fs::read_to_string(h.state.join("dispatch.json")).expect("ledger must exist after a pass");
    serde_json::from_str(&text).expect("ledger must stay valid JSON (atomic checkpoint)")
}

fn run_status_json(h: &Harness, args: &[&str]) -> serde_json::Value {
    let out = factory_cmd(h, args).assert().success().get_output().clone();
    serde_json::from_str(&String::from_utf8(out.stdout).unwrap())
        .expect("machine output must be JSON")
}

/// Simulated kill mid-implement: dispatch status, the ticket log, and every
/// machine-readable status name the phase as interrupted (resumable), with
/// identical facts in human and machine output — and the next pass resumes.
#[test]
fn kill_mid_implement_surfaces_interrupted_naming_phase_everywhere() {
    let h = setup("kill-implement");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");

    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("FACTORY_KILL_PHASE", "implement")
        .assert()
        .success();

    // Observable state: one attempt, interrupted during implement.
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1, "ledger:\n{ledger}");
    assert_eq!(
        attempts[0].get("status").and_then(|v| v.as_str()),
        Some("implement_interrupted"),
        "ledger:\n{ledger}"
    );
    assert_eq!(
        attempts[0].get("phase").and_then(|v| v.as_str()),
        Some("implement"),
        "ledger:\n{ledger}"
    );

    // dispatch status: human names the phase as interrupted, not failed.
    let human = factory_cmd(&h, &["dispatch", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(human).unwrap();
    assert!(
        human.contains("implement_interrupted") || human.contains("interrupted during implement"),
        "dispatch status must name the interrupted phase:\n{human}"
    );
    assert!(
        !human.contains("failed"),
        "interrupted must read distinct from failed:\n{human}"
    );

    // dispatch status --json: identical facts.
    let machine = run_status_json(&h, &["dispatch", "status", "--json"]);
    let m_attempts = machine.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        m_attempts[0].get("status").and_then(|v| v.as_str()),
        Some("implement_interrupted")
    );
    assert_eq!(
        m_attempts[0].get("phase").and_then(|v| v.as_str()),
        Some("implement")
    );

    // ticket log: human + machine carry the same interrupted facts.
    let log_human = factory_cmd(&h, &["log", "11"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let log_human = String::from_utf8(log_human).unwrap();
    assert!(
        log_human.to_lowercase().contains("interrupt") && log_human.contains("implement"),
        "ticket log must name the interrupted phase:\n{log_human}"
    );
    let log_machine = run_status_json(&h, &["log", "11", "--json"]);
    let rows = log_machine
        .get("attempts")
        .and_then(|a| a.as_array())
        .unwrap();
    assert_eq!(
        rows[0].get("status").and_then(|v| v.as_str()),
        Some("implement_interrupted")
    );
    assert_eq!(
        rows[0].get("phase").and_then(|v| v.as_str()),
        Some("implement")
    );
    assert_eq!(
        rows[0].get("summary").and_then(|v| v.as_str()),
        rows[0].get("summary").and_then(|v| v.as_str()),
        "summary must exist in machine output"
    );
    assert!(
        rows[0]
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .contains("interrupt"),
        "machine summary must match the human story:\n{log_machine}"
    );

    // unified status --json: same phase + interrupted facts.
    let unified = run_status_json(&h, &["status", "--json"]);
    let u_attempts = unified.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        u_attempts[0].get("status").and_then(|v| v.as_str()),
        Some("implement_interrupted")
    );
    assert_eq!(
        u_attempts[0].get("phase").and_then(|v| v.as_str()),
        Some("implement")
    );

    // The ticket stays resumable: labels untouched (still ready, never
    // blocked) and the next pass without the kill resumes with a new attempt.
    let gh = fs::read_to_string(&h.gh_log).unwrap();
    assert!(
        gh.contains("interrupted") && gh.contains("implement"),
        "must tell the ticket it was interrupted during implement:\n{gh}"
    );
    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success();
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        attempts.len(),
        2,
        "resume must admit a new attempt:\n{ledger}"
    );
    assert_eq!(
        attempts[1].get("attempt_no").and_then(|v| v.as_u64()),
        Some(2),
        "resumed attempt must not collide on attempt_no:\n{ledger}"
    );
}

/// Negative tool, test, and review verdicts still surface as failed.
#[test]
fn negative_verdicts_still_surface_as_failed() {
    // Failing check adapter -> impl_failed, unchanged wording.
    let h = setup("still-failed-verify");
    write_view(&h, 21, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":21}]"#)
        .env("ADAPTER_FAIL", "1")
        .assert()
        .success();
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        attempts[0].get("status").and_then(|v| v.as_str()),
        Some("impl_failed"),
        "failing checks must stay impl_failed:\n{ledger}"
    );
    let log_human = factory_cmd(&h, &["log", "21"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let log_human = String::from_utf8(log_human).unwrap();
    assert!(
        log_human.contains("failed"),
        "failed story must survive:\n{log_human}"
    );

    // Reviewer asking for changes -> review_failed, unchanged wording.
    let h = setup("still-failed-review");
    write_view(&h, 22, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":22}]"#)
        .env("PI_VERDICT", "changes_requested")
        .assert()
        .success();
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        attempts[0].get("status").and_then(|v| v.as_str()),
        Some("review_failed"),
        "negative review must stay review_failed:\n{ledger}"
    );
}

/// A crash during the checkpoint itself cannot corrupt the attempt record:
// NUL the process between tmp write and rename; the ledger on disk must
/// still parse with the attempt intact.
#[test]
fn crash_during_checkpoint_keeps_valid_ledger() {
    let h = setup("checkpoint-crash");
    write_view(&h, 31, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":31}]"#)
        .env("FACTORY_CHECKPOINT_CRASH", "implement")
        .assert()
        .failure();

    // Observable state: ledger parses (never torn), attempt intact with its
    // last committed checkpoint (plan) still resumable knowledge.
    let raw = fs::read_to_string(h.state.join("dispatch.json"))
        .expect("ledger file must survive a checkpoint crash");
    let ledger: serde_json::Value =
        serde_json::from_str(&raw).expect("ledger must stay valid JSON after a crash");
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1, "attempt record must survive:\n{ledger}");
    assert_eq!(
        attempts[0].get("ticket").and_then(|v| v.as_u64()),
        Some(31),
        "attempt record must be intact:\n{ledger}"
    );
    assert!(
        attempts[0].get("phase").is_some(),
        "surviving record must carry its checkpointed phase:\n{ledger}"
    );
}

/// Ledgers written before phases existed read as phase-unknown and behave
/// under the old rules for one pass, then checkpoint normally (no migration).
#[test]
fn legacy_ledger_without_phase_reads_unknown_and_runs() {
    let h = setup("legacy-unknown");
    write_view(&h, 41, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        r#"{"attempts":[
{"id":1,"ticket":41,"attempt_no":1,"base_commit":"b","head_commit":"","branch":"factory/T41-x","worktree":"","status":"impl_failed","exit_code":1,"log_path":"","started_at":"s","finished_at":"f"}]}"#,
    )
    .unwrap();

    // Old record: unknown phase, old failed story, in every surface.
    let log_machine = run_status_json(&h, &["log", "41", "--json"]);
    let rows = log_machine
        .get("attempts")
        .and_then(|a| a.as_array())
        .unwrap();
    assert_eq!(
        rows[0].get("phase").and_then(|v| v.as_str()),
        Some("unknown"),
        "legacy rows must read phase-unknown:\n{log_machine}"
    );
    assert_eq!(
        rows[0].get("status").and_then(|v| v.as_str()),
        Some("impl_failed")
    );

    // Behaves under the old rules: one more pass runs and checkpoints normally.
    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":41}]"#)
        .assert()
        .success();
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 2, "legacy ticket must run again:\n{ledger}");
    assert!(
        attempts[1]
            .get("phase")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            != "unknown",
        "new attempt must checkpoint a real phase:\n{ledger}"
    );
}
