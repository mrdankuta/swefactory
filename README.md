# swefactory — the software factory as a product

> Vision: production-grade software engineering, consumer-simple UX.
>
> Download: https://mrdankuta.github.io/swefactory/ (prebuilt binaries +
> one-line installers for every release).

Single static `factory` binary (Rust, `swefactory/`): issues are the
authority, the dispatch ledger is the audit trail, and an optional
[herdr-core](https://herdr.dev/core) sidecar gives attempts persistent,
attachable terminals (local or on remote builders).

## Layout

* `swefactory/` — the product: CLI + library, embedded templates,
  integration tests. See `swefactory/README.md` for commands.
* `swefactory/templates/` — prompts, skills, check adapters, issue
  templates, and herdr-core agent-role templates, all baked into the
  binary at compile time.
* `scripts/factory-check` — the repo's own gate: `fmt --check` →
  `clippy -D warnings` → `cargo test`. Run before every handoff.
* `factory/` — local runtime state (gitignored): dispatch ledger,
  logs, worktrees, cache snapshots.
* `docs/adr/` — architecture decisions (`0001`: herdr-core stays a
  sidecar; ledger owns audit, core owns liveness).
* `GLOSSARY.md` — domain vocabulary used in issues and code.
* `oranda.json` — download-site config (Pages home + cargo-dist
  artifacts source).

## Workflow

Product decisions live in GitHub issues (epic + spec + ticket); the
spec issue body at the `SPEC-APPROVED` freeze is authoritative.
`factory freeze/sync` manage the freeze, `factory dispatch` runs
attempts through implement → verify → review, and humans land via the
release train. See `AGENTS.md` for the delivery rules.

## Install & release

```sh
# From a release (no toolchain):
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/mrdankuta/swefactory/releases/latest/download/factory-installer.sh | sh
factory --version
```

```sh
# From source:
cargo install --path swefactory --locked
scripts/factory-check      # green before every handoff
```

Releases are tag-driven: bump `swefactory/Cargo.toml` + `CHANGELOG.md`,
commit, push tag `vX.Y.Z` — cargo-dist builds and uploads all
platforms plus installers (see `.github/workflows/release.yml`).
