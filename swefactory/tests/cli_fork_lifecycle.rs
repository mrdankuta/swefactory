//! Fork lifecycle composition (ticket #82, spec #65 slice).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (ledger + canned transcripts under
//! state/logs/); the working directory is a scratch git repo standing in
//! for the project. No daemon, no network, no real runs anywhere here.
//!
//! What these tests catch: a forked line resumes after interruption
//! exactly like a main line (same checkpoint words, same transcript
//! preservation, same worktree release/re-attach, same resumed counter)
//! with its transcript bounded by the existing tail rules; refusals when
//! there is nothing to resume; abandoned fork worktrees resuming cleanly;
//! landing still accepting exactly one human-chosen line per ticket (the
//! recorded attempt branch — forks never land directly); forkless tickets
//! keeping their legacy shapes with no migration.
//! What they miss: the shared facts document itself (spec #64, tickets
//! #77/#78/#79) and live multi-client streams (next spec).

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-forklifecycle-{}-{}-{}",
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

fn write_ledger(h: &Harness, value: serde_json::Value) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
}

/// One attempt row whose log_path points at a real file under state/logs/.
fn attempt_with_log(h: &Harness, ticket: u64, id: u64, no: u64, status: &str) -> serde_json::Value {
    let log_path = h
        .state
        .join("logs")
        .join(format!("T{ticket}-a{no}-impl.log"));
    serde_json::json!({
        "id": id,
        "ticket": ticket,
        "attempt_no": no,
        "base_commit": "abc123def456",
        "head_commit": "",
        "branch": format!("factory/T{ticket}-demo"),
        "worktree": format!("/tmp/wt-T{ticket}-a{no}"),
        "status": status,
        "exit_code": 0,
        "log_path": log_path.to_string_lossy(),
        "started_at": "1700000000",
        "finished_at": "1700000600",
        "train_branch": ""
    })
}

