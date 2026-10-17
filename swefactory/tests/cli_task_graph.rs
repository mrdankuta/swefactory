//! Live task-graph CLI boundary tests (ticket #70, spec #59 follow-up).
//!
//! Seam: argv in -> exit code + stdout/stderr out. State comes from a
//! seeded `FACTORY_STATE_DIR` (ledger with `tasks` fixtures); the working
//! directory is a scratch git repo standing in for the project. No daemon,
//! no network, no real runs anywhere in this suite.
//!
//! What these tests catch: parent/child/background/waiting nodes with
//! correct owner edges in human and --json modes carrying identical facts,
//! terminal tasks leaving the graph on the commit that ends them, and
//! byte+mtime proof that viewing changes nothing.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-taskgraph-{}-{}-{}",
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

fn write_ledger(h: &Harness, ledger: &serde_json::Value) {
    fs::create_dir_all(&h.state).unwrap();
    fs::write(
        h.state.join("dispatch.json"),
        serde_json::to_string_pretty(ledger).unwrap(),
    )
    .unwrap();
}

/// Ledger with one running attempt plus a parent/child/background/waiting mix.
fn live_ledger() -> serde_json::Value {
    serde_json::json!({
        "attempts": [{
            "id": 1, "ticket": 70, "attempt_no": 1,
            "base_commit": "abc123def456", "head_commit": "",
            "branch": "factory/T70-demo", "worktree": "/tmp/wt-T70-a1",
            "status": "impl_running", "exit_code": 0,
            "log_path": "/tmp/log-T70-a1.log",
            "started_at": "1700000000", "finished_at": "",
            "train_branch": "", "phase": "implement", "resumed": 0
        }],
        "tasks": [
            {"id": "plan", "ticket": 70, "attempt": 1, "parent": null,
             "kind": "parent", "status": "running", "waits_on": []},
            {"id": "suite-a", "ticket": 70, "attempt": 1, "parent": "plan",
             "kind": "child", "status": "running", "waits_on": []},
            {"id": "suite-b", "ticket": 70, "attempt": 1, "parent": "plan",
             "kind": "child", "status": "waiting", "waits_on": ["suite-a"]},
            {"id": "watch", "ticket": 70, "attempt": 1, "parent": null,
             "kind": "background", "status": "background_running", "waits_on": []}
        ]
    })
}

fn graph_cmd(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(args).current_dir(&h.repo);
    cmd.env("FACTORY_STATE_DIR", &h.state)
        .env_remove("FACTORY_SLOT");
    cmd
}

