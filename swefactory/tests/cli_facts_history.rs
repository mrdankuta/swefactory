//! Rewindable facts history + time-travel reads (ticket #71, spec #64).
//!
//! Seam: argv in -> exit code + stdout/stderr out + observable
//! state-directory contents. State comes from a seeded
//! `FACTORY_STATE_DIR` (a `facts.json` plus the `facts.history.json`
//! the writes append); the working directory is a scratch git repo
//! standing in for the project. No daemon, no network, no
//! issue-tracker calls.
//!
//! What these tests catch: a value written three times read back at
//! each revision in human and machine modes with identical facts;
//! history reads refused unless explicitly bounded (bound stated);
//! latest-only reads byte-identical to before; and proof that
//! history/as-of reads change no files. What they miss: the real
//! multi-machine story (facts stay per-machine local state).

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-facts-history-{}-{}-{}",
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

struct Harness {
    repo: PathBuf,
    state: PathBuf,
}

fn setup(name: &str) -> Harness {
    let root = tmpdir(name);
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_run(&repo, &["init", "-b", "dev"]);
    git_run(&repo, &["config", "user.email", "t@t"]);
    git_run(&repo, &["config", "user.name", "t"]);
    fs::write(repo.join("f.txt"), "x\n").unwrap();
    git_run(&repo, &["add", "."]);
    git_run(&repo, &["commit", "-m", "seed"]);
    Harness {
        state: root.join("state"),
        repo,
    }
}

fn facts_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

fn run_ok(h: &Harness, args: &[&str]) -> String {
    let out = facts_cmd(h, args).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "args {args:?} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn run_json(h: &Harness, args: &[&str]) -> serde_json::Value {
    let out = facts_cmd(h, args).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "args {args:?} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap()
}

