//! Replayable attempts, slices 1-2: phase checkpoints + interrupted (ticket
//! #60, spec #59) and resume-from-checkpoint (ticket #61, spec #59).
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
use std::process::Command as ProcCommand;

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
    // blocked) and the next pass without the kill resumes the SAME attempt
    // (ticket #61, spec #59) instead of opening a fresh one.
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
        1,
        "resume must continue the same attempt, never fork a new one:\n{ledger}"
    );
    assert_eq!(
        attempts[0].get("attempt_no").and_then(|v| v.as_u64()),
        Some(1),
        "resumed attempt keeps its number:\n{ledger}"
    );
    assert_eq!(
        attempts[0].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "resume must be counted on the record:\n{ledger}"
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

// --- replayable attempts, slice 2: resume-from-checkpoint (ticket #61,
// --- spec #59). Same seam: argv in -> exit code + stdout out + state-dir
// --- files out. A chatty prompt-logging `pi` stands in for the implementer
// --- so resumed briefs and tail bounds are observable without spending.

fn write_fake_pi_capture(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi",
        r#"#!/bin/sh
printf '=== PROMPT ===\n%s\n' "$2" >> "$PROMPTS_LOG"
echo "change from pi" >> change.txt
i=1; while [ $i -le 100 ]; do echo "implement line $i"; i=$((i+1)); done
echo "notes"
echo "VERDICT: ${PI_VERDICT:-accept}"
"#,
    )
}

