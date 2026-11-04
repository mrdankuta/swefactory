//! Ticket detail + Run buttons (ticket #138, spec #135).
//!
//! Seam: pure render/mapping functions over the `BoardIssue` board seam,
//! the watched factory cache (freeze hash), and `hc` terminal state.
//! Run buttons invoke the existing command functions; only the
//! exit-to-outcome mapping is asserted here (no subprocesses, no `gh`).

use factory_lib::{
    run_outcome, ticket_acceptance_criteria, ticket_checks_output,
    ticket_detail_runs_fragment_html, ticket_detail_runs_json, ticket_freeze_missing_sentence,
    TicketDetail,
};

fn detail() -> TicketDetail {
    TicketDetail {
        number: 138,
        title: "Ticket detail + Run buttons".to_string(),
        body: "## What to build\n\nDo the thing.\n\n## Acceptance criteria\n\n- [ ] Detail shows freeze hash\n- [ ] Run buttons execute sync, check, handoff\n".to_string(),
        labels: vec!["ticket".to_string()],
        state: "open".to_string(),
        attempts: vec![],
        transcript: None,
    }
}

#[test]
fn run_outcome_names_pass_fail_with_cause() {
    let pass = run_outcome("sync", 0, "sync passed");
    assert!(pass.passed);
    assert_eq!(pass.command, "sync");
    assert_eq!(pass.cause, "sync passed");
    let fail = run_outcome("check", 1, "check failed — adapter exited 1");
    assert!(!fail.passed);
    assert!(fail.cause.contains("check failed"));
    let bare = run_outcome("handoff", 2, "");
    assert!(!bare.passed);
    assert!(bare.cause.contains("handoff"));
}

#[test]
fn acceptance_criteria_parses_checklist_only() {
    let items = ticket_acceptance_criteria(&detail().body);
    assert_eq!(items.len(), 2);
    assert!(items[0].contains("freeze hash"));
    let none = ticket_acceptance_criteria("no headings here");
    assert!(none.is_empty());
}

#[test]
fn checks_output_falls_back_to_calm_sentence() {
    let out = ticket_checks_output(&detail());
    assert!(!out.is_empty());
    assert!(out.contains("No attempts yet"));
}

#[test]
fn runs_fragment_carries_freeze_criteria_checks_buttons() {
    let d = detail();
    let html = ticket_detail_runs_fragment_html(&d, Some("abc123"));
    assert!(html.contains("data-freeze-hash"));
    assert!(html.contains("abc123"));
    assert!(html.contains("data-acceptance-criteria"));
    assert!(html.contains("data-criterion"));
    assert!(html.contains("data-checks-output"));
    for cmd in ["sync", "check", "handoff"] {
        assert!(html.contains(&format!("data-run=\"{cmd}\"")));
    }
    // Glossary terms exactly.
    assert!(html.contains("AgentRunner"));
    assert!(html.contains("session identity"));
    assert!(html.contains("verdict tag"));
}

#[test]
fn runs_fragment_names_missing_freeze_calmly() {
    let d = detail();
    let html = ticket_detail_runs_fragment_html(&d, None);
    assert!(html.contains("data-freeze-missing"));
    assert!(!ticket_freeze_missing_sentence().is_empty());
}

#[test]
fn runs_json_joins_freeze_criteria_checks() {
    let d = detail();
    let v = ticket_detail_runs_json(&d, Some("abc123"));
    assert_eq!(v["freeze_hash"], serde_json::json!("abc123"));
    assert_eq!(v["acceptance_criteria"].as_array().unwrap().len(), 2);
    assert!(v["checks_output"]
        .as_str()
        .unwrap()
        .contains("No attempts yet"));
    let v = ticket_detail_runs_json(&d, None);
    assert!(v["freeze_hash"].is_null());
}
