//! Bounded phase waits (ticket #94).
//!
//! A hung agent must never wedge a pass forever: implement/review phases
//! each have a bounded wait (bash parity defaults), expiry kills the worker
//! process tree, the row becomes a resumable checkpoint, and the timeout
//! value is named in the attempt log and in status output (both modes).
//!
//! Seam: argv in -> exit code + stdout/stderr out + state-dir files and
//! subprocess argv logs out. `gh` is a fake script (no network), `git` is a
//! logging shim over the real binary, `pi` is a sleeper-or-fast fake.
//! Timing is outcome-based with wide margins (1s bound vs 60s sleep), so
//! no assertion can race the wall clock.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-timeouts-{}-{}-{}",
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

/// Fast fake: drops a file change and accepts (implement + review).
fn write_fast_pi(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi",
        r#"#!/bin/sh
echo "pi cwd=$(pwd) slot:${FACTORY_SLOT:-unset}" >> "$PI_LOG"
echo "change from pi" >> change.txt
echo "notes"
echo "VERDICT: accept"
"#,
    )
}

/// Hanging implement fake: sleeps 60 in a background child (a real process
/// tree: shell + sleep), recording the child's pid so the test can prove
/// the tree died, not just the wait.
fn write_sleepy_implement_pi(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi-sleepy-implement",
        r#"#!/bin/sh
echo "started pid=$$ cwd=$(pwd)" >> "$PI_LOG"
sleep 60 &
echo $! > "$CHILD_PID_FILE"
echo "child=$!" >> "$PI_LOG"
wait
echo "finished (must never print: the wait is bounded)" >> "$PI_LOG"
"#,
    )
}

/// Fast implement, hanging review: only review worktrees (path carries
/// `review-`) sleep; everywhere else behaves like the fast fake.
fn write_sleepy_review_pi(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi-sleepy-review",
        r#"#!/bin/sh
# Only review worktrees (basename `review-*`) hang; the implement
# worktree (basename `T<ticket>-a<attempt>`) takes the fast path.
case "$(basename "$(pwd)")" in
review-*)
  echo "started pid=$$ cwd=$(pwd)" >> "$PI_LOG"
  sleep 60 &
  echo $! > "$CHILD_PID_FILE"
  echo "child=$!" >> "$PI_LOG"
  wait
  echo "finished (must never print: the wait is bounded)" >> "$PI_LOG"
  ;;
*)
  echo "pi cwd=$(pwd) slot:${FACTORY_SLOT:-unset}" >> "$PI_LOG"
  echo "change from pi" >> change.txt
  echo "notes"
  echo "VERDICT: accept"
  ;;
esac
"#,
    )
}

fn write_fake_adapter(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "adapter.sh",
        r#"#!/bin/sh
echo "FACTORY-CHECK PASSED"
exit 0
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
    child_pid: PathBuf,
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
        child_pid: root.join("child.pid"),
        gh_log: root.join("gh.log"),
        git_log: root.join("git.log"),
        pi_log: root.join("pi.log"),
        gh: write_fake_gh(&root),
        git_shim: write_git_shim(&root),
        pi: write_fast_pi(&root),
        adapter: write_fake_adapter(&root),
        repo,
        views,
    };
    for f in [&h.gh_log, &h.git_log, &h.pi_log] {
        fs::write(f, "").unwrap();
    }
    h
}

fn write_view(h: &Harness, number: u64, title: &str, body: &str, labels: &[&str], state: &str) {
    fs::write(
        h.views.join(format!("{number}.json")),
        ticket_view(number, title, body, labels, state),
    )
    .unwrap();
}

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
        .env("CHILD_PID_FILE", &h.child_pid)
        .env("GH_VIEWS_DIR", &h.views)
        .env("GH_LIST_JSON", "[]")
        .env_remove("FACTORY_SLOT");
    cmd
}

fn ledger(h: &Harness) -> serde_json::Value {
    let text = fs::read_to_string(h.state.join("dispatch.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn attempts(v: &serde_json::Value) -> &Vec<serde_json::Value> {
    v.get("attempts").and_then(|a| a.as_array()).unwrap()
}

/// True while the pid is alive (`kill -0` succeeds; probe noise silenced).
fn pid_alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

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
    }
    assert!(
        !gh.lines().any(|l| l.trim() == "close"),
        "dispatcher must never close issues:\n{gh}"
    );
}

