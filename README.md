# swefactory — the software factory as a product

> Vision: production-grade software engineering, consumer-simple UX.
> Read first: `docs/product/FACTORY-PRODUCT.md`.

## Layout

* `pi-factory/` — git submodule of the true source, `github.com/mrdankuta/pi-factory` @ `0343c74` (v0.5.1+). This is the rewrite reference. Clone with `git clone --recurse-submodules`, or run `git submodule update --init` after clone.
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
