//! Decision memos for hooks/approvals (ticket #67, spec #59 follow-up).
//!
//! Seam: argv in -> exit code + stdout out + state-dir files out, over a
//! fixture ledger plus a scratch repo. No daemon, no network, no real runs.
//! Tests assert only invoked-arguments to exit code plus output plus
//! observable state — never store internals, function shapes, or module
//! layout.
//!
//! What these tests catch: a decision recorded before a simulated kill is
//! found (not re-asked) after resume through `dispatch status` and `log`
//! (human and --json carrying identical facts); concurrent first-writes
//! resolving to exactly one winner with losers observing the stored value;
//! memo scope pinned to the attempt (a fresh attempt starts empty); and
//! byte+mtime proof that status/log views change nothing.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-memos-{}-{}-{}",
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
    serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap()
}

/// One interrupted attempt (killed mid-implement, resumable), no memos yet.
fn interrupted_ledger() -> serde_json::Value {
    serde_json::json!({
        "attempts": [{
            "id": 1, "ticket": 67, "attempt_no": 1,
            "base_commit": "abc123def456", "head_commit": "",
            "branch": "factory/T67-demo", "worktree": "/tmp/wt-T67-a1",
            "status": "implement_interrupted", "exit_code": 0,
            "log_path": "",
            "started_at": "1700000000", "finished_at": "",
            "train_branch": "", "phase": "implement", "resumed": 1
        }]
    })
}

fn memo_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

