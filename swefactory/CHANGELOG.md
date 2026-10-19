# Changelog

All notable changes to `factory` are documented here. cargo-dist reads this
file when generating release notes for each tagged release.

## [Unreleased]

## 0.1.0-rc.9 — supervised worker pool

### Concurrent attempts up to MAX_CONCURRENT, default serial (ticket #92)

- `factory dispatch once --concurrency N` (1–8) admits waves of up to N
  tickets simultaneously in isolated slots (own worktree, branch, logs,
  temp dir); with concurrency 1 the pass is byte-identical to the
  historical serial pass. A third ticket waits with a calm waiting line.
- One setting, two surfaces: the CLI flag, `FACTORY_MAX_CONCURRENT` /
  `MAX_CONCURRENT` env, and the dispatcher UI control resolve every pass
  (flag > env > saved file > default 1) with no restart; a daemon
  startup flag seeds the saved level once and never shadows later UI
  writes. Out-of-bounds input is refused with the bounds named.
- A worker killed mid-attempt is reaped into a resumable checkpoint;
  STOP halts admission between waves while in-flight workers finish
  their phases. Per-ticket spend rides `status` output in both modes;
  `--dry-run` prints the worker plan and changes nothing.

## 0.1.0-rc.8 — readable status, watch pane, transcript view, bounded waits, timestamp polish

### One timestamp rule for both eras (ticket #96)

- Human output in `watch`, `status`, `dispatch status` and `log`
  renders epoch seconds and pre-cutover UTC civil strings identically
  (`YYYY-MM-DD HH:MM:SS` UTC); `--json` keeps the raw ledger strings
  in every machine timestamp field. Regression test pins one
  formatted timestamp from each era through the watch CLI seam.
- Test-harness hermeticity: every dispatch-running builder scrubs
  ambient `MAX_ATTEMPTS` / `MAX_CONCURRENT` / `TIMEOUT_IMPL` /
  `TIMEOUT_REVIEW` so the dogfood service env (budget 3) can no
  longer flip the default-budget tests.

### Cutover: one-shot bash-SQLite → Rust-JSON import (ticket #88)

- `factory dispatch import [--db PATH]`: reads the bash dispatcher's
  SQLite history (`attempts` + `releases` rows) and writes the
  equivalent Rust JSON ledger (`dispatch.json` + `train.json`), then
  archives the `.db` byte-identical (sha256-verified, source re-hashed
  to prove it was never mutated). Pure Rust with no new dependencies:
  parses the SQLite format directly, never links sqlite — the binary
  stays dependency-free on every target incl. musl-static. One-shot:
  refuses when the state dir already holds a ledger, a train, or an
  archive, so re-running never duplicates. Closed releases are terminal
  history (counted, preserved in the archive, never carried); the newest
  open release becomes the Rust open train. Cutover window documented
  in `docs/runbooks/cutover.md`; hermetic fixture proof in
  `cargo test --test cli_cutover` (no live state, no network, no
  `sqlite3` at test time).

## 0.1.0-rc.7 — durable attempts, shared memory, experiments, live data

### Shared cross-attempt state, slice 1: facts document + reads (spec #64, ticket #77)

- `factory facts show [PATH]`: read the revision-guarded facts document
  (`facts.json` beside the dispatch ledger) whole or at one
  slash-separated path, human-readable by default and identical
  value-plus-revision facts with `--json`. An absent document reads as
  empty at revision zero with no migration; missing paths read as plain
  sentences (never zero-filled, never error-shaped). Read-only:
  observing changes nothing.

### Durable attempts (spec #59, tickets #60–#63)

- Checkpointed attempt phases (`plan → implement → verify → review`):
  kills resume the phase instead of restarting the attempt; kills
  mid-tool read as `interrupted` (resumable), distinct from `failed`.
- Exactly-once retry keys (`dispatch once --idempotency-key`, 24h
  retention): repeats replay the stored outcome with a marker;
  cross-operation reuse is rejected.
