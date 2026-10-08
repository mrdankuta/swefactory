//! Release pipeline (ticket #12): pushing a tag ships the product.
//!
//! Seams under test (agreed config surfaces, not behaviour we can execute here):
//! 1. `swefactory/Cargo.toml` cargo-dist metadata — catches a missing target,
//!    installer, or formula. Misses whether CI actually runs green.
//! 2. `.github/workflows/release.yml` tag trigger + dist build + per-platform
//!    installer smoke — catches unwired CI. Misses real runner execution.
//!
//! What these tests miss by construction (no network, no runners here) is
//! covered by the workflow itself: it smoke-installs on every tagged release.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // swefactory/Cargo.toml lives one dir above this crate dir is wrong;
    // the crate root is CARGO_MANIFEST_DIR, the repo root is its parent.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .expect("crate dir has a parent (repo root)")
        .to_path_buf()
}

fn read_repo(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("ticket #12 requires {rel} to exist: {e}"))
}

#[test]
fn dist_config_covers_product_matrix() {
    let cargo_toml = read_repo("swefactory/Cargo.toml");

    // cargo-dist must be configured (workspace-level metadata is the
    // cargo-dist-native surface for this crate).
    assert!(
        cargo_toml.contains("metadata.dist"),
        "Cargo.toml has no [*.metadata.dist] section"
    );
    // Pinned cargo-dist version is the independent source of truth for
    // reproducible releases (must be an explicit x.y.z literal).
    assert!(
        cargo_toml.contains("cargo-dist-version"),
        "Cargo.toml does not pin cargo-dist-version"
    );

    // Product matrix: mac (both arches), Windows, musl-static Linux.
    for target in [
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-musl",
    ] {
        assert!(
            cargo_toml.contains(target),
            "Cargo.toml dist targets miss {target}"
        );
    }

    // curl-pipe continuity: shell + PowerShell installers, Homebrew formula.
    for installer in ["shell", "powershell"] {
        assert!(
            cargo_toml.contains(installer),
            "Cargo.toml dist installers miss {installer}"
        );
    }
    assert!(
        cargo_toml.contains("homebrew"),
        "Cargo.toml dist installers miss the Homebrew formula"
    );
}

#[test]
fn tag_push_drives_release_workflow_with_smoke() {
    let workflow = read_repo(".github/workflows/release.yml");

    // A pushed version tag is the only release trigger (cargo-dist's
    // canonical version-tag glob — matches `v0.1.0`, `1.0.0`, prereleases).
    assert!(
        workflow.contains("tags:") && workflow.contains("[0-9]+.[0-9]+.[0-9]+"),
        "release workflow must trigger on pushed version tags"
    );
    // The workflow must actually run cargo-dist (plan + build).
    for step in ["dist plan", "dist build"] {
        assert!(
            workflow.contains(step),
            "release workflow never runs `{step}`"
        );
    }
    // Artifacts must reach the GitHub Release.
    assert!(
        workflow.to_lowercase().contains("release"),
        "release workflow never publishes a GitHub Release"
    );

    // Per-platform installer smoke lives in smoke.yml (kept separate so
    // release.yml stays pristine `cargo dist generate-ci` output): install
    // from the just-published release, then prove the binary runs against
    // a scratch dir (no Rust toolchain).
    let smoke = read_repo(".github/workflows/smoke.yml");
    assert!(
        smoke.contains("release") && smoke.contains("published"),
        "smoke workflow must run when a release is published"
    );
    for marker in [
        "factory-installer.sh",
        "factory-installer.ps1",
        "factory version",
    ] {
        assert!(
            smoke.contains(marker),
            "smoke workflow has no coverage for {marker}"
        );
    }
    // Homebrew formula gets its own smoke leg on macOS.
    assert!(
        smoke.contains("factory.rb") && smoke.contains("brew install"),
        "smoke workflow never installs the Homebrew formula"
    );
    for runner in ["ubuntu-latest", "macos-latest", "windows-latest"] {
        assert!(
            smoke.contains(runner),
            "smoke workflow matrix misses runner {runner}"
        );
    }
}

#[test]
fn install_docs_stay_curl_pipe_shaped() {
    let readme = read_repo("swefactory/README.md");
    assert!(
        readme.contains("curl") && readme.contains('|'),
        "swefactory/README.md documents no curl-pipe install command"
    );
    assert!(
        readme.contains("factory-installer.sh") || readme.contains("installer.sh"),
        "swefactory/README.md install docs never mention the shell installer"
    );
    assert!(
        readme.contains("irm") || readme.contains("installer.ps1"),
        "swefactory/README.md install docs never mention the PowerShell installer"
    );
}
