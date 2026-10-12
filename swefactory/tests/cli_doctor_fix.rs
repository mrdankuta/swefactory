use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique scratch dir under the OS temp dir (std-only, no extra deps).
fn scratch_dir(tag: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "swefactory-doctor-fix-{}-{}-{}",
        std::process::id(),
        id,
        tag
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn text(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Fake gh: read probes succeed; every invocation is appended to $CALL_LOG_GH.
const FAKE_GH: &str = r#"
if [ -n "$CALL_LOG_GH" ]; then echo "$@" >> "$CALL_LOG_GH"; fi
case "$1" in
  --version) echo "gh version 9.9.9-test"; exit 0;;
  auth) if [ "$FIXTURE_GH_AUTH" = "fail" ]; then echo "not logged in" >&2; exit 1; fi; exit 0;;
  repo) if [ "$FIXTURE_GH_REPO" = "fail" ]; then echo "repo not found" >&2; exit 1; fi; echo '{"nameWithOwner":"o/r"}'; exit 0;;
  *) exit 0;;
esac
"#;

/// Fake git: identity + worktree prune emulation; logs to $CALL_LOG_GIT.
/// $STALE_MARKER present  => `worktree prune -n` reports a stale entry.
/// `worktree prune` removes the marker (the repair).
const FAKE_GIT: &str = r#"
if [ -n "$CALL_LOG_GIT" ]; then echo "$@" >> "$CALL_LOG_GIT"; fi
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
if [ "$1" = "worktree" ] && [ "$2" = "prune" ]; then
  if [ -n "$STALE_MARKER" ]; then rm -f "$STALE_MARKER"; fi
  exit 0
fi
exit 0
"#;

const FAKE_PI: &str = r#"
if [ -n "$CALL_LOG_PI" ]; then echo "$@" >> "$CALL_LOG_PI"; fi
if [ "$FIXTURE_PI" = "fail" ]; then echo "no runner" >&2; exit 1; fi
echo "pi version 9.9.9-test"; exit 0
"#;

const FAKE_CARGO: &str = r#"
if [ -n "$CALL_LOG_CARGO" ]; then echo "$@" >> "$CALL_LOG_CARGO"; fi
if [ "$FIXTURE_CARGO" = "fail" ]; then echo "no cargo" >&2; exit 1; fi
echo "cargo 9.9.9-test"; exit 0
"#;

const FAKE_RUSTC: &str = r#"
echo "rustc 9.9.9-test"; exit 0
"#;

struct Sandbox {
    dir: PathBuf,
    bin: PathBuf,
    /// State dir path: deliberately NOT created (missing-dirs repair case).
    state: PathBuf,
    gh_log: PathBuf,
    git_log: PathBuf,
    cargo_log: PathBuf,
    pi_log: PathBuf,
}

impl Sandbox {
    /// Bare sandbox: healthy probes, but no state dir, no adapter, no
    /// project files — everything --fix is allowed to repair is broken.
    fn broken(tag: &str) -> Self {
        let dir = scratch_dir(tag);
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_script(&bin, "gh", FAKE_GH);
        write_script(&bin, "git", FAKE_GIT);
        write_script(&bin, "pi", FAKE_PI);
        write_script(&bin, "cargo", FAKE_CARGO);
        write_script(&bin, "rustc", FAKE_RUSTC);
        Self {
            state: dir.join("state"),
            gh_log: dir.join("gh-calls.log"),
            git_log: dir.join("git-calls.log"),
            cargo_log: dir.join("cargo-calls.log"),
            pi_log: dir.join("pi-calls.log"),
            dir,
            bin,
        }
    }

    fn real_path() -> String {
        std::env::var("PATH").unwrap_or_default()
    }

