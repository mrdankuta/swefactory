//! Train auto-cut + auto-finish (ticket #35).
//!
//! Seam: argv in -> exit code + stdout/stderr out + state-dir files and
//! subprocess argv logs out. Same fake-repo harness as `cli_release.rs`:
//! `gh` is a fake script (no network), `git` is a logging shim over the
//! real binary, and both the implementer (`pi`) and the check adapter are
//! fakes. The repo is cut on `dev` so the human-merge predicate
//! (train-head-ancestor-of-dev) is exercisable hermetically.
//!
//! What these tests catch: auto-cut integrating an accepted ticket with no
//! open train (default slug, integrated label, merge commit), concurrent
//! opens merging into the existing train (never a second train),
//! human-merged trains auto-finishing on the next pass (row closed,
//! worktree gone, cleanup printed, branches never deleted), unmerged
//! trains never auto-finishing, no-train finish/abandon refusals, and the
//! negative invariant (no automation merge into dev/main, no pushes, no
//! branch deletes executed).
//! What they miss: real `origin/dev` fetches (the harness has no remote;
//! the read-only fetch is best-effort and its argv is asserted, not its
//! network effect) and real CI on the train head.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-train-auto-{}-{}-{}",
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

fn git_repo_dev(dir: &Path) {
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
    run(&["init", "-b", "dev"]);
    run(&["config", "user.email", "t@t"]);
    run(&["config", "user.name", "t"]);
    fs::write(dir.join("f.txt"), "x\n").unwrap();
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
echo "pi cwd=$(pwd) slot:${FACTORY_SLOT:-unset} tmpdir:${TMPDIR:-unset}" >> "$PI_LOG"
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
    git_repo_dev(&repo);
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

fn git_log(h: &Harness) -> String {
    fs::read_to_string(&h.git_log).unwrap()
}

fn gh_log(h: &Harness) -> String {
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

fn rev_parse(repo: &Path, rev: &str) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn branch_exists_local(repo: &Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", branch])
        .current_dir(repo)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn integration_branches(repo: &Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["branch", "--list", "integration/*"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| l.trim().trim_start_matches(['*', '+']).trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Automation safety on the git argv log: fetch/rev-parse/merge-base checks
/// are read-only and allowed; pushes, merges into dev/main, checkouts of
/// dev/main for work, and executed branch deletes are forbidden. Cleanup
/// is printed, never executed.
fn assert_auto_safety(git: &str) {
    for line in git.lines() {
        let l = line.trim();
        if l.starts_with("merge-base") {
            continue;
        }
        assert!(
            !l.starts_with("push"),
            "automation must never push:\n{line}"
        );
        assert!(
            !((l == "merge" || l.starts_with("merge "))
                && (l.contains("dev") || l.contains("main"))),
            "automation must never merge into dev/main:\n{line}"
        );
        assert!(
            !(l.starts_with("checkout") && (l.contains(" dev") || l.contains(" main"))),
            "automation must never checkout dev/main for work:\n{line}"
        );
        assert!(
            !(l.contains("branch") && (l.contains(" -d") || l.contains(" -D"))),
            "automation must never delete branches (print cleanup only):\n{line}"
        );
        assert!(
            !(l.contains("branch")
                && l.contains("-f")
                && (l.contains("dev") || l.contains("main"))),
            "automation must never force-update dev/main:\n{line}"
        );
    }
}

#[test]
fn auto_cut_integrates_accepted_ticket_with_no_open_train() {
    let h = setup("auto-cut");
    let dev_before = rev_parse(&h.repo, "dev");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cmd = factory_cmd(&h, &["dispatch", "once"]);
    cmd.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("auto-cut"))
        .stdout(predicates::str::contains("INTEGRATED"));

    // Freshly cut default-slug train, attempt recorded on it.
    let train = open_train_branch(&h);
    assert!(train.starts_with("integration/"), "train: {train}");
    assert!(
        train.ends_with("-train"),
        "auto-cut uses the default slug: {train}"
    );
    let ledger = fs::read_to_string(h.state.join("dispatch.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&ledger).unwrap();
    let a = &parsed.get("attempts").and_then(|x| x.as_array()).unwrap()[0];
    assert_eq!(
        a.get("status").and_then(|x| x.as_str()),
        Some("review_done"),
        "ledger:\n{ledger}"
    );
    assert_eq!(
        a.get("train_branch").and_then(|x| x.as_str()),
        Some(train.as_str()),
        "attempt must record its auto-cut train:\n{ledger}"
    );

    // Labels + row state correct: integrated, never accepted-only.
    let gh = gh_log(&h);
    assert!(
        gh.contains("integrated"),
        "accepted ticket must label integrated:\n{gh}"
    );
    let log = std::process::Command::new("git")
        .args(["log", "--oneline", &train])
        .current_dir(&h.repo)
        .output()
        .unwrap();
    let log = String::from_utf8(log.stdout).unwrap();
    assert!(
        log.contains("Merge ticket #11"),
        "train must carry the merge:\n{log}"
    );

    // Negative: automation never moved dev.
    assert_eq!(
        rev_parse(&h.repo, "dev"),
        dev_before,
        "auto-cut must leave dev untouched"
    );
    assert_auto_safety(&git_log(&h));
}

#[test]
fn concurrently_opened_train_is_merged_into_never_duplicated() {
    let h = setup("concurrent");
    factory_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    let train = open_train_branch(&h);
    assert!(train.contains("beta"), "slug must survive: {train}");

    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cmd = factory_cmd(&h, &["dispatch", "once"]);
    cmd.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("INTEGRATED"));

    // Merged into the concurrent train: same branch, still exactly one.
    assert_eq!(
        open_train_branch(&h),
        train,
        "concurrent train must be reused, never replaced"
    );
    assert_eq!(
        integration_branches(&h.repo),
        vec![train.clone()],
        "never a second train"
    );
    let gh = gh_log(&h);
    assert!(gh.contains("integrated"), "must label integrated:\n{gh}");
    let log = std::process::Command::new("git")
        .args(["log", "--oneline", &train])
        .current_dir(&h.repo)
        .output()
        .unwrap();
    let log = String::from_utf8(log.stdout).unwrap();
    assert!(
        log.contains("Merge ticket #11"),
        "concurrent train must carry the merge:\n{log}"
    );
    assert_auto_safety(&git_log(&h));
}

#[test]
fn human_merged_train_auto_finishes_on_next_pass() {
    let h = setup("auto-finish");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cut = factory_cmd(&h, &["dispatch", "once"]);
    cut.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cut.assert().success();
    let train = open_train_branch(&h);

    // Human merges the train into dev (automation never does this).
    git_run(&h.repo, &["checkout", "-q", "dev"]);
    git_run(
        &h.repo,
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            "human lands train",
            &train,
        ],
    );
    let dev_after_human = rev_parse(&h.repo, "dev");

    // Scope the automation log to the finish passes only.
    fs::write(&h.git_log, "").unwrap();

    // Dry-run observes but changes nothing.
    let mut dry = factory_cmd(&h, &["dispatch", "once", "--dry-run"]);
    dry.env("GH_LIST_JSON", "[]");
    dry.assert()
        .success()
        .stdout(predicates::str::contains("would finish"));
    assert_eq!(
        open_train_branch(&h),
        train,
        "dry-run must leave the merged train open"
    );
    assert!(
        h.state.join("train-worktree").exists() || git_log(&h).contains("fetch"),
        "dry-run still observes (fetch first)"
    );

    // The next real pass finishes without the command.
    let mut done = factory_cmd(&h, &["dispatch", "once"]);
    done.env("GH_LIST_JSON", "[]");
    done.assert()
        .success()
        .stdout(predicates::str::contains("merged into dev"))
        .stdout(predicates::str::contains("git branch -d"));

    // Row merged (closed), worktree gone, branches never deleted.
    assert!(
        !h.state.join("train.json").is_file(),
        "finished train must close"
    );
    assert!(
        !h.state.join("train-worktree").exists(),
        "train worktree must be removed"
    );
    assert!(
        branch_exists_local(&h.repo, &train),
        "auto-finish prints cleanup but never deletes the train branch"
    );
    assert!(
        branch_exists_local(&h.repo, "factory/T11-demo-widget"),
        "auto-finish never deletes member branches"
    );
    // Automation added no commits to dev after the human merge.
    assert_eq!(
        rev_parse(&h.repo, "dev"),
        dev_after_human,
        "auto-finish must not touch dev"
    );

    // Read-only fetch first, worktree removal only, no deletes, no dev merge.
    let git = git_log(&h);
    let fetch_pos = git
        .find("fetch")
        .expect("auto-finish fetches first:\n{git}");
    let remove_pos = git
        .find("worktree remove")
        .or_else(|| git.find("worktree prune"))
        .expect("finish removes only its own worktree:\n{git}");
    assert!(
        fetch_pos < remove_pos,
        "fetch must precede the finish:\n{git}"
    );
    assert_auto_safety(&git);
}

#[test]
fn unmerged_train_never_auto_finishes_and_no_train_refusals_hold() {
    let h = setup("unmerged");
    factory_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    let train = open_train_branch(&h);

    // Passes with an unmerged train leave it open and silent.
    let mut cmd = factory_cmd(&h, &["dispatch", "once"]);
    cmd.env("GH_LIST_JSON", "[]");
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.contains("merged into dev"),
        "unmerged train must never auto-finish:\n{stdout}"
    );
    assert_eq!(
        open_train_branch(&h),
        train,
        "unmerged train stays open across passes"
    );

    // Unmerged finish still refuses: the human must merge first.
    factory_cmd(&h, &["release", "finish"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("not merged"));

    // Abandon closes the unmerged train; then both refuse with no train.
    factory_cmd(&h, &["release", "abandon"])
        .assert()
        .success()
        .stdout(predicates::str::contains("abandoned"));
    factory_cmd(&h, &["release", "finish"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no open train"));
    factory_cmd(&h, &["release", "abandon"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no open train"));

    assert_auto_safety(&git_log(&h));
}

#[test]
fn auto_paths_never_merge_to_dev_or_delete_branches() {
    let h = setup("negative");
    let dev_before = rev_parse(&h.repo, "dev");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    // Auto-cut pass.
    let mut cut = factory_cmd(&h, &["dispatch", "once"]);
    cut.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cut.assert().success();
    let train = open_train_branch(&h);
    assert_eq!(
        rev_parse(&h.repo, "dev"),
        dev_before,
        "auto-cut must not advance dev"
    );

    // Human merge, then auto-finish pass.
    git_run(&h.repo, &["checkout", "-q", "dev"]);
    git_run(
        &h.repo,
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            "human lands train",
            &train,
        ],
    );
    let dev_human = rev_parse(&h.repo, "dev");
    let mut done = factory_cmd(&h, &["dispatch", "once"]);
    done.env("GH_LIST_JSON", "[]");
    done.assert().success();
    assert_eq!(
        rev_parse(&h.repo, "dev"),
        dev_human,
        "auto-finish must not add commits to dev"
    );

    // No automation argv merges into dev/main, pushes, or deletes: the
    // train merge (`merge --no-ff -m "Merge ticket ..." <ticket-branch>`)
    // runs inside the train worktree and never names dev/main, so the
    // shared helper plus the dev-head pins above are the full proof.
    // (Printed cleanup is the only place branch deletes appear.)
    let git = git_log(&h);
    assert_auto_safety(&git);
    assert!(
        branch_exists_local(&h.repo, &train),
        "train branch survives auto-finish"
    );
}
