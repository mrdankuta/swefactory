//! Desktop board feed tests (ticket #137, spec #135).
use factory_lib::{
    desktop_board_column_for_labels, desktop_board_html, desktop_board_issues_to_columns,
    desktop_board_json, BoardIssue, DesktopBoardFeed, DESKTOP_BOARD_COLUMNS,
    DESKTOP_BOARD_POLL_MS, DESKTOP_BOARD_POLL_MS_MAX, DESKTOP_BOARD_POLL_MS_MIN,
};

fn issue(n: u64, t: &str, labels: &[&str]) -> BoardIssue {
    BoardIssue { number: n, title: t.into(), body: String::new(),
        labels: labels.iter().map(|s| s.to_string()).collect(), state: "open".into() }
}

#[test]
fn three_columns_todo_review_done() {
    assert_eq!(DESKTOP_BOARD_COLUMNS.len(), 3);
    assert_eq!(DESKTOP_BOARD_COLUMNS[0].0, "todo");
    assert_eq!(DESKTOP_BOARD_COLUMNS[1].0, "review");
    assert_eq!(DESKTOP_BOARD_COLUMNS[2].0, "done");
}

#[test]
fn labels_map_via_existing_mapping() {
    assert_eq!(desktop_board_column_for_labels(&["review".into()]).0, "review");
    assert_eq!(desktop_board_column_for_labels(&["accepted".into()]).0, "done");
    assert_eq!(desktop_board_column_for_labels(&["ticket".into()]).0, "todo");
    assert_eq!(desktop_board_column_for_labels(&vec![]).0, "todo");
    assert_eq!(desktop_board_column_for_labels(&["blocked".into()]).0, "todo");
}

#[test]
fn cards_carry_number_title_marker() {
    let cols = desktop_board_issues_to_columns(&[
        issue(7, "Alpha", &["review"]), issue(8, "Beta", &["ticket"]),
    ]);
    let feed = DesktopBoardFeed { columns: cols, stale: false };
    let html = desktop_board_html(&feed, DESKTOP_BOARD_POLL_MS);
    assert!(html.contains("data-card=\"7\"") && html.contains("#7 Alpha"));
    assert!(html.contains("data-marker") && html.contains(">review<"));
    assert!(!html.contains("data-stale-banner"));
    let json = desktop_board_json(&feed);
    assert!(json.contains("\"number\":7") && json.contains("\"marker\":\"review\""));
}

#[test]
fn poll_bounded_30_to_60s() {
    assert!(DESKTOP_BOARD_POLL_MS_MIN == 30_000 && DESKTOP_BOARD_POLL_MS_MAX == 60_000);
    assert!((DESKTOP_BOARD_POLL_MS_MIN..=DESKTOP_BOARD_POLL_MS_MAX).contains(&DESKTOP_BOARD_POLL_MS));
    let cols = desktop_board_issues_to_columns(&[]);
    let feed = DesktopBoardFeed { columns: cols, stale: false };
    // clamping: below min clamps up, above max clamps down
    assert!(desktop_board_html(&feed, 1).contains("data-poll-ms=\"30000\""));
    assert!(desktop_board_html(&feed, 999_999).contains("data-poll-ms=\"60000\""));
    assert!(desktop_board_html(&feed, DESKTOP_BOARD_POLL_MS).contains("data-poll-ms=\"45000\""));
}

#[test]
fn stale_feed_re_serves_cache_with_banner() {
    let cols = desktop_board_issues_to_columns(&[issue(9, "Cached", &["ticket"])]);
    let stale = DesktopBoardFeed { columns: cols, stale: true };
    let html = desktop_board_html(&stale, DESKTOP_BOARD_POLL_MS);
    assert!(html.contains("data-stale-banner"), "got:\n{html}");
    assert!(html.contains("data-card=\"9\""), "cache re-served, got:\n{html}");
}

#[test]
fn fetch_is_read_only_issue_list() {
    let src = include_str!("../src/lib.rs");
    let i = src.find("pub fn desktop_fetch_board_issues").expect("fetch fn exists");
    let body = &src[i..i + 900];
    assert!(body.contains("\"issue\"") && body.contains("\"list\""), "uses gh issue list, got:\n{body}");
    for w in ["issue create", "issue edit", "issue close", "--add-label", "--remove-label"] {
        assert!(!body.contains(w), "read-only: must not contain {w:?}");
    }
}