    fn cmd(&self, fix: bool, json: bool) -> Command {
        let mut cmd = Command::cargo_bin("factory").unwrap();
        if json {
            cmd.arg("--json");
        }
        cmd.arg("doctor");
        if fix {
            cmd.arg("--fix");
        }
        // Hermetic seams: never touch the developer's real gh/git/pi.
        // NOTE: no FACTORY_CHECK_ADAPTER — discovery must find
        // <cwd>/scripts/factory-check so --fix can re-seed it.
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
            "FACTORY_STATE_DIR",
            self.state.to_string_lossy().to_string(),
        );
        cmd.env("CALL_LOG_GH", self.gh_log.to_string_lossy().to_string());
        cmd.env("CALL_LOG_GIT", self.git_log.to_string_lossy().to_string());
        cmd.env(
            "CALL_LOG_CARGO",
            self.cargo_log.to_string_lossy().to_string(),
        );
        cmd.env("CALL_LOG_PI", self.pi_log.to_string_lossy().to_string());
        cmd.env(
            "PATH",
            format!("{}:{}", self.bin.display(), Self::real_path()),
        );
        cmd.env_remove("FACTORY_CHECK_ADAPTER");
        cmd.current_dir(&self.dir);
        cmd
    }

    fn output(&self, fix: bool, json: bool, extra_envs: &[(&str, &str)]) -> std::process::Output {
        let mut cmd = self.cmd(fix, json);
        for (k, v) in extra_envs {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn log_lines(&self, path: &std::path::Path) -> Vec<String> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| l.to_string())
            .collect()
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

/// AC: --fix repairs dirs, metadata, and project files; report shows what changed.
#[test]
fn fix_repairs_broken_sandbox_and_reports_repairs() {
    let sb = Sandbox::broken("repairs");
    assert!(!sb.state.exists());
    assert!(!sb.dir.join("scripts/factory-check").exists());

    let out = sb.output(true, false, &[]);
    let report = text(&out);
    assert!(
        out.status.success(),
        "--fix must heal a fully repairable sandbox, got {}:\n{report}",
        out.status
    );
    // Dirs repaired.
    assert!(sb.state.is_dir(), "state dir must be created");
    assert!(
        report.contains("fixed:") && report.contains("state-dirs"),
        "human report must show the dir repair, got:\n{report}"
    );
    // Project files re-seeded through init's path (never overwriting).
    let adapter = sb.dir.join("scripts/factory-check");
    assert!(adapter.is_file(), "adapter must be re-seeded");
    assert!(
        report.contains("scripts/factory-check"),
        "human report must name the re-seeded adapter, got:\n{report}"
    );
    assert!(
        sb.dir.join(".pi/prompts/plan.md").is_file(),
        "prompts must be re-seeded"
    );
}

/// AC: human report and JSON both reflect repairs.
#[test]
fn fix_json_reflects_repairs_with_stable_schema() {
    let sb = Sandbox::broken("json-repairs");
    let out = sb.output(true, true, &[]);

    assert!(out.status.success(), "must exit 0:\n{}", text(&out));
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
        assert_eq!(c.get("ok").and_then(|b| b.as_bool()), Some(true));
        c.get("detail").and_then(|d| d.as_str()).expect("detail");
        c.get("hint").and_then(|h| h.as_str()).expect("hint");
    }
    let fixed = v
        .get("fixed")
        .and_then(|f| f.as_array())
        .expect("fixed array");
    assert!(!fixed.is_empty(), "JSON must list the repairs, got: {v}");
    let joined: Vec<&str> = fixed.iter().filter_map(|f| f.as_str()).collect();
    assert!(
        joined.iter().any(|s| s.contains("state-dirs")),
        "fixed must mention state-dirs, got: {joined:?}"
    );
    assert!(
        joined.iter().any(|s| s.contains("scripts/factory-check")),
        "fixed must mention the adapter, got: {joined:?}"
    );
}

/// AC: second run is a green no-op.
#[test]
fn fix_second_run_is_green_noop() {
    let sb = Sandbox::broken("idempotent");
    let first = sb.output(true, true, &[]);
    assert!(first.status.success());

    let sentinel = sb.dir.join(".pi/prompts/plan.md");
    let before = fs::read(&sentinel).unwrap();

    let second = sb.output(true, true, &[]);
    let report = text(&second);
    assert!(
        second.status.success(),
        "second --fix must still exit 0, got {}:\n{report}",
        second.status
    );
    // stdout is JSON; stderr must not carry warnings about repairs.
    let v: Value = serde_json::from_slice(&second.stdout).expect("stdout must be JSON");
    assert_eq!(v.get("ok").and_then(|b| b.as_bool()), Some(true));
    assert_eq!(
        v.get("fixed").and_then(|f| f.as_array()).map(|a| a.len()),
        Some(0),
        "second run must report zero repairs, got: {v}"
    );
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        before,
        "second run must not rewrite files"
    );

    let human = sb.output(true, false, &[]);
    let human_text = text(&human);
    assert!(
        human_text.contains("nothing to repair"),
        "human report must state the no-op, got:\n{human_text}"
    );
    assert!(
        !human_text.contains("fixed:"),
        "human report must list no repairs on second run, got:\n{human_text}"
    );
}

/// AC: stale worktree metadata pruned (and only once).
#[test]
fn fix_prunes_stale_worktree_metadata_once() {
    let sb = Sandbox::broken("prune");
    let marker = sb.dir.join("stale-worktree");
    fs::write(&marker, "stale").unwrap();

    let out = sb.output(
        true,
        false,
        &[("STALE_MARKER", marker.to_string_lossy().as_ref())],
    );
    let report = text(&out);
    assert!(out.status.success());
    assert!(!marker.exists(), "prune must clear the stale marker");
    assert!(
        report.contains("worktree-metadata") || report.contains("worktree"),
        "human report must show the prune, got:\n{report}"
    );

    // Second run: nothing left to prune.
    let second = sb.output(
        true,
        true,
        &[("STALE_MARKER", marker.to_string_lossy().as_ref())],
    );
    let v: Value = serde_json::from_slice(&second.stdout).expect("JSON");
    let fixed: Vec<&str> = v
        .get("fixed")
        .and_then(|f| f.as_array())
        .map(|a| a.iter().filter_map(|e| e.as_str()).collect())
        .unwrap_or_default();
    assert!(
        fixed.iter().all(|s| !s.contains("worktree")),
        "second run must not re-report a prune, got: {fixed:?}"
    );

    // Negative: prune path never touches remotes or branches.
    for line in sb.log_lines(&sb.git_log) {
        for forbidden in [
            "push",
            "fetch",
            "pull",
            "remote",
            "commit",
            "worktree add",
            "worktree remove",
            "worktree move",
        ] {
            assert!(
                !line.contains(forbidden),
                "git call {line:?} must never touch remotes/branches"
            );
        }
    }
}