fn write_view_with_comments(
    h: &Harness,
    number: u64,
    title: &str,
    body: &str,
    labels: &[&str],
    state: &str,
    comments: &[&str],
) {
    let labels_json = labels
        .iter()
        .map(|l| format!(r#"{{"name":"{l}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let comments_json = comments
        .iter()
        .map(|c| format!(r#"{{"body":{}}}"#, serde_json::to_string(c).unwrap()))
        .collect::<Vec<_>>()
        .join(",");
    let body_esc = body
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    fs::write(
        h.views.join(format!("{number}.json")),
        format!(
            r#"{{"number":{number},"title":"{title}","body":"{body_esc}","labels":[{labels_json}],"state":"{state}","comments":[{comments_json}]}}"#
        ),
    )
    .unwrap();
}

fn repo_head(h: &Harness) -> String {
    let out = ProcCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&h.repo)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn repo_branch(h: &Harness, name: &str) {
    let st = ProcCommand::new("git")
        .args(["branch", name])
        .current_dir(&h.repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap();
    assert!(st.success(), "git branch {name} failed");
}

fn write_ledger(h: &Harness, text: &str) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(h.state.join("dispatch.json"), text).unwrap();
}

fn ledger_raw(h: &Harness) -> String {
    fs::read_to_string(h.state.join("dispatch.json")).expect("ledger must exist")
}

fn crafted_attempt(
    h: &Harness,
    ticket: u64,
    attempt_no: u64,
    status: &str,
    phase: &str,
    log_marker: Option<&str>,
) -> String {
    let log_path = h
        .state
        .join("logs")
        .join(format!("T{ticket}-a{attempt_no}-impl.log"));
    fs::create_dir_all(log_path.parent().unwrap()).unwrap();
    if let Some(marker) = log_marker {
        fs::write(&log_path, marker).unwrap();
    }
    format!(
        r#"{{"id":{attempt_no},"ticket":{ticket},"attempt_no":{attempt_no},"base_commit":"{}","head_commit":"","branch":"factory/T{ticket}-widget","worktree":"","status":"{status}","exit_code":0,"log_path":"{}","started_at":"s","finished_at":"","train_branch":"","phase":"{phase}"}}"#,
        repo_head(h),
        log_path.to_string_lossy()
    )
}

fn prompts_log(h: &Harness) -> PathBuf {
    h.state.parent().unwrap().join("prompts.log")
}

fn clear_prompts(h: &Harness) {
    fs::write(prompts_log(h), "").unwrap();
}

fn read_prompts(h: &Harness) -> Vec<String> {
    fs::read_to_string(prompts_log(h))
        .unwrap()
        .split("=== PROMPT ===")
        .skip(1)
        .map(str::to_string)
        .collect()
}

/// Kill mid-implement, then a pass: the SAME attempt continues its phase,
/// the transcript survives under the existing tail bounds, and the resume
/// is marked in log + status, human and machine-readable alike.
#[test]
fn resume_same_attempt_continues_phase_with_transcript_preserved() {
    let h = setup("resume-same");
    let cap = write_fake_pi_capture(h.state.parent().unwrap());
    clear_prompts(&h);
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");

    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("FACTORY_PI_BIN", &cap)
        .env("PROMPTS_LOG", prompts_log(&h))
        .env("FACTORY_KILL_PHASE", "implement")
        .assert()
        .success();

    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("FACTORY_PI_BIN", &cap)
        .env("PROMPTS_LOG", prompts_log(&h))
        .assert()
        .success();

    // Observable state: one attempt, same number, resumed once, completed.
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1, "resume must not fork:\n{ledger}");
    assert_eq!(
        attempts[0].get("attempt_no").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        attempts[0].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "resume must be counted:\n{ledger}"
    );
    assert_eq!(
        attempts[0].get("status").and_then(|v| v.as_str()),
        Some("review_done"),
        "resumed attempt runs to completion:\n{ledger}"
    );

    // Transcript: pre-kill lines survive, resume marker appended, still
    // bounded by the existing default tail (50) and full on request.
    let full = run_status_json(&h, &["log", "11", "--json", "--tail", "0"]);
    let rows = full.get("attempts").and_then(|a| a.as_array()).unwrap();
    let transcript = rows[0].get("transcript").and_then(|v| v.as_str()).unwrap();
    assert!(
        transcript.contains("interrupted during implement"),
        "pre-kill story must survive the resume:\n{transcript}"
    );
    assert!(
        transcript.contains("resumed from implement checkpoint"),
        "resume marker must land in the transcript:\n{transcript}"
    );
    assert!(
        transcript.contains("implement line 100"),
        "post-resume work must append to the same transcript:\n{}",
        &transcript[transcript.len().saturating_sub(400)..]
    );
    let tail = run_status_json(&h, &["log", "11", "--json"]);
    let trows = tail.get("attempts").and_then(|a| a.as_array()).unwrap();
    let shown = trows[0].get("transcript").and_then(|v| v.as_str()).unwrap();
    assert_eq!(
        shown.lines().count(),
        50,
        "default tail bound still holds after resume"
    );
    assert_eq!(
        trows[0].get("truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    // The bounded tail is exactly the last 50 lines of the full story:
    // the bound is preserved, post-resume work is visible, and the
    // resume marker itself lives in the full transcript and the summary.
    let full_lines: Vec<&str> = transcript.lines().collect();
    let expect = full_lines[full_lines.len() - 50..].join("\n") + "\n";
    assert_eq!(
        shown, expect,
        "bounded tail must be the last 50 lines of the preserved transcript"
    );
    assert!(
        shown.contains("implement line 100"),
        "bounded tail must show post-resume work:\n{shown}"
    );
    assert_eq!(
        trows[0].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "machine log must carry the resume count:\n{tail}"
    );

    // dispatch status: human + machine carry the resumed fact.
    let human = factory_cmd(&h, &["dispatch", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(human).unwrap();
    assert!(
        human.contains("resumed"),
        "dispatch status must mark the resumed pass:\n{human}"
    );
    let machine = run_status_json(&h, &["dispatch", "status", "--json"]);
    let m_attempts = machine.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        m_attempts[0].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "machine status must match the human story:\n{machine}"
    );

    // unified status --json: identical resumed fact.
    let unified = run_status_json(&h, &["status", "--json"]);
    let u_attempts = unified.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        u_attempts[0].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "unified status must match:\n{unified}"
    );

    // ticket log human: the calm story names the resume.
    let log_human = factory_cmd(&h, &["log", "11"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let log_human = String::from_utf8(log_human).unwrap();
    assert!(
        log_human.to_lowercase().contains("resum"),
        "ticket log must tell the resume story:\n{log_human}"
    );

    // Audit trail: the ticket heard about the resume.
    let gh = fs::read_to_string(&h.gh_log).unwrap();
    assert!(
        gh.contains("resumed from implement checkpoint"),
        "ticket must hear about the resume:\n{gh}"
    );
}

/// STOP mid-attempt leaves a resumable checkpoint; a later pass (after the
/// loop restarts) resumes the same attempt instead of forking.
#[test]
fn stop_mid_attempt_leaves_resumable_then_later_pass_resumes() {
    let h = setup("stop-resume");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    repo_branch(&h, "factory/T11-widget");
    let running = crafted_attempt(
        &h,
        11,
        1,
        "impl_running",
        "implement",
        Some("dispatch attempt T11-a1 implement base=b branch=factory/T11-widget\n"),
    );
    write_ledger(&h, &format!(r#"{{"attempts":[{running}]}}"#));
    fs::write(h.state.join("STOP"), "stopped\n").unwrap();

    // The STOP pass does no work but leaves the resumable checkpoint.
    let out = factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("STOP file present"),
        "STOP pass must halt:\n{out}"
    );
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        attempts[0].get("status").and_then(|v| v.as_str()),
        Some("implement_interrupted"),
        "STOP mid-attempt must leave a resumable checkpoint:\n{ledger}"
    );

    // Restart the loop, then the next pass resumes the same attempt.
    factory_cmd(&h, &["dispatch", "resume"]).assert().success();
    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success();
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1, "resume must not fork:\n{ledger}");
    assert_eq!(
        attempts[0].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "STOP-interrupted attempt resumes:\n{ledger}"
    );
    assert_eq!(
        attempts[0].get("status").and_then(|v| v.as_str()),
        Some("review_done"),
        "resumed attempt completes:\n{ledger}"
    );
}

/// A stale lease (running record, dead worker) is reaped to resumable and
/// resumed in the next pass — crashes never fork competing histories.
#[test]
fn stale_running_reaped_to_resumable_and_resumed() {
    let h = setup("stale-resume");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    repo_branch(&h, "factory/T11-widget");
    let running = crafted_attempt(
        &h,
        11,
        1,
        "impl_running",
        "implement",
        Some("dispatch attempt T11-a1 implement base=b branch=factory/T11-widget\n"),
    );
    write_ledger(&h, &format!(r#"{{"attempts":[{running}]}}"#));

    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success();

    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1, "stale lease must not fork:\n{ledger}");
    assert_eq!(
        attempts[0].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "stale lease resumes on the same record:\n{ledger}"
    );
    let gh = fs::read_to_string(&h.gh_log).unwrap();
    assert!(
        gh.to_lowercase().contains("stale") || gh.to_lowercase().contains("lease"),
        "the reaping must stay audible:\n{gh}"
    );
}

/// The slot gate holds for resumes: at capacity, a resume is deferred
/// (dry-run admits nothing and writes nothing); capacity freed, it resumes.
#[test]
fn slot_gate_blocks_resume_before_admission() {
    let h = setup("gate-resume");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    write_view(&h, 22, "Gadget", "Build it.", &["ticket", "ready"], "OPEN");
    repo_branch(&h, "factory/T11-widget");
    let resumable = crafted_attempt(
        &h,
        11,
        1,
        "implement_interrupted",
        "implement",
        Some("interrupted during implement (slot T11-a1) — resumable\n"),
    );
    let live = crafted_attempt(&h, 22, 1, "impl_running", "implement", Some("working\n"));
    write_ledger(&h, &format!(r#"{{"attempts":[{resumable},{live}]}}"#));
    let before = ledger_raw(&h);

    // Dry-run at capacity: the gate bites before any resume is admitted,
    // and nothing is written anywhere.
    let out = factory_cmd(&h, &["dispatch", "once", "--dry-run"])
        .env("GH_LIST_JSON", r#"[{"number":11},{"number":22}]"#)
        .env("MAX_CONCURRENT", "1")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.to_lowercase().contains("capacity"),
        "slot gate must refuse the resume first:\n{out}"
    );
    assert!(
        !out.contains("would resume"),
        "no resume may be admitted at capacity:\n{out}"
    );
    assert_eq!(
        ledger_raw(&h),
        before,
        "dry-run must write no checkpoints and admit nothing"
    );
    assert!(
        fs::read_to_string(&h.gh_log).unwrap().is_empty(),
        "dry-run must not touch the ticket"
    );

    // Dry-run with capacity: the resume is announced, still nothing written.
    // (A ledger with only the resumable row: no stale lease occupies a slot.)
    write_ledger(&h, &format!(r#"{{"attempts":[{resumable}]}}"#));
    let before = ledger_raw(&h);
    let out = factory_cmd(&h, &["dispatch", "once", "--dry-run"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("MAX_CONCURRENT", "1")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("would resume"),
        "dry-run must announce the pending resume:\n{out}"
    );
    assert_eq!(
        ledger_raw(&h),
        before,
        "dry-run announcement must not write"
    );
}

/// Mid-attempt comments and structured answers wait for the next attempt
/// boundary: a resume replays only already-delivered answers, and the next
/// fresh attempt delivers the arrival exactly once.
#[test]
fn mid_attempt_answers_wait_for_next_attempt_boundary() {
    let h = setup("answer-resume");
    let cap = write_fake_pi_capture(h.state.parent().unwrap());
    clear_prompts(&h);
    write_view_with_comments(
        &h,
        11,
        "Widget",
        "Build it.",
        &["ticket", "ready"],
        "OPEN",
        &["Answer[q1]: first answer words."],
    );

    // Pass 1: q1 delivered at admission, then killed mid-implement.
    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("FACTORY_PI_BIN", &cap)
        .env("PROMPTS_LOG", prompts_log(&h))
        .env("FACTORY_KILL_PHASE", "implement")
        .assert()
        .success();
    let delivered: Vec<String> = ledger_json(&h)
        .get("delivered_answers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap();
    assert_eq!(delivered.len(), 1, "q1 delivered at admission");

    // Bea replies mid-attempt: q2 must wait for the next attempt boundary.
    write_view_with_comments(
        &h,
        11,
        "Widget",
        "Build it.",
        &["ticket", "ready"],
        "OPEN",
        &[
            "Answer[q1]: first answer words.",
            "Answer[q2]: mid-attempt words.",
        ],
    );

    // Pass 2 (resume): same attempt, q1 survives the restart, q2 waits.
    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("FACTORY_PI_BIN", &cap)
        .env("PROMPTS_LOG", prompts_log(&h))
        .assert()
        .success();
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1, "resume must not fork:\n{ledger}");
    let delivered_now: Vec<String> = ledger
        .get("delivered_answers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap();
    assert_eq!(
        delivered_now, delivered,
        "resume must redeliver nothing new"
    );
    // Implement prompts start with the ticket brief; review prompts start
    // with the worktree path. Only implement briefs carry answers.
    let prompts = read_prompts(&h);
    let implement: Vec<&String> = prompts
        .iter()
        .filter(|p| p.trim_start().starts_with("Ticket issue:"))
        .collect();
    assert_eq!(implement.len(), 1, "one resumed implement run");
    assert!(
        implement[0].contains("first answer words"),
        "delivered q1 must survive the restart:\n{}",
        implement[0]
    );
    assert!(
        !implement[0].contains("mid-attempt words"),
        "mid-attempt q2 must wait for the next boundary:\n{}",
        implement[0]
    );

    // Pass 3 (fresh attempt): q2 delivered exactly once, q1 never again.
    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("FACTORY_PI_BIN", &cap)
        .env("PROMPTS_LOG", prompts_log(&h))
        .assert()
        .success();
    let ledger = ledger_json(&h);
    let delivered_after: Vec<String> = ledger
        .get("delivered_answers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap();
    assert_eq!(
        delivered_after.len(),
        2,
        "q2 delivered exactly once at the next boundary:\n{ledger}"
    );
    let prompts = read_prompts(&h);
    let implement: Vec<&String> = prompts
        .iter()
        .filter(|p| p.trim_start().starts_with("Ticket issue:"))
        .collect();
    assert_eq!(implement.len(), 2, "resume run plus one fresh attempt");
    let last = implement.last().unwrap();
    assert!(
        last.contains("mid-attempt words"),
        "next attempt must carry q2:\n{last}"
    );
    assert!(
        !last.contains("first answer words"),
        "q1 must stay single-use:\n{last}"
    );
}

/// A conflicting resume refuses instead of forking: with the pass lock
/// held elsewhere, nothing is admitted and history is untouched.
#[test]
fn conflicting_resume_refuses_without_forking() {
    let h = setup("conflict-resume");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    repo_branch(&h, "factory/T11-widget");
    let resumable = crafted_attempt(
        &h,
        11,
        1,
        "implement_interrupted",
        "implement",
        Some("interrupted during implement (slot T11-a1) — resumable\n"),
    );
    write_ledger(&h, &format!(r#"{{"attempts":[{resumable}]}}"#));
    fs::create_dir_all(h.state.join(".lock")).unwrap();
    let before = ledger_raw(&h);

    let out = factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("another dispatcher holds the lock"),
        "conflicting pass must refuse:\n{out}"
    );
    assert_eq!(
        ledger_raw(&h),
        before,
        "conflicting resume must not fork history"
    );
}

/// The newest resumable checkpoint wins: older interrupted history is left
/// untouched while the newest resumes on its own record.
#[test]
fn newest_resumable_wins_and_older_history_untouched() {
    let h = setup("newest-resume");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    repo_branch(&h, "factory/T11-widget");
    let older = crafted_attempt(
        &h,
        11,
        1,
        "plan_interrupted",
        "plan",
        Some("interrupted during plan (slot T11-a1) — resumable\n"),
    );
    let log2 = h.state.join("logs").join("T11-a2-impl.log");
    fs::create_dir_all(log2.parent().unwrap()).unwrap();
    fs::write(
        &log2,
        "interrupted during implement (slot T11-a2) — resumable\n",
    )
    .unwrap();
    let newer = format!(
        r#"{{"id":2,"ticket":11,"attempt_no":2,"base_commit":"{}","head_commit":"","branch":"factory/T11-widget","worktree":"","status":"implement_interrupted","exit_code":0,"log_path":"{}","started_at":"s","finished_at":"","train_branch":"","phase":"implement"}}"#,
        repo_head(&h),
        log2.to_string_lossy()
    );
    write_ledger(&h, &format!(r#"{{"attempts":[{older},{newer}]}}"#));
    let older_log_before =
        fs::read_to_string(h.state.join("logs").join("T11-a1-impl.log")).unwrap();

    factory_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success();

    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 2, "resume must not fork:\n{ledger}");
    assert_eq!(
        attempts[0].get("status").and_then(|v| v.as_str()),
        Some("plan_interrupted"),
        "older history untouched:\n{ledger}"
    );
    assert_eq!(
        attempts[0].get("resumed").and_then(|v| v.as_u64()),
        Some(0),
        "older record gains no resume mark:\n{ledger}"
    );
    assert_eq!(
        attempts[1].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "newest checkpoint resumes:\n{ledger}"
    );
    assert_eq!(
        attempts[1].get("status").and_then(|v| v.as_str()),
        Some("review_done"),
        "newest resumes to completion:\n{ledger}"
    );
    assert_eq!(
        fs::read_to_string(h.state.join("logs").join("T11-a1-impl.log")).unwrap(),
        older_log_before,
        "older transcript untouched"
    );
}

/// Paused-and-will-resume states read as calm sentences on every human
/// surface, and the replay-safety blind spot is stated in output copy
/// rather than docs alone (ticket #63, spec #59): a restart must never
/// look like breakage, and the "replays only where replay-safe" limit
/// must stay visible where Engineer Eli already looks.
#[test]
fn paused_states_read_calm_and_name_the_replay_safety_blind_spot() {
    let h = setup("calm-blind-spot");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    repo_branch(&h, "factory/T11-widget");
    let resumable = crafted_attempt(
        &h,
        11,
        1,
        "implement_interrupted",
        "implement",
        Some("interrupted during implement (slot T11-a1) — resumable\n"),
    );
    write_ledger(&h, &format!(r#"{{"attempts":[{resumable}]}}"#));

    for args in [
        &["dispatch", "status"][..],
        &["status"][..],
        &["log", "11"][..],
    ] {
        let out = factory_cmd(&h, args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("Paused") && out.to_lowercase().contains("will resume"),
            "paused state must read as a calm sentence ({args:?}):\n{out}"
        );
        assert!(
            out.contains("replay-safe"),
            "output copy must state the replay-safety blind spot ({args:?}):\n{out}"
        );
    }

    // The blind spot is also stated where the pass is surveyed.
    let help = factory_cmd(&h, &["dispatch", "once", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help_text = String::from_utf8(help).unwrap();
    assert!(
        help_text.contains("replay-safe"),
        "dispatch once help must state the blind spot:\n{help_text}"
    );
}
