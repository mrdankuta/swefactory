//! Ticket #151 (spec #149): center board over the DesktopBoard feed.
//! Columns derive from labels; cards show number + title + status marker;
//! poll interval bounded 30-60s; stale banner when unreachable; card
//! selection scopes the detail pane; polling never writes to the tracker.
use factory_lib::{
    desktop_board_html, desktop_board_html_with_selection, desktop_board_issues_to_columns,
    desktop_board_select, BoardIssue, DesktopBoardFeed, DESKTOP_BOARD_POLL_MS,
};

fn issue(n: u64, t: &str, labels: &[&str]) -> BoardIssue {
    BoardIssue {
        number: n,
        title: t.into(),
        body: String::new(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        state: "open".into(),
    }
}

fn feed() -> DesktopBoardFeed {
    DesktopBoardFeed {
        columns: desktop_board_issues_to_columns(&[
            issue(7, "Alpha", &["review"]),
            issue(8, "Beta", &["ticket"]),
        ]),
        stale: false,
    }
}

#[test]
fn columns_derive_from_labels_with_number_title_marker() {
    let html = desktop_board_html(&feed(), DESKTOP_BOARD_POLL_MS);
    assert!(html.contains("data-column=\"todo\""));
    assert!(html.contains("data-column=\"review\""));
    assert!(html.contains("data-column=\"done\""));
    assert!(html.contains("data-card=\"7\"") && html.contains("#7 Alpha"));
    assert!(html.contains("data-marker") && html.contains(">review<"));
}

#[test]
fn poll_interval_bounded_30_to_60s() {
    let f = feed();
    assert!(desktop_board_html(&f, 1).contains("data-poll-ms=\"30000\""));
    assert!(desktop_board_html(&f, 999_999).contains("data-poll-ms=\"60000\""));
    assert!(desktop_board_html(&f, DESKTOP_BOARD_POLL_MS).contains("data-poll-ms=\"45000\""));
}

#[test]
fn stale_banner_when_unreachable() {
    let stale = DesktopBoardFeed {
        columns: feed().columns.clone(),
        stale: true,
    };
    let html = desktop_board_html(&stale, DESKTOP_BOARD_POLL_MS);
    assert!(html.contains("data-stale-banner"));
    assert!(html.contains("data-card=\"7\""), "last cache re-served");
}

#[test]
fn card_selection_scopes_detail_pane() {
    let f = feed();
    let card = desktop_board_select(&f, 7).expect("card 7 exists");
    assert_eq!(card.number, 7);
    assert_eq!(card.title, "Alpha");
    assert!(desktop_board_select(&f, 999).is_none());
    let html = desktop_board_html_with_selection(&f, DESKTOP_BOARD_POLL_MS, Some(7));
    assert!(html.contains("data-card=\"7\""));
    assert!(
        html.contains("data-selected=\"true\""),
        "selected card flagged, got:\n{html}"
    );
    assert!(
        html.contains("data-selected-card=\"7\"") && html.contains("data-scopes=\"detail\""),
        "selection scopes detail pane, got:\n{html}"
    );
    // Unselected cards are not flagged.
    let unselected = desktop_board_html_with_selection(&f, DESKTOP_BOARD_POLL_MS, None);
    assert!(!unselected.contains("data-selected=\"true\""));
    assert!(!unselected.contains("data-selected-card"));
}

/// Extract the full body of `pub fn {name}` from `src`: from the `pub fn`
/// marker to the next top-level `pub fn` (or EOF). Anchored to function
/// boundaries, not fixed char windows, so refactors don't shift/break it.
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let marker = format!("pub fn {name}");
    let start = src
        .find(&marker)
        .unwrap_or_else(|| panic!("{name} fn exists"));
    let rest = &src[start..];
    // Skip past the marker itself, then find the next top-level fn.
    let after = &rest[marker.len()..];
    let end = after.find("\npub fn ").map(|i| i + marker.len());
    match end {
        Some(e) => &rest[..e],
        None => rest,
    }
}

#[test]
fn polling_is_read_only() {
    let src = include_str!("../src/lib.rs");
    let feed_body = fn_body(src, "desktop_board_feed");
    assert!(
        feed_body.contains("desktop_fetch_board_issues"),
        "got:\n{feed_body}"
    );
    let fetch_body = fn_body(src, "desktop_fetch_board_issues");
    assert!(fetch_body.contains("\"issue\"") && fetch_body.contains("\"list\""));
    for w in ["issue create", "issue edit", "--add-label"] {
        assert!(!fetch_body.contains(w), "read-only: must not contain {w:?}");
    }
    assert!(
        !fetch_body.contains("INSERT") && !fetch_body.contains("UPDATE"),
        "feed path must not write"
    );
    // No client board database: selection is pure over the feed.
    let select_body = fn_body(src, "desktop_board_select");
    assert!(!select_body.contains("INSERT") && !select_body.contains("UPDATE"));
}

#[test]
fn repeated_card_selection_keeps_scoping_detail() {
    let f = feed();
    for n in [7u64, 8, 7] {
        let html = desktop_board_html_with_selection(&f, DESKTOP_BOARD_POLL_MS, Some(n));
        assert!(
            html.contains(&format!("data-selected-card=\"{n}\""))
                && html.contains("data-scopes=\"detail\""),
            "repeat select scopes detail, got:\n{html}"
        );
    }
    let shell = include_str!("../../shell/index.html");
    assert!(
        !shell.contains("once:true") && !shell.contains("once: true"),
        "click listener must not use once:true"
    );
    assert!(
        shell.contains("addEventListener"),
        "click listener registered"
    );
}
