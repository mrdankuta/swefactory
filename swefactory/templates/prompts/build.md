# /build — implement exactly one ticket (isolated worktree)

Inputs: the ticket's GitHub issue (+ linked spec issue and its freeze hash).
You are running inside the ticket's worktree on its branch. Read AGENTS.md first.

1. Read the ticket issue's acceptance criteria and the linked spec issue.
   Verify the spec freeze hash still matches (`factory sync <spec>` — hash must
   equal the `SPEC-APPROVED` comment). If it doesn't, STOP and ask for re-freeze.
   Use GLOSSARY.md terms.
2. Implement ONLY this ticket's scope with the tdd skill (red-green-refactor).
   Prefer existing seams; make the smallest vertical slice that satisfies the criteria.
3. If a consequential decision is unresolved, STOP and report it — do not invent policy.
4. Add/extend tests for the criteria (incl. tenant isolation + permissions where relevant).
5. Run scripts/factory-check. Record actual output.
6. Do NOT merge, deploy, touch unrelated deps, or weaken tests.
7. Handoff (dispatcher posts — never post issue comments directly).
   You do NOT have `gh` posting rights in this flow: NEVER run `gh issue
   comment`, `factory handoff`, or any command that writes to GitHub.
   The dispatcher owns all issue comments and labels.
   a. Write the full handoff report (per .pi/prompts/handoff.md: ticket +
      base/head commits, changes made, checks run with ACTUAL output,
      acceptance criteria → evidence each, risks, blast radius, next-step
      owner) to `HANDOFF.md` in the worktree root. This file IS the handoff;
      the dispatcher attaches it to the reviewer brief and posts it.
   b. NEVER `git add`/`git commit` `.factory-brief.md`, `.factory-prompt.md`, or `HANDOFF.md` —
      leave all three uncommitted in the worktree; the dispatcher posts HANDOFF to the issue
      and deletes them before committing (empty-change work finishes with dispatcher-created
      `--allow-empty` commit; implementer must not create its own). Leave all work uncommitted —
      do NOT create your own commits; the dispatcher owns all commits on the ticket branch.
   c. As the LAST line of stdout, print exactly one terminal signal line
      and nothing after it:
      `FACTORY:READY_FOR_REVIEW`, `FACTORY:NEEDS_CLARIFICATION`, or
      `FACTORY:BLOCKED` (matching the handoff verdict). The dispatcher waits
      on this line instead of polling agent state — it must be byte-exact.
