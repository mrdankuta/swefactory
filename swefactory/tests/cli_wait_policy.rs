//! Child-task waiting policies (ticket #68, spec #59 follow-up).
//!
//! Seam: argv in -> exit code + stdout/stderr out + state-dir files out,
//! over a fixture ledger plus a scratch git repo. No daemon, no network,
//! no subprocesses: waiting runs no code. Tests assert only
//! invoked-arguments to exit code plus output plus observable state.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-wait-{}-{}-{}",
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

fn read_ledger(h: &Harness) -> serde_json::Value {
    let text = fs::read_to_string(h.state.join("dispatch.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn attempt_row(ticket: u64, log_path: &str) -> serde_json::Value {
    serde_json::json!({
        "id": 1, "ticket": ticket, "attempt_no": 1,
        "base_commit": "abc123def456", "head_commit": "",
        "branch": "factory/T68-demo", "worktree": "/tmp/wt-T68-a1",
        "status": "impl_running", "exit_code": 0,
        "log_path": log_path,
        "started_at": "1700000000", "finished_at": "",
        "train_branch": "", "phase": "implement", "resumed": 0
    })
}

fn task(
    id: &str,
    ticket: u64,
    parent: Option<&str>,
    kind: &str,
    status: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id, "ticket": ticket, "attempt": 1,
        "parent": parent, "kind": kind, "status": status,
        "waits_on": [], "wait_policy": "", "replay_safe": false
    })
}

fn seed_logs(h: &Harness, ticket: u64) -> String {
    let dir = h.state.join("logs");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("T{ticket}-a1.log"));
    fs::write(&path, "seed transcript\n").unwrap();
    path.to_string_lossy().into_owned()
}

fn wait_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

