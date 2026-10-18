//! Fork lineage + nesting + caps (ticket #81, spec #65 slice).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (ledger + canned transcripts under
//! state/logs/ + optional shared `facts.json`); the working directory
//! is a scratch git repo standing in for the project. No daemon, no
//! network, no real runs anywhere here.
//!
//! What these tests catch: lineage (parent ticket, attempt, entry, plus
//! the chain for nested forks, oldest-first) renders in `status` and
//! `log` in both human and machine-readable modes with identical facts;
//! a fork of a fork inherits its parent fork's prefix and records the
//! full chain; over-cap forks are refused cleanly with the cap stated;
//! each experiment reads as a calm one-line summary with per-line
//! figures sourced from the shared facts document.
//! What they miss: lifecycle composition (resume/land on forks,
//! ticket #82) and the facts document itself (spec #64, tickets #77/78).

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-forklineage-{}-{}-{}",
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

fn write_facts(h: &Harness, revision: u64, facts: serde_json::Value) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("facts.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "revision": revision,
            "facts": facts,
        }))
        .unwrap(),
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

/// One fork row as the ledger carries it (lineage oldest-first).
#[allow(clippy::too_many_arguments)]
fn fork_row(
    h: &Harness,
    ticket: u64,
    id: u64,
    fork_no: u64,
    name: &str,
    parent_attempt: u64,
    parent_entry: u64,
    parent_fork: &str,
    lineage: serde_json::Value,
    log_body: &str,
) -> serde_json::Value {
    let log_path = h
        .state
        .join("logs")
        .join(format!("T{ticket}-fork-{name}.log"));
    fs::create_dir_all(h.state.join("logs")).unwrap();
    fs::write(&log_path, log_body).unwrap();
    serde_json::json!({
        "id": id,
        "ticket": ticket,
        "fork_no": fork_no,
        "name": name,
        "parent_attempt": parent_attempt,
        "parent_entry": parent_entry,
        "parent_fork": parent_fork,
        "lineage": lineage,
        "slot": format!("T{ticket}-fork-{name}"),
        "branch": format!("factory/T{ticket}-fork-{name}"),
        "worktree": format!("/tmp/wt-T{ticket}-fork-{name}"),
        "log_path": log_path.to_string_lossy(),
        "base_commit": "abc123def456",
        "started_at": "1700000000",
    })
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

fn stdout_text(mut cmd: Command) -> String {
    String::from_utf8_lossy(&cmd.assert().success().get_output().stdout).into_owned()
}

#[test]
fn lineage_renders_in_status_both_modes_with_identical_facts() {
    let h = setup("status-lineage");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "entry line 1\nentry line 2\nentry line 3\n");
    // Direct fork via the CLI so the lineage is recorded, not hand-seeded.
    cli(&h, &["fork", "9", "--at", "2", "--name", "alpha"])
        .assert()
        .success();

    let human = stdout_text(cli(&h, &["status"]));
    let alpha_lines: Vec<&str> = human.lines().filter(|l| l.contains("alpha")).collect();
    assert_eq!(
        alpha_lines.len(),
        1,
        "each experiment reads as one calm line, got:\n{human}"
    );
    let line = alpha_lines[0];
    for fact in ["9", "attempt 1", "entry 2", "alpha"] {
        assert!(
            line.contains(fact),
            "status line must carry the lineage fact '{fact}', got:\n{line}"
        );
    }

    let json_out = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let forks = v["forks"].as_array().expect("status json carries forks");
    assert_eq!(forks.len(), 1);
    let row = &forks[0];
    let lineage = row["lineage"].as_array().expect("fork row carries lineage");
    assert_eq!(lineage.len(), 1, "direct fork chain has one link");
    assert_eq!(lineage[0]["ticket"], serde_json::json!(9));
    assert_eq!(lineage[0]["kind"], serde_json::json!("attempt"));
    assert_eq!(lineage[0]["no"], serde_json::json!(1));
    assert_eq!(lineage[0]["entry"], serde_json::json!(2));
    // Identical facts: every structured lineage fact appears in the prose.
    for word in [
        lineage[0]["ticket"].to_string(),
        lineage[0]["no"].to_string(),
        lineage[0]["entry"].to_string(),
    ] {
        assert!(
            line.contains(&word),
            "human and machine modes must share lineage facts ('{word}'), line:\n{line}"
        );
    }
    assert!(
        row["summary"].as_str().unwrap_or("").contains("alpha"),
        "json carries the same one-line summary, got:\n{row}"
    );
    assert_eq!(
        row["summary"].as_str().unwrap_or(""),
        line.trim(),
        "human line and json summary must be the identical string"
    );
}

