use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(tag: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "node-adapter-{}-{}-{}",
        std::process::id(),
        id,
        tag
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn template_adapter() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/check-adapters/node")
}

/// Stage a fake JS project: copy the adapter template to
/// `<proj>/scripts/factory-check`, write the given files, and install
/// stub toolchains on PATH. Returns (proj dir, stub log).
fn stage_project(tag: &str, files: &[(&str, &str)], pm_stub: &str) -> (PathBuf, PathBuf) {
    let proj = scratch_dir(tag);
    let scripts = proj.join("scripts");
    fs::create_dir_all(&scripts).unwrap();
    let adapter_src = template_adapter();
    let adapter_dst = scripts.join("factory-check");
    fs::copy(&adapter_src, &adapter_dst).unwrap();
    fs::set_permissions(&adapter_dst, fs::Permissions::from_mode(0o755)).unwrap();

    for (rel, content) in files {
        let path = proj.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
    }

    let bin = proj.join("stubs");
    fs::create_dir_all(&bin).unwrap();
    let log = proj.join("stub.log");
    // Tool stubs: log invocation, fail when FAIL_TOOL matches the tool name.
    for tool in [
        "prettier",
        "eslint",
        "svelte-check",
        "tsc",
        "vitest",
        "jest",
        "playwright",
    ] {
        let stub = format!(
            "#!/bin/sh\nlog=\"$STUB_LOG\"\necho \"{tool} $@\" >> \"$log\"\nif [ \"${{FAIL_TOOL:-}}\" = \"{tool}\" ]; then exit 1; fi\nexit 0\n"
        );
        let path = bin.join(tool);
        fs::write(&path, stub).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    // Package-manager stub: logs, fails on FAIL_TOOL, and dispatches
    // `<pm> exec <tool> ...` to the matching tool stub (like npx/pnpm exec).
    let pm_template = "#!/bin/sh\nlog=\"$STUB_LOG\"\nme=\"@@PM@@\"\necho \"$me $@\" >> \"$log\"\nif [ \"${FAIL_TOOL:-}\" = \"$me\" ]; then exit 1; fi\nif [ \"$1\" = \"exec\" ]; then shift; tool=\"$1\"; shift\n  if [ \"${FAIL_TOOL:-}\" = \"$tool\" ]; then echo \"$tool $@\" >> \"$log\"; exit 1; fi\n  echo \"$tool $@\" >> \"$log\"; exit 0; fi\nif [ \"$1\" = \"test\" ]; then exit 0; fi\nif [ \"$me\" = \"npx\" ] || [ \"$me\" = \"bunx\" ]; then tool=\"$1\"; shift\n  if [ \"${FAIL_TOOL:-}\" = \"$tool\" ]; then echo \"$tool $@\" >> \"$log\"; exit 1; fi\n  echo \"$tool $@\" >> \"$log\"; exit 0; fi\nexit 0\n";
    for pm_bin in ["npm", "npx", "pnpm", "yarn", "bun", "bunx"] {
        let path = bin.join(pm_bin);
        let body = pm_template.replace("@@PM@@", pm_bin);
        let _ = pm_stub;
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    (proj, log)
}

fn run_adapter(proj: &Path, args: &[&str], extra_envs: &[(&str, &str)]) -> Output {
    let adapter = proj.join("scripts/factory-check");
    let bin = proj.join("stubs");
    let mut cmd = std::process::Command::new(&adapter);
    cmd.args(args).current_dir(proj);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    cmd.env("PATH", path);
    cmd.env("STUB_LOG", proj.join("stub.log"));
    for (k, v) in extra_envs {
        cmd.env(k, v);
    }
    cmd.env_remove("FACTORY_CHECK_ADAPTER");
    cmd.output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn svelte_convex_files() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "package.json",
            r#"{"name":"svelte-convex-app","private":true,"scripts":{"test":"vitest run"},"devDependencies":{"svelte":"4.2.0","typescript":"5.4.0","vitest":"1.4.0"}}"#,
        ),
        ("pnpm-lock.yaml", "lockfileVersion: 9\n"),
        ("svelte.config.js", "export default {};\n"),
        ("tsconfig.json", "{\"compilerOptions\":{\"strict\":true}}\n"),
        (".prettierrc", "{}\n"),
    ]
}

