use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique scratch dir under the OS temp dir (std-only, no extra deps).
fn scratch_dir(tag: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "swefactory-doctor-{}-{}-{}",
        std::process::id(),
        id,
        tag
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, "#!/bin/sh").unwrap();
    f.write_all(body.as_bytes()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

const FAKE_GH: &str = r#"
case "$1" in
  --version) echo "gh version 9.9.9-test"; exit 0;;
  auth) if [ "$FIXTURE_GH_AUTH" = "fail" ]; then echo "not logged in" >&2; exit 1; fi; exit 0;;
  repo) if [ "$FIXTURE_GH_REPO" = "fail" ]; then echo "repo not found" >&2; exit 1; fi; echo '{"nameWithOwner":"o/r"}'; exit 0;;
  *) exit 0;;
esac
"#;

const FAKE_GIT: &str = r#"
if [ "$1" = "config" ] && [ "$2" = "user.name" ]; then
  if [ "$FIXTURE_GIT_IDENTITY" = "fail" ]; then exit 1; fi
  echo "Test User"; exit 0
fi
if [ "$1" = "config" ] && [ "$2" = "user.email" ]; then
  if [ "$FIXTURE_GIT_IDENTITY" = "fail" ]; then exit 1; fi
  echo "test@example.com"; exit 0
fi
if [ "$1" = "worktree" ] && [ "$2" = "prune" ] && [ "$3" = "-n" ]; then
  if [ -n "$STALE_MARKER" ] && [ -f "$STALE_MARKER" ]; then
    echo "Removing worktrees/wt1: gitdir file points to non-existent location"
  fi
  exit 0
fi
exit 0
"#;

const FAKE_PI: &str = r#"
if [ "$FIXTURE_PI" = "fail" ]; then echo "no runner" >&2; exit 1; fi
echo "pi version 9.9.9-test"; exit 0
"#;

const FAKE_CARGO: &str = r#"
if [ "$FIXTURE_CARGO" = "fail" ]; then echo "no cargo" >&2; exit 1; fi
echo "cargo 9.9.9-test"; exit 0
"#;

const FAKE_RUSTC: &str = r#"
if [ "$FIXTURE_RUSTC" = "fail" ]; then echo "no rustc" >&2; exit 1; fi
echo "rustc 9.9.9-test"; exit 0
"#;

struct Sandbox {
    dir: PathBuf,
    bin: PathBuf,
    state: PathBuf,
    adapter: PathBuf,
}

impl Sandbox {
    /// Healthy sandbox: every probe passes.
    fn healthy(tag: &str) -> Self {
        let dir = scratch_dir(tag);
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let gh = write_script(&bin, "gh", FAKE_GH);
        let git = write_script(&bin, "git", FAKE_GIT);
        let pi = write_script(&bin, "pi", FAKE_PI);
        write_script(&bin, "cargo", FAKE_CARGO);
        write_script(&bin, "rustc", FAKE_RUSTC);
        let _ = (gh, git, pi);
        let state = dir.join("state");
        fs::create_dir_all(&state).unwrap();
        // Project cache dir: the widened state-dirs check probes it.
        fs::create_dir_all(dir.join("factory").join(".cache")).unwrap();
        let adapter = write_script(&dir, "factory-check", "echo ok\n");
        Self {
            dir,
            bin,
            state,
            adapter,
        }
    }

    fn real_path() -> String {
        std::env::var("PATH").unwrap_or_default()
    }

