//! Factory desktop shell backend (ticket #156, spec #149).
//!
//! The backend IS the shell: every command calls `factory_lib`
//! directly in-process (no daemon, no new API layer). `hc` stays a
//! sidecar binary (`FACTORY_HC_BIN`-overridable); the app never owns
//! PTYs. Presence is Program Status Protocol only, never inferred.

use factory_lib::{
    agents_resync, desktop_board_feed, desktop_board_select, desktop_detail_runs_json,
    desktop_review_html, desktop_shell_boot, hc_terminal_list_json, list_factory_projects,
    parse_hc_terminal_list, parse_verdict_tag, peek_read as peek_screen,
    review_route, review_route_target, review_spawn_argv, review_terminal_name, sort_agents_sidebar,
    ticket_detail as fetch_ticket_detail, ticket_detail_freeze_hash, ticket_run_check, ticket_run_handoff, ticket_run_sync,
    AgentTerminal, DesktopBoardCard, DesktopBoardFeed,
};
use std::collections::{HashMap, HashSet};

#[tauri::command]
fn shell_boot() -> serde_json::Value {
    desktop_shell_boot()
}

#[tauri::command]
fn list_projects(root: Option<String>) -> Vec<String> {
    let base = root.unwrap_or_else(|| ".".to_string());
    list_factory_projects(std::path::Path::new(&base))
}

#[tauri::command]
fn agents_snapshot() -> Vec<AgentTerminal> {
    // Live read: the frontend holds no hc access, so the backend owns
    // the poll — list via the hc seam, screens via the peek seam.
    let list_json = hc_terminal_list_json().unwrap_or_default();
    let screens: HashMap<String, String> = parse_hc_terminal_list(&list_json)
        .iter()
        .filter_map(|(address, _)| {
            peek_screen(address).map(|screen| (address.clone(), screen))
        })
        .collect();
    let mut terms = agents_resync(&list_json, &screens, &HashSet::new());
    // Presence for terminals the list did not name is impossible; the
    // resync already covers every listed address.
    sort_agents_sidebar(&mut terms);
    void_seen_done(&mut terms);
    terms
}

/// The client tracks seen-Done itself; a fresh snapshot never claims it.
fn void_seen_done(terms: &mut [AgentTerminal]) {
    for t in terms.iter_mut() {
        t.seen_done = false;
    }
}

#[tauri::command]
fn board_feed(last: Option<DesktopBoardFeed>) -> DesktopBoardFeed {
    // Read-only poll of the tracker (`gh issue list` only); an
    // unreachable tracker re-serves the last cache with a stale banner.
    // No client board database; Issues stay canonical.
    desktop_board_feed(last.as_ref())
}

#[tauri::command]
fn board_select(feed: DesktopBoardFeed, number: u64) -> Option<DesktopBoardCard> {
    // Pure over the feed — selecting a card scopes the detail pane;
    // never writes to the tracker.
    desktop_board_select(&feed, number)
}

#[tauri::command]
fn ticket_detail(number: u64, tail: usize) -> Option<serde_json::Value> {
    // Freeze hash comes from the watched factory cache, joined with
    // tracker + hc state.
    let detail = fetch_ticket_detail(number, tail)?;
    let freeze = ticket_detail_freeze_hash();
    Some(desktop_detail_runs_json(&detail, freeze.as_deref()))
}

#[tauri::command]
fn run_sync(issue: String) -> factory_lib::TicketRunOutcome {
    ticket_run_sync(&issue)
}

#[tauri::command]
fn run_check() -> factory_lib::TicketRunOutcome {
    ticket_run_check()
}

#[tauri::command]
fn run_handoff(issue: String, signal: String) -> factory_lib::TicketRunOutcome {
    ticket_run_handoff(&issue, &signal)
}

#[tauri::command]
fn peek_read(addr: String) -> Option<String> {
    peek_screen(&addr)
}

#[tauri::command]
fn window_close(terms: Vec<AgentTerminal>) -> Vec<AgentTerminal> {
    // Detach peeks only — terminals keep running in the hc core.
    let mut terms = terms;
    factory_lib::detach_all_peeks(&mut terms);
    terms
}

#[tauri::command]
fn move_to_review(ticket: u64, attempt: u64, review_cmd: String) -> Vec<String> {
    // Kept `--bg --keep` hc pane; spawning stays behind the hc seam;
    // the returned row joins the agents sidebar.
    review_spawn_argv(ticket, attempt, &review_cmd)
}

#[tauri::command]
fn review_detail(ticket: u64, attempt: u64, diff_stat: String, transcript: String) -> String {
    // Diff summary + verdict tag payload + findings inline, plus the
    // peekable review terminal address.
    let addr = review_terminal_name(ticket, attempt);
    desktop_review_html(ticket, attempt, &diff_stat, &transcript, &addr)
}

#[tauri::command]
fn review_accept(transcript: String) -> String {
    // Accept reads only the schema-validated verdict tag decision;
    // `accept` routes to accepted/land, anything else to To Do.
    let route = review_route(parse_verdict_tag(&transcript).ok().as_deref());
    review_route_target(route).to_string()
}

#[tauri::command]
fn review_request_changes(ticket: u64) -> String {
    // Explicit Request-changes always returns the ticket to To Do.
    let _ = ticket;
    review_route_target(factory_lib::ReviewRoute::RequestChanges).to_string()
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            shell_boot,
            list_projects,
            agents_snapshot,
            board_feed,
            board_select,
            ticket_detail,
            run_sync,
            run_check,
            run_handoff,
            peek_read,
            window_close,
            move_to_review,
            review_detail,
            review_accept,
            review_request_changes
        ])
        .run(tauri::generate_context!())
        .expect("factory shell failed to boot");
}