#[test]
fn svelte_convex_fixture_runs_green_with_skip_reasons() {
    let (proj, log) = stage_project("svelte-convex", &svelte_convex_files(), "");
    let out = run_adapter(&proj, &[], &[]);
    let text = stdout(&out);
    assert!(
        out.status.success(),
        "Svelte/Convex-like fixture must exit 0, got {} stdout:\n{text} stderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("FACTORY-CHECK PASSED"),
        "success marker must appear on green run, got:\n{text}"
    );
    // No eslint config: lint skips with a reason instead of failing.
    assert!(
        text.to_lowercase().contains("skipping lint") || text.to_lowercase().contains("skip"),
        "absent linter must print a skip reason, got:\n{text}"
    );
    // Lockfile present: pnpm resolves the test runner.
    let logged = fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains("pnpm"),
        "pnpm-lock.yaml must select pnpm, stub log was:\n{logged}"
    );
    // Type configs present: a typecheck ran (svelte-check preferred).
    assert!(
        logged.contains("svelte-check"),
        "Svelte project must prefer svelte-check, stub log was:\n{logged}"
    );
    // Formatter config present: prettier ran.
    assert!(
        logged.contains("prettier"),
        "formatter config present must run prettier, stub log was:\n{logged}"
    );
}

#[test]
fn real_check_failures_exit_nonzero_and_omit_marker() {
    for fail_tool in ["prettier", "svelte-check", "pnpm"] {
        let (proj, _log) = stage_project("failure", &svelte_convex_files(), "");
        let out = run_adapter(&proj, &[], &[("FAIL_TOOL", fail_tool)]);
        assert!(
            !out.status.success(),
            "FAIL_TOOL={fail_tool} must exit nonzero"
        );
        assert!(
            !stdout(&out).contains("FACTORY-CHECK PASSED"),
            "FAIL_TOOL={fail_tool}: success marker must be omitted on failure"
        );
    }
}

#[test]
fn empty_project_reports_nothing_to_check_and_succeeds() {
    let (proj, _log) = stage_project("empty", &[], "");
    fs::remove_file(proj.join("package.json")).unwrap_or(());
    // stage_project writes no package.json here (files list empty), so the
    // adapter must take the no-op path.
    let out = run_adapter(&proj, &[], &[]);
    let text = stdout(&out);
    assert!(
        out.status.success(),
        "empty project must exit 0, got {} output:\n{text}",
        out.status
    );
    assert!(
        text.contains("nothing to check"),
        "empty project must report nothing to check, got:\n{text}"
    );
    assert!(
        text.contains("FACTORY-CHECK PASSED"),
        "empty project must still print the success marker, got:\n{text}"
    );
}

#[test]
fn contract_order_and_success_marker_unchanged() {
    let files = vec![
        (
            "package.json",
            r#"{"name":"full","scripts":{"test":"node test.js"},"devDependencies":{"typescript":"5.4.0"},"prettier":{},"eslintConfig":{}}"#,
        ),
        ("package-lock.json", "{\"lockfileVersion\":3}\n"),
        ("tsconfig.json", "{\"compilerOptions\":{\"strict\":true}}\n"),
    ];
    let (proj, _log) = stage_project("order", &files, "");
    let out = run_adapter(&proj, &[], &[]);
    let text = stdout(&out);
    assert!(
        out.status.success(),
        "full fixture must exit 0, got:\n{text}"
    );
    let fmt = text
        .find("prettier")
        .or_else(|| text.to_lowercase().find("format"));
    let lint = text
        .find("eslint")
        .or_else(|| text.to_lowercase().find("lint"));
    let test = text.to_lowercase().rfind("test");
    let marker = text.find("FACTORY-CHECK PASSED");
    match (fmt, lint, test, marker) {
        (Some(f), Some(l), Some(t), Some(m)) => {
            assert!(
                f < l && l < t && t < m,
                "contract order format→lint→test→marker broken:\n{text}"
            );
        }
        _ => panic!("all contract stages and the marker must appear, got:\n{text}"),
    }
    assert_eq!(
        text.trim_end().rsplit('\n').next().unwrap().trim(),
        "FACTORY-CHECK PASSED",
        "success marker must be the last line, got:\n{text}"
    );
}
