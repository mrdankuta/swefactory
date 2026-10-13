//! Ticket #28 (spec #27 prefactor): command logic callable as library
//! functions; CLI output byte-identical before/after extraction.
//!
//! Seams under test:
//! - lib function API (`factory_lib::version_text`, `web_health_response`):
//!   catches handler-callability (what future Axum handlers will call).
//! - CLI subprocess stdout bytes: catches byte-identical regressions.

use assert_cmd::Command;

/// CLI `factory version` stdout must equal the library function output.
#[test]
fn cli_version_matches_library() {
    let lib_text = factory_lib::version_text(false);
    let mut cmd = Command::cargo_bin("factory").expect("factory binary builds");
    cmd.arg("version");
    let out = cmd.output().expect("run factory version");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        lib_text,
        "CLI `factory version` stdout must be byte-identical to factory_lib::version_text(false)"
    );
}

/// CLI `factory version --json` stdout must equal the library JSON output.
#[test]
fn cli_version_json_matches_library() {
    let lib_text = factory_lib::version_text(true);
    let mut cmd = Command::cargo_bin("factory").expect("factory binary builds");
    cmd.args(["--json", "version"]);
    let out = cmd.output().expect("run factory --json version");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        lib_text,
        "CLI `factory --json version` stdout must be byte-identical to factory_lib::version_text(true)"
    );
}

/// The web dependencies must be linked: a handler-shaped function returns
/// the health fragment without starting a server or adding CLI surface.
#[test]
fn web_health_fragment_renders_version() {
    let html = factory_lib::web_health_response();
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        html.contains(version),
        "health fragment must carry the binary version, got: {html}"
    );
    assert!(
        html.contains("<"),
        "health fragment must be HTML, got: {html}"
    );
}