    fn cmd(&self, json: bool) -> Command {
        let mut cmd = Command::cargo_bin("factory").unwrap();
        if json {
            cmd.arg("--json");
        }
        cmd.arg("doctor");
        // Hermetic seams: never touch the developer's real gh/git/pi.
        cmd.env(
            "FACTORY_GH_BIN",
            self.bin.join("gh").to_string_lossy().to_string(),
        );
        cmd.env(
            "FACTORY_GIT_BIN",
            self.bin.join("git").to_string_lossy().to_string(),
        );
        cmd.env(
            "FACTORY_PI_BIN",
            self.bin.join("pi").to_string_lossy().to_string(),
        );
        cmd.env(
            "FACTORY_CHECK_ADAPTER",
            self.adapter.to_string_lossy().to_string(),
        );
        cmd.env(
            "FACTORY_STATE_DIR",
            self.state.to_string_lossy().to_string(),
        );
        // Fake toolchain shadows the real one via PATH.
        cmd.env(
            "PATH",
            format!("{}:{}", self.bin.display(), Self::real_path()),
        );
        // Never inherit the developer's own overrides.
        cmd.current_dir(&self.dir);
        cmd
    }

    fn output(&self, json: bool, extra_envs: &[(&str, &str)]) -> std::process::Output {
        let mut cmd = self.cmd(json);
        for (k, v) in extra_envs {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn text(out: &std::process::Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }
}

const EXPECTED_NAMES: &[&str] = &[
    "gh-installed",
    "gh-auth",
    "gh-repo-access",
    "git-identity",
    "state-dirs",
    "adapter-present",
    "adapter-executable",
    "toolchain",
    "model-runner",
];

#[test]
fn doctor_all_green_reports_ready_and_exits_zero() {
    let sb = Sandbox::healthy("green");
    let out = sb.output(false, &[]);

    assert!(
        out.status.success(),
        "healthy sandbox must exit 0, got {} output:\n{}",
        out.status,
        Sandbox::text(&out)
    );
    let text = Sandbox::text(&out);
    for name in EXPECTED_NAMES {
        assert!(
            text.contains(name),
            "human report must name check {name}, got:\n{text}"
        );
    }
    assert!(
        text.contains("ready"),
        "green report must say ready:\n{text}"
    );
}

#[test]
fn doctor_missing_gh_fails_with_install_hint() {
    let sb = Sandbox::healthy("no-gh");
    let mut cmd = sb.cmd(false);
    cmd.env("FACTORY_GH_BIN", "/nonexistent/gh-fixture");
    let out = cmd.output().unwrap();

    assert!(!out.status.success(), "missing gh must exit nonzero");
    let text = Sandbox::text(&out);
    assert!(
        text.contains("gh-installed") && text.contains("fix:"),
        "must name the check and print a fix, got:\n{text}"
    );
}

#[test]
fn doctor_unauthenticated_gh_names_login_fix() {
    let sb = Sandbox::healthy("no-auth");
    let out = sb.output(false, &[("FIXTURE_GH_AUTH", "fail")]);

    assert!(!out.status.success());
    let text = Sandbox::text(&out);
    assert!(
        text.contains("gh-auth") && text.contains("gh auth login"),
        "must print the exact login fix, got:\n{text}"
    );
}

#[test]
fn doctor_repo_inaccessible_fails() {
    let sb = Sandbox::healthy("no-repo");
    let out = sb.output(false, &[("FIXTURE_GH_REPO", "fail")]);

    assert!(!out.status.success());
    assert!(
        Sandbox::text(&out).contains("gh-repo-access"),
        "must name gh-repo-access, got:\n{}",
        Sandbox::text(&out)
    );
}

#[test]
fn doctor_missing_git_identity_names_config_fix() {
    let sb = Sandbox::healthy("no-identity");
    let out = sb.output(false, &[("FIXTURE_GIT_IDENTITY", "fail")]);

    assert!(!out.status.success());
    let text = Sandbox::text(&out);
    assert!(
        text.contains("git-identity") && text.contains("git config"),
        "must name the fix command, got:\n{text}"
    );
}

#[test]
fn doctor_missing_state_dir_names_mkdir_fix() {
    let sb = Sandbox::healthy("no-state");
    fs::remove_dir_all(&sb.state).unwrap();
    let out = sb.output(false, &[]);

    assert!(!out.status.success());
    let text = Sandbox::text(&out);
    assert!(
        text.contains("state-dirs") && text.contains("mkdir -p"),
        "must print the mkdir fix, got:\n{text}"
    );
}

#[test]
fn doctor_missing_adapter_fails() {
    let sb = Sandbox::healthy("no-adapter");
    fs::remove_file(&sb.adapter).unwrap();
    let out = sb.output(false, &[]);

    assert!(!out.status.success());
    let text = Sandbox::text(&out);
    assert!(
        text.contains("adapter-present"),
        "must name adapter-present, got:\n{text}"
    );
}

#[test]
fn doctor_non_executable_adapter_names_chmod_fix() {
    let sb = Sandbox::healthy("noexec-adapter");
    fs::set_permissions(&sb.adapter, fs::Permissions::from_mode(0o644)).unwrap();
    let out = sb.output(false, &[]);

    assert!(!out.status.success());
    let text = Sandbox::text(&out);
    assert!(
        text.contains("adapter-executable") && text.contains("chmod +x"),
        "must print the chmod fix, got:\n{text}"
    );
}

#[test]
fn doctor_missing_toolchain_fails() {
    let sb = Sandbox::healthy("no-toolchain");
    // PATH without cargo: sandbox bin minus the cargo fixture.
    let bare = sb.dir.join("bare");
    fs::create_dir_all(&bare).unwrap();
    let mut cmd = sb.cmd(false);
    cmd.env("PATH", bare.to_string_lossy().to_string());
    let out = cmd.output().unwrap();

    assert!(!out.status.success());
    let text = Sandbox::text(&out);
    assert!(
        text.contains("toolchain"),
        "must name toolchain, got:\n{text}"
    );
}

#[test]
fn doctor_unreachable_runner_fails() {
    let sb = Sandbox::healthy("no-runner");
    let mut cmd = sb.cmd(false);
    cmd.env("FACTORY_PI_BIN", "/nonexistent/pi-fixture");
    let out = cmd.output().unwrap();

    assert!(!out.status.success());
    assert!(
        Sandbox::text(&out).contains("model-runner"),
        "must name model-runner, got:\n{}",
        Sandbox::text(&out)
    );
}

#[test]
fn doctor_json_emits_stable_schema() {
    let sb = Sandbox::healthy("json-green");
    let out = sb.output(true, &[]);

    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).expect("stdout must be JSON");
    assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
    let checks = v
        .get("checks")
        .and_then(|c| c.as_array())
        .expect("checks array");
    let names: Vec<&str> = checks
        .iter()
        .filter_map(|c| c.get("name").and_then(|n| n.as_str()))
        .collect();
    assert_eq!(names, EXPECTED_NAMES);
    for c in checks {
        assert!(c.get("ok").and_then(|b| b.as_bool()) == Some(true));
        c.get("detail").and_then(|d| d.as_str()).expect("detail");
        c.get("hint").and_then(|h| h.as_str()).expect("hint");
    }
}

#[test]
fn doctor_json_hints_on_every_failure() {
    let sb = Sandbox::healthy("json-broken");
    fs::remove_dir_all(&sb.state).unwrap();
    let out = sb.output(
        true,
        &[
            ("FIXTURE_GH_AUTH", "fail"),
            ("FIXTURE_GIT_IDENTITY", "fail"),
        ],
    );

    assert!(!out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).expect("stdout must be JSON");
    assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(false));
    let checks = v
        .get("checks")
        .and_then(|c| c.as_array())
        .expect("checks array");
    assert_eq!(checks.len(), EXPECTED_NAMES.len());
    for c in checks {
        if c.get("ok").and_then(|b| b.as_bool()) != Some(true) {
            let hint = c.get("hint").and_then(|h| h.as_str()).unwrap_or("");
            assert!(!hint.is_empty(), "every failing check needs a hint: {c}");
        }
    }
    // Exit code answers red: nonzero iff anything blocks.
    let failing: Vec<&Value> = checks
        .iter()
        .filter(|c| c.get("ok").and_then(|b| b.as_bool()) != Some(true))
        .collect();
    assert!(!failing.is_empty());
}

