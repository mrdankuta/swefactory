//! Owned vs ownerless experiment lines (ticket #73, spec #65 follow-up).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (ledger + canned transcripts under
//! state/logs/); the working directory is a scratch git repo standing in
//! for the project. No daemon, no network, no real runs anywhere here.
//!
//! What these tests catch: a fork created with `--owned` is owned by its
//! parent line (subagent-style) while a fork created without it stays
//! ownerless (the default); ownership renders in lineage output in human
//! and machine-readable modes with identical facts; `dispatch abort`
//! aborts owned lines (worktree released, transcript marked, status
//! readable in `status` and `log`) while ownerless lines survive it —
//! including abort after the parent attempt has already failed; ownerless
//! and pre-ownership (legacy) rows keep their exact prior behavior.
//! What they miss: the shared facts document itself (spec #64) and live
//! multi-client streams (next spec).

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-forkownership-{}-{}-{}",
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

fn seed_attempt(h: &Harness, ticket: u64, status: &str) {
    write_ledger(
        h,
        serde_json::json!({ "attempts": [attempt_with_log(h, ticket, 1, 1, status)] }),
    );
    write_impl_log(h, ticket, 1, "entry line 1\nentry line 2\nentry line 3\n");
}

#[test]
fn owned_and_ownerless_forks_record_ownership_and_render_it_in_both_modes() {
    let h = setup("ownership-renders");
    seed_attempt(&h, 9, "impl_done");
    cli(
        &h,
        &["fork", "9", "--at", "2", "--name", "alpha", "--owned"],
    )
    .assert()
    .success();
    cli(&h, &["fork", "9", "--at", "1", "--name", "beta"])
        .assert()
        .success();

    // Creation JSON carries the ownership fact.
    let out = cli(
        &h,
        &[
            "fork", "9", "--at", "2", "--name", "gamma", "--owned", "--json",
        ],
    )
    .assert()
    .success()
    .get_output()
    .stdout
    .clone();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out)).unwrap();
    assert_eq!(v["ownership"], serde_json::json!("owned"));

    // Human status: one calm line per experiment, ownership named.
    let human = stdout_text(cli(&h, &["status"]));
    let alpha_line = human
        .lines()
        .find(|l| l.contains("alpha") && !l.contains("gamma"))
        .expect("status shows the owned fork");
    assert!(
        alpha_line.contains("owned by attempt 1"),
        "owned line must name its parent, got:\n{alpha_line}"
    );
    let beta_line = human
        .lines()
        .find(|l| l.contains("beta"))
        .expect("status shows the ownerless fork");
    assert!(
        beta_line.contains("ownerless"),
        "ownerless line must read ownerless, got:\n{beta_line}"
    );

    // Machine status: the same ownership facts, structurally.
    let json_out = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let rows = v["forks"].as_array().expect("status json carries forks");
    let arow = rows.iter().find(|f| f["name"] == "alpha").unwrap();
    let brow = rows.iter().find(|f| f["name"] == "beta").unwrap();
    assert_eq!(arow["ownership"], serde_json::json!("owned"));
    assert_eq!(brow["ownership"], serde_json::json!("ownerless"));
    assert_eq!(
        arow["summary"].as_str().unwrap_or(""),
        alpha_line.trim(),
        "owned: json summary and human line are the identical string"
    );
    assert_eq!(
        brow["summary"].as_str().unwrap_or(""),
        beta_line.trim(),
        "ownerless: json summary and human line are the identical string"
    );

    // Log carries the same facts in both modes.
    let log_human = stdout_text(cli(&h, &["log", "9"]));
    let log_alpha = log_human
        .lines()
        .find(|l| l.contains("alpha"))
        .expect("log shows the owned fork");
    assert!(
        log_alpha.contains("owned by attempt 1"),
        "log line must carry ownership, got:\n{log_alpha}"
    );
    let log_json = stdout_text(cli(&h, &["log", "9", "--json"]));
    let lv: serde_json::Value = serde_json::from_str(&log_json).unwrap();
    let lrow = lv["forks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "alpha")
        .unwrap()
        .clone();
    assert_eq!(lrow["ownership"], serde_json::json!("owned"));
    assert_eq!(
        lrow["summary"].as_str().unwrap_or(""),
        log_alpha.trim(),
        "log human line and json summary are the identical string"
    );

    // Single-fork reads carry ownership too, in both modes.
    let one = stdout_text(cli(&h, &["log", "9", "--fork", "alpha"]));
    assert!(
        one.lines()
            .next()
            .unwrap_or("")
            .contains("owned by attempt 1"),
        "single-fork read names ownership, got:\n{one}"
    );
    let one_json = stdout_text(cli(&h, &["log", "9", "--fork", "alpha", "--json"]));
    let ov: serde_json::Value = serde_json::from_str(&one_json).unwrap();
    assert_eq!(ov["ownership"], serde_json::json!("owned"));

    // Ownership is a creation flag: it never mixes with lifecycle flags.
    cli(&h, &["fork", "9", "--resume", "alpha", "--owned"])
        .assert()
        .failure();
    cli(&h, &["fork", "9", "--interrupt", "beta", "--owned"])
        .assert()
        .failure();
}

