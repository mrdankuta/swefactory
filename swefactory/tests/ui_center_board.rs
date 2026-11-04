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

#[test]
fn polling_is_read_only() {
    let src = include_str!("../src/lib.rs");
    let i = src
        .find("pub fn desktop_board_feed")
        .expect("board feed fn exists");
    let body = &src[i..i + 600];
    assert!(body.contains("desktop_fetch_board_issues"), "got:\n{body}");
    let j = src
        .find("pub fn desktop_fetch_board_issues")
        .expect("fetch fn exists");
    let fbody = &src[j..j + 900];
    assert!(fbody.contains("\"issue\"") && fbody.contains("\"list\""));
    for w in ["issue create", "issue edit", "--add-label"] {
        assert!(!fbody.contains(w), "read-only: must not contain {w:?}");
    }
    // No client board database: selection is pure over the feed.
    let k = src
        .find("pub fn desktop_board_select")
        .expect("select fn exists");
    let sbody = &src[k..k + 500];
    assert!(!sbody.contains("INSERT") && !sbody.contains("UPDATE"));
}