#[test]
fn implement_timeout_kills_tree_checkpoints_and_resumes() {
    let h = setup("impl-timeout");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let sleepy = write_sleepy_implement_pi(&tmpdir("impl-timeout-pi"));
    // Pass 1: the implementer sleeps 60s, the bound is 1s. The pass must
    // finish promptly with the worker tree dead and the row resumable.
    let out = dispatch_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("FACTORY_PI_BIN", &sleepy)
        .env("TIMEOUT_IMPL", "1")
        .env("TIMEOUT_REVIEW", "1200")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out).into_owned();
    assert!(
        stdout.contains("timed out") && stdout.contains("1s"),
        "expiry must name the timeout value on stdout:\n{stdout}"
    );

    let parsed = ledger(&h);
    assert_eq!(attempts(&parsed).len(), 1, "one attempt:\n{parsed}");
    let row = &attempts(&parsed)[0];
    assert_eq!(
        row.get("status").and_then(|v| v.as_str()),
        Some("implement_interrupted"),
        "expiry checkpoints as interrupted (resumable), never failed:\n{parsed}"
    );
    assert!(
        row.get("timed_out")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "expiry records its cause on the row:\n{parsed}"
    );
    assert_eq!(
        row.get("timeout_secs").and_then(|v| v.as_u64()),
        Some(1),
        "expiry records the bound value:\n{parsed}"
    );

    // The attempt log names the timeout value.
    let log_path = row
        .get("log_path")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    let log = fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("timed out") && log.contains("1s"),
        "attempt log must name the timeout value:\n{log}"
    );

    // The worker slept 60s: it must be dead now (killed, not abandoned).
    let pi_log = fs::read_to_string(&h.pi_log).unwrap();
    assert!(pi_log.contains("started"), "sleeper ran:\n{pi_log}");
    assert!(
        !pi_log.contains("finished"),
        "bounded wait never lets the sleeper finish:\n{pi_log}"
    );
    let child = fs::read_to_string(&h.child_pid).unwrap();
    let child = child.trim();
    // Reaped zombies read as dead to kill -0 once waited; give the
    // kernel a beat, then the tree must be gone.
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        !pid_alive(child),
        "expiry kills the worker process tree, not just the wait (child {child} alive)"
    );

    // The ticket comment names the timeout value too.
    let gh = fs::read_to_string(&h.gh_log).unwrap();
    assert!(
        gh.contains("timed out") && gh.contains("1s"),
        "ticket comment must name the timeout value:\n{gh}"
    );

    // Status names the timeout value in both modes (same 1s bound in env
    // so status reads the effective values the pass used).
    let status_human = dispatch_cmd(&h, &["dispatch", "status"])
        .env("TIMEOUT_IMPL", "1")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_human = String::from_utf8_lossy(&status_human).into_owned();
    assert!(
        status_human.contains("expired (timeout 1s)") && status_human.contains("implement=1s"),
        "human status must name the timeout value:\n{status_human}"
    );
    let status_json = dispatch_cmd(&h, &["--json", "dispatch", "status"])
        .env("TIMEOUT_IMPL", "1")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status_json = String::from_utf8_lossy(&status_json).into_owned();
    let v: serde_json::Value = serde_json::from_str(&status_json).unwrap();
    assert_eq!(
        v.get("timeouts")
            .and_then(|t| t.get("implement"))
            .and_then(|n| n.as_u64()),
        Some(1),
        "JSON status must carry the effective implement bound:\n{status_json}"
    );
    let row = v
        .get("attempts")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap();
    assert_eq!(
        row.get("timed_out").and_then(|b| b.as_bool()),
        Some(true),
        "JSON row must record the expiry cause:\n{status_json}"
    );
    assert_eq!(
        row.get("timeout_secs").and_then(|n| n.as_u64()),
        Some(1),
        "JSON row must record the bound value:\n{status_json}"
    );

    // Pass 2: no human lease surgery — the same attempt resumes from its
    // checkpoint with the fast runner and runs to completion.
    dispatch_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("TIMEOUT_IMPL", "1200")
        .assert()
        .success();
    let parsed = ledger(&h);
    assert_eq!(
        attempts(&parsed).len(),
        1,
        "resume keeps one row:\n{parsed}"
    );
    let row = &attempts(&parsed)[0];
    assert_eq!(
        row.get("status").and_then(|v| v.as_str()),
        Some("review_done"),
        "resumed attempt completes:\n{parsed}"
    );
    assert!(
        row.get("resumed").and_then(|v| v.as_u64()).unwrap_or(0) >= 1,
        "resume is counted:\n{parsed}"
    );
    assert_never_touches_dev_or_lands(
        &fs::read_to_string(&h.git_log).unwrap(),
        &fs::read_to_string(&h.gh_log).unwrap(),
    );
}

