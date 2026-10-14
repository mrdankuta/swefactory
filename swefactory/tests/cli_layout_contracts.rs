//! Ticket #48 (spec #45): layout contracts + predecessor-text removal.
//!
//! Regression proof that seeding (`init`/`update`) and repair
//! (`doctor --fix`, same seeding path) never read from the retired
//! predecessor tree, so it can be archived without breaking anything.
//! Also pins the published contract table (integration contracts vs
//! factory-owned, plus the future-seeds rule) and the never-move /
//! never-overwrite guarantees.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Distinctive bytes planted in the decoy predecessor tree. If seeding or
/// repair ever read that tree, these bytes leak into seeded output.
const DECOY_MARKER: &str = "PREDECESSOR-DECOY-9f3c1a-do-not-seed";

fn scratch_dir(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "factory-layout-contracts-{}-{}-{}",
        name,
        std::process::id(),
        n
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// PATH pointing at an empty dir: `gh`/`herdr` are "missing".
fn empty_path() -> String {
    let bin = scratch_dir("nobin");
    bin.to_string_lossy().into_owned()
}

/// PATH with a stub `gh` that always exits 0 (best-effort labels/auth pass).
fn stub_gh_path() -> String {
    let bin = scratch_dir("stubbin");
    let gh = bin.join("gh");
    fs::write(&gh, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    }
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn run_init(dir: &Path, path: &str) {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(dir).env("PATH", path).arg("init");
    cmd.assert().success();
}

/// Repair path shares init's seeding function; outcome is asserted on files
/// (probes around the repair may still fail for missing tooling, so the
/// exit code is intentionally not asserted here).
fn run_doctor_fix(dir: &Path, path: &str) {
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(dir)
        .env("PATH", path)
        .args(["doctor", "--fix"]);
    let _ = cmd.output().unwrap().status;
}

/// Every file the seed step owns, at the exact relative path it must keep.
/// Nothing seeded today moves (spec #45 implementation decision).
const SEEDED_FILES: &[&str] = &[
    ".pi/prompts/plan.md",
    ".pi/prompts/build.md",
    ".pi/prompts/review.md",
    ".pi/prompts/handoff.md",
    ".agents/skills/tdd/SKILL.md",
    ".agents/skills/to-spec/SKILL.md",
    ".agents/skills/to-tickets/SKILL.md",
    ".agents/skills/code-review/SKILL.md",
    ".agents/skills/land/SKILL.md",
    "scripts/factory-check",
    "AGENTS.md",
    "GLOSSARY.md",
    "docs/README.md",
    ".github/ISSUE_TEMPLATE/spec.yml",
    ".github/ISSUE_TEMPLATE/ticket.yml",
    ".factory-version",
    ".gitignore",
];

/// Collect all files under `dir`, skipping runtime state and the decoy tree.
fn seeded_bytes(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(&d).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            let rel = p.strip_prefix(dir).unwrap().to_path_buf();
            if rel.starts_with("pi-factory") || rel.starts_with("factory/.cache") {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((rel, fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

fn assert_no_predecessor_bytes(dir: &Path, stage: &str) {
    for (rel, bytes) in seeded_bytes(dir) {
        let text = String::from_utf8_lossy(&bytes).to_lowercase();
        assert!(
            !text.contains("pi-factory"),
            "{stage}: {} references the retired predecessor project",
            rel.display()
        );
        assert!(
            !bytes
                .windows(DECOY_MARKER.len())
                .any(|w| w == DECOY_MARKER.as_bytes()),
            "{stage}: {} leaked bytes from the predecessor tree",
            rel.display()
        );
    }
}

/// Plant a decoy predecessor tree inside the sandbox: if seeding or repair
/// ever read it, the marker bytes show up in seeded output.
fn plant_decoy_predecessor(dir: &Path) {
    let root = dir.join("pi-factory");
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/HERDR.md"), "# decoy herdr notes\n").unwrap();
    fs::write(root.join("DECOY-MARKER.md"), format!("{DECOY_MARKER}\n")).unwrap();
}

#[test]
fn seeding_never_touches_the_predecessor_tree() {
    let dir = scratch_dir("no-predecessor-read");
    let path = empty_path();
    plant_decoy_predecessor(&dir);

    run_init(&dir, &path);
    assert_no_predecessor_bytes(&dir, "init");

    // Repair re-seeds missing files through the same path: remove two and
    // prove the repair also ignores the predecessor tree.
    fs::remove_file(dir.join(".pi/prompts/plan.md")).unwrap();
    fs::remove_file(dir.join("scripts/factory-check")).unwrap();
    run_doctor_fix(&dir, &stub_gh_path());
    assert!(dir.join(".pi/prompts/plan.md").is_file());
    assert!(dir.join("scripts/factory-check").is_file());
    assert_no_predecessor_bytes(&dir, "doctor --fix");

    // The predecessor tree itself is untouched (archival is out of scope).
    assert!(dir.join("pi-factory/DECOY-MARKER.md").is_file());
}

#[test]
fn contract_table_is_published_with_future_seeds_rule() {
    let dir = scratch_dir("contract-table");
    let path = empty_path();
    run_init(&dir, &path);

    let docs = fs::read_to_string(dir.join("docs/README.md")).unwrap();
    for probe in [
        "Integration contracts",
        "Factory-owned",
        "future",
        "factory/",
        "scripts/factory-check",
        "AGENTS.md",
    ] {
        assert!(
            docs.contains(probe),
            "seeded docs/README.md must document {probe:?}, got:\n{docs}"
        );
    }
    assert!(
        !docs.to_lowercase().contains("pi-factory"),
        "contract table must not reference the predecessor, got:\n{docs}"
    );
}

#[test]
fn no_seeded_path_moved_and_custom_files_survive() {
    let dir = scratch_dir("no-moves");
    let path = empty_path();
    run_init(&dir, &path);

    for rel in SEEDED_FILES {
        assert!(
            dir.join(rel).is_file(),
            "seeded path must still exist at its conventional location: {rel}"
        );
    }

    // Customized files are never overwritten, by init or by repair.
    let custom = "# my custom plan prompt — hands off\n";
    fs::write(dir.join(".pi/prompts/plan.md"), custom).unwrap();
    run_init(&dir, &path);
    run_doctor_fix(&dir, &stub_gh_path());
    assert_eq!(
        fs::read_to_string(dir.join(".pi/prompts/plan.md")).unwrap(),
        custom,
        "customized seeded files must never be overwritten"
    );
}
