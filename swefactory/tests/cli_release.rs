//! Release-train lifecycle CLI boundary tests (ticket #14).
//!
//! Seam: argv in -> exit code + stdout/stderr out + state-dir files and
//! subprocess argv logs out. `gh` is a fake script (no network), `git` is a
//! logging shim over the real binary, and both the implementer (`pi`) and
//! the check adapter are fakes. Mirrors the `cli_dispatch.rs` patterns.
//!
//! What these tests catch: train cutting from dev, second-train refusal,
//! clean-only merges, loud drops, verified finishes, clean abandons,
//! no-train refusals, and the structural safety invariant.
//! What they miss: real `origin/dev` fetches and real CI on the train head.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-release-{}-{}-{}",
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
    root: PathBuf,
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
        root,
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

fn release_cmd(h: &Harness, args: &[&str]) -> Command {
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

fn open_train_branch(h: &Harness) -> String {
    let text = fs::read_to_string(h.state.join("train.json")).expect("train.json must exist");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v.get("branch")
        .and_then(|b| b.as_str())
        .unwrap()
        .to_string()
}

/// Structural safety: automation never pushes, never merges into dev/main,
/// never checks out dev/main for work, and never executes a branch delete.
/// Cleanup is printed, never executed.
fn assert_release_safety(git: &str) {
    for line in git.lines() {
        let l = line.trim();
        // `merge-base --is-ancestor` is a read-only check, not a merge.
        if l.starts_with("merge-base") {
            continue;
        }
        assert!(!l.starts_with("push"), "release must never push:\n{line}");
        assert!(
            !((l == "merge" || l.starts_with("merge "))
                && (l.contains("dev") || l.contains("main"))),
            "release must never merge into dev/main:\n{line}"
        );
        assert!(
            !(l.starts_with("checkout") && (l.contains(" dev") || l.contains(" main"))),
            "release must never checkout dev/main for work:\n{line}"
        );
        assert!(
            !(l.contains("branch") && (l.contains(" -d") || l.contains(" -D"))),
            "release must never delete branches (print cleanup only):\n{line}"
        );
        assert!(
            !(l.contains("branch")
                && l.contains("-f")
                && (l.contains("dev") || l.contains("main"))),
            "release must never force-update dev/main:\n{line}"
        );
    }
}

#[test]
fn release_start_cuts_train_and_refuses_second() {
    let h = setup("start");
    release_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success()
        .stdout(predicates::str::contains("integration/"))
        .stdout(predicates::str::contains("beta"));

    let train = open_train_branch(&h);
    assert!(
        train.starts_with("integration/"),
        "train branch must live under integration/: {train}"
    );
    assert!(train.contains("beta"), "slug must survive: {train}");

    // Train was cut from dev (same commit).
    let dev_sha = std::process::Command::new("git")
        .args(["rev-parse", "dev"])
        .current_dir(&h.repo)
        .output()
        .unwrap();
    let dev_sha = String::from_utf8(dev_sha.stdout).unwrap();
    let train_sha = std::process::Command::new("git")
        .args(["rev-parse", &train])
        .current_dir(&h.repo)
        .output()
        .unwrap();
    let train_sha = String::from_utf8(train_sha.stdout).unwrap();
    assert_eq!(
        dev_sha.trim(),
        train_sha.trim(),
        "train must start at dev head"
    );

    // A second train is refused while one is open.
    release_cmd(&h, &["release", "start", "again"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("already open"));
    assert_eq!(
        open_train_branch(&h),
        train,
        "refused start must leave the open train untouched"
    );

    // Status shows the open train.
    release_cmd(&h, &["release", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains(&train));

    assert_release_safety(&git_log(&h));
}

#[test]
fn release_status_json_is_machine_parseable() {
    let h = setup("status-json");
    release_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    let out = release_cmd(&h, &["--json", "release", "status"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    assert_eq!(v.get("open").and_then(|x| x.as_bool()), Some(true));
    let train = v.get("train").and_then(|x| x.as_str()).unwrap().to_string();
    assert!(train.starts_with("integration/"), "train: {v}");
    assert!(v.get("members").is_some(), "missing members: {v}");
}

#[test]
fn release_merge_and_drop_roundtrip_with_loud_double_drop() {
    let h = setup("merge-drop");
    release_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    let train = open_train_branch(&h);

    // Two ticket branches with real commits.
    git_run(&h.repo, &["checkout", "-qb", "factory/T11-widget"]);
    fs::write(h.repo.join("w11.txt"), "eleven\n").unwrap();
    git_run(&h.repo, &["add", "-A"]);
    git_run(&h.repo, &["commit", "-qm", "feat(T11): widget"]);
    git_run(&h.repo, &["checkout", "-q", "dev"]);

    // Ledger records the ticket branch on this train.
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        format!(
            r#"{{"attempts":[{{"id":1,"ticket":11,"attempt_no":1,"base_commit":"b","head_commit":"h","branch":"factory/T11-widget","worktree":"","status":"review_done","exit_code":0,"log_path":"","started_at":"s","finished_at":"f","train_branch":"{train}"}}]}}"#
        ),
    )
    .unwrap();

    release_cmd(&h, &["release", "merge", "11"])
        .assert()
        .success()
        .stdout(predicates::str::contains("merged"));

    // Drop reverts loudly.
    release_cmd(&h, &["release", "drop", "11"])
        .assert()
        .success()
        .stdout(predicates::str::contains("dropped"));

    // A second drop fails loudly — never a silent no-op.
    release_cmd(&h, &["release", "drop", "11"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("NOT merged"));

    // A ticket that was never merged also fails loudly.
    release_cmd(&h, &["release", "drop", "99"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("NOT merged"));

    assert_release_safety(&git_log(&h));
}

#[test]
fn release_merge_conflict_aborts_with_train_untouched() {
    let h = setup("conflict");
    release_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    let train = open_train_branch(&h);

    // Base commit, then two sides add the same file differently.
    git_run(&h.repo, &["checkout", "-qb", "factory/T21-conf"]);
    fs::write(h.repo.join("c.txt"), "ticket-side\n").unwrap();
    git_run(&h.repo, &["add", "-A"]);
    git_run(&h.repo, &["commit", "-qm", "ticket side"]);
    git_run(&h.repo, &["checkout", "-q", &train]);
    fs::write(h.repo.join("c.txt"), "train-side\n").unwrap();
    git_run(&h.repo, &["add", "-A"]);
    git_run(&h.repo, &["commit", "-qm", "train-side"]);
    git_run(&h.repo, &["checkout", "-q", "dev"]);

    let head_before = std::process::Command::new("git")
        .args(["rev-parse", &train])
        .current_dir(&h.repo)
        .output()
        .unwrap();
    let head_before = String::from_utf8(head_before.stdout).unwrap();

    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        format!(
            r#"{{"attempts":[{{"id":1,"ticket":21,"attempt_no":1,"base_commit":"b","head_commit":"h","branch":"factory/T21-conf","worktree":"","status":"review_done","exit_code":0,"log_path":"","started_at":"s","finished_at":"f","train_branch":"{train}"}}]}}"#
        ),
    )
    .unwrap();

    release_cmd(&h, &["release", "merge", "21"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("conflicted"));

    let head_after = std::process::Command::new("git")
        .args(["rev-parse", &train])
        .current_dir(&h.repo)
        .output()
        .unwrap();
    let head_after = String::from_utf8(head_after.stdout).unwrap();
    assert_eq!(
        head_before.trim(),
        head_after.trim(),
        "conflicted merge must leave the train untouched"
    );

    assert_release_safety(&git_log(&h));
}

#[test]
fn dispatch_auto_merges_clean_ticket_into_open_train() {
    let h = setup("auto-clean");
    release_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    let train = open_train_branch(&h);
    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cmd = release_cmd(&h, &["dispatch", "once"]);
    cmd.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cmd.assert().success();

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
        "attempt must record its train:\n{ledger}"
    );

    let gh = fs::read_to_string(&h.gh_log).unwrap();
    assert!(
        gh.contains("integrated"),
        "accepted ticket must label integrated:\n{gh}"
    );

    // The train carries the merge commit.
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

    assert_release_safety(&git_log(&h));
}

#[test]
fn dispatch_conflict_routes_ticket_to_blocked() {
    let h = setup("auto-conflict");
    release_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    let train = open_train_branch(&h);

    // Pre-create the ticket branch from dev with a file that will collide
    // with a train-side commit: both add c.txt differently.
    git_run(&h.repo, &["checkout", "-qb", "factory/T11-demo-widget"]);
    fs::write(h.repo.join("c.txt"), "ticket-side\n").unwrap();
    git_run(&h.repo, &["add", "-A"]);
    git_run(&h.repo, &["commit", "-qm", "ticket side"]);
    git_run(&h.repo, &["checkout", "-q", &train]);
    fs::write(h.repo.join("c.txt"), "train-side\n").unwrap();
    git_run(&h.repo, &["add", "-A"]);
    git_run(&h.repo, &["commit", "-qm", "train-side"]);
    git_run(&h.repo, &["checkout", "-q", "dev"]);

    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cmd = release_cmd(&h, &["dispatch", "once"]);
    cmd.env("GH_LIST_JSON", r#"[{"number":11}]"#);
    cmd.assert().success();

    let ledger = fs::read_to_string(h.state.join("dispatch.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&ledger).unwrap();
    let a = &parsed.get("attempts").and_then(|x| x.as_array()).unwrap()[0];
    assert_eq!(
        a.get("status").and_then(|x| x.as_str()),
        Some("review_failed"),
        "conflicted merge must not stay done:\n{ledger}"
    );
    let gh = fs::read_to_string(&h.gh_log).unwrap();
    assert!(
        gh.contains("blocked"),
        "conflict must route the ticket to blocked:\n{gh}"
    );

    assert_release_safety(&git_log(&h));
}

#[test]
fn release_finish_verifies_dev_merge_and_prints_cleanup() {
    let h = setup("finish");
    release_cmd(&h, &["release", "finish"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no open train"));

    release_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    let train = open_train_branch(&h);

    // Unmerged trains refuse finish: the human must merge first.
    release_cmd(&h, &["release", "finish"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("not merged"));

    // Give the train a real member commit so the human merge is provable.
    git_run(&h.repo, &["checkout", "-qb", "factory/T11-widget"]);
    fs::write(h.repo.join("w11.txt"), "eleven\n").unwrap();
    git_run(&h.repo, &["add", "-A"]);
    git_run(&h.repo, &["commit", "-qm", "feat(T11): widget"]);
    git_run(&h.repo, &["checkout", "-q", "dev"]);
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        format!(
            r#"{{"attempts":[{{"id":1,"ticket":11,"attempt_no":1,"base_commit":"b","head_commit":"h","branch":"factory/T11-widget","worktree":"","status":"review_done","exit_code":0,"log_path":"","started_at":"s","finished_at":"f","train_branch":"{train}"}}]}}"#
        ),
    )
    .unwrap();
    release_cmd(&h, &["release", "merge", "11"])
        .assert()
        .success();
    // Still unmerged into dev: finish keeps refusing.
    release_cmd(&h, &["release", "finish"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("not merged"));

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

    release_cmd(&h, &["release", "finish"])
        .assert()
        .success()
        .stdout(predicates::str::contains("merged into dev"))
        .stdout(predicates::str::contains("git branch -d"));

    assert!(
        !h.state.join("train.json").is_file(),
        "finished train must close"
    );
    assert!(
        !h.state.join("train-worktree").exists(),
        "train worktree must be removed"
    );

    // Both finish and abandon refuse when no train is open.
    release_cmd(&h, &["release", "finish"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no open train"));
    release_cmd(&h, &["release", "abandon"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no open train"));

    // Automation printed cleanup but executed no deletes.
    assert_release_safety(&git_log(&h));
    let git = git_log(&h);
    assert!(
        git.contains("worktree remove") || git.contains("worktree prune"),
        "finish removes only its own worktree:\n{git}"
    );
}

#[test]
fn release_abandon_closes_unmerged_train_without_merge_check() {
    let h = setup("abandon");
    release_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    let train = open_train_branch(&h);

    release_cmd(&h, &["release", "abandon"])
        .assert()
        .success()
        .stdout(predicates::str::contains("abandoned"));

    assert!(
        !h.state.join("train.json").is_file(),
        "abandoned train must close"
    );
    assert!(
        branch_exists_local(&h.repo, &train),
        "abandon prints cleanup but never deletes the branch"
    );

    assert_release_safety(&git_log(&h));
}

fn branch_exists_local(repo: &Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", branch])
        .current_dir(repo)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn release_help_lists_the_lifecycle() {
    let h = setup("help");
    release_cmd(&h, &["release", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("start"))
        .stdout(predicates::str::contains("status"))
        .stdout(predicates::str::contains("merge"))
        .stdout(predicates::str::contains("drop"))
        .stdout(predicates::str::contains("finish"))
        .stdout(predicates::str::contains("abandon"));
}

#[test]
fn release_never_touches_dev_or_deletes() {
    let h = setup("safety");
    release_cmd(&h, &["release", "start", "beta"])
        .assert()
        .success();
    release_cmd(&h, &["release", "status"]).assert().success();
    release_cmd(&h, &["release", "abandon"]).assert().success();
    assert_release_safety(&git_log(&h));

    // No GitHub writes at all on this path, and the source itself carries
    // no push/delete-branch affordance in the release section.
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let code = fs::read_to_string(&src).unwrap();
    let section = code
        .split("RELEASE TRAIN (ticket #14)")
        .nth(1)
        .unwrap_or("")
        .split("fn main()")
        .next()
        .unwrap_or("")
        .to_string();
    for banned in ["\"push\"", "\"branch\", \"-D\"", "\"branch\", \"-d\""] {
        assert!(
            !section.contains(banned),
            "release section must not contain {banned}"
        );
    }
    let _ = &h.root;
}
