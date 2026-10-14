//! Draft-approve loop UI tests (ticket #43, spec #41 slice).
//!
//! Seam: pure render markers over `OnboardDraft` fixtures plus the
//! state-dir round-trip (`onboard_apply_change` / `onboard_apply_approve`
//! mutate the same files the CLI walks, so the screen and `--plain` can
//! never disagree). No browser, no network — mirrors `ui_board.rs`.

use std::fs;
use std::path::PathBuf;

use axum::response::IntoResponse;

use factory_lib::{
    onboard_apply_approve, onboard_apply_change, onboard_change_handler, onboard_current_draft,
    onboard_fragment_html, onboard_page_html, OnboardChangeForm, OnboardDraft,
};

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap()
}

async fn response_body_text(resp: axum::response::Response) -> (axum::http::StatusCode, String) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-uionboard-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn draft_fixture() -> OnboardDraft {
    OnboardDraft {
        version: 1,
        repo: "bea-cook".to_string(),
        idea: "Groceries remind me what to cook. Dinner plans itself on Sunday.".to_string(),
        epic: "bea-cook: Groceries remind me what to cook.".to_string(),
        spec: "Bea wants: Groceries remind me what to cook. Dinner plans itself on Sunday."
            .to_string(),
        tickets: vec![
            "Ticket 1 — capture the idea in her own words.".to_string(),
            "Ticket 2 — turn the idea into an approvable plan.".to_string(),
            "Ticket 3 — put the approved plan on the board.".to_string(),
        ],
        changes: vec![],
        approved_version: 0,
        hash: String::new(),
    }
}

fn seed_state(dir: &std::path::Path, draft: &OnboardDraft) {
    let state = dir.join("state");
    fs::create_dir_all(&state).unwrap();
    fs::write(
        state.join("onboard.json"),
        serde_json::json!({
            "install_ok": true,
            "auth": "valid",
            "repo": draft.repo,
            "visibility": "private",
            "created": true,
            "seeded": true,
            "steps": ["install", "auth", "repo", "idea", "draft"],
            "idea": draft.idea,
            "draft_version": draft.version,
            "approved_version": draft.approved_version,
            "draft_hash": draft.hash,
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        state.join(format!("onboard-draft-v{}.md", draft.version)),
        format!("# Draft v{}\n\n{}\n", draft.version, draft.idea),
    )
    .unwrap();
}

#[test]
fn onboard_screen_is_primary_sections_plus_decision_forms() {
    let draft = draft_fixture();
    let page = onboard_page_html(Some(&draft));
    let fragment = onboard_fragment_html(Some(&draft));

    for html in [&page, &fragment] {
        assert!(
            html.contains("data-onboard"),
            "the onboard screen carries its marker"
        );
        assert!(
            html.contains("data-draft=\"1\""),
            "the version round-trips visibly"
        );
        assert!(
            html.contains("Groceries remind me what to cook."),
            "her own words on screen"
        );
        assert!(html.contains("data-epic"), "epic section primary");
        assert!(html.contains("data-spec"), "spec section primary");
        assert!(html.contains("data-tickets"), "tickets section primary");
        assert!(
            html.contains("data-approve-form"),
            "Approve is a real decision made easy"
        );
        assert!(
            html.contains("data-change-form"),
            "Change-something is one step away"
        );
    }
    assert!(
        page.contains("data-onboard-fragment=\"/fragments/onboard\""),
        "the page polls its fragment like every other screen"
    );
}

#[test]
fn onboard_screen_without_draft_points_at_the_cli() {
    let page = onboard_page_html(None);
    assert!(page.contains("data-onboard-empty"), "calm empty state");
    assert!(
        page.contains("factory onboard"),
        "Bea learns the one command that starts onboarding"
    );
}

#[test]
fn approved_screen_hands_the_board() {
    let mut draft = draft_fixture();
    draft.approved_version = 1;
    draft.hash = "abc123".to_string();
    let fragment = onboard_fragment_html(Some(&draft));
    assert!(
        fragment.contains("data-approved=\"1\""),
        "the frozen version is provable on screen"
    );
    assert!(
        fragment.contains("href=\"/\"") && fragment.contains("data-board-link"),
        "the approved draft hands the living board"
    );
}

#[test]
fn ui_change_and_approve_round_trip_the_same_files_as_plain() {
    let _guard = env_lock();
    let root = tmpdir("roundtrip");
    let state = root.join("state");
    let draft = draft_fixture();
    seed_state(&root, &draft);
    // SAFETY: static lock held; no other test in this binary touches env.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
    }

    let v2 = onboard_apply_change("Simpler words, please.").expect("change applies");
    assert_eq!(v2, 2, "the UI change round-trips the version forward");
    assert!(
        state.join("onboard-draft-v1.md").is_file(),
        "v1 survives the round-trip"
    );
    let current = onboard_current_draft().expect("draft reads back");
    assert_eq!(current.version, 2);
    assert!(
        current.idea.contains("Groceries remind me what to cook."),
        "v2 still reads as her own words"
    );
    assert!(
        current.changes.iter().any(|c| c.contains("Simpler words")),
        "v2 records what she asked to change"
    );

    let (approved, hash) = onboard_apply_approve().expect("approve applies");
    assert_eq!(approved, 2, "approval freezes the latest round");
    assert_eq!(hash.len(), 64, "sha256 of what she approved");
    let fragment = onboard_fragment_html(onboard_current_draft().as_ref());
    assert!(
        fragment.contains("data-board-link"),
        "after approval the screen hands the board"
    );

    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
    }
}