#[test]
fn doctor_runs_with_zero_args_in_any_directory() {
    // No seams pointed at the sandbox adapter: bare temp cwd, real PATH.
    // Must still exit with a code and print a per-check report, never crash.
    let dir = scratch_dir("anywhere");
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.arg("doctor").current_dir(&dir);
    cmd.env_remove("FACTORY_CHECK_ADAPTER");
    cmd.env_remove("FACTORY_GH_BIN");
    cmd.env_remove("FACTORY_GIT_BIN");
    cmd.env_remove("FACTORY_PI_BIN");
    let out = cmd.output().unwrap();

    let text = Sandbox::text(&out);
    assert!(
        text.contains("gh-installed") && text.contains("model-runner"),
        "report must cover the catalogue even in a bare dir, got:\n{text}"
    );
    assert!(
        !out.status.success(),
        "bare dir without adapter must exit nonzero"
    );
}

#[test]
fn doctor_filters_mise_noise_from_gh_probes() {
    // gh probes must go through the mise-noise filter: a noisy gh that
    // prints version-manager chatter before its real output must still
    // yield clean details and a parsed repo name.
    let sb = Sandbox::healthy("gh-noise");
    let noisy = r#"
case "$1" in
  --version) echo "mise ~/.config/mise/config.toml tools: gh@2.102.0"; echo "gh version 9.9.9-test"; exit 0;;
  auth) exit 0;;
  repo) echo "mise ~/.config/mise/config.toml tools: gh@2.102.0"; echo '{"nameWithOwner":"o/r"}'; exit 0;;
  *) exit 0;;
