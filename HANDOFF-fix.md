# HANDOFF-fix — Ticket #152 changes_requested fix pass

- Branch: factory/T152-ticket-hc-bundling-installers; base e84db21, head 56ed22a.
- Fix 1 (weak-test): `swefactory/tests/ui_shell_bundling.rs` — bundling test now parses real `shell/tauri.conf.json` (externalBin contains bin/hc; bundle.targets contains deb/appimage/dmg); update test asserts `plugins.updater.downloadInBackground` + `installOnRestart` true. Policy-fn assertions kept as contract pins.
- Fix 2 (spec-drift): scope narrowed in-file (test header comment): this ticket = policy contract + config only; installer matrix (deb/appimage/dmg/nsis shipping/bootstrapping bin/hc) + take-over-on-restart are release-pipeline/manual-beta per spec #149, not unit-proven. Follow-up = installer/updater wiring tracked as release-pipeline/manual-beta work. No GitHub post (per instructions).
- Checks (actual): `cargo test --manifest-path swefactory/Cargo.toml --test ui_shell_bundling` → 4 passed; `scripts/factory-check` → FACTORY-CHECK PASSED (after cargo fmt).
- Commit: 56ed22a on this branch. Not pushed (push optional; no network op done). Not merged, nothing posted to GitHub.