/// (relative path, bytes, mtime) for every file under the state dir.
fn snapshot(state: &Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![state.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
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

#[test]
fn memo_recorded_before_kill_is_reused_after_resume_through_status_and_log() {
    let h = setup("kill-resume");
    write_ledger(&h, &interrupted_ledger());

    // First decision: the hook asks once and the answer is stored.
    memo_cmd(
        &h,
        &[
            "dispatch",
            "memo",
            "--ticket",
            "67",
            "--attempt",
            "1",
            "--key",
            "approve-deploy",
            "--decision",
            "allow",
        ],
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("recorded"))
    .stdout(predicates::str::contains("approve-deploy=allow"));

    // Simulated kill mid-write (tmp written, rename never happens): the
    // ledger keeps the earlier decision, parseable and unchanged.
    memo_cmd(
        &h,
        &[
            "dispatch",
            "memo",
            "--ticket",
            "67",
            "--attempt",
            "1",
            "--key",
            "allow-flagged-tool",
            "--decision",
            "deny",
        ],
    )
    .env("FACTORY_MEMO_CRASH", "1")
    .assert()
    .failure();
    let ledger = read_ledger(&h);
    let memos = ledger
        .get("decision_memos")
        .and_then(|m| m.as_array())
        .expect("ledger must carry decision_memos");
    assert_eq!(memos.len(), 1, "crash must not tear the ledger:\n{ledger}");
    assert_eq!(
        memos[0].get("key").and_then(|k| k.as_str()),
        Some("approve-deploy")
    );

    // After resume the hook finds the stored answer instead of re-prompting:
    // a repeat carries a different decision but observes the stored one.
    memo_cmd(
        &h,
        &[
            "dispatch",
            "memo",
            "--ticket",
            "67",
            "--attempt",
            "1",
            "--key",
            "approve-deploy",
            "--decision",
            "deny",
        ],
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("reused"))
    .stdout(predicates::str::contains("approve-deploy=allow"));

    // End to end through dispatch status (human): interrupted + resumed +
    // the stored decision on one screen.
    memo_cmd(&h, &["dispatch", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("interrupted"))
        .stdout(predicates::str::contains("resumed=1"))
        .stdout(predicates::str::contains("approve-deploy=allow"));

    // Same facts in machine output.
    let status_out = memo_cmd(&h, &["--json", "dispatch", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&status_out).unwrap();
    let memos = parsed
        .get("decision_memos")
        .and_then(|m| m.as_array())
        .expect("status JSON must carry decision_memos");
    assert_eq!(memos.len(), 1, "status JSON:\n{parsed}");
    assert_eq!(memos[0].get("ticket").and_then(|t| t.as_u64()), Some(67));
    assert_eq!(memos[0].get("attempt").and_then(|a| a.as_u64()), Some(1));
    assert_eq!(
        memos[0].get("key").and_then(|k| k.as_str()),
        Some("approve-deploy")
    );
    assert_eq!(
        memos[0].get("decision").and_then(|d| d.as_str()),
        Some("allow")
    );

    // End to end through log (human): the attempt story carries the resume
    // count and the decision.
    memo_cmd(&h, &["log", "67"])
        .assert()
        .success()
        .stdout(predicates::str::contains("interrupted"))
        .stdout(predicates::str::contains(
            "Resumed from checkpoint 1 time(s).",
        ))
        .stdout(predicates::str::contains("approve-deploy=allow"));

    // Same facts in machine output.
    let log_out = memo_cmd(&h, &["--json", "log", "67"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&log_out).unwrap();
    let rows = parsed
        .get("attempts")
        .and_then(|a| a.as_array())
        .expect("log JSON must carry attempts");
    assert_eq!(rows.len(), 1, "log JSON:\n{parsed}");
    let row_memos = rows[0]
        .get("memos")
        .and_then(|m| m.as_array())
        .expect("log JSON rows must carry memos");
    assert_eq!(row_memos.len(), 1, "log JSON:\n{parsed}");
    assert_eq!(
        row_memos[0].get("decision").and_then(|d| d.as_str()),
        Some("allow")
    );
}

#[test]
fn memo_concurrent_first_writes_resolve_to_exactly_one_winner() {
    let h = setup("concurrent");
    write_ledger(&h, &interrupted_ledger());

    // Eight hooks ask at once with distinct answers; exactly one wins and
    // every loser observes the stored value.
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let repo = h.repo.clone();
            let state = h.state.clone();
            std::thread::spawn(move || {
                let mut cmd = Command::cargo_bin("factory").unwrap();
                cmd.args([
                    "dispatch",
                    "memo",
                    "--ticket",
                    "67",
                    "--attempt",
                    "1",
                    "--key",
                    "guard-answer",
                    "--decision",
                    &format!("answer-{i}"),
                ])
                .current_dir(&repo);
                cmd.env("FACTORY_STATE_DIR", &state)
                    .env_remove("FACTORY_SLOT");
                let out = cmd.output().unwrap();
                assert!(out.status.success(), "memo record must exit 0");
                String::from_utf8(out.stdout).unwrap()
            })
        })
        .collect();
    let outputs: Vec<String> = handles
        .into_iter()
        .map(|t| t.join().expect("memo thread panicked"))
        .collect();

    let recorded = outputs.iter().filter(|o| o.contains("recorded")).count();
    assert_eq!(recorded, 1, "exactly one winner:\n{outputs:?}");
    let reused = outputs.iter().filter(|o| o.contains("reused")).count();
    assert_eq!(reused, 7, "losers observe the stored value:\n{outputs:?}");

    // Every line names the same stored answer.
    let stored: Vec<String> = outputs
        .iter()
        .map(|o| {
            (0..8)
                .map(|i| format!("answer-{i}"))
                .find(|a| o.contains(&format!("guard-answer={a}")))
                .expect("each output must name the stored decision")
        })
        .collect();
    assert!(
        stored.windows(2).all(|w| w[0] == w[1]),
        "all hooks observe one value:\n{outputs:?}"
    );

    let ledger = read_ledger(&h);
    let memos = ledger
        .get("decision_memos")
        .and_then(|m| m.as_array())
        .expect("ledger must carry decision_memos");
    assert_eq!(memos.len(), 1, "one row wins:\n{ledger}");
    assert_eq!(
        memos[0].get("decision").and_then(|d| d.as_str()),
        Some(stored[0].split('=').next_back().unwrap())
    );
}

#[test]
fn memo_scope_is_the_attempt_and_fresh_attempts_start_empty() {
    let h = setup("scope");
    let mut ledger = interrupted_ledger();
    ledger["attempts"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": 2, "ticket": 67, "attempt_no": 2,
            "base_commit": "abc123def456", "head_commit": "",
            "branch": "factory/T67-demo", "worktree": "/tmp/wt-T67-a2",
            "status": "impl_running", "exit_code": 0,
            "log_path": "",
            "started_at": "1700000001", "finished_at": "",
            "train_branch": "", "phase": "implement", "resumed": 0
        }));
    write_ledger(&h, &ledger);

    // A decision for attempt 1 is stored.
    memo_cmd(
        &h,
        &[
            "dispatch",
            "memo",
            "--ticket",
            "67",
            "--attempt",
            "1",
            "--key",
            "approve-deploy",
            "--decision",
            "allow",
        ],
    )
    .assert()
    .success();

    // The fresh attempt starts with an empty cache: its log row carries no
    // memos even though the same key exists for attempt 1.
    let log_out = memo_cmd(&h, &["--json", "log", "67"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&log_out).unwrap();
    let rows = parsed.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(rows.len(), 2, "log JSON:\n{parsed}");
    // Newest first: attempt 2 has no memos, attempt 1 has one.
    assert_eq!(rows[0].get("attempt").and_then(|a| a.as_u64()), Some(2));
    assert!(
        rows[0]
            .get("memos")
            .and_then(|m| m.as_array())
            .unwrap()
            .is_empty(),
        "fresh attempt starts empty:\n{parsed}"
    );
    assert_eq!(
        rows[1]
            .get("memos")
            .and_then(|m| m.as_array())
            .unwrap()
            .len(),
        1,
        "attempt 1 keeps its memo:\n{parsed}"
    );

    // The same key records independently for attempt 2 (a miss, not a reuse).
    memo_cmd(
        &h,
        &[
            "dispatch",
            "memo",
            "--ticket",
            "67",
            "--attempt",
            "2",
            "--key",
            "approve-deploy",
            "--decision",
            "deny",
        ],
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("recorded"))
    .stdout(predicates::str::contains("approve-deploy=deny"));

    let ledger = read_ledger(&h);
    let memos = ledger
        .get("decision_memos")
        .and_then(|m| m.as_array())
        .unwrap();
    assert_eq!(memos.len(), 2, "per-attempt rows:\n{ledger}");
}

#[test]
fn memo_status_and_log_views_change_nothing() {
    let h = setup("readonly");
    write_ledger(&h, &interrupted_ledger());
    memo_cmd(
        &h,
        &[
            "dispatch",
            "memo",
            "--ticket",
            "67",
            "--attempt",
            "1",
            "--key",
            "approve-deploy",
            "--decision",
            "allow",
        ],
    )
    .assert()
    .success();

    let before = snapshot(&h.state);
    for args in [
        vec!["dispatch", "status"],
        vec!["--json", "dispatch", "status"],
        vec!["log", "67"],
        vec!["--json", "log", "67"],
    ] {
        memo_cmd(&h, &args).assert().success();
    }
    let after = snapshot(&h.state);
    assert_eq!(
        before.len(),
        after.len(),
        "views must not add or drop state files"
    );
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a, "state file set must be unchanged");
        assert_eq!(bytes_b, bytes_a, "viewing must not rewrite {rel_b}");
        assert_eq!(mtime_b, mtime_a, "viewing must not touch {rel_b}");
    }
}