esac
"#;
    write_script(&sb.bin, "gh", noisy);
    let out = sb.output(false, &[]);
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "noisy-but-healthy gh must still exit 0, got {}:\n{report}",
        out.status
    );
    assert!(
        report.contains("gh version 9.9.9-test"),
        "gh-installed detail must show the version, got:\n{report}"
    );
    assert!(
        !report.contains("mise "),
        "mise noise must never leak into the report, got:\n{report}"
    );
    assert!(
        report.contains("o/r"),
        "gh-repo-access must parse the repo name through the noise, got:\n{report}"
    );
}

#[test]
fn doctor_missing_cache_dir_names_mkdir_fix() {
    let sb = Sandbox::healthy("no-cache");
    fs::remove_dir_all(sb.dir.join("factory").join(".cache")).unwrap();
    let out = sb.output(false, &[]);

    assert!(!out.status.success());
    let text = Sandbox::text(&out);
    assert!(
        text.contains("state-dirs") && text.contains("mkdir -p"),
        "must name state-dirs and print the mkdir fix, got:\n{text}"
    );
}

#[test]
fn doctor_stale_worktree_metadata_fails_with_prune_hint() {
    let sb = Sandbox::healthy("stale-wt");
    let marker = sb.dir.join("stale-worktree");
    fs::write(&marker, "stale").unwrap();
    let out = sb.output(
        false,
        &[("STALE_MARKER", marker.to_string_lossy().as_ref())],
    );

    assert!(!out.status.success());
    let text = Sandbox::text(&out);
    assert!(
        text.contains("state-dirs") && text.contains("git worktree prune"),
        "must name state-dirs and hint the prune command, got:\n{text}"
    );
}

#[test]
fn doctor_missing_rustc_fails_with_toolchain_hint() {
    let sb = Sandbox::healthy("no-rustc");
    let out = sb.output(false, &[("FIXTURE_RUSTC", "fail")]);

    assert!(!out.status.success());
    let text = Sandbox::text(&out);
    assert!(
        text.contains("toolchain") && text.contains("rustc"),
        "cargo-ok/rustc-missing must fail naming the toolchain, got:\n{text}"
    );
}
