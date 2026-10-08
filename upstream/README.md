# pi-factory

Replicable delivery factory for Pi + Herdr projects. GitHub issues are the
authority (epic → spec → tickets); this repo holds the installer, the `factory`
CLI, prompts, issue templates, check adapters, and vendored skills — so any
project works without depending on the user's global setup.

## Quickstart (new or existing project)

```bash
cd /path/to/project
curl -fsSL https://raw.githubusercontent.com/mrdankuta/pi-factory/main/install.sh | bash -s -- --stack elixir
# stacks: elixir | node | python
```

What `factory init` does (idempotent, never overwrites):
1. Checks `gh` (authenticated) and `herdr` — warns with install hints if missing.
2. Installs `.pi/prompts/{plan,build,review,handoff}.md`.
3. Copies vendored `skills/*` into `.agents/skills/` (skips existing).
4. Installs `scripts/factory-check` (stack adapter) + `scripts/factory` (project shim).
5. Merges the factory header into `AGENTS.md` (marker-based; stack rules stay yours).
6. Seeds `GLOSSARY.md`, `docs/` skeleton only if missing.
7. Installs `.github/ISSUE_TEMPLATE/{spec,ticket}.yml` + factory labels (if `gh` authed).

## CLI

`factory init | update | freeze <issue> | sync <issue> | handoff <issue> --signal SIG | reconcile | check | version`

- `freeze` records spec approval (`SPEC-APPROVED <hash>` comment + `spec-approved` label).
- `sync` verifies the body against the freeze and writes gitignored `factory/.cache/`.
- `handoff` posts the handoff comment and prints the `FACTORY:` signal line.
- `reconcile` is the read-only drift report (open issues vs `origin/dev`).

## Layout

```
VERSION  install.sh  bin/factory
prompts/            plan, build, review, handoff (issues-native)
docs/USAGE.md      this guide: install, daily loop, dispatcher ops
templates/          AGENTS.header, GLOSSARY, docs-README, review-evidence, land-skill.template
skills/             vendored (grill-with-docs, to-spec, to-tickets, tdd, diagnosing-bugs, code-review, pr, retro)
github/             ISSUE_TEMPLATE/spec.yml, ticket.yml, labels.txt
checks/             factory-check-{elixir,node,python}
docs/               WORKFLOW, HERDR, SKILL-PINS
```

## Versioning

Tag releases (`v0.1.0` …); projects float on `main` or pin via
`install.sh --pin vX.Y.Z`. `factory update` explains refresh (delete-then-init
to force a file refresh — init never overwrites).

## Adding a language stack

Stacks are cheap by design — three touch points, no framework changes:

1. **Check adapter** `checks/factory-check-<name>` (executable bash).
   Contract (enforced by review, not code):
   - `cd` to the project root first; accept extra args and pass them to the test runner.
   - Stages in order: format check → lint/compile → tests (→ assets/build if any).
   - Exit non-zero on ANY failure; print `FACTORY-CHECK PASSED` only on success.
   - Concurrency: honor `FACTORY_SLOT` (1..N, set per dispatcher worker).
     Concurrent workers must not share mutable resources — offset test DB
     names, ports, and tmpdirs by slot (see the elixir adapter's
     `MIX_TEST_PARTITION` pattern and `docs/DISPATCHER.md`).
2. **Register it** in `bin/factory` `cmd_init`'s stack case (one line next to
   elixir/node/python) so `--stack <name>` installs the adapter.
3. **CI template** `github/workflows/ci-<name>.yml` (copy the generic one):
   toolchain setup matching local dev versions (formatter/linter output
   differs per toolchain minor — CI MUST match developers), service
   containers, then `./scripts/factory-check` and nothing else.
4. **Smoke it:** `install.sh --stack <name> --dir /tmp/smoke-<name>` must
   seed cleanly; run the adapter where its toolchain exists.

Prompts, skills, labels, dispatcher, and freeze mechanics are stack-agnostic
— nothing else changes. Bump `VERSION` and note the stack in this README.

## L3 dispatcher

`bin/factory-dispatch` drains `ready` tickets mechanically (implement →
verify → review → `accepted`), never landing. See `docs/DISPATCHER.md`.
Operator loop: `factory-dispatch once --dry-run` first, then cron/systemd
`once`, `status` to watch, `stop` to halt. Human owns `accepted` → land.

## Release train (multi-ticket)

`factory-dispatch release start [<slug>]` cuts an `integration/*` train from
`origin/dev`; review-accepted tickets auto-merge into it (clean merges
only, conflicts route to `blocked`), and a human reviews the whole train
(`prompts/review-integration.md`) before merging to `dev`. Details in
`docs/DISPATCHER.md`. Automation never touches `dev`/`main`.
