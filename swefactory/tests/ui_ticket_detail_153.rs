//! Ticket detail + Run buttons in the desktop shell (ticket #153, spec #149).
//!
//! Seam: pure desktop wrappers over the detail-plus-runs join — freeze
//! hash from the watched factory cache (spec #149), acceptance
//! criteria, checks output — plus the Tauri backend command contract
//! (`desktop_shell_boot`). Run buttons invoke the existing command
//! functions; only the exit-to-outcome mapping is asserted here
//! (no subprocesses, no `gh`).

use factory_lib::{
    desktop_detail_runs_html, desktop_detail_runs_json, desktop_shell_boot,
    ticket_acceptance_criteria, ticket_checks_output, ticket_run_sync, TicketDetail,
    DESKTOP_DETAIL_SPEC_ISSUE,
};

fn detail() -> TicketDetail {
    TicketDetail {
        number: 153,
        title: "Ticket detail + Run buttons".to_string(),
        body: "## What to build\n\nDo the thing.\n\n## Acceptance criteria\n\n- [ ] Detail shows freeze hash, acceptance criteria, checks output\n- [ ] Run buttons execute sync, check, handoff with inline pass/fail\n".to_string(),
        labels: vec!["ticket".to_string()],
        state: "open".to_string(),
        attempts: vec![],
        transcript: None,
    }
}

#[test]
fn desktop_spec_issue_is_149() {
    assert_eq!(DESKTOP_DETAIL_SPEC_ISSUE, 149);
}

#[test]
fn desktop_fragment_carries_freeze_criteria_checks_buttons() {
    let d = detail();
    let html = desktop_detail_runs_html(&d, Some("4a00ee8a"));
    assert!(html.contains("data-freeze-hash"));
    assert!(html.contains("4a00ee8a"));
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
fn desktop_fragment_names_missing_freeze_for_149() {
    let d = detail();
    let html = desktop_detail_runs_html(&d, None);
    assert!(html.contains("data-freeze-missing"));
    assert!(html.contains("149"));
}

#[test]
fn desktop_json_joins_freeze_criteria_checks() {
    let d = detail();
    let value = desktop_detail_runs_json(&d, Some("4a00ee8a"));
    assert_eq!(value["freeze_hash"], serde_json::Value::from("4a00ee8a"));
    assert_eq!(value["acceptance_criteria"].as_array().unwrap().len(), 2);
    assert!(!value["checks_output"].as_str().unwrap().is_empty());
    let missing = desktop_detail_runs_json(&d, None);
    assert!(missing["freeze_hash"].is_null());
}

#[test]
fn boot_contract_advertises_detail_and_run_commands() {
    let boot = desktop_shell_boot();
    for cmd in ["ticket_detail", "run_sync", "run_check", "run_handoff"] {
        assert!(
            boot["commands"].get(cmd).is_some(),
            "boot must advertise {cmd}"
        );
    }
}

#[test]
fn criteria_and_checks_helpers_cover_detail() {
    let d = detail();
    assert_eq!(ticket_acceptance_criteria(&d.body).len(), 2);
    assert!(!ticket_checks_output(&d).is_empty());
    // run_outcome mapping smoke: sync against a bogus issue fails calmly
    // with the cause named (no panic, no subprocess beyond cmd_sync).
    let outcome = ticket_run_sync("not-a-ticket-xyz");
    assert!(!outcome.passed);
    assert!(outcome.cause.contains("sync failed"));
}
