//! PromptArgs in the brief composer (ticket #106, spec #103 slice 3).
//!
//! Seam: the pure composer public API (`factory_lib::compose_brief` +
//! `PromptArgs`) — no `gh`/`git`/`pi` involved, so these tests are hermetic.
//!
//! What these tests catch: a brief referencing an undeclared `{{KEY}}`
//! fails composition with an error naming the key (unknown keys and
//! known-keys-without-values alike); a declared-but-unused key warns while
//! composition proceeds with correct substitution; keyless templates render
//! byte-identical (malformed braces, shell-looking text pass through
//! literally); composition never executes commands (host-side substitution
//! only).
//! What they miss: the dispatch wiring that supplies the values (covered
//! by the unchanged existing CLI tests) and the real `pi` worker reading
//! the composed prompt.

use factory_lib::{compose_brief, PromptArgs};
use std::collections::HashMap;

fn args(base: Option<&str>, head: Option<&str>, freeze: Option<&str>) -> PromptArgs {
    PromptArgs::new(
        base.map(str::to_string),
        head.map(str::to_string),
        freeze.map(str::to_string),
    )
}

fn full_args() -> PromptArgs {
    args(
        Some("abc123"),
        Some("def456"),
        Some("f".repeat(64).as_str()),
    )
}

// --- fail: undeclared key ---

#[test]
fn unknown_key_fails_naming_the_key() {
    let err = compose_brief("base is {{NOPE}}", &full_args()).unwrap_err();
    assert!(
        err.contains("NOPE"),
        "error must name the undeclared key, got: {err}"
    );
}

#[test]
fn known_key_without_value_fails_naming_the_key() {
    let a = args(Some("abc123"), None, None);
    let err = compose_brief("head is {{HEAD_COMMIT}}", &a).unwrap_err();
    assert!(
        err.contains("HEAD_COMMIT"),
        "error must name the key with no declared value, got: {err}"
    );
}

#[test]
fn first_undeclared_key_wins_when_several_are_missing() {
    let a = args(None, None, None);
    let err = compose_brief("{{AAA}} then {{BBB}}", &a).unwrap_err();
    assert!(
        err.contains("AAA"),
        "error must name the first undeclared key, got: {err}"
    );
}

// --- warn: declared but unused ---

#[test]
fn declared_but_unused_key_warns_and_composition_proceeds() {
    let a = args(Some("abc123"), Some("def456"), None);
    let composed = compose_brief("base is {{BASE_COMMIT}}", &a).unwrap();
    assert_eq!(composed.text, "base is abc123");
    assert_eq!(composed.warnings.len(), 1, "one unused key must warn");
    assert!(
        composed.warnings[0].contains("HEAD_COMMIT"),
        "warning must name the unused key, got: {}",
        composed.warnings[0]
    );
}

// --- substitution ---

#[test]
fn all_three_keys_substitute_everywhere() {
    let freeze = "f".repeat(64);
    let a = args(Some("abc123"), Some("def456"), Some(&freeze));
    let composed = compose_brief(
        "base={{BASE_COMMIT}} head={{HEAD_COMMIT}} freeze={{SPEC_FREEZE}} again={{BASE_COMMIT}}",
        &a,
    )
    .unwrap();
    assert_eq!(
        composed.text,
        format!("base=abc123 head=def456 freeze={freeze} again=abc123")
    );
    assert!(composed.warnings.is_empty(), "all used: no warnings");
}

#[test]
fn whitespace_inside_braces_is_trimmed() {
    let a = args(Some("abc123"), None, None);
    let composed = compose_brief("base is {{  BASE_COMMIT  }}", &a).unwrap();
    assert_eq!(composed.text, "base is abc123");
}

// --- unchanged: keyless templates ---

#[test]
fn keyless_template_renders_byte_identical() {
    let template = "# /build — implement exactly one ticket\n\nRead AGENTS.md first.\n";
    let composed = compose_brief(template, &full_args()).unwrap();
    assert_eq!(composed.text, template);
    assert!(
        composed.warnings.is_empty(),
        "keyless legacy briefs stay silent, got: {:?}",
        composed.warnings
    );
}

#[test]
fn malformed_braces_pass_through_literally() {
    for template in [
        "unterminated {{BASE_COMMIT",
        "empty {{}} stays",
        "blank {{   }} stays",
        "dash {{foo-bar}} stays",
        "spaces {{a b}} stay",
        "bang {{!BASE_COMMIT}} stays",
    ] {
        let composed = compose_brief(template, &full_args()).unwrap();
        assert_eq!(composed.text, template, "template: {template}");
    }
}

// --- no command execution ---

#[test]
fn shell_looking_text_passes_through_literally() {
    let template = "run !echo hi and $(echo hi) and `echo hi` now";
    let composed = compose_brief(template, &full_args()).unwrap();
    assert_eq!(composed.text, template);
}

#[test]
fn substitution_values_are_never_executed() {
    // A value that looks like a command substitution must land verbatim.
    let mut map_args = full_args();
    map_args.base_commit = Some("$(echo pwned)".to_string());
    let composed = compose_brief("base={{BASE_COMMIT}}", &map_args).unwrap();
    assert_eq!(composed.text, "base=$(echo pwned)");
}

// --- map constructor parity (hidden seam check) ---

#[test]
fn prompt_args_exposes_its_declared_values() {
    let a = full_args();
    let map: HashMap<&str, &str> = [
        ("BASE_COMMIT", a.base_commit.as_deref().unwrap()),
        ("HEAD_COMMIT", a.head_commit.as_deref().unwrap()),
        ("SPEC_FREEZE", a.spec_freeze.as_deref().unwrap()),
    ]
    .into_iter()
    .collect();
    assert_eq!(map["BASE_COMMIT"], "abc123");
    assert_eq!(map["SPEC_FREEZE"], "f".repeat(64));
}
