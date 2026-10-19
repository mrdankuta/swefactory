//! Site-wide style contract: every page shell embeds the stylesheet,
//! scopes its header, and keeps dynamic titles free of style leakage.
//! Regression guard for the `?variant`-free fold-in (a misplaced format
//! arg once rendered CSS inside `<title>`).

use factory_lib::{
    board_page_html, dispatcher_page_html, files_page_html, git_page_html, onboard_page_html,
    ticket_detail_page_html, ticket_missing_page_html, train_landed_page_html,
    train_review_page_html, BoardCard, BoardColumn, DispatcherData, GitData, TicketDetail,
    TrainReviewData,
};

fn assert_site_shell(name: &str, page: &str) {
    assert!(
        page.contains("<style>") && page.contains("box-sizing"),
        "{name}: missing embedded stylesheet"
    );
    assert!(
        page.contains("<header class=\"site\">"),
        "{name}: header missing site class"
    );
}

fn board_cols() -> Vec<BoardColumn> {
    vec![BoardColumn {
        id: "ready",
        title: "Ready to build",
        cards: vec![BoardCard {
            number: 7,
            title: "Probe".to_string(),
            status_text: "Ready to build.".to_string(),
            cost_text: "no cost recorded".to_string(),
            changed_text: "nothing yet".to_string(),
            labels: vec!["ready".to_string()],
        }],
    }]
}

#[test]
fn every_page_shell_carries_the_stylesheet() {
    let detail = TicketDetail {
        number: 46,
        title: "Probe".to_string(),
        body: "body".to_string(),
        labels: vec![],
        state: "open".to_string(),
        transcript: None,
        attempts: vec![],
    };
    let pages = [
        ("board", board_page_html(&board_cols())),
        ("ticket", ticket_detail_page_html(&detail)),
        ("ticket-missing", ticket_missing_page_html("xyz")),
        (
            "dispatch",
            dispatcher_page_html(&DispatcherData {
                attempts: vec![],
                stopped: false,
                worktrees: vec![],
                max_concurrent: 1,
                tasks: vec![],
            }),
        ),
        (
            "git",
            git_page_html(&GitData {
                commits: vec![],
                branches: vec![],
                worktrees: vec![],
            }),
        ),
        ("files", files_page_html(&[], "HEAD")),
        (
            "train",
            train_review_page_html(&TrainReviewData {
                open: false,
                train: "".to_string(),
                base: "".to_string(),
                head: "".to_string(),
                members: vec![],
                notice: "".to_string(),
            }),
        ),
        ("landed", train_landed_page_html("t", &[], "note")),
        ("onboard", onboard_page_html(None)),
    ];
    for (name, page) in pages {
        assert_site_shell(name, &page);
    }
}

#[test]
fn dynamic_titles_keep_their_data_and_no_css() {
    let detail = TicketDetail {
        number: 46,
        title: "Probe".to_string(),
        body: "body".to_string(),
        labels: vec![],
        state: "open".to_string(),
        transcript: None,
        attempts: vec![],
    };
    let page = ticket_detail_page_html(&detail);
    let title = page
        .split("<title>")
        .nth(1)
        .and_then(|t| t.split("</title>").next())
        .unwrap_or("");
    assert_eq!(title, "Ticket #46 — Factory", "title got: {title}");
}
