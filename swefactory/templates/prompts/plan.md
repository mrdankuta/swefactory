# /plan — clarify then specify (no implementation)

You are in the planning session. Authority is the GitHub issue, not local files.
Skills: `grill-with-docs` then `to-spec` (both vendored in `.agents/skills/`).

1. Interview the user on users, permissions, tenant boundaries, validation,
   duplicates, audit behaviour, and non-goals. Inspect existing code where relevant.
2. Record agreed terms in GLOSSARY.md and decisions in docs/adr/ (propose; don't auto-apply).
3. Write the spec as a GitHub issue using the `spec` issue template
   (`.github/ISSUE_TEMPLATE/spec.yml`): problem, users/permissions, behaviour,
   domain rules, acceptance criteria, non-goals, design, verification, release,
   open decisions. Link the epic (`Depends on:` / epic tasklist).
4. Do NOT implement. Do NOT move past open consequential decisions.
5. End with: issue link, open decisions needing human approval.
   Optional local cache for offline reading: `factory sync <issue>`
   (writes gitignored `factory/.cache/SPEC-<n>.md` — never commit it).
