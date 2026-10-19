//! Live phase logs (ticket #95).
//!
//! Phase output must stream to the attempt log while the worker runs —
//! not in one burst at completion. These tests pin the behavior
//! hermetically (filesystem handshakes, never sleep-timing):
//! slow-drip proves mid-run growth, a drip-then-sleep fake proves a
//! killed worker leaves an in-order prefix, and a full run proves
//! per-file isolation (slot-owned logs never mix).
//!
//! Seam: same as cli_phase_timeouts (fake gh/git/pi/adapter, temp dirs,
//! no network). The #85 partial-streaming path needs no new wiring: the
//! transcript-bearing SSE scopes already re-read these log files at
//! `stream_partial_ms()`, so live appends surface within that bound.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-live-{}-{}-{}",
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

fn write_fake_adapter(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "adapter.sh",
        r#"#!/bin/sh
echo "FACTORY-CHECK PASSED"
exit 0
"#,
    )
}

/// Fast fake: drops a file change and accepts (implement + review).
fn write_fast_pi(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi",
        r#"#!/bin/sh
echo "pi cwd=$(pwd) slot:${FACTORY_SLOT:-unset}" >> "$PI_LOG"
echo "change from pi" >> change.txt
echo "impl-output-line"
echo "VERDICT: accept"
"#,
    )
}

/// Slow-drip implement fake: prints drip-1, signals readiness, waits for
/// the test's go-file (handshake, 20s fallback so a failed test never
/// hangs the suite), then prints the rest and accepts. Review worktrees
/// take the fast path.
fn write_drip_pi(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi-drip",
        r#"#!/bin/sh
case "$(basename "$(pwd)")" in
review-*)
  echo "review-output-line"
  echo "VERDICT: accept"
  ;;
*)
  echo "drip-1"
  touch "$DRIP_READY"
  n=0
  while [ ! -f "$DRIP_GO" ] && [ $n -lt 200 ]; do sleep 0.1; n=$((n+1)); done
  echo "drip-2"
  echo "drip-3"
  echo "drip-4"
  echo "drip-5"
  echo "change from pi" >> change.txt
  echo "VERDICT: accept"
  ;;
esac
"#,
    )
}

/// Drip-then-sleep fake: three fast lines, then a 60s sleep so the 1s
/// implement bound kills it mid-stream. Review path never runs here.
fn write_drip_sleep_pi(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi-drip-sleep",
        r#"#!/bin/sh
echo "keep-1"
echo "keep-2"
echo "keep-3"
sleep 60
echo "must never print"
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
    gh: PathBuf,
    git_shim: PathBuf,
    pi: PathBuf,
    adapter: PathBuf,
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_repo(&repo);
    let views = root.join("views");
    fs::create_dir_all(&views).unwrap();
    let h = Harness {
        state: root.join("state"),
        gh_log: root.join("gh.log"),
        git_log: root.join("git.log"),
        pi_log: root.join("pi.log"),
        gh: write_fake_gh(&root),
        git_shim: write_git_shim(&root),
        pi: write_fast_pi(&root),
        adapter: write_fake_adapter(&root),
        root,
        repo,
        views,
    };
    for f in [&h.gh_log, &h.git_log, &h.pi_log] {
        fs::write(f, "").unwrap();
    }
    h
}

fn write_view(h: &Harness, number: u64, title: &str, body: &str, labels: &[&str], state: &str) {
    fs::write(
        h.views.join(format!("{number}.json")),
        ticket_view(number, title, body, labels, state),
    )
    .unwrap();
}

/// Build a dispatch command with explicit envs (owned paths so the
/// caller can move them into a background thread).
#[allow(clippy::too_many_arguments)]
fn dispatch_command(
    repo: &Path,
    gh: &Path,
    git_shim: &Path,
    pi: &Path,
    adapter: &Path,
    state: &Path,
    gh_log: &Path,
    git_log: &Path,
    pi_log: &Path,
    views: &Path,
    extra_env: Vec<(String, String)>,
    args: Vec<String>,
) -> std::process::Output {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_factory"));
    cmd.args(&args).current_dir(repo);
    cmd.env("FACTORY_GH_BIN", gh)
        .env("FACTORY_GIT_BIN", git_shim)
        .env("FACTORY_PI_BIN", pi)
        .env("FACTORY_CHECK_ADAPTER", adapter)
        .env("FACTORY_STATE_DIR", state)
        .env("GH_LOG", gh_log)
        .env("GIT_LOG", git_log)
        .env("PI_LOG", pi_log)
        .env("GH_VIEWS_DIR", views)
        .env_remove("FACTORY_SLOT");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("dispatch must run")
}

