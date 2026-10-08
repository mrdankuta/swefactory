# Pinned factory skills (vendored under skills/ — do NOT depend on global installs)

`factory init` copies these into the project's `.agents/skills/` (never
overwriting existing ones), so any checkout works without global setup.
`--global-skills` installs to `~/.agents/skills/` instead.

| Skill | Purpose | Upstream |
|---|---|---|
| grill-with-docs | requirements interview + glossary/ADR | mattpocock/skills (local snapshot; record commit when vendoring) |
| to-spec | synthesize discussion → spec | mattpocock/skills |
| to-tickets | spec → dependency-aware vertical slices | mattpocock/skills |
| tdd | red-green-refactor implementation | mattpocock/skills |
| diagnosing-bugs | disciplined debug loop | mattpocock/skills |
| code-review | standards + spec compliance passes | mattpocock/skills |
| pr | PR-body Evidence/Blast-Radius sections (reference only — no PRs in our flow; Blast radius adopted into handoff/review shape) | mattpocock/skills |
| retro | env/process improvement proposals | mattpocock/skills |
| land (per-project .agents/skills) | push reviewed scope to the project's dev branch | template: templates/land-skill.template.md — adapt per project |

Adaptation note: upstream orchestration-heavy skills assume subagents Pi
doesn't ship. Here: implementers = separate Pi processes in Herdr panes;
standards + spec reviews = separate sessions; handoff via issues + commits.
When re-vendoring, record the upstream SHA below.

## Vendored SHAs
<!-- per-skill last upstream commit touching skills/engineering/<skill>;
     SKILL.md + supporting files byte-identical to upstream at the pin
     and at HEAD f3fc563 (2026-10-07). agents/openai.yaml intentionally
     omitted (Codex-only metadata; `factory init` copies skills/ as-is). -->
- grill-with-docs: `447ca70` 2026-08-15 — Clarify multi-skill steps as multiple Skill tool calls (verified)
- to-spec: `5c89081` 2026-08-19 — fix: quote SKILL.md descriptions with unquoted colons (verified)
- to-tickets: `9e2abf8` 2026-10-06 — tighten sub-issue and Blocked-by wording, drop GitLab (verified)
- tdd: `3f59913` 2026-10-07 — note what each proposed seam catches and misses (verified)
- diagnosing-bugs: `f3fc563` 2026-10-07 — prove a forced mutation landed before trusting the red (verified)
- code-review: `3da8c01` 2026-10-07 — search for standards files, foreground sub-agents (verified)
- pr: `e484a80` 2026-09-24 — Merge #876: rename CONTEXT.md convention to GLOSSARY.md (verified)
- retro: `a7d038f` 2026-09-24 — Graduate pr and retro to engineering (verified)
