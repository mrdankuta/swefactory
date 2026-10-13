//! Onboarding backbone CLI boundary tests (ticket #42, spec #41 slice).
//!
//! Seam: argv in -> exit code + stdout/stderr out + state-dir files and
//! subprocess argv logs out. `gh` is a PATH-sandbox fake (no network)
//! across auth states (absent/expired/valid) and repo list/create
//! outcomes; the seeded fresh repo is byte-checked on disk.
//!
//! What these tests catch: the guided backbone install check, the GitHub
//! auth tour, repo pick-or-create with private default and identity-only
//! seed, resumability without duplicating created resources, and --plain
//! coverage of every step.
//! What they miss: real browser device flow, real `gh repo create`, and
//! the later-ticket slices (idea capture, draft generation, approve loop,
//! visual UI screens).

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-onboard-{}-{}-{}",
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

/// Fake `gh`: logs every argv line, then answers from envknobs.
/// GH_AUTH=expired fails `auth status`; GH_REPO_LIST is the JSON array
/// for `repo list`; GH_CREATE_FAIL=1 fails `repo create`.
fn write_fake_gh(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "gh",
        r#"#!/bin/sh
for a in "$@"; do printf '%s ' "$a"; done >> "$GH_LOG"
printf '\n' >> "$GH_LOG"
if [ "$1" = "--version" ]; then echo "gh version 2.102.0"; exit 0; fi
if [ "$1" = "auth" ]; then
  if [ "$GH_AUTH" = "expired" ]; then echo "authentication expired" >&2; exit 1; fi
  echo "Logged in to github.com"; exit 0
fi
if [ "$1" = "repo" ] && [ "$2" = "list" ]; then printf '%s' "$GH_REPO_LIST"; exit 0; fi
if [ "$1" = "repo" ] && [ "$2" = "view" ]; then printf '{"nameWithOwner":"%s"}' "$3"; exit 0; fi
if [ "$1" = "repo" ] && [ "$2" = "create" ]; then
  if [ "$GH_CREATE_FAIL" = "1" ]; then echo "create failed (boom)" >&2; exit 1; fi
  echo "created"; exit 0
fi
exit 0
"#,
    )
}

struct Harness {
    #[allow(dead_code)]
    root: PathBuf,
    work: PathBuf,
    state: PathBuf,
    gh_log: PathBuf,
    gh: PathBuf,
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    let h = Harness {
        state: root.join("state"),
        gh_log: root.join("gh.log"),
        gh: write_fake_gh(&root),
        root,
        work,
    };
    fs::write(&h.gh_log, "").unwrap();
    h
}

fn onboard_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.work);
    cmd.env("FACTORY_GH_BIN", &h.gh)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("GH_LOG", &h.gh_log)
        .env("GH_AUTH", "valid")
        .env("GH_REPO_LIST", "[]")
        .env_remove("GH_CREATE_FAIL")
        .env_remove("FACTORY_ONBOARD_REPO")
        .env_remove("FACTORY_ONBOARD_NEW")
        .env_remove("FACTORY_ONBOARD_VISIBILITY")
        .env_remove("FACTORY_ONBOARD_IDEA");
    cmd
}

fn gh_log(h: &Harness) -> String {
    fs::read_to_string(&h.gh_log).unwrap()
}

