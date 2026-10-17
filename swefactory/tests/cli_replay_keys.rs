//! Replayable attempts, slice 3: exactly-once idempotency keys (ticket #62,
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
        "swefactory-replay-keys-{}-{}-{}",
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

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn ledger_json(h: &Harness) -> serde_json::Value {
    let text =
        fs::read_to_string(h.state.join("dispatch.json")).expect("ledger must exist after a pass");
    serde_json::from_str(&text).expect("ledger must stay valid JSON")
}

/// The trailing machine-readable summary line of a `--json` once pass:
/// stdout mixes human lines with one JSON line, so find the last line
/// that parses as JSON instead of parsing all of stdout.
fn json_summary(stdout: &str) -> serde_json::Value {
    stdout
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .next()
        .expect("a --json pass must end with a JSON summary line")
}

/// Sends are `issue comment` / `issue edit` invocations in the fake-gh log
/// (blocks are separated by `---`; `issue list` / `issue view` are reads).
fn gh_sends(h: &Harness) -> usize {
    let log = fs::read_to_string(&h.gh_log).unwrap();
    log.split("---\n")
        .filter(|block| {
            let mut lines = block.lines();
            matches!(
                (lines.next(), lines.next()),
                (Some("issue"), Some("comment" | "edit"))
            )
        })
        .count()
}

