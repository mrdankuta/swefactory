//! Land core ritual CLI boundary tests (ticket #21, spec #20).
//!
//! Seam: argv in -> exit code + stdout/stderr out + repo/issue state
//! (branches, shas, worktrees, state-dir files, `gh` argv log). `gh` is a
//! fake script (no network), `git` is a logging shim over the real binary,
//! and the check adapter is a fake honoring `ADAPTER_FAIL`. Mirrors the
//! `cli_release.rs` patterns.
//!
//! What these tests catch: green open trains landing end to end
//! (merge --no-ff, push, closes with release note, worktree removed,
//! cleanup printed never executed), red-head and dirty-tree refusals
//! before any mutation, and dry-run plans with zero state changes.
//! What they miss: real remote pushes and real CI on the train head.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-land-{}-{}-{}",
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

fn git_real(repo: &Path, args: &[&str]) {
    let st = std::process::Command::new("/usr/bin/git")
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

fn git_rev(repo: &Path, rev: &str) -> String {
    let out = std::process::Command::new("/usr/bin/git")
        .args(["rev-parse", rev])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "rev-parse {rev} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn write_fake_gh(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "gh",
        r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done >> "$GH_LOG"
printf -- '---\n' >> "$GH_LOG"
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

fn write_fake_adapter(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "adapter.sh",
        r#"#!/bin/sh
echo "adapter slot:${FACTORY_SLOT:-unset} cwd:$(pwd)" >> "$ADAPTER_LOG"
echo "FACTORY-CHECK PASSED"
exit "${ADAPTER_FAIL:-0}"
"#,
    )
}

fn ticket_view(number: u64, title: &str, body: &str, state: &str) -> String {
    let body_esc = body
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!(
        r#"{{"number":{number},"title":"{title}","body":"{body_esc}","labels":[],"state":"{state}","comments":[]}}"#
    )
}

struct Harness {
    repo: PathBuf,
    state: PathBuf,
    views: PathBuf,
    gh_log: PathBuf,
    git_log: PathBuf,
    adapter_log: PathBuf,
    gh: PathBuf,
    git_shim: PathBuf,
    adapter: PathBuf,
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    // Seed repo on dev, with a local bare origin so push/ls-remote work offline.
    git_real(&repo, &["init", "-b", "dev"]);
    git_real(&repo, &["config", "user.email", "t@t"]);
    git_real(&repo, &["config", "user.name", "t"]);
    fs::write(repo.join("f.txt"), "x\n").unwrap();
    git_real(&repo, &["add", "."]);
    git_real(&repo, &["commit", "-m", "seed"]);
    let origin = root.join("origin.git");
    git_real(&repo, &["init", "--bare", origin.to_str().unwrap()]);
    git_real(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_real(&repo, &["push", "-q", "origin", "dev"]);
    let views = root.join("views");
    fs::create_dir_all(&views).unwrap();
    let h = Harness {
        state: root.join("state"),
        gh_log: root.join("gh.log"),
        git_log: root.join("git.log"),
        adapter_log: root.join("adapter.log"),
        gh: write_fake_gh(&root),
        git_shim: write_git_shim(&root),
        adapter: write_fake_adapter(&root),
        repo,
        views,
    };
    fs::write(&h.gh_log, "").unwrap();
    fs::write(&h.git_log, "").unwrap();
    fs::write(&h.adapter_log, "").unwrap();
    h
}

fn write_view(h: &Harness, number: u64, title: &str, body: &str, state: &str) {
    fs::write(
        h.views.join(format!("{number}.json")),
        ticket_view(number, title, body, state),
    )
    .unwrap();
}

fn write_ledger(h: &Harness, train: &str, tickets: &[u64]) {
    fs::create_dir_all(&h.state).unwrap();
    let attempts: Vec<String> = tickets
        .iter()
        .enumerate()
        .map(|(i, t)| {
            format!(
                r#"{{"id":{},"ticket":{t},"attempt_no":1,"base_commit":"b","head_commit":"h","branch":"factory/T{t}-work","worktree":"","status":"review_done","exit_code":0,"log_path":"","started_at":"s","finished_at":"f","train_branch":"{train}"}}"#,
                i + 1
            )
        })
        .collect();
    fs::write(
        h.state.join("dispatch.json"),
        format!("{{\"attempts\":[{}]}}", attempts.join(",")),
    )
    .unwrap();
}

fn land_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_GH_BIN", &h.gh)
        .env("FACTORY_GIT_BIN", &h.git_shim)
        .env("FACTORY_CHECK_ADAPTER", &h.adapter)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("GH_LOG", &h.gh_log)
        .env("GIT_LOG", &h.git_log)
        .env("ADAPTER_LOG", &h.adapter_log)
        .env("GH_VIEWS_DIR", &h.views)
        .env("ADAPTER_FAIL", "0")
        .env_remove("FACTORY_SLOT");
    cmd
}

fn git_log_text(h: &Harness) -> String {
    fs::read_to_string(&h.git_log).unwrap()
}

fn gh_log_text(h: &Harness) -> String {
    fs::read_to_string(&h.gh_log).unwrap()
}

fn open_train_branch(h: &Harness) -> String {
    let text = fs::read_to_string(h.state.join("train.json")).expect("train.json must exist");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v.get("branch")
        .and_then(|b| b.as_str())
        .unwrap()
        .to_string()
}

/// No merge into dev, no push, no issue close may have run through the shim/log.
fn assert_no_mutations(h: &Harness) {
    let git = git_log_text(h);
    for line in git.lines() {
        let l = line.trim();
        if l.starts_with("merge-base") || l.starts_with("rev-parse") {
            continue;
        }
        assert!(!l.starts_with("merge "), "refusal must not merge:\n{line}");
        assert!(!l.starts_with("push"), "refusal must not push:\n{line}");
        assert!(
            !l.starts_with("checkout"),
            "refusal must not switch branches:\n{line}"
        );
    }
    let gh = gh_log_text(h);
    assert!(
        !gh.contains("close"),
        "refusal must not close tickets:\n{gh}"
    );
}

fn cut_train(h: &Harness) -> String {
    land_cmd(h, &["release", "start", "beta"])
        .assert()
        .success();
    open_train_branch(h)
}

fn snapshot_state(h: &Harness) -> String {
    let mut parts = vec![
        git_rev(&h.repo, "dev"),
        git_rev(&h.repo, "origin/dev"),
        fs::read_to_string(h.state.join("train.json")).unwrap(),
        fs::read_to_string(h.state.join("dispatch.json")).unwrap(),
        gh_log_text(h),
    ];
    for args in [
        vec!["branch", "--list"],
        vec!["worktree", "list", "--porcelain"],
        vec!["status", "--porcelain"],
    ] {
        let out = std::process::Command::new("/usr/bin/git")
            .args(&args)
            .current_dir(&h.repo)
            .output()
            .unwrap();
        parts.push(String::from_utf8(out.stdout).unwrap());
    }
    parts.join("\n====\n")
}

#[test]
fn land_refuses_with_no_open_train() {
    let h = setup("no-train");
    land_cmd(&h, &["land"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no open train"));
    assert_no_mutations(&h);
}

#[test]
fn land_refuses_dirty_working_tree_before_any_mutation() {
    let h = setup("dirty");
    let train = cut_train(&h);
    // Dirty the checkout: uncommitted work must never leak into a release merge.
    fs::write(h.repo.join("unrelated.txt"), "wip\n").unwrap();

    land_cmd(&h, &["land"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("dirty"));
    assert_no_mutations(&h);
    assert_eq!(
        open_train_branch(&h),
        train,
        "refused land must leave the open train untouched"
    );
    // The check runner must not even run: tree refusal comes first.
    assert!(
        fs::read_to_string(&h.adapter_log).unwrap().is_empty(),
        "dirty tree must refuse before running checks"
    );
}

#[test]
fn land_refuses_red_train_head_before_any_mutation_naming_the_check() {
    let h = setup("red-head");
    let train = cut_train(&h);
    write_ledger(&h, &train, &[11]);

    let mut cmd = land_cmd(&h, &["land"]);
    cmd.env("ADAPTER_FAIL", "1");
    cmd.assert()
        .failure()
        .stderr(predicates::str::contains("checks failed"))
        .stderr(predicates::str::contains("adapter"));
    assert_no_mutations(&h);
    assert_eq!(
        open_train_branch(&h),
        train,
        "refused land must leave the open train untouched"
    );
    // dev must still equal origin/dev: nothing merged, nothing pushed.
    assert_eq!(git_rev(&h.repo, "dev"), git_rev(&h.repo, "origin/dev"));
}

#[test]
fn land_dry_run_prints_full_plan_with_zero_state_changes() {
    let h = setup("dry-run");
    let train = cut_train(&h);
    write_ledger(&h, &train, &[11]);
    write_view(&h, 11, "Demo widget", "Build the widget.", "OPEN");

    let before = snapshot_state(&h);
    land_cmd(&h, &["land", "--dry-run"])
        .assert()
        .success()
        .stdout(predicates::str::contains("dry-run"))
        .stdout(predicates::str::contains("merge --no-ff"))
        .stdout(predicates::str::contains(&train))
        .stdout(predicates::str::contains("push origin dev"))
        .stdout(predicates::str::contains("close"))
        .stdout(predicates::str::contains("11"))
        .stdout(predicates::str::contains("cleanup"))
        .stdout(predicates::str::contains("git branch -d"));
    let after = snapshot_state(&h);
    assert_eq!(
        before, after,
        "dry-run must leave every snapshotted state identical"
    );
    assert!(
        !h.state.join("train-worktree").exists(),
        "dry-run must not stage persistent worktrees"
    );
}

fn branch_exists_local(repo: &Path, branch: &str) -> bool {
    std::process::Command::new("/usr/bin/git")
        .args(["rev-parse", "--verify", "--quiet", branch])
        .current_dir(repo)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("/usr/bin/git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(repo)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn land_green_open_train_lands_end_to_end() {
    let h = setup("green");
    let train = cut_train(&h);

    // Member work: a real ticket branch with a real commit, recorded on the train.
    git_real(&h.repo, &["checkout", "-qb", "factory/T11-work"]);
    fs::write(h.repo.join("w11.txt"), "eleven\n").unwrap();
    git_real(&h.repo, &["add", "-A"]);
    git_real(&h.repo, &["commit", "-qm", "feat(T11): widget"]);
    git_real(&h.repo, &["checkout", "-q", "dev"]);
    write_ledger(&h, &train, &[11]);
    write_view(&h, 11, "Demo widget", "Build the widget.", "OPEN");
    land_cmd(&h, &["release", "merge", "11"]).assert().success();

    land_cmd(&h, &["land"])
        .assert()
        .success()
        .stdout(predicates::str::contains("merged"))
        .stdout(predicates::str::contains("pushed"))
        .stdout(predicates::str::contains("closed"))
        .stdout(predicates::str::contains("11"))
        .stdout(predicates::str::contains("cleanup"))
        .stdout(predicates::str::contains("git branch -d"));

    // Merge --no-ff landed on dev and was pushed: origin/dev tracks dev.
    assert!(
        is_ancestor(&h.repo, &train, "dev"),
        "train must be merged into dev"
    );
    assert_eq!(git_rev(&h.repo, "dev"), git_rev(&h.repo, "origin/dev"));
    let log = std::process::Command::new("/usr/bin/git")
        .args(["log", "--oneline", "dev"])
        .current_dir(&h.repo)
        .output()
        .unwrap();
    let log = String::from_utf8(log.stdout).unwrap();
    assert!(
        log.contains(&format!("Merge {train} into dev")),
        "merge must be --no-ff with the train name in the message:\n{log}"
    );

    // Member ticket closed with the release note.
    let gh = gh_log_text(&h);
    assert!(gh.contains("close"), "member ticket must close:\n{gh}");
    assert!(gh.contains("11"), "close must name the ticket:\n{gh}");
    assert!(
        gh.contains("Landed"),
        "close must carry the release note:\n{gh}"
    );

    // Train worktree removed, train record closed.
    assert!(
        !h.state.join("train.json").is_file(),
        "landed train must close"
    );
    assert!(
        !h.state.join("train-worktree").exists(),
        "train worktree must be removed"
    );

    // Cleanup printed, never executed: member and train branches survive.
    assert!(
        branch_exists_local(&h.repo, "factory/T11-work"),
        "cleanup never deletes member branches"
    );
    assert!(
        branch_exists_local(&h.repo, &train),
        "cleanup never deletes the train branch"
    );
    for line in git_log_text(&h).lines() {
        assert!(
            !(line.contains("branch") && (line.contains(" -d") || line.contains(" -D"))),
            "land must never delete branches (print cleanup only):\n{line}"
        );
    }
}
