# ADR-0001: Keep herdr-core as an execution sidecar (persistent terminals)

- Status: accepted
- Date: 2026-11-04
- Scope: tickets #123–#129 (shipped in 0.1.0-rc.11)

## Context

The spike series asked whether the factory should keep shelling out to
one-shot `pi -p` subprocesses, adopt herdr-core (`hc`) as a durable
execution sidecar, or move execution onto `hc` entirely (ledger as a
client of the core: delegation protocol, `doc` facts, killing our
wait/kill machinery). The two candidate architectures:

- **A. Sidecar (chosen):** the factory owns audit and product decisions
  (dispatch ledger, spec freeze hashes, train membership, land). `hc`
  owns terminal liveness (spawn, wait, read, attach, remote machines).
  The ledger records terminal addresses; it never surrenders the
  attempt record.
- **B. hc-owned kernel:** dispatch phases become `hc` delegation
  (`run`/`send`/`wait --children`), facts move to `doc.set`, and the
  subprocess runner (`run_phase`, process-tree kills, timeout resumes)
  is deleted.

## Measurements from the spike (0.1.0-rc.11, commits 124c005–435a26a)

- Footprint: +1485/−35 lines across 13 files, 6 commits. Production
  code is one new seam (`hc_bin`, like the existing `pi`/`gh` seams),
  one execution branch (`run_phase_hc`, implement-phase only, behind
  `--persistent-terminal`), one ledger field (`terminal`, serde-default
  `""`), and read-only surfaces (`log` live tail, board card marker).
  The default path (flag off) is byte-identical: no existing test
  changed behavior, only gained the new field in one exact-match
  fixture (`cli_cutover`).
- Coverage added: dispatch 34 tests (4 persistent-terminal incl.
  remote), log 12 (3 live-tail), doctor 19+13, init 35+3, lib 61
  (5 pure-helper). Full suite green except pre-existing
  `site_oranda::oranda_config_points_at_dist_releases`, which fails on
  the clean tree.
- Live verification against real `hc 0.1.17` (this machine, 2026-11-04):
  every wire shape the spike parses leniently was observed directly.

  | Command | Observed shape | Spike handling |
  |---|---|---|
  | `run --bg --json --name N --on M --cwd D -- prog args` |accepted; returns object with `address`, `name`, `state`, `first_message:{sent,started,turn}` | address extracted leniently; `--` separator confirmed |
  | `wait --json ADDR --timeout N` | `{"terminal","exit_code","reason":"exit",…}` on finish | blocks; loop re-waits on check-in |
  | `get --json ADDR` | `state` live/finished, top-level `exit_code`, `reason: exited`, `first_message.sent` | finished/exit-code/delivery all read here, never inferred |
  | `read --lines 0 ADDR` | screen + history text | appended to attempt log |
  | `machine ls` (piped) | `{"machines":[]}` | substring preflight for the machine name |

- What the spike did **not** prove: multi-restart resume re-entry
  (stale `impl_running` row pointing at a live terminal still reaps to
  an interrupted checkpoint and spawns fresh), timeout-orphan cleanup,
  and remote worktree provisioning (`--cwd` must already resolve on
  the builder).

## Decision

**Stay with A (sidecar).** The spike shows the valuable half of the
integration — durability, attachability, remote scheduling, live
surfaces — arrives as a small additive seam with the ledger untouched
as the audit authority. Option B would delete working machinery
(`run_phase` streaming, checkpoint/resume, exactly-once keys,
`--fix` repair) to re-depend on failure modes we have not observed
(core restarts, `missed` replay under load, `doc` CAS contention),
with no demoable payoff beyond what A already delivers.

Concretely, out of scope indefinitely: agent-spawns-agent delegation
(implementers never spawn reviewer children; the dispatcher keeps
orchestrating phases), `doc.*` facts transport, and deleting the
subprocess path (it stays the default; `--persistent-terminal` is
opt-in per pass).

## Consequences

- `hc` is now a soft dependency with one hard edge: `factory doctor`
  reports red (`agent-runtime`) when `hc` is missing. That ratchet is
  intentional (sidecar direction) but pre-dates any user-facing need —
  if it bites before the execution path is default, soften to a
  warning in a follow-up ticket.
- Kill-vs-keep on expiry is deliberately undecided: timed-out
  terminals keep running and the retry spawns a successor (double-spend
  risk noted in the log). Any lifecycle policy change goes through a
  new ticket, not a quiet default flip.
- Real-`hc` gaps closed by live probing (table above); the fake `hc`
  in `cli_dispatch` mirrors observed shapes, not guesses.

## Revisit when

- Resume-after-restart re-entry is needed (reattach to the recorded
  terminal instead of reaping + respawning), or
- the orphan-on-timeout rate (measured, not anecdotal) justifies a
  kill/keep policy, or
- a second consumer needs the terminal record (watch-pane streaming,
  multi-machine fan-out) and the `log`-append bridge strains.

Any of these reopens the question as a new ticket with new
measurements; this ADR is not a blanket approval for B.