fn onboard_state(h: &Harness) -> serde_json::Value {
    let text = fs::read_to_string(h.state.join("onboard.json"))
        .expect("onboard.json must exist after a run");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn onboard_full_walk_absent_expired_valid_ends_ready() {
    let h = setup("walk");

    // Absent gh: the tour names the install, and the run stops there.
    let mut absent = onboard_cmd(&h, &["onboard", "--plain"]);
    absent.env("FACTORY_GH_BIN", "/nonexistent/gh-missing");
    absent
        .assert()
        .failure()
        .stderr(predicates::str::contains("GitHub CLI"))
        .stderr(predicates::str::contains("cli.github.com"));

    // Expired login: the tour walks the browser device flow, then stops.
    let mut expired = onboard_cmd(&h, &["onboard", "--plain"]);
    expired.env("GH_AUTH", "expired");
    expired
        .assert()
        .failure()
        .stderr(predicates::str::contains("gh auth login"))
        .stderr(predicates::str::contains("browser"));

    // Valid login + fresh repo: the walk ends in a ready repo.
    let mut ready = onboard_cmd(&h, &["onboard", "--plain", "--new", "bea-idea"]);
    ready.env("GH_AUTH", "valid");
    ready
        .assert()
        .success()
        .stdout(predicates::str::contains("[onboard:install]"))
        .stdout(predicates::str::contains("[onboard:auth]"))
        .stdout(predicates::str::contains("[onboard:repo]"))
        .stdout(predicates::str::contains("[onboard:ready]"))
        .stdout(predicates::str::contains("bea-idea"));

    let state = onboard_state(&h);
    assert_eq!(
        state.get("repo").and_then(|r| r.as_str()),
        Some("bea-idea"),
        "state must record the ready repo: {state}"
    );
}

#[test]
fn onboard_fresh_private_default_identity_seed_byte_checked() {
    let h = setup("fresh");

    onboard_cmd(&h, &["onboard", "--plain", "--new", "bea-fresh"])
        .assert()
        .success();

    // Private by default: the create call carries --private, never --public.
    let log = gh_log(&h);
    let create_line = log
        .lines()
        .find(|l| l.contains("repo create") || (l.contains("repo") && l.contains("create")))
        .unwrap_or_else(|| panic!("gh log must show repo create:\n{log}"));
    assert!(
        create_line.contains("--private"),
        "fresh repos are private by default:\n{create_line}"
    );
    assert!(
        !create_line.contains("--public"),
        "default create must not pass --public:\n{create_line}"
    );

    // Identity-only seed, byte-exact: name + idea README, ignore file, no code.
    let seed = h.work.join("bea-fresh");
    let readme = fs::read_to_string(seed.join("README.md")).expect("seed README.md");
    assert_eq!(readme, "# bea-fresh\n\nBuilt with the factory.\n");
    let ignore = fs::read_to_string(seed.join(".gitignore")).expect("seed .gitignore");
    assert_eq!(ignore, "target/\nfactory/.state/\n.DS_Store\n");
    let mut entries: Vec<String> = fs::read_dir(&seed)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    let visible: Vec<&str> = entries
        .iter()
        .map(|s| s.as_str())
        .filter(|s| *s != ".git")
        .collect();
    assert_eq!(
        visible,
        vec![".gitignore", "IDEA.md", "README.md"],
        "seed is identity only (name, idea README, ignore file): {visible:?}"
    );
    assert!(
        !visible
            .iter()
            .any(|f| f.ends_with(".rs") || f.ends_with(".py") || f.ends_with(".js")),
        "no generated sample code may pollute the seed: {visible:?}"
    );
}

#[test]
fn onboard_explicit_public_is_a_plain_words_choice() {
    let h = setup("public");

    onboard_cmd(
        &h,
        &[
            "onboard",
            "--plain",
            "--new",
            "bea-open",
            "--visibility",
            "public",
        ],
    )
    .assert()
    .success();

    let log = gh_log(&h);
    assert!(
        log.lines().any(|l| l.contains("--public")),
        "explicit public choice must reach gh:\n{log}"
    );
    let state = onboard_state(&h);
    assert_eq!(
        state.get("visibility").and_then(|v| v.as_str()),
        Some("public"),
        "the visibility choice is recorded: {state}"
    );
}

#[test]
fn onboard_existing_path_picks_cleanly_without_create() {
    let h = setup("existing");

    onboard_cmd(&h, &["onboard", "--plain", "--repo", "bea/existing"])
        .assert()
        .success()
        .stdout(predicates::str::contains("[onboard:ready]"))
        .stdout(predicates::str::contains("bea/existing"));

    let log = gh_log(&h);
    assert!(
        !log.lines().any(|l| l.contains("create")),
        "picking an existing repo must never create:\n{log}"
    );
    assert!(
        log.lines()
            .any(|l| l.contains("repo view") && l.contains("bea/existing")),
        "the pick is verified through gh:\n{log}"
    );
    let state = onboard_state(&h);
    assert_eq!(
        state.get("repo").and_then(|r| r.as_str()),
        Some("bea/existing"),
        "state must record the picked repo: {state}"
    );
    assert_eq!(
        state.get("created").and_then(|c| c.as_bool()),
        Some(false),
        "existing path records created=false: {state}"
    );
}

#[test]
fn onboard_kill_mid_flow_reruns_without_duplicating_repo() {
    let h = setup("resume");

    // First run: auth passes, the repo is created, then creation of the
    // seed is interrupted — simulated here as a failed create, which still
    // records nothing, so the sharper case below pre-records a create.
    let mut first = onboard_cmd(&h, &["onboard", "--plain", "--new", "bea-resume"]);
    first.env("GH_CREATE_FAIL", "1");
    first.assert().failure();
    assert!(
        !h.state.join("onboard.json").is_file()
            || onboard_state(&h).get("created").and_then(|c| c.as_bool()) != Some(true),
        "a failed create must not record created=true"
    );

    // The kill-mid-flow case: a previous run created the repo (state says
    // so) but never finished. Rerun must continue WITHOUT creating again:
    // the fake fails loudly if `repo create` is ever invoked.
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("onboard.json"),
        r#"{"install_ok":true,"auth":"valid","repo":"bea-resume","visibility":"private","created":true,"seeded":false,"steps":["install","auth","repo"]}"#,
    )
    .unwrap();
    fs::write(&h.gh_log, "").unwrap();
    let mut rerun = onboard_cmd(&h, &["onboard", "--plain", "--new", "bea-resume"]);
    rerun.env("GH_CREATE_FAIL", "1");
    rerun
        .assert()
        .success()
        .stdout(predicates::str::contains("[onboard:ready]"));

    let log = gh_log(&h);
    assert!(
        !log.lines().any(|l| l.contains("create")),
        "resume must not create the repo a second time:\n{log}"
    );
    let state = onboard_state(&h);
    assert_eq!(
        state.get("seeded").and_then(|s| s.as_bool()),
        Some(true),
        "resume finishes the seed: {state}"
    );
}

#[test]
fn onboard_plain_covers_every_step() {
    let h_plain = setup("plain");
    onboard_cmd(&h_plain, &["onboard", "--plain", "--new", "bea-plain"])
        .assert()
        .success()
        .stdout(predicates::str::contains("[onboard:install]"))
        .stdout(predicates::str::contains("[onboard:auth]"))
        .stdout(predicates::str::contains("[onboard:repo]"))
        .stdout(predicates::str::contains("[onboard:ready]"));

    // The default (future visual) path carries the same plain backbone:
    // no step is ever interactive-only.
    let h_default = setup("default");
    onboard_cmd(&h_default, &["onboard", "--new", "bea-visual"])
        .assert()
        .success()
        .stdout(predicates::str::contains("[onboard:install]"))
        .stdout(predicates::str::contains("[onboard:auth]"))
        .stdout(predicates::str::contains("[onboard:repo]"))
        .stdout(predicates::str::contains("[onboard:ready]"));
}

#[test]
fn onboard_needs_a_repo_choice_in_plain_words() {
    let h = setup("choice");
    onboard_cmd(&h, &["onboard", "--plain"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--repo"))
        .stderr(predicates::str::contains("--new"));
}