#[test]
fn review_timeout_kills_tree_checkpoints_and_resumes() {
    let h = setup("review-timeout");
    write_view(
        &h,
        12,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let sleepy = write_sleepy_review_pi(&tmpdir("review-timeout-pi"));
    let out = dispatch_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":12}]"#)
        .env("FACTORY_PI_BIN", &sleepy)
        .env("TIMEOUT_IMPL", "1200")
        .env("TIMEOUT_REVIEW", "1")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out).into_owned();
    assert!(
        stdout.contains("timed out") && stdout.contains("1s"),
        "review expiry must name the timeout value on stdout:\n{stdout}"
    );

    let parsed = ledger(&h);
    assert_eq!(attempts(&parsed).len(), 1, "one attempt:\n{parsed}");
    let row = &attempts(&parsed)[0];
    assert_eq!(
        row.get("status").and_then(|v| v.as_str()),
        Some("review_interrupted"),
        "review expiry checkpoints as interrupted (resumable):\n{parsed}"
    );
    assert_eq!(
        row.get("timeout_secs").and_then(|v| v.as_u64()),
        Some(1),
        "review expiry records the bound value:\n{parsed}"
    );
    let rlog = h.state.join("logs").join("T12-a1-review.log");
    let rlog_text = fs::read_to_string(&rlog).unwrap();
    assert!(
        rlog_text.contains("timed out") && rlog_text.contains("1s"),
        "review log must name the timeout value:\n{rlog_text}"
    );

    // No stuck running row: nothing reads *_running after expiry.
    for a in attempts(&parsed) {
        let s = a.get("status").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            !s.ends_with("_running"),
            "expired attempt leaves no stuck lease:\n{parsed}"
        );
    }

    // Pass 2 with the fast runner resumes the same attempt to completion.
    dispatch_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":12}]"#)
        .env("TIMEOUT_REVIEW", "1200")
        .assert()
        .success();
    let parsed = ledger(&h);
    assert_eq!(
        attempts(&parsed).len(),
        1,
        "resume keeps one row:\n{parsed}"
    );
    assert_eq!(
        attempts(&parsed)[0].get("status").and_then(|v| v.as_str()),
        Some("review_done"),
        "resumed review completes:\n{parsed}"
    );
    assert_never_touches_dev_or_lands(
        &fs::read_to_string(&h.git_log).unwrap(),
        &fs::read_to_string(&h.gh_log).unwrap(),
    );
}

#[test]
fn dry_run_shows_effective_timeouts_and_writes_nothing() {
    let h = setup("dry-run-timeouts");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build it.",
        &["ticket", "ready"],
        "OPEN",
    );
    // Env configures the bounds; dry-run announces them and changes nothing.
    let out = dispatch_cmd(&h, &["dispatch", "once", "--dry-run"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("TIMEOUT_IMPL", "7")
        .env("TIMEOUT_REVIEW", "9")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out).into_owned();
    assert!(
        stdout.contains("implement=7s") && stdout.contains("review=9s"),
        "dry-run must show the effective values:\n{stdout}"
    );
    assert!(
        !h.state.join("dispatch.json").is_file(),
        "dry-run records no attempts"
    );

    // Flags win over env, same bounds story as the concurrency level.
    let out = dispatch_cmd(&h, &["dispatch", "once", "--dry-run"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("TIMEOUT_IMPL", "7")
        .env("TIMEOUT_REVIEW", "9")
        .args(["--timeout-impl", "5", "--timeout-review", "6"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out).into_owned();
    assert!(
        stdout.contains("implement=5s") && stdout.contains("review=6s"),
        "flags must win over env:\n{stdout}"
    );

    // Bash parity defaults when nothing configures them.
    let out = dispatch_cmd(&h, &["dispatch", "once", "--dry-run"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env_remove("TIMEOUT_IMPL")
        .env_remove("TIMEOUT_REVIEW")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&out).into_owned();
    assert!(
        stdout.contains("implement=1800s") && stdout.contains("review=1200s"),
        "defaults stay at bash parity:\n{stdout}"
    );
}

#[test]
fn status_names_timeout_bounds_in_both_modes() {
    let h = setup("status-bounds");
    let human = dispatch_cmd(&h, &["dispatch", "status"])
        .env("TIMEOUT_IMPL", "11")
        .env("TIMEOUT_REVIEW", "13")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8_lossy(&human).into_owned();
    assert!(
        human.contains("implement=11s") && human.contains("review=13s"),
        "human status must name the bounds:\n{human}"
    );
    let machine = dispatch_cmd(&h, &["--json", "dispatch", "status"])
        .env("TIMEOUT_IMPL", "11")
        .env("TIMEOUT_REVIEW", "13")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let machine = String::from_utf8_lossy(&machine).into_owned();
    let v: serde_json::Value = serde_json::from_str(&machine).unwrap();
    assert_eq!(
        v.get("timeouts")
            .and_then(|t| t.get("implement"))
            .and_then(|n| n.as_u64()),
        Some(11),
        "JSON status must carry the implement bound:\n{machine}"
    );
    assert_eq!(
        v.get("timeouts")
            .and_then(|t| t.get("review"))
            .and_then(|n| n.as_u64()),
        Some(13),
        "JSON status must carry the review bound:\n{machine}"
    );
}
