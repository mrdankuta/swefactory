//! Answer delivery end to end (ticket #44, spec #41 slice).
//!
//! Seam: parser public API (`factory_lib::parse_answer_line`,
//! `collect_answers`, `pending_answers`) for the isolated format proof,
//! plus argv in -> exit code + implementer-prompt out for the loop proof
//! (`gh`/`git`/`pi`/adapter all faked, mirroring `cli_dispatch.rs`).
//!
//! What these tests catch: Bea's structured `Answer[id]` replies parse
//! (valid), older shapes still parse (legacy), broken markers are refused
//! (malformed); a posted answer lands in the next attempt's brief; an
//! answer arriving mid-attempt waits for the following attempt (atomic
//! boundary); a delivered answer is never redelivered (single-use); the
//! pinned routing rule travels in the brief the loop hands the worker.
//! What they miss: the real `pi` worker reading the brief, and real GitHub
//! comments (the fake `gh` serves static view files).

use assert_cmd::Command;
use factory_lib::{
    answer_delivery_key, collect_answers, parse_answer_line, pending_answers,
    render_answers_section, AnswerLine, ParsedAnswer, ANSWER_ROUTING_RULE,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

// --- parser unit tests (isolated format proof) ---

#[test]
fn structured_answer_parses() {
    match parse_answer_line("Answer[q1]: Use private by default.") {
        AnswerLine::Valid(a) => assert_eq!(
            a,
            ParsedAnswer {
                id: "q1".to_string(),
                body: "Use private by default.".to_string(),
                legacy: false,
            }
        ),
        other => panic!("structured line must parse valid, got {other:?}"),
    }
}

#[test]
fn structured_answer_tolerates_surrounding_whitespace() {
    match parse_answer_line("  Answer[re-po_2]  :  yes, go ahead \t") {
        AnswerLine::Valid(a) => {
            assert_eq!(a.id, "re-po_2");
            assert_eq!(a.body, "yes, go ahead");
            assert!(!a.legacy);
        }
        other => panic!("padded line must parse valid, got {other:?}"),
    }
}

#[test]
fn legacy_lowercase_marker_parses_with_flag() {
    match parse_answer_line("answer[q0]: legacy lowercase marker.") {
        AnswerLine::Valid(a) => {
            assert_eq!(a.id, "q0");
            assert_eq!(a.body, "legacy lowercase marker.");
            assert!(a.legacy, "lowercase marker must flag legacy");
        }
        other => panic!("legacy lowercase line must parse valid, got {other:?}"),
    }
}

#[test]
fn legacy_bracketless_shape_parses_with_flag() {
    match parse_answer_line("Answer q3: bracket-less legacy shape.") {
        AnswerLine::Valid(a) => {
            assert_eq!(a.id, "q3");
            assert_eq!(a.body, "bracket-less legacy shape.");
            assert!(a.legacy, "bracket-less shape must flag legacy");
        }
        other => panic!("legacy bracket-less line must parse valid, got {other:?}"),
    }
}

#[test]
fn malformed_markers_are_refused_with_reasons() {
    for line in [
        "Answer[]: oops no id here",
        "Answer[q1] missing the colon",
        "Answer[q1]:",
        "Answer[q1]:   ",
        "Answer[bad id!]: illegal id chars",
        "answer nonsense",
        "Answer:",
    ] {
        match parse_answer_line(line) {
            AnswerLine::Malformed(reason) => {
                assert!(!reason.trim().is_empty(), "reason must say why: {line:?}");
            }
            other => panic!("{line:?} must be malformed, got {other:?}"),
        }
    }
}

#[test]
fn chatter_is_not_an_answer() {
    for line in [
        "Looks good, keep going Bea",
        "",
        "   ",
        "SPEC-APPROVED 93114c191e0d02bb088dfa853fe9c544cff478df5f51a332f4feee6bc9d09398",
    ] {
        assert!(
            matches!(parse_answer_line(line), AnswerLine::NotAnAnswer),
            "{line:?} must be ignored"
        );
    }
}

#[test]
fn collect_scans_every_line_first_id_wins_and_reports_malformed() {
    let comments = vec![
        "Thanks!\nAnswer[q1]: first words stand.".to_string(),
        "answer[q0]: legacy line.".to_string(),
        "Answer[q1]: restated words lose to the first.".to_string(),
        "Answer[]: broken marker.".to_string(),
        "Just chatter.".to_string(),
    ];
    let (valid, malformed) = collect_answers(&comments);
    assert_eq!(
        valid,
        vec![
            ParsedAnswer {
                id: "q1".to_string(),
                body: "first words stand.".to_string(),
                legacy: false,
            },
            ParsedAnswer {
                id: "q0".to_string(),
                body: "legacy line.".to_string(),
                legacy: true,
            },
        ]
    );
    assert_eq!(malformed.len(), 1, "one broken marker expected");
}

#[test]
fn pending_drops_delivered_keys() {
    let all = vec![
        ParsedAnswer {
            id: "q1".to_string(),
            body: "seen before.".to_string(),
            legacy: false,
        },
        ParsedAnswer {
            id: "q2".to_string(),
            body: "fresh words.".to_string(),
            legacy: false,
        },
    ];
    let delivered = vec![answer_delivery_key(9, &all[0])];
    assert_eq!(pending_answers(&all, &delivered, 9), vec![all[1].clone()]);
    // Same id from another ticket is a different answer.
    assert_eq!(pending_answers(&all, &delivered, 7).len(), 2);
}

#[test]
fn rendered_section_carries_answers_and_pinned_rule() {
    let section = render_answers_section(&[ParsedAnswer {
        id: "q1".to_string(),
        body: "Use private by default.".to_string(),
        legacy: false,
    }]);
    assert!(section.contains("## Answers from Bea"), "section header");
    assert!(
        section.contains("Use private by default."),
        "answer body travels"
    );
    assert!(
        section.contains(ANSWER_ROUTING_RULE),
        "pinned rule travels in the brief"
    );
}

// --- loop-level end-to-end proof ---

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-answers-{}-{}-{}",
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

/// Fake `gh`: serves `issue list` from $GH_LIST_JSON and `issue view <n>`
/// from $GH_VIEWS_DIR/<n>.json; every other call succeeds silently.
fn write_fake_gh(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "gh",
        r#"#!/bin/sh
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
exec /usr/bin/git "$@"
"#,
    )
}