#[test]
fn lineage_renders_in_log_both_modes_with_identical_facts() {
    let h = setup("log-lineage");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "entry line 1\nentry line 2\nentry line 3\n");
    cli(&h, &["fork", "9", "--at", "2", "--name", "alpha"])
        .assert()
        .success();

    let human = stdout_text(cli(&h, &["log", "9"]));
    let alpha_lines: Vec<&str> = human.lines().filter(|l| l.contains("alpha")).collect();
    assert_eq!(
        alpha_lines.len(),
        1,
        "log lists each experiment as one calm line, got:\n{human}"
    );

    let json_out = stdout_text(cli(&h, &["log", "9", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let forks = v["forks"].as_array().expect("log json carries forks");
    assert_eq!(forks.len(), 1);
    let lineage = forks[0]["lineage"]
        .as_array()
        .expect("log fork row carries lineage");
    assert_eq!(lineage.len(), 1);
    assert_eq!(lineage[0]["ticket"], serde_json::json!(9));
    assert_eq!(lineage[0]["kind"], serde_json::json!("attempt"));
    assert_eq!(lineage[0]["no"], serde_json::json!(1));
    assert_eq!(lineage[0]["entry"], serde_json::json!(2));
    assert_eq!(
        forks[0]["summary"].as_str().unwrap_or(""),
        alpha_lines[0].trim(),
        "log human line and json summary must be the identical string"
    );
}

#[test]
fn fork_of_fork_inherits_parent_prefix_and_records_full_chain() {
    let h = setup("nested");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "line 1\nline 2\nline 3\nline 4\n");
    cli(&h, &["fork", "9", "--at", "2", "--name", "alpha"])
        .assert()
        .success();
    let alpha_before = fs::read(h.state.join("logs").join("T9-fork-alpha.log")).unwrap();

    // A fork of a fork: parent addressed by name, prefix from the fork log.
    cli(
        &h,
        &[
            "fork", "9", "--fork", "alpha", "--at", "1", "--name", "gamma",
        ],
    )
    .assert()
    .success();

    // Parent fork log stays byte-identical; child holds the fork prefix.
    let alpha_after = fs::read(h.state.join("logs").join("T9-fork-alpha.log")).unwrap();
    assert_eq!(
        alpha_before, alpha_after,
        "forking a fork must not touch the parent fork log"
    );
    let gamma_body = fs::read_to_string(h.state.join("logs").join("T9-fork-gamma.log"))
        .expect("child log exists");
    assert_eq!(
        gamma_body, "line 1\n",
        "child inherits the parent fork prefix through the entry, got:\n{gamma_body}"
    );

    // Full chain recorded oldest-first: attempt link, then fork link.
    let ledger = read_ledger(&h);
    let forks = ledger["forks"].as_array().unwrap();
    assert_eq!(forks.len(), 2);
    let gamma = forks.iter().find(|f| f["name"] == "gamma").unwrap();
    assert_eq!(gamma["parent_fork"], serde_json::json!("alpha"));
    let chain = gamma["lineage"].as_array().unwrap();
    assert_eq!(
        chain.len(),
        2,
        "nested chain has two links, got:\n{chain:?}"
    );
    assert_eq!(chain[0]["kind"], serde_json::json!("attempt"));
    assert_eq!(chain[0]["no"], serde_json::json!(1));
    assert_eq!(chain[0]["entry"], serde_json::json!(2));
    assert_eq!(chain[1]["kind"], serde_json::json!("fork"));
    assert_eq!(chain[1]["name"], serde_json::json!("alpha"));
    assert_eq!(chain[1]["entry"], serde_json::json!(1));

    // The chain renders oldest-first in both modes.
    let human = stdout_text(cli(&h, &["status"]));
    let gamma_line = human
        .lines()
        .find(|l| l.contains("gamma"))
        .expect("status shows the nested fork");
    let attempt_pos = gamma_line
        .find("attempt 1")
        .expect("chain names the attempt");
    let fork_pos = gamma_line
        .find("alpha")
        .expect("chain names the parent fork");
    assert!(
        attempt_pos < fork_pos,
        "status chain renders oldest-first, got:\n{gamma_line}"
    );
    let json_out = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let rows = v["forks"].as_array().unwrap();
    let grow = rows.iter().find(|f| f["name"] == "gamma").unwrap();
    assert_eq!(
        grow["lineage"],
        serde_json::json!(chain),
        "json carries the same oldest-first chain"
    );

    // Parent fork addressable by number too.
    cli(
        &h,
        &["fork", "9", "--fork", "1", "--at", "2", "--name", "delta"],
    )
    .assert()
    .success();
    let ledger = read_ledger(&h);
    let delta = ledger["forks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "delta")
        .unwrap()
        .clone();
    assert_eq!(delta["parent_fork"], serde_json::json!("alpha"));
    assert_eq!(delta["lineage"].as_array().unwrap().len(), 2);

    // --attempt and --fork together are refused with nothing written.
    let before = read_ledger(&h);
    let wt_before = worktree_list(&h);
    cli(
        &h,
        &[
            "fork",
            "9",
            "--attempt",
            "1",
            "--fork",
            "alpha",
            "--at",
            "1",
            "--name",
            "nope",
        ],
    )
    .assert()
    .failure();
    assert_eq!(read_ledger(&h), before, "refused fork writes nothing");
    assert_eq!(
        worktree_list(&h),
        wt_before,
        "refused fork adds no worktree"
    );

    // Unknown parent fork names the taken names.
    let out = cli(
        &h,
        &["fork", "9", "--fork", "zz", "--at", "1", "--name", "nope2"],
    )
    .assert()
    .failure()
    .get_output()
    .stderr
    .clone();
    let err = String::from_utf8_lossy(&out).into_owned();
    assert!(
        err.contains("alpha"),
        "unknown parent fork must list the taken names, got:\n{err}"
    );
}

#[test]
fn over_cap_forks_are_refused_cleanly_with_cap_stated() {
    let h = setup("cap");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "one\ntwo\nthree\n");
    // Seed the documented cap worth of forks through the CLI itself.
    for i in 1..=8 {
        cli(
            &h,
            &["fork", "9", "--at", "1", "--name", &format!("exp{i}")],
        )
        .assert()
        .success();
    }
    let ledger_before = read_ledger(&h);
    let wt_before = worktree_list(&h);
    let branches_before = branch_list(&h);

    let out = cli(&h, &["fork", "9", "--at", "1", "--name", "toomany"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8_lossy(&out).into_owned();
    assert!(
        err.contains('8'),
        "over-cap refusal must state the cap, got:\n{err}"
    );
    assert!(
        err.to_lowercase().contains("cap"),
        "over-cap refusal must name it as a cap, got:\n{err}"
    );

    assert_eq!(
        read_ledger(&h),
        ledger_before,
        "refused fork writes no ledger row"
    );
    assert_eq!(
        worktree_list(&h),
        wt_before,
        "refused fork adds no worktree"
    );
    assert_eq!(
        branch_list(&h),
        branches_before,
        "refused fork adds no branch"
    );
    assert!(
        !h.state.join("logs").join("T9-fork-toomany.log").exists(),
        "refused fork seeds no log file"
    );
}

#[test]
fn experiment_summaries_carry_shared_facts_figures() {
    let h = setup("figures");
    write_ledger(
        &h,
        serde_json::json!({ "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")] }),
    );
    write_impl_log(&h, 9, 1, "line 1\nline 2\n");
    write_facts(
        &h,
        3,
        serde_json::json!({
            "forks": { "9": { "alpha": { "cost": "14 USD", "changed": "2 files" } } },
        }),
    );
    cli(&h, &["fork", "9", "--at", "2", "--name", "alpha"])
        .assert()
        .success();
    cli(&h, &["fork", "9", "--at", "1", "--name", "bare"])
        .assert()
        .success();

    // Human status: one calm line per experiment, figures from the document.
    let human = stdout_text(cli(&h, &["status"]));
    let alpha_line = human
        .lines()
        .find(|l| l.contains("alpha"))
        .expect("status shows alpha");
    for fact in ["14 USD", "2 files"] {
        assert!(
            alpha_line.contains(fact),
            "summary must carry the shared figure '{fact}', got:\n{alpha_line}"
        );
    }
    let bare_line = human
        .lines()
        .find(|l| l.contains("bare"))
        .expect("status shows bare");
    assert!(
        bare_line.contains("not yet recorded"),
        "missing figures read as not-recorded, never zero-filled, got:\n{bare_line}"
    );
    assert!(
        !bare_line.contains("0 USD") && !bare_line.contains("cost 0"),
        "missing figures must never be zero-filled, got:\n{bare_line}"
    );

    // JSON carries the identical summary string plus structured figures.
    let json_out = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let rows = v["forks"].as_array().unwrap();
    let arow = rows.iter().find(|f| f["name"] == "alpha").unwrap();
    assert_eq!(
        arow["summary"].as_str().unwrap_or(""),
        alpha_line.trim(),
        "json summary and human line are the identical string"
    );
    assert_eq!(arow["cost"], serde_json::json!("14 USD"));
    assert_eq!(arow["changed"], serde_json::json!("2 files"));
    assert_eq!(arow["facts_revision"], serde_json::json!(3));
    let brow = rows.iter().find(|f| f["name"] == "bare").unwrap();
    assert!(brow["cost"].is_null(), "missing cost is null, got:\n{brow}");
    assert!(
        brow["changed"].is_null(),
        "missing changed is null, got:\n{brow}"
    );
    assert_eq!(
        brow["summary"].as_str().unwrap_or(""),
        bare_line.trim(),
        "json summary and human line are the identical string"
    );

    // Log carries the same figures in both modes.
    let log_human = stdout_text(cli(&h, &["log", "9"]));
    let log_alpha = log_human
        .lines()
        .find(|l| l.contains("alpha"))
        .expect("log shows alpha");
    assert!(
        log_alpha.contains("14 USD") && log_alpha.contains("2 files"),
        "log line carries the shared figures, got:\n{log_alpha}"
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
    assert_eq!(lrow["cost"], serde_json::json!("14 USD"));
    assert_eq!(
        lrow["summary"].as_str().unwrap_or(""),
        log_alpha.trim(),
        "log human line and json summary are the identical string"
    );
}

#[test]
fn seeded_lineage_without_cli_creation_renders_oldest_first() {
    // Fixture-seeded chains (e.g. from an earlier version) render the
    // same oldest-first way as CLI-created ones.
    let h = setup("seeded");
    let lineage = serde_json::json!([
        {"ticket": 9, "kind": "attempt", "no": 1, "entry": 2, "name": ""},
        {"ticket": 9, "kind": "fork", "no": 1, "entry": 1, "name": "alpha"},
    ]);
    write_ledger(
        &h,
        serde_json::json!({
            "attempts": [attempt_with_log(&h, 9, 1, 1, "impl_done")],
            "forks": [
                fork_row(&h, 9, 2, 1, "alpha", 1, 2, "", serde_json::json!([
                    {"ticket": 9, "kind": "attempt", "no": 1, "entry": 2, "name": ""},
                ]), "line 1\nline 2\n"),
                fork_row(&h, 9, 3, 2, "gamma", 1, 1, "alpha", lineage, "line 1\n"),
            ],
        }),
    );
    write_impl_log(&h, 9, 1, "line 1\nline 2\nline 3\n");

    let human = stdout_text(cli(&h, &["status"]));
    let gamma_line = human
        .lines()
        .find(|l| l.contains("gamma"))
        .expect("status shows gamma");
    let attempt_pos = gamma_line
        .find("attempt 1")
        .expect("chain names the attempt");
    let fork_pos = gamma_line
        .find("alpha")
        .expect("chain names the parent fork");
    assert!(
        attempt_pos < fork_pos,
        "seeded chain renders oldest-first, got:\n{gamma_line}"
    );
    let json_out = stdout_text(cli(&h, &["status", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let rows = v["forks"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let grow = rows.iter().find(|f| f["name"] == "gamma").unwrap();
    assert_eq!(grow["lineage"].as_array().unwrap().len(), 2);
    assert_eq!(grow["parent_fork"], serde_json::json!("alpha"));
}
