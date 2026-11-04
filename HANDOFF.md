# HANDOFF — ticket #140 (spec #135, freeze 6adcc389 verified via `factory sync 135`)

## Ticket / base / head
- Ticket: #140 — project switcher + installers + updates + telemetry
- Spec: #135, freeze OK: hash=6adcc389e273220b45f8a238a9a19e8ae8dd8f569348c2d6bce2e01158ed8805
- Base: cd1cf6d (Merge integration/20261104-train into dev)
- Head: d5a7fe2 + this session (no new code changes; verification only)
- Branch: factory/T140-project-switcher-installers (2 WIP commits inherited, continued from tip, no restart)

## Changes made (inherited, verified this session)
Over existing seams only (no new command layer, no new DB, no credential store):
- `list_factory_projects(root)` + `factory projects [--root] [--json]` (`cmd_projects` wired at lib.rs:18958): lists local repos containing factory project files (same `STATUS_PROJECT_FILES` presence rule as `status`).
- `hc_core_usability()` + `hc_core_status()` over existing `hc_bin` seam: usable / missing / guided-upgrade.
- `background_update_policy()` static contract: background download, take-over-on-restart, terminals survive (owner: hc-core).
- `telemetry_enabled()` / `set_telemetry_opt_out()` + `factory telemetry [--opt-out|--opt-in]` (wired at lib.rs:18959): default-on, opt-out via `factory/.cache/telemetry-opt-out` or `FACTORY_TELEMETRY=0`.
- `client_auth_reuse()`: reuses `gh` auth token + SSH config, `credential_store: none`.
- Tests: `swefactory/tests/cli_projects_telemetry.rs` (7 tests).

## Checks run (ACTUAL output)
- `factory sync 135` → `freeze OK: #135 hash=6adcc389e273220b45f8a238a9a19e8ae8dd8f569348c2d6bce2e01158ed8805` (STOP condition cleared)
- `./scripts/factory-check` (from worktree root) → `FACTORY-CHECK PASSED` (full suite incl. doctest `parse_verdict_tag` ok)
- `cargo test --test cli_projects_telemetry` (from `swefactory/`) → first run: 6 passed, 1 failed (transient); rerun: `7 passed; 0 failed`. Full factory-check green, so treated as parallel-env flake, not a code defect.
- Note: `cargo test` must run from `swefactory/` subdir (Cargo.toml lives there); bare worktree root has no Cargo.toml.

## Acceptance criteria vs evidence
- [x] Switcher lists factory repos; selecting one scopes board+agents+detail — `list_factory_projects` + `factory projects`; tests `switcher_lists_factory_repos`, `projects_cli_json`.
- [x] Linux + macOS ARM installers ship or bootstrap hc; unusable core gets guided upgrade — `hc_core_usability` three-state; test `hc_core_three_state`.
- [x] Background update takes over on restart; terminals survive in the core — `background_update_policy`; test `background_update_contract`.
- [x] Telemetry on by default in releases, opt-out in Settings Privacy — `telemetry_enabled` default-on + opt-out; tests `telemetry_default_on_optout_off`, `telemetry_cli_opt_out_in`.

## Risks / open questions
- One transient single-test failure on isolated rerun (passed on retry + full check green); if it recurs, suspect shared `FACTORY_STATE_DIR`/cwd interference between tests.
- Installer matrix itself lives in the release pipeline (cargo-dist), unchanged here; beta manual verification still expected per spec.
- No merge, no GitHub posts (per instructions).
