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

## Checks

```sh
scripts/factory-check      # fmt --check → clippy (-D warnings) → tests
```
