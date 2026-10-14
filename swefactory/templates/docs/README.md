# docs map

- `product/` — product notes, roadmaps.
- `architecture/` — system diagrams, integration notes.
- `adr/` — architecture decision records (one file per decision).
- `runbooks/` — operational procedures.

## Seed layout contracts

`factory init` seeds files in two classes. Nothing already seeded ever
moves; re-seeding and `factory doctor --fix` never overwrite a file you
have customized (existing files are reported as `keep:` and left
byte-identical).

### Integration contracts

Expected at their conventional paths by editors, agents, and hosts.
Keep them where they are; their *contents* are yours to edit.

| Path | Expected by |
|------|-------------|
| `AGENTS.md` (factory header block) | Agents and editors reading repo rules at the root |
| `GLOSSARY.md` | The header block above; agents resolving domain terms |
| `scripts/factory-check` | Hosts and CI running the format → lint → test gate |
| `.pi/prompts/*` | The Pi agent loading its `/plan`, `/build`, `/review`, `/handoff` prompts |
| `.agents/skills/*` (`~/.agents/skills/*` with `--global-skills`) | Agents discovering skills |
| `.github/ISSUE_TEMPLATE/spec.yml`, `ticket.yml` | GitHub issue forms |
| `docs/` (this map plus `product/`, `architecture/`, `adr/`, `runbooks/`) | The factory method's paper trail; editors browsing docs |
| `.gitignore` (`factory/.cache/` entry) | Git ignoring factory scratch state |

### Factory-owned

The factory's own footprint. Leave these to the tool.

| Path | Notes |
|------|-------|
| `factory/.cache/` | Gitignored snapshots (e.g. `factory sync` output); never committed |
| `factory/.state/` | Created at runtime by dispatch (attempt ledger, logs, worktrees) |
| `.factory-version` | Marker recording which factory release seeded the project; refreshed by `init`/`update` |

### Future-seeds rule

All future seeds land under the factory-owned directory (`factory/`).
No new top-level or conventional-path files will be added by seeding;
what exists today stays exactly where it is.
