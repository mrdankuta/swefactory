//! Supervised worker pool (ticket #92): concurrency, reap, spend, plan.
//!
//! Seam: argv in -> exit code + stdout/stderr out + state-dir files and
//! subprocess argv logs out. `gh` is a fake script (no network), `git` is a
//! logging shim over the real binary, implementer (`pi`) and check adapter
//! are fakes. Hermetic: no network, no spend, no wall-clock races —
//! simultaneity is proven by overlapping marker-file windows, never timing.

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Process-env access for UI-server tests: FACTORY_STATE_DIR is global,
/// so every test that points it at a scratch dir serializes here —
/// including the direct-handler test further below.
static UI_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-pool-{}-{}-{}",
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
    git_repo(&repo);
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

fn dispatch_cmd(h: &Harness, args: &[&str]) -> Command {
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
        .env_remove("FACTORY_SLOT")
        // Hermetic budgets too: the attempt budget defaults to 2 and tests
        // that need another budget set MAX_ATTEMPTS explicitly.
        .env_remove("MAX_CONCURRENT")
        .env_remove("FACTORY_MAX_CONCURRENT")
        .env_remove("MAX_ATTEMPTS")
        .env_remove("TIMEOUT_IMPL")
        .env_remove("TIMEOUT_REVIEW");
    cmd
}

fn gh_log(h: &Harness) -> String {
    fs::read_to_string(&h.gh_log).unwrap()
}

/// Serial default (concurrency 1): the pass reads exactly like the
/// pre-pool pass — byte-identical full stdout (finding #5: ordered
/// subsequence + banned vocab could not catch extra lines), same ledger
/// transition. The two train lines name the auto-cut train, which embeds
/// today's date: the branch is read back from the repo so the assertion
/// stays exact without pinning the clock, while still proving both lines
/// name the SAME train.
#[test]
fn serial_default_pass_reads_as_before() {
    let h = setup("serial-default");
    write_view(
        &h,
        11,
        "Demo widget",
        "Build it.",
        &["ticket", "ready"],
        "OPEN",
    );
    let out = dispatch_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let trains: Vec<String> = std::process::Command::new("git")
        .args([
            "branch",
            "--list",
            "integration/*",
            "--format=%(refname:short)",
        ])
        .current_dir(&h.repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
        .stdout
        .split(|b| *b == b'\n')
        .filter_map(|b| {
            let s = String::from_utf8_lossy(b).trim().to_string();
            (!s.is_empty()).then_some(s)
        })
        .collect();
    assert_eq!(
        trains.len(),
        1,
        "serial pass auto-cuts exactly one train: {trains:?}"
    );
    let train = &trains[0];
    let expected = format!(
        "dispatch: #11 'Demo widget' (attempt 1)\n  implement: pi -p (slot T11-a1, timeout 1800s)\n  verify: check adapter\n  review: pi -p (timeout 1200s)\n  auto-cut train {train} (no open train)\n  ACCEPTED + INTEGRATED into {train} (auto-cut) — #11\ndispatch: pass complete (1 attempted)\n"
    );
    assert_eq!(
        stdout, expected,
        "serial default must be byte-identical to the pre-pool pass"
    );
    for banned in [
        "concurrency",
        "wave",
        "waiting for a free slot",
        "worker plan",
        "Worker slots",
    ] {
        assert!(
            !stdout.contains(banned),
            "serial default must not mention {banned:?}:\n{stdout}"
        );
    }
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].get("status").and_then(|v| v.as_str()),
        Some("review_done")
    );
}

