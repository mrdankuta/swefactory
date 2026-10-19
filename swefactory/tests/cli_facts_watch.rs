//! Document-subscription CLI boundary tests (ticket #72, spec #64).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (a `facts.json` plus the `facts-events.jsonl`
//! journal beside the dispatch ledger, or no document at all); the working
//! directory is a scratch git repo standing in for the project. No daemon,
//! no network, no timing.
//!
//! What these tests catch: attach delivers the current value plus
//! revision first, then exactly one delivery per accepted write touching
//! the watched path (other paths excluded, order preserved, no
//! duplicates across a simulated drop and re-attach); a slow watcher
//! past the delivery bound is told to re-attach instead of buffered
//! forever; watching changes no document, journal, ledger, or log state;
//! human and machine modes carry identical facts.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-factswatch-{}-{}-{}",
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

fn seed_ledger_and_logs(h: &Harness) {
    fs::create_dir_all(h.state.join("logs")).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "attempts": [] })).unwrap(),
    )
    .unwrap();
    fs::write(h.state.join("logs").join("T98-a1-impl.log"), "evidence\n").unwrap();
}

fn facts_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

/// One accepted write; asserts exit 0 and returns stdout.
fn write_ok(h: &Harness, args: &[&str]) -> String {
    let out = facts_cmd(h, args).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "write {args:?} failed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
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

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[test]
fn watch_attach_then_one_delivery_per_touching_write() {
    let h = setup("attach-deliveries");
    seed_ledger_and_logs(&h);
    fs::create_dir_all(&h.state).unwrap();

    // Five accepted writes; revision 2 touches another path only.
    write_ok(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "10",
        ],
    );
    write_ok(
        &h,
        &[
            "facts",
            "set",
            "other/path",
            "--revision",
            "1",
            "--value",
            "1",
        ],
    );
    write_ok(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "2",
            "--value",
            "11",
        ],
    );
    write_ok(
        &h,
        &[
            "facts",
            "increment",
            "costs/total",
            "--revision",
            "3",
            "--by",
            "4",
        ],
    );
    write_ok(&h, &["facts", "delete", "costs/total", "--revision", "4"]);

    // Human watch from the beginning: attach (deleted path reads as no
    // value set at the current revision) plus one delivery per touching
    // write, in revision order.
    let out = facts_cmd(&h, &["facts", "watch", "costs/total", "--since", "0"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "watch stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("costs/total") && text.contains("at revision 5"),
        "attach names path + current revision, got:\n{text}"
    );
    assert!(
        text.to_lowercase().contains("no value set"),
        "deleted path attaches as missing, got:\n{text}"
    );
    for (rev, op) in [(1, "create"), (3, "set"), (4, "increment"), (5, "delete")] {
        assert_eq!(
            count_occurrences(&text, &format!("delivery at revision {rev} ({op})")),
            1,
            "exactly one {op} delivery at revision {rev}, got:\n{text}"
        );
    }
    assert_eq!(
        count_occurrences(&text, "delivery at revision 2"),
        0,
        "other-path write is never delivered, got:\n{text}"
    );
    assert!(text.contains("11"), "set value delivered, got:\n{text}");
    assert!(
        text.contains("15"),
        "increment total delivered, got:\n{text}"
    );

    // Machine mode carries the same facts structurally.
    let out = facts_cmd(
        &h,
        &["facts", "watch", "costs/total", "--since", "0", "--json"],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(5));
    assert_eq!(v["path"], serde_json::json!("costs/total"));
    assert_eq!(v["missing"], serde_json::json!(true));
    assert_eq!(v["since"], serde_json::json!(0));
    assert_eq!(v["reattach"], serde_json::json!(false));
    let deliveries = v["deliveries"].as_array().unwrap();
    assert_eq!(deliveries.len(), 4, "one delivery per touching write");
    assert_eq!(deliveries[0]["revision"], serde_json::json!(1));
    assert_eq!(deliveries[0]["op"], serde_json::json!("create"));
    assert_eq!(deliveries[0]["value"], serde_json::json!(10));
    assert_eq!(deliveries[1]["revision"], serde_json::json!(3));
    assert_eq!(deliveries[1]["value"], serde_json::json!(11));
    assert_eq!(deliveries[2]["revision"], serde_json::json!(4));
    assert_eq!(deliveries[2]["op"], serde_json::json!("increment"));
    assert_eq!(deliveries[2]["value"], serde_json::json!(15));
    assert_eq!(deliveries[3]["revision"], serde_json::json!(5));
    assert_eq!(deliveries[3]["op"], serde_json::json!("delete"));
    assert_eq!(deliveries[3]["deleted"], serde_json::json!(true));
}