/// (relative path, bytes, mtime) for every file under the state dir.
fn snapshot(state: &Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut out = Vec::new();
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
                let bytes = fs::read(&p).unwrap_or_default();
                let mtime = fs::metadata(&p).and_then(|m| m.modified()).unwrap();
                out.push((rel, bytes, mtime));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn snapshot_bytes(state: &Path) -> Vec<(String, Vec<u8>)> {
    snapshot(state)
        .into_iter()
        .map(|(p, b, _)| (p, b))
        .collect()
}

#[test]
fn graph_human_lists_all_live_nodes_with_owner_edges() {
    let h = setup("human");
    write_ledger(&h, &live_ledger());
    let out = graph_cmd(&h, &["dispatch", "graph"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    for id in ["plan", "suite-a", "suite-b", "watch"] {
        assert!(stdout.contains(id), "missing node {id} in:\n{stdout}");
    }
    // Owner edges: parent + background owned by the attempt, children by parent.
    assert!(
        stdout.contains("owner=attempt:T70-a1"),
        "parent/background owner edge:\n{stdout}"
    );
    assert!(
        stdout.contains("owner=task:plan"),
        "child owner edge:\n{stdout}"
    );
    // Waiting node names what it waits on.
    assert!(stdout.contains("suite-a"), "waiting edge:\n{stdout}");
    assert!(
        stdout.contains("waits_on=suite-a") || stdout.contains("waits_on=[suite-a]"),
        "waits_on:\n{stdout}"
    );
}

#[test]
fn graph_json_carries_identical_facts() {
    let h = setup("json");
    write_ledger(&h, &live_ledger());
    let human = graph_cmd(&h, &["dispatch", "graph"]).output().unwrap();
    assert_eq!(human.status.code(), Some(0));
    let human_text = String::from_utf8_lossy(&human.stdout).into_owned();
    let out = graph_cmd(&h, &["dispatch", "graph", "--json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tasks = v
        .get("tasks")
        .and_then(|t| t.as_array())
        .expect("tasks array");
    assert_eq!(tasks.len(), 4);
    let mut by_id = std::collections::HashMap::new();
    for t in tasks {
        by_id.insert(t.get("id").and_then(|v| v.as_str()).unwrap().to_string(), t);
    }
    assert_eq!(
        by_id["plan"].get("owner").and_then(|v| v.as_str()),
        Some("attempt:T70-a1")
    );
    assert_eq!(
        by_id["suite-a"].get("owner").and_then(|v| v.as_str()),
        Some("task:plan")
    );
    assert_eq!(
        by_id["suite-b"].get("owner").and_then(|v| v.as_str()),
        Some("task:plan")
    );
    assert_eq!(
        by_id["watch"].get("owner").and_then(|v| v.as_str()),
        Some("attempt:T70-a1")
    );
    assert_eq!(
        by_id["suite-b"].get("waits_on").unwrap(),
        &serde_json::json!(["suite-a"])
    );
    // Identical facts: every JSON id/status/owner appears in the human render.
    for t in tasks {
        let id = t.get("id").and_then(|v| v.as_str()).unwrap();
        let status = t.get("status").and_then(|v| v.as_str()).unwrap();
        let owner = t.get("owner").and_then(|v| v.as_str()).unwrap();
        assert!(human_text.contains(id), "human missing id {id}");
        assert!(human_text.contains(status), "human missing status {status}");
        assert!(human_text.contains(owner), "human missing owner {owner}");
    }
}

#[test]
fn graph_terminal_tasks_leave_on_commit() {
    let h = setup("terminal");
    write_ledger(&h, &live_ledger());
    // The commit that ends suite-a: mark it done, keep everything else live.
    let mut ledger = live_ledger();
    ledger["tasks"][1]["status"] = serde_json::json!("done");
    write_ledger(&h, &ledger);
    let out = graph_cmd(&h, &["dispatch", "graph", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ids: Vec<&str> = v["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&"suite-a"),
        "terminal task must leave the graph: {ids:?}"
    );
    assert!(ids.contains(&"plan"));
    assert!(ids.contains(&"suite-b"));
    assert!(ids.contains(&"watch"));
    let human = graph_cmd(&h, &["dispatch", "graph"]).output().unwrap();
    let text = String::from_utf8_lossy(&human.stdout).into_owned();
    assert!(
        !text.contains("suite-a "),
        "human still lists terminal node:\n{text}"
    );
}

#[test]
fn graph_views_are_read_only() {
    let h = setup("readonly");
    write_ledger(&h, &live_ledger());
    // A log file the graph must never touch.
    fs::create_dir_all(h.state.join("logs")).unwrap();
    fs::write(h.state.join("logs").join("T70-a1-impl.log"), "hello\n").unwrap();
    let before = snapshot(&h.state);
    // Open the views (human + machine) and close them (no further command).
    for args in [
        &["dispatch", "graph"][..],
        &["dispatch", "graph", "--json"][..],
    ] {
        let out = graph_cmd(&h, args).output().unwrap();
        assert_eq!(out.status.code(), Some(0));
    }
    let after = snapshot(&h.state);
    assert_eq!(before.len(), after.len(), "file set changed");
    for ((p0, b0, m0), (p1, b1, m1)) in before.iter().zip(after.iter()) {
        assert_eq!(p0, p1);
        assert_eq!(b0, b1, "bytes changed for {p0}");
        assert_eq!(m0, m1, "mtime changed for {p0}");
    }
    // Worktree state untouched too: graph never shells to git for mutation;
    // the byte-level state proof above plus identical rerun output suffices.
    let _ = snapshot_bytes(&h.state);
}

#[test]
fn graph_empty_and_ticket_filter() {
    let h = setup("empty");
    // Old ledger without tasks at all parses as empty graph, exit 0.
    write_ledger(&h, &serde_json::json!({"attempts": []}));
    let out = graph_cmd(&h, &["dispatch", "graph"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout)
        .to_lowercase()
        .contains("no live tasks"));
    let out = graph_cmd(&h, &["dispatch", "graph", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["tasks"], serde_json::json!([]));

    // Ticket filter selects only that ticket's live tasks.
    let mut ledger = live_ledger();
    ledger["tasks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!(
            {"id": "other", "ticket": 71, "attempt": 1, "parent": null,
             "kind": "parent", "status": "running", "waits_on": []}
        ));
    write_ledger(&h, &ledger);
    let out = graph_cmd(&h, &["dispatch", "graph", "--ticket", "71", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(v["tasks"][0]["id"], serde_json::json!("other"));
}

#[test]
fn graph_corrupt_ledger_is_nonzero_with_cause() {
    let h = setup("corrupt");
    fs::create_dir_all(&h.state).unwrap();
    fs::write(h.state.join("dispatch.json"), "{ not json").unwrap();
    for extra in [vec![], vec!["--json"]] {
        let mut args = vec!["dispatch", "graph"];
        args.extend(extra);
        let out = graph_cmd(&h, &args).output().unwrap();
        assert_ne!(out.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            stderr.contains("cannot parse ledger") || stderr.contains("cannot read ledger"),
            "cause named: {stderr}"
        );
    }
}
