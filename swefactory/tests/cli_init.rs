use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Fresh scratch dir for one test (unique per test, no extra deps).
fn scratch_dir(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "factory-init-test-{}-{}-{}",
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
fn empty_path() -> (PathBuf, String) {
    let bin = scratch_dir("nobin");
    (bin.clone(), bin.to_string_lossy().into_owned())
}

/// PATH with a stub `gh` that logs argv to a file and exits 0.
fn stub_gh_path(log: &Path) -> String {
    let bin = scratch_dir("stubbin");
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> {}\nexit 0\n",
        log.to_string_lossy()
    );
    let gh = bin.join("gh");
    fs::write(&gh, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin.to_string_lossy().into_owned()
}

fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(&d).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().to_path_buf();
                out.push((rel, fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn fresh_init_installs_prompts_skills_adapter_and_templates() {
    let dir = scratch_dir("fresh");
    let (_bin, path) = empty_path();
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert().success();

    // prompts
    for p in ["plan", "build", "review", "handoff"] {
        let f = dir.join(".pi/prompts").join(format!("{p}.md"));
        assert!(f.is_file(), "missing .pi/prompts/{p}.md");
    }
    // skills (vendored set incl. land placeholder source)
    for s in ["tdd", "to-spec", "land", "code-review"] {
        let f = dir.join(".agents/skills").join(s).join("SKILL.md");
        assert!(f.is_file(), "missing skill {s}/SKILL.md");
    }
    // check adapter (default stack: rust)
    let adapter = dir.join("scripts/factory-check");
    assert!(adapter.is_file(), "missing scripts/factory-check");
    let body = fs::read_to_string(&adapter).unwrap();
    assert!(
        body.contains("cargo"),
        "default adapter must be the rust one"
    );
    assert!(body.contains("FACTORY-CHECK PASSED"));
    // issue templates
    assert!(dir.join(".github/ISSUE_TEMPLATE/spec.yml").is_file());
    assert!(dir.join(".github/ISSUE_TEMPLATE/ticket.yml").is_file());
    let spec = fs::read_to_string(dir.join(".github/ISSUE_TEMPLATE/spec.yml")).unwrap();
    assert!(spec.contains("name: Spec"));
    // AGENTS.md header + glossary + version marker
    let agents = fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(agents.contains("factory:header:start"));
    assert!(dir.join("GLOSSARY.md").is_file());
    assert!(dir.join(".factory-version").is_file());
}

#[test]
fn rerunning_init_changes_nothing() {
    let dir = scratch_dir("idempotent");
    let (_bin, path) = empty_path();
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert().success();
    let before = snapshot(&dir);

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("keep:"));
    let after = snapshot(&dir);
    assert_eq!(before, after, "re-running init must change nothing");
}

#[test]
fn update_is_idempotent_noop() {
    let dir = scratch_dir("update");
    let (_bin, path) = empty_path();
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert().success();
    let before = snapshot(&dir);

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).args(["update"]);
    cmd.assert().success();
    let after = snapshot(&dir);
    assert_eq!(before, after, "update must change nothing when in place");
}

#[test]
fn stack_selects_matching_check_adapter() {
    for (stack, marker) in [
        ("rust", "cargo"),
        ("elixir", "mix"),
        ("node", "npm"),
        ("python", "pytest"),
    ] {
        let dir = scratch_dir(stack);
        let (_bin, path) = empty_path();
        let mut cmd = Command::cargo_bin("factory").unwrap();
        cmd.current_dir(&dir)
            .env("PATH", &path)
            .args(["init", "--stack", stack]);
        cmd.assert().success();
        let body =
            fs::read_to_string(dir.join("scripts/factory-check")).expect("adapter installed");
        assert!(
            body.contains(marker),
            "--stack {stack} adapter must mention {marker}"
        );
    }
}

#[test]
fn missing_gh_warns_instead_of_failing() {
    let dir = scratch_dir("nogh");
    let (_bin, path) = empty_path();
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert()
        .success()
        .stderr(predicates::str::contains("WARN").and(predicates::str::contains("gh")));
}

#[test]
fn existing_files_are_never_clobbered() {
    let dir = scratch_dir("noclobber");
    let (_bin, path) = empty_path();
    let custom = "# my custom plan prompt — hands off\n";
    fs::create_dir_all(dir.join(".pi/prompts")).unwrap();
    fs::write(dir.join(".pi/prompts/plan.md"), custom).unwrap();

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert().success();

    let kept = fs::read_to_string(dir.join(".pi/prompts/plan.md")).unwrap();
    assert_eq!(kept, custom, "existing files must never be overwritten");
}

#[test]
fn init_attempts_labels_via_gh_best_effort() {
    let dir = scratch_dir("labels");
    let log = dir.join("gh-argv.log");
    let path = stub_gh_path(&log);

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert().success();

    let logged = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        logged.contains("label"),
        "init should ensure labels via gh, got: {logged}"
    );
}

