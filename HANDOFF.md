# HANDOFF — Ticket #141: review auto-spawn + verdict routing

- Ticket: #141 (`factory/T141-review-routing`, base `dev` @ `44c2e27`)
- Head: `169ff0a` (commits `b35daa0`, `7e61df0`, `169ff0a`)
- Spec freeze: `scripts/factory sync 135` →
  `cached: factory/.cache/SPEC-135.md (hash=6adcc389e273220b45f8a238a9a19e8dd805)`
  equals SPEC-APPROVED `6adcc389…` — verified before changes.
- Deps #136 + #138: present in dev (merges `9916f18` T136, `6580e66` T138
  in `integration/20261104-train` → dev `44c2e27`).

## Changes
- `swefactory/src/lib.rs`: new section "review auto-spawn + verdict routing":
  `review_terminal_name`, `review_spawn_argv` (`--bg --keep --json --name`,
  pure argv behind `FACTORY_HC_BIN` seam), `review_sidebar_entry`
  (`AgentTerminal` row, peek via existing `peek_attach`), `ReviewRoute` +
  `review_route` (only `accept` → Accept; missing/invalid → RequestChanges)
  + `review_route_target` (`accepted/land` / `To Do`), `review_diff_summary`,
  `review_findings` (strips `<verdict>` tags), `desktop_review_html`
  (diff summary + verdict tag payload + findings + Accept/Request-changes
  buttons + peekable review terminal; glossary terms AgentRunner,
  session identity, verdict tag exact). Verdict parsing reuses
  `parse_verdict_tag` (#105: schema-validated, last-match-wins).
- `swefactory/tests/ui_review_routing.rs`: 6 integration tests pinning the
  four acceptance criteria + calm edges.

## Checks (actual output)
- `(cd swefactory && cargo test --test ui_review_routing)`: `6 passed; 0 failed`.
- `scripts/factory-check`: `FACTORY-CHECK PASSED` (fmt, clippy deny-warnings,
  full test suite incl. doctests).

## Acceptance evidence
- [x] Move-to-Review spawns kept hc review terminal in sidebar
  (`move_to_review_spawns_kept_terminal`, `review_terminal_is_visible_in_sidebar_and_peekable`)
- [x] Detail shows diff summary + verdict tag payload + findings
  (`detail_shows_diff_summary_verdict_payload_and_findings`)
- [x] Accept → accepted/land; Request-changes → To Do
  (`accept_routes_to_land_request_changes_to_todo`)
- [x] Schema-validated last-match-wins
  (`verdict_extraction_validates_schema_last_match_wins`)

## Risks / open questions
- Spawn argv is pure (no live `hc` assertion); actual `hc run --keep`
  flag support assumed from #136 seam.
- No tracker label mutation wired (Accept/Request-changes buttons render;
  routing target is a pure map) — landing/To-Do moves stay with existing
  command paths.
- No merge, no GitHub posts per instructions.
