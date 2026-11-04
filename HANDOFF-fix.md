# Handoff — #151 fix pass (changes_requested)

Branch: factory/T151-ticket-center-board-over-deskt
Base: e84db215f8cdfe3bbb964e40a4ec3d963d160e45 · Head: 03ee215

## Per finding
1. click-to-select `{once:true}` → shell/index.html: listener moved outside `refreshBoard()`, registered a single time, no `once` option; `refreshBoard` only refreshes feed data.
2. repeated-click test → `repeated_card_selection_keeps_scoping_detail`: selects 7→8→7, each asserts `data-selected-card="<n>"` + `data-scopes="detail"`; plus asserts shell has `addEventListener` and no `once:true`/`once: true`.
3. dedup render → `desktop_board_html` is now `desktop_board_html_inner(feed, poll_ms, None)` wrapper; `desktop_board_html_with_selection` resolves `picked` then calls same inner core (single copy of markup).
4. harden `polling_is_read_only` → new `fn_body()` helper slices full function bodies (marker → next `\npub fn `), replacing fixed 600/900/500-char windows; also asserts feed body has no INSERT/UPDATE.

## Checks (actual)
- `cargo test --manifest-path swefactory/Cargo.toml --test ui_center_board` → 6 passed, 0 failed.
- `scripts/factory-check` → FACTORY-CHECK PASSED, EXIT:0 (after `cargo fmt`; first run EXIT:1 on fmt diffs only, fixed and recommitted).
- Committed WIP 07294f5 + 03ee215, pushed to origin. No merge, no GitHub posts.