#[test]
fn json_init_is_machine_parseable() {
    let dir = scratch_dir("json");
    let (_bin, path) = empty_path();
    for args in [&["--json", "init"][..], &["init", "--json"][..]] {
        // fresh dir per spelling so both assert the install path
        let d = dir.join(if args[0] == "--json" { "a" } else { "b" });
        fs::create_dir_all(&d).unwrap();
        let mut cmd = Command::cargo_bin("factory").unwrap();
        cmd.current_dir(&d).env("PATH", &path).args(args);
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "args {args:?} exited with {}",
            output.status
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("args {args:?} stdout is not JSON: {e}\n{stdout}"));
        assert!(
            parsed.get("installed").is_some(),
            "args {args:?} JSON must have installed key: {parsed}"
        );
        assert!(
            parsed.get("kept").is_some(),
            "args {args:?} JSON must have kept key: {parsed}"
        );
    }
}

#[test]
fn update_pin_is_acknowledged_not_recorded() {
    let dir = scratch_dir("pin");
    let (_bin, path) = empty_path();
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir)
        .env("PATH", &path)
        .args(["update", "--pin", "v9.9.9"]);
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("v9.9.9"),
        "pin output must name the tag, got:\n{text}"
    );
    assert!(
        !text.contains("recorded"),
        "pin must not claim persistence it does not do, got:\n{text}"
    );
}

#[test]
fn update_detects_installed_stack() {
    let dir = scratch_dir("update-stack");
    let (_bin, path) = empty_path();
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir)
        .env("PATH", &path)
        .args(["init", "--stack", "node"]);
    cmd.assert().success();
    let before = fs::read(dir.join("scripts/factory-check")).unwrap();

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir)
        .env("PATH", &path)
        .args(["update", "--json"]);
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(
        parsed.get("stack").and_then(|s| s.as_str()),
        Some("node"),
        "update must report the detected stack, got: {parsed}"
    );
    assert_eq!(
        fs::read(dir.join("scripts/factory-check")).unwrap(),
        before,
        "update must not clobber the installed adapter"
    );
}

#[test]
fn init_does_not_chmod_kept_adapter() {
    let dir = scratch_dir("keepmode");
    let (_bin, path) = empty_path();
    fs::create_dir_all(dir.join("scripts")).unwrap();
    fs::write(dir.join("scripts/factory-check"), "# custom adapter\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            dir.join("scripts/factory-check"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
    }

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("keep: scripts/factory-check"),
        "kept adapter must be reported as kept, got:\n{stdout}"
    );
    assert_eq!(
        fs::read_to_string(dir.join("scripts/factory-check")).unwrap(),
        "# custom adapter\n",
        "kept adapter content must be untouched"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.join("scripts/factory-check"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644, "kept adapter permissions must stay untouched");
    }
}

#[test]
fn global_skills_installs_to_home() {
    let dir = scratch_dir("globalskills");
    let home = scratch_dir("fakehome");
    let (_bin, path) = empty_path();
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir)
        .env("PATH", &path)
        .env("HOME", &home)
        .args(["init", "--global-skills"]);
    cmd.assert().success();

    assert!(
        home.join(".agents/skills/tdd/SKILL.md").is_file(),
        "global skills must land under $HOME"
    );
    assert!(
        !dir.join(".agents/skills").exists(),
        "project skills dir must stay absent with --global-skills"
    );
}

#[test]
fn agents_header_merges_into_existing_file() {
    let dir = scratch_dir("agentsmerge");
    let (_bin, path) = empty_path();
    fs::write(dir.join("AGENTS.md"), "# my project notes\n").unwrap();

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert().success();

    let body = fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(
        body.contains("factory:header:start"),
        "header must be prepended, got:\n{body}"
    );
    assert!(
        body.contains("# my project notes"),
        "existing body must be preserved, got:\n{body}"
    );
}

#[test]
fn gitignore_gains_cache_entry() {
    let dir = scratch_dir("gitignore-append");
    let (_bin, path) = empty_path();
    fs::write(dir.join(".gitignore"), "node_modules/\n").unwrap();

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert().success();

    let body = fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(
        body.contains("node_modules/"),
        "existing entries must be preserved, got:\n{body}"
    );
    assert!(
        body.contains("factory/.cache/"),
        "cache entry must be appended, got:\n{body}"
    );
}

#[test]
fn gitignore_is_created_when_absent() {
    let dir = scratch_dir("gitignore-create");
    let (_bin, path) = empty_path();
    assert!(!dir.join(".gitignore").exists());

    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.current_dir(&dir).env("PATH", &path).arg("init");
    cmd.assert().success();

    let body = fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(
        body.contains("factory/.cache/"),
        "fresh init must create .gitignore with the cache entry, got:\n{body}"
    );
}
