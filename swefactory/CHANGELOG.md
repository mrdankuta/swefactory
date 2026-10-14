# Changelog

All notable changes to `factory` are documented here. cargo-dist reads this
file when generating release notes for each tagged release.

## [Unreleased]

### 0.1.0-rc.4 — UI hardening + release readiness (spec #27, ticket #34)

- `factory ui` / `factory serve`: localhost-only bind documented with
  `--addr` / `--allow-remote` overrides; no auth in v1 stated loudly
  (the loopback bind is the security boundary).
- Read-only proof now covers every GET endpoint (board, ticket, dispatch,
  git, files, train review, health): byte+mtime snapshots must not move.
- Empty states read as calm Bea sentences in every view.
- Trial release proving the binary still ships clean with the web
  dependencies (axum, askama, tokio) via the tag-driven cargo-dist
  pipeline + per-platform installer smoke.

Download prebuilt binaries and one-line installers from the download home:

https://mrdankuta.github.io/swefactory/