/// Two ready tickets with `--concurrency 2` run their implement phases in
/// overlapping windows (marker files), each in its own isolated slot.
#[test]
fn two_workers_run_simultaneously_in_isolated_slots() {
    let h = setup("fan-out");
    let markers = h.repo.parent().unwrap().join("markers");
    fs::create_dir_all(&markers).unwrap();
    // Overlap-proving pi: implement-phase invocations open a window
    // (started file, 5s work, finished file); review invocations skip the
    // sleep (cwd carries the review worktree name).
    let pi = write_exe(
        h.repo.parent().unwrap(),
        "pi-overlap",
        &format!(
            r#"#!/bin/sh
echo "pi cwd=$(pwd) slot:${{FACTORY_SLOT:-unset}} tmpdir:${{TMPDIR:-unset}}" >> "{}"
case "$(pwd)" in *review-*) ;; *) touch "{}/started-${{FACTORY_SLOT:-unset}}"; sleep 5; touch "{}/finished-${{FACTORY_SLOT:-unset}}";; esac
echo "change from pi" >> change.txt
echo "notes"
echo "VERDICT: changes_requested"
"#,
            h.pi_log.display(),
            markers.display(),
            markers.display(),
        ),
    );
    write_view(&h, 11, "First", "Build one.", &["ticket", "ready"], "OPEN");
    write_view(&h, 12, "Second", "Build two.", &["ticket", "ready"], "OPEN");
    let mut cmd = dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2"]);
    cmd.env("FACTORY_PI_BIN", &pi)
        .env("GH_LIST_JSON", r#"[{"number":11},{"number":12}]"#);
    cmd.assert().success();

    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 2, "both tickets attempted:\n{ledger}");

    // Overlap: each implement window starts before the other finishes.
    let t = |slot: &str, mark: &str| {
        fs::metadata(markers.join(format!("{mark}-{slot}")))
            .unwrap()
            .modified()
            .unwrap()
    };
    let s11 = t("T11-a1", "started");
    let f11 = t("T11-a1", "finished");
    let s12 = t("T12-a1", "started");
    let f12 = t("T12-a1", "finished");
    assert!(
        s11 < f12 && s12 < f11,
        "implement windows must overlap (s11={s11:?} f11={f11:?} s12={s12:?} f12={f12:?})"
    );

    // Isolation: distinct slots and private temp dirs per worker.
    let pi_log = fs::read_to_string(&h.pi_log).unwrap();
    let mut slots: Vec<String> = pi_log
        .lines()
        .filter_map(|l| {
            l.split("slot:")
                .nth(1)
                .and_then(|s| s.split_whitespace().next().map(|s| s.to_string()))
        })
        .collect();
    slots.sort();
    slots.dedup();
    assert_eq!(slots, vec!["T11-a1".to_string(), "T12-a1".to_string()]);
}

/// A third ticket waits for a free slot with a calm waiting line, then is
/// admitted on the next wave.
#[test]
fn third_ticket_waits_for_free_slot_then_runs() {
    let h = setup("waiting-line");
    for (n, title) in [(11, "One"), (12, "Two"), (13, "Three")] {
        write_view(&h, n, title, "Build it.", &["ticket", "ready"], "OPEN");
    }
    let out = dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2"])
        .env("PI_VERDICT", "changes_requested")
        .env(
            "GH_LIST_JSON",
            r#"[{"number":11},{"number":12},{"number":13}]"#,
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("waiting for a free slot") && stdout.contains("#13"),
        "calm waiting line naming the deferred ticket, got:\n{stdout}"
    );
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 3, "every ticket admitted:\n{ledger}");
    assert!(
        stdout.contains("dispatch: pass complete (3 attempted)"),
        "got:\n{stdout}"
    );
}

/// A worker killed mid-attempt is reaped: its row becomes a resumable
/// checkpoint (never a stuck lease), provable by a simulated stale row.
#[test]
fn killed_worker_reaped_to_resumable_checkpoint() {
    let h = setup("reap-stale");
    write_view(&h, 11, "Demo", "Build it.", &["ticket", "ready"], "OPEN");
    fs::create_dir_all(h.state.join("logs")).unwrap();
    let log_path = h.state.join("logs").join("T11-a1-impl.log");
    fs::write(&log_path, "abandoned mid-implement\n").unwrap();
    let ledger = serde_json::json!({"attempts": [{
        "id": 1, "ticket": 11, "attempt_no": 1,
        "base_commit": "abc", "head_commit": "", "branch": "factory/T11-demo",
        "worktree": "", "status": "impl_running", "exit_code": 0,
        "log_path": log_path.to_string_lossy(), "started_at": "1",
        "finished_at": "", "train_branch": "", "phase": "implement", "resumed": 0,
    }]});
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string(&ledger).unwrap(),
    )
    .unwrap();
    // STOP right behind the reaper: the pass reaps, then halts before any
    // survey — no gh views, no new attempts, no network.
    fs::write(h.state.join("STOP"), "stopped\n").unwrap();
    let out = dispatch_cmd(&h, &["dispatch", "once"])
        .env("GH_LIST_JSON", r#"[{"number":11}]"#)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("stale") && stdout.contains("resumable"),
        "reap must be audible, got:\n{stdout}"
    );
    assert!(
        stdout.contains("STOP file present"),
        "STOP still halts admission, got:\n{stdout}"
    );
    assert!(
        !gh_log(&h).contains("view"),
        "halted loop must not survey:\n{}",
        gh_log(&h)
    );
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let attempts = after.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 1, "reap never duplicates:\n{after}");
    assert_eq!(
        attempts[0].get("status").and_then(|v| v.as_str()),
        Some("implement_interrupted"),
        "resumable checkpoint, never a stuck lease:\n{after}"
    );
}

