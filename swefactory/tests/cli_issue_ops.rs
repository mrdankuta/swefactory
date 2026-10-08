use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Fake-`gh` seam: every test points the binary at a shell script via
/// `FACTORY_GH_BIN`. The script logs each argv on its own line to $GH_LOG
/// and replays canned JSON from $GH_LIST_JSON / $GH_VIEW_JSON.
fn write_fake_gh(dir: &Path) -> PathBuf {
    let path = dir.join("gh");
    fs::write(
        &path,
        r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done >> "$GH_LOG"
if [ "$1" = "issue" ] && [ "$2" = "list" ]; then printf '%s' "$GH_LIST_JSON"; exit 0; fi
if [ "$1" = "issue" ] && [ "$2" = "view" ]; then printf '%s' "$GH_VIEW_JSON"; exit 0; fi
if [ "$1" = "issue" ] && [ "$2" = "comment" ]; then exit 0; fi
exit 0
"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-{}-{}-{}",
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

#[test]
fn handoff_requires_signal() {
    let dir = tmpdir("handoff-nosignal");
    let gh = write_fake_gh(&dir);
    let log = dir.join("gh.log");
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.arg("handoff")
        .arg("11")
        .env("FACTORY_GH_BIN", &gh)
        .env("GH_LOG", &log);
    cmd.assert()
        .failure()
        .stderr(predicates::str::contains("signal"));
}

#[test]
fn handoff_rejects_unknown_signal() {
    let dir = tmpdir("handoff-badsignal");
    let gh = write_fake_gh(&dir);
    let log = dir.join("gh.log");
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(["handoff", "11", "--signal", "BOGUS"])
        .env("FACTORY_GH_BIN", &gh)
        .env("GH_LOG", &log);
    cmd.assert().failure();
}

#[test]
fn handoff_posts_via_gh_and_prints_signal() {
    let dir = tmpdir("handoff-ok");
    let gh = write_fake_gh(&dir);
    let log = dir.join("gh.log");
    let body = dir.join("handoff-body.md");
    fs::write(&body, "ticket 11 done").unwrap();
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args([
        "handoff",
        "11",
        "--signal",
        "READY_FOR_REVIEW",
        "--body",
        body.to_str().unwrap(),
    ])
    .env("FACTORY_GH_BIN", &gh)
    .env("GH_LOG", &log)
    .env("GH_LIST_JSON", "[]")
    .env("GH_VIEW_JSON", "{}");
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("FACTORY:READY_FOR_REVIEW"));
    let logged = fs::read_to_string(&log).unwrap();
    assert!(logged.contains("comment"), "gh argv log:\n{logged}");
    assert!(
        logged.contains('1') && logged.contains('1'),
        "gh argv log:\n{logged}"
    );
    assert!(
        logged.contains("ticket 11 done"),
        "handoff body file contents must reach gh:\n{logged}"
    );
    assert!(
        logged.contains("FACTORY:READY_FOR_REVIEW"),
        "handoff trailer must reach gh:\n{logged}"
    );
}

#[test]
fn handoff_json_is_machine_parseable() {
    let dir = tmpdir("handoff-json");
    let gh = write_fake_gh(&dir);
    let log = dir.join("gh.log");
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(["--json", "handoff", "11", "--signal", "BLOCKED"])
        .env("FACTORY_GH_BIN", &gh)
        .env("GH_LOG", &log)
        .env("GH_LIST_JSON", "[]")
        .env("GH_VIEW_JSON", "{}");
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    assert_eq!(
        parsed.get("signal").and_then(|v| v.as_str()),
        Some("FACTORY:BLOCKED")
    );
}

#[test]
fn reconcile_reports_drift_without_mutating() {
    let dir = tmpdir("reconcile");
    let repo = dir.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_repo(&repo);
    // A commit mentioning #42 so drift detection has a hit.
    fs::write(repo.join("g.txt"), "y").unwrap();
    let st = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(st.success());
    let st = std::process::Command::new("git")
        .args(["commit", "-m", "land #42 widget"])
        .current_dir(&repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(st.success());
    let gh = write_fake_gh(&dir);
    let log = dir.join("gh.log");
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.arg("reconcile")
        .current_dir(&repo)
        .env("FACTORY_GH_BIN", &gh)
        .env("GH_LOG", &log)
        .env(
            "GH_LIST_JSON",
            r#"[{"number":42,"title":"widget"},{"number":43,"title":"gadget"}]"#,
        )
        .env("GH_VIEW_JSON", "{}");
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("POSSIBLY-LANDED"))
        .stdout(predicates::str::contains("#43 open"));
    let logged = fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains("list"),
        "reconcile must list issues:\n{logged}"
    );
    for banned in ["comment", "edit", "create", "close"] {
        assert!(
            !logged.lines().any(|l| l.trim() == banned),
            "reconcile is read-only, must not call gh issue {banned}:\n{logged}"
        );
    }
}

