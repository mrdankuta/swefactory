//! Ticket #140 remainder: switcher selection scopes board+agents+detail,
//! Settings Privacy telemetry toggle.
use factory_lib::{desktop_project_switcher_html, desktop_settings_privacy_html};

#[test]
fn switcher_html_scopes_selection() {
    let html = desktop_project_switcher_html(&["a".to_string(), "b".to_string()], Some("b"));
    assert!(html.contains("data-project-switcher"));
    assert!(html.contains("data-project=\"b\""));
    assert!(html.contains("data-selected=\"true\""));
    // Selection scopes the three panes.
    assert!(html.contains("data-scopes=\"board agents detail\""));
    assert!(html.contains("data-active-project=\"b\""));
}

#[test]
fn switcher_html_no_selection() {
    let html = desktop_project_switcher_html(&["a".to_string()], None);
    assert!(html.contains("data-active-project=\"\""));
    assert!(!html.contains("data-selected=\"true\""));
}

#[test]
fn privacy_html_reflects_telemetry_state() {
    let html_on = desktop_settings_privacy_html(true);
    assert!(html_on.contains("data-settings-privacy"));
    assert!(html_on.contains("checked"));
    let html_off = desktop_settings_privacy_html(false);
    assert!(html_off.contains("data-settings-privacy"));
    assert!(!html_off.contains("checked"));
}