/// Per-ticket spend is visible in status output in both modes; parallel
/// spend never hides.
#[test]
fn spend_lines_visible_in_status_both_modes() {
    let h = setup("spend");
    write_view(&h, 11, "One", "Build one.", &["ticket", "ready"], "OPEN");
    write_view(&h, 12, "Two", "Build two.", &["ticket", "ready"], "OPEN");
    dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2"])
        .env("PI_VERDICT", "changes_requested")
        .env("GH_LIST_JSON", r#"[{"number":11},{"number":12}]"#)
        .assert()
        .success();

    let out = dispatch_cmd(&h, &["dispatch", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("== spend =="),
        "spend section, got:\n{stdout}"
    );
    assert!(
        stdout.contains("ticket 11") && stdout.contains("ticket 12"),
        "both tickets listed, got:\n{stdout}"
    );
    assert!(stdout.contains("min"), "per-ticket minutes, got:\n{stdout}");
    assert!(
        stdout.contains("spend tracking not yet wired"),
        "cost reads as not-yet-wired, never zero, got:\n{stdout}"
    );

    let json_out = dispatch_cmd(&h, &["--json", "dispatch", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8(json_out).unwrap()).unwrap();
    let spend = parsed.get("spend").and_then(|s| s.as_array()).unwrap();
    assert_eq!(spend.len(), 2, "both tickets in machine spend: {parsed}");
    for row in spend {
        assert!(row.get("ticket").and_then(|t| t.as_u64()).is_some());
        assert!(row.get("total_minutes").is_some(), "minutes key: {row}");
        assert_eq!(
            row.get("cost_text").and_then(|c| c.as_str()),
            Some("spend tracking not yet wired")
        );
    }
}

/// Dry-run shows the worker plan and changes nothing.
#[test]
fn dry_run_shows_worker_plan_and_changes_nothing() {
    let h = setup("dry-plan");
    write_view(&h, 11, "One", "Build one.", &["ticket", "ready"], "OPEN");
    write_view(&h, 12, "Two", "Build two.", &["ticket", "ready"], "OPEN");
    write_view(
        &h,
        13,
        "Three",
        "Build three.",
        &["ticket", "ready"],
        "OPEN",
    );
    write_view(
        &h,
        14,
        "Blocked",
        "Build blocked.\nDepends on: #99",
        &["ticket", "ready"],
        "OPEN",
    );
    write_view(&h, 99, "Blocker", "The blocker.", &["ticket"], "OPEN");
    let out = dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2", "--dry-run"])
        .env(
            "GH_LIST_JSON",
            r#"[{"number":11},{"number":12},{"number":13},{"number":14},{"number":99}]"#,
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("worker plan"),
        "plan header, got:\n{stdout}"
    );
    assert!(
        stdout.contains("#11") && stdout.contains("#12"),
        "would-run tickets, got:\n{stdout}"
    );
    assert!(
        stdout.contains("#13") && stdout.contains("waiting"),
        "waiting ticket named, got:\n{stdout}"
    );
    assert!(
        stdout.contains("#14") && stdout.contains("skip"),
        "blocked ticket named as skipped, got:\n{stdout}"
    );
    assert!(
        !h.state.join("dispatch.json").exists(),
        "dry-run writes no ledger"
    );
    let gh = gh_log(&h);
    for banned in ["comment", "edit"] {
        assert!(
            !gh.lines().any(|l| l.trim() == banned),
            "dry-run must not write (gh issue {banned}):\n{gh}"
        );
    }
    assert!(
        !fs::read_to_string(&h.git_log)
            .unwrap()
            .lines()
            .any(|l| l.contains("worktree add")),
        "dry-run cuts no worktrees"
    );
}

/// The dependency gate precedes slot admission: a ticket with open
/// blockers is never admitted to a free slot — skipped, and named.
#[test]
fn dependency_gate_precedes_slot_admission() {
    let h = setup("dep-gate");
    write_view(
        &h,
        11,
        "Blocked",
        "Build blocked.\nDepends on: #99",
        &["ticket", "ready"],
        "OPEN",
    );
    write_view(&h, 12, "Free", "Build free.", &["ticket", "ready"], "OPEN");
    write_view(&h, 99, "Blocker", "The blocker.", &["ticket"], "OPEN");
    let out = dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2"])
        .env("PI_VERDICT", "changes_requested")
        .env(
            "GH_LIST_JSON",
            r#"[{"number":11},{"number":12},{"number":99}]"#,
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("#11") && stdout.contains("waiting on open deps"),
        "blocked ticket named as skipped, got:\n{stdout}"
    );
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let tickets: Vec<u64> = ledger
        .get("attempts")
        .and_then(|a| a.as_array())
        .unwrap()
        .iter()
        .filter_map(|a| a.get("ticket").and_then(|t| t.as_u64()))
        .collect();
    assert!(
        !tickets.contains(&11),
        "dep-blocked ticket never admitted:\n{ledger}"
    );
    assert!(tickets.contains(&12), "free ticket admitted:\n{ledger}");
}

/// STOP halts admission between parallel waves while the in-flight wave
/// finishes its phases: wave one completes, wave two is never admitted.
#[test]
fn stop_halts_admission_while_inflight_finishes() {
    let h = setup("stop-inflight");
    for (n, title) in [(11, "One"), (12, "Two"), (13, "Three")] {
        write_view(&h, n, title, "Build it.", &["ticket", "ready"], "OPEN");
    }
    // The implementer raises STOP mid-pass from inside the first wave.
    let state_env = h.state.clone();
    let pi = write_exe(
        h.repo.parent().unwrap(),
        "pi-stop",
        &format!(
            r#"#!/bin/sh
echo "pi slot:${{FACTORY_SLOT:-unset}}" >> "{}"
touch "{}/STOP"
echo "change from pi" >> change.txt
echo "notes"
echo "VERDICT: changes_requested"
"#,
            h.pi_log.display(),
            state_env.display(),
        ),
    );
    let out = dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2"])
        .env("FACTORY_PI_BIN", &pi)
        .env(
            "GH_LIST_JSON",
            r#"[{"number":11},{"number":12},{"number":13}]"#,
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("STOP") && stdout.contains("halting admission"),
        "halt line, got:\n{stdout}"
    );
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    let tickets: Vec<u64> = attempts
        .iter()
        .filter_map(|a| a.get("ticket").and_then(|t| t.as_u64()))
        .collect();
    assert_eq!(tickets.len(), 2, "third ticket never admitted:\n{ledger}");
    assert!(!tickets.contains(&13), "wave two halted:\n{ledger}");
}

/// The concurrency level is one setting in both surfaces: the CLI flag and
/// env agree, out-of-bounds is refused with the bounds named, and the
/// dispatcher UI carries the same control over the same setting.
#[test]
fn concurrency_level_configurable_cli_and_ui() {
    use factory_lib::{
        dispatcher_fragment_html, resolve_max_concurrent, set_concurrency_level,
        CONCURRENCY_DEFAULT, CONCURRENCY_MAX, CONCURRENCY_MIN,
    };
    assert_eq!(CONCURRENCY_DEFAULT, 1);
    assert_eq!(CONCURRENCY_MIN, 1);
    // The cap is usable through the shared resolver (flag path).
    assert_eq!(
        factory_lib::resolve_max_concurrent(Some(CONCURRENCY_MAX), &std::env::temp_dir()).unwrap(),
        CONCURRENCY_MAX
    );

    // Rejects out-of-bounds with the bounds named.
    let h = setup("bounds");
    write_view(&h, 11, "One", "Build it.", &["ticket", "ready"], "OPEN");
    for bad in ["0", "99"] {
        dispatch_cmd(&h, &["dispatch", "once", "--concurrency", bad])
            .env("GH_LIST_JSON", r#"[{"number":11}]"#)
            .assert()
            .failure()
            .stderr(predicates::str::contains("1"))
            .stderr(predicates::str::contains("8"));
    }

    // One resolver: flag beats env beats file beats default. Hermetic:
    // ambient slot env (e.g. the dispatcher's own MAX_CONCURRENT) must
    // not leak into these assertions.
    let saved_max = std::env::var("MAX_CONCURRENT").ok();
    let saved_factory_max = std::env::var("FACTORY_MAX_CONCURRENT").ok();
    std::env::remove_var("MAX_CONCURRENT");
    std::env::remove_var("FACTORY_MAX_CONCURRENT");
    let state = tmpdir("resolver");
    assert_eq!(resolve_max_concurrent(None, &state).unwrap(), 1);
    assert_eq!(resolve_max_concurrent(Some(3), &state).unwrap(), 3);
    assert!(resolve_max_concurrent(Some(0), &state).is_err());
    assert!(resolve_max_concurrent(Some(99), &state).is_err());
    set_concurrency_level(&state, "4").unwrap();
    assert_eq!(resolve_max_concurrent(None, &state).unwrap(), 4);
    assert!(set_concurrency_level(&state, "0").is_err());
    assert!(set_concurrency_level(&state, "many").is_err());
    assert_eq!(
        std::fs::read_to_string(state.join("concurrency"))
            .unwrap()
            .trim(),
        "4"
    );
    if let Some(v) = saved_max {
        std::env::set_var("MAX_CONCURRENT", v);
    }
    if let Some(v) = saved_factory_max {
        std::env::set_var("FACTORY_MAX_CONCURRENT", v);
    }

    // The UI fragment carries the same control over the same setting.
    let data = factory_lib::DispatcherData {
        attempts: vec![],
        stopped: false,
        worktrees: vec![],
        max_concurrent: 4,
        concurrency_source: "saved setting".to_string(),
        tasks: vec![],
    };
    let frag = dispatcher_fragment_html(&data);
    assert!(
        frag.contains("data-concurrency"),
        "control section, got:\n{frag}"
    );
    assert!(
        frag.contains("/dispatch/concurrency"),
        "POST endpoint, got:\n{frag}"
    );
    assert!(
        frag.contains("4") && frag.contains("1") && frag.contains("8"),
        "level plus bounds, got:\n{frag}"
    );
}

/// The dispatcher UI writes the same setting the CLI reads: POSTing a
/// level persists it for the next pass; out-of-bounds is refused with the
/// bounds named and nothing is written.
#[test]
fn ui_post_persists_concurrency_for_next_pass() {
    use axum::response::IntoResponse;
    use factory_lib::{dispatcher_concurrency_handler, resolve_max_concurrent, ConcurrencyForm};
    let _guard = UI_ENV_LOCK.lock().unwrap();
    let saved_state = std::env::var("FACTORY_STATE_DIR").ok();
    let saved_max = std::env::var("MAX_CONCURRENT").ok();
    let saved_factory_max = std::env::var("FACTORY_MAX_CONCURRENT").ok();
    let state = tmpdir("ui-post");
    // SAFETY: static lock held: no sibling test observes these values.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::remove_var("MAX_CONCURRENT");
        std::env::remove_var("FACTORY_MAX_CONCURRENT");
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ok = rt.block_on(dispatcher_concurrency_handler(axum::Form(
        ConcurrencyForm {
            level: "3".to_string(),
        },
    )));
    assert!(ok.is_ok(), "valid level redirects");
    assert_eq!(
        ok.unwrap().into_response().status(),
        axum::http::StatusCode::SEE_OTHER
    );
    assert_eq!(resolve_max_concurrent(None, &state).unwrap(), 3);
    let bad = rt.block_on(dispatcher_concurrency_handler(axum::Form(
        ConcurrencyForm {
            level: "99".to_string(),
        },
    )));
    let err = bad.unwrap_err();
    assert_eq!(err.0, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        err.1.contains('1') && err.1.contains('8'),
        "bounds named: {}",
        err.1
    );
    assert_eq!(resolve_max_concurrent(None, &state).unwrap(), 3);
    unsafe {
        if let Some(v) = saved_state {
            std::env::set_var("FACTORY_STATE_DIR", v);
        } else {
            std::env::remove_var("FACTORY_STATE_DIR");
        }
        if let Some(v) = saved_max {
            std::env::set_var("MAX_CONCURRENT", v);
        }
        if let Some(v) = saved_factory_max {
            std::env::set_var("FACTORY_MAX_CONCURRENT", v);
        }
    }
}

/// Wave rows keep distinct ids and per-ticket integrity (finding #1):
/// id, push, and index resolve under one guard, so concurrent workers can
/// neither mint duplicate ids nor land in each other's rows.
#[test]
fn wave_rows_keep_distinct_ids_and_ticket_integrity() {
    let h = setup("wave-integrity");
    for (n, title) in [(11, "One"), (12, "Two")] {
        write_view(&h, n, title, "Build it.", &["ticket", "ready"], "OPEN");
    }
    dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2"])
        .env("GH_LIST_JSON", r#"[{"number":11},{"number":12}]"#)
        .assert()
        .success();
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let attempts = ledger.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 2, "one row per ticket:\n{ledger}");
    let mut ids: Vec<u64> = attempts
        .iter()
        .filter_map(|a| a.get("id").and_then(|v| v.as_u64()))
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 2, "distinct row ids:\n{ledger}");
    for a in attempts {
        let ticket = a.get("ticket").and_then(|v| v.as_u64()).unwrap();
        let attempt_no = a.get("attempt_no").and_then(|v| v.as_u64()).unwrap();
        let branch = a.get("branch").and_then(|v| v.as_str()).unwrap_or("");
        let worktree = a.get("worktree").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            branch.starts_with(&format!("factory/T{ticket}-")),
            "row carries its own ticket branch, got {branch}:\n{ledger}"
        );
        assert!(
            worktree.contains(&format!("T{ticket}-a{attempt_no}")),
            "row carries its own slot worktree, got {worktree}:\n{ledger}"
        );
        assert_eq!(
            a.get("status").and_then(|v| v.as_str()),
            Some("review_done"),
            "both finish clean:\n{ledger}"
        );
    }
}

/// The pass flag beats the environment end to end (finding #2):
/// `--concurrency 2` admits a 2-wide wave even with MAX_CONCURRENT=1.
#[test]
fn flag_beats_env_end_to_end() {
    let h = setup("flag-beats-env");
    for (n, title) in [(11, "One"), (12, "Two")] {
        write_view(&h, n, title, "Build it.", &["ticket", "ready"], "OPEN");
    }
    let out = dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2"])
        .env("MAX_CONCURRENT", "1")
        .env("GH_LIST_JSON", r#"[{"number":11},{"number":12}]"#)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("dispatch: pass complete (2 attempted)"),
        "flag admits the full wave, got:\n{stdout}"
    );
}

/// Resume honors the pass level, not the environment (finding #2): with
/// `--concurrency 2` and MAX_CONCURRENT=1, a checkpointed attempt resumes
/// instead of deferring at the stale env gate — while its sibling stays
/// live through a slow implement.
#[test]
fn resume_honors_pass_level_not_env() {
    let h = setup("resume-level");
    write_view(&h, 11, "Slow", "Build it.", &["ticket", "ready"], "OPEN");
    write_view(&h, 12, "Quick", "Build it.", &["ticket", "ready"], "OPEN");
    // #11's implement sleeps with its slot held; #12 resumes instantly.
    let pi = write_exe(
        h.repo.parent().unwrap(),
        "pi-slow11",
        &format!(
            r#"#!/bin/sh
echo "pi slot:${{FACTORY_SLOT:-unset}}" >> "{}"
if [ "${{FACTORY_SLOT:-}}" = "T11-a1" ]; then sleep 5; fi
echo "change from pi" >> change.txt
echo "notes"
echo "VERDICT: accept"
"#,
            h.pi_log.display(),
        ),
    );
    fs::create_dir_all(h.state.join("logs")).unwrap();
    let log_path = h.state.join("logs").join("T12-a1-impl.log");
    fs::write(&log_path, "abandoned mid-implement\n").unwrap();
    let ledger = serde_json::json!({"attempts": [{
        "id": 1, "ticket": 12, "attempt_no": 1,
        "base_commit": "", "head_commit": "", "branch": "factory/T12-demo",
        "worktree": "", "status": "impl_interrupted", "exit_code": 0,
        "log_path": log_path.to_string_lossy(), "started_at": "1",
        "finished_at": "2", "train_branch": "", "phase": "implement", "resumed": 0,
    }]});
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string(&ledger).unwrap(),
    )
    .unwrap();
    let out = dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2"])
        .env("FACTORY_PI_BIN", &pi)
        .env("MAX_CONCURRENT", "1")
        .env("GH_LIST_JSON", r#"[{"number":11},{"number":12}]"#)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("resumed from implement checkpoint"),
        "checkpoint resumes, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("slot gate at capacity"),
        "pass level admits over the stale env gate, got:\n{stdout}"
    );
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let attempts = after.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 2, "resume reuses its row:\n{after}");
    let resumed: Vec<&serde_json::Value> = attempts
        .iter()
        .filter(|a| a.get("ticket").and_then(|v| v.as_u64()) == Some(12))
        .collect();
    assert_eq!(resumed.len(), 1, "no duplicate row:\n{after}");
    assert_eq!(
        resumed[0].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "checkpoint resume counted:\n{after}"
    );
    assert_eq!(
        resumed[0].get("status").and_then(|v| v.as_str()),
        Some("review_done"),
        "resumed attempt finishes:\n{after}"
    );
}