/// Fake implementer: records the exact prompt (`$2`) it was handed, then
/// simulates Bea replying mid-attempt by inserting a fresh `Answer[q2]`
/// comment into the ticket view the fake `gh` serves. The running attempt
/// already holds its brief, so q2 must wait for the next attempt.
fn write_fake_pi(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "pi",
        r#"#!/bin/sh
printf '%s\n---PROMPT-SEPARATOR---\n' "$2" >> "$PI_PROMPT_LOG"
view="$GH_VIEWS_DIR/$PI_TICKET.json"
if ! grep -q 'Answer\[q2\]' "$view"; then
  sed -i 's/"comments":\[/"comments":[{"body":"Answer[q2]: mid-attempt words."}, /' "$view"
fi
echo "change from pi" >> change.txt
echo "notes"
echo "VERDICT: accept"
echo '<verdict>{"decision":"accept"}</verdict>'
"#,
    )
}

fn write_fake_adapter(dir: &Path) -> PathBuf {
    write_exe(
        dir,
        "adapter.sh",
        r#"#!/bin/sh
echo "FACTORY-CHECK PASSED"
exit "${ADAPTER_FAIL:-0}"
"#,
    )
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

struct Harness {
    repo: PathBuf,
    state: PathBuf,
    views: PathBuf,
    prompt_log: PathBuf,
    gh: PathBuf,
    git_shim: PathBuf,
    pi: PathBuf,
    adapter: PathBuf,
}

fn setup() -> Harness {
    let root = tmpdir("loop");
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_repo(&repo);
    let views = root.join("views");
    fs::create_dir_all(&views).unwrap();
    Harness {
        state: root.join("state"),
        prompt_log: root.join("prompts.log"),
        gh: write_fake_gh(&root),
        git_shim: write_git_shim(&root),
        pi: write_fake_pi(&root),
        adapter: write_fake_adapter(&root),
        repo,
        views,
    }
}

/// Ticket view whose comments hold: one structured answer, one legacy
/// answer, one malformed marker, and plain chatter. Bodies avoid `spec:`
/// and `depends on:` so the freeze and dependency gates stay out of the way.
fn write_ticket_view(h: &Harness) {
    let comments = [
        "Answer[q1]: Use private by default, keep it simple.",
        "answer[q0]: legacy lowercase marker, still counts.",
        "Answer[]: oops no id here",
        "Looks good, keep going Bea",
    ];
    let comments_json = comments
        .iter()
        .map(|c| format!("{{\"body\":\"{}\"}}", json_escape(c)))
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        h.views.join("9.json"),
        format!(
            "{{\"number\":9,\"title\":\"Bea reply test\",\"body\":\"Bea reply test ticket.\",\"labels\":[{{\"name\":\"ticket\"}},{{\"name\":\"ready\"}}],\"state\":\"OPEN\",\"comments\":[{comments_json}]}}"
        ),
    )
    .unwrap();
}