/// AC negative: issues, remotes, toolchain installs, auth flows never touched.
#[test]
fn fix_never_touches_issues_remotes_toolchains_or_auth() {
    let sb = Sandbox::broken("negative");
    let out = sb.output(true, false, &[]);
    assert!(out.status.success(), "must heal:\n{}", text(&out));

    for line in sb.log_lines(&sb.gh_log) {
        for forbidden in [
            "issue comment",
            "issue create",
            "issue edit",
            "label",
            "auth login",
            "auth refresh",
            "repo create",
            "repo fork",
            "pr create",
            "pr merge",
            "api",
        ] {
            assert!(
                !line
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .contains(forbidden),
                "gh call {line:?} must stay read-only"
            );
        }
    }
    for line in sb.log_lines(&sb.cargo_log) {
        assert!(
            !line.contains("install") && !line.contains("uninstall"),
            "toolchain must never be installed, got cargo call {line:?}"
        );
    }
    let report = text(&out);
    assert!(
        !report.contains("gh auth login"),
        "green report must not demand auth, got:\n{report}"
    );
    // gh was only probed (version / auth status / repo view), never a write.
    let gh_lines = sb.log_lines(&sb.gh_log);
    assert!(
        gh_lines.iter().all(|l| l.starts_with("--version")
            || l.starts_with("auth status")
            || l.starts_with("repo view")),
        "gh calls must be read probes only, got: {gh_lines:?}"
    );
}

/// Spec story 11: diagnosing without --fix never changes anything.
#[test]
fn plain_doctor_stays_readonly() {
    let sb = Sandbox::broken("readonly");
    let out = sb.output(false, false, &[]);

    assert!(
        !out.status.success(),
        "broken sandbox must still report red without --fix"
    );
    assert!(
        !sb.state.exists(),
        "plain doctor must not create the state dir"
    );
    assert!(
        !sb.dir.join("scripts/factory-check").exists(),
        "plain doctor must not re-seed the adapter"
    );
    assert!(
        !sb.dir.join(".pi/prompts/plan.md").exists(),
        "plain doctor must not seed project files"
    );
    for line in sb.log_lines(&sb.git_log) {
        assert!(
            !line.contains("prune"),
            "plain doctor must not prune, got git call {line:?}"
        );
    }
}

/// --fix reports but never heals auth or toolchain (out of scope per spec).
#[test]
fn fix_reports_but_never_heals_auth_or_toolchain() {
    let sb = Sandbox::broken("unhealable");
    let bare = sb.dir.join("bare-path");
    fs::create_dir_all(&bare).unwrap();
    // PATH without cargo/rustc, auth failing: nothing --fix may touch.
    let mut cmd = sb.cmd(true, false);
    cmd.env("PATH", bare.to_string_lossy().to_string());
    cmd.env("FIXTURE_GH_AUTH", "fail");
    let out = cmd.output().unwrap();
    let report = text(&out);

    assert!(
        !out.status.success(),
        "unrepairable failures must stay red, got:\n{report}"
    );
    assert!(
        report.contains("gh auth login"),
        "must keep the exact login hint, got:\n{report}"
    );
    assert!(
        report.contains("toolchain"),
        "must keep the toolchain failure, got:\n{report}"
    );
    // ... yet safe local repairs still happened and are reported.
    assert!(sb.state.is_dir(), "state dir repair must still apply");
    assert!(
        report.contains("fixed:"),
        "report must show the repairs that did apply, got:\n{report}"
    );
    for line in sb.log_lines(&sb.gh_log) {
        assert!(
            !line.contains("auth login") && !line.contains("auth refresh"),
            "auth flows must never start, got gh call {line:?}"
        );
    }
}

/// Non-executable adapter is repaired with chmod +x (the hint's own command).
#[test]
fn fix_chmods_non_executable_adapter() {
    let sb = Sandbox::broken("chmod");
    let adapter = sb.dir.join("scripts/factory-check");
    fs::create_dir_all(adapter.parent().unwrap()).unwrap();
    fs::write(&adapter, "#!/bin/sh\necho ok\n").unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o644)).unwrap();

    let out = sb.output(true, true, &[]);
    assert!(out.status.success(), "must heal:\n{}", text(&out));
    assert!(
        fs::metadata(&adapter).unwrap().permissions().mode() & 0o111 != 0,
        "adapter must be executable after --fix"
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let fixed: Vec<&str> = v
        .get("fixed")
        .and_then(|f| f.as_array())
        .map(|a| a.iter().filter_map(|e| e.as_str()).collect())
        .unwrap_or_default();
    assert!(
        fixed.iter().any(|s| s.contains("chmod")),
        "repairs must record the chmod, got: {fixed:?}"
    );
}