/// Admitting twice with the same key opens one attempt; the second response
/// carries a replay marker and performs no new sends.
#[test]
fn same_key_twice_admits_once_with_replay_marker() {
    let h = setup("same-key");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");

    let out1 = factory_cmd(&h, &["dispatch", "once", "--idempotency-key", "key-abc"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout1 = stdout_of(&out1);
    assert!(
        stdout1.contains("key-abc"),
        "admission must name the key:\n{stdout1}"
    );
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        attempts.len(),
        1,
        "first pass admits one attempt:\n{ledger}"
    );
    let sends_after_first = gh_sends(&h);

    // Repeat with the same key: no new attempt, replay marker, no new sends.
    let out2 = factory_cmd(&h, &["dispatch", "once", "--idempotency-key", "key-abc"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout2 = stdout_of(&out2);
    assert!(
        stdout2.to_lowercase().contains("replay"),
        "repeat must carry a replay marker:\n{stdout2}"
    );
    assert!(
        stdout2.contains("key-abc"),
        "replay must name the key:\n{stdout2}"
    );
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1, "repeat must not double-admit:\n{ledger}");
    assert_eq!(gh_sends(&h), sends_after_first, "replay must not re-send");
}

/// Reusing one key across different operations is rejected, nonzero, with
/// nothing admitted.
#[test]
fn key_reuse_across_operations_is_rejected() {
    let h = setup("cross-op");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    write_view(&h, 12, "Gadget", "Build it.", &["ticket", "ready"], "OPEN");

    factory_cmd(&h, &["dispatch", "once", "--idempotency-key", "key-shared"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success();
    assert_eq!(
        ledger_json(&h)
            .get("attempts")
            .and_then(|a| a.as_array())
            .unwrap()
            .len(),
        1
    );

    // Same key, different operation (a different open ticket): rejected.
    let out = factory_cmd(&h, &["dispatch", "once", "--idempotency-key", "key-shared"])
        .env("GH_LIST_JSON", r#"[{"number":12}]"#)
        .assert()
        .failure()
        .get_output()
        .clone();
    let stdout = stdout_of(&out);
    let stderr = String::from_utf8(out.stderr.clone()).unwrap();
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.to_lowercase().contains("different operation"),
        "reuse across operations must be rejected loudly:\n{combined}"
    );
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert!(
        attempts
            .iter()
            .all(|a| a.get("ticket").and_then(|t| t.as_u64()) != Some(12)),
        "rejected reuse must admit nothing for the new operation:\n{ledger}"
    );
    assert_eq!(attempts.len(), 1, "ledger must be unchanged:\n{ledger}");
}

/// The key retention horizon is stated in help output.
#[test]
fn retention_horizon_stated_in_help() {
    let h = setup("help-horizon");
    let out = factory_cmd(&h, &["dispatch", "once", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("24 hour"),
        "help must state the key retention horizon:\n{stdout}"
    );
    assert!(
        stdout.contains("idempotency-key"),
        "help must document the key flag:\n{stdout}"
    );
}

/// Human and machine-readable outputs carry identical admission and replay
/// facts (key, ticket, attempt number, replayed flag).
#[test]
fn human_and_json_carry_identical_admission_and_replay_facts() {
    let h = setup("parity");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");

    // Admission in machine mode.
    let out = factory_cmd(
        &h,
        &["--json", "dispatch", "once", "--idempotency-key", "key-par"],
    )
    .env("GH_LIST_JSON", r#"[{"number":11}]"#)
    .assert()
    .success()
    .get_output()
    .clone();
    let summary = json_summary(&stdout_of(&out));
    assert_eq!(
        summary.get("idempotency_key").and_then(|v| v.as_str()),
        Some("key-par"),
        "machine admission must carry the key:\n{summary}"
    );
    assert_eq!(
        summary.get("replayed").and_then(|v| v.as_bool()),
        Some(false),
        "first admission must not read replayed:\n{summary}"
    );
    let ticket = summary.get("ticket").and_then(|v| v.as_u64()).unwrap();
    let attempt_no = summary.get("attempt_no").and_then(|v| v.as_u64()).unwrap();
    let status = summary
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    assert_eq!(ticket, 11);

    // Replay in human mode carries the identical facts.
    let out = factory_cmd(&h, &["dispatch", "once", "--idempotency-key", "key-par"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success()
        .get_output()
        .clone();
    let human = stdout_of(&out);
    for fact in [
        "key-par".to_string(),
        ticket.to_string(),
        attempt_no.to_string(),
        status.clone(),
    ] {
        assert!(
            human.contains(&fact),
            "human replay must carry the same fact '{fact}':\n{human}"
        );
    }
    assert!(human.to_lowercase().contains("replay"));

    // Replay in machine mode carries the identical facts too.
    let out = factory_cmd(
        &h,
        &["--json", "dispatch", "once", "--idempotency-key", "key-par"],
    )
    .env("GH_LIST_JSON", r#"[{"number":11}]"#)
    .assert()
    .success()
    .get_output()
    .clone();
    let replay = json_summary(&stdout_of(&out));
    assert_eq!(
        replay.get("replayed").and_then(|v| v.as_bool()),
        Some(true),
        "repeat must read replayed in machine output:\n{replay}"
    );
    assert_eq!(
        replay.get("idempotency_key").and_then(|v| v.as_str()),
        Some("key-par")
    );
    assert_eq!(replay.get("ticket").and_then(|v| v.as_u64()), Some(ticket));
    assert_eq!(
        replay.get("attempt_no").and_then(|v| v.as_u64()),
        Some(attempt_no)
    );
    assert_eq!(
        replay.get("status").and_then(|v| v.as_str()),
        Some(status.as_str())
    );
}

/// A key older than the retention horizon admits anew (the window bounds
/// the guarantee).
#[test]
fn expired_key_admits_anew() {
    let h = setup("expired");
    write_view(&h, 11, "Widget", "Build it.", &["ticket", "ready"], "OPEN");
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        r#"{"attempts":[],"idempotency_keys":[
{"key":"key-old","operation":"dispatch-once:11","ticket":11,"attempt_no":1,"status":"impl_done","stored_at":"1"}]}"#,
    )
    .unwrap();

    let out = factory_cmd(&h, &["dispatch", "once", "--idempotency-key", "key-old"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = stdout_of(&out);
    assert!(
        !stdout.to_lowercase().contains("replay"),
        "an expired key must admit anew, not replay:\n{stdout}"
    );
    let ledger = ledger_json(&h);
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(
        attempts.len(),
        1,
        "expired key admits a fresh attempt:\n{ledger}"
    );
}
