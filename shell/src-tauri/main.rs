// Ticket #150: Tauri backend sketch — the backend IS the shell.
// Each command calls factory_lib directly in-process (no daemon,
// no new API layer). Reference for the beta installer milestone;
// the contract is pinned by desktop_shell_boot() + ui_shell_left_rail tests.
//
// use factory_lib::{agents_resync, desktop_board_feed, list_factory_projects, peek_read};
// use std::collections::{HashMap, HashSet};
//
// #[tauri::command]
// fn shell_boot() -> serde_json::Value { factory_lib::desktop_shell_boot() }
//
// #[tauri::command]
// fn list_projects(root: String) -> Vec<String> {
//     factory_lib::list_factory_projects(std::path::Path::new(&root))
// }
//
// #[tauri::command]
// fn agents_snapshot(
//     list_json: String,
//     screens: HashMap<String, String>,
//     seen_done: HashSet<String>,
// ) -> Vec<factory_lib::AgentTerminal> {
//     factory_lib::agents_resync(&list_json, &screens, &seen_done)
// }
//
// #[tauri::command]
// fn peek_read(addr: String) -> Option<String> { factory_lib::peek_read(&addr) }
//
// #[tauri::command]
// fn window_close(terms: &mut [factory_lib::AgentTerminal]) {
//     // Detach peeks only — terminals keep running in the hc core.
//     factory_lib::detach_all_peeks(terms);
// }
//
// Ticket #152 (spec #149): packaging track. Installers ship or bootstrap
// the hc core (externalBin `bin/hc`: linux-x86_64, linux-aarch64,
// macos-aarch64; unusable core -> guided upgrade per
// `desktop_hc_core_policy`). Background updates download and take over on
// restart (`plugins.updater` in tauri.conf.json; terminals survive in the
// core per `desktop_update_policy`). Telemetry on by default in releases
// with Settings Privacy opt-out (`desktop_release_telemetry_default` +
// `desktop_settings_privacy_html`). Auth reuses gh token + SSH config;
// no credential store (`desktop_auth_policy`).
//
// fn main() {
//     tauri::Builder::default()
//         .invoke_handler(tauri::generate_handler![
//             shell_boot,
//             list_projects,
//             agents_snapshot,
//             peek_read
//         ])
//         .run(tauri::generate_context!())
//         .expect("factory shell failed to boot");
// }
