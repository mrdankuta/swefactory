# swefactory (Rust)

Single static `factory` binary — the software factory as a product.
Parity target: `pi-factory` @ `0343c74`.

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
```

`--json` is a global output mode: every command that produces output
emits machine-parseable JSON when it is set.

```sh
factory handoff 11 --signal READY_FOR_REVIEW --body HANDOFF.md
factory handoff 11 --signal BLOCKED            # posts the trailer alone
factory reconcile                               # read-only: open issues vs dev
factory start 11                                # worktree + branch + brief
factory --json reconcile                        # machine-parseable drift report
```

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
