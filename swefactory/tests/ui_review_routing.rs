//! Review auto-spawn + verdict routing (ticket #141, spec #135).
//!
//! Seam: pure argv builders, verdict-tag extraction, route mapping, and
//! HTML markers — never a live `hc` core, webview, or tracker. Unit pins
//! live in `lib.rs`; these integration tests assert the ticket's four
//! acceptance criteria end to end over fixtures: (1) move-to-Review
//! auto-spawns a kept hc review terminal visible in the sidebar, (2)
//! detail shows diff summary + verdict tag payload + findings inline,
//! (3) Accept routes to accepted/land and Request-changes to To Do,
//! (4) verdict extraction validates schema with last-match-wins.

use factory_lib::{
    desktop_review_html, parse_verdict_tag, peek_attach, review_diff_summary, review_findings,
    review_route, review_route_target, review_sidebar_entry, review_spawn_argv,
    review_terminal_name, sort_agents_sidebar, AgentPresence, AgentTerminal, ReviewRoute,
};

#[test]
fn move_to_review_spawns_kept_terminal() {
    // Acceptance: auto-spawned review agent is a kept hc terminal.
    let argv = review_spawn_argv(141, 1, "factory review 141 --attempt 1");
    assert!(argv.contains(&"--keep".to_string()), "kept pane: {argv:?}");
    assert!(argv.contains(&"--bg".to_string()), "background: {argv:?}");
    assert!(
        argv.contains(&review_terminal_name(141, 1)),
        "named pane: {argv:?}"
    );
    assert_eq!(review_terminal_name(141, 1), "t141-review-a1");
}

#[test]
fn review_terminal_is_visible_in_sidebar_and_peekable() {
    // Acceptance: the kept terminal shows as a sidebar row and peeks.
    let mut row = review_sidebar_entry(141, 1);
    let mut sidebar: Vec<AgentTerminal> = vec![AgentTerminal::new("m/impl", "impl"), row.clone()];
    sidebar[1].presence = Some(AgentPresence::Working);
    sort_agents_sidebar(&mut sidebar);
    assert!(
        sidebar.iter().any(|t| t.address == row.address),
        "review row visible: {:?}",
        sidebar.iter().map(|t| &t.address).collect::<Vec<_>>()
    );
    // Peekable from the ticket: same attach as the agents sidebar.
    assert!(!row.attached);
    peek_attach(&mut row);
    assert!(row.attached);
}

#[test]
fn detail_shows_diff_summary_verdict_payload_and_findings() {
    // Acceptance: detail carries diff summary + verdict tag payload + findings.
    let transcript = "looks good\nVERDICT: accept\n<verdict>{\"decision\":\"accept\"}</verdict>\n";
    let html = desktop_review_html(141, 1, " w.rs | 2 +-", transcript, "t141-review-a1");
    assert!(html.contains("data-diff-summary"), "diff summary: {html}");
    assert!(html.contains("w.rs"), "diff text: {html}");
    assert!(html.contains("data-verdict-payload"), "payload: {html}");
    assert!(html.contains("accept"), "decision: {html}");
    assert!(html.contains("data-findings"), "findings: {html}");
    assert!(html.contains("looks good"), "finding text: {html}");
    assert!(html.contains("data-review-terminal"), "peekable terminal: {html}");
    assert!(html.contains("t141-review-a1"), "terminal addr: {html}");
    // Glossary terms exactly.
    assert!(html.contains("AgentRunner"), "glossary: {html}");
    assert!(html.contains("session identity"), "glossary: {html}");
    assert!(html.contains("verdict tag"), "glossary: {html}");
}

#[test]
fn accept_routes_to_land_request_changes_to_todo() {
    // Acceptance: Accept -> accepted/land; Request-changes -> To Do.
    assert_eq!(review_route(Some("accept")), ReviewRoute::Accept);
    assert_eq!(
        review_route_target(review_route(Some("accept"))),
        "accepted/land"
    );
    assert_eq!(
        review_route(Some("changes_requested")),
        ReviewRoute::RequestChanges
    );
    assert_eq!(
        review_route_target(review_route(Some("changes_requested"))),
        "To Do"
    );
    // Silent reviewers never read as accepted.
    assert_eq!(review_route(None), ReviewRoute::RequestChanges);
    assert_eq!(
        review_route_target(review_route(None)),
        "To Do"
    );
}

#[test]
fn verdict_extraction_validates_schema_last_match_wins() {
    // Acceptance: schema-validated, last-match-wins (parser owns it;
    // routing reads the final word through it).
    assert_eq!(
        parse_verdict_tag("first <verdict>{\"decision\":\"accept\"}</verdict> then <verdict>{\"decision\":\"changes_requested\"}</verdict>").unwrap(),
        "changes_requested"
    );
    assert!(parse_verdict_tag("VERDICT: accept\n").is_err());
    assert!(parse_verdict_tag("<verdict>{\"decision\":\"ship it\"}</verdict>").is_err());
    assert!(parse_verdict_tag("<verdict>not json</verdict>").is_err());
}

#[test]
fn findings_strip_tags_and_empty_diff_reads_calmly() {
    let findings = review_findings("a\n<verdict>{\"decision\":\"accept\"}</verdict>\nb");
    assert!(!findings.contains("<verdict>"), "tags stripped: {findings}");
    assert!(findings.contains('a') && findings.contains('b'));
    assert_eq!(review_diff_summary("   "), "(no changes)");
    assert_eq!(review_diff_summary(" f.rs | 1 +"), "f.rs | 1 +");
}
