use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "factory-start-test-{}-{}-{}",
        name,
        std::process::id(),
        n
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_exe(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

/// Fake `gh` that mimics a mise shim: a `mise ...` status line on stdout
/// ahead of the real `issue view` JSON payload.
fn write_noisy_gh(dir: &std::path::Path) -> PathBuf {
    write_exe(
        dir,
        "gh",
        r#"#!/bin/sh
if [ "$1" = "issue" ] && [ "$2" = "view" ]; then
  echo "mise ~/.config/mise/config.toml tools: gh@2.102.0"
  printf '{"number":99,"title":"Noise probe","body":"Part of #45."}'
  exit 0
fi
exit 0
"#,
    )
}

fn init_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            "git {args:?} failed"
        );
    };
    run(&["init", "-q", "-b", "main", "."]);
    run(&["config", "user.email", "test@local"]);
    run(&["config", "user.name", "test"]);
    run(&["commit", "-q", "--allow-empty", "-m", "base"]);
}

#[test]
fn start_survives_version_manager_noise_on_gh_stdout() {
    let dir = scratch_dir("noise");
    init_repo(&dir);
    let gh = write_noisy_gh(&dir);
    let wt = dir.join("wt-99");

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir)
        .env("FACTORY_GH_BIN", &gh)
        .args(["start", "99", "--worktree"])
        .arg(&wt);
    cmd.assert().success();

    let brief = fs::read_to_string(wt.join(".factory-brief.md"))
        .expect("start must write the brief in the worktree");
    assert!(
        brief.contains("Ticket issue: #99"),
        "brief must carry the parsed ticket, got:\n{brief}"
    );
}