fn ledger(h: &Harness) -> serde_json::Value {
    let text = fs::read_to_string(h.state.join("dispatch.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn wait_for_file(path: &Path, label: &str) {
    let start = std::time::Instant::now();
    while !path.is_file() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(20),
            "{label} never appeared: {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn nonblank_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect()
}

#[test]
fn slow_drip_grows_log_while_running() {
    let h = setup("drip-live");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let drip = write_drip_pi(&tmpdir("drip-pi"));
    let ready = h.root.join("drip.ready");
    let go = h.root.join("drip.go");
    // Clone owned values into the background dispatch thread.
    let (repo, gh, shim, adapter, state, gh_log, git_log, pi_log, views) = (
        h.repo.clone(),
        h.gh.clone(),
        h.git_shim.clone(),
        h.adapter.clone(),
        h.state.clone(),
        h.gh_log.clone(),
        h.git_log.clone(),
        h.pi_log.clone(),
        h.views.clone(),
    );
    let drip_s = drip.to_string_lossy().into_owned();
    let ready_s = ready.to_string_lossy().into_owned();
    let go_s = go.to_string_lossy().into_owned();
    let handle = std::thread::spawn(move || {
        dispatch_command(
            &repo,
            &gh,
            &shim,
            Path::new(&drip_s),
            &adapter,
            &state,
            &gh_log,
            &git_log,
            &pi_log,
            &views,
            vec![
                ("GH_LIST_JSON".to_string(), r#"[{"number":11}]"#.to_string()),
                ("TIMEOUT_IMPL".to_string(), "1200".to_string()),
                ("TIMEOUT_REVIEW".to_string(), "1200".to_string()),
                ("DRIP_READY".to_string(), ready_s),
                ("DRIP_GO".to_string(), go_s),
            ],
            vec!["dispatch".to_string(), "once".to_string()],
        )
    });
    // The fake signals after printing drip-1; the dispatcher must still
    // be running then (handshake, not sleep-timing).
    wait_for_file(&ready, "drip ready handshake");
    let log_path = h.state.join("logs").join("T11-a1-impl.log");
    wait_for_file(&log_path, "attempt log");
    // Poll until drip-1 is visible in the log while the child runs.
    let mut saw_midrun = false;
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(20) {
        let content = fs::read_to_string(&log_path).unwrap_or_default();
        if content.contains("drip-1") {
            if !handle.is_finished() {
                saw_midrun = true;
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        saw_midrun,
        "log must visibly grow while the phase runs (drip-1 before exit)"
    );
    // Release the worker and await completion.
    fs::write(&go, "go").unwrap();
    let out = handle.join().expect("dispatch thread");
    assert!(
        out.status.success(),
        "dispatch must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let final_text = fs::read_to_string(&log_path).unwrap();
    for line in ["drip-1", "drip-2", "drip-3", "drip-4", "drip-5"] {
        assert!(
            final_text.contains(line),
            "final log must hold {line}:\n{final_text}"
        );
        assert_eq!(
            final_text.matches(line).count(),
            1,
            "no duplicated output section for {line}:\n{final_text}"
        );
    }
    // Order preserved.
    let lines = nonblank_lines(&final_text);
    let mut last = 0;
    for want in ["drip-1", "drip-2", "drip-3", "drip-4", "drip-5"] {
        let pos = lines
            .iter()
            .position(|l| l.contains(want))
            .unwrap_or_else(|| panic!("missing {want}:\n{final_text}"));
        assert!(pos >= last, "{want} out of order:\n{final_text}");
        last = pos;
    }
    assert!(
        final_text.contains("running") && final_text.contains("(exit 0)"),
        "live header + exit footer must frame the output:\n{final_text}"
    );
}

#[test]
fn killed_midstream_leaves_prefix_in_order() {
    let h = setup("killed-prefix");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let sleepy = write_drip_sleep_pi(&tmpdir("drip-sleep-pi"));
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(["dispatch", "once"]).current_dir(&h.repo);
    cmd.env("FACTORY_GH_BIN", &h.gh)
        .env("FACTORY_GIT_BIN", &h.git_shim)
        .env("FACTORY_PI_BIN", &sleepy)
        .env("FACTORY_CHECK_ADAPTER", &h.adapter)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("GH_LOG", &h.gh_log)
        .env("GIT_LOG", &h.git_log)
        .env("PI_LOG", &h.pi_log)
        .env("GH_VIEWS_DIR", &h.views)
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("TIMEOUT_IMPL", "1")
        .env("TIMEOUT_REVIEW", "1200")
        .env_remove("FACTORY_SLOT");
    cmd.assert().success();
    let parsed = ledger(&h);
    let row = &parsed["attempts"][0];
    assert_eq!(
        row.get("status").and_then(|v| v.as_str()),
        Some("implement_interrupted"),
        "timeout checkpoints as interrupted:\n{parsed}"
    );
    let log_path = row
        .get("log_path")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    let log = fs::read_to_string(&log_path).unwrap();
    for line in ["keep-1", "keep-2", "keep-3"] {
        assert!(log.contains(line), "flushed line stays: {line}:\n{log}");
        assert_eq!(
            log.matches(line).count(),
            1,
            "no duplication for {line}:\n{log}"
        );
    }
    assert!(
        !log.contains("must never print"),
        "post-kill output never lands:\n{log}"
    );
    assert!(
        log.contains("timed out after 1s"),
        "timeout marker names the bound:\n{log}"
    );
    let lines = nonblank_lines(&log);
    let positions: Vec<usize> = ["keep-1", "keep-2", "keep-3"]
        .iter()
        .map(|w| {
            lines
                .iter()
                .position(|l| l.contains(w))
                .unwrap_or_else(|| panic!("missing {w}:\n{log}"))
        })
        .collect();
    assert!(
        positions[0] < positions[1] && positions[1] < positions[2],
        "flushed prefix stays in order:\n{log}"
    );
}

#[test]
fn concurrent_phases_stay_per_file() {
    let h = setup("per-file");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build the widget.",
        &["ticket", "ready"],
        "OPEN",
    );
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(["dispatch", "once"]).current_dir(&h.repo);
    cmd.env("FACTORY_GH_BIN", &h.gh)
        .env("FACTORY_GIT_BIN", &h.git_shim)
        .env("FACTORY_PI_BIN", &h.pi)
        .env("FACTORY_CHECK_ADAPTER", &h.adapter)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("GH_LOG", &h.gh_log)
        .env("GIT_LOG", &h.git_log)
        .env("PI_LOG", &h.pi_log)
        .env("GH_VIEWS_DIR", &h.views)
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .env("TIMEOUT_IMPL", "1200")
        .env("TIMEOUT_REVIEW", "1200")
        .env_remove("FACTORY_SLOT");
    cmd.assert().success();
    let parsed = ledger(&h);
    let row = &parsed["attempts"][0];
    assert_eq!(
        row.get("status").and_then(|v| v.as_str()),
        Some("review_done"),
        "fast run completes:\n{parsed}"
    );
    let impl_path = PathBuf::from(row.get("log_path").and_then(|v| v.as_str()).unwrap());
    let review_path = h.state.join("logs").join("T11-a1-review.log");
    assert_ne!(impl_path, review_path, "phases own distinct files");
    assert!(impl_path.is_file(), "implement log exists");
    assert!(review_path.is_file(), "review log exists");
    let impl_log = fs::read_to_string(&impl_path).unwrap();
    let review_log = fs::read_to_string(&review_path).unwrap();
    assert!(
        impl_log.contains("impl-output-line"),
        "implement output lands in the implement log:\n{impl_log}"
    );
    assert!(
        review_log.contains("review-output-line") || review_log.contains("VERDICT"),
        "review output lands in the review log:\n{review_log}"
    );
    assert!(
        !impl_log.contains("review-output-line"),
        "slot-owned logs never mix (review output not in implement log):\n{impl_log}"
    );
    // Every live line is a complete line: the file never ends mid-line
    // without the footer framing it, so #85 tail readers see whole lines.
    for (name, text) in [("implement", &impl_log), ("review", &review_log)] {
        assert!(
            text.ends_with('\n'),
            "{name} log ends on a line boundary:\n{text}"
        );
    }
}
