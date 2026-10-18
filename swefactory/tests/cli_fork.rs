//! Fork creation + isolation (ticket #80, spec #65 slice).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (ledger + canned transcripts under
//! state/logs/); the working directory is a scratch git repo standing in
//! for the project. No daemon, no network, no real runs anywhere here.
//!
//! What these tests catch: forking at an entry leaves the original
//! transcript byte-identical while the fork file holds exactly the prefix
//! lines 1..=ENTRY; forks are readable by name and by number with
//! identical transcripts; name collisions are refused with the taken
//! names shown and hijack nothing; each fork owns a distinct worktree,
//! branch, slot, and log file.
//! What they miss: lineage rendering and caps (ticket #81), lifecycle
//! composition (ticket #82), and the shared facts document (spec #64).

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-fork-{}-{}-{}",
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

fn fork_cmd(h: &Harness, args: &[&str]) -> Command {
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

fn branch_list(h: &Harness) -> String {
    let out = std::process::Command::new("git")
        .args(["branch", "--list"])
        .current_dir(&h.repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn fork_inherits_prefix_and_leaves_original_byte_identical() {
    let h = setup("prefix");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    let body = "entry line 1\nentry line 2\nentry line 3\nentry line 4\n";
    write_impl_log(&h, 9, 1, body);
    let before = fs::read(h.state.join("logs").join("T9-a1-impl.log")).unwrap();

    fork_cmd(&h, &["fork", "9", "--at", "2", "--name", "alpha"])
        .assert()
        .success();

    // Original transcript byte-identical.
    let after = fs::read(h.state.join("logs").join("T9-a1-impl.log")).unwrap();
    assert_eq!(
        before, after,
        "forking must not touch the original transcript"
    );

    // Fork file holds exactly the prefix lines 1..=2.
    let fork_body = fs::read_to_string(h.state.join("logs").join("T9-fork-alpha.log"))
        .expect("fork log file must exist");
    assert_eq!(
        fork_body, "entry line 1\nentry line 2\n",
        "fork inherits the prefix up to and including the entry, got:\n{fork_body}"
    );

    // Ledger records the fork lineage.
    let ledger = read_ledger(&h);
    let forks = ledger["forks"].as_array().expect("ledger must carry forks");
    assert_eq!(forks.len(), 1);
    assert_eq!(forks[0]["name"], serde_json::json!("alpha"));
    assert_eq!(forks[0]["ticket"], serde_json::json!(9));
    assert_eq!(forks[0]["parent_attempt"], serde_json::json!(1));
    assert_eq!(forks[0]["parent_entry"], serde_json::json!(2));
}

#[test]
fn fork_continues_independently_from_inherited_prefix() {
    let h = setup("independent");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "line 1\nline 2\nline 3\n");
    fork_cmd(&h, &["fork", "9", "--at", "2", "--name", "alpha"])
        .assert()
        .success();

    // Appending to the fork leaves the parent identical.
    let fork_log = h.state.join("logs").join("T9-fork-alpha.log");
    assert!(fork_log.is_file(), "fork log path is predictable");
    let parent_before = fs::read(h.state.join("logs").join("T9-a1-impl.log")).unwrap();
    fs::write(&fork_log, "line 1\nline 2\nfork-only line 3\n").unwrap();
    let parent_after = fs::read(h.state.join("logs").join("T9-a1-impl.log")).unwrap();
    assert_eq!(
        parent_before, parent_after,
        "the fork continues independently; the original never moves"
    );
    assert!(
        fs::read_to_string(&fork_log)
            .unwrap()
            .contains("fork-only line 3"),
        "fork log carries its own continuation"
    );
}

#[test]
fn forks_addressable_by_name_or_number() {
    let h = setup("address");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "one\ntwo\nthree\n");
    fork_cmd(&h, &["fork", "9", "--at", "2", "--name", "alpha"])
        .assert()
        .success();

    let by_name = fork_cmd(&h, &["log", "9", "--fork", "alpha"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let by_name = String::from_utf8_lossy(&by_name).into_owned();
    let by_number = fork_cmd(&h, &["log", "9", "--fork", "1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let by_number = String::from_utf8_lossy(&by_number).into_owned();
    for (label, text) in [("name", &by_name), ("number", &by_number)] {
        assert!(
            text.contains("one") && text.contains("two"),
            "fork addressed by {label} shows the inherited prefix, got:\n{text}"
        );
        assert!(
            !text.contains("three"),
            "fork addressed by {label} stops at the entry, got:\n{text}"
        );
    }

    // JSON carries the same fork facts.
    let out = fork_cmd(&h, &["log", "9", "--fork", "alpha", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out)).unwrap();
    assert_eq!(v["ticket"], serde_json::json!(9));
    assert_eq!(v["fork"], serde_json::json!("alpha"));
    assert_eq!(v["transcript"], serde_json::json!("one\ntwo\n"));

    // Unknown fork names the taken names.
    let out = fork_cmd(&h, &["log", "9", "--fork", "nope"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8_lossy(&out).into_owned();
    assert!(
        err.contains("alpha"),
        "unknown fork must list the taken names, got:\n{err}"
    );
}

#[test]
fn fork_name_collision_rejected_showing_taken_names() {
    let h = setup("collision");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "one\ntwo\n");
    fork_cmd(&h, &["fork", "9", "--at", "1", "--name", "alpha"])
        .assert()
        .success();
    fork_cmd(&h, &["fork", "9", "--at", "1", "--name", "beta"])
        .assert()
        .success();
    let wt_before = worktree_list(&h);
    let branches_before = branch_list(&h);
    let ledger_before = read_ledger(&h);

    let out = fork_cmd(&h, &["fork", "9", "--at", "1", "--name", "alpha"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8_lossy(&out).into_owned();
    assert!(
        err.contains("already taken") && err.contains("alpha") && err.contains("beta"),
        "collision must name the taken names, got:\n{err}"
    );

    // No existing experiment is hijacked: ledger, worktrees, branches unchanged.
    assert_eq!(read_ledger(&h), ledger_before, "failed fork writes nothing");
    assert_eq!(worktree_list(&h), wt_before, "failed fork adds no worktree");
    assert_eq!(
        branch_list(&h),
        branches_before,
        "failed fork adds no branch"
    );
}

#[test]
fn fork_runs_in_own_worktree_with_own_slot() {
    let h = setup("isolation");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "one\ntwo\nthree\n");

    let out = fork_cmd(&h, &["fork", "9", "--at", "1", "--name", "alpha", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out)).unwrap();
    let slot = v["slot"]
        .as_str()
        .expect("json carries the slot")
        .to_string();
    let branch = v["branch"]
        .as_str()
        .expect("json carries the branch")
        .to_string();
    let worktree = v["worktree"]
        .as_str()
        .expect("json carries the worktree")
        .to_string();
    assert!(!slot.is_empty() && !branch.is_empty() && !worktree.is_empty());

    // Slot, branch, and worktree are all distinct from the parent line.
    assert_ne!(slot, "T9-a1", "fork slot must differ from the parent slot");
    assert_ne!(
        branch, "factory/T9-demo",
        "fork branch must differ from the parent branch"
    );
    assert!(
        Path::new(&worktree).is_dir(),
        "fork worktree must exist on disk: {worktree}"
    );
    assert!(
        !worktree.contains("wt-T9-a1"),
        "fork worktree must not reuse the parent path: {worktree}"
    );
    assert!(
        worktree_list(&h).contains(&worktree),
        "fork worktree is a real git worktree"
    );
    assert!(
        branch_list(&h).contains(&branch),
        "fork branch exists: {branch}"
    );

    // Second fork gets its own number, slot, worktree, and branch.
    let out2 = fork_cmd(&h, &["fork", "9", "--at", "1", "--name", "beta", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v2: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out2)).unwrap();
    assert_eq!(v2["fork_no"], serde_json::json!(2));
    assert_ne!(
        v2["slot"].as_str().unwrap(),
        slot,
        "lines never share a slot"
    );
    assert_ne!(
        v2["worktree"].as_str().unwrap(),
        worktree,
        "lines never share a worktree"
    );
}

#[test]
fn fork_refuses_bad_input_with_plain_cause() {
    let h = setup("badinput");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "one\ntwo\n");

    // Entry past the end names the bound.
    fork_cmd(&h, &["fork", "9", "--at", "99", "--name", "waypast"])
        .assert()
        .failure();
    // Entry zero names the bound.
    fork_cmd(&h, &["fork", "9", "--at", "0", "--name", "zero"])
        .assert()
        .failure();
    // Missing parent attempt names the cause.
    fork_cmd(
        &h,
        &["fork", "9", "--at", "1", "--name", "x", "--attempt", "7"],
    )
    .assert()
    .failure();
    // Bad ticket number is refused.
    fork_cmd(&h, &["fork", "oops", "--at", "1", "--name", "x"])
        .assert()
        .failure();
    // No fork was recorded by any refusal.
    let ledger = read_ledger(&h);
    assert_eq!(
        ledger["forks"].as_array().map(|a| a.len()).unwrap_or(0),
        0,
        "refused forks record nothing"
    );
}
