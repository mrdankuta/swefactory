# swefactory (Rust)

Single static `factory` binary — the software factory as a product.
Parity target: `pi-factory` @ `0343c74`.

## Install

No Rust toolchain needed — pushing a version tag ships tarballs plus
shell/PowerShell installers and a Homebrew formula via cargo-dist
(mac, Windows, and musl-static Linux):

```sh
# Linux / macOS (one curl-pipe command):
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/mrdankuta/swefactory/releases/latest/download/factory-installer.sh | sh
```

```powershell
# Windows PowerShell (one pipe, no toolchain):
irm https://github.com/mrdankuta/swefactory/releases/latest/download/factory-installer.ps1 | iex
```

```sh
# …or via Homebrew from the published formula:
brew install https://github.com/mrdankuta/swefactory/releases/latest/download/factory.rb
```

Verify with `factory version` (or `factory --json version`).
Every tagged release smoke-installs all three paths on CI per platform
(see `.github/workflows/smoke.yml`).

## Build

No setup beyond the Rust toolchain:

```sh
cargo build --manifest-path swefactory/Cargo.toml
```

## Commands

```sh
factory version            # prints the crate version, e.g. 0.1.0
factory --json version     # {"version":"0.1.0"} (machine-parseable)
factory version --json     # same shape; --json is a global flag

# Spec-freeze protocol (shells out to `gh`, like the bash surface):
# canonical body hash = sha256 of the issue body + exactly one newline.
factory freeze 6           # post SPEC-APPROVED <hash> comment + spec-approved label
factory sync 6             # write factory/.cache/SPEC-6.md, verify vs freeze
factory verify-freeze 6    # exit 0 iff body still matches the freeze;
                           # else exit 1 with a diff of what changed

factory init [--stack rust|elixir|node|python] [--global-skills]
                         # seed prompts, skills, check adapter, issue
                         # templates, and labels into the current project
factory update [--pin vX.Y.Z]
                         # refresh from embedded templates (no-op when in place)
```

`--json` is a global output mode: every command that produces output
emits machine-parseable JSON when it is set. `init`/`update` emit
`{"stack", "installed", "kept", "warnings"}`.

## init / update (embedded templates)

The binary carries its templates compiled in (`swefactory/templates/`, via
`include_str!`) — no network, no checkout needed. Seeding never overwrites:
every target prints `install:` (written) or `keep:` (already there, bytes
untouched), so re-running `init`/`update` is a no-op when in place.

| `--stack` | `scripts/factory-check` adapter |
|-----------|----------------------------------|
| `rust` (default) | `cargo fmt --check` → `clippy -D warnings` → `cargo test` |
| `elixir`  | `mix format --check` → `mix compile --warnings-as-errors` → `mix test` |
| `node`    | `prettier --check` → `eslint` → `npm test` |
| `python`  | `ruff format --check` → `ruff check` → `pytest` |

Installed into the project: `.pi/prompts/*`, `.agents/skills/*`
(`--global-skills` targets `~/.agents/skills` instead), the stack's
check adapter, `.github/ISSUE_TEMPLATE/{spec,ticket}.yml`, the `AGENTS.md`
header merge, `GLOSSARY.md` + `docs/README.md` skeletons,
`factory/.cache/`, and `.factory-version`.

Divergence from bash (documented): the Rust binary *is* `factory`, so no
`scripts/factory` shim is installed; `update --pin` acknowledges the tag but
does not fetch or persist it (network refresh is out of scope at parity). Missing `gh`/`herdr`
or unauthenticated `gh` only warn (`WARN:`/`skip:` on stderr) — init
never fails for missing tooling; labels are ensured best-effort via `gh`.

## Check runner

`factory check` executes the check-adapter contract (format, then lint,
then test) as a subprocess. Adapters stay plain scripts; the runner only
discovers the adapter, forwards the slot, streams its output, and
propagates its exit code — so green keeps meaning green.

```sh
factory check                          # nearest scripts/factory-check upward
factory check --adapter ./my-adapter   # explicit adapter script
factory check --slot worker-1          # slot isolation (sets FACTORY_SLOT)
factory check -- arg1 arg2              # extra args forwarded to the adapter
factory --json check                   # same run + a JSON summary as the last line
```

Adapter resolution: `--adapter`, else `$FACTORY_CHECK_ADAPTER`, else the
nearest `scripts/factory-check` walking up from the current directory.
On success the adapter prints `FACTORY-CHECK PASSED`; any stage failure
fails the run with a non-zero exit and no marker.
```sh
factory handoff 11 --signal READY_FOR_REVIEW --body HANDOFF.md
factory handoff 11 --signal BLOCKED            # posts the trailer alone
factory reconcile                               # read-only: open issues vs dev
factory start 11                                # worktree + branch + brief
factory --json reconcile                        # machine-parseable drift report
```

## Dispatch loop

`factory dispatch` runs single scheduling passes that move a ready ticket
through implement, verify, and review. It never lands and never touches
`dev`/`main` — accepted tickets wait for a human land; worktrees are cut
from `HEAD` and removed afterwards.

```sh
factory dispatch once          # implement → verify → review one pass
factory dispatch once --dry-run # survey only: no worktrees, subprocesses, or writes
factory dispatch status        # attempts + worktrees + STOP state
factory dispatch stop          # halt the loop (later `once` exits 0)
factory dispatch resume        # restart the loop after a stop
factory dispatch self-test     # hermetic release-mechanics proof (no gh, no network, no tokens)
factory --json dispatch status # machine-parseable ledger
```

State lives in `factory/.state/` (`dispatch.json` attempt ledger,
`logs/` per-attempt structured logs, `worktrees/` isolated per-attempt
worktrees, `STOP` halt flag). Every attempt runs with its own
`FACTORY_SLOT` and private `TMPDIR`, forwarded to the implementer, the
check adapter, and the reviewer — concurrent workers share no databases,
ports, or temp dirs. Attempt statuses:
`impl_running → impl_done|impl_failed → review_running →
review_done|review_failed`. Knobs: `MAX_CONCURRENT=1`,
`MAX_ATTEMPTS=2`, `PI_BIN`/`FACTORY_PI_BIN` (implementer binary),
`FACTORY_CHECK_ADAPTER`, `FACTORY_STATE_DIR` (state root override).

All GitHub access shells out to the `gh` CLI (same as the bash parity
target); there is no API client code. `FACTORY_GH_BIN` / `FACTORY_GIT_BIN`
override the binaries under test. `reconcile` never writes: no issue
comments/edits and no pushes. `start` never touches GitHub state either —
it reads the ticket via `gh issue view` and creates only local
worktree/branch/brief state.

## Checks

```sh
scripts/factory-check      # fmt --check → clippy (-D warnings) → tests
```
