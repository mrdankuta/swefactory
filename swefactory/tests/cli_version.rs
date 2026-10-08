use assert_cmd::Command;

/// Crate version (compile-time) is the independent source of truth:
/// `factory version` must print exactly this.

#[test]
fn version_prints_crate_version() {
    let expected = env!("CARGO_PKG_VERSION");
    let mut cmd = Command::cargo_bin("factory").unwrap();
    cmd.arg("version");
    cmd.assert()
        .success()
        .stdout(predicates::str::contains(expected));
}

#[test]
fn json_version_is_machine_parseable() {
    let expected = env!("CARGO_PKG_VERSION");
    for args in [&["--json", "version"][..], &["version", "--json"][..]] {
        let mut cmd = Command::cargo_bin("factory").unwrap();
        cmd.args(args);
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "args {args:?} exited with {}",
            output.status
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("args {args:?} stdout is not JSON: {e}\n{stdout}"));
        assert_eq!(
            parsed.get("version").and_then(|v| v.as_str()),
            Some(expected),
            "args {args:?} JSON missing matching version field: {parsed}"
        );
    }
}

#[test]
fn json_flag_before_subcommand_works() {
    // `--json` is global: `factory --json version` must equal
    // `factory version --json` in shape.
    let mut before = Command::cargo_bin("factory").unwrap();
    before.args(["--json", "version"]);
    let before_out = String::from_utf8(before.output().unwrap().stdout).unwrap();

    let mut after = Command::cargo_bin("factory").unwrap();
    after.args(["version", "--json"]);
    let after_out = String::from_utf8(after.output().unwrap().stdout).unwrap();

    let before_json: serde_json::Value = serde_json::from_str(&before_out).unwrap();
    let after_json: serde_json::Value = serde_json::from_str(&after_out).unwrap();
    assert_eq!(before_json, after_json);
}