#[test]
fn approved_draft_refuses_change_and_reapprove_preserves_freeze() {
    let _guard = env_lock();
    let root = tmpdir("approved-lock");
    let state = root.join("state");
    seed_state(&root, &draft_fixture());
    // SAFETY: static lock held; no other test in this binary touches env.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
    }

    let (approved_version, approved_hash) = onboard_apply_approve().expect("first approval");
    assert_eq!(approved_version, 1);
    let before = fs::read_to_string(state.join("onboard.json")).unwrap();
    let v1_before = fs::read(state.join("onboard-draft-v1.md")).unwrap();

    let err = onboard_apply_change("change after approval").unwrap_err();
    assert!(err.contains("already approved"), "plain refusal: {err}");
    assert_eq!(
        fs::read_to_string(state.join("onboard.json")).unwrap(),
        before
    );
    assert_eq!(
        fs::read(state.join("onboard-draft-v1.md")).unwrap(),
        v1_before
    );
    assert!(!state.join("onboard-draft-v2.md").exists());

    assert_eq!(
        onboard_apply_approve().expect("repeat approval is idempotent"),
        (approved_version, approved_hash)
    );
    assert_eq!(
        fs::read_to_string(state.join("onboard.json")).unwrap(),
        before
    );
    assert_eq!(
        fs::read(state.join("onboard-draft-v1.md")).unwrap(),
        v1_before
    );

    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
    }
}

#[test]
fn handlers_serve_the_onboard_screen_through_the_router_code_path() {
    let _guard = env_lock();
    let root = tmpdir("handlers");
    let state = root.join("state");
    seed_state(&root, &draft_fixture());
    // SAFETY: static lock held; no other test in this binary touches env.
    unsafe {
        std::env::set_var("FACTORY_STATE_DIR", &state);
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // GET /onboard: sections plus the decision forms.
    let (status, body) = rt.block_on(response_body_text(
        rt.block_on(factory_lib::onboard_page_handler())
            .into_response(),
    ));
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-onboard"));
    assert!(body.contains("data-approve-form"));
    assert!(body.contains("data-change-form"));

    // GET /fragments/onboard: the polling unit.
    let (status, body) = rt.block_on(response_body_text(
        rt.block_on(factory_lib::onboard_fragment_handler())
            .into_response(),
    ));
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("data-onboard"));

    // POST /onboard/change: the round-trip without the CLI.
    let resp = rt.block_on(onboard_change_handler(axum::Form(OnboardChangeForm {
        change: "Simpler words, please.".to_string(),
    })));
    let (status, _) = rt.block_on(response_body_text(resp.into_response()));
    assert_eq!(status, axum::http::StatusCode::SEE_OTHER);
    let current = onboard_current_draft().expect("change handler bumps the draft");
    assert_eq!(current.version, 2);

    // POST /onboard/approve: the freeze without the CLI.
    let resp = rt.block_on(factory_lib::onboard_approve_handler());
    let (status, _) = rt.block_on(response_body_text(resp.into_response()));
    assert_eq!(status, axum::http::StatusCode::SEE_OTHER);
    let current = onboard_current_draft().expect("approve handler freezes the draft");
    assert_eq!(current.approved_version, 2);
    let v2_before = fs::read(state.join("onboard-draft-v2.md")).unwrap();

    // Direct POSTs remain harmless after approval even though the UI hides
    // the forms: the shared applier enforces the freeze, not presentation.
    let resp = rt.block_on(onboard_change_handler(axum::Form(OnboardChangeForm {
        change: "post-approval change must be refused".to_string(),
    })));
    let (status, _) = rt.block_on(response_body_text(resp.into_response()));
    assert_eq!(status, axum::http::StatusCode::SEE_OTHER);
    assert_eq!(onboard_current_draft().as_ref(), Some(&current));
    assert_eq!(
        fs::read(state.join("onboard-draft-v2.md")).unwrap(),
        v2_before
    );
    assert!(!state.join("onboard-draft-v3.md").exists());

    let resp = rt.block_on(factory_lib::onboard_approve_handler());
    let (status, _) = rt.block_on(response_body_text(resp.into_response()));
    assert_eq!(status, axum::http::StatusCode::SEE_OTHER);
    assert_eq!(onboard_current_draft().as_ref(), Some(&current));

    unsafe {
        std::env::remove_var("FACTORY_STATE_DIR");
    }
}
