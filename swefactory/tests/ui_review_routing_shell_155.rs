//! Review auto-spawn + verdict routing in the Tauri shell (ticket #155, spec #149).
//!
//! Seam: pins the Tauri backend command contract for the review flow —
//! move-to-Review auto-spawns a kept hc review terminal visible in the
//! sidebar, detail shows diff summary + verdict tag payload + findings
//! with Accept / Request-changes routing, and the review terminal stays
//! peekable. Pure over `factory_lib`; never a live `hc` core, webview,
//! or tracker. Verdict extraction reuses `parse_verdict_tag`
//! (schema-validated, last-match-wins).

use factory_lib::{
    desktop_review_html, desktop_shell_boot, parse_verdict_tag, peek_attach, review_route,
    review_route_target, review_sidebar_entry, review_spawn_argv, review_terminal_name,
    sort_agents_sidebar, AgentTerminal, ReviewRoute,
};

#[test]
fn boot_advertises_review_commands() {
    let boot = desktop_shell_boot();
    for cmd in [
        "move_to_review",
        "review_detail",
        "review_accept",
        "review_request_changes",
    ] {
        assert!(
            boot["commands"].get(cmd).is_some(),
            "boot must advertise {cmd}"
        );
    }
}

#[test]
fn move_to_review_spawns_kept_sidebar_terminal() {
    // Tauri `move_to_review` contract: kept `--bg --keep` hc argv under
    // the `t{ticket}-review-a{attempt}` name; the row joins the sidebar.
    let argv = review_spawn_argv(155, 2, "factory review 155 --attempt 2");
    assert!(argv.contains(&"--keep".to_string()));
    assert!(argv.contains(&"--bg".to_string()));
    assert!(argv.contains(&review_terminal_name(155, 2)));
    assert_eq!(review_terminal_name(155, 2), "t155-review-a2");
    let row = review_sidebar_entry(155, 2);
    let mut sidebar: Vec<AgentTerminal> = vec![AgentTerminal::new("m/impl", "impl"), row.clone()];
    sort_agents_sidebar(&mut sidebar);
    assert!(sidebar.iter().any(|t| t.address == row.address));
}

#[test]
fn review_detail_html_shows_diff_verdict_findings_and_peek() {
    // Tauri `review_detail` contract: diff summary + verdict tag payload
    // (last-match-wins via parse_verdict_tag) + findings + peekable terminal.
    let transcript =
        "looks good\n<verdict>{\"decision\":\"accept\"}</verdict>\nlate change\n<verdict>{\"decision\":\"changes_requested\"}</verdict>\n";
    assert_eq!(parse_verdict_tag(transcript).unwrap(), "changes_requested");
    let addr = review_terminal_name(155, 2);
    let html = desktop_review_html(155, 2, " w.rs | 2 +-", transcript, &addr);
    assert!(html.contains("data-diff-summary"));
    assert!(html.contains("data-verdict-payload"));
    assert!(html.contains("changes_requested"));
    assert!(html.contains("data-findings"));
    assert!(html.contains("data-review-accept"));
    assert!(html.contains("data-review-request-changes"));
    assert!(html.contains("data-review-peek"));
    assert!(html.contains(&addr));
    assert!(html.contains("AgentRunner"));
    assert!(html.contains("session identity"));
    assert!(html.contains("verdict tag"));
}

#[test]
fn accept_and_request_changes_routing() {
    // Tauri `review_accept` reads the validated tag; explicit
    // `review_request_changes` always returns to To Do.
    let decision = parse_verdict_tag("<verdict>{\"decision\":\"accept\"}</verdict>\n").ok();
    assert_eq!(
        review_route_target(review_route(decision.as_deref())),
        "accepted/land"
    );
    assert_eq!(review_route_target(review_route(None)), "To Do");
    assert_eq!(review_route_target(ReviewRoute::RequestChanges), "To Do");
    // Peekable from the ticket: same attach as the agents sidebar.
    let mut row = review_sidebar_entry(155, 2);
    assert!(!row.attached);
    peek_attach(&mut row);
    assert!(row.attached);
}
