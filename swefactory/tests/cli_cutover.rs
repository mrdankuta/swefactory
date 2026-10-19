//! Cutover importer CLI boundary tests (ticket #88).
//!
//! Seam: argv in -> exit code + stdout/stderr + state-dir files out. A
//! checked-in fixture `.db` (built once with the `sqlite3` CLI; the exact
//! SQL lives in `tests/fixtures/cutover/README.md`) stands in for the
//! bash dispatcher's history: `attempts` + `releases` rows. No live
//! state, no network, and no `sqlite3` at test time — the importer is
//! pure Rust and so are these tests.
//!
//! What these tests catch: row-for-row import of every attempt field,
//! the open train carried into `train.json`, the source `.db` archived
//! byte-identical and never mutated, clean refusal on re-run, and
//! `dispatch status` / `log` reading the same facts after the import
//! that `sqlite3` reads before it.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-cutover-{}-{}-{}",
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

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cutover")
        .join(name)
}

/// Stage a state dir around a copy of the fixture `.db`: the copy plays
/// the bash loop's `dispatch.db`, and the canned transcripts are what the
/// fixture's (relative) `log_path` values point at. Returns the state dir
/// plus the fixture bytes for byte-identity proofs.
fn stage(name: &str, fixture_db: &str) -> (PathBuf, PathBuf, Vec<u8>) {
    let root = tmpdir(name);
    let state = root.join("state");
    fs::create_dir_all(state.join("logs")).unwrap();
    let src = fixture(fixture_db);
    let before = fs::read(&src).expect("fixture .db must exist");
    fs::copy(&src, state.join("dispatch.db")).unwrap();
    (root, state, before)
}

fn factory(state: &Path) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    // Relative fixture log_path values resolve against the working
    // directory, exactly like a bash loop run from its repo root with a
    // relative STATE_DIR.
    cmd.env("FACTORY_STATE_DIR", state).current_dir(state);
    cmd
}

#[test]
fn import_maps_fixture_rows_and_archives_untouched() {
    let (_root, state, before) = stage("import", "dispatch-fixture.db");
    fs::write(
        state.join("logs/T101-a1-impl.log"),
        "impl line 1\nimpl line 2\n",
    )
    .unwrap();
    fs::write(state.join("logs/T101-a1-review.log"), "review line 1\n").unwrap();
    fs::write(state.join("logs/T101-a2-impl.log"), "second attempt\n").unwrap();
    fs::write(state.join("logs/T102-a1-impl.log"), "ticket 102\n").unwrap();

    factory(&state)
        .args(["dispatch", "import"])
        .assert()
        .success();

    // Every attempt row lands field-for-field in the JSON ledger.
    let ledger: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state.join("dispatch.json")).expect("ledger must exist"),
    )
    .unwrap();
    let attempts = ledger.get("attempts").unwrap().as_array().unwrap();
    assert_eq!(attempts.len(), 3, "all fixture attempts imported");
    assert_eq!(
        attempts[0],
        serde_json::json!({
            "id": 1, "ticket": 101, "attempt_no": 1,
            "base_commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "head_commit": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "worktree": "/tmp/wt/T101-a1",
            "branch": "factory/T101-first",
            "status": "impl_done",
            "exit_code": 0,
            "log_path": "logs/T101-a1-impl.log",
            "started_at": "2026-09-01 10:00:00",
            "finished_at": "2026-09-01 10:07:31",
            "train_branch": "integration/20260901-train",
            "phase": "", "resumed": 0,
        })
    );
    assert_eq!(
        attempts[1],
        serde_json::json!({
            "id": 2, "ticket": 101, "attempt_no": 2,
            "base_commit": "cccccccccccccccccccccccccccccccccccccccc",
            "head_commit": "",
            "worktree": "/tmp/wt/T101-a2",
            "branch": "factory/T101-second",
            "status": "review_failed",
            "exit_code": 1,
            "log_path": "logs/T101-a2-impl.log",
            "started_at": "2026-09-02 11:00:00",
            "finished_at": "2026-09-02 11:09:02",
            "train_branch": "",
            "phase": "", "resumed": 0,
        })
    );
    // Sparse bash rows (NULL commits, NULL exit, NULL train) read as the
    // same empty-string defaults a pre-checkpoint Rust ledger carries.
    assert_eq!(
        attempts[2],
        serde_json::json!({
            "id": 3, "ticket": 102, "attempt_no": 1,
            "base_commit": "",
            "head_commit": "",
            "worktree": "",
            "branch": "factory/T102-first",
            "status": "impl_running",
            "exit_code": 0,
            "log_path": "logs/T102-a1-impl.log",
            "started_at": "2026-09-03 12:00:00",
            "finished_at": "",
            "train_branch": "",
            "phase": "", "resumed": 0,
        })
    );

    // The open release becomes the Rust open train, byte for byte.
    let train: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(state.join("train.json")).expect("train must exist"),
    )
    .unwrap();
    assert_eq!(
        train,
        serde_json::json!({
            "branch": "integration/20260901-train",
            "base_commit": "dddddddddddddddddddddddddddddddddddddddd",
            "status": "open",
            "created_at": "2026-09-01 09:00:00",
        })
    );

    // The source .db is archived byte-identical and never mutated.
    let archived = fs::read(state.join("dispatch.db.archived")).expect("archive must exist");
    assert_eq!(archived, before, "archive is byte-identical to the source");
    let after = fs::read(state.join("dispatch.db")).unwrap();
    assert_eq!(after, before, "source .db untouched by the import");
}

