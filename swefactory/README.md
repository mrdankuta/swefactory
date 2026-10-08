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

# Spec-freeze protocol (shells out to `gh`, like the bash surface):
# canonical body hash = sha256 of the issue body + exactly one newline.
factory freeze 6           # post SPEC-APPROVED <hash> comment + spec-approved label
factory sync 6             # write factory/.cache/SPEC-6.md, verify vs freeze
factory verify-freeze 6    # exit 0 iff body still matches the freeze;
                           # else exit 1 with a diff of what changed
```

`--json` is a global output mode: every command that produces output
emits machine-parseable JSON when it is set.

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
