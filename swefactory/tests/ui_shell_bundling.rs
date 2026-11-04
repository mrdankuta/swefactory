//! Ticket #152 (spec #149): hc bundling + installers, background updates,
//! telemetry default, auth reuse. Pure policy seams the installer and the
//! Tauri updater sketch read; behaviour lives in the core/updater.

use factory_lib::{
    desktop_auth_policy, desktop_hc_core_policy, desktop_release_telemetry_default,
    desktop_update_policy,
};

#[test]
fn hc_core_policy_bundles_supported_targets_and_guides_upgrade() {
    let p = desktop_hc_core_policy();
    let targets = p["bundled_targets"].as_array().unwrap();
    let names: Vec<&str> = targets.iter().map(|t| t.as_str().unwrap()).collect();
    assert!(names.contains(&"linux-x86_64"));
    assert!(names.contains(&"linux-aarch64"));
    assert!(names.contains(&"macos-aarch64"));
    assert_eq!(p["unusable_core"], "guided-upgrade");
    assert_eq!(p["terminal_owner"], "hc-core");
}

#[test]
fn update_policy_background_takeover_terminals_survive() {
    let p = desktop_update_policy();
    assert_eq!(p["download"], "background");
    assert_eq!(p["takeover"], "on-restart");
    assert_eq!(p["terminals_survive"], true);
    assert_eq!(p["terminal_owner"], "hc-core");
}

#[test]
fn release_telemetry_on_by_default_privacy_opt_out() {
    assert!(desktop_release_telemetry_default());
    assert!(factory_lib::telemetry_enabled());
    let html = factory_lib::desktop_settings_privacy_html(true);
    assert!(html.contains("data-telemetry-toggle"));
}

#[test]
fn auth_reuses_gh_and_ssh_no_credential_store() {
    let p = desktop_auth_policy();
    assert_eq!(p["gh_token"], "reuse");
    assert_eq!(p["ssh_config"], "reuse");
    assert_eq!(p["credential_store"], "none");
}
