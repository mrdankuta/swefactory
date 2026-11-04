# Shell skeleton (ticket #150, spec #149)

The Tauri desktop shell boots with its Rust backend calling
`factory_lib` **in-process** — the same call pattern the web-board
handlers already use. No separate daemon process, no new API layer.
`hc` (herdr-core) owns terminal liveness; the app never owns PTYs.

## Tauri command → `factory_lib` seam map

The contract is pinned by `desktop_shell_boot()` in `factory_lib`
(`swefactory/src/lib.rs`; tests in `swefactory/tests/ui_shell_left_rail.rs`).
Each Tauri command below is a thin in-process call into the named seam:

| Tauri command     | `factory_lib` seam              | Notes                                    |
|-------------------|---------------------------------|------------------------------------------|
| `list_projects`   | `list_factory_projects(root)`   | Left-rail switcher; selection scopes client |
| `agents_snapshot` | `agents_resync(list, screens, seen)` | OSC 7501 presence only, never inferred |
| `peek_read`       | `peek_read(addr)`               | `hc read` stream; calm note on failure   |
| `board_feed`      | `desktop_board_feed(last)`      | Center board poll every 45s (30-60s bound); stale banner on unreachable tracker (ticket #151) |
| `board_select`    | `desktop_board_select(feed, n)`   | Card selection scopes detail pane; pure over feed, no writes (ticket #151) |

Left-rail rendering is pure HTML over seam values:
`desktop_left_rail_html` = `desktop_project_switcher_html` +
`desktop_agents_sidebar_html`; peek pane = `desktop_peek_html`.

## Window lifecycle

Close detaches peek sessions only (`detach_all_peeks`); terminals keep
running in the core. Reopen resyncs via snapshot (list) + replay
(presence re-read per screen). Pinned by `desktop_window_close_policy()`.

## Layout

- `tauri.conf.json` — window + allowlist (no shell-outside-`hc` scope).
- `index.html` — left rail mount (`data-left-rail`), peek pane mount.
- `src-tauri/main.rs` — backend sketch: `tauri::Builder` invoking the
  seams above in-process (reference; the crate builds once Tauri deps
  are vendored for the beta installer milestone).
