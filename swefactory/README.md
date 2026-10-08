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

## Checks

```sh
scripts/factory-check      # fmt --check → clippy (-D warnings) → tests
```