/// A pre-existing live row competes fairly without overshoot (finding #4):
/// the reaper converts it to a resumable checkpoint, every wave re-checks
/// capacity (never admitting past max_concurrent), and the row resumes on
/// its own record — never duplicated, never starved.
#[test]
fn live_row_competes_fairly_without_overshoot() {
    let h = setup("live-row-cap");
    for (n, title) in [(10, "Ten"), (11, "One"), (12, "Two")] {
        write_view(&h, n, title, "Build it.", &["ticket", "ready"], "OPEN");
    }
    fs::create_dir_all(h.state.join("logs")).unwrap();
    let log_path = h.state.join("logs").join("T10-a1-impl.log");
    fs::write(&log_path, "abandoned mid-implement\n").unwrap();
    let ledger = serde_json::json!({"attempts": [{
        "id": 1, "ticket": 10, "attempt_no": 1,
        "base_commit": "", "head_commit": "", "branch": "factory/T10-demo",
        "worktree": "", "status": "impl_running", "exit_code": 0,
        "log_path": log_path.to_string_lossy(), "started_at": "1",
        "finished_at": "", "train_branch": "", "phase": "implement", "resumed": 0,
    }]});
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string(&ledger).unwrap(),
    )
    .unwrap();
    let out = dispatch_cmd(&h, &["dispatch", "once", "--concurrency", "2"])
        .env(
            "GH_LIST_JSON",
            r#"[{"number":10},{"number":11},{"number":12}]"#,
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("waiting for a free slot") && stdout.contains("#12"),
        "queue discipline names the deferred ticket, got:\n{stdout}"
    );
    for line in stdout.lines().filter(|l| l.contains("admitting #")) {
        // Count only the admitted segment (before any waiting list).
        let admitted_segment = line.split("admitting").nth(1).unwrap_or("");
        let admitted_segment = admitted_segment.split(';').next().unwrap_or("");
        let admitted = admitted_segment.matches('#').count();
        assert!(
            admitted <= 2,
            "no wave admits past capacity, got {line:?}:\n{stdout}"
        );
    }
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(h.state.join("dispatch.json")).unwrap()).unwrap();
    let attempts = after.get("attempts").and_then(|a| a.as_array()).unwrap();
    assert_eq!(attempts.len(), 3, "one row per ticket:\n{after}");
    let mut ids: Vec<u64> = attempts
        .iter()
        .filter_map(|a| a.get("id").and_then(|v| v.as_u64()))
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "distinct row ids:\n{after}");
    let ten: Vec<&serde_json::Value> = attempts
        .iter()
        .filter(|a| a.get("ticket").and_then(|v| v.as_u64()) == Some(10))
        .collect();
    assert_eq!(ten.len(), 1, "ex-live row resumes on its record:\n{after}");
    assert_eq!(
        ten[0].get("resumed").and_then(|v| v.as_u64()),
        Some(1),
        "reaped row counted as resumed:\n{after}"
    );
    for a in attempts {
        assert_eq!(
            a.get("status").and_then(|v| v.as_str()),
            Some("review_done"),
            "every ticket finishes:\n{after}"
        );
    }
}

