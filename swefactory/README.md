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

## Checks

```sh
scripts/factory-check      # fmt --check → clippy (-D warnings) → tests
```
