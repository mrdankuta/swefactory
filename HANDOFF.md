# Handoff — Ticket #137: board feed + columns with stale handling

- Ticket: #137 (parent spec #135, freeze verified: `factory sync 135` →
  `freeze OK: #135 hash=6adcc389...`, matches SPEC-APPROVED 6adcc389)
- Base: cd1cf6d (dev) · Branch: factory/T137-board-feed · Head: 9fc7eb6
- Changes: applied /tmp/t137.patch (+184-line DesktopBoard feed slice in
  swefactory/src/lib.rs); added swefactory/tests/ui_desktop_board.rs (6 tests);
  cargo fmt + 2 clippy-lint fixes.
- Acceptance criteria evidence (all in ui_desktop_board.rs, green):
  - Columns derive from labels via existing mapping: `three_columns_todo_review_done`,
    `labels_map_via_existing_mapping` (review→Review, accepted→Done, else To Do).
  - Cards carry number+title+marker; poll 30–60s clamped: `cards_carry_number_title_marker`,
    `poll_bounded_30_to_60s` (default 45000, clamp 30000/60000).
  - Stale: `stale_feed_re_serves_cache_with_banner` (data-stale-banner + cached card).
  - Read-only: `fetch_is_read_only_issue_list` (gh issue list only, no write verbs).
- Checks run (actual output): `scripts/factory-check` → `FACTORY-CHECK PASSED`
  (incl. new suite: 6 passed; doc-test 1 passed). `cargo test --test ui_desktop_board` → 6 passed.
- Risks/open: no endpoint wiring yet (pure feed/html/json functions only; daemon
  polling loop + route to follow); DESKTOP columns intentionally 3 (not the 6 web columns).
- No GitHub posts, no merge performed.
