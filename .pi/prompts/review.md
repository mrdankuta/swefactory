# /review — independent review of a fixed diff (no edits)

Pi has no subagents: run the reviewer as a separate Pi process in a Herdr
pane (recipe: `docs/HERDR.md` in pi-factory). Never review your own diff in-session.

## Orchestrator steps (the implementer or integrator runs these)

1. Fix the revision: the work must be committed. Record BASE (parent/dev HEAD)
   and HEAD (ticket commit). They must not move during review.
2. Give the reviewer an immutable checkout:
   `git worktree add ../<repo>-review-<sha> <HEAD>` (detached HEAD).
3. Open the review pane and agent (from any pane in the workspace):
   `herdr pane split --pane <shell-pane> --direction right --cwd <worktree>`
   then `herdr agent start pi --kind pi --pane <new-pane> --timeout 60000`.
4. Submit the brief below with `herdr agent prompt <pane> "$(cat brief)"`.
   Prefer the signal wait: have the brief require a terminal `FACTORY:`
   signal and wait with `herdr pane wait-output <pane> --regex
   '^\s*FACTORY:(READY_FOR_REVIEW|NEEDS_CLARIFICATION|BLOCKED)\s*$'`
   (see pi-factory docs/HERDR.md for why anchored, not substring). Fall back to
   `herdr agent wait <pane> --timeout 600000`.
5. Collect: the reviewer writes its full report to `/tmp/<ticket>-review.md`
   and pastes verdict + findings in reply. Read the pane
   (`herdr pane read <pane> --lines 60 --format text`); if scrollback
   truncated the report, take the file. Post the report as a comment on the
   ticket issue (`gh issue comment <ticket> --body-file /tmp/<ticket>-review.md`).
6. Clean up: `herdr pane close <pane>` and
   `git worktree remove --force <worktree>`.

## Reviewer brief (must be self-contained — the reviewer sees nothing else)

- Repo + worktree path; BASE_COMMIT; HEAD_COMMIT (single commit if possible).
- Spec issue number + `SPEC-APPROVED` freeze hash; ticket issue number;
  implementer handoff comment URL (verify its claims, do not trust them).
  The reviewer re-fetches issue bodies with `gh` and checks the body hash
  against the freeze hash before starting — mismatch blocks review.
- Project rules pointer: AGENTS.md.
- `git diff BASE..HEAD --stat` then the full diff is the review scope.
- Pass 1 — specification compliance (Spec axis): every acceptance criterion
  → supporting test/evidence or explicit MISSING, tagged `[spec-drift]` where
  the implementation diverges from the spec. Tenant boundaries
  (`[tenant-violation]`), permission gaps (`[authz-gap]`), weak tests (`[weak-test]`).
- Pass 2 — engineering standards & risk (Standards axis): AGENTS.md rules,
  unnecessary complexity (`[complexity]`), migration/compat/rollback risk,
  secrets (`[secret-leak]`; grep, false positives noted).
  Shared-asset sweep (required when hook JS, components, or contexts change:
  enumerate every usage site repo-wide and state each site's fate).
  Interactive elements require behavior assertions, not presence-only tests.
- Checks: fresh `deps.get` (new worktree), focused test file(s) with actual
  output, `compile --warnings-as-errors` (or the stack's `scripts/factory-check`
  equivalent). Known env gaps go in the brief (what to skip and how to
  compensate). Browser smoke (for `assets/`-touching diffs): steps + observed
  result, or "not run" — which blocks accept.
- Report only actionable violations, each as: `[tag]` one-sentence
  description + file ref. End with a `Checked:` summary line listing every
  category examined (e.g. `Checked: spec-drift, tenant-violation, authz-gap,
  weak-test, complexity, secret-leak`). Categories with nothing to report
  are omitted from findings but present in `Checked:`.
- Policy: single review round by default. Issues found get one fix pass with
  NO re-review loop — the fix is noted as unverified and the human judges at
  land. Multi-round only on explicit human request.
- Rules: READ ONLY — no file creates/edits/deletes in the worktree. Full
  report goes to `/tmp/<ticket>-review.md`; reply contains verdict
  (accept | changes_requested with concrete fixes + re-review scope)
  plus findings by severity with file refs. Reviewer starts pi with a
  unique session id (`review-<ticket>-<yyyymmdd>`, see docs/HERDR.md).
  End the reply with the terminal signal line, e.g.
  `FACTORY:READY_FOR_REVIEW`, so the orchestrator can wait on it.