#[test]
fn memo_rejects_bad_input_with_named_cause() {
    let h = setup("bad-input");
    write_ledger(&h, &interrupted_ledger());

    // Unknown attempt: the scope must exist before a hook can memoize it.
    memo_cmd(
        &h,
        &[
            "dispatch",
            "memo",
            "--ticket",
            "67",
            "--attempt",
            "9",
            "--key",
            "k",
            "--decision",
            "allow",
        ],
    )
    .assert()
    .failure()
    .stderr(predicates::str::contains("no such attempt"));

    // Empty key / decision name their cause.
    memo_cmd(
        &h,
        &[
            "dispatch",
            "memo",
            "--ticket",
            "67",
            "--attempt",
            "1",
            "--key",
            "  ",
            "--decision",
            "allow",
        ],
    )
    .assert()
    .failure()
    .stderr(predicates::str::contains("key"));
    memo_cmd(
        &h,
        &[
            "dispatch",
            "memo",
            "--ticket",
            "67",
            "--attempt",
            "1",
            "--key",
            "k",
            "--decision",
            "",
        ],
    )
    .assert()
    .failure()
    .stderr(predicates::str::contains("decision"));

    // Bad input writes nothing.
    let ledger = read_ledger(&h);
    assert!(
        ledger
            .get("decision_memos")
            .and_then(|m| m.as_array())
            .map(|m| m.is_empty())
            .unwrap_or(true),
        "rejected writes must store nothing:\n{ledger}"
    );
}
