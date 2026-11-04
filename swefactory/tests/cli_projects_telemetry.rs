//! Ticket #140 (spec #135): project switcher + installers + updates + telemetry.
use factory_lib::{
    background_update_policy, client_auth_reuse, hc_core_usability, list_factory_projects,
    telemetry_enabled,
};
use std::io::Write;

fn seed_repo(dir: &std::path::Path, name: &str, factory: bool) -> std::path::PathBuf {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    if factory {
        for f in ["AGENTS.md", "GLOSSARY.md", "scripts/factory-check"] {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x\n").unwrap();
        }
    } else {
        std::fs::write(d.join("README.md"), "plain\n").unwrap();
    }
    d
}

#[test]
fn switcher_lists_factory_repos() {
    let tmp = std::env::temp_dir().join(format!("t140-proj-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    seed_repo(&tmp, "alpha", true);
    seed_repo(&tmp, "beta", false);
    seed_repo(&tmp, "gamma", true);
    let got = list_factory_projects(&tmp);
    assert_eq!(got, vec!["alpha".to_string(), "gamma".to_string()]);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn hc_core_three_state() {
    // Missing binary -> missing.
    std::env::set_var("FACTORY_HC_BIN", "/nonexistent-hc-bin-xyz");
    assert_eq!(hc_core_usability(), "missing");
    // Silent-but-present file -> guided-upgrade.
    let tmp = std::env::temp_dir().join(format!("t140-hc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let fake = tmp.join("hc");
    #[cfg(unix)]
    {
        std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&fake, "x").unwrap();
    }
    std::env::set_var("FACTORY_HC_BIN", &fake);
    assert_eq!(hc_core_usability(), "guided-upgrade");
    std::env::remove_var("FACTORY_HC_BIN");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn background_update_contract() {
    let p = background_update_policy();
    assert_eq!(p["download"], "background");
    assert_eq!(p["takeover"], "on-restart");
    assert_eq!(p["terminals_survive"], true);
    assert_eq!(p["terminal_owner"], "hc-core");
}

#[test]
fn telemetry_default_on_optout_off() {
    let dir = std::env::temp_dir().join(format!("t140-tel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("FACTORY_STATE_DIR", &dir);
    std::env::remove_var("FACTORY_TELEMETRY");
    assert!(telemetry_enabled());
    std::env::set_var("FACTORY_TELEMETRY", "0");
    assert!(!telemetry_enabled());
    std::env::remove_var("FACTORY_TELEMETRY");
    // Opt-out file disables.
    std::fs::write(dir.join("telemetry-opt-out"), "opt-out\n").unwrap();
    assert!(!telemetry_enabled());
    std::env::remove_var("FACTORY_STATE_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn telemetry_cli_opt_out_in() {
    let dir = std::env::temp_dir().join(format!("t140-telcli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_factory");
    let run = |args: &[&str]| {
        std::process::Command::new(bin)
            .args(args)
            .env("FACTORY_STATE_DIR", &dir)
            .output()
            .unwrap()
    };
    let out = run(&["telemetry", "--json"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"enabled\":true"));
    let out = run(&["telemetry", "--opt-out", "--json"]);
    assert!(out.status.success());
    assert!(dir.join("telemetry-opt-out").is_file());
    let out = run(&["telemetry", "--opt-in", "--json"]);
    assert!(out.status.success());
    assert!(!dir.join("telemetry-opt-out").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn projects_cli_json() {
    let tmp = std::env::temp_dir().join(format!("t140-projcli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    seed_repo(&tmp, "one", true);
    let bin = env!("CARGO_BIN_EXE_factory");
    let out = std::process::Command::new(bin)
        .args(["projects", "--root", tmp.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["projects"], serde_json::json!(["one"]));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn client_auth_reuse_contract() {
    let a = client_auth_reuse();
    assert_eq!(a["credential_store"], "none");
    assert_eq!(a["gh_auth_token"], "reuse");
    let _ = std::io::stdout().write_all(&[]);
}