#[test]
fn reconcile_json_is_machine_parseable() {
    let dir = tmpdir("reconcile-json");
    let repo = dir.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_repo(&repo);
    let gh = write_fake_gh(&dir);
    let log = dir.join("gh.log");
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(["--json", "reconcile"])
        .current_dir(&repo)
        .env("FACTORY_GH_BIN", &gh)
        .env("GH_LOG", &log)
        .env("GH_LIST_JSON", r#"[{"number":7,"title":"scaffold"}]"#)
        .env("GH_VIEW_JSON", "{}");
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8(out.stdout).unwrap()).unwrap();
    assert!(parsed.get("issues").is_some(), "missing issues: {parsed}");
    assert!(
        parsed.get("branches").is_some(),
        "missing branches: {parsed}"
    );
}

#[test]
fn start_creates_worktree_branch_and_brief() {
    let dir = tmpdir("start");
    let repo = dir.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_repo(&repo);
    let gh = write_fake_gh(&dir);
    let log = dir.join("gh.log");
    let wt = dir.join("repo-T11");
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args([
        "start",
        "11",
        "--worktree",
        wt.to_str().unwrap(),
    ])
    .current_dir(&repo)
    .env("FACTORY_GH_BIN", &gh)
    .env("GH_LOG", &log)
    .env("GH_LIST_JSON", "[]")
    .env(
        "GH_VIEW_JSON",
        r#"{"number":11,"title":"Issue Ops: Handoff, Reconcile, Start","body":"ticket body here"}"#,
    );
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("factory/T11-"));
    assert!(wt.join(".factory-brief.md").is_file(), "brief missing");
    let brief = fs::read_to_string(wt.join(".factory-brief.md")).unwrap();
    assert!(brief.contains("#11"), "brief:\n{brief}");
    assert!(brief.contains("ticket body here"), "brief:\n{brief}");
    let logged = fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains("view"),
        "start must read the ticket via gh:\n{logged}"
    );
    for banned in ["comment", "edit", "create", "close"] {
        assert!(
            !logged.lines().any(|l| l.trim() == banned),
            "start must not write to GitHub (gh issue {banned}):\n{logged}"
        );
    }
    // Cleanup: remove the worktree registration so temp dirs stay tidy.
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", wt.to_str().unwrap()])
        .current_dir(&repo)
        .status();
}

#[test]
fn no_api_client_dependency() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let content = fs::read_to_string(&manifest).unwrap().to_lowercase();
    for banned in ["reqwest", "octocrab", "ureq", "hyper", "graphql"] {
        assert!(
            !content.contains(banned),
            "Cargo.toml must not contain API client {banned}"
        );
    }
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let code = fs::read_to_string(&src).unwrap().to_lowercase();
    for banned in ["reqwest", "octocrab", "ureq", "hyper"] {
        assert!(
            !code.contains(banned),
            "src/main.rs must not use API client {banned}"
        );
    }
}