/// A daemon startup flag seeds the saved level once, then later passes
/// re-resolve without it (finding #3): a UI write after startup takes
/// effect on the very next pass instead of being shadowed.
#[test]
fn daemon_flag_seeds_saved_level_then_ui_wins() {
    let h = setup("daemon-flag-once");
    // STOP halts after the first pass; no tickets listed.
    fs::create_dir_all(&h.state).unwrap();
    fs::write(h.state.join("STOP"), "stopped\n").unwrap();
    dispatch_cmd(
        &h,
        &[
            "dispatch",
            "daemon",
            "--interval",
            "0",
            "--concurrency",
            "2",
        ],
    )
    .env("GH_LIST_JSON", r#"[]"#)
    .assert()
    .success();
    assert_eq!(
        fs::read_to_string(h.state.join("concurrency"))
            .unwrap()
            .trim(),
        "2",
        "startup flag persists as the saved level"
    );
    // The UI handler→file link is pinned by
    // ui_post_persists_concurrency_for_next_pass; here the file→pass link:
    // a saved 1 with no flag runs the serial path over three tickets.
    fs::remove_file(h.state.join("STOP")).unwrap();
    fs::write(h.state.join("concurrency"), "1\n").unwrap();
    for (n, title) in [(11, "One"), (12, "Two"), (13, "Three")] {
        write_view(&h, n, title, "Build it.", &["ticket", "ready"], "OPEN");
    }
    let out = dispatch_cmd(&h, &["dispatch", "once"])
        .env(
            "GH_LIST_JSON",
            r#"[{"number":11},{"number":12},{"number":13}]"#,
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        !stdout.contains("wave"),
        "saved level 1 runs the serial path, got:\n{stdout}"
    );
    assert!(
        stdout.contains("dispatch: pass complete (3 attempted)"),
        "serial pass still drains the queue, got:\n{stdout}"
    );
}

/// The concurrency route is registered end to end (finding #6): POSTing
/// the form path from the fragment redirects, persists the level, and the
/// fragment then reflects the new level — a typo'd path or method would
/// 404 here while handler-direct tests stay green.
#[test]
fn router_serves_concurrency_post_and_fragment_reflects_level() {
    use std::io::{Read, Write};
    let _guard = UI_ENV_LOCK.lock().unwrap();
    let saved_state = std::env::var("FACTORY_STATE_DIR").ok();
    let saved_max = std::env::var("MAX_CONCURRENT").ok();
    let saved_factory_max = std::env::var("FACTORY_MAX_CONCURRENT").ok();
    let state = tmpdir("ui-router");
    fs::create_dir_all(&state).unwrap();
    // SAFETY: UI_ENV_LOCK held: no sibling test observes these values.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
        std::env::remove_var("MAX_CONCURRENT");
        std::env::remove_var("FACTORY_MAX_CONCURRENT");
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, factory_lib::web_router()).await;
        });
        let post = tokio::task::spawn_blocking(move || {
            let body = "level=2";
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                s,
                "POST /dispatch/concurrency HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).unwrap();
            String::from_utf8_lossy(&buf).into_owned()
        })
        .await
        .unwrap();
        assert!(
            post.contains("303") || post.contains("See Other"),
            "POST redirects through the registered route, got:\n{post}"
        );
        server.abort();
    });
    assert_eq!(
        fs::read_to_string(state.join("concurrency"))
            .unwrap()
            .trim(),
        "2",
        "routed POST persists the level"
    );
    // The polling fragment now reflects the posted level.
    let frag = factory_lib::dispatcher_fragment_html(&factory_lib::dispatcher_data());
    assert!(
        frag.contains("data-concurrency-level=\"2\""),
        "fragment reflects the new level, got:\n{frag}"
    );
    unsafe {
        if let Some(v) = saved_state {
            std::env::set_var("FACTORY_STATE_DIR", v);
        } else {
            std::env::remove_var("FACTORY_STATE_DIR");
        }
        if let Some(v) = saved_max {
            std::env::set_var("MAX_CONCURRENT", v);
        }
        if let Some(v) = saved_factory_max {
            std::env::set_var("FACTORY_MAX_CONCURRENT", v);
        }
    }
}
