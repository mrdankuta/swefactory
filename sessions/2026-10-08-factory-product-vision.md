# Session export — 2026-10-08, factory product vision

> Forked from Orca session `9cff49ef-ab2c-434d-83c7-13d0c9aeaacc`
> (`2c746b51-ba32-454d-813e-3379a541fe1d`), agent `pi`.
> Original terminal context: cutting the first live release train
> (`integration/20261008-first-train` from `843e598`), dispatching tickets
> #98 (skill SHAs) and #99 (train docs) with `MAX_CONCURRENT=1`.
> This session is independent — no coordination with the original terminal.

## Conversation arc

1. **Fork acknowledgement.** Confirmed independent session, waiting for instruction.
2. **"Can the factory be bundled as a product?"** Explored the repo
   (`scripts/factory` 0.4.0, `factory-dispatch`, `factory-check`,
   `factory/README.md`). Answered: yes, ~60% there — installer + workflow
   CLI + autonomous runner + gate + prompts/skills. Sketched 3 layers
   (OSS core / runner moat / optional control plane), friction fixes
   (`doctor --fix`, `status`, `land --checklist`), hardening (runner rewrite,
   cost caps, CI-gated trains, Linear/Jira adapters).
3. **Consumer-product reframing.** User: not just an engineer's tool but a
   consumer product — high-quality practices, radically simple UI, for
   engineers *and* non-technical builders. No lock-in to Herdr or Pi TUI.
   Open to Rust or Go. Asked to research Orca IDE/repo, liked GitHub/Linear/
   Jira depth and agent-drivable `orca-cli` (like Herdr panes).
4. **Orca research (Tavily).** Findings: `stablyai/orca` (MIT, ~76k★) is an
   agentic orchestrator (ADE): parallel git worktrees, BYO 27 CLI agents,
   `orca worktree/terminal/snapshot/click/fill/emulator/automations/artifacts`
   CLI with `--json` for humans and agents alike, native GitHub/Linear/Jira/
   GitLab tasks, inline diff annotation → send back to agent, SSH worktrees,
   mobile companion, skills registry. Proposed: don't compete with the ADE —
   factory owns the *method* (freeze → tickets → isolated builds → separate
   review → train → human land) and runs inside any orchestrator.
5. **This export.** User approved the vision doc + asked to: copy factory to
   `~/Deving/rc_int/swefactory`, git-init it, and carry the session over.

## Decisions captured (in `docs/product/FACTORY-PRODUCT.md`)

* Product = quality engine, two faces (invisible rigor / simple UI).
* Users: Builder Bea (non-technical) + Engineer Eli (owns production).
* Orca mapping table (6 patterns → factory equivalents).
* Rust single binary (`factory` CLI + `factoryd` daemon, tokio), thin web UI
  later; ship a `factory` skill for Orca/Herdr interop. No new editor/model/tracker.
* Roadmap: v0.5 Rust parity (gh + pi-headless) → v1.0 daemon + Linear +
  browser acceptance + metrics → v2.0 Jira + hosted dispatch + RBAC/SSO.
* Open questions: wedge (solo vs Linear teams), Orca relationship
  (skill vs standalone UI first), greenfield scaffolding opinion, pricing
  (open-core + hosted dispatch + team packs).

## Continuation pointers

* Work from `~/Deving/rc_int/swefactory` from here on.
* Proven behavior reference: `upstream/` (true source, pinned @ `51df377` — do not edit — rewrite target). `factory-v0.4.0/` shows the installed/adapted shape in a real project.
* Next: answer the 4 open questions, then scaffold the Rust workspace.
