# swefactory — the software factory as a product

> Vision: production-grade software engineering, consumer-simple UX.
> Read first: `docs/product/FACTORY-PRODUCT.md`.

## Layout

* `upstream/` — exact mirror of the true source, `github.com/mrdankuta/pi-factory` @ `dc477a7` (minus `.git`; worktree at sync included uncommitted v0.5.1 `release finish`/`abandon` changes). Pinned commit recorded in `upstream/UPSTREAM-COMMIT`. This is the rewrite reference.
  * `bin/` — `factory`, `factory-dispatch` · `checks/` — elixir/node/python adapters
  * `prompts/` (incl. `review-integration.md`), `skills/`, `templates/`,
  * `docs/` (`DISPATCHER`, `HERDR`, `SKILL-PINS`, `USAGE`, `WORKFLOW`),
  * `github/` (issue templates + labels + CI workflows), `install.sh`, `README.md`
* `factory-v0.4.0/` — installed/adapted instance as deployed in booksbuddy
  (project shim layout: `scripts/` instead of `bin/`, adapted `SKILL.md`
  files, `AGENTS.header.md` merge result, `GLOSSARY` example). Shows how
  the toolkit lands in a real project; secondary reference.
  * `scripts/` — `factory`, `factory-dispatch`, `factory-check`, `factory-reconcile`
  * `prompts/` — `plan`, `build`, `review`, `handoff`
  * `skills/` — `to-spec`, `to-tickets`, `tdd`, `code-review`, `land`, `pr`, `retro`, …
  * `factory/` — toolkit README (issues-are-authority model)
  * `AGENTS.header.md`, `GLOSSARY.example.md`, `VERSION`
* `docs/product/FACTORY-PRODUCT.md` — product vision (users, Orca learnings, Rust architecture, roadmap).
* `sessions/` — exported working-session notes that led here.

## Origin

Forked from an Orca agent session (`9cff49ef…`, agent `pi`) that was
proving the release-train dispatcher (`integration/20261008-first-train`,
tickets #98/#99). This repo is the independent continuation.

## Next steps

1. Decide wedge (solo builders vs Linear teams) — see open questions in the vision doc.
2. Scaffold Rust workspace: `factory` CLI + `factoryd` daemon (v0.5 parity).
3. `TaskStore: github` backed by the v0.4.0 protocol, then `linear`.