#[test]
fn rerun_against_imported_state_refuses_without_duplicating() {
    let (_root, state, _before) = stage("rerun", "dispatch-fixture.db");
    fs::write(state.join("logs/T101-a1-impl.log"), "impl\n").unwrap();
    fs::write(state.join("logs/T101-a2-impl.log"), "impl\n").unwrap();
    fs::write(state.join("logs/T102-a1-impl.log"), "impl\n").unwrap();

    factory(&state)
        .args(["dispatch", "import"])
        .assert()
        .success();
    let ledger_once = fs::read(state.join("dispatch.json")).unwrap();
    let archive_once = fs::read(state.join("dispatch.db.archived")).unwrap();

    // A second pass refuses cleanly instead of duplicating rows.
    factory(&state)
        .args(["dispatch", "import"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("already exists"))
        .stderr(predicates::str::contains("refusing to duplicate"));
    assert_eq!(
        fs::read(state.join("dispatch.json")).unwrap(),
        ledger_once,
        "failed re-run changes nothing"
    );
    assert_eq!(
        fs::read(state.join("dispatch.db.archived")).unwrap(),
        archive_once,
        "archive untouched by the refused re-run"
    );

    // The refusal survives process boundaries: JSON output refuses too.
    factory(&state)
        .args(["--json", "dispatch", "import"])
        .assert()
        .failure();
}

/// Stage the main fixture, import it, and leave canned transcripts in
/// place: the shared setup for every read-parity test below.
fn imported(name: &str) -> (PathBuf, PathBuf) {
    let (root, state, _before) = stage(name, "dispatch-fixture.db");
    fs::write(
        state.join("logs/T101-a1-impl.log"),
        "impl line 1\nimpl line 2\n",
    )
    .unwrap();
    fs::write(state.join("logs/T101-a1-review.log"), "review line 1\n").unwrap();
    fs::write(state.join("logs/T101-a2-impl.log"), "second attempt\n").unwrap();
    fs::write(state.join("logs/T102-a1-impl.log"), "ticket 102\n").unwrap();
    factory(&state)
        .args(["dispatch", "import"])
        .assert()
        .success();
    (root, state)
}

#[test]
fn status_reads_imported_facts_human_and_json() {
    let (_root, state) = imported("status");

    // Human: one line per attempt with the same ids, tickets, statuses,
    // and short commits `sqlite3` shows for the fixture rows.
    factory(&state)
        .args(["dispatch", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("#1 T101 a1 impl_done"))
        .stdout(predicates::str::contains("#2 T101 a2 review_failed"))
        .stdout(predicates::str::contains("#3 T102 a1 impl_running"))
        .stdout(predicates::str::contains("base=aaaaaaaa"))
        .stdout(predicates::str::contains("head=bbbbbbbb"))
        .stdout(predicates::str::contains("2026-09-01 10:00:00"));

    // JSON: the same rows structurally — ids, tickets, attempt numbers,
    // statuses, full commits, timestamps.
    let out = factory(&state)
        .args(["--json", "dispatch", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let attempts = status.get("attempts").unwrap().as_array().unwrap();
    assert_eq!(attempts.len(), 3);
    assert_eq!(attempts[0]["id"], 1);
    assert_eq!(attempts[0]["ticket"], 101);
    assert_eq!(attempts[0]["attempt_no"], 1);
    assert_eq!(attempts[0]["status"], "impl_done");
    assert_eq!(
        attempts[0]["base_commit"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(attempts[0]["started_at"], "2026-09-01 10:00:00");
    assert_eq!(attempts[2]["status"], "impl_running");
    assert_eq!(attempts[2]["exit_code"], 0);
}

#[test]
fn log_reads_imported_transcripts_human_and_json() {
    let (_root, state) = imported("log");

    // Ticket 101: newest-first attempts, outcome summaries, then the
    // bounded transcripts — the implement log plus its review sibling.
    factory(&state)
        .args(["log", "101"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Ticket 101"))
        .stdout(predicates::str::contains("second attempt"))
        .stdout(predicates::str::contains("impl line 1"))
        .stdout(predicates::str::contains("review line 1"));

    let out = factory(&state)
        .args(["--json", "log", "101"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let log: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let attempts = log.get("attempts").unwrap().as_array().unwrap();
    assert_eq!(attempts.len(), 2, "newest first");
    assert_eq!(attempts[0]["attempt"], 2);
    assert_eq!(attempts[0]["status"], "review_failed");
    assert!(attempts[0]["transcript"]
        .as_str()
        .unwrap()
        .contains("second attempt"));
    assert_eq!(attempts[1]["attempt"], 1);
    let t1 = attempts[1]["transcript"].as_str().unwrap();
    assert!(t1.contains("impl line 2"), "implement log carried");
    assert!(t1.contains("review line 1"), "review sibling appended");

    // Sparse rows still log: no transcript file is missing here, and the
    // running attempt reads with its started stamp.
    factory(&state)
        .args(["log", "102"])
        .assert()
        .success()
        .stdout(predicates::str::contains("ticket 102"));
}

#[test]
fn import_without_open_train_carries_attempts_and_no_train() {
    let (_root, state, _before) = stage("no-train", "dispatch-fixture-no-train.db");
    fs::write(state.join("logs/T201-a1-impl.log"), "only attempt\n").unwrap();

    factory(&state)
        .args(["dispatch", "import"])
        .assert()
        .success()
        .stdout(predicates::str::contains("1 attempt(s)"))
        .stdout(predicates::str::contains("no open train"));
    assert!(
        !state.join("train.json").exists(),
        "no open release means no train file"
    );
    let ledger: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state.join("dispatch.json")).unwrap()).unwrap();
    assert_eq!(ledger["attempts"].as_array().unwrap().len(), 1);
    factory(&state)
        .args(["release", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("no open train"));
}

#[test]
fn import_without_history_refuses_and_names_the_db() {
    let root = tmpdir("no-db");
    let state = root.join("state");
    fs::create_dir_all(&state).unwrap();

    factory(&state)
        .args(["dispatch", "import"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no SQLite history"));
    assert!(
        !state.join("dispatch.json").exists(),
        "refused import writes nothing"
    );
}

#[test]
fn import_honours_explicit_db_path_and_reports_json() {
    let (_root, state, _before) = stage("explicit-db", "dispatch-fixture-no-train.db");
    fs::write(state.join("logs/T201-a1-impl.log"), "only attempt\n").unwrap();
    let elsewhere = state.join("elsewhere.db");
    fs::rename(state.join("dispatch.db"), &elsewhere).unwrap();

    let out = factory(&state)
        .args([
            "--json",
            "dispatch",
            "import",
            "--db",
            elsewhere.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["attempts"], 1);
    assert!(report["open_train"].is_null());
    assert!(report["archive"]
        .as_str()
        .unwrap()
        .ends_with("dispatch.db.archived"));
}
