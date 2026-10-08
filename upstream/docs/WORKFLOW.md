# The Factory (pi-factory)

How product decisions become landed changes. Pi has no subagents:
every role is a separate Pi process in a Herdr pane, with GitHub issues +
commits as handoffs.

## The loop

```
idea / bug report            (GitHub issue — the authority)
  → /plan                    (.pi/prompts/plan.md → grill, then spec issue)
  → tickets                  (ticket issues linked to the spec, tracer bullet first)
  → freeze                   (factory freeze <spec> — SPEC-APPROVED hash comment)
  → /build                   (.pi/prompts/build.md, one worktree + branch per ticket)
  → scripts/factory-check    (format → compile → test → assets; local gate)
  → /review                  (.pi/prompts/review.md, separate pi in Herdr pane, read-only)
  → human accept → land      (project .agents/skills/land/SKILL.md)
```

Handoff signals between panes: `FACTORY:READY_FOR_REVIEW`,
`FACTORY:NEEDS_CLARIFICATION`, `FACTORY:BLOCKED` — waited on with anchored
`pane wait-output`, never bare substrings (recipe: docs/HERDR.md).

## Map

- Product authority: GitHub issues (epic → spec → tickets). Drift check: `factory reconcile`.
- Approval: `factory freeze <spec>` posts the `SPEC-APPROVED <hash>` comment;
  `factory sync <spec>` verifies the body still matches (and writes the
  gitignored `factory/.cache/` snapshot for offline reading).
- Handoffs + review evidence: issue comments via `factory handoff` / `gh issue comment`.
- Roles/prompts: `.pi/prompts/{plan,build,review,handoff}.md` (installed copies).
- Workspace orchestration: `docs/HERDR.md` (panes, session-id hygiene, cleanup).
- Verification: `scripts/factory-check`; reconcile: `factory reconcile`.
- Domain language: `GLOSSARY.md`. Architecture decisions: `docs/adr/`.
- Standing rules: `AGENTS.md` (factory header first, stack rules below).
- Skills: `.agents/skills/` (project-local copies installed by `factory init`;
  upstream SHAs recorded in `docs/SKILL-PINS.md`).

## Policies (learned the hard way)

- Single review round by default; fixes get one pass, noted unverified.
- Reviewers never touch the implementation; implementers never review.
- `assets/`-touching diffs need browser evidence (steps + observed result), not just unit tests.
- Success flashes must only fire after confirmed writes.
- Shared-asset changes require enumerating every usage site.
- Conservative landings: explicit land only, never `--force` on dev.
- Never commit `factory/.cache/` — it is regenerable (`factory sync`).
