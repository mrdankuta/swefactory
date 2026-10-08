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
```

`--json` is a global output mode: every command that produces output
emits machine-parseable JSON when it is set.

## Checks

```sh
scripts/factory-check      # fmt --check → clippy (-D warnings) → tests
```
