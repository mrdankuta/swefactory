# /handoff — report a finished unit of work (no new work)

Report exactly (post to the ticket issue with `factory handoff <issue> --signal ... --body FILE`):
- Ticket + GitHub issue link; base commit → head commit; branch/worktree.
- Changes made (files/behaviour, no unrelated scope).
- Checks run: commands + ACTUAL results (paste failures verbatim). Never claim unrun checks.
- Acceptance criteria → evidence (test name or observed behaviour) each.
- Risks, migration/compat notes, unresolved questions.
- Blast radius: one-word scope of the change + ramifications if wrong.
- Next step owner (needs review / needs human decision / ready to land).
- End with the terminal signal line (`FACTORY:READY_FOR_REVIEW`,
  `FACTORY:NEEDS_CLARIFICATION`, or `FACTORY:BLOCKED`) so the orchestrator
  can `pane wait-output` on it instead of polling agent state.
  `factory handoff` prints the signal line for you after posting the comment.
