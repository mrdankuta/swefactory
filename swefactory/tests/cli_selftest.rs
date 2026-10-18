//! Hermetic self-test oracle (ticket #15).
//!
//! Seam: argv in -> exit code + stdout/stderr out. The oracle itself builds
//! a temp bare origin + scratch clone and drives the real `release`
//! subcommands with tokens scrubbed and `gh` replaced by `/bin/false`, so
//! green here means the release mechanics hold with no network, no tokens,
//! and no spend. Any mechanics failure fails loudly inside the oracle AND
//! fails this test.
//!
//! What these tests catch: the oracle refusing to run hermetically, or any
//! named mechanics step regressing.
//! What they miss: real `origin/dev` fetches and real CI on a train head
//! (same boundary as `cli_release.rs`).

use assert_cmd::Command;

fn selftest_cmd(json: bool) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    if json {
        cmd.arg("--json");
    }
    cmd.args(["dispatch", "self-test"]);
    cmd.env("FACTORY_GH_BIN", "/bin/false")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_HOST")
        .env_remove("GITHUB_API_URL")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    cmd
}

#[test]
fn dispatch_self_test_proves_release_mechanics_hermetically() {
    let out = selftest_cmd(false).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "dispatch self-test must exit 0.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    for step in [
        "PASS: start cuts train",
        "PASS: second train refused",
        "PASS: status shows open train",
        "PASS: merge clean",
        "PASS: merge commit present",
        "PASS: drop reverts",
        "PASS: drop-idempotence refusal",
        "PASS: conflict-abort refuses merge",
        "PASS: train clean after abort",
        "PASS: status runs after abort",
        "PASS: finish unmerged refusal",
        "PASS: finish merged close",
        "PASS: finish no-train refusal",
        "PASS: abandon refuses with no train",
        "PASS: train restarts after finish",
        "PASS: abandon closes unmerged train",
        "PASS: abandon never deletes branches",
        "PASS: second abandon refused",
        "PASS: dispatch kill-mid-phase interrupts (resumable)",
        "PASS: dispatch resume completes the same attempt",
        "PASS: dispatch resume spends nothing (no network, fake implementer only)",
        "PASS: facts conflict rejects stale",
        "PASS: facts retry succeeds",
        "PASS: no gh invoked",
        "SELF-TEST ALL PASS",
    ] {
        assert!(
            stdout.contains(step),
            "self-test output misses {step:?}.\n--- stdout ---\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("FAIL:"),
        "self-test reported a mechanics failure.\n--- stdout ---\n{stdout}"
    );
}

#[test]
fn dispatch_self_test_json_is_machine_parseable() {
    let out = selftest_cmd(true).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "json self-test must exit 0.\n--- stderr ---\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("json self-test must print one JSON object");
    assert_eq!(v.get("passed").and_then(|p| p.as_bool()), Some(true));
    let checks = v
        .get("checks")
        .and_then(|c| c.as_array())
        .expect("json self-test must carry a checks array");
    assert!(
        checks.len() >= 21,
        "expected all mechanics checks, got {checks:?}"
    );
    for c in checks {
        assert_eq!(
            c.get("ok").and_then(|o| o.as_bool()),
            Some(true),
            "mechanics check failed: {c}"
        );
    }
}