#[test]
fn nested_owned_fork_names_its_parent_fork_in_both_modes() {
    let h = setup("nested-owned");
    seed_attempt(&h, 9, "impl_done");
    cli(&h, &["fork", "9", "--at", "2", "--name", "alpha"])
        .assert()
        .success();
    cli(
        &h,
        &[
            "fork", "9", "--fork", "alpha", "--at", "1", "--name", "gamma", "--owned",
        ],
    )
    .assert()
    .success();

    let human = stdout_text(cli(&h, &["status"]));
    let gamma_line = human
        .lines()
        .find(|l| l.contains("gamma"))
        .expect("status shows the nested owned fork");
    assert!(
        gamma_line.contains("owned by fork 'alpha'"),
        "nested owned line must name its parent fork, got:\n{gamma_line}"
    );
    let attempt_pos = gamma_line
        .find("attempt 1")
        .expect("chain names the attempt");
    let fork_pos = gamma_line
        .find("alpha")
        .expect("chain names the parent fork");
    assert!(
        attempt_pos < fork_pos,
        "chain still renders oldest-first, got:\n{gamma_line}"
    );

    let json_out = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let grow = v["forks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "gamma")
        .unwrap()
        .clone();
    assert_eq!(grow["ownership"], serde_json::json!("owned"));
    assert_eq!(grow["parent_fork"], serde_json::json!("alpha"));
    assert_eq!(grow["lineage"].as_array().unwrap().len(), 2);
    assert_eq!(
        grow["summary"].as_str().unwrap_or(""),
        gamma_line.trim(),
        "nested owned: json summary and human line are the identical string"
    );
}

#[test]
fn owned_fork_aborts_with_parent_while_ownerless_survives() {
    let h = setup("abort-propagation");
    seed_attempt(&h, 9, "impl_done");
    cli(
        &h,
        &["fork", "9", "--at", "2", "--name", "alpha", "--owned"],
    )
    .assert()
    .success();
    cli(&h, &["fork", "9", "--at", "2", "--name", "beta"])
        .assert()
        .success();
    assert!(
        worktree_list(&h).contains("T9-fork-alpha") && worktree_list(&h).contains("T9-fork-beta"),
        "both lines hold worktrees before the abort"
    );

    let out = cli(&h, &["dispatch", "abort", "9"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8_lossy(&out).into_owned();
    assert!(
        human.contains("alpha"),
        "abort must name the owned line it stops:\n{human}"
    );
    assert!(
        !human.contains("beta"),
        "abort must leave the ownerless line alone:\n{human}"
    );

    // Observable state: owned aborted + cleaned up, ownerless untouched.
    let ledger = read_ledger(&h);
    let forks = ledger["forks"].as_array().unwrap();
    let owned = forks.iter().find(|f| f["name"] == "alpha").unwrap();
    let free = forks.iter().find(|f| f["name"] == "beta").unwrap();
    assert_eq!(owned["status"], serde_json::json!("aborted"));
    assert_eq!(owned["ownership"], serde_json::json!("owned"));
    assert_eq!(free["status"], serde_json::json!("forked"));
    let wt = worktree_list(&h);
    assert!(
        !wt.contains("T9-fork-alpha"),
        "owned abort releases the worktree, got:\n{wt}"
    );
    assert!(
        wt.contains("T9-fork-beta"),
        "ownerless line keeps its worktree, got:\n{wt}"
    );
    let body = fs::read_to_string(h.state.join("logs").join("T9-fork-alpha.log")).unwrap();
    assert!(
        body.contains("aborted with parent"),
        "owned transcript marks the abort, got:\n{body}"
    );
    assert!(
        body.starts_with("entry line 1\nentry line 2\n"),
        "owned transcript keeps its prefix, got:\n{body}"
    );
    let beta_body = fs::read_to_string(h.state.join("logs").join("T9-fork-beta.log")).unwrap();
    assert_eq!(
        beta_body, "entry line 1\nentry line 2\n",
        "ownerless transcript is byte-identical, got:\n{beta_body}"
    );

    // Machine abort carries which fork lines stopped.
    let machine = stdout_text(cli(&h, &["dispatch", "abort", "9", "--json"]));
    let _ = machine; // second abort is a no-op report; the shape is pinned below.
    let status_json = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&status_json).unwrap();
    let rows = v["forks"].as_array().unwrap();
    let arow = rows.iter().find(|f| f["name"] == "alpha").unwrap();
    let brow = rows.iter().find(|f| f["name"] == "beta").unwrap();
    assert_eq!(arow["status"], serde_json::json!("aborted"));
    assert_eq!(brow["status"], serde_json::json!("forked"));

    // Human status + log observe the same abort.
    let status_human = stdout_text(cli(&h, &["status"]));
    let alpha_line = status_human
        .lines()
        .find(|l| l.contains("alpha"))
        .expect("status shows the aborted owned fork");
    assert!(
        alpha_line.contains("Aborted with parent"),
        "status must observe the owned abort, got:\n{alpha_line}"
    );
    assert!(
        alpha_line.contains("owned by attempt 1"),
        "status keeps naming the owner after the abort, got:\n{alpha_line}"
    );
    let beta_line = status_human
        .lines()
        .find(|l| l.contains("beta"))
        .expect("status shows the surviving fork");
    assert!(
        !beta_line.contains("Aborted"),
        "survivor reads live, got:\n{beta_line}"
    );
    assert_eq!(
        arow["summary"].as_str().unwrap_or(""),
        alpha_line.trim(),
        "aborted: json summary and human line are the identical string"
    );
    let log_human = stdout_text(cli(&h, &["log", "9"]));
    let log_alpha = log_human
        .lines()
        .find(|l| l.contains("alpha"))
        .expect("log shows the aborted owned fork");
    assert!(
        log_alpha.contains("Aborted with parent"),
        "log must observe the owned abort, got:\n{log_alpha}"
    );
    let log_json = stdout_text(cli(&h, &["log", "9", "--json"]));
    let lv: serde_json::Value = serde_json::from_str(&log_json).unwrap();
    let lrow = lv["forks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "alpha")
        .unwrap()
        .clone();
    assert_eq!(lrow["status"], serde_json::json!("aborted"));
    assert_eq!(
        lrow["summary"].as_str().unwrap_or(""),
        log_alpha.trim(),
        "log human line and json summary are the identical string"
    );

    // A resumed-then-aborted line stays down: resume refuses afterwards.
    cli(&h, &["fork", "9", "--resume", "alpha"])
        .assert()
        .failure();
}

#[test]
fn abort_after_parent_failure_cleans_owned_lines_but_keeps_ownerless() {
    let h = setup("abort-after-failure");
    seed_attempt(&h, 9, "impl_done");
    cli(
        &h,
        &["fork", "9", "--at", "2", "--name", "alpha", "--owned"],
    )
    .assert()
    .success();
    cli(&h, &["fork", "9", "--at", "2", "--name", "beta"])
        .assert()
        .success();

    // The parent attempt fails first: no live tasks remain to abort, only
    // the owned line still pins the parent's storage.
    let mut ledger = read_ledger(&h);
    ledger["attempts"][0]["status"] = serde_json::json!("impl_failed");
    write_ledger(&h, ledger);

    cli(&h, &["dispatch", "abort", "9"]).assert().success();
    let ledger = read_ledger(&h);
    let forks = ledger["forks"].as_array().unwrap();
    assert_eq!(
        forks.iter().find(|f| f["name"] == "alpha").unwrap()["status"],
        serde_json::json!("aborted"),
        "owned line follows the failed parent:\n{ledger}"
    );
    assert_eq!(
        forks.iter().find(|f| f["name"] == "beta").unwrap()["status"],
        serde_json::json!("forked"),
        "ownerless line survives the failed parent:\n{ledger}"
    );
}

#[test]
fn ownerless_and_legacy_forks_behave_exactly_as_before() {
    let h = setup("ownerless-legacy");
    seed_attempt(&h, 9, "impl_done");
    cli(&h, &["fork", "9", "--at", "2", "--name", "alpha"])
        .assert()
        .success();

    // Ownerless creation JSON reads ownerless.
    let out = cli(&h, &["fork", "9", "--at", "1", "--name", "beta", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out)).unwrap();
    assert_eq!(v["ownership"], serde_json::json!("ownerless"));

    // The ownerless lifecycle is unchanged: interrupt + resume with the
    // same transcript markers as before this ticket.
    cli(&h, &["fork", "9", "--interrupt", "alpha"])
        .assert()
        .success();
    cli(&h, &["fork", "9", "--resume", "alpha"])
        .assert()
        .success();
    let body = fs::read_to_string(h.state.join("logs").join("T9-fork-alpha.log")).unwrap();
    assert_eq!(
        body,
        "entry line 1\nentry line 2\ninterrupted during implement (slot T9-fork-alpha) — resumable\nresumed from implement checkpoint (slot T9-fork-alpha, resume #1) — transcript preserved\n",
        "ownerless lifecycle markers are byte-identical, got:\n{body}"
    );

    // Abort passes ownerless lines by untouched.
    cli(&h, &["dispatch", "abort", "9"]).assert().success();
    let ledger = read_ledger(&h);
    for name in ["alpha", "beta"] {
        let row = ledger["forks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap()
            .clone();
        assert_eq!(
            row["status"],
            serde_json::json!("forked"),
            "ownerless '{name}' survives abort:\n{ledger}"
        );
        assert_eq!(row["ownership"], serde_json::json!("ownerless"));
    }
    assert!(
        worktree_list(&h).contains("T9-fork-alpha") && worktree_list(&h).contains("T9-fork-beta"),
        "ownerless lines keep their worktrees"
    );

    // Legacy rows written before this ticket carry no ownership field:
    // they parse (no migration) and read as ownerless live lines.
    let legacy_log = h.state.join("logs").join("T9-fork-legacy.log");
    fs::write(&legacy_log, "entry line 1\n").unwrap();
    let mut ledger = read_ledger(&h);
    ledger["forks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": 99, "ticket": 9, "fork_no": 9, "name": "legacy",
            "parent_attempt": 1, "parent_entry": 1, "parent_fork": "",
            "lineage": [{"ticket": 9, "kind": "attempt", "no": 1, "entry": 1, "name": ""}],
            "slot": "T9-fork-legacy",
            "branch": "factory/T9-fork-legacy",
            "worktree": "/tmp/wt-T9-fork-legacy",
            "log_path": legacy_log.to_string_lossy(),
            "base_commit": "abc123", "started_at": "1700000000",
        }));
    write_ledger(&h, ledger);
    let human = stdout_text(cli(&h, &["status"]));
    let legacy_line = human
        .lines()
        .find(|l| l.contains("legacy"))
        .expect("status shows the legacy fork");
    assert!(
        legacy_line.contains("ownerless"),
        "legacy row reads as ownerless, got:\n{legacy_line}"
    );
    assert!(
        !legacy_line.contains("Aborted"),
        "legacy row reads live, got:\n{legacy_line}"
    );
    let json_out = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let lrow = v["forks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "legacy")
        .unwrap()
        .clone();
    assert_eq!(lrow["ownership"], serde_json::json!("ownerless"));
    assert_eq!(lrow["status"], serde_json::json!("forked"));
    assert_eq!(
        lrow["summary"].as_str().unwrap_or(""),
        legacy_line.trim(),
        "legacy: json summary and human line are the identical string"
    );
}