- Stale-lease reaper + STOP-safe halts: dead workers become resumable
  checkpoints; halting never destroys work.
- Follow-ups: decision memos (`dispatch memo`, #67), child-task waiting
  policies (`dispatch wait --policy failFast|allSettled`, #68),
  foreground/background tasks + bottom-up abort (#69, landed via #86
  rework resolving the `tasks`-field collision), live task graph
  (`dispatch graph`, #70).

### Shared cross-attempt state, slices 2–3 (spec #64, tickets #78–#79)

- Guarded writes (`facts create/set/delete --revision`): one atomic path
  op per write; stale revisions rejected naming both; no silent
  last-writer-wins.
- Atomic counters, document size caps, path validation, calm conflict
  sentences.
- Follow-ups: time-travel reads (`facts show --at`, `facts history`,
  #71) and subscriptions (`facts watch`, #72).

### Fork-at-entry experiments (spec #65, tickets #80–#82)

- `factory fork --at <entry> --name alt`: prefix inherited, original
  byte-identical, own worktree/branch/slot, name-or-number addressing.
- Nesting + lineage (`log --fork`, status, oldest-first chains) and
  owned vs ownerless lines (`fork --owned`, #73); replayability per
  line; exactly one human-chosen line lands (#81, #82).

### Live event streams (spec #66, tickets #83–#85)

- Push over poll (hello → snapshot → deltas per scope); fresh-snapshot
  reconnects; slow-viewer disconnect with re-attach; close-all-clients
  safety; task-graph watch, compact feed, sub-second partial output
  (#74); structural read-only boundary; version-gated evolution.

### Guards and fairness

- Lease-death guard (#75): no pass dies holding a lease on a missing
  worktree.
- Spec-link fix (#76): `Part of #N` bodies engage freeze verification
  (previously skipped silently).
- Oldest-ready-first pass order (#87): ascending ticket order so old
  tickets never starve behind new ones.

### 0.1.0-rc.6 — styled board + outcome metrics (spec #56, tickets #57–#58)

- Board and all nine page shells restyled in the kanban design language
  (embedded hermetic stylesheet, Trello-style lanes, status accents,
  dark transcripts); prototype variants B/C discarded after review.
- Outcome metrics dashboard (query-only, reconcile-shaped): per-ticket
  cycle time, attempt minutes, and a caveated autonomy verdict — no new
  instrumentation, dual-audience figures for CLI now and cards later.

### 0.1.0-rc.5 — init learns the project (spec #45, tickets #46–#50)

- `factory init` auto-detects the project stack from manifest files
  (precedence-ordered, evidence reported) instead of silently defaulting
  to Rust; empty directories get an honest generic stub, unknown stacks
  a parameterized one.
- New `--interactive` guided mode (max two questions, never prompts in
  JSON mode or piped input) and a runtime-probing smart node adapter
  (lockfile package manager, config-gated lint/typecheck).
- Six new first-class check adapters: go, ruby, php, java, dotnet, swift.
- Seeded predecessor-project references removed; layout contract table
  documented (integration contracts vs factory-owned).
- Robustness: `gh` stdout is filtered for version-manager noise before
  JSON parsing (all commands), and the dispatcher recognizes
  `Part of #N` spec links.

### 0.1.0-rc.4 — UI hardening + release readiness (spec #27, ticket #34)

- `factory ui` / `factory serve`: localhost-only bind documented with
  `--addr` / `--allow-remote` overrides; no auth in v1 stated loudly
  (the loopback bind is the security boundary).
- Read-only proof now covers every GET endpoint (board, ticket, dispatch,
  git, files, train review, health): byte+mtime snapshots must not move.
- Empty states read as calm Bea sentences in every view.
- Trial release proving the binary still ships clean with the web
  dependencies (axum, askama, tokio) via the tag-driven cargo-dist
  pipeline + per-platform installer smoke.

Download prebuilt binaries and one-line installers from the download home:

https://mrdankuta.github.io/swefactory/
