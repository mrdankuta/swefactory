# factory/

Work state lives in **GitHub issues** (epic #1; specs `#93`+; tickets like `#94`;
closed: Slice 12 `#13`). Issues are the authority — there are no committed
spec/ticket/review `.md` files.

- Approval: `scripts/factory freeze <spec>` → `SPEC-APPROVED <hash>` comment.
- Local cache: `scripts/factory sync <issue>` → gitignored `factory/.cache/`.
- Handoffs + review evidence: issue comments (`scripts/factory handoff`, `gh issue comment`).
- Drift: `scripts/factory reconcile`.

Toolkit source: [mrdankuta/pi-factory](https://github.com/mrdankuta/pi-factory)
(prompts, CLI, skills, issue templates). `scripts/factory update` refreshes.
