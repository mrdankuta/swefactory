<!-- factory:header:start -->
# Factory: how we deliver changes (applies to all work)

> Managed by [`factory`](https://github.com/mrdankuta/swefactory) (`factory update` refreshes the shaded block only — stack rules below are yours).

## Source of truth
- Product decisions: GitHub issues (epic + spec + ticket issues). The issue is authoritative.
- Approved specs: the spec issue body at the `SPEC-APPROVED <hash>` freeze comment
  (`factory freeze <issue>` records it; `factory sync <issue>` verifies / caches locally).
- Local `factory/.cache/` holds gitignored snapshots only — never the authority, never committed.
- Architecture decisions: `docs/adr/`.
- Domain vocabulary: `GLOSSARY.md` — use its terms in issues and code.

## Working rules
- Implement only the assigned ticket. Read the linked spec issue (verify freeze hash) before changing code.
- Keep changes small and focused. Prefer one complete vertical path (tracer bullet) before parallel expansion.
- If requirements conflict or a consequential domain/auth/tenancy decision is unresolved, stop and report. Do not invent business rules.
- Never weaken tests or CI to make a change pass.

## Verification
- Run `scripts/factory-check` before every handoff. Record commands executed and actual results.
- Never claim a check passed if it was not run. Add regression tests for bug fixes.
- Tenant isolation, permissions, monetary calculations, and duplicate-request handling get explicit tests wherever relevant.

## Boundaries (guidance, not sandbox)
- No production credentials or customer data. No deployment or destructive DB ops from implementation sessions.
- Ask before dependency, schema, authentication, authorization, or tenant-boundary changes.
- Production deploys and destructive migrations are separate human approvals, executed outside implementation sessions.

## Handoff
Report via `factory handoff <ticket-issue>`: ticket + base/head commits, changes made,
checks run (actual output), acceptance criteria satisfied with evidence, risks and open questions.
Review runs in a separate Pi process in a Herdr pane — never in-session (recipe + brief in `.pi/prompts/review.md`) — against the fixed diff with findings posted to the ticket issue.
Only the human moves a ticket to accepted/landed.
<!-- factory:header:end -->
