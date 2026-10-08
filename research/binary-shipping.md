# Research: how to ship one static binary (for swefactory)

Context: wayfinder ticket mrdankuta/swefactory#5. Question: how a Rust `factory`
CLI ships as one static binary (mac/win/linux + installer/update story), so the
divergence-scope ticket can decide distribution without guessing.
Researched Oct 2026. Throwaway branch — never merged, no PR.

## Recommendation

**cargo-dist (`dist`, axodotdev, v0.33.0, actively maintained) as the release
mechanism**, because it is the continuity option: it generates the GitHub
release CI (`cargo dist init --ci=github`), builds per-platform tarballs plus
**shell + PowerShell installers** — i.e. the same `curl | bash` shape pi-factory's
`install.sh` already uses — plus npm/Homebrew-publish artifacts and
machine-readable manifests. Pair with `cargo-release` for tag-driven cuts.

- Targets: `x86_64`/`aarch64-apple-darwin`, `x86_64-pc-windows-msvc`,
  `x86_64`/`aarch64-unknown-linux-musl` (static). musl gives the "one static
  binary" story on Linux with no glibc floor to negotiate.
- `cargo binstall` reuses cargo-dist manifests, so Rust users get fast installs free.
- **Self-update: defer.** axoupdater (same org: standalone updater + embeddable
  Rust library) is the in-app-update answer, but it is a v1+ concern — parity
  needs install, not auto-update.

## Alternatives considered

- `cargo install` from crates.io: fine as a fallback, wrong as the primary —
  compiles from source (slow, toolchain-dependent), hostile to Builder Bea.
- brew tap / apt / npm: cargo-dist can emit Homebrew formula + npm package;
  treat them as generated outputs of the same pipeline, not separate work.
- Manual GitHub-release tarballs: what cargo-dist automates away; no reason to hand-roll.
- equinox: older self-update option, superseded in practice by axoupdater.

## Suggested parity-scope input (for the divergence ticket)

Distribution at parity = cargo-dist pipeline producing
tarballs + shell/PowerShell installers + Homebrew formula, triggered by tag,
with the install UX kept curl-pipe-shaped. Everything else (self-update,
package-manager native packaging, signed attestations) is post-parity.
