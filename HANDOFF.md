# HANDOFF — Ticket #153: ticket detail + Run buttons (spec #149)

- Ticket: #153 (parent spec #149). Base `dev fddeefa`, head `479d6e2` on `factory/T153-ticket-detail`.
- Freeze: `factory sync 149` → `4a00ee8a226bafe525d3f8621a97a73c55fb1109b70018a4b2134b3d22d89129` = SPEC-APPROVED hash. OK.
- Deps #150 + #151 merged in dev (present in base). No new deps.

## Changes
- `swefactory/src/lib.rs`: new spec-#149 section — `DESKTOP_DETAIL_SPEC_ISSUE=149`,
  `ticket_detail_freeze_hash()`, `ticket_detail_freeze_missing_sentence()`,
  `desktop_detail_runs_html()` (freeze + acceptance criteria + checks output +
  `data-run` buttons + AgentRunner/session identity/verdict tag note),
  `desktop_detail_runs_json()`; `desktop_shell_boot()` now advertises
  `ticket_detail`/`run_sync`/`run_check`/`run_handoff`. Existing #138 fns untouched
  (run_outcome mapping + ticket_run_sync/check/handoff reused as-is).
- `swefactory/tests/ui_ticket_detail_153.rs`: 6 tests (spec=149, fragment markers,
  missing-freeze names 149, JSON join, boot contract, helpers + sync-fail calm cause).
- `shell/src-tauri/main.rs`: Ticket #153 sketch commands
  (ticket_detail/run_sync/run_check/run_handoff, in-process factory_lib calls).
- `shell/index.html`: detail pane (`data-detail-runs` + run buttons + outcome line)
  wired to `ticket_detail`/`run_sync`/`run_check`/`run_handoff` with inline pass/fail + cause.

## Checks (actual)
- `cargo test --test ui_ticket_detail_153 --test ui_ticket_runs --test ui_desktop_board --test ui_center_board`: 6+6+6+6 passed.
- `./scripts/factory-check` → `FACTORY-CHECK PASSED` (full suite + doc-tests).
- `cargo fmt --check` clean.

## Acceptance criteria
- [x] Detail shows freeze hash, acceptance criteria, checks output for the selected card
  (`desktop_detail_runs_html` markers; test `desktop_fragment_carries_freeze_criteria_checks_buttons`).
- [x] Run buttons execute sync, check, handoff with inline pass/fail + named cause
  (`ticket_run_*` via `run_outcome`; UI outcome line; boot advertises commands).
- [x] Freeze hash comes from the watched factory cache, joined with tracker + hc state
  (`ticket_detail_freeze_hash` reads `factory/.cache/SPEC-149.md` via `read_cache`; checks via `ticket_checks_output` hc join).
- [x] UI copy uses glossary terms exactly (AgentRunner, session identity, verdict tag — asserted in test).
- [x] scripts/factory-check passes (actual output above).

## Risks / open questions
- `shell/src-tauri` has no Cargo.toml (consistent with #150–#152): Tauri commands are
  sketch comments pinned by the `desktop_shell_boot` contract test, not compiled Rust.
- `ticket_detail(number, tail)` hits the tracker (`gh`); unreachable tracker → detail None → UI keeps card only. Same read-only posture as #151.
- Not merged, nothing posted to GitHub. Pushed WIP to `factory/T153-ticket-detail` (allowed).
