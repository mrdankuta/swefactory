use assert_cmd::Command;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique scratch dir under the OS temp dir (std-only, no extra deps).
fn scratch_dir(tag: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "swefactory-check-{}-{}-{}",
        std::process::id(),
        id,
        tag
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write an executable sh fixture adapter. The script content decides
/// stage order, failure injection, and slot observation via env vars.
fn write_adapter(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, "#!/bin/sh").unwrap();
    f.write_all(body.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// Adapter that logs the three contract stages in order, then succeeds.
/// Honors FAIL_AT=format|lint|test to exit non-zero at that stage
/// (without the success marker), and records $FACTORY_SLOT for isolation
/// proofs. Log target comes from $FIXTURE_LOG.
fn stage_adapter_source() -> &'static str {
    r#"
log="$FIXTURE_LOG"
echo "slot:${FACTORY_SLOT:-unset}" >> "$log"
echo "format" >> "$log"
if [ "$FAIL_AT" = "format" ]; then exit 1; fi
echo "lint" >> "$log"
if [ "$FAIL_AT" = "lint" ]; then exit 1; fi
echo "test" >> "$log"
if [ "$FAIL_AT" = "test" ]; then exit 1; fi
echo "FACTORY-CHECK PASSED"
"#
}

fn run_check(adapter: &Path, extra: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.arg("check").arg("--adapter").arg(adapter);
    for a in extra {
        cmd.arg(a);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    // Never inherit the developer's real slot into fixture runs.
    cmd.env_remove("FACTORY_CHECK_ADAPTER");
    cmd.output().unwrap()
}

#[test]
fn check_runs_stages_in_order_and_prints_success_marker() {
    let dir = scratch_dir("order");
    let adapter = write_adapter(&dir, "adapter.sh", stage_adapter_source());
    let log = dir.join("stages.log");

    let out = run_check(&adapter, &[], &[("FIXTURE_LOG", log.to_str().unwrap())]);

    assert!(
        out.status.success(),
        "passing adapter must exit 0, got {} stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let logged = fs::read_to_string(&log).unwrap();
    let stages: Vec<&str> = logged
        .lines()
        .filter(|l| *l == "format" || *l == "lint" || *l == "test")
        .collect();
    assert_eq!(stages, vec!["format", "lint", "test"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("FACTORY-CHECK PASSED"),
        "success marker must appear when every stage passes"
    );
}

#[test]
fn check_fails_when_any_stage_fails_and_omits_marker() {
    for fail_at in ["format", "lint", "test"] {
        let dir = scratch_dir(fail_at);
        let adapter = write_adapter(&dir, "adapter.sh", stage_adapter_source());
        let log = dir.join("stages.log");

        let out = run_check(
            &adapter,
            &[],
            &[("FIXTURE_LOG", log.to_str().unwrap()), ("FAIL_AT", fail_at)],
        );

        assert!(!out.status.success(), "FAIL_AT={fail_at} must fail the run");
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("FACTORY-CHECK PASSED"),
            "FAIL_AT={fail_at}: success marker must not appear on failure"
        );
        // Earlier stages still ran in order; later stages never ran.
        let logged = fs::read_to_string(&log).unwrap();
        let stages: Vec<&str> = logged
            .lines()
            .filter(|l| *l == "format" || *l == "lint" || *l == "test")
            .collect();
        let expected: Vec<&str> = match fail_at {
            "format" => vec!["format"],
            "lint" => vec!["format", "lint"],
            _ => vec!["format", "lint", "test"],
        };
        assert_eq!(
            stages, expected,
            "FAIL_AT={fail_at}: stages before failure run in order"
        );
    }
}

#[test]
fn check_propagates_slot_to_adapter_and_isolates_concurrent_runs() {
    let dir = scratch_dir("slots");
    let adapter = write_adapter(&dir, "adapter.sh", stage_adapter_source());

    let run_with_slot = |slot: &str, log: PathBuf, adapter: PathBuf| {
        let mut cmd = Command::cargo_bin("factory").unwrap();
        cmd.arg("check")
            .arg("--adapter")
            .arg(&adapter)
            .arg("--slot")
            .arg(slot);
        cmd.env("FIXTURE_LOG", &log);
        cmd.env_remove("FACTORY_CHECK_ADAPTER");
        cmd.env_remove("FACTORY_SLOT");
        cmd.output().unwrap()
    };

    let log_a = dir.join("a.log");
    let log_b = dir.join("b.log");
    let (log_a_c, log_b_c) = (log_a.clone(), log_b.clone());
    let (ad_a, ad_b) = (adapter.clone(), adapter.clone());

    // Concurrent runs with different slots must never share mutable state:
    // each run observes exactly its own slot.
    let ha = std::thread::spawn(move || run_with_slot("slot-a", log_a_c, ad_a));
    let hb = std::thread::spawn(move || run_with_slot("slot-b", log_b_c, ad_b));
    let (out_a, out_b) = (ha.join().unwrap(), hb.join().unwrap());

    assert!(out_a.status.success() && out_b.status.success());
    let logged_a = fs::read_to_string(&log_a).unwrap();
    let logged_b = fs::read_to_string(&log_b).unwrap();
    assert!(
        logged_a.contains("slot:slot-a") && !logged_a.contains("slot:slot-b"),
        "slot-a run must observe only its own slot: {logged_a}"
    );
    assert!(
        logged_b.contains("slot:slot-b") && !logged_b.contains("slot:slot-a"),
        "slot-b run must observe only its own slot: {logged_b}"
    );
}

#[test]
fn check_forwards_extra_args_to_adapter() {
    let dir = scratch_dir("args");
    let adapter = write_adapter(
        &dir,
        "adapter.sh",
        r#"
echo "received:$1,$2" >> "$FIXTURE_LOG"
echo "FACTORY-CHECK PASSED"
"#,
    );
    let log = dir.join("args.log");

    let out = run_check(
        &adapter,
        &["--", "alpha", "beta"],
        &[("FIXTURE_LOG", log.to_str().unwrap())],
    );

    assert!(out.status.success());
    assert_eq!(
        fs::read_to_string(&log).unwrap().trim(),
        "received:alpha,beta"
    );
}

#[test]
fn check_json_mode_emits_parseable_summary() {
    let dir = scratch_dir("json");
    let adapter = write_adapter(&dir, "adapter.sh", stage_adapter_source());
    let log = dir.join("json.log");

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.arg("--json")
        .arg("check")
        .arg("--adapter")
        .arg(&adapter);
    cmd.env("FIXTURE_LOG", log.to_str().unwrap());
    cmd.env_remove("FACTORY_CHECK_ADAPTER");
    let out = cmd.output().unwrap();

    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let last = stdout
        .lines()
        .last()
        .expect("check --json prints a summary");
    let parsed: serde_json::Value = serde_json::from_str(last).expect("last stdout line is JSON");
    assert_eq!(parsed.get("passed"), Some(&serde_json::Value::Bool(true)));
}

#[test]
fn check_json_mode_reports_failure_as_not_passed() {
    let dir = scratch_dir("json-fail");
    let adapter = write_adapter(&dir, "adapter.sh", stage_adapter_source());
    let log = dir.join("json-fail.log");

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.arg("--json")
        .arg("check")
        .arg("--adapter")
        .arg(&adapter);
    cmd.env("FIXTURE_LOG", log.to_str().unwrap());
    cmd.env("FAIL_AT", "test");
    cmd.env_remove("FACTORY_CHECK_ADAPTER");
    let out = cmd.output().unwrap();

    assert!(!out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let last = stdout
        .lines()
        .last()
        .expect("check --json prints a summary");
    let parsed: serde_json::Value = serde_json::from_str(last).expect("last stdout line is JSON");
    assert_eq!(parsed.get("passed"), Some(&serde_json::Value::Bool(false)));
}

#[test]
fn check_missing_adapter_fails_loudly() {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.arg("check")
        .arg("--adapter")
        .arg("/nonexistent/factory-check-fixture");
    cmd.env_remove("FACTORY_CHECK_ADAPTER");
    let out = cmd.output().unwrap();
    assert!(!out.status.success());
}
