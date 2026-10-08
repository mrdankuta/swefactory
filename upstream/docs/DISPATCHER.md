# The dispatcher (L3): mechanical scheduling, human acceptance

`bin/factory-dispatch` drains `ready` tickets without human scheduling. It
NEVER lands, merges to dev, or pushes. `accepted` → `/land` stays human.

## Lifecycle (labels)

```
ready → [dispatch reserves] → impl → review → accepted → (human /land) → closed
  ↑                              ↓ changes_requested
  └────── attempts left ──────────┘   (budget exhausted → blocked)
```

`ready`/`running`/`review`/`accepted`/`blocked` labels are managed by the
dispatcher and `factory start`. Humans own `accepted` and `blocked` resolution.

## Operator setup

1. `gh` authed, `pi` authed for non-interactive `pi -p`, sqlite3 installed.
2. Project seeded (`install.sh`), CI green on the branch strategy.
3. Labels present (`install.sh` ensures them, incl. `ready`, `running`).
4. Tickets carry `ticket` + `ready` labels and `Depends on: #N` (or none).
5. Specs frozen (`factory freeze`); dispatcher refuses tickets whose spec
   hash no longer matches.

## Running

```bash
./scripts/factory-dispatch once --dry-run   # see what it would do (no spend)
./scripts/factory-dispatch once              # one pass (cron-safe)
./scripts/factory-dispatch daemon --interval 300 &
./scripts/factory-dispatch status
./scripts/factory-dispatch stop   # human stop; resume to continue
```

systemd user timer or cron calling `once` every 5–15 min is the recommended
L3 shape — no long-lived process to babysit.

## What one pass does (per ticket)

1. Skip if STOP file, at capacity (`MAX_CONCURRENT`, default 1), already
   active (sqlite reservation — duplicate-dispatch safe), `blocked` label,
   open deps, or attempt budget exhausted (`MAX_ATTEMPTS`, default 2).
2. Fetch `origin/dev`, create worktree `../<repo>-T<N>` on
   `factory/T<N>-<slug>`, record attempt row.
3. `verify-freeze` the linked spec — mismatch releases the ticket for human re-freeze.
4. `pi -p` implement (timeout `TIMEOUT_IMPL`), then `scripts/factory-check`
   in the worktree. Non-zero exit or check failure → comment + retry next pass.
5. Empty diff → `no changes` comment, worktree removed. Else commit fallback
   (`feat(T<N>): …`), label `review`.
6. Detached review worktree + `pi -p` reviewer (timeout `TIMEOUT_REVIEW`).
   The reviewer writes `/tmp/T<N>-review.md`; the **dispatcher** posts it as
   the issue comment (reviewer side effects not trusted). `VERDICT: accept`
   → label `accepted` + land-ready comment. Otherwise back to `ready`.

## Safety notes

- Review runs in a detached worktree, but `pi -p` is not an OS sandbox:
  reviewer prompts forbid edits; true containment is future work.
- Retries are bounded; exhaustion lands the ticket in `blocked` with a
  human-readable reason — never silent.
- State is sqlite (`factory/.state/`, gitignored); logs per attempt.
- Cost control: `MAX_CONCURRENT=1` default, dry-run first, timeouts on
  every agent invocation. Set `PI_MODEL` / `PI_BIN` env to pin the worker model.

## Release train (multi-ticket integration)

The train collects accepted tickets on an `integration/*` branch so a batch
can be reviewed and landed together. Dispatcher automation still NEVER
touches `dev`/`main` — the train → `dev` merge is human-only (checklist:
`prompts/review-integration.md`).

### Lifecycle

```
release start [<slug>] → integration/<yyyymmdd>-<slug> cut from origin/dev
  → ready tickets base worktrees on the TRAIN head (not origin/dev)
  → review accept auto-merges the ticket branch into the train (clean only)
    → label `integrated` (merged into the train; distinct from `accepted`)
  → human train→dev review → human merges train into dev
  → releases row `merged` (or `abandoned`)
```

- `release start [<slug>]`: cut the train (default slug `train`). Refuses
  when a train is already open — exactly one open train is allowed.
- `release status`: open train row + member ticket branches (attempts bound
  to the train) + train head sha (the CI-relevant commit).
- `release merge <ticket>`: explicit helper for the accept path; merges the
  ticket's recorded branch with `git merge --no-ff --no-edit` only.
  Conflict (or dirty train worktree) aborts the merge and returns nonzero —
  nothing is ever forced.
- `release drop <ticket>`: reverts that ticket's merge (`git revert -m 1`)
  from the train. Fails loudly (nonzero) when the ticket is not merged —
  there is no silent no-op.
- Accept-path routing: clean merge → `integrated`; merge failure or red
  checks → `blocked` (human resolves, or drops the ticket and re-runs the
  combined gate).

### Drift policy

Automation NEVER rebases the train, NEVER fast-forwards it past merges
except via `release merge`/`release drop`, and NEVER pushes. If the train
needs to catch up with `dev`, a human rebases it with an explicit command
and re-runs the combined gate + CI (`integration/**` triggers
`factory-check`).

### Auto-expire rule

A new train can only start after the old one is `merged` or `abandoned`.
There are never two open trains — `release start` enforces this.