fn task_status(ledger: &serde_json::Value, id: &str) -> String {
    ledger["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap_or_else(|| panic!("missing task {id}"))
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap()
        .to_string()
}

#[test]
fn all_settled_waits_for_every_child_then_resumes_with_outcomes_in_log() {
    let h = setup("all-settled");
    let log = seed_logs(&h, 68);
    let mut ledger = serde_json::json!({
        "attempts": [attempt_row(68, &log)],
        "tasks": [
            task("plan", 68, None, "parent", "running"),
            task("suite-a", 68, Some("plan"), "child", "done"),
            task("suite-b", 68, Some("plan"), "child", "running"),
        ]
    });
    write_ledger(&h, &ledger);

    // Child still running: parent waits, stays live.
    let out = wait_cmd(
        &h,
        &[
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "plan",
            "--policy",
            "all-settled",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("waiting"), "must read waiting:\n{stdout}");
    assert!(
        stdout.contains("suite-b"),
        "must name the pending child:\n{stdout}"
    );
    ledger = read_ledger(&h);
    assert_eq!(task_status(&ledger, "plan"), "waiting");
    assert!(!is_terminal(&task_status(&ledger, "plan")));

    // Graph still lists the waiting parent.
    let graph = wait_cmd(&h, &["dispatch", "graph", "--ticket", "68"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&graph.stdout).contains("plan"));

    // suite-b finishes outside the wait (waiting runs no code): resume.
    ledger["tasks"][2]["status"] = serde_json::json!("done");
    write_ledger(&h, &ledger);
    let out = wait_cmd(
        &h,
        &[
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "plan",
            "--policy",
            "all-settled",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("resumed"), "must resume:\n{stdout}");
    assert!(
        stdout.contains("suite-a"),
        "each outcome visible:\n{stdout}"
    );
    assert!(
        stdout.contains("suite-b"),
        "each outcome visible:\n{stdout}"
    );
    ledger = read_ledger(&h);
    assert!(is_terminal(&task_status(&ledger, "plan")));

    // Outcomes visible in the ticket log transcript.
    let log_out = wait_cmd(&h, &["log", "68"]).output().unwrap();
    assert_eq!(log_out.status.code(), Some(0));
    let log_text = String::from_utf8_lossy(&log_out.stdout).into_owned();
    assert!(
        log_text.contains("suite-a"),
        "log shows child outcomes:\n{log_text}"
    );
    assert!(
        log_text.contains("suite-b"),
        "log shows child outcomes:\n{log_text}"
    );
}

#[test]
fn fail_fast_aborts_siblings_before_parent_decides_with_observable_order() {
    let h = setup("fail-fast");
    let log = seed_logs(&h, 68);
    write_ledger(
        &h,
        &serde_json::json!({
            "attempts": [attempt_row(68, &log)],
            "tasks": [
                task("plan", 68, None, "parent", "running"),
                task("suite-a", 68, Some("plan"), "child", "failed"),
                task("suite-b", 68, Some("plan"), "child", "running"),
                task("suite-c", 68, Some("plan"), "child", "waiting"),
            ]
        }),
    );

    let out = wait_cmd(
        &h,
        &[
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "plan",
            "--policy",
            "fail-fast",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("suite-a"),
        "names the failed child:\n{stdout}"
    );
    assert!(stdout.contains("abort"), "abort visible:\n{stdout}");
    // Abort order observable: suite-b before suite-c (sorted id order).
    let b = stdout.find("suite-b").expect("suite-b in output");
    let c = stdout.find("suite-c").expect("suite-c in output");
    assert!(b < c, "abort order suite-b then suite-c:\n{stdout}");

    let ledger = read_ledger(&h);
    assert_eq!(task_status(&ledger, "suite-b"), "aborted");
    assert_eq!(task_status(&ledger, "suite-c"), "aborted");
    assert_eq!(task_status(&ledger, "suite-a"), "failed");
    assert!(is_terminal(&task_status(&ledger, "plan")));

    let log_out = wait_cmd(&h, &["log", "68"]).output().unwrap();
    let log_text = String::from_utf8_lossy(&log_out.stdout).into_owned();
    let lb = log_text.find("suite-b").expect("suite-b in log");
    let lc = log_text.find("suite-c").expect("suite-c in log");
    assert!(lb < lc, "abort order in log:\n{log_text}");
    assert!(log_text.contains("abort"), "abort in log:\n{log_text}");
}

#[test]
fn kill_mid_wait_resumes_same_wait_without_rerunning_children() {
    let h = setup("kill");
    let log = seed_logs(&h, 68);
    write_ledger(
        &h,
        &serde_json::json!({
            "attempts": [attempt_row(68, &log)],
            "tasks": [
                task("plan", 68, None, "parent", "running"),
                task("suite-a", 68, Some("plan"), "child", "done"),
                task("suite-b", 68, Some("plan"), "child", "running"),
            ]
        }),
    );

    // Simulated kill mid-wait: nonzero, wait checkpointed, children untouched.
    let out = wait_cmd(
        &h,
        &[
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "plan",
            "--policy",
            "all-settled",
        ],
    )
    .env("FACTORY_KILL_WAIT", "1")
    .output()
    .unwrap();
    assert_ne!(out.status.code(), Some(0), "kill must fail the pass");
    let ledger = read_ledger(&h);
    assert_eq!(task_status(&ledger, "plan"), "waiting");
    assert_eq!(task_status(&ledger, "suite-a"), "done");
    assert_eq!(task_status(&ledger, "suite-b"), "running");

    // Resume without the seam: same wait, still waiting, children unchanged.
    let out = wait_cmd(
        &h,
        &[
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "plan",
            "--policy",
            "all-settled",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("waiting"));
    let ledger2 = read_ledger(&h);
    assert_eq!(task_status(&ledger2, "suite-a"), "done");
    assert_eq!(task_status(&ledger2, "suite-b"), "running");

    // Finish outside the wait, then resume completes without rerunning children.
    ledger2_snapshot_finish(&h);
    let out = wait_cmd(
        &h,
        &[
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "plan",
            "--policy",
            "all-settled",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("resumed"));
    let ledger3 = read_ledger(&h);
    assert_eq!(task_status(&ledger3, "suite-a"), "done");
    assert_eq!(task_status(&ledger3, "suite-b"), "done");
}

fn ledger2_snapshot_finish(h: &Harness) {
    let mut ledger = read_ledger(h);
    for t in ledger["tasks"].as_array_mut().unwrap() {
        if t["id"] == "suite-b" {
            t["status"] = serde_json::json!("done");
        }
    }
    write_ledger(h, &ledger);
}

fn is_terminal(status: &str) -> bool {
    matches!(
        status,
        "done" | "failed" | "aborted" | "cancelled" | "completed"
    ) || status.ends_with("_done")
        || status.ends_with("_failed")
        || status.ends_with("_aborted")
        || status.ends_with("_cancelled")
        || status.ends_with("_completed")
}

#[test]
fn wait_json_carries_identical_facts_and_rejects_bad_input() {
    let h = setup("json-err");
    let log = seed_logs(&h, 68);
    write_ledger(
        &h,
        &serde_json::json!({
            "attempts": [attempt_row(68, &log)],
            "tasks": [
                task("plan", 68, None, "parent", "running"),
                task("suite-a", 68, Some("plan"), "child", "failed"),
                task("suite-b", 68, Some("plan"), "child", "running"),
            ]
        }),
    );

    let human = wait_cmd(
        &h,
        &[
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "plan",
            "--policy",
            "fail-fast",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(human.status.code(), Some(0));
    let human_text = String::from_utf8_lossy(&human.stdout).into_owned();

    // Reset fixture so JSON sees the same pre-state.
    write_ledger(
        &h,
        &serde_json::json!({
            "attempts": [attempt_row(68, &log)],
            "tasks": [
                task("plan", 68, None, "parent", "running"),
                task("suite-a", 68, Some("plan"), "child", "failed"),
                task("suite-b", 68, Some("plan"), "child", "running"),
            ]
        }),
    );
    let out = wait_cmd(
        &h,
        &[
            "--json",
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "plan",
            "--policy",
            "fail-fast",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["parent"], serde_json::json!("plan"));
    assert_eq!(v["policy"], serde_json::json!("fail-fast"));
    let abort_order = v["abort_order"].as_array().unwrap();
    assert!(abort_order.contains(&serde_json::json!("suite-b")));
    for id in ["plan", "suite-a", "suite-b"] {
        assert!(human_text.contains(id), "human missing {id}:\n{human_text}");
    }

    // Bad policy, unknown parent, corrupt ledger: nonzero with cause.
    for args in [
        vec![
            "dispatch", "wait", "--ticket", "68", "--parent", "plan", "--policy", "bogus",
        ],
        vec![
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "nope",
            "--policy",
            "all-settled",
        ],
    ] {
        let out = wait_cmd(&h, &args).output().unwrap();
        assert_ne!(out.status.code(), Some(0), "args {args:?} must fail");
        assert!(
            !String::from_utf8_lossy(&out.stderr).trim().is_empty(),
            "cause named on stderr for {args:?}"
        );
    }
    fs::write(h.state.join("dispatch.json"), "{ not json").unwrap();
    let out = wait_cmd(
        &h,
        &[
            "dispatch",
            "wait",
            "--ticket",
            "68",
            "--parent",
            "plan",
            "--policy",
            "all-settled",
        ],
    )
    .output()
    .unwrap();
    assert_ne!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot parse ledger")
            || String::from_utf8_lossy(&out.stderr).contains("cannot read ledger")
    );
}
