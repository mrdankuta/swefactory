# Handoff — Ticket #155: review auto-spawn + verdict routing (spec #149)

- Ticket: #155 (base dev 5b848f9, branch factory/T155-review-routing, head d803fe9)
- Spec freeze: `factory sync 149` → hash 4a00ee8a226bafe525d3f8621a97a73c55fb1109b70018a4b2134b3d22d89129 — MATCHES SPEC-APPROVED, proceeded.
- Deps #150 + #153 present in dev (board feed, detail+Run seams landed).

## Changes
- `swefactory/src/lib.rs`: `desktop_shell_boot` commands map gains `move_to_review`, `review_detail`, `review_accept`, `review_request_changes` (thin contract over existing #141 seam: `review_spawn_argv`, `desktop_review_html`, `review_route`/`review_route_target`, `parse_verdict_tag` reused last-match-wins).
- `shell/src-tauri/main.rs`: Ticket #155 Tauri command sketch block (`move_to_review`, `review_detail`, `review_accept`, `review_request_changes`) calling `factory_lib` in-process, same commented-sketch style as #150/#153.
- `swefactory/tests/ui_review_routing_shell_155.rs` (new, 4 tests): boot advertises review commands; move-to-Review spawns kept `--bg --keep` terminal `t155-review-a2` joining the sidebar; detail HTML shows diff summary + verdict tag payload + findings + Accept/Request-changes + peek with glossary-exact terms (AgentRunner, session identity, verdict tag); accept→accepted/land, none/request-changes→To Do, peek attach.

## Acceptance criteria evidence
- Kept hc review terminal in sidebar: `move_to_review_spawns_kept_sidebar_terminal` ✓
- Detail diff + verdict payload + findings inline: `review_detail_html_shows_diff_verdict_findings_and_peek` ✓ (last-match-wins asserted via `parse_verdict_tag`)
- Accept→accepted/land, Request-changes→To Do: `accept_and_request_changes_routing` ✓
- Schema-validated last-match-wins: reused `parse_verdict_tag` (existing `ui_review_routing` 6 tests still pass) ✓
- `scripts/factory-check` passes: see below ✓

## Checks (actual output)
- `cargo test --test ui_review_routing_shell_155`: 4 passed
- `cargo test --test ui_review_routing`: 6 passed
- `./scripts/factory-check` → `FACTORY-CHECK PASSED` (after `cargo fmt` fixed a formatting nit in the new test)

## Risks / open questions
- Tauri commands are sketch (commented, like #150/#153 blocks) — live `hc run` spawning and tracker lane moves happen at integration, not here.
- Review accept path reads the tag only; explicit Request-changes button always routes to To Do regardless of tag — matches spec.

Not merged, nothing posted to GitHub. WIP committed on factory/T155-review-routing (pushing allowed but not done from here).
