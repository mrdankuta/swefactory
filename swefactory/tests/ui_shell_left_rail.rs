//! Ticket #150 (spec #149): Tauri shell skeleton + left rail.
//! The shell-plus-seam contract: backend calls factory_lib in-process,
//! left rail lists factory repos + live agents, click-to-peek renders
//! the live terminal, close-window keeps terminals running in the core.
use factory_lib::{
    desktop_agents_sidebar_html, desktop_left_rail_html, desktop_peek_html,
    desktop_window_close_policy, AgentPresence, AgentTerminal,
};
use std::collections::{HashMap, HashSet};

fn terms() -> Vec<AgentTerminal> {
    let mut blocked = AgentTerminal::new("m/blocked", "impl");
    blocked.presence = Some(AgentPresence::Blocked);
    let mut working = AgentTerminal::new("m/working", "review");
    working.presence = Some(AgentPresence::Working);
    let mut done = AgentTerminal::new("m/done", "done-job");
    done.presence = Some(AgentPresence::Done);
    vec![blocked, working, done]
}

#[test]
fn shell_boot_manifest_is_in_process_with_no_daemon() {
    let boot = factory_lib::desktop_shell_boot();
    assert_eq!(boot["backend"], "factory_lib-in-process");
    assert_eq!(boot["daemon"], serde_json::Value::Null);
    assert_eq!(boot["api_layer"], "none");
    assert_eq!(boot["terminal_owner"], "hc-core");
}

#[test]
fn window_close_keeps_terminals_running() {
    let policy = desktop_window_close_policy();
    assert_eq!(policy["on_close"], "detach-peeks");
    assert_eq!(policy["terminals_keep_running"], true);
    assert_eq!(policy["terminal_owner"], "hc-core");
}

#[test]
fn agents_sidebar_shows_presence_and_blocked_stands_out() {
    let html = desktop_agents_sidebar_html(&terms());
    assert!(html.contains("data-agents-sidebar"));
    assert!(html.contains("data-presence=\"blocked\""));
    assert!(html.contains("data-presence=\"working\""));
    assert!(html.contains("data-presence=\"done\""));
    // Blocked stands out: flagged row + peek target.
    assert!(html.contains("data-blocked=\"true\""));
    assert!(html.contains("data-peek=\"m/blocked\""));
}

#[test]
fn agents_sidebar_clears_seen_done() {
    let mut t = terms();
    t[2].mark_done_seen();
    let html = desktop_agents_sidebar_html(&t);
    // Seen-Done clears: no done badge remains.
    assert!(!html.contains("data-presence=\"done\""));
}

#[test]
fn agents_sidebar_never_infers_presence() {
    let unknown = vec![AgentTerminal::new("m/x", "x")];
    let html = desktop_agents_sidebar_html(&unknown);
    assert!(html.contains("data-presence=\"unknown\""));
    assert!(!html.contains("data-blocked=\"true\""));
}

#[test]
fn left_rail_scopes_client_on_project_select() {
    let html = desktop_left_rail_html(&["a".to_string()], Some("a"), &terms());
    assert!(html.contains("data-left-rail"));
    assert!(html.contains("data-project-switcher"));
    assert!(html.contains("data-agents-sidebar"));
    assert!(html.contains("data-active-project=\"a\""));
}

#[test]
fn peek_renders_live_terminal_or_calm_note() {
    let live = desktop_peek_html("m/blocked", Some("output text"));
    assert!(live.contains("data-peek-pane=\"m/blocked\""));
    assert!(live.contains("output text"));
    let missing = desktop_peek_html("m/gone", None);
    assert!(missing.contains("data-peek-pane=\"m/gone\""));
    assert!(missing.contains("data-peek-empty"));
}

#[test]
fn left_rail_resync_keeps_seen_done_cleared() {
    // Reopen path: snapshot + replay via agents_resync, then render.
    let list = r#"[{"terminal":"m/done","name":"done-job"}]"#;
    let mut screens = HashMap::new();
    screens.insert(
        "m/done".to_string(),
        "\x1b]7501;status=done\x07".to_string(),
    );
    let mut seen = HashSet::new();
    seen.insert("m/done".to_string());
    let resynced = factory_lib::agents_resync(list, &screens, &seen);
    let html = desktop_agents_sidebar_html(&resynced);
    assert!(!html.contains("data-presence=\"done\""));
}