fn write_impl_log(h: &Harness, ticket: u64, no: u64, body: &str) {
    let dir = h.state.join("logs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("T{ticket}-a{no}-impl.log")), body).unwrap();
}

fn cli(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

fn read_ledger(h: &Harness) -> serde_json::Value {
    let text = fs::read_to_string(h.state.join("dispatch.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn fork_entry(h: &Harness, ticket: u64, name: &str) -> serde_json::Value {
    read_ledger(h)["forks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .unwrap_or_else(|| panic!("fork '{name}' must exist for ticket #{ticket}"))
        .clone()
}

fn worktree_list(h: &Harness) -> String {
    let out = std::process::Command::new("git")
        .args(["worktree", "list"])
        .current_dir(&h.repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stdout_text(mut cmd: Command) -> String {
    String::from_utf8_lossy(&cmd.assert().success().get_output().stdout).into_owned()
}

fn seed_fork(h: &Harness, ticket: u64, name: &str, at: u64) {
    write_ledger(
        h,
        serde_json::json!({ "attempts": [attempt_with_log(h, ticket, 1, 1, "impl_done")] }),
    );
    write_impl_log(h, ticket, 1, "entry line 1\nentry line 2\nentry line 3\n");
    cli(
        h,
        &[
            "fork",
            &ticket.to_string(),
            "--at",
            &at.to_string(),
            "--name",
            name,
        ],
    )
    .assert()
    .success();
}

#[test]
fn forked_line_resumes_after_interrupt_with_transcript_preserved() {
    let h = setup("resume-cycle");
    seed_fork(&h, 9, "alpha", 2);
    let fork_log = h.state.join("logs").join("T9-fork-alpha.log");

    // The experiment line does work: transcript grows past the prefix.
    fs::write(&fork_log, "entry line 1\nentry line 2\nwork line 3\n").unwrap();
    let wt_before = worktree_list(&h);
    assert!(
        wt_before.contains("T9-fork-alpha"),
        "live fork holds its worktree, got:\n{wt_before}"
    );

    // Interruption marks the line resumable and releases the worktree so
    // resume recreates it — the existing per-line discipline.
    cli(&h, &["fork", "9", "--interrupt", "alpha"])
        .assert()
        .success();
    let row = fork_entry(&h, 9, "alpha");
    assert_eq!(
        row["status"],
        serde_json::json!("implement_interrupted"),
        "interrupt marks the line interrupted, got:\n{row}"
    );
    let wt_mid = worktree_list(&h);
    assert!(
        !wt_mid.contains("T9-fork-alpha"),
        "interrupt releases the fork worktree, got:\n{wt_mid}"
    );

    // Resume continues the SAME line: no new fork, resumed counter bumps,
    // worktree re-attaches on the same branch, transcript preserved.
    let out = cli(&h, &["fork", "9", "--resume", "alpha"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let words = String::from_utf8_lossy(&out).into_owned();
    assert!(
        words.contains("resumed") && words.contains("alpha"),
        "resume names the continued line, got:\n{words}"
    );
    let ledger = read_ledger(&h);
    assert_eq!(
        ledger["forks"].as_array().unwrap().len(),
        1,
        "resume continues the same line, never a new one"
    );
    let row = fork_entry(&h, 9, "alpha");
    assert_eq!(row["fork_no"], serde_json::json!(1));
    assert_eq!(row["resumed"], serde_json::json!(1));
    assert_eq!(row["status"], serde_json::json!("forked"));
    let wt_after = worktree_list(&h);
    assert!(
        wt_after.contains("T9-fork-alpha"),
        "resume re-attaches the fork worktree, got:\n{wt_after}"
    );

    // Transcript: prefix + work + interrupt marker + resume marker, in
    // order — nothing rewritten, nothing lost.
    let body = fs::read_to_string(&fork_log).unwrap();
    assert_eq!(
        body,
        "entry line 1\nentry line 2\nwork line 3\ninterrupted during implement (slot T9-fork-alpha) — resumable\nresumed from implement checkpoint (slot T9-fork-alpha, resume #1) — transcript preserved\n",
        "resume preserves the transcript, got:\n{body}"
    );

    // Tail bounds hold on the resumed line: bounded by default, full on 0.
    let tail = stdout_text(cli(&h, &["log", "9", "--fork", "alpha", "--tail", "2"]));
    assert!(
        tail.contains("Transcript (last 2 lines):"),
        "bounded tail names its bound, got:\n{tail}"
    );
    assert!(
        tail.contains("resume #1"),
        "bounded tail shows the newest lines, got:\n{tail}"
    );
    assert!(
        !tail.contains("entry line 1"),
        "bounded tail drops older lines, got:\n{tail}"
    );
    let full = stdout_text(cli(&h, &["log", "9", "--fork", "alpha", "--tail", "0"]));
    assert!(
        full.contains("entry line 1") && full.contains("work line 3") && full.contains("resume #1"),
        "full transcript keeps everything, got:\n{full}"
    );

    // JSON carries the same facts: status live again, resumed counted.
    let out = stdout_text(cli(&h, &["log", "9", "--fork", "alpha", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["status"], serde_json::json!("forked"));
    assert_eq!(v["resumed"], serde_json::json!(1));
    assert!(
        v["transcript"].as_str().unwrap().contains("resume #1"),
        "json transcript matches, got:\n{v}"
    );
}

#[test]
fn fork_resume_and_interrupt_refuse_cleanly_with_nothing_to_do() {
    let h = setup("refusals");
    seed_fork(&h, 9, "alpha", 2);
    seed_fork_second(&h);
    let ledger_before = read_ledger(&h);

    // A live line has nothing to resume.
    let out = cli(&h, &["fork", "9", "--resume", "alpha"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8_lossy(&out).into_owned();
    assert!(
        err.contains("not interrupted") && err.contains("alpha"),
        "resume on a live line names the line and the reason, got:\n{err}"
    );

    // Unknown lines name the taken names — same rule as every fork read.
    let out = cli(&h, &["fork", "9", "--resume", "nope"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8_lossy(&out).into_owned();
    assert!(
        err.contains("alpha") && err.contains("beta"),
        "unknown resume names the taken names, got:\n{err}"
    );

    // A second interrupt on an interrupted line refuses instead of stacking.
    cli(&h, &["fork", "9", "--interrupt", "alpha"])
        .assert()
        .success();
    cli(&h, &["fork", "9", "--interrupt", "alpha"])
        .assert()
        .failure();
    let row = fork_entry(&h, 9, "alpha");
    assert_eq!(row["status"], serde_json::json!("implement_interrupted"));

    // Creation flags and lifecycle flags never mix on one invocation.
    cli(
        &h,
        &["fork", "9", "--resume", "alpha", "--at", "1", "--name", "x"],
    )
    .assert()
    .failure();
    cli(&h, &["fork", "9", "--interrupt", "alpha", "--name", "x"])
        .assert()
        .failure();

    // Refusals admit no new lines and hijack none: still two forks, and
    // the untouched line stays byte-identical.
    let ledger_after = read_ledger(&h);
    assert_eq!(
        ledger_after["forks"].as_array().unwrap().len(),
        ledger_before["forks"].as_array().unwrap().len(),
        "refusals admit no new lines"
    );
    // Only the one admitted interrupt may move: alpha reads interrupted
    // afterwards, everything else identical.
    let mut expected = ledger_before["forks"].clone();
    for f in expected.as_array_mut().unwrap() {
        if f["name"] == "alpha" {
            f["status"] = serde_json::json!("implement_interrupted");
        }
    }
    assert_eq!(
        ledger_after["forks"], expected,
        "refusals change nothing except the one admitted interrupt"
    );
    assert!(
        fs::read_to_string(h.state.join("logs").join("T9-fork-beta.log")).unwrap()
            == "entry line 1\nentry line 2\n",
        "untouched lines stay byte-identical"
    );
}

fn seed_fork_second(h: &Harness) {
    cli(h, &["fork", "9", "--at", "2", "--name", "beta"])
        .assert()
        .success();
}

#[test]
fn abandoned_fork_worktree_resumes_cleanly_with_transcript_intact() {
    let h = setup("abandoned");
    seed_fork(&h, 9, "alpha", 2);
    let fork_log = h.state.join("logs").join("T9-fork-alpha.log");
    fs::write(&fork_log, "entry line 1\nentry line 2\nabandoned work\n").unwrap();

    // The human deleted the worktree out-of-band (abandoned experiment):
    // a stale worktree entry may linger; the discipline reaps it.
    let wt = h.state.join("worktrees").join("T9-fork-alpha");
    assert!(wt.is_dir(), "fork worktree starts attached");
    let _ = std::fs::remove_dir_all(&wt);
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(&h.repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output();
    assert!(
        !worktree_list(&h).contains("T9-fork-alpha"),
        "abandoned worktree is gone from the list"
    );

    // Reads never break on the missing worktree: transcript comes from
    // the log file, which abandonment never touches.
    let shown = stdout_text(cli(&h, &["log", "9", "--fork", "alpha", "--tail", "0"]));
    assert!(
        shown.contains("abandoned work"),
        "abandoned line still reads its transcript, got:\n{shown}"
    );

    // Interrupt + resume recover the line on its recorded branch with the
    // transcript intact — abandonment costs a re-attach, never the line.
    cli(&h, &["fork", "9", "--interrupt", "alpha"])
        .assert()
        .success();
    cli(&h, &["fork", "9", "--resume", "alpha"])
        .assert()
        .success();
    assert!(
        worktree_list(&h).contains("T9-fork-alpha"),
        "resume re-attaches the abandoned line"
    );
    let body = fs::read_to_string(&fork_log).unwrap();
    assert!(
        body.starts_with("entry line 1\nentry line 2\nabandoned work\n"),
        "abandoned transcript preserved under resume, got:\n{body}"
    );
    assert!(
        body.contains("resume #1"),
        "resume marker appended, got:\n{body}"
    );
}

#[test]
fn release_merge_lands_exactly_the_attempt_line_when_forks_exist() {
    let h = setup("land-one-line");
    // Attempt branch with a real commit so the train merge is clean.
    git_run(&h.repo, &["checkout", "-qb", "factory/T9-demo"]);
    fs::write(h.repo.join("demo.txt"), "demo\n").unwrap();
    git_run(&h.repo, &["add", "-A"]);
    git_run(&h.repo, &["commit", "-qm", "feat(T9): demo"]);
    git_run(&h.repo, &["checkout", "-q", "dev"]);
    // Fork branches with divergent commits: experiments, never landing.
    for name in ["alpha", "beta"] {
        git_run(
            &h.repo,
            &["checkout", "-qb", &format!("factory/T9-fork-{name}")],
        );
        fs::write(h.repo.join(format!("{name}.txt")), "experiment\n").unwrap();
        git_run(&h.repo, &["add", "-A"]);
        git_run(&h.repo, &["commit", "-qm", &format!("experiment {name}")]);
        git_run(&h.repo, &["checkout", "-q", "dev"]);
    }
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "review_done")] }),
    );
    write_impl_log(&h, 9, 1, "entry line 1\nentry line 2\n");
    cli(&h, &["fork", "9", "--at", "1", "--name", "alpha"])
        .assert()
        .success();
    cli(&h, &["fork", "9", "--at", "1", "--name", "beta"])
        .assert()
        .success();

    cli(&h, &["release", "start", "proof"]).assert().success();
    let train = train_branch(&h);
    cli(&h, &["release", "merge", "9"]).assert().success();

    // Exactly one line lands: the recorded attempt branch is in the train,
    // no fork branch is.
    assert!(
        is_ancestor(&h, "factory/T9-demo", &train),
        "the human-chosen attempt line merges into the train"
    );
    for name in ["alpha", "beta"] {
        assert!(
            !is_ancestor(&h, &format!("factory/T9-fork-{name}"), &train),
            "fork '{name}' never lands through the train"
        );
    }
}

#[test]
fn release_merge_refuses_a_ticket_with_only_forks() {
    // Forks never land directly: with no recorded attempt branch there is
    // nothing to merge, and the refusal says so.
    let h = setup("forks-never-land");
    write_ledger(
        &h,
        serde_json::json!({
            "attempts": [],
            "forks": [{
                "id": 1, "ticket": 9, "fork_no": 1, "name": "alpha",
                "parent_attempt": 1, "parent_entry": 1, "parent_fork": "",
                "lineage": [], "slot": "T9-fork-alpha",
                "branch": "factory/T9-fork-alpha",
                "worktree": "/tmp/wt-T9-fork-alpha",
                "log_path": "/tmp/T9-fork-alpha.log",
                "base_commit": "abc123", "started_at": "1700000000",
            }],
        }),
    );
    cli(&h, &["release", "start", "proof"]).assert().success();
    let out = cli(&h, &["release", "merge", "9"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8_lossy(&out).into_owned();
    assert!(
        err.contains("no recorded branch"),
        "fork-only ticket refuses with no recorded branch, got:\n{err}"
    );
}

fn train_branch(h: &Harness) -> String {
    let text = fs::read_to_string(h.state.join("train.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["branch"].as_str().unwrap().to_string()
}

fn is_ancestor(h: &Harness, maybe_ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", maybe_ancestor, descendant])
        .current_dir(&h.repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap()
        .success()
}

#[test]
fn forkless_tickets_keep_legacy_shapes_with_no_migration() {
    // Tickets that never forked behave exactly as before this chain: no
    // forks key on log JSON, an empty forks list on status JSON, and a
    // ledger without any forks field parses and renders.
    let h = setup("forkless");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "line 1\nline 2\n");

    let out = stdout_text(cli(&h, &["log", "9", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        v.get("forks").is_none(),
        "forkless log JSON carries no forks key, got:\n{v}"
    );

    let out = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["forks"],
        serde_json::json!([]),
        "forkless status JSON carries an empty forks list, got:\n{v}"
    );
}

#[test]
fn seeded_fork_rows_without_lifecycle_fields_render_as_live() {
    // Ledgers written before this ticket carry fork rows with no status
    // or resumed fields: they parse (no migration) and render as live
    // lines with no resume history — byte-identical summaries.
    let h = setup("legacy-rows");
    write_ledger(
        &h,
        serde_json::json!({
            "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")],
            "forks": [{
                "id": 2, "ticket": 9, "fork_no": 1, "name": "alpha",
                "parent_attempt": 1, "parent_entry": 2, "parent_fork": "",
                "lineage": [{"ticket": 9, "kind": "attempt", "no": 1, "entry": 2, "name": ""}],
                "slot": "T9-fork-alpha",
                "branch": "factory/T9-fork-alpha",
                "worktree": "/tmp/wt-T9-fork-alpha",
                "log_path": h.state.join("logs").join("T9-fork-alpha.log").to_string_lossy(),
                "base_commit": "abc123", "started_at": "1700000000",
            }],
        }),
    );
    write_impl_log(&h, 9, 1, "entry line 1\nentry line 2\nentry line 3\n");
    fs::write(
        h.state.join("logs").join("T9-fork-alpha.log"),
        "entry line 1\nentry line 2\n",
    )
    .unwrap();

    let human = stdout_text(cli(&h, &["status"]));
    let line = human
        .lines()
        .find(|l| l.contains("alpha"))
        .expect("status shows the legacy fork");
    assert!(
        !line.contains("Resumed"),
        "never-resumed lines carry no resume history, got:\n{line}"
    );
    let out = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let row = v["forks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "alpha")
        .unwrap()
        .clone();
    assert_eq!(row["status"], serde_json::json!("forked"));
    assert_eq!(row["resumed"], serde_json::json!(0));
    assert_eq!(
        row["summary"].as_str().unwrap_or(""),
        line.trim(),
        "legacy row renders the identical summary in both modes"
    );
}