/// Write one value three times: create v1 (rev 0->1), set v2, set v3.
fn write_three(h: &Harness, path: &str, v1: &str, v2: &str, v3: &str) {
    fs::create_dir_all(&h.state).unwrap();
    for (rev, v) in [(0, v1), (1, v2), (2, v3)] {
        let op = if rev == 0 { "create" } else { "set" };
        let out = facts_cmd(
            h,
            &[
                "facts",
                op,
                path,
                "--revision",
                &rev.to_string(),
                "--value",
                v,
            ],
        )
        .output()
        .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{op} rev {rev} stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// (relative path, bytes, mtime) for every file under the state dir.
fn snapshot(state: &Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut out = Vec::new();
    if !state.is_dir() {
        return out;
    }
    let mut stack = vec![state.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries: Vec<_> = fs::read_dir(&dir).map(|r| r.collect()).unwrap_or_default();
        for e in entries {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p
                    .strip_prefix(state)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let bytes = fs::read(&p).unwrap();
                let mtime = fs::metadata(&p).unwrap().modified().unwrap();
                out.push((rel, bytes, mtime));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn three_writes_read_back_at_each_revision_both_modes() {
    let h = setup("three-writes");
    write_three(&h, "costs/total", "10", "20", "30");

    for (rev, want) in [(1u64, 10), (2, 20), (3, 30)] {
        let text = run_ok(
            &h,
            &["facts", "show", "costs/total", "--at", &rev.to_string()],
        );
        assert!(
            text.contains("costs/total"),
            "human names the path at rev {rev}, got:\n{text}"
        );
        assert!(
            text.contains(&format!("revision {rev}")),
            "human names the as-of revision {rev}, got:\n{text}"
        );
        assert!(
            text.contains(&want.to_string()),
            "human carries the rev-{rev} value {want}, got:\n{text}"
        );

        let v = run_json(
            &h,
            &[
                "facts",
                "show",
                "costs/total",
                "--at",
                &rev.to_string(),
                "--json",
            ],
        );
        assert_eq!(v["revision"], serde_json::json!(rev));
        assert_eq!(v["path"], serde_json::json!("costs/total"));
        assert_eq!(v["value"], serde_json::json!(want));

        // Identical facts in both modes: same revision, same value.
        assert!(
            text.contains(&format!("revision {rev}")) && text.contains(&want.to_string()),
            "human and machine agree at rev {rev}"
        );
    }

    // Whole-document as-of read carries the same revision facts.
    let v = run_json(&h, &["facts", "show", "--at", "2", "--json"]);
    assert_eq!(v["revision"], serde_json::json!(2));
    assert_eq!(v["facts"]["costs"]["total"], serde_json::json!(20));
    let text = run_ok(&h, &["facts", "show", "--at", "2"]);
    assert!(text.contains("revision 2"), "got:\n{text}");
    assert!(text.contains("20"), "got:\n{text}");
}

#[test]
fn history_reads_bounded_bound_stated_on_refusal() {
    let h = setup("bounded");
    write_three(&h, "costs/total", "10", "20", "30");
    let before = snapshot(&h.state);

    // No unbounded dumps: a bare history read is refused with the bound.
    for args in [
        vec!["facts", "history", "costs/total"],
        vec!["facts", "history", "costs/total", "--json"],
    ] {
        let out = facts_cmd(&h, &args).output().unwrap();
        assert!(
            !out.status.success(),
            "unbounded history must fail: {args:?}"
        );
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.contains("50"),
            "refusal states the bound (50), got:\n{err}"
        );
        assert!(
            err.to_lowercase().contains("--limit"),
            "refusal names the bound flag, got:\n{err}"
        );
    }

    // Zero and over-cap limits are refused the same way.
    for limit in ["0", "51"] {
        let out = facts_cmd(&h, &["facts", "history", "costs/total", "--limit", limit])
            .output()
            .unwrap();
        assert!(!out.status.success(), "--limit {limit} must fail");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.contains("50"),
            "--limit {limit} refusal states the bound, got:\n{err}"
        );
    }

    // Bounded read: exactly the last 2 entries, oldest-first.
    let text = run_ok(&h, &["facts", "history", "costs/total", "--limit", "2"]);
    assert!(text.contains("revision 2"), "got:\n{text}");
    assert!(text.contains("revision 3"), "got:\n{text}");
    assert!(
        !text.contains("revision 1"),
        "limit 2 drops rev 1, got:\n{text}"
    );
    assert!(text.contains("20") && text.contains("30"), "got:\n{text}");

    let v = run_json(
        &h,
        &["facts", "history", "costs/total", "--limit", "2", "--json"],
    );
    assert_eq!(v["path"], serde_json::json!("costs/total"));
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["limit"], serde_json::json!(2));
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "exactly two entries, got: {v}");
    assert_eq!(entries[0]["revision"], serde_json::json!(2));
    assert_eq!(entries[0]["value"], serde_json::json!(20));
    assert_eq!(entries[1]["revision"], serde_json::json!(3));
    assert_eq!(entries[1]["value"], serde_json::json!(30));

    // Refusals changed nothing on disk.
    let after = snapshot(&h.state);
    assert_eq!(before.len(), after.len(), "no files added or removed");
    for ((rel_b, bytes_b, _), (rel_a, bytes_a, _)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
    }
}

#[test]
fn latest_only_reads_unchanged() {
    let h = setup("latest");
    write_three(&h, "costs/total", "10", "20", "30");

    // Per-path latest read: same shape as before this ticket.
    let path_text = run_ok(&h, &["facts", "show", "costs/total"]);
    assert!(path_text.contains("costs/total"), "got:\n{path_text}");
    assert!(path_text.contains("revision 3"), "got:\n{path_text}");
    assert!(path_text.contains("30"), "got:\n{path_text}");
    let pv = run_json(&h, &["facts", "show", "costs/total", "--json"]);
    assert_eq!(pv["revision"], serde_json::json!(3));
    assert_eq!(pv["path"], serde_json::json!("costs/total"));
    assert_eq!(pv["value"], serde_json::json!(30));

    // Whole-document latest read: same shape as before.
    let text = run_ok(&h, &["facts", "show"]);
    assert!(text.contains("revision 3"), "got:\n{text}");
    assert!(text.contains("30"), "got:\n{text}");
    let v = run_json(&h, &["facts", "show", "--json"]);
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["facts"]["costs"]["total"], serde_json::json!(30));

    // As-of the current revision is exactly the latest read.
    let at_text = run_ok(&h, &["facts", "show", "costs/total", "--at", "3"]);
    let at_v = run_json(&h, &["facts", "show", "costs/total", "--at", "3", "--json"]);
    assert_eq!(at_v["revision"], pv["revision"]);
    assert_eq!(at_v["value"], pv["value"]);
    assert_eq!(at_text, path_text, "show --at CURRENT ≡ show, human");
}

