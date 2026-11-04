//! Ticket #152 (spec #149): hc bundling + installers, background updates,
//! telemetry default, auth reuse. Pure policy seams the installer and the
//! Tauri updater sketch read; behaviour lives in the core/updater.
//!
//! Scope note (review fix, spec #149): this ticket delivers the policy
//! contract + `shell/tauri.conf.json` config only. Actual installer-matrix
//! builds (deb/appimage/dmg/nsis shipping/bootstrapping `bin/hc`) and
//! updater take-over-on-restart are release-pipeline / manual-beta concerns
//! per spec #149, NOT unit-proven here. Follow-up: installer-matrix +
//! take-over-on-restart wiring tracked as release-pipeline/manual-beta work.

use factory_lib::{
    desktop_auth_policy, desktop_hc_core_policy, desktop_release_telemetry_default,
    desktop_update_policy,
};
use std::path::PathBuf;

fn tauri_conf() -> serde_json::Value {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "shell", "tauri.conf.json"]
        .iter()
        .collect();
    let text = std::fs::read_to_string(&path).expect("read shell/tauri.conf.json");
    serde_json::from_str(&text).expect("parse shell/tauri.conf.json")
}

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
    // Non-tautological pin: the real artifact must actually ship/bootstrap hc.
    let conf = tauri_conf();
    let bins: Vec<&str> = conf["bundle"]["externalBin"]
        .as_array()
        .expect("bundle.externalBin")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        bins.iter().any(|b| b.contains("bin/hc")),
        "externalBin ships bin/hc"
    );
    let targets: Vec<&str> = conf["bundle"]["targets"]
        .as_array()
        .expect("bundle.targets")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for t in ["deb", "appimage", "dmg"] {
        assert!(targets.contains(&t), "bundle.targets contains {t}");
    }
}

#[test]
fn update_policy_background_takeover_terminals_survive() {
    let p = desktop_update_policy();
    assert_eq!(p["download"], "background");
    assert_eq!(p["takeover"], "on-restart");
    assert_eq!(p["terminals_survive"], true);
    assert_eq!(p["terminal_owner"], "hc-core");
    // Non-tautological pin: the real artifact must enable background/install-on-restart.
    let conf = tauri_conf();
    assert_eq!(conf["plugins"]["updater"]["downloadInBackground"], true);
    assert_eq!(conf["plugins"]["updater"]["installOnRestart"], true);
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
