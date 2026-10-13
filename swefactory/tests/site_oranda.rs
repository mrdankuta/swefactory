//! Download site (ticket #36): oranda renders the Pages download home from
//! cargo-dist release data.
//!
//! Seams under test (agreed config surfaces, not behaviour we can execute here):
//! 1. `oranda.json` site config — catches a missing/unwired site definition.
//!    Misses whether GitHub Pages actually serves it (needs the human
//!    Settings → Pages flip, flagged in the handoff).
//! 2. `swefactory/Cargo.toml` `homepage` — the dist-metadata surface that
//!    points installers and release bodies at the download home.
//! 3. `swefactory/CHANGELOG.md` — the dist autodetect surface that lands the
//!    site link in generated release notes.
//! 4. `.github/workflows/site.yml` — catches an unwired Pages deploy.
//!    Misses real runner execution.
//! 5. README download-home links — catches docs pointing only at raw release
//!    assets instead of the site.
//!
//! What these tests miss by construction (no network, no runners here) is
//! covered by `oranda build` locally and the site workflow itself.

use std::fs;
use std::path::PathBuf;

const PAGES_URL: &str = "https://mrdankuta.github.io/swefactory/";
const REPO_URL: &str = "https://github.com/mrdankuta/swefactory";

fn repo_root() -> PathBuf {
    // The crate root is CARGO_MANIFEST_DIR, the repo root is its parent.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .expect("crate dir has a parent (repo root)")
        .to_path_buf()
}

fn read_repo(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("ticket #36 requires {rel} to exist: {e}"))
}

#[test]
fn oranda_config_points_at_dist_releases() {
    let raw = read_repo("oranda.json");
    let config: serde_json::Value =
        serde_json::from_str(&raw).expect("oranda.json must parse as JSON");

    // The site must know which GitHub repo to read dist-manifest.json from.
    assert_eq!(
        config
            .pointer("/project/repository")
            .and_then(|v| v.as_str()),
        Some(REPO_URL),
        "oranda.json project.repository must be the swefactory repo"
    );
    // Project Pages serve under /<repo>/, so the build needs the prefix.
    assert_eq!(
        config
            .pointer("/build/path_prefix")
            .and_then(|v| v.as_str()),
        Some("swefactory"),
        "oranda.json build.path_prefix must be the repo name"
    );
    // cargo-dist integration is what renders installer buttons from releases.
    assert_eq!(
        config
            .pointer("/components/artifacts/cargo_dist")
            .and_then(|v| v.as_bool()),
        Some(true),
        "oranda.json must enable the cargo-dist artifacts source"
    );
}

#[test]
fn dist_homepage_is_the_download_home() {
    let cargo_toml = read_repo("swefactory/Cargo.toml");
    assert!(
        cargo_toml.contains(PAGES_URL),
        "swefactory/Cargo.toml homepage must point at the Pages download home"
    );
    // dist requires a stable repository URL for hosting/announcing.
    assert!(
        cargo_toml.contains(REPO_URL),
        "swefactory/Cargo.toml repository must stay the GitHub repo"
    );
}

#[test]
fn changelog_links_the_download_home() {
    // cargo-dist autodetects swefactory/CHANGELOG.md and prepends the
    // matching section to generated release notes.
    let changelog = read_repo("swefactory/CHANGELOG.md");
    assert!(
        changelog.contains(PAGES_URL),
        "swefactory/CHANGELOG.md must link the Pages download home"
    );
}

#[test]
fn site_workflow_builds_and_deploys_pages() {
    let workflow = read_repo(".github/workflows/site.yml");
    assert!(
        workflow.contains("oranda build"),
        "site workflow must build the oranda site"
    );
    assert!(
        workflow.contains("deploy-pages"),
        "site workflow must deploy to GitHub Pages"
    );
    // Token-created releases never fire `release: published` (same reason
    // smoke.yml chains off the Release workflow instead).
    assert!(
        workflow.contains("workflow_run") || workflow.contains("workflow_dispatch"),
        "site workflow must trigger off the Release workflow or manual dispatch"
    );
}

#[test]
fn readmes_link_the_download_home() {
    for readme in ["README.md", "swefactory/README.md"] {
        let body = read_repo(readme);
        assert!(
            body.contains(PAGES_URL),
            "{readme} must link the Pages site as the download home"
        );
    }
}