fn dispatch_cmd(h: &Harness) -> Command {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.args(["dispatch", "once"]).current_dir(&h.repo);
    cmd.env("FACTORY_GH_BIN", &h.gh)
        .env("FACTORY_GIT_BIN", &h.git_shim)
        .env("FACTORY_PI_BIN", &h.pi)
        .env("FACTORY_CHECK_ADAPTER", &h.adapter)
        .env("FACTORY_STATE_DIR", &h.state)
        .env("GH_VIEWS_DIR", &h.views)
        .env("GH_LIST_JSON", r#"[{"number":9}]"#)
        .env("PI_PROMPT_LOG", &h.prompt_log)
        .env("PI_TICKET", "9")
        .env("ADAPTER_FAIL", "1")
        // Hermetic budgets first: scrub ambient service env so the
        // explicit non-default below is the only signal (2 -> 5, not 3 -> 5).
        .env_remove("FACTORY_SLOT")
        .env_remove("MAX_ATTEMPTS")
        .env_remove("MAX_CONCURRENT")
        .env_remove("TIMEOUT_IMPL")
        .env_remove("TIMEOUT_REVIEW")
        .env("MAX_ATTEMPTS", "5");
    cmd
}

fn prompts(h: &Harness) -> Vec<String> {
    let text = fs::read_to_string(&h.prompt_log).unwrap_or_default();
    text.split("---PROMPT-SEPARATOR---\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn ledger_delivered(h: &Harness) -> Vec<String> {
    let text = fs::read_to_string(h.state.join("dispatch.json")).unwrap();
    let ledger: serde_json::Value = serde_json::from_str(&text).unwrap();
    ledger
        .get("delivered_answers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn posted_answer_lands_in_next_brief_and_mid_attempt_arrival_waits() {
    let h = setup();
    write_ticket_view(&h);

    // Pass 1: q1 + legacy q0 posted before the attempt → in the brief.
    dispatch_cmd(&h).assert().success();
    let first = prompts(&h);
    assert_eq!(first.len(), 1, "one implement prompt per pass");
    assert!(
        first[0].contains("Use private by default, keep it simple."),
        "posted structured answer visible in next brief:\n{}",
        first[0]
    );
    assert!(
        first[0].contains("legacy lowercase marker, still counts."),
        "legacy answer still delivered:\n{}",
        first[0]
    );
    assert!(
        first[0].contains("## Answers from Bea"),
        "answers section header in brief"
    );
    assert!(
        first[0].contains("atomic boundary"),
        "pinned routing rule travels in the brief:\n{}",
        first[0]
    );
    assert!(
        !first[0].contains("mid-attempt words"),
        "mid-attempt arrival waits for the next attempt:\n{}",
        first[0]
    );
    assert!(
        !first[0].contains("oops no id here"),
        "malformed marker never delivered:\n{}",
        first[0]
    );
    let delivered = ledger_delivered(&h);
    assert!(
        delivered.iter().any(|k| k.starts_with("9:q1:")),
        "q1 delivery recorded single-use: {delivered:?}"
    );

    // Pass 2: the mid-attempt q2 arrival lands; q1 is not redelivered.
    dispatch_cmd(&h).assert().success();
    let all = prompts(&h);
    assert_eq!(all.len(), 2, "second pass hands a second brief");
    assert!(
        all[1].contains("mid-attempt words"),
        "waited answer lands in the following brief:\n{}",
        all[1]
    );
    assert!(
        !all[1].contains("Use private by default"),
        "delivered answer never redelivered:\n{}",
        all[1]
    );
}