#[test]
fn watch_fresh_attach_is_snapshot_only_and_reattach_has_no_duplicates() {
    let h = setup("drop-reattach");
    seed_ledger_and_logs(&h);
    fs::create_dir_all(&h.state).unwrap();

    write_ok(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "1",
        ],
    );
    write_ok(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "1",
            "--value",
            "2",
        ],
    );

    // Fresh watch (no cursor) is a snapshot: attach, zero deliveries.
    let out = facts_cmd(&h, &["facts", "watch", "costs/total", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(2));
    assert_eq!(v["value"], serde_json::json!(2));
    assert_eq!(v["deliveries"].as_array().unwrap().len(), 0);

    // Simulated drop: two more writes land while the watcher is away.
    write_ok(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "2",
            "--value",
            "3",
        ],
    );
    write_ok(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "3",
            "--value",
            "4",
        ],
    );

    // Re-attach with the last-seen revision: attach at 4 plus only the
    // missed deliveries — nothing from before the cursor repeats.
    let out = facts_cmd(
        &h,
        &["facts", "watch", "costs/total", "--since", "2", "--json"],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(4));
    assert_eq!(v["value"], serde_json::json!(4));
    let revs: Vec<u64> = v["deliveries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["revision"].as_u64().unwrap())
        .collect();
    assert_eq!(revs, vec![3, 4], "only post-cursor revisions replay");

    // Human re-attach likewise replays exactly the missed pair.
    let out = facts_cmd(&h, &["facts", "watch", "costs/total", "--since", "2"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(count_occurrences(&text, "delivery at revision 3"), 1);
    assert_eq!(count_occurrences(&text, "delivery at revision 4"), 1);
    assert_eq!(count_occurrences(&text, "delivery at revision 1"), 0);
    assert_eq!(count_occurrences(&text, "delivery at revision 2"), 0);
}

#[test]
fn watch_slow_viewer_is_told_to_reattach_not_buffered() {
    let h = setup("slow");
    seed_ledger_and_logs(&h);
    fs::create_dir_all(&h.state).unwrap();

    // 17 touching writes: one past the 16-delivery bound.
    write_ok(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "0",
        ],
    );
    for rev in 1..17u64 {
        write_ok(
            &h,
            &[
                "facts",
                "set",
                "costs/total",
                "--revision",
                &rev.to_string(),
                "--value",
                &rev.to_string(),
            ],
        );
    }

    // A cursor that far behind is refused with a re-attach instruction
    // (current revision named as the way back), not a 17-deep backlog.
    let out = facts_cmd(&h, &["facts", "watch", "costs/total", "--since", "0"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "slow watcher exits 1, stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let combined = format!("{stdout}\n{err}");
    assert!(
        combined.to_lowercase().contains("re-attach"),
        "names the re-attach action, got:\n{combined}"
    );
    assert!(
        combined.contains("--since 17"),
        "names the current revision as the way back, got:\n{combined}"
    );
    assert_eq!(
        count_occurrences(&stdout, "delivery at revision"),
        0,
        "no backlog is dumped, got:\n{stdout}"
    );

    // Machine mode refuses structurally with the same facts.
    let out = facts_cmd(
        &h,
        &["facts", "watch", "costs/total", "--since", "0", "--json"],
    )
    .output()
    .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["reattach"], serde_json::json!(true));
    assert_eq!(v["revision"], serde_json::json!(17));
    assert_eq!(v["pending"], serde_json::json!(17));

    // Re-attaching at the named cursor lands current with no replay.
    let out = facts_cmd(
        &h,
        &["facts", "watch", "costs/total", "--since", "17", "--json"],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(17));
    assert_eq!(v["reattach"], serde_json::json!(false));
    assert_eq!(v["deliveries"].as_array().unwrap().len(), 0);
}

#[test]
fn watch_changes_no_document_ledger_or_log_state() {
    let h = setup("readonly");
    seed_ledger_and_logs(&h);
    fs::create_dir_all(&h.state).unwrap();
    write_ok(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "7",
        ],
    );

    let before = snapshot(&h.state);
    assert!(
        before.iter().any(|(rel, _, _)| rel == "facts.json"),
        "document seeded"
    );
    assert!(
        before.iter().any(|(rel, _, _)| rel == "facts-events.jsonl"),
        "journal seeded by the accepted write"
    );

    for args in [
        vec!["facts", "watch", "costs/total"],
        vec!["facts", "watch", "costs/total", "--since", "0"],
        vec!["facts", "watch", "costs/total", "--since", "0", "--json"],
        vec!["facts", "watch", "missing/path"],
        vec!["facts", "watch", "costs/total", "--since", "99"],
    ] {
        let _ = facts_cmd(&h, &args).output().unwrap();
        assert_eq!(
            snapshot(&h.state),
            before,
            "watch {args:?} changed state (bytes or mtime)"
        );
    }
}

#[test]
fn watch_absent_document_attaches_empty_without_creating_state() {
    let h = setup("watch-absent");
    // No state dir contents at all: no document, no journal, no ledger.

    let out = facts_cmd(&h, &["facts", "watch", "costs/total"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("revision 0"), "got:\n{text}");
    assert!(
        text.to_lowercase().contains("no value set"),
        "missing path stated plainly, got:\n{text}"
    );

    let out = facts_cmd(&h, &["facts", "watch", "costs/total", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(v["revision"], serde_json::json!(0));
    assert_eq!(v["missing"], serde_json::json!(true));
    assert_eq!(v["deliveries"].as_array().unwrap().len(), 0);

    assert!(
        !h.state.join("facts.json").exists(),
        "watch creates no document"
    );
    assert!(
        !h.state.join("facts-events.jsonl").exists(),
        "watch creates no journal"
    );
}

#[test]
fn watch_future_cursor_is_refused_with_current_revision() {
    let h = setup("future");
    seed_ledger_and_logs(&h);
    fs::create_dir_all(&h.state).unwrap();
    write_ok(
        &h,
        &[
            "facts",
            "create",
            "costs/total",
            "--revision",
            "0",
            "--value",
            "1",
        ],
    );
    write_ok(
        &h,
        &[
            "facts",
            "set",
            "costs/total",
            "--revision",
            "1",
            "--value",
            "2",
        ],
    );

    let before = snapshot(&h.state);
    let out = facts_cmd(&h, &["facts", "watch", "costs/total", "--since", "9"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains('9') && err.contains('2'),
        "names presented and current revisions, got:\n{err}"
    );
    assert_eq!(snapshot(&h.state), before, "refusal changed nothing");
}

#[test]
fn watch_malformed_path_shows_accepted_shape() {
    let h = setup("badpath");
    fs::create_dir_all(&h.state).unwrap();

    let out = facts_cmd(&h, &["facts", "watch", "costs//total"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("slash-separated"),
        "accepted shape shown, got:\n{err}"
    );
}