#[test]
fn history_and_asof_reads_change_nothing() {
    let h = setup("readonly");
    write_three(&h, "costs/total", "10", "20", "30");
    let before = snapshot(&h.state);
    assert!(before.len() >= 2, "facts + history seeded");

    for args in [
        vec!["facts", "show", "costs/total", "--at", "1"],
        vec!["facts", "show", "costs/total", "--at", "2", "--json"],
        vec!["facts", "show", "--at", "1"],
        vec!["facts", "show", "--at", "1", "--json"],
        vec!["facts", "history", "costs/total", "--limit", "2"],
        vec!["facts", "history", "costs/total", "--limit", "2", "--json"],
        vec!["facts", "history", "nope/missing", "--limit", "5"],
        vec!["facts", "history", "nope/missing", "--limit", "5", "--json"],
    ] {
        let out = facts_cmd(&h, &args).output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "args {args:?} stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let after = snapshot(&h.state);
    assert_eq!(
        before.len(),
        after.len(),
        "no files added or removed by history reads"
    );
    for ((rel_b, bytes_b, mtime_b), (rel_a, bytes_a, mtime_a)) in before.iter().zip(after.iter()) {
        assert_eq!(rel_b, rel_a);
        assert_eq!(bytes_b, bytes_a, "bytes of {rel_b} unchanged");
        assert_eq!(mtime_b, mtime_a, "mtime of {rel_b} unchanged");
    }
}

#[test]
fn history_records_delete_and_missing_paths() {
    let h = setup("delete-history");
    fs::create_dir_all(&h.state).unwrap();
    assert_eq!(
        facts_cmd(
            &h,
            &[
                "facts",
                "create",
                "costs/total",
                "--revision",
                "0",
                "--value",
                "10"
            ]
        )
        .output()
        .unwrap()
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        facts_cmd(&h, &["facts", "delete", "costs/total", "--revision", "1"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        facts_cmd(
            &h,
            &[
                "facts",
                "set",
                "costs/total",
                "--revision",
                "2",
                "--value",
                "30"
            ]
        )
        .output()
        .unwrap()
        .status
        .code(),
        Some(0)
    );

    // History shows value, deletion, value in both modes.
    let v = run_json(
        &h,
        &["facts", "history", "costs/total", "--limit", "10", "--json"],
    );
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3, "got: {v}");
    assert_eq!(entries[0]["value"], serde_json::json!(10));
    assert_eq!(entries[1].get("deleted"), Some(&serde_json::json!(true)));
    assert_eq!(entries[2]["value"], serde_json::json!(30));
    let text = run_ok(&h, &["facts", "history", "costs/total", "--limit", "10"]);
    assert!(text.contains("10"), "got:\n{text}");
    assert!(
        text.to_lowercase().contains("delet"),
        "human marks the deletion, got:\n{text}"
    );
    assert!(text.contains("30"), "got:\n{text}");

    // As-of the deletion revision reads missing (exit 0, never error).
    let text = run_ok(&h, &["facts", "show", "costs/total", "--at", "2"]);
    assert!(
        text.to_lowercase().contains("no value set"),
        "deleted-at-rev reads missing, got:\n{text}"
    );
    let v = run_json(&h, &["facts", "show", "costs/total", "--at", "2", "--json"]);
    assert_eq!(v["revision"], serde_json::json!(2));
    assert_eq!(v["value"], serde_json::Value::Null);

    // A never-written path has an empty history (exit 0, never error).
    let v = run_json(
        &h,
        &[
            "facts",
            "history",
            "never/written",
            "--limit",
            "5",
            "--json",
        ],
    );
    assert_eq!(v["entries"], serde_json::json!([]));
    let text = run_ok(&h, &["facts", "history", "never/written", "--limit", "5"]);
    assert!(
        text.contains("never/written"),
        "human names the path, got:\n{text}"
    );
}

#[test]
fn future_revision_refused_nothing_changed() {
    let h = setup("future");
    write_three(&h, "costs/total", "10", "20", "30");

    let out = facts_cmd(&h, &["facts", "show", "costs/total", "--at", "99"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "future revision must fail");
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("99") && err.contains('3'),
        "refusal names presented (99) and current (3), got:\n{err}"
    );

    // Nothing changed: latest value intact at the current revision.
    let v = run_json(&h, &["facts", "show", "costs/total", "--json"]);
    assert_eq!(v["revision"], serde_json::json!(3));
    assert_eq!(v["value"], serde_json::json!(30));
}

#[test]
fn corrupt_history_errors_naming_facts() {
    let h = setup("corrupt-history");
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("facts.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "revision": 2,
            "facts": {"costs": {"total": 10}},
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(h.state.join("facts.history.json"), "this is not json{{{\n").unwrap();

    for args in [
        vec!["facts", "history", "costs/total", "--limit", "5"],
        vec!["facts", "history", "costs/total", "--limit", "5", "--json"],
        vec!["facts", "show", "costs/total", "--at", "1"],
    ] {
        let out = facts_cmd(&h, &args).output().unwrap();
        assert!(!out.status.success(), "corrupt history must fail: {args:?}");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.to_lowercase().contains("facts"),
            "error names the facts document, got:\n{err}"
        );
    }

    // Latest-only reads still work with a corrupt history file.
    let v = run_json(&h, &["facts", "show", "costs/total", "--json"]);
    assert_eq!(v["value"], serde_json::json!(10));
}
