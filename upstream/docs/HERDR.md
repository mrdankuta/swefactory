# Herdr workspace — Pi-native orchestration (no subagents)

Pi ships no subagents. Parallelism and independence come from separate Pi
processes in Herdr panes, with files + commits as handoffs. Verified on
herdr 0.8.2 / pi 1.0.4; all commands run from any pane via the socket API
(no Herdr skill required).

## Layout (3 panes to start; 4th for review)

```
Workspace: acctmgt_app (find yours: herdr pane list)
├── Planning / integration  — pi (this checkout; owns specs, tickets, /land)
├── Implementation          — pi in task worktree (../acctmgt-<Txxx>, ticket branch)
├── Verification            — tests, server, logs (mix phx.server, factory-check output)
└── Review (on demand)       — fresh pi in detached-HEAD worktree, read-only
```

Setup (once): `herdr integration install pi`, then `herdr` in the repo.

## Implementation pane

```bash
git worktree add ../acctmgt-T001 -b factory/T001-<slug>
herdr pane split --pane <an-idle-shell-pane> --direction right --cwd ../acctmgt-T001
herdr agent start pi --kind pi --pane <new-pane> --timeout 60000
herdr agent prompt <new-pane> "$(cat build-brief.txt)"
```

Start every ticket agent with a unique session id — pi resumes any session
whose id already exists, so a reused id replays a previous run's state:

```bash
herdr pane run <pane> "pi --session-id impl-T001-20261008 --name 'impl T001'"
```

Slugs look like `<role>-<ticket>-<yyyymmdd>` (e.g. `impl-T001-20261008`,
`review-T001-20261008`). Stable *names* are fine and expected (addressing
by name); only session ids must be unique per ticket and role.

One worktree + one branch per ticket; separate test DB/ports per concurrent
worker; no production secrets in worktrees. Start with ONE implementer; add a
second only for tickets with no shared interfaces. Worktrees are NOT sandboxes
(shared shell/env/creds) — isolate secrets, not just files.

## Review pane (full recipe; short form lives in .pi/prompts/review.md)

```bash
# immutable checkout of the reviewed revision (detached HEAD — cannot drift)
git worktree add ../acctmgt-review-<shortSHA> <HEAD_SHA>
herdr pane split --pane <shell-pane> --direction right --cwd ../acctmgt-review-<shortSHA>
herdr agent start pi --kind pi --pane <new-pane> --timeout 60000
herdr agent prompt <new-pane> "$(cat /tmp/review-brief.txt)"
herdr agent wait <new-pane> --timeout 600000   # blocks until idle/done/blocked
herdr pane read <new-pane> --lines 60 --format text
```

Prefer event-waiting over state-polling: `agent wait` only observes
lifecycle (`idle` = ready for input, not "finished"). For handoffs, have
the agent print a `FACTORY:` signal as its last line and wait for the
signal itself:

```bash
herdr pane wait-output <pane> --regex '^\s*FACTORY:(READY_FOR_REVIEW|NEEDS_CLARIFICATION|BLOCKED)\s*$' --timeout 600000
```

The `^\s*…\s*$` anchor form matters: pi's TUI indents every content
line, so a strict `^FACTORY:` never matches, while a bare substring
matches the agent's own reasoning prose *about* the signal. The anchored
form matches only a real single-line signal. Signal vocabulary:

- `FACTORY:READY_FOR_REVIEW` — implementer done, checks green, committed.
- `FACTORY:NEEDS_CLARIFICATION` — blocked on an undecided question (text follows).
- `FACTORY:BLOCKED` — unrecoverable (reason in output).

Reviewer contract (put it in the brief): read-only worktree, no file writes
there; full report to `/tmp/<ticket>-review.md`, verdict + findings in reply.
If scrollback truncated the reply, take the file (same machine). Move it to
`factory/reviews/` on the ticket branch, then:

```bash
herdr pane close <new-pane>
git worktree remove --force ../acctmgt-review-<shortSHA>
```

Proven 2026-10-07 (SPEC-013): brief → working → idle in one wait; report
recovered from pane + file; verdict accept with one real finding (second
hook instance) the implementer had missed.

## Fallbacks

- `fusion` sidekick (pi's own delegation) is an alternative when enabled;
  Herdr panes are the default because they are observable (user can watch),
  use the exact repo environment, and need no extra trust decisions.
- `herdr agent get <pane>` for status; `herdr pane list` to find IDs.
  Pane IDs look like `w4:p5`; they change per session — always list first.
