# Research: terminal core for desktop client (map #130, ticket #131)

Sources: herdr-core README + schema/API docs (primary), `docs/adr/0001-persistent-terminal-sidecar.md` (#129 sidecar decision), Tauri v2 sidecar docs + tauri-plugin-pty (lib.rs/Context7).

## Answer: reuse `hc` as sidecar, do not own PTYs

- herdr-core (`hc`) already provides exactly the Herdr loop we want: persistent terminals that survive viewer close, `run --bg --json`, `wait` (180s check-in default), `get`/`read` replay, `--keep` recovery with fresh shell, Program Status Protocol (OSC 7501) with Working/Blocked/Done stored server-side, children (`wait --children all`, `send`), machines/SSH with `machine ls`, and a machine-addressed list (`box/t3`).
- `hc schema` exposes the request/response API as OpenRPC+JSON Schema; `hc connect` carries it over stdin/stdout incl. SSH. The API docs cover connecting, terminal streams, events, status — i.e. a build-your-own-client contract already exists.
- Tauri natively supports this shape: `shell().sidecar()` / `Command.sidecar()` with capability-gated execute/spawn. The desktop app would bundle/depend on `hc` + require a running core, same as herdr-app installers do (they install herdr-core if missing, and prompt on unusable core).
- `tauri-plugin-pty` (+ xterm.js) exists but is the wrong layer: it gives in-window PTYs owned by the app process — losing close-window-keeps-running, remote machines, status protocol, and agent-children semantics. It is a fallback only if `hc` is absent; even then, prefer spawning `hc` over owning PTYs.
- Consistent with #129 (sidecar wins: ledger owns audit/product decisions, `hc` owns terminal liveness). The desktop client extends the same split: client owns arrangement + board views, `hc` owns liveness.

## Consequences for daemon/tickets

- Daemon (#132) should talk to core via `hc` CLI JSON / `hc connect` streaming + events (`hc events`, `hc get`), not libpty.
- Board read model stays separate (gh Issues polling); terminals come from core — two sources, joined in UI.
- Installer story copies herdr-app: bundle or auto-install `hc`, `hc update` keeps running terminals alive across app updates.
