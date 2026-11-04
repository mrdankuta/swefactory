# Handoff — Ticket #138: ticket detail + Run buttons

- Ticket: #138 (spec #135, freeze `6adcc389` verified via `factory sync 135` → `freeze OK`)
- Branch: `factory/T138-ticket-detail`, base `f3c81ff` (dev, includes #136+#137)
- Head: `git log -1` at handoff time for the exact commit

## What changed

` swefactory/src/lib.rs` — new `ticket #138` section reusing landed seams only:

- `ticket_freeze_hash()` — freeze hash from the watched factory cache
  (`read_cache(135)` over `factory/.cache/SPEC-135.md`); `None` when absent.
  `cache_path` gained a `FACTORY_CACHE_DIR` env override (default unchanged)
  so the join is hermetic in tests.
- `ticket_acceptance_criteria(body)` — `- [ ]` checklist under
  `## Acceptance criteria`.
- `ticket_checks_output(detail)` — newest attempt summary + exit code joined
  with the `hc` terminal from the existing `board_attempt_terminals()` seam;
  calm no-attempts sentence otherwise.
- Run buttons invoke the existing command functions: `ticket_run_sync` →
  `cmd_sync`, `ticket_run_check` → the same adapter script `factory check`
  discovers (`$FACTORY_CHECK_ADAPTER` / `discover_adapter`; `run_check`
  diverges via process exit so the backend spawns the adapter instead —
  same binary, same semantics), `ticket_run_handoff` → `cmd_handoff`.
  `run_outcome()` is the pure exit→pass/fail+cause mapping.
- `ticket_detail_runs_fragment_html` / `ticket_detail_runs_json` — existing
  detail fragment/JSON joined with `data-freeze-hash`,
  `data-acceptance-criteria`/`data-criterion`, `data-checks-output`, and
  `data-run="sync|check|handoff"` buttons. Existing `ticket_detail_*`
  functions untouched (no snapshot churn).
- UI copy uses glossary terms exactly: AgentRunner, session identity,
  verdict tag (`data-runner-note`).

`swefactory/tests/ui_ticket_runs.rs` — 6 tests: outcome mapping (pass/fail +
named cause), criteria parsing, checks fallback, fragment markers + glossary
terms, missing-freeze calm path, JSON join.

## Acceptance criteria → evidence

- [x] Detail shows freeze hash, acceptance criteria, checks output for the
      selected card → `runs_fragment_carries_freeze_criteria_checks_buttons`,
      `runs_json_joins_freeze_criteria_checks`
- [x] Run buttons execute sync, check, handoff with inline pass/fail + named
      cause → `ticket_run_sync/check/handoff` call `cmd_sync`/adapter/
      `cmd_handoff`; `run_outcome_names_pass_fail_with_cause`
- [x] Freeze hash comes from the watched factory cache, joined with tracker
      + hc state → `ticket_freeze_hash` via `read_cache`; checks output joins
      `board_attempt_terminals` (hc seam)
- [x] UI copy uses glossary terms exactly → asserted in fragment test

## Checks (actual output)

- `cargo test --offline --test ui_ticket_runs` → 6 passed
- `cargo test` (ui_ticket_detail) → ok
- `cargo fmt --check` → clean; `cargo clippy --all-targets` → 0 warnings
- `./scripts/factory-check` (from worktree root) → `FACTORY-CHECK PASSED`
  (full suite incl. doc-tests)

## Risks / open questions

- `run_check` returns `!`; Run-check spawns the adapter as a subprocess
  rather than calling it — same script, but a second code path worth a look
  in review.
- `ticket_run_sync/handoff` call the real `cmd_*` (stdout side effects);
  only the mapping is unit-tested. Live behavior needs the UI harness.
- Check/checks-output for "latest checks output" uses the attempt ledger +
  hc terminal map; if the intent was `scripts/factory-check` output
  verbatim, that is a follow-up, not this ticket's join.
- No GitHub posts, no merge performed.

FACTORY:READY_FOR_REVIEW
