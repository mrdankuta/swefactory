# swefactory — the software factory as a product

> Vision: production-grade software engineering, consumer-simple UX.
> Read first: `docs/product/FACTORY-PRODUCT.md`.

## Layout

* `factory-v0.4.0/` — frozen snapshot of the working bash factory (v0.4.0),
  exported from booksbuddy Oct 2026. Source of proven behavior for the Rust rewrite.
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
