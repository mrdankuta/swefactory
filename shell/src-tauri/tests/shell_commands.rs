//! Shell command tests (ticket #156): the Tauri handlers are thin
//! wrappers over `factory_lib` seams, so tests pin the wiring —
//! boot contract, project listing, live agents snapshot over a fake
//! `hc`, peek detach — never the webview.

use std::io::Write;

fn write_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

#[test]
fn shell_boot_advertises_backend() {
    // NOTE: shell_boot lives in the binary target; the contract it
    // serves is pinned here through the lib seam it calls.
    let boot = factory_lib::desktop_shell_boot();
    assert_eq!(boot["backend"], serde_json::json!("factory_lib-in-process"));
}

#[test]
fn list_projects_finds_factory_repos() {
    let root = std::env::temp_dir().join(format!(
        "factory-shell-projects-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let repo = root.join("demo");
    std::fs::create_dir_all(&repo).unwrap();
    for file in ["AGENTS.md", "GLOSSARY.md", "scripts/factory-check"] {
        let path = repo.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, "x").unwrap();
    }
    let found = factory_lib::list_factory_projects(&root);
    assert!(
        found.iter().any(|p| p.ends_with("demo")),
        "switcher lists the factory repo, got: {found:?}"
    );
    let empty = std::env::temp_dir().join(format!(
        "factory-shell-empty-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&empty).unwrap();
    assert!(
        factory_lib::list_factory_projects(&empty).is_empty(),
        "non-project dirs are not listed"
    );
}

#[test]
fn agents_snapshot_reads_fake_hc_with_status() {
    let dir = std::env::temp_dir().join(format!(
        "factory-shell-hc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // Fake `hc`: `ls` prints one terminal, `read` replays a screen
    // carrying an OSC 7501 Working report.
    let hc = write_script(
        &dir,
        "hc",
        "#!/bin/sh\nif [ \"$1\" = \"ls\" ]; then echo '{\"terminals\":[{\"address\":\"t9\",\"agent\":\"pi\"}]}'; else printf '\\033]7501;status=working\\a'; fi\n",
    );
    unsafe {
        std::env::set_var("FACTORY_HC_BIN", &hc);
    }
    let list = factory_lib::hc_terminal_list_json().expect("fake hc answers ls");
    assert!(list.contains("t9"), "list seam reads the core, got: {list}");
    let screen = factory_lib::peek_read("t9").expect("fake hc replays a screen");
    assert_eq!(
        factory_lib::parse_osc7501_status(&screen),
        Some(factory_lib::AgentPresence::Working),
        "presence comes from the protocol only"
    );
    unsafe {
        std::env::remove_var("FACTORY_HC_BIN");
    }
}
