use askama::Template;
use clap::{Parser, Subcommand};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// swefactory: the software factory as a product (single static binary).
#[derive(Parser, Debug)]
#[command(
    name = "factory",
    version,
    about = "swefactory — production-grade software, consumer-simple UX"
)]
/// CLI parser (also the shape future HTTP handlers mirror: one function
/// per command below). The binary in `main.rs` is a thin wrapper over
/// [`run`]; handlers call the `cmd_*` functions directly.
pub struct Cli {
    /// Emit machine-parseable JSON for every command that produces output.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

/// Project files `factory status` treats as project presence.
const STATUS_PROJECT_FILES: &[&str] = &["AGENTS.md", "GLOSSARY.md", "scripts/factory-check"];

/// Default transcript tail (lines) for `factory log` when --tail is absent.
/// 50 lines bounds a context window (~a few KB) while keeping enough story;
/// --tail 0 opts into the full transcript.
const LOG_DEFAULT_TAIL: usize = 50;

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Print the factory version.
    Version,
    /// One-screen factory health: version, project files, open train,
    /// live attempts, STOP state. Read-only: observing changes nothing.
    /// Exit 0 whenever the state was read successfully (even when empty);
    /// nonzero only on unreadable state (missing/corrupt ledger) with the
    /// cause named. The global --json flag selects machine output.
    Status,
    /// Ticket history: per-attempt entries newest-first with outcome
    /// summaries before bounded transcripts. Read-only: observing changes
    /// nothing. Exit 0 whenever the state was read successfully (even when
    /// the ticket has no attempts); nonzero only on unreadable state (bad
    /// ticket number, missing/corrupt ledger) with the cause named.
    /// --tail N shows the last N transcript lines (default 50); --tail 0
    /// means the full transcript. The global --json flag selects machine
    /// output carrying the same facts structurally.
    Log {
        /// Ticket issue number (e.g. 98 or #98).
        ticket: String,
        /// Transcript lines shown per attempt (0 means full).
        #[arg(long, default_value_t = LOG_DEFAULT_TAIL)]
        tail: usize,
    },
    /// Seed the factory into the current project (idempotent, never clobbers).
    Init {
        /// Check-adapter stack to install (default: auto-detect from the project).
        #[arg(long)]
        stack: Option<String>,
        /// Install skills to $HOME/.agents/skills instead of the project.
        #[arg(long)]
        global_skills: bool,
        /// Guided setup: ask at most stack and skills location via
        /// numbered stdin choices (default: the detected stack). Engages
        /// automatically on ambiguous detection when stdin is an
        /// interactive terminal. Never prompts with --json or when stdin
        /// is piped/non-terminal, so scripts and CI can never hang.
        #[arg(long)]
        interactive: bool,
    },
    /// Refresh prompts/skills/checks in place (idempotent, never clobbers).
    Update {
        /// Release tag to refresh from (recorded; network fetch is out of
        /// scope for the static binary, which carries embedded templates).
        #[arg(long)]
        pin: Option<String>,
    },
    /// Record spec approval as a content hash (SPEC-APPROVED comment + label).
    Freeze {
        /// Issue number (e.g. 6 or #6).
        issue: String,
        /// Label marking the issue approved.
        #[arg(long, default_value = "spec-approved")]
        label: String,
    },
    /// Snapshot the spec locally and verify it against the freeze.
    Sync {
        /// Issue number (e.g. 6 or #6).
        issue: String,
    },
    /// Exit 0 iff the spec body still matches its SPEC-APPROVED hash.
    #[command(name = "verify-freeze")]
    VerifyFreeze {
        /// Spec issue number (e.g. 6 or #6).
        #[arg(value_name = "SPEC-ISSUE")]
        issue: String,
    },
    /// Run the check-adapter contract (format, then lint, then test) as a
    /// subprocess. The adapter stays a plain script; this runner only
    /// discovers it, forwards the slot, streams its output, and propagates
    /// its exit code — so green keeps meaning green.
    Check {
        /// Path to the check-adapter executable. Defaults to
        /// $FACTORY_CHECK_ADAPTER, else the nearest scripts/factory-check
        /// walking up from the current directory.
        #[arg(long)]
        adapter: Option<PathBuf>,

        /// Slot name for this run. Sets FACTORY_SLOT for the adapter so
        /// concurrent workers never share mutable resources. Defaults to
        /// the inherited FACTORY_SLOT, if any.
        #[arg(long)]
        slot: Option<String>,

        /// Extra arguments forwarded verbatim to the adapter (after `--`).
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Pre-flight check: probe every prerequisite, report each in plain
    /// English with its fix, exit nonzero when anything blocks.
    /// Read-only by default: diagnosing never changes anything. With
    /// --fix, repair safe local state (missing dirs, stale worktree
    /// metadata, missing project files re-seeded through init's own
    /// seeding path) — never issues, remotes, toolchains, or auth flows.
    /// The global --json flag selects machine output.
    Doctor {
        /// Repair everything safe to repair locally, then report.
        #[arg(long)]
        fix: bool,
    },
    /// Post a handoff record to the ticket issue via the gh CLI.
    Handoff {
        /// Ticket issue number (e.g. 11 or #11).
        issue: String,
        /// Handoff verdict.
        #[arg(long, value_name = "SIGNAL")]
        signal: String,
        /// File whose contents form the handoff report body.
        #[arg(long, value_name = "FILE")]
        body: Option<PathBuf>,
    },
    /// Read-only drift report: open issues vs the dev branch.
    Reconcile,
    /// Open the work for a ticket: worktree + branch + brief, ready to implement in.
    Start {
        /// Ticket issue number (e.g. 11 or #11).
        ticket: String,
        /// Override the default worktree location (testing / advanced).
        #[arg(long, value_name = "DIR")]
        worktree: Option<PathBuf>,
    },
    /// Deterministic dispatch loop: single passes moving a ready ticket
    /// through implement, verify, and review. Never lands, never touches
    /// dev/main.
    Dispatch {
        #[command(subcommand)]
        action: DispatchAction,
    },
    /// Release train: accepted tickets accumulate onto an integration
    /// branch that only clean merges can join. Never touches dev/main,
    /// never deletes branches (finish/abandon only print cleanup).
    Release {
        #[command(subcommand)]
        action: ReleaseAction,
    },
    /// Land the open train: the human's one-command accept-and-land.
    /// Preflights everything (open train, clean tree, green head,
    /// reachable remote), then merges train→dev --no-ff, pushes dev,
    /// closes member tickets with the release note, removes the train
    /// worktree, and prints branch cleanup (never executed). With
    /// --dry-run, prints the full mutation plan and changes nothing.
    /// With --train, lands the named branch instead of the open train;
    /// an already-merged branch is verified (merged-elsewhere check) and
    /// not re-merged. Land never runs under automation: a dispatcher
    /// context refuses with a plain-English error before anything else.
    Land {
        /// Preview every mutation without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Land this train branch instead of the open train.
        #[arg(long, value_name = "BRANCH")]
        train: Option<String>,
    },
    /// Serve the Bea-first board over localhost (spec #27 board slice).
    /// Server-rendered HTML with polling fragments; localhost-only unless
    /// --allow-remote is passed explicitly. Read-only: GETs observe
    /// through the ledger, `gh`, and git seams and never mutate.
    Ui {
        /// Address to bind (host:port). Localhost by default.
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: String,
        /// Allow binding a non-localhost address (explicit exposure).
        #[arg(long)]
        allow_remote: bool,
    },
    /// Alias for `ui` (headless use): serves the same board app.
    Serve {
        /// Address to bind (host:port). Localhost by default.
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: String,
        /// Allow binding a non-localhost address (explicit exposure).
        #[arg(long)]
        allow_remote: bool,
    },
    /// Bea's front door (spec #41 backbone, ticket #42): install check,
    /// GitHub auth as a guided tour, repo pick-or-create (private by
    /// default, identity-only seed for fresh repos), idea capture, and the
    /// draft-approve loop (idea in, approved plan out). A state machine
    /// that resumes: rerunning continues where the last run stopped and
    /// never duplicates created resources. The `/onboard` screen is
    /// primary; --plain carries the same facts for SSH and automation.
    /// Choices arrive via flags or FACTORY_ONBOARD_* env so SSH and
    /// automation never hit an interactive wall.
    Onboard {
        /// Print the plain transcript for every step (explicit).
        #[arg(long)]
        plain: bool,
        /// Pick an existing repo (e.g. `bea/existing`).
        #[arg(long)]
        repo: Option<String>,
        /// Create a fresh repo with this name (private by default).
        #[arg(long)]
        new: Option<String>,
        /// Fresh-repo visibility: `private` (default) or `public`.
        #[arg(long)]
        visibility: Option<String>,
        /// The idea in plain sentences, stored verbatim in the seed.
        #[arg(long)]
        idea: Option<String>,
        /// Approve the current draft version (freezes it with its hash).
        #[arg(long)]
        approve: bool,
        /// Ask for a change in plain words (mints the next version).
        #[arg(long)]
        change: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ReleaseAction {
    /// Cut an integration train from the dev branch (refused while one is open).
    Start {
        /// Train slug (e.g. `beta`). Defaults to `train`.
        #[arg(value_name = "SLUG")]
        slug: Option<String>,
    },
    /// Show the open train + member branches + head.
    Status,
    /// Merge an accepted ticket branch into the train (clean only).
    Merge {
        /// Ticket issue number (e.g. 11 or #11).
        ticket: String,
    },
    /// Revert a ticket's merge from the train (fails loudly when not merged).
    Drop {
        /// Ticket issue number (e.g. 11 or #11).
        ticket: String,
    },
    /// Verify the human merged the train into dev, mark merged, remove the
    /// train worktree, and print branch cleanup (never deletes).
    Finish,
    /// Close an unmerged train with no merge check, remove the worktree.
    Abandon,
}

#[derive(Subcommand, Debug)]
pub enum DispatchAction {
    /// Single scheduling pass over ready tickets.
    Once {
        /// Survey only: no worktrees, subprocesses, or writes.
        #[arg(long)]
        dry_run: bool,
        /// Client-generated exactly-once key for this pass: a repeat with
        /// the same key replays the stored outcome with a replay marker
        /// instead of double-admitting. Keys are retained for 24 hours;
        /// reuse of one key across different operations is rejected.
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Loop single passes on the interval until the STOP file appears
    /// (Rust port of the bash `factory-dispatch daemon` loop).
    Daemon {
        /// Seconds to sleep between passes.
        #[arg(long, default_value_t = 300)]
        interval: u64,
    },
    /// Show attempts + worktrees + STOP state.
    Status,
    /// Halt the loop: later `once` runs exit 0 without work.
    Stop,
    /// Restart the loop after a stop.
    Resume,
    /// Hermetic proof of the release mechanics (temp bare origin, no gh,
    /// no network, no tokens, no spend). Every failure names the mechanics
    /// that broke and exits non-zero.
    SelfTest,
}

#[derive(Serialize)]
struct VersionOutput<'a> {
    version: &'a str,
}

/// Exact stdout text for `factory version` (trailing newline included).
/// Single code path shared by the CLI printer and future HTTP handlers:
/// change the bytes here and both change together.
pub fn version_text(json: bool) -> String {
    let version = env!("CARGO_PKG_VERSION");
    if json {
        format!(
            "{}\n",
            serde_json::to_string(&VersionOutput { version }).expect("version JSON serialization")
        )
    } else {
        format!("{version}\n")
    }
}

// --- web board (ticket #29, spec #27) ------------------------------------
//
// `factory ui` (alias `factory serve`) serves Bea's home: issues as
// cards in columns, polling fragments, calm empty states. Same-binary
// Axum, server-rendered Askama HTML, no JS build, no websockets.
//
// Data flows read-only through the existing seams: `gh issue list` via
// `run_gh_output` (overridable with FACTORY_GH_BIN), the dispatch ledger
// via `dispatch_state_dir` (overridable with FACTORY_STATE_DIR), and
// `git log --grep` via `run_git` (overridable with FACTORY_GIT_BIN).
// Rendering is pure over `BoardIssue` fixtures so endpoint tests never
// need the network.

/// One open issue as the board sees it (labels drive the column).
#[derive(Debug, Clone, PartialEq)]
pub struct BoardIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub state: String,
}

/// One rendered card: status, cost, and what changed in plain words.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardCard {
    pub number: u64,
    pub title: String,
    pub status_text: String,
    pub cost_text: String,
    pub changed_text: String,
    pub labels: Vec<String>,
}

/// One board column: a label-mapped lane of cards.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardColumn {
    pub id: &'static str,
    pub title: &'static str,
    pub cards: Vec<BoardCard>,
}

/// Fixed column order (Bea's reading order): needs-you first, done last.
/// (id, title) pairs; `board_column_for_labels` maps labels into them.
pub const BOARD_COLUMNS: &[(&str, &str)] = &[
    ("needs-you", "Needs you"),
    ("ready", "Ready to build"),
    ("running", "Being built"),
    ("review", "In review"),
    ("done", "Done"),
    ("open", "New work"),
];

/// Poll interval for the board fragment (spec: board ~15s).
pub const BOARD_POLL_MS: u64 = 15_000;

fn has_label(labels: &[String], name: &str) -> bool {
    labels.iter().any(|l| l.eq_ignore_ascii_case(name))
}

/// Map issue labels to a column (id, title). First match wins in Bea
/// priority order: blocked/needs-* → ready → running → review →
/// accepted/integrated → fallback open. Pure: unit-tested without `gh`.
pub fn board_column_for_labels(labels: &[String]) -> (&'static str, &'static str) {
    if has_label(labels, "blocked")
        || has_label(labels, "needs-info")
        || has_label(labels, "needs-you")
    {
        return ("needs-you", "Needs you");
    }
    if has_label(labels, "review") {
        return ("review", "In review");
    }
    if has_label(labels, "running") {
        return ("running", "Being built");
    }
    if has_label(labels, "ready") {
        return ("ready", "Ready to build");
    }
    if has_label(labels, "accepted") || has_label(labels, "integrated") {
        return ("done", "Done");
    }
    ("open", "New work")
}

/// Bea-readable status for one card. The optional attempt status is the
/// dispatcher's latest word for the ticket (e.g. `impl_running`); labels
/// alone still read as a sentence when there is no attempt yet.
pub fn board_status_text(labels: &[String], attempt: Option<&str>) -> String {
    let (id, _) = board_column_for_labels(labels);
    let base = match id {
        "needs-you" => "Waiting on you \u{2014} a question needs your answer.",
        "ready" => "Ready to build.",
        "running" => "Being built now.",
        "review" => "In review \u{2014} automation is checking the work.",
        "done" => "Done \u{2014} accepted.",
        _ => "New \u{2014} waiting to be picked up.",
    };
    match attempt {
        None => base.to_string(),
        Some(s) => match s {
            "impl_running" => format!("{base} An implement attempt is running."),
            "review_running" => format!("{base} A review attempt is running."),
            "impl_failed" => format!("{base} The last attempt failed during implement."),
            "review_failed" => format!("{base} The last attempt failed review."),
            "review_done" | "impl_done" => {
                format!("{base} The last attempt finished: {s}.")
            }
            other => format!("{base} Last attempt: {other}."),
        },
    }
}

/// Bea-readable cost for one card: the first `Cost:` line in the body, or
/// a calm sentence when the issue names no cost.
pub fn board_cost_text(body: &str) -> String {
    for line in body.lines() {
        let t = line.trim();
        if t.len() > 5 && t[..5].eq_ignore_ascii_case("cost:") {
            let value = t[5..].trim();
            if !value.is_empty() {
                return format!("Cost: {value}");
            }
        }
    }
    "Cost not estimated yet.".to_string()
}

/// Bea-readable what-changed for one card: the newest `git log` hit that
/// mentions the ticket, or a calm sentence when nothing landed yet.
pub fn board_changed_text(hits: &[String]) -> String {
    match hits.iter().find(|h| !h.trim().is_empty()) {
        Some(h) => format!("What changed: {}", h.trim()),
        None => "No changes recorded yet.".to_string(),
    }
}

/// Group issues into the fixed column order. `attempts` maps ticket →
/// latest attempt status; `changes` maps ticket → `git log` hit lines.
/// Pure over fixtures: the seam endpoint tests assert.
pub fn board_issues_to_columns(
    issues: &[BoardIssue],
    attempts: &std::collections::HashMap<u64, String>,
    changes: &std::collections::HashMap<u64, Vec<String>>,
) -> Vec<BoardColumn> {
    let mut cols: Vec<BoardColumn> = BOARD_COLUMNS
        .iter()
        .map(|(id, title)| BoardColumn {
            id,
            title,
            cards: Vec::new(),
        })
        .collect();
    for issue in issues {
        let (id, _) = board_column_for_labels(&issue.labels);
        let empty: Vec<String> = Vec::new();
        let hits = changes.get(&issue.number).unwrap_or(&empty);
        let card = BoardCard {
            number: issue.number,
            title: issue.title.clone(),
            status_text: board_status_text(
                &issue.labels,
                attempts.get(&issue.number).map(|s| s.as_str()),
            ),
            cost_text: board_cost_text(&issue.body),
            changed_text: board_changed_text(hits),
            labels: issue.labels.clone(),
        };
        if let Some(col) = cols.iter_mut().find(|c| c.id == id) {
            col.cards.push(card);
        }
    }
    cols
}

/// Calm sentence for a fully empty board (Bea: ready, not broken).
pub fn board_empty_sentence() -> &'static str {
    "No work is waiting \u{2014} your board is clear and ready for the next idea."
}

/// Calm sentence for an empty column.
pub fn board_column_empty_sentence() -> &'static str {
    "Nothing here right now."
}

fn gh_labels(value: &serde_json::Value) -> Vec<String> {
    match value.get("labels") {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|l| match l {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(m) => m
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Read-only issue fetch: open issues with labels + bodies. On any `gh`
/// failure returns empty (the board renders its calm empty state, never
/// a 500 for a missing CLI). No writes, no side effects.
pub fn fetch_board_issues() -> Vec<BoardIssue> {
    let out = match run_gh_output(&[
        "issue",
        "list",
        "--state",
        "open",
        "--limit",
        "50",
        "--json",
        "number,title,body,labels,state",
    ]) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return Vec::new(),
    };
    let items: Vec<serde_json::Value> = match serde_json::from_str(&out) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    items
        .iter()
        .map(|v| BoardIssue {
            number: v.get("number").and_then(|n| n.as_u64()).unwrap_or(0),
            title: v
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            body: v
                .get("body")
                .and_then(|b| b.as_str())
                .unwrap_or("")
                .to_string(),
            labels: gh_labels(v),
            state: v
                .get("state")
                .and_then(|s| s.as_str())
                .unwrap_or("open")
                .to_string(),
        })
        .filter(|i| i.number != 0)
        .collect()
}

/// Read-only: latest attempt status per ticket from the dispatch ledger.
/// Missing/unreadable ledger reads as no attempts (empty board lanes).
pub fn board_attempt_status() -> std::collections::HashMap<u64, String> {
    let state = dispatch_state_dir();
    let ledger = load_ledger(&state);
    let mut map = std::collections::HashMap::new();
    for a in &ledger.attempts {
        map.insert(a.ticket, a.status.clone());
    }
    map
}

/// Read-only: newest `git log` lines mentioning `#<ticket>` (same seam
/// `reconcile` uses). Failures read as no changes.
pub fn board_changed_hits(ticket: u64) -> Vec<String> {
    let pattern = format!("#{ticket}");
    let base = drift_base();
    run_git(&["log", &base, "--oneline", "--grep", &pattern], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .take(3)
                .map(|l| l.to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Read-only board assembly: issues × attempts × git hits → columns.
pub fn board_data() -> Vec<BoardColumn> {
    let issues = fetch_board_issues();
    let attempts = board_attempt_status();
    let mut changes = std::collections::HashMap::new();
    for issue in &issues {
        changes.insert(issue.number, board_changed_hits(issue.number));
    }
    board_issues_to_columns(&issues, &attempts, &changes)
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Fragment HTML for the board columns (the polling unit). Pure over
/// columns so tests assert markers without a server.
pub fn board_fragment_html(columns: &[BoardColumn]) -> String {
    let total: usize = columns.iter().map(|c| c.cards.len()).sum();
    let mut html = String::new();
    if total == 0 {
        html.push_str(&format!(
            "<section data-board-empty><p>{}</p></section>",
            board_empty_sentence()
        ));
    }
    html.push_str("<section data-board-columns>");
    for col in columns {
        html.push_str(&format!(
            "<section data-column=\"{}\"><h2>{} ({})</h2>",
            col.id,
            col.title,
            col.cards.len()
        ));
        if col.cards.is_empty() {
            html.push_str(&format!(
                "<p data-empty-column>{}</p>",
                board_column_empty_sentence()
            ));
        }
        for card in &col.cards {
            html.push_str(&format!(
                "<article data-card=\"{}\" data-column=\"{}\"><h3>#{} {}</h3><p data-status>{}</p><p data-cost>{}</p><p data-changed>{}</p><a data-detail-link href=\"/tickets/{}\">Open detail</a></article>",
                card.number,
                col.id,
                card.number,
                escape_html(&card.title),
                escape_html(&card.status_text),
                escape_html(&card.cost_text),
                escape_html(&card.changed_text),
                card.number,
            ));
        }
        html.push_str("</section>");
    }
    html.push_str("</section>");
    html
}

#[derive(Template)]
#[template(source = "<main><p>factory {{ version }} ok</p></main>", ext = "html")]
struct HealthTemplate<'a> {
    version: &'a str,
}

/// Handler-shaped health fragment: proves Askama renders in-process.
pub fn web_health_response() -> String {
    HealthTemplate {
        version: env!("CARGO_PKG_VERSION"),
    }
    .render()
    .expect("health template renders")
}

/// Board stylesheet (spec #27 Bea-first): hand-written embedded CSS — the
/// binary is hermetic, so no frameworks and no external fonts. Trello-style
/// lanes: horizontal strip, fixed-width lanes with their own vertical
/// scroll. Selectors hook the same `data-*` attributes the tests pin.
const SITE_CSS: &str = "*{box-sizing:border-box}body{margin:0;background:#faf8f4;color:#2b2721;font-family:-apple-system,BlinkMacSystemFont,\"Segoe UI\",Roboto,Helvetica,Arial,sans-serif;line-height:1.45}\n\
header.site{padding:20px 28px 12px;border-bottom:1px solid #e5e1d8}\n\
header.site h1{margin:0;font-size:22px;letter-spacing:-.01em}\n\
header.site p{margin:4px 0;color:#6b655a;font-size:14px}\n\
header.site nav{margin-top:10px;font-size:14px}\n\
header.site nav a{color:#3b6fd4;text-decoration:none;margin-right:4px}\n\
header.site nav a:hover{text-decoration:underline}\n\
main{padding:20px 28px 96px;max-width:1280px}\n\
code{background:#f1ede4;border-radius:4px;padding:1px 6px;font-size:.92em}\n\
pre{background:#232228;color:#e8e6e1;border-radius:10px;padding:14px 16px;overflow-x:auto;font-size:12.5px;line-height:1.5;max-width:100%}\n\
table{border-collapse:collapse;font-size:14px}\n\
th{text-align:left;font-size:11px;text-transform:uppercase;letter-spacing:.08em;color:#6b655a;padding:8px 10px;border-bottom:2px solid #e5e1d8}\n\
td{padding:8px 10px;border-bottom:1px solid #ece7db;vertical-align:top}\n\
button,input[type=submit]{font:inherit;font-weight:600;border:1px solid #d8d2c4;background:#fff;color:#2b2721;border-radius:999px;padding:8px 18px;cursor:pointer}\n\
button:hover,input[type=submit]:hover{background:#f1ede4}\n\
input[type=text],input:not([type]),textarea,select{font:inherit;border:1px solid #d8d2c4;border-radius:8px;padding:8px 12px;background:#fff;color:#2b2721;max-width:100%}\n\
form{margin:12px 0}\n\
label{display:block;font-size:13px;font-weight:600;margin:10px 0 4px;color:#4a4438}\n\
details{margin:12px 0}\n\
summary{cursor:pointer;color:#4a4438;font-weight:600}\n\
[data-board-columns]{display:flex;gap:16px;overflow-x:auto;padding-bottom:16px;scroll-snap-type:x proximity}\n\
[data-column]{flex:0 0 300px;scroll-snap-align:start;max-height:calc(100vh - 240px);overflow-y:auto;background:#fff;border:1px solid #e5e1d8;border-radius:12px;padding:12px;margin:0}\n\
[data-column]>h2{margin:0 0 10px;font-size:13px;text-transform:uppercase;letter-spacing:.06em;color:#6b655a}\n\
[data-card]{background:#fffdf8;border:1px solid #e5e1d8;border-left:4px solid #b9b2a4;border-radius:8px;padding:10px 12px;margin:0 0 10px}\n\
[data-column=\"needs-you\"] [data-card]{border-left-color:#c2410c}\n\
[data-column=\"review\"] [data-card]{border-left-color:#6d5bd0}\n\
[data-column=\"done\"] [data-card]{border-left-color:#4d7c0f}\n\
[data-column=\"running\"] [data-card]{border-left-color:#0284c7}\n\
[data-card] h3{margin:0 0 6px;font-size:15px}\n\
[data-card] [data-status]{display:inline-block;margin:0 0 6px;font-size:12px;font-weight:600;background:#f1ede4;border-radius:999px;padding:2px 10px;color:#4a4438}\n\
[data-card] [data-cost],[data-card] [data-changed]{margin:2px 0;font-size:13px;color:#6b655a}\n\
[data-card] [data-detail-link]{display:inline-block;margin-top:8px;font-size:13px;font-weight:600;color:#3b6fd4;text-decoration:none}\n\
[data-empty-column],[data-board-empty] p,[data-attempts-empty] p,[data-git-empty] p,[data-empty-tree],[data-no-train] p,[data-onboard-empty]{color:#8a8378;font-size:14px;font-style:italic}\n\
[data-ticket]>h2{margin-top:0}\n\
[data-ticket-body]{background:#fff;border:1px solid #e5e1d8;border-radius:12px;padding:14px 18px;max-width:860px}\n\
ol[data-attempts]{list-style:none;margin:16px 0 0;padding:0;max-width:960px}\n\
li[data-attempt]{border-left:3px solid #d8d2c4;padding:0 0 4px 16px;margin:0 0 16px}\n\
li[data-attempt] [data-summary]{font-weight:600;margin:0 0 8px}\n\
[data-truncated]{font-size:13px;color:#8a8378}\n\
[data-attempt]{background:#fff;border:1px solid #e5e1d8;border-radius:12px;padding:12px 16px;margin:0 0 12px;max-width:960px}\n\
[data-attempt]>h3{margin:0 0 8px;font-size:15px}\n\
[data-attempt][data-status*=\"fail\"]{border-left:4px solid #b91c1c}\n\
[data-attempt][data-status*=\"accept\"],[data-attempt][data-status*=\"done\"]{border-left:4px solid #4d7c0f}\n\
[data-stop-controls]{display:flex;gap:10px}\n\
[data-stop-controls] form{margin:0}\n\
[data-stop=\"halted\"]{background:#fef2f2;border:1px solid #fecaca;border-radius:10px;padding:10px 14px;max-width:960px}\n\
[data-stop=\"running\"]{color:#4d7c0f}\n\
[data-where]{font-size:13px;color:#6b655a}\n\
[data-worktrees]{margin-top:20px;max-width:960px}\n\
[data-worktrees] ul{list-style:none;padding:0}\n\
[data-worktrees] li{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12.5px;background:#fff;border:1px solid #e5e1d8;border-radius:8px;padding:6px 10px;margin:0 0 6px;overflow-x:auto;white-space:nowrap}\n\
[data-git-log],[data-branches]{max-width:960px}\n\
[data-commit],[data-branch]{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12.5px;background:#fff;border:1px solid #e5e1d8;border-radius:8px;padding:6px 10px;margin:0 0 6px;overflow-x:auto;white-space:nowrap}\n\
[data-file-view] pre{max-width:1100px}\n\
[data-diff]{max-width:1100px}\n\
[data-member]{background:#fff;border:1px solid #e5e1d8;border-radius:10px;padding:10px 14px;margin:0 0 10px;max-width:960px}\n\
[data-member-verdict]{display:inline-block;font-size:12px;font-weight:700;border-radius:999px;padding:2px 10px;background:#f1ede4}\n\
[data-member-verdict*=\"accept\"]{background:#dcfce7;color:#365314}\n\
[data-done-feed]{margin-top:16px}\n\
[data-done-feed] summary{cursor:pointer;color:#6b655a;font-size:14px}\n\
\n";
/// Full board page: columns plus the polling hook that refreshes only
/// the fragment (no full reloads). Pure over columns for testing.
pub fn board_page_html(columns: &[BoardColumn]) -> String {
    let fragment = board_fragment_html(columns);
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
    <title>Factory board</title><style>{}</style></head>\
    <body><header class=\"site\"><h1>Factory board</h1>\
    <p>Bea's home for the work being built. Matches <code>factory status</code> + <code>factory log</code>.</p>\
    <nav><a href=\"/\">Board</a> | <a href=\"/dispatch\">Dispatcher live</a> | <a href=\"/git\">Git</a> | <a href=\"/files\">Files</a> | <a href=\"/train\">Train review</a></nav>\
    <p data-version>factory {version}</p></header>\
    <main><div id=\"board\" data-board-fragment=\"/fragments/board\" data-poll-ms=\"{}\">\
    {fragment}</div></main>\
    <script>(function(){{var el=document.getElementById('board');if(!el)return;var url=el.getAttribute('data-board-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'15000',10);\
    setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
    </body></html>",
        SITE_CSS,
        BOARD_POLL_MS,
    )
}

async fn board_page_handler() -> axum::response::Html<String> {
    axum::response::Html(board_page_html(&board_data()))
}

async fn board_fragment_handler() -> axum::response::Html<String> {
    axum::response::Html(board_fragment_html(&board_data()))
}

// --- ticket detail (ticket #30, spec #27) ---------------------------------
//
// Bea taps a board card and lands here: the issue body in plain words,
// then the per-ticket attempt history newest-first with the outcome
// summary (the story, failure cause first) before the bounded transcript
// tail (the evidence). Same read-only contract as the board and `log`:
// `gh issue view` via `run_gh_output` (FACTORY_GH_BIN-overridable), ledger
// + logs via `dispatch_state_dir` (FACTORY_STATE_DIR-overridable). No git
// calls, no writes. Rendering is pure over `TicketDetail` fixtures so
// endpoint tests assert markers without scraping a browser.

/// Transcript lines shown per attempt on the detail page. Matches
/// LOG_DEFAULT_TAIL so the page and `factory log <n>` tell the same
/// bounded story; callers pass an explicit `tail` (0 means full) and the
/// views carry a `truncated` flag whenever older lines were dropped.
pub const TICKET_DETAIL_TAIL: usize = 50;

/// One attempt as the detail page sees it: summary first, then the
/// bounded transcript tail. Field-for-field the `factory log --json` row
/// so prose and JSON shapes carry the same facts.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketAttemptView {
    pub attempt_no: u64,
    pub status: String,
    pub summary: String,
    pub base: String,
    pub head: String,
    pub started: String,
    pub finished: String,
    pub exit_code: i32,
    pub transcript: String,
    pub truncated: bool,
}

/// One ticket as the detail page sees it: body plus attempt history.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketDetail {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub state: String,
    pub attempts: Vec<TicketAttemptView>,
}

/// Calm sentence when the ticket has no attempts yet (Bea: ready, not broken).
pub fn ticket_no_attempts_sentence() -> &'static str {
    "No attempts yet \u{2014} this ticket is waiting to be picked up."
}

/// Calm sentence when `gh` has no such issue (never a bare 500).
pub fn ticket_missing_sentence(raw: &str) -> String {
    format!(
        "There is no ticket {raw} that the factory can see \u{2014} it may be closed, mistyped, or outside this repo."
    )
}

/// Read-only single-issue fetch via the `gh` seam. `None` on any failure
/// (bad number, missing CLI, unknown issue): the page renders its calm
/// missing-ticket sentence instead of a 500. No writes, no side effects.
pub fn fetch_ticket_issue(number: u64) -> Option<BoardIssue> {
    let num = number.to_string();
    let out = match run_gh_output(&[
        "issue",
        "view",
        &num,
        "--json",
        "number,title,body,labels,state",
    ]) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return None,
    };
    let v: serde_json::Value = serde_json::from_str(&out).ok()?;
    let n = v.get("number").and_then(|n| n.as_u64())?;
    if n == 0 {
        return None;
    }
    Some(BoardIssue {
        number: n,
        title: v
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        body: v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .to_string(),
        labels: gh_labels(&v),
        state: v
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or("open")
            .to_string(),
    })
}

/// Read-only per-ticket attempt history, newest-first, summaries before
/// bounded transcripts: the same rows `cmd_log` prints, as structs. Sort
/// order matches `cmd_log` exactly (attempt_no, then id, descending).
/// Missing/unreadable ledger reads as no attempts. Read-only.
pub fn ticket_attempt_views(ticket: u64, tail: usize) -> Vec<TicketAttemptView> {
    let state = dispatch_state_dir();
    let ledger = load_ledger(&state);
    let mut entries: Vec<&DispatchAttempt> = ledger
        .attempts
        .iter()
        .filter(|a| a.ticket == ticket)
        .collect();
    entries.sort_by(|a, b| b.attempt_no.cmp(&a.attempt_no).then(b.id.cmp(&a.id)));
    entries
        .into_iter()
        .map(|a| {
            let full = log_read_transcript(&state, a);
            let (shown, truncated) = log_tail(&full, tail);
            TicketAttemptView {
                attempt_no: a.attempt_no,
                status: a.status.clone(),
                summary: log_summary(a),
                base: a.base_commit.clone(),
                head: a.head_commit.clone(),
                started: a.started_at.clone(),
                finished: a.finished_at.clone(),
                exit_code: a.exit_code,
                transcript: shown,
                truncated,
            }
        })
        .collect()
}

/// Read-only detail assembly: issue body plus attempt history. `None`
/// when `gh` has no such issue; `Some` with possibly-empty attempts
/// otherwise. Read-only.
pub fn ticket_detail(number: u64, tail: usize) -> Option<TicketDetail> {
    fetch_ticket_issue(number).map(|issue| TicketDetail {
        number: issue.number,
        title: issue.title,
        body: issue.body,
        labels: issue.labels,
        state: issue.state,
        attempts: ticket_attempt_views(number, tail),
    })
}

/// JSON shape for the detail: the same facts as the prose page with the
/// same bounded transcripts, so machines and Bea read one story.
pub fn ticket_detail_json(detail: &TicketDetail) -> serde_json::Value {
    let attempts: Vec<serde_json::Value> = detail
        .attempts
        .iter()
        .map(|a| {
            serde_json::json!({
                "attempt": a.attempt_no,
                "slot": format!("T{}-a{}", detail.number, a.attempt_no),
                "status": a.status,
                "summary": a.summary,
                "base": a.base,
                "head": a.head,
                "started": a.started,
                "finished": a.finished,
                "exit_code": a.exit_code,
                "transcript": a.transcript,
                "truncated": a.truncated,
            })
        })
        .collect();
    serde_json::json!({
        "ticket": detail.number,
        "title": detail.title,
        "body": detail.body,
        "labels": detail.labels,
        "state": detail.state,
        "attempts": attempts,
    })
}

/// Fragment HTML for one ticket (the polling unit). Pure over the detail
/// so tests assert markers without a server: `data-ticket` plus
/// `data-ticket-body`, per-attempt `data-attempt` with `data-summary`
/// before `data-transcript`, and `data-truncated` naming the bound when
/// older lines were dropped.
pub fn ticket_detail_fragment_html(detail: &TicketDetail) -> String {
    let mut html = String::new();
    html.push_str(&format!(
        "<section data-ticket=\"{}\"><h2>Ticket #{} {}</h2><div data-ticket-body>{}</div>",
        detail.number,
        detail.number,
        escape_html(&detail.title),
        escape_html(&detail.body),
    ));
    if detail.attempts.is_empty() {
        html.push_str(&format!(
            "<p data-no-attempts>{}</p>",
            ticket_no_attempts_sentence()
        ));
    } else {
        html.push_str("<ol data-attempts>");
        for a in &detail.attempts {
            html.push_str(&format!(
                "<li data-attempt=\"{}\"><p data-summary>{}</p>",
                a.attempt_no,
                escape_html(&a.summary),
            ));
            if a.transcript.is_empty() {
                html.push_str("<p data-transcript-empty>(no transcript yet)</p>");
            } else {
                html.push_str(&format!(
                    "<pre data-transcript>{}</pre>",
                    escape_html(&a.transcript)
                ));
            }
            if a.truncated {
                let shown = a.transcript.lines().count();
                html.push_str(&format!(
                    "<p data-truncated>Showing the last {shown} lines \u{2014} older lines are hidden. Full transcript: <code>factory log {} --tail 0</code>.</p>",
                    detail.number
                ));
            }
            html.push_str("</li>");
        }
        html.push_str("</ol>");
    }
    html
}
// --- dispatcher live (ticket #31, spec #27) ---------------------------------
//
// The status/tail/worktree ritual as one polling view: live attempts in
// slot lanes, STOP state with STOP/Resume control, per-attempt streaming
// log tails, worktree list. Same-binary Axum, server-rendered HTML,
// polling fragment (~3s), no websockets.
//
// Reads flow through the existing seams: the dispatch ledger via
// `dispatch_state_dir` (overridable with FACTORY_STATE_DIR), transcripts
// via the log-dir files the dispatcher writes, worktrees via
// `git worktree list` (`run_git`, overridable with FACTORY_GIT_BIN).
// Writes flow only through the loop controls: the POST handlers below
// call `cmd_dispatch_stop` / `cmd_dispatch_resume`, the same functions
// the CLI calls. GETs never mutate (snapshot-tested like the board).

/// Poll interval for the dispatcher fragment (spec: dispatcher-live ~3s).
pub const DISPATCHER_POLL_MS: u64 = 3_000;

/// Transcript tail (lines) per attempt on the live screen. Matches the
/// `factory log` default: bounded context, newest lines stream.
pub const DISPATCHER_LOG_TAIL: usize = LOG_DEFAULT_TAIL;

/// One live attempt as the dispatcher screen sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatcherAttemptView {
    pub ticket: u64,
    pub attempt_no: u64,
    pub slot: String,
    pub status: String,
    pub summary: String,
    pub transcript_tail: String,
    pub truncated: bool,
    pub worktree: String,
    pub branch: String,
    pub started: String,
    pub finished: String,
}

/// The live screen: attempts in slot lanes + STOP state + worktrees.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatcherData {
    pub attempts: Vec<DispatcherAttemptView>,
    pub stopped: bool,
    pub worktrees: Vec<String>,
    pub max_concurrent: usize,
}

/// Calm sentence for zero attempts (ready, not broken).
pub fn dispatcher_empty_sentence() -> &'static str {
    "No attempts yet \u{2014} the dispatcher is idle and ready for the next ticket."
}

/// Calm sentence for an attempt with no log lines yet.
pub fn dispatcher_no_transcript_sentence() -> &'static str {
    "No transcript yet \u{2014} the attempt has not written any log lines."
}

/// Calm sentence for zero checked-out worktrees.
pub fn dispatcher_no_worktrees_sentence() -> &'static str {
    "No worktrees checked out right now \u{2014} nothing is running, so there is nothing to tidy."
}

/// Read-only: `git worktree list` lines. Failures read as none (the
/// screen renders its calm empty state, never a 500 for missing git).
fn dispatcher_worktree_lines() -> Vec<String> {
    run_git(&["worktree", "list"], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .filter(|l| !l.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Read-only live assembly: ledger x transcripts x STOP x worktrees.
/// Missing/unreadable ledger reads as idle (no attempts, running).
pub fn dispatcher_data() -> DispatcherData {
    let state = dispatch_state_dir();
    let ledger = load_ledger(&state);
    let stopped = state.join("STOP").is_file();
    let worktrees = dispatcher_worktree_lines();
    let mut attempts = Vec::with_capacity(ledger.attempts.len());
    for a in &ledger.attempts {
        let slot = format!("T{}-a{}", a.ticket, a.attempt_no);
        let full = log_read_transcript(&state, a);
        let (tail, truncated) = log_tail(&full, DISPATCHER_LOG_TAIL);
        attempts.push(DispatcherAttemptView {
            ticket: a.ticket,
            attempt_no: a.attempt_no,
            slot,
            status: a.status.clone(),
            summary: log_summary(a),
            transcript_tail: tail,
            truncated,
            worktree: a.worktree.clone(),
            branch: a.branch.clone(),
            started: a.started_at.clone(),
            finished: a.finished_at.clone(),
        });
    }
    DispatcherData {
        attempts,
        stopped,
        worktrees,
        max_concurrent: dispatch_max_concurrent(),
    }
}

/// Fragment HTML for the live screen (the polling unit). Pure over data
/// so tests assert markers without a server.
pub fn dispatcher_fragment_html(data: &DispatcherData) -> String {
    let mut html = String::new();
    html.push_str("<section data-dispatch>");
    if data.stopped {
        html.push_str("<p data-stop=\"halted\">STOP: dispatch loop is halted (STOP file present). Resume to let the loop schedule again.</p>");
    } else {
        html.push_str("<p data-stop=\"running\">STOP: dispatch loop running (no STOP file).</p>");
    }
    html.push_str("<div data-stop-controls><form method=\"post\" action=\"/dispatch/stop\" data-stop-form><button type=\"submit\">STOP the loop</button></form>");
    html.push_str("<form method=\"post\" action=\"/dispatch/resume\" data-resume-form><button type=\"submit\">Resume the loop</button></form></div>");
    html.push_str(&format!(
        "<p data-cli>Matches <code>factory dispatch status</code> + <code>factory log &lt;ticket&gt; --tail</code> + <code>git worktree list</code>. Brake: <code>factory dispatch stop</code> / <code>factory dispatch resume</code>. Slots: up to {} concurrent.</p>",
        data.max_concurrent
    ));
    if data.attempts.is_empty() {
        html.push_str(&format!(
            "<section data-attempts-empty><p>{}</p></section>",
            dispatcher_empty_sentence()
        ));
    } else {
        html.push_str("<section data-attempts>");
        for a in &data.attempts {
            html.push_str(&format!(
                "<article data-attempt=\"{}\" data-slot=\"{}\" data-ticket=\"{}\" data-status=\"{}\"><h3>#{ticket} attempt {n} ({slot}) \u{2014} {status}</h3><p data-summary>{summary}</p>",
                escape_html(&a.slot),
                escape_html(&a.slot),
                a.ticket,
                escape_html(&a.status),
                ticket = a.ticket,
                n = a.attempt_no,
                slot = escape_html(&a.slot),
                status = escape_html(&a.status),
                summary = escape_html(&a.summary),
            ));
            if !a.branch.is_empty() || !a.worktree.is_empty() {
                html.push_str(&format!(
                    "<p data-where>Branch <code>{}</code> at <code>{}</code>. Tail: <code>factory log {} --tail {}</code>.</p>",
                    escape_html(&a.branch),
                    escape_html(&a.worktree),
                    a.ticket,
                    DISPATCHER_LOG_TAIL,
                ));
            }
            if a.transcript_tail.is_empty() {
                html.push_str(&format!(
                    "<p data-no-transcript>{}</p>",
                    dispatcher_no_transcript_sentence()
                ));
            } else {
                if a.truncated {
                    html.push_str(&format!(
                        "<p data-truncated>(showing last {} lines)</p>",
                        DISPATCHER_LOG_TAIL
                    ));
                }
                html.push_str(&format!(
                    "<pre data-log-tail>{}</pre>",
                    escape_html(&a.transcript_tail)
                ));
            }
            html.push_str("</article>");
        }
        html.push_str("</section>");
    }
    html.push_str("<section data-worktrees><h2>Worktrees</h2>");
    if data.worktrees.is_empty() {
        html.push_str(&format!(
            "<p data-no-worktrees>{}</p>",
            dispatcher_no_worktrees_sentence()
        ));
    } else {
        html.push_str("<ul>");
        for w in &data.worktrees {
            html.push_str(&format!("<li>{}</li>", escape_html(w)));
        }
        html.push_str("</ul>");
    }
    html.push_str("</section>");
    html.push_str("</section>");
    html
}

/// Calm missing-ticket fragment (Bea: a sentence, never a bare 500).
pub fn ticket_missing_fragment_html(raw: &str) -> String {
    format!(
        "<section data-ticket-missing><p>{}</p></section>",
        escape_html(&ticket_missing_sentence(raw))
    )
}

/// Full detail page: body plus attempt history with the polling hook that
/// refreshes only the fragment (no full reloads). Pure over the detail
/// for testing.
pub fn ticket_detail_page_html(detail: &TicketDetail) -> String {
    let fragment = ticket_detail_fragment_html(detail);
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Ticket #{} \u{2014} Factory</title><style>{}</style></head>\
<body><header class=\"site\"><h1>Ticket #{}</h1>\
<p>Body, attempts, and transcript tails. Matches <code>factory log {}</code>.</p>\
<p data-version>factory {version}</p></header>\
<main><div id=\"ticket\" data-ticket-fragment=\"/fragments/ticket/{}\" data-poll-ms=\"{}\">\
{fragment}</div></main>\
<script>(function(){{var el=document.getElementById('ticket');if(!el)return;var url=el.getAttribute('data-ticket-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'15000',10);\
setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
</body></html>",
        detail.number, SITE_CSS,
        detail.number, detail.number, detail.number, BOARD_POLL_MS,
    )
}

/// Calm missing-ticket page (404 body, still a sentence).
pub fn ticket_missing_page_html(raw: &str) -> String {
    let fragment = ticket_missing_fragment_html(raw);
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Ticket not found \u{2014} Factory</title><style>{}</style></head>\
<body><header class=\"site\"><h1>Ticket not found</h1></header><main>{fragment}</main></body></html>",
        SITE_CSS,
    )
}

async fn ticket_page_handler(
    axum::extract::Path(raw): axum::extract::Path<String>,
) -> (axum::http::StatusCode, axum::response::Html<String>) {
    match parse_issue_number(&raw)
        .ok()
        .and_then(|n| ticket_detail(n, TICKET_DETAIL_TAIL))
    {
        Some(detail) => (
            axum::http::StatusCode::OK,
            axum::response::Html(ticket_detail_page_html(&detail)),
        ),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            axum::response::Html(ticket_missing_page_html(&raw)),
        ),
    }
}

async fn ticket_fragment_handler(
    axum::extract::Path(raw): axum::extract::Path<String>,
) -> (axum::http::StatusCode, axum::response::Html<String>) {
    match parse_issue_number(&raw)
        .ok()
        .and_then(|n| ticket_detail(n, TICKET_DETAIL_TAIL))
    {
        Some(detail) => (
            axum::http::StatusCode::OK,
            axum::response::Html(ticket_detail_fragment_html(&detail)),
        ),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            axum::response::Html(ticket_missing_fragment_html(&raw)),
        ),
    }
}

async fn ticket_api_handler(
    axum::extract::Path(raw): axum::extract::Path<String>,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    match parse_issue_number(&raw)
        .ok()
        .and_then(|n| ticket_detail(n, TICKET_DETAIL_TAIL))
    {
        Some(detail) => (
            axum::http::StatusCode::OK,
            axum::Json(ticket_detail_json(&detail)),
        ),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": ticket_missing_sentence(&raw)})),
        ),
    }
}

/// Handler shape for UI tickets: board page + polling fragment + health,
/// plus the ticket detail slice (page + fragment + JSON). All GETs are
/// read-only observers (same contract as `status`/`log`).
/// Full dispatcher page: live fragment plus the polling hook that
/// refreshes only the fragment (no full reloads). Pure over data.
pub fn dispatcher_page_html(data: &DispatcherData) -> String {
    let fragment = dispatcher_fragment_html(data);
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Factory dispatcher</title><style>{}</style></head>\
<body><header class=\"site\"><h1>Factory dispatcher</h1>\
<p>The status/tail/worktree ritual as one live view. Matches <code>factory dispatch status</code> + <code>factory log &lt;ticket&gt; --tail</code> + <code>git worktree list</code>.</p>\
<nav><a href=\"/\">Board</a> | <a href=\"/dispatch\">Dispatcher live</a> | <a href=\"/git\">Git</a> | <a href=\"/files\">Files</a> | <a href=\"/train\">Train review</a></nav>\
<p data-version>factory {version}</p></header>\
<main><div id=\"dispatch\" data-dispatch-fragment=\"/fragments/dispatch\" data-poll-ms=\"{}\">\
{fragment}</div></main>\
<script>(function(){{var el=document.getElementById('dispatch');if(!el)return;var url=el.getAttribute('data-dispatch-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'3000',10);\
setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
</body></html>",
        SITE_CSS,
        DISPATCHER_POLL_MS,
    )
}

/// GET /dispatch: the live screen. Read-only observer.
pub async fn dispatcher_page_handler() -> axum::response::Html<String> {
    axum::response::Html(dispatcher_page_html(&dispatcher_data()))
}

/// GET /fragments/dispatch: the polling unit. Read-only observer.
pub async fn dispatcher_fragment_handler() -> axum::response::Html<String> {
    axum::response::Html(dispatcher_fragment_html(&dispatcher_data()))
}

/// POST /dispatch/stop: halt the loop through the loop controls (the
/// same `cmd_dispatch_stop` the CLI calls), then back to the screen.
pub async fn dispatcher_stop_handler() -> axum::response::Redirect {
    let _ = cmd_dispatch_stop(false);
    axum::response::Redirect::to("/dispatch")
}

/// POST /dispatch/resume: restart the loop through the loop controls
/// (the same `cmd_dispatch_resume` the CLI calls), then back.
pub async fn dispatcher_resume_handler() -> axum::response::Redirect {
    let _ = cmd_dispatch_resume(false);
    axum::response::Redirect::to("/dispatch")
}

// --- git + files views (ticket #33, spec #27) --------------------------------
//
// Code awareness as one polling view: the log graph with diff stats,
// branches with tracking state, worktrees with ticket links, plus the
// file tree + highlighted viewer + search from any branch context.
// Same-binary Axum, server-rendered HTML, polling fragments (~15s),
// no websockets.
//
// Reads flow through the `git` runner seam (`run_git`, overridable with
// FACTORY_GIT_BIN): `log` + `show --stat`, `branch -vv`,
// `worktree list --porcelain`, `ls-tree`, `show <ref>:<path>`,
// `grep -n`. GETs never mutate (snapshot-tested like the board).
// The viewer is copy-only: a copy button over an escaped `<pre>` block
// with no forms, no textarea, no contenteditable — edits stay outside
// the ticket flow (freeze drift), per the spec.

/// Poll interval for the git fragment (board-class panel: ~15s).
pub const GIT_POLL_MS: u64 = 15_000;

/// Poll interval for the files tree fragment (board-class panel: ~15s).
pub const FILES_POLL_MS: u64 = 15_000;

/// Commits read per git view render. Bounds `git log` plus the
/// per-commit `git show --stat` fan-out (one subprocess per commit).
pub const GIT_LOG_LIMIT: usize = 30;

/// Default ref for the files view; any branch name works as `?ref=`.
pub const FILE_REF_DEFAULT: &str = "HEAD";

/// File viewer caps: body bytes kept, search hits kept, tree rows kept.
/// Bounds `git show` / `git grep` / `git ls-tree` output per render.
pub const FILE_VIEW_MAX_BYTES: usize = 200_000;
pub const FILE_SEARCH_MAX_HITS: usize = 50;
pub const FILES_TREE_MAX: usize = 2000;

/// One log row as the git view sees it: ticket links + diff stats.
#[derive(Debug, Clone, PartialEq)]
pub struct GitCommit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub ticket_refs: Vec<u64>,
    pub stat: String,
}

/// One branch as the git view sees it: upstream + tracking state.
#[derive(Debug, Clone, PartialEq)]
pub struct GitBranch {
    pub name: String,
    pub upstream: String,
    pub tracking: String,
    pub is_current: bool,
    pub ticket_ref: Option<u64>,
}

/// One worktree as the git view sees it, with its ticket link.
#[derive(Debug, Clone, PartialEq)]
pub struct GitWorktree {
    pub path: String,
    pub branch: String,
    pub head: String,
    pub ticket_ref: Option<u64>,
}

/// The git view: log graph + branches + worktrees.
#[derive(Debug, Clone, PartialEq)]
pub struct GitData {
    pub commits: Vec<GitCommit>,
    pub branches: Vec<GitBranch>,
    pub worktrees: Vec<GitWorktree>,
}

/// Ticket numbers referenced by free text: `#<n>` plus the branch forms
/// `factory/T<n>` and `(T<n>)` subjects. Deduped, ascending. Pure.
pub fn extract_ticket_refs(text: &str) -> Vec<u64> {
    let bytes = text.as_bytes();
    let mut out: Vec<u64> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let is_hash = c == b'#';
        let is_branch_t = (c == b'T' || c == b't')
            && i > 0
            && (bytes[i - 1] == b'/' || bytes[i - 1] == b'(')
            && i + 1 < bytes.len()
            && bytes[i + 1].is_ascii_digit();
        let start = if is_hash || is_branch_t {
            Some(i + 1)
        } else {
            None
        };
        if let Some(s) = start {
            let mut j = s;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > s {
                if let Ok(n) = text[s..j].parse::<u64>() {
                    if n > 0 && !out.contains(&n) {
                        out.push(n);
                    }
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out.sort_unstable();
    out
}

/// Parse one `git branch -vv` line into name, upstream, and tracking
/// state (`ahead 1`, `behind 2`, `in sync`, `upstream gone`,
/// `no upstream`, `detached`). `None` on blank input. Pure.
pub fn parse_git_branch_vv_line(line: &str) -> Option<GitBranch> {
    let trimmed = line.trim_end();
    if trimmed.trim().is_empty() {
        return None;
    }
    let is_current = trimmed.starts_with('*');
    let rest = trimmed.get(1..)?.trim_start();
    if rest.starts_with('(') {
        return Some(GitBranch {
            name: "(detached)".to_string(),
            upstream: String::new(),
            tracking: "detached".to_string(),
            is_current,
            ticket_ref: None,
        });
    }
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = rest[..name_end].to_string();
    if name.is_empty() {
        return None;
    }
    let after_name = rest[name_end..].trim_start();
    let mut upstream = String::new();
    let mut tracking = "no upstream".to_string();
    if let Some(open) = after_name.find('[') {
        if let Some(close) = after_name[open..].find(']') {
            let inside = &after_name[open + 1..open + close];
            let mut split = inside.splitn(2, ':');
            upstream = split.next().unwrap_or("").trim().to_string();
            match split.next() {
                None => tracking = "in sync".to_string(),
                Some(state) if state.trim() == "gone" => {
                    tracking = "upstream gone".to_string();
                }
                Some(state) => tracking = state.trim().to_string(),
            }
        }
    }
    let ticket_ref = extract_ticket_refs(&name).into_iter().next();
    Some(GitBranch {
        name,
        upstream,
        tracking,
        is_current,
        ticket_ref,
    })
}

/// Parse `git worktree list --porcelain` into path/branch/HEAD rows
/// (bare markers ignored, detached heads shown as such). Pure.
pub fn parse_git_worktree_porcelain(text: &str) -> Vec<GitWorktree> {
    let mut out = Vec::new();
    let mut path = String::new();
    let mut head = String::new();
    let mut branch = String::new();
    for line in text.lines().chain(std::iter::once("")) {
        let line = line.trim();
        if line.is_empty() {
            if !path.is_empty() || !head.is_empty() {
                let display = branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch.as_str())
                    .to_string();
                let ticket_ref = extract_ticket_refs(&display).into_iter().next();
                out.push(GitWorktree {
                    path: std::mem::take(&mut path),
                    branch: if display.is_empty() {
                        "(detached)".to_string()
                    } else {
                        display
                    },
                    head: std::mem::take(&mut head),
                    ticket_ref,
                });
                branch.clear();
            }
            continue;
        }
        if let Some(p) = line.strip_prefix("worktree ") {
            path = p.trim().to_string();
        } else if let Some(h) = line.strip_prefix("HEAD ") {
            head = h.trim().to_string();
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = b.trim().to_string();
        } else if line == "detached" {
            branch.clear();
        }
    }
    out
}

/// Read-only: newest log rows with per-commit diff stats. Any `git`
/// failure reads as no commits (calm empty state, never a 500).
pub fn git_log_data(limit: usize) -> Vec<GitCommit> {
    let limit = limit.to_string();
    let out = match run_git(
        &[
            "log",
            "--pretty=format:%H|%h|%an|%ad|%s",
            "--date=short",
            "-n",
            &limit,
        ],
        None,
    ) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return Vec::new(),
    };
    let mut commits = Vec::new();
    for line in out.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut cols = line.splitn(5, '|');
        let hash = cols.next().unwrap_or("").to_string();
        let short = cols.next().unwrap_or("").to_string();
        let author = cols.next().unwrap_or("").to_string();
        let date = cols.next().unwrap_or("").to_string();
        let subject = cols.next().unwrap_or("").to_string();
        if hash.is_empty() {
            continue;
        }
        let ticket_refs = extract_ticket_refs(&subject);
        let stat = run_git(&["show", "--stat", "--oneline", "--format=", &hash], None)
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .take(12)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        commits.push(GitCommit {
            hash,
            short,
            author,
            date,
            subject,
            ticket_refs,
            stat,
        });
    }
    commits
}

/// Read-only: `git branch -vv` rows. Failures read as none.
pub fn git_branch_data() -> Vec<GitBranch> {
    run_git(&["branch", "-vv"], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(parse_git_branch_vv_line)
                .collect()
        })
        .unwrap_or_default()
}

/// Read-only: `git worktree list --porcelain` rows. Failures read as none.
pub fn git_worktree_data() -> Vec<GitWorktree> {
    run_git(&["worktree", "list", "--porcelain"], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_git_worktree_porcelain(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Read-only git assembly: log x branches x worktrees.
pub fn git_data() -> GitData {
    GitData {
        commits: git_log_data(GIT_LOG_LIMIT),
        branches: git_branch_data(),
        worktrees: git_worktree_data(),
    }
}

/// JSON shape for the git view: the same facts as the prose page, so
/// machines and the maintainer read one story.
pub fn git_json(data: &GitData) -> serde_json::Value {
    serde_json::json!({
        "commits": data.commits.iter().map(|c| serde_json::json!({
            "hash": c.hash,
            "short": c.short,
            "author": c.author,
            "date": c.date,
            "subject": c.subject,
            "tickets": c.ticket_refs,
            "stat": c.stat,
        })).collect::<Vec<_>>(),
        "branches": data.branches.iter().map(|b| serde_json::json!({
            "name": b.name,
            "upstream": b.upstream,
            "tracking": b.tracking,
            "current": b.is_current,
            "ticket": b.ticket_ref,
        })).collect::<Vec<_>>(),
        "worktrees": data.worktrees.iter().map(|w| serde_json::json!({
            "path": w.path,
            "branch": w.branch,
            "head": w.head,
            "ticket": w.ticket_ref,
        })).collect::<Vec<_>>(),
    })
}

/// Calm sentence for a fully empty git view (ready, not broken).
pub fn git_empty_sentence() -> &'static str {
    "No git history in view \u{2014} the repo may be fresh, or git may be unavailable."
}

/// Fragment HTML for the git view (the polling unit). Pure over data so
/// tests assert markers without a server: `data-git-log` with per-commit
/// `data-commit` + `data-stat`, `data-branches` with per-branch
/// `data-branch` + `data-tracking`, `data-worktrees` with per-worktree
/// `data-worktree`, and `data-ticket` links into the ticket detail.
pub fn git_fragment_html(data: &GitData) -> String {
    fn ticket_links(refs: &[u64]) -> String {
        let mut s = String::new();
        for t in refs {
            s.push_str(&format!(
                "<a data-ticket=\"{t}\" href=\"/tickets/{t}\">#{t}</a> "
            ));
        }
        s
    }
    let mut html = String::new();
    html.push_str("<section data-git>");
    html.push_str("<p data-cli>Matches <code>git log --graph --decorate</code> + <code>git branch -vv</code> + <code>git worktree list</code>.</p>");
    if data.commits.is_empty() && data.branches.is_empty() && data.worktrees.is_empty() {
        html.push_str(&format!(
            "<section data-git-empty><p>{}</p></section>",
            git_empty_sentence()
        ));
    }
    html.push_str("<section data-git-log><h2>Log</h2>");
    if data.commits.is_empty() {
        html.push_str("<p data-no-commits>No commits in view right now.</p>");
    } else {
        for c in &data.commits {
            html.push_str(&format!(
                "<article data-commit=\"{}\"><h3>{} {}</h3><p data-meta>{} · {}</p>",
                escape_html(&c.short),
                escape_html(&c.short),
                escape_html(&c.subject),
                escape_html(&c.author),
                escape_html(&c.date),
            ));
            let links = ticket_links(&c.ticket_refs);
            if !links.is_empty() {
                html.push_str(&format!("<p data-tickets>{links}</p>"));
            }
            if c.stat.is_empty() {
                html.push_str("<p data-stat-empty>(no diff stats)</p>");
            } else {
                html.push_str(&format!("<pre data-stat>{}</pre>", escape_html(&c.stat)));
            }
            html.push_str("</article>");
        }
    }
    html.push_str("</section>");
    html.push_str("<section data-branches><h2>Branches</h2>");
    if data.branches.is_empty() {
        html.push_str("<p data-no-branches>No branches in view right now.</p>");
    } else {
        for b in &data.branches {
            html.push_str(&format!(
                "<article data-branch=\"{}\" data-current=\"{}\"><code>{}</code> ",
                escape_html(&b.name),
                b.is_current,
                escape_html(&b.name),
            ));
            if b.upstream.is_empty() {
                html.push_str("<span data-upstream>(no upstream)</span> ");
            } else {
                html.push_str(&format!(
                    "<span data-upstream>tracks <code>{}</code></span> ",
                    escape_html(&b.upstream)
                ));
            }
            html.push_str(&format!(
                "<span data-tracking>{}</span> ",
                escape_html(&b.tracking)
            ));
            if let Some(t) = b.ticket_ref {
                html.push_str(&format!(
                    "<a data-ticket=\"{t}\" href=\"/tickets/{t}\">#{t}</a>"
                ));
            }
            html.push_str("</article>");
        }
    }
    html.push_str("</section>");
    html.push_str("<section data-worktrees><h2>Worktrees</h2>");
    if data.worktrees.is_empty() {
        html.push_str("<p data-no-worktrees>None.</p>");
    } else {
        for w in &data.worktrees {
            html.push_str(&format!(
                "<article data-worktree=\"{}\"><code>{}</code> <span data-branch-name>branch <code>{}</code></span> <span data-head>at <code>{}</code></span> ",
                escape_html(&w.path),
                escape_html(&w.path),
                escape_html(&w.branch),
                escape_html(&w.head),
            ));
            if let Some(t) = w.ticket_ref {
                html.push_str(&format!(
                    "<a data-ticket=\"{t}\" href=\"/tickets/{t}\">#{t}</a>"
                ));
            }
            html.push_str("</article>");
        }
    }
    html.push_str("</section>");
    html.push_str("</section>");
    html
}

/// Full git page: the fragment plus the polling hook that refreshes only
/// the fragment (no full reloads). Pure over data for testing.
pub fn git_page_html(data: &GitData) -> String {
    let fragment = git_fragment_html(data);
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Factory git</title><style>{}</style></head>\
<body><header class=\"site\"><h1>Factory git</h1>\
<p>Log, branches, and worktrees with ticket links. Matches <code>git log --graph --decorate</code> + <code>git branch -vv</code> + <code>git worktree list</code>.</p>\
<nav><a href=\"/\">Board</a> | <a href=\"/dispatch\">Dispatcher live</a> | <a href=\"/git\">Git</a> | <a href=\"/files\">Files</a> | <a href=\"/train\">Train review</a></nav>\
<p data-version>factory {version}</p></header>\
<main><div id=\"git\" data-git-fragment=\"/fragments/git\" data-poll-ms=\"{}\">\
{fragment}</div></main>\
<script>(function(){{var el=document.getElementById('git');if(!el)return;var url=el.getAttribute('data-git-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'15000',10);\
setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
</body></html>",
        SITE_CSS,
        GIT_POLL_MS,
    )
}

/// GET /git: the code-awareness screen. Read-only observer.
pub async fn git_page_handler() -> axum::response::Html<String> {
    axum::response::Html(git_page_html(&git_data()))
}

/// GET /fragments/git: the polling unit. Read-only observer.
pub async fn git_fragment_handler() -> axum::response::Html<String> {
    axum::response::Html(git_fragment_html(&git_data()))
}

/// GET /api/git: machines read the same facts as JSON.
pub async fn git_api_handler() -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    (
        axum::http::StatusCode::OK,
        axum::Json(git_json(&git_data())),
    )
}

// --- files view (ticket #33, spec #27) --------------------------------------
//
// View-only file awareness from any branch context (`?ref=`, default
// HEAD): the tree (`git ls-tree`), the viewer (`git show <ref>:<path>`
// as escaped HTML with a copy button, never an editor), and search
// (`git grep -n`). Same read-only contract as the git view: failures
// render calm sentences, never 500s.

/// Query: `?ref=<branch>` (default HEAD) for the tree and the page.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct FilesTreeQuery {
    #[serde(alias = "ref")]
    pub gitref: Option<String>,
}

/// Query: `?ref=<branch>&path=<repo-path>` for the viewer.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct FilesViewQuery {
    #[serde(alias = "ref")]
    pub gitref: Option<String>,
    pub path: String,
}

/// Query: `?ref=<branch>&q=<text>` for search.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct FilesSearchQuery {
    #[serde(alias = "ref")]
    pub gitref: Option<String>,
    pub q: String,
}

fn clean_ref(raw: Option<&str>) -> String {
    match raw.map(str::trim) {
        None | Some("") => FILE_REF_DEFAULT.to_string(),
        Some(r) => r.to_string(),
    }
}

fn ref_is_safe(gitref: &str) -> bool {
    !gitref.is_empty()
        && !gitref.starts_with('-')
        && !gitref.bytes().any(|b| b.is_ascii_whitespace() || b == 0)
}

/// Keep a viewer path inside the repo: reject absolute paths, parent
/// escapes, and empty selections. Returns the cleaned path. Pure.
pub fn sanitize_repo_path(path: &str) -> Result<String, String> {
    let t = path.trim();
    if t.is_empty() {
        return Err(
            "invalid path: no file selected \u{2014} pick a file from the tree.".to_string(),
        );
    }
    if t.starts_with('/') {
        return Err(format!(
            "invalid path {t}: paths must stay inside the repo."
        ));
    }
    if t.contains('\\') {
        return Err(format!(
            "invalid path {t}: paths must stay inside the repo."
        ));
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in t.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(format!(
                "invalid path {t}: paths must stay inside the repo."
            ));
        }
        parts.push(seg);
    }
    if parts.is_empty() {
        return Err(
            "invalid path: no file selected \u{2014} pick a file from the tree.".to_string(),
        );
    }
    Ok(parts.join("/"))
}

/// Highlight hint for the viewer `<code>` block, by file extension.
pub fn file_language(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "md" | "markdown" => "markdown",
        "toml" => "toml",
        "json" => "json",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" => "typescript",
        "py" => "python",
        "sh" | "bash" => "bash",
        "html" => "html",
        "css" => "css",
        "yml" | "yaml" => "yaml",
        "xml" => "xml",
        "rb" => "ruby",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "hpp" => "cpp",
        _ => "text",
    }
}

/// Calm sentence for an empty file tree.
pub fn files_empty_sentence() -> &'static str {
    "No files listed for this branch \u{2014} the tree may be empty or git may be unavailable."
}

/// Read-only: `git ls-tree` rows at one ref. Failures read as none.
pub fn files_tree(gitref: &str) -> Vec<String> {
    if !ref_is_safe(gitref) {
        return Vec::new();
    }
    run_git(&["ls-tree", "-r", "--name-only", gitref], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .filter(|l| !l.trim().is_empty())
                .take(FILES_TREE_MAX)
                .collect()
        })
        .unwrap_or_default()
}

/// Read-only: file body at `<ref>:<path>` with a truncation flag.
/// Binary files read as a calm note; missing files and bad paths are
/// `Err` sentences (the fragment renders them, never a 500).
pub fn file_view(gitref: &str, path: &str) -> Result<(String, bool), String> {
    let clean = sanitize_repo_path(path)?;
    if !ref_is_safe(gitref) {
        return Err(format!(
            "There is no ref {gitref} that the factory can show."
        ));
    }
    let spec = format!("{gitref}:{clean}");
    let out = match run_git(&["show", &spec], None) {
        Ok(o) if o.status.success() => o.stdout,
        _ => {
            return Err(format!(
                "There is no file {clean} at {gitref} that the factory can see."
            ));
        }
    };
    if out.contains(&0) {
        return Ok(("(binary file: preview not shown.)".to_string(), false));
    }
    let body = String::from_utf8_lossy(&out).into_owned();
    if body.len() > FILE_VIEW_MAX_BYTES {
        let mut end = FILE_VIEW_MAX_BYTES;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        return Ok((body[..end].to_string(), true));
    }
    Ok((body, false))
}

/// Read-only: `git grep -n` hits at one ref (capped). Empty queries and
/// failures read as no hits.
pub fn files_search(gitref: &str, q: &str) -> Vec<String> {
    if q.trim().is_empty() || !ref_is_safe(gitref) {
        return Vec::new();
    }
    run_git(&["grep", "-n", "-I", "-e", q, gitref, "--"], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .filter(|l| !l.trim().is_empty())
                .take(FILE_SEARCH_MAX_HITS)
                .collect()
        })
        .unwrap_or_default()
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"._~/-".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Fragment HTML for the file tree (a polling unit). Pure over the tree
/// so tests assert markers without a server.
pub fn files_tree_fragment_html(tree: &[String], gitref: &str) -> String {
    let mut html = format!(
        "<section data-files-tree data-ref=\"{}\"><h2>Files at {}</h2>",
        escape_html(gitref),
        escape_html(gitref),
    );
    if tree.is_empty() {
        html.push_str(&format!(
            "<p data-empty-tree>{}</p>",
            files_empty_sentence()
        ));
    } else {
        html.push_str("<ul>");
        for p in tree {
            html.push_str(&format!(
                "<li><a data-file=\"{}\" href=\"/fragments/files/view?ref={}&amp;path={}\">{}</a></li>",
                escape_html(p),
                url_encode(gitref),
                url_encode(p),
                escape_html(p),
            ));
        }
        html.push_str("</ul>");
    }
    html.push_str("</section>");
    html
}

/// Fragment HTML for the viewer: copy button over escaped code, never an
/// editor (no forms, no textarea, no contenteditable). Pure for testing.
pub fn file_view_fragment_html(gitref: &str, path: &str, body: &str, truncated: bool) -> String {
    let mut html = format!(
        "<section data-file-view data-ref=\"{}\" data-path=\"{}\"><h3>{}</h3>\
<p data-cli>Matches <code>git show {}:{}</code>. View-only \u{2014} copy what you need.</p>\
<button type=\"button\" data-copy-button data-copy-target=\"file-body\">Copy</button>",
        escape_html(gitref),
        escape_html(path),
        escape_html(path),
        escape_html(gitref),
        escape_html(path),
    );
    html.push_str(&format!(
        "<pre data-file-body id=\"file-body\"><code class=\"language-{}\">{}</code></pre>",
        file_language(path),
        escape_html(body),
    ));
    if truncated {
        html.push_str(&format!(
            "<p data-truncated>Showing the first {} bytes \u{2014} the rest is hidden. Full file: <code>git show {}:{}</code>.</p>",
            FILE_VIEW_MAX_BYTES,
            escape_html(gitref),
            escape_html(path),
        ));
    }
    html.push_str("</section>");
    html
}

/// Calm missing-file fragment (a sentence, never a bare 500).
pub fn file_view_missing_fragment_html(gitref: &str, path: &str, reason: &str) -> String {
    format!(
        "<section data-file-missing data-ref=\"{}\"><p>There is no file {} at {} that the factory can show \u{2014} {}</p></section>",
        escape_html(gitref),
        escape_html(path),
        escape_html(gitref),
        escape_html(reason),
    )
}

/// Fragment HTML for search hits at one ref. Pure for testing.
pub fn files_search_fragment_html(gitref: &str, q: &str, hits: &[String]) -> String {
    let mut html = format!(
        "<section data-files-search data-ref=\"{}\"><h2>Search &quot;{}&quot; at {}</h2>",
        escape_html(gitref),
        escape_html(q),
        escape_html(gitref),
    );
    if hits.is_empty() {
        html.push_str(&format!(
            "<p data-no-matches>No matches for &quot;{}&quot; in {} \u{2014} try another search.</p>",
            escape_html(q),
            escape_html(gitref),
        ));
    } else {
        html.push_str("<ol>");
        for h in hits {
            html.push_str(&format!(
                "<li data-search-hit><code>{}</code></li>",
                escape_html(h)
            ));
        }
        html.push_str("</ol>");
    }
    html.push_str("</section>");
    html
}
// --- train review + land button (ticket #32, spec #27) --------------------
//
// The human land ritual as a screen: the open train with its members,
// each member's review verdict and diff, the train-head checks, and a
// one-click land button. Same-binary Axum, server-rendered HTML,
// polling fragment, no websockets.
//
// Reads flow through the existing seams: the open train + ledger via
// `dispatch_state_dir` (FACTORY_STATE_DIR-overridable), verdicts via the
// review transcripts the dispatcher writes (same `VERDICT:` parser the
// dispatcher uses), diffs via `run_git` (FACTORY_GIT_BIN-overridable).
// Writes flow only through the land command: the POST handler below
// calls `cmd_land`, the same function the CLI calls, guarded by the
// same shared preflight (`train_land_preflight`). GETs never mutate
// (snapshot-tested like the board and dispatcher screens).

/// Poll interval for the train fragment: live enough to watch merges
/// land, calm enough to never hammer the check adapter (the fragment
/// itself never runs checks — they run at land time, like the CLI).
pub const TRAIN_POLL_MS: u64 = 5_000;

/// Diff lines shown per member before the calm truncation note.
pub const TRAIN_DIFF_LINES: usize = 30;

/// One train member as the review screen sees it: which ticket, which
/// branch, the review ritual's word, and what changed.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainMemberView {
    pub ticket: u64,
    pub branch: String,
    pub verdict: Option<String>,
    pub verdict_text: String,
    pub diff_stat: String,
    pub diff_truncated: bool,
}

/// The review screen: the open train (or its absence) + members +
/// an optional notice (land refusal or hint) rendered once.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainReviewData {
    pub open: bool,
    pub train: String,
    pub base: String,
    pub head: String,
    pub members: Vec<TrainMemberView>,
    pub notice: String,
}

/// Calm sentence when no train is open (ready, not broken).
pub fn train_no_train_sentence() -> &'static str {
    "No train is open right now \u{2014} cut one with `release start` and it will appear here for review."
}

/// Calm sentence when the review has not reported a verdict yet.
pub fn train_no_verdict_sentence() -> &'static str {
    "No review verdict yet \u{2014} the review attempt has not reported one."
}

/// Calm sentence when a member carries no file changes.
pub fn train_no_diff_sentence() -> &'static str {
    "No file changes recorded for this member."
}

/// Calm sentence when git cannot describe a member (never a 500).
pub fn train_diff_unavailable_sentence() -> &'static str {
    "Diff unavailable \u{2014} git could not describe this member."
}

/// Bea-readable verdict for one member. `None` reads as a calm
/// sentence, never blank.
pub fn train_verdict_text(verdict: Option<&str>) -> String {
    match verdict {
        Some("accept") => "Accepted \u{2014} the review passed this member.".to_string(),
        Some("changes_requested") => {
            "Changes requested \u{2014} the review asked for fixes.".to_string()
        }
        Some(other) => format!("Review said: {other}."),
        None => train_no_verdict_sentence().to_string(),
    }
}

/// Shared land preflight (ticket #32): the exact gate `cmd_land` runs
/// before any mutation — clean working tree, green train head, reachable
/// remote. The UI POST handler calls this same function first so the
/// land button refuses red/dirty exactly like the CLI by construction.
/// Read-only: the head check stages the train in an ephemeral worktree
/// and removes it, mutating nothing (same as the CLI preflight).
pub fn train_land_preflight(target: &str) -> Result<(), String> {
    if !land_working_tree_clean() {
        return Err(format!(
            "land: working tree is dirty \u{2014} commit or stash before landing {target}"
        ));
    }
    land_run_checks(target)?;
    if !land_remote_reachable() {
        return Err(
            "land: remote origin unreachable \u{2014} refusing before any mutation".to_string(),
        );
    }
    Ok(())
}

/// Newest attempt for one ticket, preferring an attempt already on this
/// train (the member's word), else the newest attempt overall.
fn train_member_attempt(
    ledger: &DispatchLedger,
    ticket: u64,
    train: &str,
) -> Option<DispatchAttempt> {
    let mut same: Option<&DispatchAttempt> = None;
    let mut any: Option<&DispatchAttempt> = None;
    for a in &ledger.attempts {
        if a.ticket != ticket {
            continue;
        }
        match any {
            Some(b) if a.id <= b.id => {}
            _ => any = Some(a),
        }
        if a.train_branch == train {
            match same {
                Some(b) if a.id <= b.id => {}
                _ => same = Some(a),
            }
        }
    }
    same.or(any).cloned()
}

/// Read-only per-member diff: `git diff --stat base...branch`, bounded
/// to TRAIN_DIFF_LINES with a calm note when truncated. Empty diffs and
/// git failures read as sentences, never errors.
fn train_member_diff(base: &str, branch: &str) -> (String, bool) {
    if base.is_empty() || branch.is_empty() {
        return (train_diff_unavailable_sentence().to_string(), false);
    }
    let range = format!("{base}...{branch}");
    let out = match run_git(&["diff", "--stat", &range], None) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return (train_diff_unavailable_sentence().to_string(), false),
    };
    if out.trim().is_empty() {
        return (train_no_diff_sentence().to_string(), false);
    }
    let lines: Vec<&str> = out.lines().collect();
    if lines.len() <= TRAIN_DIFF_LINES {
        return (out, false);
    }
    (lines[..TRAIN_DIFF_LINES].join("\n") + "\n", true)
}

/// Read-only review assembly: open train x members x verdicts x diffs.
/// No open train reads as closed (`open: false`); callers attach a
/// `notice` (land refusal or hint) rendered once. Read-only.
pub fn train_review_data() -> TrainReviewData {
    let state = dispatch_state_dir();
    let Some(train) = load_open_train(&state) else {
        return TrainReviewData {
            open: false,
            train: String::new(),
            base: String::new(),
            head: String::new(),
            members: Vec::new(),
            notice: String::new(),
        };
    };
    let ledger = load_ledger(&state);
    let head = train_head(&train.branch);
    let mut members = Vec::new();
    for ticket in member_tickets(&ledger, &train.branch) {
        let branch = ticket_branch_for_merge(&ledger, ticket, &train.branch).unwrap_or_default();
        let verdict = train_member_attempt(&ledger, ticket, &train.branch)
            .map(|a| log_read_transcript(&state, &a))
            .and_then(|t| parse_verdict(&t));
        let verdict_text = train_verdict_text(verdict.as_deref());
        let (diff_stat, diff_truncated) = train_member_diff(&train.base_commit, &branch);
        members.push(TrainMemberView {
            ticket,
            branch,
            verdict,
            verdict_text,
            diff_stat,
            diff_truncated,
        });
    }
    TrainReviewData {
        open: true,
        train: train.branch,
        base: train.base_commit,
        head,
        members,
        notice: String::new(),
    }
}

/// Fragment HTML for the review screen (the polling unit). Pure over
/// data so tests assert markers without a server.
pub fn train_review_fragment_html(data: &TrainReviewData) -> String {
    let mut html = String::new();
    html.push_str("<section data-train-review>");
    if !data.notice.is_empty() {
        html.push_str(&format!("<p data-notice>{}</p>", escape_html(&data.notice)));
    }
    if !data.open {
        html.push_str(&format!(
            "<section data-no-train><p>{}</p></section>",
            train_no_train_sentence()
        ));
        html.push_str("</section>");
        return html;
    }
    html.push_str(&format!(
        "<h2 data-train=\"{}\">Train {}</h2>",
        escape_html(&data.train),
        escape_html(&data.train),
    ));
    if data.head.is_empty() {
        html.push_str("<p data-train-head=\"unknown\">Train head is not present locally.</p>");
    } else {
        html.push_str(&format!(
            "<p data-train-head>Head <code>{}</code> (base <code>{}</code>).</p>",
            escape_html(&data.head),
            escape_html(&land_short(&data.base)),
        ));
    }
    if data.members.is_empty() {
        html.push_str("<section data-members-empty><p>No members merged yet \u{2014} merge accepted tickets with <code>release merge &lt;ticket&gt;</code>.</p></section>");
    } else {
        html.push_str("<section data-members>");
        for m in &data.members {
            let verdict_attr = m.verdict.as_deref().unwrap_or("none");
            html.push_str(&format!(
                "<article data-member=\"{}\" data-verdict=\"{}\" data-ticket=\"{}\"><h3>Member #{} \u{2014} {}</h3><p data-member-verdict>{}</p>",
                m.ticket,
                escape_html(verdict_attr),
                m.ticket,
                m.ticket,
                escape_html(&m.branch),
                escape_html(&m.verdict_text),
            ));
            if m.diff_truncated {
                html.push_str(&format!(
                    "<p data-diff-truncated>(showing first {} lines)</p>",
                    TRAIN_DIFF_LINES
                ));
            }
            html.push_str(&format!(
                "<pre data-diff>{}</pre>",
                escape_html(&m.diff_stat)
            ));
            html.push_str("</article>");
        }
        html.push_str("</section>");
    }
    html.push_str("<div data-land><form method=\"post\" action=\"/train/land\" data-land-form><button type=\"submit\">Land this train</button></form></div>");
    html.push_str("<p data-cli>Matches <code>factory land</code> + <code>release status</code>. Landing runs the same preflight as the CLI (clean tree, green head, reachable remote) \u{2014} red or dirty refuses here exactly like there.</p>");
    html.push_str("</section>");
    html
}

/// Full files page: tree plus search plus the viewer hook, with the ref
/// context echoed and polling on the tree. Pure for testing.
pub fn files_page_html(tree: &[String], gitref: &str) -> String {
    let tree_html = files_tree_fragment_html(tree, gitref);
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Factory files</title><style>{}</style></head>\
<body><header class=\"site\"><h1>Factory files</h1>\
<p>Tree, viewer, and search from any branch. Matches <code>git ls-tree</code> + <code>git show &lt;ref&gt;:&lt;path&gt;</code> + <code>git grep</code>.</p>\
<nav><a href=\"/\">Board</a> | <a href=\"/dispatch\">Dispatcher live</a> | <a href=\"/git\">Git</a> | <a href=\"/files\">Files</a> | <a href=\"/train\">Train review</a></nav>\
<p data-version>factory {version}</p></header>\
<main><div id=\"files\" data-files-fragment=\"/fragments/files/tree?ref={}\" data-poll-ms=\"{}\">\
{tree_html}</div>\
<section data-files-search data-ref=\"{}\"><h2>Search</h2>\
<form method=\"get\" action=\"/fragments/files/search\" data-search-form>\
<input type=\"hidden\" name=\"ref\" value=\"{}\">\
<input name=\"q\" placeholder=\"Search this branch...\">\
<button type=\"submit\">Search</button></form></section>\
<div id=\"file-view\" data-view-fragment=\"/fragments/files/view\">\
<section data-file-view-empty><p>Pick a file from the tree to read it here. Files are view-only \u{2014} copy what you need.</p></section></div></main>\
<script>(function(){{var el=document.getElementById('files');if(!el)return;var url=el.getAttribute('data-files-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'15000',10);\
setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
<script>document.addEventListener('click',function(e){{var b=e.target&&e.target.closest?e.target.closest('[data-copy-button]'):null;if(!b)return;\
var t=document.getElementById(b.getAttribute('data-copy-target'));if(!t)return;\
var text=t.innerText||t.textContent||'';\
if(navigator.clipboard&&navigator.clipboard.writeText){{navigator.clipboard.writeText(text).then(function(){{b.textContent='Copied';}}).catch(function(){{}});}}}});</script>\
</body></html>",
        SITE_CSS,
        url_encode(gitref),
        FILES_POLL_MS,
        escape_html(gitref),
        escape_html(gitref),
    )
}

/// GET /files: tree + search + viewer hook for one ref. Read-only observer.
pub async fn files_page_handler(
    axum::extract::Query(q): axum::extract::Query<FilesTreeQuery>,
) -> axum::response::Html<String> {
    let gitref = clean_ref(q.gitref.as_deref());
    axum::response::Html(files_page_html(&files_tree(&gitref), &gitref))
}

/// GET /fragments/files/tree?ref=: the tree polling unit. Read-only observer.
pub async fn files_tree_fragment_handler(
    axum::extract::Query(q): axum::extract::Query<FilesTreeQuery>,
) -> axum::response::Html<String> {
    let gitref = clean_ref(q.gitref.as_deref());
    axum::response::Html(files_tree_fragment_html(&files_tree(&gitref), &gitref))
}

/// GET /fragments/files/view?ref=&path=: the viewer. Read-only and
/// copy-only; unknown files and bad paths render a calm sentence.
pub async fn file_view_fragment_handler(
    axum::extract::Query(q): axum::extract::Query<FilesViewQuery>,
) -> (axum::http::StatusCode, axum::response::Html<String>) {
    let gitref = clean_ref(q.gitref.as_deref());
    match file_view(&gitref, &q.path) {
        Ok((body, truncated)) => (
            axum::http::StatusCode::OK,
            axum::response::Html(file_view_fragment_html(&gitref, &q.path, &body, truncated)),
        ),
        Err(reason) => (
            axum::http::StatusCode::OK,
            axum::response::Html(file_view_missing_fragment_html(&gitref, &q.path, &reason)),
        ),
    }
}

/// GET /fragments/files/search?ref=&q=: search hits. Read-only observer.
pub async fn files_search_fragment_handler(
    axum::extract::Query(q): axum::extract::Query<FilesSearchQuery>,
) -> axum::response::Html<String> {
    let gitref = clean_ref(q.gitref.as_deref());
    axum::response::Html(files_search_fragment_html(
        &gitref,
        &q.q,
        &files_search(&gitref, &q.q),
    ))
}

/// Full review page: fragment plus the polling hook that refreshes only
/// the fragment (no full reloads). Pure over data for testing.
pub fn train_review_page_html(data: &TrainReviewData) -> String {
    let fragment = train_review_fragment_html(data);
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Train review \u{2014} Factory</title><style>{}</style></head>\
<body><header class=\"site\"><h1>Train review</h1>\
<p>Members, verdicts, diffs, and the human land. Matches <code>factory land</code> + <code>release status</code>.</p>\
<nav><a href=\"/\">Board</a> | <a href=\"/dispatch\">Dispatcher live</a> | <a href=\"/git\">Git</a> | <a href=\"/files\">Files</a> | <a href=\"/train\">Train review</a></nav>\
<p data-version>factory {version}</p></header>\
<main><div id=\"train\" data-train-fragment=\"/fragments/train\" data-poll-ms=\"{}\">\
{fragment}</div></main>\
<script>(function(){{var el=document.getElementById('train');if(!el)return;var url=el.getAttribute('data-train-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'5000',10);\
setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
</body></html>",
        SITE_CSS,
        TRAIN_POLL_MS,
    )
}

/// Release state after a successful land: the train named, its closed
/// members listed, the stamped release note streaming. Pure for testing.
pub fn train_landed_page_html(train: &str, closed: &[u64], note: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let members = closed
        .iter()
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Train landed \u{2014} Factory</title><style>{}</style></head>\
<body><header class=\"site\"><h1>Train landed</h1>\
<nav><a href=\"/\">Board</a> | <a href=\"/dispatch\">Dispatcher live</a> | <a href=\"/git\">Git</a> | <a href=\"/files\">Files</a> | <a href=\"/train\">Train review</a></nav>\
<p data-version>factory {version}</p></header>\
<main><section data-released><p>Train {} landed into dev. Closed members: {}.</p><p data-release-note>{}</p>\
<p>Matches <code>factory land</code>. Cleanup commands were printed to the land log (branches are never deleted by automation).</p></section></main>\
</body></html>",
        SITE_CSS,
        escape_html(train),
        escape_html(&members),
        escape_html(note),
    )
}

/// GET /train: the review screen. Read-only observer.
pub async fn train_review_page_handler() -> axum::response::Html<String> {
    axum::response::Html(train_review_page_html(&train_review_data()))
}

/// GET /fragments/train: the polling unit. Read-only observer.
pub async fn train_review_fragment_handler() -> axum::response::Html<String> {
    axum::response::Html(train_review_fragment_html(&train_review_data()))
}

/// POST /train/land: one-click land through the command function — the
/// same `cmd_land` the CLI calls, behind the identical shared preflight
/// (`train_land_preflight`). Refusals re-render the review carrying the
/// CLI's own message (nothing mutated); success renders the release
/// state naming the train and its closed members.
pub async fn train_land_handler() -> (axum::http::StatusCode, axum::response::Html<String>) {
    let before = train_review_data();
    if !before.open {
        let mut data = before;
        data.notice = "land: no open train \u{2014} cut one with `release start` first".to_string();
        return (
            axum::http::StatusCode::OK,
            axum::response::Html(train_review_page_html(&data)),
        );
    }
    if let Err(e) = train_land_preflight(&before.train) {
        let mut data = before;
        data.notice = e;
        return (
            axum::http::StatusCode::OK,
            axum::response::Html(train_review_page_html(&data)),
        );
    }
    let rc = cmd_land(false, false, None);
    if rc == 0 {
        let tickets: Vec<u64> = before.members.iter().map(|m| m.ticket).collect();
        let note = land_release_note(&before.train);
        (
            axum::http::StatusCode::OK,
            axum::response::Html(train_landed_page_html(&before.train, &tickets, &note)),
        )
    } else {
        let mut data = train_review_data();
        data.notice =
            "Land did not complete \u{2014} run `factory land` to see each step.".to_string();
        (
            axum::http::StatusCode::OK,
            axum::response::Html(train_review_page_html(&data)),
        )
    }
}

/// Handler shape for UI tickets: board page + polling fragment + health.
/// All GETs are read-only observers (same contract as `status`/`log`).
pub fn web_router() -> axum::Router {
    axum::Router::new()
        .route("/", axum::routing::get(board_page_handler))
        .route(
            "/fragments/board",
            axum::routing::get(board_fragment_handler),
        )
        .route("/tickets/{number}", axum::routing::get(ticket_page_handler))
        .route(
            "/fragments/ticket/{number}",
            axum::routing::get(ticket_fragment_handler),
        )
        .route(
            "/api/tickets/{number}",
            axum::routing::get(ticket_api_handler),
        )
        .route("/dispatch", axum::routing::get(dispatcher_page_handler))
        .route(
            "/fragments/dispatch",
            axum::routing::get(dispatcher_fragment_handler),
        )
        .route(
            "/dispatch/stop",
            axum::routing::post(dispatcher_stop_handler),
        )
        .route(
            "/dispatch/resume",
            axum::routing::post(dispatcher_resume_handler),
        )
        .route("/git", axum::routing::get(git_page_handler))
        .route("/fragments/git", axum::routing::get(git_fragment_handler))
        .route("/api/git", axum::routing::get(git_api_handler))
        .route("/files", axum::routing::get(files_page_handler))
        .route(
            "/fragments/files/tree",
            axum::routing::get(files_tree_fragment_handler),
        )
        .route(
            "/fragments/files/view",
            axum::routing::get(file_view_fragment_handler),
        )
        .route(
            "/fragments/files/search",
            axum::routing::get(files_search_fragment_handler),
        )
        .route("/train", axum::routing::get(train_review_page_handler))
        .route(
            "/fragments/train",
            axum::routing::get(train_review_fragment_handler),
        )
        .route("/train/land", axum::routing::post(train_land_handler))
        .route("/onboard", axum::routing::get(onboard_page_handler))
        .route(
            "/fragments/onboard",
            axum::routing::get(onboard_fragment_handler),
        )
        .route(
            "/onboard/approve",
            axum::routing::post(onboard_approve_handler),
        )
        .route(
            "/onboard/change",
            axum::routing::post(onboard_change_handler),
        )
        .route(
            "/health",
            axum::routing::get(|| async { web_health_response() }),
        )
}

/// True for loopback binds (127.0.0.1, ::1, localhost, with or without port).
fn ui_addr_is_local(addr: &str) -> bool {
    let host = addr
        .rsplit(':')
        .next()
        .unwrap_or(addr)
        .trim_matches(|c| c == '[' || c == ']');
    let host = addr
        .rsplit_once(':')
        .map(|(h, _)| h.trim().trim_matches(|c| c == '[' || c == ']'))
        .unwrap_or(host);
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}

/// Serve the board until interrupted. Blocks on a Tokio runtime created
/// here so the sync `run()` CLI keeps one code path for binary + handlers.
pub fn web_serve(addr: &str, allow_remote: bool) -> Result<(), String> {
    if !allow_remote && !ui_addr_is_local(addr) {
        return Err(format!(
            "refusing non-localhost bind {addr}: pass --allow-remote to expose the factory UI explicitly"
        ));
    }
    let owned = addr.to_string();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start UI runtime: {e}"))?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&owned)
            .await
            .map_err(|e| format!("cannot bind {owned}: {e}"))?;
        println!("factory ui serving the board on http://{owned} (board at /)");
        axum::serve(listener, web_router())
            .await
            .map_err(|e| format!("factory ui failed: {e}"))
    })
}

/// `factory ui` / `factory serve`: bind and serve the board. Exit 0 once
/// the server stops cleanly; nonzero with a plain-English cause when the
/// address is rejected or the listener cannot start.
pub fn cmd_ui(addr: &str, allow_remote: bool, json: bool) -> i32 {
    match web_serve(addr, allow_remote) {
        Ok(()) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"serving": false, "addr": addr, "stopped": true})
                );
            }
            0
        }
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({"error": e, "addr": addr}));
            }
            eprintln!("{e}");
            1
        }
    }
}

// ---------------------------------------------------------------------------
// ONBOARD (ticket #42, spec #41 backbone: install check, auth tour, repo)
//
// `factory onboard [--plain] [--repo NAME] [--new NAME]
// [--visibility private|public] [--idea TEXT]` walks Bea from zero to a
// ready repo. Each step is a function returning the next step; progress
// persists to `<state>/onboard.json` so a rerun resumes where the last run
// stopped and never duplicates created resources (the `gh repo create`
// call is skipped when the state already records `created: true` for the
// same name).
//
// Seams reused, never reinvented: `gh` through `dgh` (FACTORY_GH_BIN-
// overridable, like every other call site), state through
// `dispatch_state_dir` (FACTORY_STATE_DIR-overridable). No custom OAuth,
// no held secrets: auth is the guided `gh` inheritance the spec locks in
// (detect, install hint, browser device-flow tour, verify with an API
// call). Fresh repos are private by default and get the identity-only
// seed (name README, idea file, ignore file, first commit) — never sample
// code. `--plain` covers every step: the `/onboard` screen is primary
// (ticket #43) and carries the same facts as the plain transcript.
// ---------------------------------------------------------------------------

/// Fixed bytes of the identity-only seed README (`{name}` substituted).
/// Byte-checked by `cli_onboard.rs`: change these bytes and the test must
/// change with them (the spec locks identity-only scope).
pub fn onboard_readme_text(name: &str) -> String {
    format!("# {name}\n\nBuilt with the factory.\n")
}

/// Fixed bytes of the identity-only seed ignore file.
pub fn onboard_gitignore_text() -> &'static str {
    "target/\nfactory/.state/\n.DS_Store\n"
}

/// Default idea text when Bea has not described hers yet (stored verbatim
/// once given via `--idea` / FACTORY_ONBOARD_IDEA).
pub fn onboard_default_idea() -> &'static str {
    "My factory idea.\n"
}

/// Resumable onboarding progress. `steps` holds the visited step names in
/// order (`install`, `auth`, `repo`, `idea`, `draft`, `approve`, `ready`);
/// `created` is true only after a successful `gh repo create` for `repo`,
/// so resumes skip the create. `idea` is her telling verbatim;
/// `draft_version` counts the approve-loop rounds (v1 kept when v2
/// lands); `approved_version` + `draft_hash` freeze what she approved
/// (freeze lineage starts here, per the spec). New fields carry
/// `#[serde(default)]` so pre-loop state files still parse.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct OnboardState {
    pub install_ok: bool,
    pub auth: String,
    pub repo: String,
    pub visibility: String,
    pub created: bool,
    pub seeded: bool,
    pub steps: Vec<String>,
    #[serde(default)]
    pub idea: String,
    #[serde(default)]
    pub draft_version: u64,
    #[serde(default)]
    pub approved_version: u64,
    #[serde(default)]
    pub draft_hash: String,
    #[serde(default)]
    pub changes: Vec<String>,
}

impl Default for OnboardState {
    fn default() -> Self {
        Self {
            install_ok: false,
            auth: String::new(),
            repo: String::new(),
            visibility: "private".to_string(),
            created: false,
            seeded: false,
            steps: Vec::new(),
            idea: String::new(),
            draft_version: 0,
            approved_version: 0,
            draft_hash: String::new(),
            changes: Vec::new(),
        }
    }
}

/// CLI-supplied onboarding choices (flags win over FACTORY_ONBOARD_* env).
#[derive(Debug, Clone, Default)]
pub struct OnboardOpts {
    pub plain: bool,
    pub repo: Option<String>,
    pub new: Option<String>,
    pub visibility: Option<String>,
    pub idea: Option<String>,
    pub approve: bool,
    pub change: Option<String>,
}

fn onboard_state_path(state: &std::path::Path) -> PathBuf {
    state.join("onboard.json")
}

fn onboard_load(state: &std::path::Path) -> OnboardState {
    std::fs::read_to_string(onboard_state_path(state))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn onboard_save(state: &std::path::Path, progress: &OnboardState) -> Result<(), String> {
    std::fs::create_dir_all(state)
        .map_err(|e| format!("cannot create {}: {e}", state.display()))?;
    let tmp = state.join("onboard.json.tmp");
    let text = serde_json::to_string_pretty(progress)
        .map_err(|e| format!("cannot serialize onboard state: {e}"))?;
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write onboard state: {e}"))?;
    std::fs::rename(&tmp, onboard_state_path(state))
        .map_err(|e| format!("cannot replace onboard state: {e}"))?;
    Ok(())
}

fn onboard_note_step(progress: &mut OnboardState, step: &str) {
    if !progress.steps.iter().any(|s| s == step) {
        progress.steps.push(step.to_string());
    }
}

fn onboard_choice(flag: Option<String>, env_name: &str) -> Option<String> {
    flag.or_else(|| {
        std::env::var(env_name)
            .ok()
            .filter(|v| !v.trim().is_empty())
    })
}

/// Step 1, install check: the `gh` shell-out seam must answer (the same
/// probe `doctor_checks` uses for `gh-installed`), and the state dir must
/// exist. Read-only except for creating the state dir. Returns the next
/// step (`auth`) on success.
fn onboard_step_install(progress: &mut OnboardState) -> Result<&'static str, String> {
    println!("[onboard:install] Checking the factory's prerequisites.");
    match dgh(&["--version"]) {
        Ok(_) => {
            println!("[onboard:install] GitHub CLI found — good, that is all we need.");
        }
        Err(_) => {
            return Err(format!(
                "factory onboard: the GitHub CLI (`{}`) is not installed. Install it first: https://cli.github.com — then re-run: factory onboard",
                gh_bin()
            ));
        }
    }
    let state = dispatch_state_dir();
    if let Err(e) = std::fs::create_dir_all(&state) {
        return Err(format!(
            "factory onboard: cannot create state dir {}: {e}",
            state.display()
        ));
    }
    progress.install_ok = true;
    onboard_note_step(progress, "install");
    if let Err(e) = onboard_save(&state, progress) {
        return Err(format!("factory onboard: {e}"));
    }
    Ok("auth")
}

/// Step 2, the GitHub auth tour: verifies the login with an API call. On
/// an expired or missing login it prints the browser device-flow tour and
/// stops (resumable: rerun after approving in the browser). Returns the
/// next step (`repo`) on success.
fn onboard_step_auth(progress: &mut OnboardState) -> Result<&'static str, String> {
    println!("[onboard:auth] Checking your GitHub login.");
    match dgh(&["auth", "status"]) {
        Ok(_) => {
            println!("[onboard:auth] GitHub login works — no codes, no tokens to copy.");
        }
        Err(_) => {
            return Err(
                "factory onboard: GitHub login is missing or expired. I will show you a code — approve it in your browser and you are done: run `gh auth login`, follow the browser step, then re-run: factory onboard"
                    .to_string(),
            );
        }
    }
    progress.auth = "valid".to_string();
    onboard_note_step(progress, "auth");
    if let Err(e) = onboard_save(&dispatch_state_dir(), progress) {
        return Err(format!("factory onboard: {e}"));
    }
    Ok("repo")
}

/// Parse the visibility choice: empty means the safe default (`private`);
/// anything other than private/public is refused in plain words.
fn onboard_visibility(choice: Option<String>) -> Result<String, String> {
    match choice.as_deref().map(str::trim) {
        None | Some("") => Ok("private".to_string()),
        Some("private") => Ok("private".to_string()),
        Some("public") => Ok("public".to_string()),
        Some(other) => Err(format!(
            "factory onboard: visibility {other:?} is not a choice — say `private` (keeps your idea yours, the default) or `public`: factory onboard --new NAME --visibility public"
        )),
    }
}

/// Write the identity-only seed into `<cwd>/<name>`: name README, idea
/// file, ignore file — never sample code — then `git init` plus a first
/// commit (hermetic: local `-c` identity, best-effort when git is
/// missing). Byte-exact: `cli_onboard.rs` asserts the file bytes.
fn onboard_seed_fresh(name: &str, idea: &str) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dir = cwd.join(name);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("factory onboard: cannot create {}: {e}", dir.display()))?;
    std::fs::write(dir.join("README.md"), onboard_readme_text(name))
        .map_err(|e| format!("factory onboard: cannot seed README.md: {e}"))?;
    std::fs::write(dir.join("IDEA.md"), idea)
        .map_err(|e| format!("factory onboard: cannot seed IDEA.md: {e}"))?;
    std::fs::write(dir.join(".gitignore"), onboard_gitignore_text())
        .map_err(|e| format!("factory onboard: cannot seed .gitignore: {e}"))?;
    // First commit, hermetic: repo-local identity so Bea's global git
    // config (or its absence) never blocks the seed. Best-effort: files
    // on disk are the seed; a missing git binary must not fail onboarding.
    let git_ok = run_git(&["init", "-b", "main"], Some(&dir))
        .map(|o| o.status.success())
        .unwrap_or(false);
    if git_ok {
        let _ = run_git(
            &["config", "user.name", "Bea (factory onboard)"],
            Some(&dir),
        );
        let _ = run_git(&["config", "user.email", "bea@factory.local"], Some(&dir));
        let _ = run_git(&["add", "-A"], Some(&dir));
        let _ = run_git(
            &["commit", "-qm", "factory onboard: identity seed"],
            Some(&dir),
        );
    }
    Ok(())
}

/// Step 3, repo pick-or-create. Existing (`--repo`): verify through `gh
/// repo view`, record, continue. Fresh (`--new`): create private by
/// default (public only as a plain-words choice), then the identity-only
/// seed — skipping the create when the state already records it for the
/// same name (resume without duplicating). Neither: stop in plain words
/// naming both flags (resumable). Returns the next step (`ready`).
fn onboard_step_repo(
    progress: &mut OnboardState,
    opts: &OnboardOpts,
) -> Result<&'static str, String> {
    println!("[onboard:repo] Choosing where your idea will live.");
    let pick = onboard_choice(opts.repo.clone(), "FACTORY_ONBOARD_REPO");
    let fresh = onboard_choice(opts.new.clone(), "FACTORY_ONBOARD_NEW");
    let visibility = onboard_visibility(onboard_choice(
        opts.visibility.clone(),
        "FACTORY_ONBOARD_VISIBILITY",
    ))?;
    let requested_idea = onboard_choice(opts.idea.clone(), "FACTORY_ONBOARD_IDEA")
        .filter(|idea| !idea.trim().is_empty());
    // A rerun continues from the stored words. Once approved, even an
    // explicit replacement is ignored so the seed and frozen draft agree.
    let idea = if progress.approved_version > 0 && !progress.idea.is_empty() {
        progress.idea.clone()
    } else if let Some(idea) = requested_idea {
        idea
    } else if !progress.idea.is_empty() {
        progress.idea.clone()
    } else {
        onboard_default_idea().to_string()
    };

    if let Some(name) = pick {
        let name = name.trim().to_string();
        dgh(&["repo", "view", &name, "--json", "nameWithOwner"]).map_err(|_| {
            format!(
                "factory onboard: cannot read repo {name:?} through gh — check the name (or your access) with: gh repo view {name} — then re-run: factory onboard --repo {name}"
            )
        })?;
        println!("[onboard:repo] Using your existing repo {name} — nothing created.");
        progress.repo = name;
        progress.visibility = "existing".to_string();
        progress.created = false;
        progress.seeded = false;
    } else if let Some(name) = fresh {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(
                "factory onboard: the fresh repo needs a name — re-run: factory onboard --new NAME"
                    .to_string(),
            );
        }
        progress.visibility = visibility.clone();
        let already = progress.created && progress.repo == name;
        if already {
            println!(
                "[onboard:repo] {name} is already yours from the last run — not creating it again."
            );
        } else {
            let scope = if visibility == "public" {
                "--public"
            } else {
                "--private"
            };
            println!(
                "[onboard:repo] Creating {name} as {visibility} (your idea stays yours by default)."
            );
            dgh(&["repo", "create", &name, scope, "--confirm"]).map_err(|e| {
                format!("factory onboard: could not create repo {name:?}: {e} — fix it, then re-run: factory onboard --new {name}")
            })?;
            progress.repo = name.clone();
            progress.created = true;
        }
        progress.repo = name.clone();
        onboard_seed_fresh(&name, &idea)?;
        println!("[onboard:repo] Seeded {name} with identity only (name, idea, ignore files) — no sample code.");
        progress.seeded = true;
    } else {
        return Err(
            "factory onboard: tell me where your idea should live — pick an existing repo: factory onboard --repo OWNER/NAME — or start fresh (private by default): factory onboard --new NAME"
                .to_string(),
        );
    }
    onboard_note_step(progress, "repo");
    if let Err(e) = onboard_save(&dispatch_state_dir(), progress) {
        return Err(format!("factory onboard: {e}"));
    }
    Ok("idea")
}

/// Split Bea's idea into sentences (her own words, kept in order).
/// Splits on `.`, `!`, `?`; empty pieces are dropped. Pure: the draft
/// renderer and its readers share it, so the CLI and the screen agree.
pub fn onboard_sentences(idea: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in idea.chars() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?') {
            let s = cur.trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
            cur.clear();
        }
    }
    let tail = cur.trim().to_string();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

/// Epic title in her own words: the repo plus her first sentence.
pub fn onboard_epic_title(repo: &str, idea: &str) -> String {
    match onboard_sentences(idea).into_iter().next() {
        Some(first) => format!("{repo}: {first}"),
        None => format!("{repo}: Bea's idea"),
    }
}

/// Ticket beginnings in her own words: capture the verbatim idea, shape
/// it into an approvable plan, build the approved plan onto the board.
pub fn onboard_tickets(repo: &str, idea: &str) -> Vec<String> {
    let sentences = onboard_sentences(idea);
    let first = sentences
        .first()
        .cloned()
        .unwrap_or_else(|| "Bea's idea.".to_string());
    let second = sentences.get(1).cloned().unwrap_or_else(|| first.clone());
    vec![
        format!("Capture \u{2014} keep Bea's idea verbatim: \"{first}\""),
        format!(
            "Shape \u{2014} turn \"{second}\" into an epic, a spec, and tickets for approval."
        ),
        format!(
            "Build \u{2014} put the approved {repo} plan on the board with moving cards (`factory ui`)."
        ),
    ]
}

/// Render draft vN: her idea verbatim first, then the epic/spec/tickets
/// phrased from her sentences, then the change history. Pure over inputs
/// so the CLI transcript, the stored file, and the `/onboard` screen
/// print one story. Deterministic: re-rendering the same inputs yields
/// the same bytes (the approval hash covers exactly these bytes).
pub fn onboard_draft_text(repo: &str, idea: &str, version: u64, changes: &[String]) -> String {
    let clean = format!("{}\n", idea.trim_end());
    let mut text = format!(
        "# Draft v{version} \u{2014} {repo}\n\n## Your idea, in your words\n\n{clean}\n## Epic\n\n{}\n\n## Spec\n\nBea wants: {clean}\n## Tickets\n\n",
        onboard_epic_title(repo, idea)
    );
    for (i, ticket) in onboard_tickets(repo, idea).iter().enumerate() {
        text.push_str(&format!("{}. {ticket}\n", i + 1));
    }
    text.push_str("\n## Change history\n\n- v1: first draft.\n");
    for change in changes {
        text.push_str(&format!("- you asked: \"{}\"\n", change.trim()));
    }
    text
}

fn onboard_draft_path(state: &std::path::Path, version: u64) -> PathBuf {
    state.join(format!("onboard-draft-v{version}.md"))
}

fn onboard_sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Write the current `progress.draft_version` from the progress fields.
/// Versions are append-only: a written file is never rewritten (a change
/// round mints the next version instead).
fn onboard_write_draft(state: &std::path::Path, progress: &OnboardState) -> Result<(), String> {
    let text = onboard_draft_text(
        &progress.repo,
        &progress.idea,
        progress.draft_version,
        &progress.changes,
    );
    std::fs::write(onboard_draft_path(state, progress.draft_version), &text)
        .map_err(|e| format!("factory onboard: cannot write draft: {e}"))?;
    Ok(())
}

/// True when the idea arrived explicitly (flag or env), i.e. Bea told us
/// her idea this run. Without it there is nothing to approve, and the
/// flow keeps the backbone contract: ready without a decision.
fn onboard_explicit_idea(opts: &OnboardOpts) -> bool {
    onboard_choice(opts.idea.clone(), "FACTORY_ONBOARD_IDEA")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// True when approval arrived via `--approve` or FACTORY_ONBOARD_APPROVE.
fn onboard_wants_approve(opts: &OnboardOpts) -> bool {
    if opts.approve {
        return true;
    }
    matches!(
        std::env::var("FACTORY_ONBOARD_APPROVE").as_deref(),
        Ok(v) if !v.trim().is_empty() && v.trim() != "0"
    )
}

/// Step 4, idea capture: two free sentences stored verbatim (the seed
/// already carries them; here they enter the draft loop). A restated
/// idea before approval is a change round: the next version, with the
/// old ones kept. Returns the next step (`draft`).
fn onboard_step_idea(
    progress: &mut OnboardState,
    opts: &OnboardOpts,
) -> Result<&'static str, String> {
    println!("[onboard:idea] Capturing your idea in your own words.");
    let supplied_idea = onboard_choice(opts.idea.clone(), "FACTORY_ONBOARD_IDEA")
        .filter(|idea| !idea.trim().is_empty());
    if progress.approved_version > 0 {
        if supplied_idea
            .as_ref()
            .is_some_and(|idea| idea != &progress.idea)
        {
            println!(
                "[onboard:idea] This plan is already approved and frozen — keeping the approved idea."
            );
        }
    } else {
        // Absence of --idea on a resume means keep Bea's stored words; it
        // is not a restatement with the default idea.
        let idea = supplied_idea.unwrap_or_else(|| {
            if progress.idea.is_empty() {
                onboard_default_idea().to_string()
            } else {
                progress.idea.clone()
            }
        });
        let restated =
            progress.draft_version > 0 && !progress.idea.is_empty() && progress.idea != idea;
        progress.idea = idea.clone();
        if restated {
            progress
                .changes
                .push(format!("Restated idea: {}", idea.trim()));
            progress.draft_version += 1;
            let state = dispatch_state_dir();
            onboard_write_draft(&state, progress)?;
        }
    }
    onboard_note_step(progress, "idea");
    if let Err(e) = onboard_save(&dispatch_state_dir(), progress) {
        return Err(format!("factory onboard: {e}"));
    }
    Ok("draft")
}

/// Step 5, draft generation: render v1 from her stored idea (her own
/// words first, then epic/spec/tickets), or replay the current version
/// when a rerun arrives without a new decision. Versions are never
/// re-rendered in place. Returns the next step (`approve`).
fn onboard_step_draft(progress: &mut OnboardState) -> Result<&'static str, String> {
    let state = dispatch_state_dir();
    if progress.draft_version == 0 {
        progress.draft_version = 1;
        onboard_write_draft(&state, progress)?;
    }
    let body = std::fs::read_to_string(onboard_draft_path(&state, progress.draft_version))
        .map_err(|e| format!("factory onboard: cannot read draft: {e}"))?;
    println!(
        "[onboard:draft] Draft v{} for {} \u{2014} in your own words:",
        progress.draft_version, progress.repo
    );
    println!("{body}");
    onboard_note_step(progress, "draft");
    if let Err(e) = onboard_save(&state, progress) {
        return Err(format!("factory onboard: {e}"));
    }
    Ok("approve")
}

/// One draft as the `/onboard` screen sees it: sections plus the
/// decision state. Rebuilt from the same state files the CLI walks, so
/// the screen and `--plain` can never disagree.
#[derive(Debug, Clone, PartialEq)]
pub struct OnboardDraft {
    pub version: u64,
    pub repo: String,
    pub idea: String,
    pub epic: String,
    pub spec: String,
    pub tickets: Vec<String>,
    pub changes: Vec<String>,
    pub approved_version: u64,
    pub hash: String,
}

/// Read the current draft from the state dir. `None` when no draft was
/// generated yet (Bea has not described her idea). Read-only.
pub fn onboard_current_draft() -> Option<OnboardDraft> {
    let state = dispatch_state_dir();
    let progress = onboard_load(&state);
    if progress.draft_version == 0 || progress.repo.is_empty() {
        return None;
    }
    std::fs::read_to_string(onboard_draft_path(&state, progress.draft_version)).ok()?;
    Some(OnboardDraft {
        version: progress.draft_version,
        repo: progress.repo.clone(),
        idea: progress.idea.clone(),
        epic: onboard_epic_title(&progress.repo, &progress.idea),
        spec: format!("Bea wants: {}", progress.idea.trim_end()),
        tickets: onboard_tickets(&progress.repo, &progress.idea),
        changes: progress.changes.clone(),
        approved_version: progress.approved_version,
        hash: progress.draft_hash.clone(),
    })
}

/// Apply her change words: mint the next version (v1 kept), record what
/// she asked, persist. Shared by the CLI `--change` and the screen's
/// Change-something form. Returns the new version.
pub fn onboard_apply_change(feedback: &str) -> Result<u64, String> {
    let feedback = feedback.trim().to_string();
    if feedback.is_empty() {
        return Err(
            "factory onboard: say what to change in plain words — re-run: factory onboard --change TEXT"
                .to_string(),
        );
    }
    let state = dispatch_state_dir();
    let mut progress = onboard_load(&state);
    if progress.approved_version > 0 {
        return Err(format!(
            "factory onboard: plan v{} is already approved and frozen — changes are not applied after approval.",
            progress.approved_version
        ));
    }
    if progress.draft_version == 0 {
        return Err(
            "factory onboard: no draft to change yet — start one: factory onboard --idea TEXT"
                .to_string(),
        );
    }
    progress.changes.push(feedback);
    progress.draft_version += 1;
    onboard_write_draft(&state, &progress)?;
    onboard_note_step(&mut progress, "draft");
    onboard_save(&state, &progress)?;
    Ok(progress.draft_version)
}

/// Approve the current version: freeze it with the sha256 of the exact
/// draft bytes (what she approved is provable later). Shared by the CLI
/// `--approve` and the screen's Approve button. Returns (version, hash).
pub fn onboard_apply_approve() -> Result<(u64, String), String> {
    let state = dispatch_state_dir();
    let mut progress = onboard_load(&state);
    if progress.approved_version > 0 {
        // Approval freezes an immutable version. Repeated approvals are a
        // no-op and must never hash a newer, unapproved draft.
        return Ok((progress.approved_version, progress.draft_hash));
    }
    if progress.draft_version == 0 {
        return Err(
            "factory onboard: no draft to approve yet — start one: factory onboard --idea TEXT"
                .to_string(),
        );
    }
    let body = std::fs::read_to_string(onboard_draft_path(&state, progress.draft_version))
        .map_err(|e| format!("factory onboard: cannot read draft: {e}"))?;
    let hash = onboard_sha256_hex(body.as_bytes());
    progress.approved_version = progress.draft_version;
    progress.draft_hash = hash.clone();
    onboard_note_step(&mut progress, "approve");
    onboard_save(&state, &progress)?;
    Ok((progress.approved_version, hash))
}

/// Step 6, the approve loop: `--change` mints the next version and stops
/// for review; `--approve` freezes the current version and continues;
/// without a decision an explicit idea stops with the draft and the two
/// plain-words reruns; without an explicit idea there is nothing to
/// approve and the backbone flows straight to ready. Returns `ready`.
fn onboard_step_approve(
    progress: &mut OnboardState,
    opts: &OnboardOpts,
    pending_draft_on_entry: bool,
) -> Result<&'static str, String> {
    if progress.approved_version > 0 {
        if let Some(feedback) = onboard_choice(opts.change.clone(), "FACTORY_ONBOARD_CHANGE") {
            onboard_apply_change(&feedback).map_err(|e| e.to_string())?;
        }
        if onboard_wants_approve(opts) {
            let (version, hash) = onboard_apply_approve().map_err(|e| e.to_string())?;
            println!(
                "[onboard:approve] Plan v{version} is already approved and frozen ({}).",
                hash.chars().take(12).collect::<String>()
            );
        }
        return Ok("ready");
    }
    if let Some(feedback) = onboard_choice(opts.change.clone(), "FACTORY_ONBOARD_CHANGE") {
        let version = onboard_apply_change(&feedback).map_err(|e| e.to_string())?;
        *progress = onboard_load(&dispatch_state_dir());
        let body = std::fs::read_to_string(onboard_draft_path(&dispatch_state_dir(), version))
            .unwrap_or_default();
        println!("[onboard:draft] Draft v{version} \u{2014} with your change:");
        println!("{body}");
        return Err(format!(
            "factory onboard: draft v{version} is ready to review — rerun with the same repo and idea using `factory onboard --new NAME --idea TEXT --approve` (or `--change TEXT`); for an existing repo, use `--repo OWNER/NAME` instead of `--new NAME`."
        ));
    }
    if onboard_wants_approve(opts) {
        let (version, hash) = onboard_apply_approve().map_err(|e| e.to_string())?;
        *progress = onboard_load(&dispatch_state_dir());
        println!(
            "[onboard:approve] Approved plan v{version} ({}) \u{2014} frozen as the plan the board builds from.",
            &hash[..12.min(hash.len())]
        );
        return Ok("ready");
    }
    if onboard_explicit_idea(opts) || pending_draft_on_entry {
        let body = std::fs::read_to_string(onboard_draft_path(
            &dispatch_state_dir(),
            progress.draft_version,
        ))
        .unwrap_or_default();
        println!(
            "[onboard:draft] Draft v{} \u{2014} your decision:",
            progress.draft_version
        );
        println!("{body}");
        return Err(format!(
            "factory onboard: review draft v{} above — rerun with the same repo and idea using `factory onboard --new NAME --idea TEXT --approve` (or `--change TEXT`); for an existing repo, use `--repo OWNER/NAME` instead of `--new NAME`.",
            progress.draft_version
        ));
    }
    Ok("ready")
}

// --- onboard screen (ticket #43, spec #41 draft-approve loop) -------------
//
// The approve loop with eyes: her draft in sections (idea, epic, spec,
// tickets, change history), an Approve button and a Change-something
// form, and — once frozen — the handoff to the living board. Same-binary
// Axum, server-rendered HTML, polling fragment (~15s), no websockets.
//
// Reads flow through the same state files the CLI walks
// (`dispatch_state_dir`, overridable with FACTORY_STATE_DIR): the screen
// and `--plain` print one story. Writes flow only through the POST
// handlers below, which call `onboard_apply_approve` /
// `onboard_apply_change` — the same functions the CLI calls. GETs never
// mutate (snapshot-tested like the board).

/// Poll interval for the onboard fragment (board-class panel: ~15s).
pub const ONBOARD_POLL_MS: u64 = 15_000;

/// Calm sentence when no draft exists yet (Bea: an invitation, not an error).
pub fn onboard_empty_sentence() -> &'static str {
    "No draft yet \u{2014} describe your idea to begin: factory onboard --idea TEXT."
}

/// Fragment HTML for the draft (the polling unit). Pure over the draft
/// so tests assert markers without a server: `data-onboard` plus
/// `data-draft`, sections `data-idea`/`data-epic`/`data-spec`/
/// `data-tickets`, the decision forms (`data-approve-form` +
/// `data-change-form`) while unapproved, and the frozen version
/// (`data-approved`) with the board handoff (`data-board-link`) after.
pub fn onboard_fragment_html(draft: Option<&OnboardDraft>) -> String {
    let Some(d) = draft else {
        return format!(
            "<section data-onboard><p data-onboard-empty>{}</p></section>",
            onboard_empty_sentence()
        );
    };
    let mut html = format!("<section data-onboard data-draft=\"{}\">", d.version);
    html.push_str(&format!(
        "<h2>Draft v{} for {}</h2><div data-idea>{}</div>",
        d.version,
        escape_html(&d.repo),
        escape_html(&d.idea),
    ));
    html.push_str(&format!(
        "<section data-epic><h3>Epic</h3><p>{}</p></section>",
        escape_html(&d.epic)
    ));
    html.push_str(&format!(
        "<section data-spec><h3>Spec</h3><p>{}</p></section>",
        escape_html(&d.spec)
    ));
    html.push_str("<section data-tickets><h3>Tickets</h3><ol>");
    for ticket in &d.tickets {
        html.push_str(&format!("<li>{}</li>", escape_html(ticket)));
    }
    html.push_str("</ol></section>");
    if !d.changes.is_empty() {
        html.push_str("<section data-changes><h3>Change history</h3><ol>");
        for change in &d.changes {
            html.push_str(&format!("<li>{}</li>", escape_html(change)));
        }
        html.push_str("</ol></section>");
    }
    if d.approved_version > 0 {
        html.push_str(&format!(
            "<p data-approved=\"{}\">Approved plan v{} ({}) \u{2014} frozen.</p><p><a data-board-link href=\"/\">Open the board</a> and watch it move.</p>",
            d.approved_version, d.approved_version, escape_html(&d.hash),
        ));
    } else {
        html.push_str("<div data-decide><form method=\"post\" action=\"/onboard/approve\" data-approve-form><button type=\"submit\">Approve</button></form>");
        html.push_str("<form method=\"post\" action=\"/onboard/change\" data-change-form><input name=\"change\" placeholder=\"What should change?\"><button type=\"submit\">Change something</button></form></div>");
    }
    html.push_str("</section>");
    html
}

/// Full onboard page: the draft plus the polling hook that refreshes
/// only the fragment (no full reloads). Pure over the draft for testing.
pub fn onboard_page_html(draft: Option<&OnboardDraft>) -> String {
    let fragment = onboard_fragment_html(draft);
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Factory onboard</title><style>{}</style></head>\
<body><header class=\"site\"><h1>Factory onboard</h1>\
<p>Your idea, your draft, your decision. Matches <code>factory onboard --plain</code>.</p>\
<nav><a href=\"/\">Board</a> | <a href=\"/onboard\">Onboard</a> | <a href=\"/dispatch\">Dispatcher live</a></nav>\
<p data-version>factory {version}</p></header>\
<main><div id=\"onboard\" data-onboard-fragment=\"/fragments/onboard\" data-poll-ms=\"{}\">\
{fragment}</div></main>\
<script>(function(){{var el=document.getElementById('onboard');if(!el)return;var url=el.getAttribute('data-onboard-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'15000',10);\
setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
</body></html>",
        SITE_CSS,
        ONBOARD_POLL_MS,
    )
}

/// GET /onboard: the draft screen. Read-only observer.
pub async fn onboard_page_handler() -> axum::response::Html<String> {
    axum::response::Html(onboard_page_html(onboard_current_draft().as_ref()))
}

/// GET /fragments/onboard: the polling unit. Read-only observer.
pub async fn onboard_fragment_handler() -> axum::response::Html<String> {
    axum::response::Html(onboard_fragment_html(onboard_current_draft().as_ref()))
}

/// POST /onboard/approve: freeze the current version through the same
/// `onboard_apply_approve` the CLI calls, then back to the screen.
pub async fn onboard_approve_handler() -> axum::response::Redirect {
    let _ = onboard_apply_approve();
    axum::response::Redirect::to("/onboard")
}

/// Change form for POST /onboard/change (one field: her words).
#[derive(Debug, serde::Deserialize)]
pub struct OnboardChangeForm {
    pub change: String,
}

/// POST /onboard/change: mint the next version through the same
/// `onboard_apply_change` the CLI calls, then back to the screen.
pub async fn onboard_change_handler(
    axum::Form(form): axum::Form<OnboardChangeForm>,
) -> axum::response::Redirect {
    let _ = onboard_apply_change(&form.change);
    axum::response::Redirect::to("/onboard")
}

/// `factory onboard`: drive install → auth → repo → ready, resuming from
/// `<state>/onboard.json`. Exit 0 with the ready sentence; nonzero with a
/// plain-English cause naming the fix and the rerun. `--plain` selects the
/// plain transcript explicitly; `--json` carries the same facts
/// structurally. Completed steps are never repeated; created repos are
/// never created twice.
pub fn cmd_onboard(json: bool, opts: OnboardOpts) -> i32 {
    let state = dispatch_state_dir();
    let mut progress = onboard_load(&state);
    // Remember that this invocation is resuming an unapproved draft. The
    // legacy no-idea first run still reaches ready; a later resume cannot
    // bypass the decision merely because --idea was omitted.
    let pending_draft_on_entry = progress.draft_version > 0
        && progress.approved_version == 0
        && progress.steps.iter().any(|step| step == "draft")
        && !progress.steps.iter().any(|step| step == "ready");
    if opts.plain {
        println!("[onboard] Plain mode: every step in plain words, no visuals.");
    }

    let mut step: &'static str = "install";
    loop {
        let next = match step {
            "install" => onboard_step_install(&mut progress),
            "auth" => onboard_step_auth(&mut progress),
            "repo" => onboard_step_repo(&mut progress, &opts),
            "idea" => onboard_step_idea(&mut progress, &opts),
            "draft" => onboard_step_draft(&mut progress),
            "approve" => onboard_step_approve(&mut progress, &opts, pending_draft_on_entry),
            "ready" => break,
            other => {
                return fail_onboard(json, &format!("factory onboard: unknown step {other:?}"))
            }
        };
        match next {
            Ok(n) => step = n,
            Err(e) => return fail_onboard(json, &e),
        }
    }
    onboard_note_step(&mut progress, "ready");
    if let Err(e) = onboard_save(&state, &progress) {
        return fail_onboard(json, &format!("factory onboard: {e}"));
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ready": true,
                "repo": progress.repo,
                "visibility": progress.visibility,
                "created": progress.created,
                "seeded": progress.seeded,
                "steps": progress.steps,
                "draft_version": progress.draft_version,
                "approved_version": progress.approved_version,
                "draft_hash": progress.draft_hash,
            })
        );
    } else {
        println!(
            "[onboard:ready] Your repo {} is ready — the board will pick it up from here.",
            progress.repo
        );
        if progress.approved_version > 0 {
            println!(
                "[onboard:board] Approved plan v{} ({}) is on your board — watch it move: factory ui",
                progress.approved_version,
                progress.draft_hash.chars().take(12).collect::<String>(),
            );
        }
    }
    0
}

fn fail_onboard(json: bool, msg: &str) -> i32 {
    if json {
        println!("{}", serde_json::json!({"ready": false, "error": msg}));
    }
    eprintln!("{msg}");
    1
}

#[derive(Serialize)]
pub struct InitOutput {
    stack: String,
    detected_from: Option<String>,
    installed: Vec<String>,
    kept: Vec<String>,
    warnings: Vec<String>,
}

// --- embedded templates (the binary is hermetic: no network, no checkout) ---

const PROMPT_PLAN: &str = include_str!("../templates/prompts/plan.md");
const PROMPT_BUILD: &str = include_str!("../templates/prompts/build.md");
const PROMPT_REVIEW: &str = include_str!("../templates/prompts/review.md");
const PROMPT_HANDOFF: &str = include_str!("../templates/prompts/handoff.md");

const ISSUE_SPEC: &str = include_str!("../templates/issue-templates/spec.yml");
const ISSUE_TICKET: &str = include_str!("../templates/issue-templates/ticket.yml");

const ADAPTER_RUST: &str = include_str!("../templates/check-adapters/rust");
const ADAPTER_ELIXIR: &str = include_str!("../templates/check-adapters/elixir");
const ADAPTER_NODE: &str = include_str!("../templates/check-adapters/node");
const ADAPTER_PYTHON: &str = include_str!("../templates/check-adapters/python");
const ADAPTER_GO: &str = include_str!("../templates/check-adapters/go");
const ADAPTER_RUBY: &str = include_str!("../templates/check-adapters/ruby");
const ADAPTER_PHP: &str = include_str!("../templates/check-adapters/php");
const ADAPTER_JAVA: &str = include_str!("../templates/check-adapters/java");
const ADAPTER_DOTNET: &str = include_str!("../templates/check-adapters/dotnet");
const ADAPTER_SWIFT: &str = include_str!("../templates/check-adapters/swift");

const AGENTS_HEADER: &str = include_str!("../templates/header/AGENTS.header.md");
const GLOSSARY: &str = include_str!("../templates/glossary/GLOSSARY.md");
const DOCS_README: &str = include_str!("../templates/docs/README.md");

/// (relative path under a skills dir, content)
const SKILLS: &[(&str, &str)] = &[
    (
        "code-review/SKILL.md",
        include_str!("../templates/skills/code-review/SKILL.md"),
    ),
    (
        "diagnosing-bugs/SKILL.md",
        include_str!("../templates/skills/diagnosing-bugs/SKILL.md"),
    ),
    (
        "diagnosing-bugs/scripts/hitl-loop.template.sh",
        include_str!("../templates/skills/diagnosing-bugs/scripts/hitl-loop.template.sh"),
    ),
    (
        "grill-with-docs/SKILL.md",
        include_str!("../templates/skills/grill-with-docs/SKILL.md"),
    ),
    (
        "land/SKILL.md",
        include_str!("../templates/skills/land/SKILL.md"),
    ),
    (
        "pr/CREDITS.md",
        include_str!("../templates/skills/pr/CREDITS.md"),
    ),
    (
        "pr/SKILL.md",
        include_str!("../templates/skills/pr/SKILL.md"),
    ),
    (
        "retro/SKILL.md",
        include_str!("../templates/skills/retro/SKILL.md"),
    ),
    (
        "tdd/SKILL.md",
        include_str!("../templates/skills/tdd/SKILL.md"),
    ),
    (
        "tdd/mocking.md",
        include_str!("../templates/skills/tdd/mocking.md"),
    ),
    (
        "tdd/tests.md",
        include_str!("../templates/skills/tdd/tests.md"),
    ),
    (
        "to-spec/SKILL.md",
        include_str!("../templates/skills/to-spec/SKILL.md"),
    ),
    (
        "to-tickets/SKILL.md",
        include_str!("../templates/skills/to-tickets/SKILL.md"),
    ),
];

/// Factory protocol labels: (name, color, description).
const LABELS: &[(&str, &str, &str)] = &[
    (
        "spec",
        "0E8A16",
        "Product spec (authoritative at SPEC-APPROVED)",
    ),
    ("epic", "5319E7", "Epic grouping specs and tickets"),
    ("ticket", "FBCA04", "Single unit of implementation work"),
    ("ready", "1D76DB", "Ticket ready for implementation"),
    ("running", "F9D0C4", "Ticket under implementation"),
    ("review", "B60205", "Ticket awaiting independent review"),
    ("accepted", "0E8A16", "Ticket reviewed and accepted"),
    ("integrated", "5319E7", "Ticket merged into a release train"),
    (
        "blocked",
        "B60205",
        "Ticket blocked on a decision or dependency",
    ),
    (
        "spec-approved",
        "0E8A16",
        "Spec frozen — body hash recorded in SPEC-APPROVED comment",
    ),
];

fn adapter_for(stack: &str) -> Option<&'static str> {
    match stack {
        "rust" => Some(ADAPTER_RUST),
        "elixir" => Some(ADAPTER_ELIXIR),
        "node" => Some(ADAPTER_NODE),
        "python" => Some(ADAPTER_PYTHON),
        "go" => Some(ADAPTER_GO),
        "ruby" => Some(ADAPTER_RUBY),
        "php" => Some(ADAPTER_PHP),
        "java" => Some(ADAPTER_JAVA),
        "dotnet" => Some(ADAPTER_DOTNET),
        "swift" => Some(ADAPTER_SWIFT),
        _ => None,
    }
}

/// One stack family the project probe knows: plain marker filenames plus
/// filename extensions (without dot) matched against the directory listing.
/// Order is precedence: strongest manifest first, `package.json` late because
/// it is the most common secondary file (Rails+JS, Go+JS, …).
struct StackProbe {
    stack: &'static str,
    markers: &'static [&'static str],
    ext_markers: &'static [&'static str],
}

const STACK_PROBES: &[StackProbe] = &[
    StackProbe {
        stack: "rust",
        markers: &["Cargo.toml"],
        ext_markers: &[],
    },
    StackProbe {
        stack: "go",
        markers: &["go.mod"],
        ext_markers: &[],
    },
    StackProbe {
        stack: "elixir",
        markers: &["mix.exs"],
        ext_markers: &[],
    },
    StackProbe {
        stack: "ruby",
        markers: &["Gemfile", "Gemfile.lock", ".ruby-version"],
        ext_markers: &["gemspec"],
    },
    StackProbe {
        stack: "java",
        markers: &["pom.xml", "build.gradle", "build.gradle.kts"],
        ext_markers: &[],
    },
    StackProbe {
        stack: "php",
        markers: &["composer.json"],
        ext_markers: &[],
    },
    StackProbe {
        stack: "dotnet",
        markers: &["global.json"],
        ext_markers: &["csproj", "sln"],
    },
    StackProbe {
        stack: "swift",
        markers: &["Package.swift"],
        ext_markers: &[],
    },
    StackProbe {
        stack: "node",
        markers: &["package.json"],
        ext_markers: &[],
    },
    StackProbe {
        stack: "python",
        markers: &[
            "pyproject.toml",
            "requirements.txt",
            "setup.py",
            "setup.cfg",
        ],
        ext_markers: &[],
    },
];

/// Extra files that mark the node family even without a package manifest
/// (they only exist inside JavaScript/TypeScript projects) and enrich
/// node evidence when the manifest is present.
const NODE_EVIDENCE_FILES: &[&str] = &[
    "bun.lock",
    "svelte.config.js",
    "convex.json",
    "vite.config.ts",
    "vite.config.js",
    "tsconfig.json",
    "pnpm-lock.yaml",
    "yarn.lock",
];

/// A project stack guess: winning stack, human-readable evidence for it,
/// and every marker seen (winner first, precedence order).
struct DetectedStack {
    stack: String,
    evidence: String,
    markers: Vec<String>,
}

fn has_extension(dir: &Path, ext: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some(ext) {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Guess the project stack from manifest files — never assumed, always
/// reported with evidence. Empty-handed directories fall back to "generic"
/// (an honest stub, never a pretend stack). Explicit `--stack` always
/// bypasses this: it only runs when the user named no stack.
fn detect_project_stack(dir: &Path) -> DetectedStack {
    let mut markers: Vec<String> = Vec::new();
    let mut winner: Option<(String, Vec<String>)> = None;
    for probe in STACK_PROBES {
        let mut found: Vec<String> = probe
            .markers
            .iter()
            .filter(|m| dir.join(m).is_file())
            .map(|m| m.to_string())
            .collect();
        for ext in probe.ext_markers {
            if let Some(name) = has_extension(dir, ext) {
                found.push(name);
            }
        }
        if found.is_empty() {
            // Node-family config files are weak markers on their own: they
            // only exist inside JavaScript/TypeScript projects.
            if probe.stack == "node" {
                for extra in NODE_EVIDENCE_FILES {
                    if dir.join(extra).is_file() {
                        found.push(extra.to_string());
                    }
                }
            }
        }
        if found.is_empty() {
            continue;
        }
        markers.push(format!("{} ({})", probe.stack, found.join(", ")));
        if winner.is_none() {
            winner = Some((probe.stack.to_string(), found));
        }
    }
    match winner {
        None => DetectedStack {
            stack: "generic".to_string(),
            evidence: "no stack markers found".to_string(),
            markers,
        },
        Some((stack, mut files)) => {
            if stack == "node" {
                for extra in NODE_EVIDENCE_FILES {
                    if !files.iter().any(|f| f == extra) && dir.join(extra).is_file() {
                        files.push(extra.to_string());
                    }
                }
            }
            DetectedStack {
                evidence: files.join(" + "),
                markers,
                stack,
            }
        }
    }
}

/// Generic check adapter, parameterized by stack name. It probes for the
/// obvious tools of the known families (manifest-gated, so a Ruby checkout
/// never runs Go tools), runs whatever it finds in contract order, skips
/// with a reason what it lacks, and fails with wiring guidance when the
/// project has markers but nothing runnable. Empty projects pass cleanly.
/// The `# STACK:` line lets the refresh path recognize the seed.
fn generic_adapter(stack: &str) -> String {
    format!(
        r###"#!/usr/bin/env bash
# factory-check (generic stub for '{stack}')
# STACK: {stack}
# Seeded by `factory init`: no first-class adapter for '{stack}' yet.
# Contract: format check -> lint/compile -> tests; exit non-zero on ANY
# real failure; print FACTORY-CHECK PASSED only on success. Extra args pass
# through to the test runner. This stub runs whatever it finds (each stage
# is manifest-gated), skips with a reason what it lacks, and fails with
# wiring guidance when nothing runnable matches. Edit this file to wire
# your runner precisely — seeds never overwrite, so customizations stick.
set -euo pipefail

cd "$(dirname "$0")/.."

have() {{ command -v "$1" >/dev/null 2>&1; }}

ran_any=0
note() {{ echo "(skip: $1)"; }}

# --- manifest inventory: a markerless tree has nothing to check ---
if ! {{ ls Cargo.toml go.mod go.sum mix.exs Gemfile Gemfile.lock pom.xml build.gradle build.gradle.kts settings.gradle composer.json Package.swift package.json pyproject.toml requirements.txt setup.py setup.cfg stack.yaml dune-project dune-workspace CMakeLists.txt meson.build Makefile Pipfile poetry.lock *.gemspec *.csproj *.sln *.cabal 2>/dev/null || true; }} | grep -q .; then
  echo "factory-check: no project markers yet — nothing to check"
  echo "FACTORY-CHECK PASSED"
  exit 0
fi

fail_missing() {{
  echo "factory-check: '{stack}' project detected but no runnable checks found (missing toolchains?)"
  echo "Edit scripts/factory-check to wire your runner (contract: format→lint→test, non-zero on fail)"
  exit 1
}}

if [ -f Cargo.toml ]; then
  have cargo || fail_missing
  echo "==> cargo fmt --check"; cargo fmt --check
  echo "==> cargo clippy (deny warnings)"; cargo clippy --all-targets -- -D warnings
  echo "==> cargo test"; cargo test -- "$@"
  ran_any=1
elif [ -f go.mod ]; then
  have go || fail_missing
  echo "==> gofmt -l"; test -z "$(gofmt -l .)"
  echo "==> go vet"; go vet ./...
  echo "==> go test"; go test ./... "$@"
  ran_any=1
elif [ -f mix.exs ]; then
  have mix || fail_missing
  echo "==> mix format --check-formatted"; mix format --check-formatted
  echo "==> mix compile (warnings as errors)"; mix compile --warnings-as-errors
  echo "==> mix test"; MIX_ENV=test mix test "$@"
  ran_any=1
elif [ -f Gemfile ] || ls *.gemspec >/dev/null 2>&1; then
  have bundle || fail_missing
  if bundle exec rubocop -V >/dev/null 2>&1; then echo "==> rubocop"; bundle exec rubocop; else note "rubocop not available"; fi
  if [ -f Rakefile ]; then echo "==> rake test"; bundle exec rake test "$@"; elif [ -d spec ] && bundle exec rspec -V >/dev/null 2>&1; then echo "==> rspec"; bundle exec rspec "$@"; else note "no Rakefile or spec/ suite found"; fi
  ran_any=1
elif [ -f pom.xml ] || [ -f build.gradle ] || [ -f build.gradle.kts ]; then
  if [ -f pom.xml ]; then have mvn || fail_missing; echo "==> mvn test"; mvn -B -q test "$@";
  elif [ -x ./gradlew ]; then echo "==> gradlew test"; ./gradlew test "$@";
  elif have gradle; then echo "==> gradle test"; gradle test "$@";
  else fail_missing; fi
  ran_any=1
elif [ -f composer.json ]; then
  have php || fail_missing
  if [ -x vendor/bin/pint ]; then echo "==> pint --test"; vendor/bin/pint --test; else note "pint not installed"; fi
  if [ -x vendor/bin/phpunit ]; then echo "==> phpunit"; vendor/bin/phpunit "$@"; else note "phpunit not installed"; fi
  ran_any=1
elif ls *.csproj *.sln >/dev/null 2>&1; then
  have dotnet || fail_missing
  echo "==> dotnet format (verify)"; dotnet format --verify-no-changes --verbosity quiet || note "dotnet format reported changes (run: dotnet format)"
  echo "==> dotnet test"; dotnet test "$@"
  ran_any=1
elif [ -f Package.swift ]; then
  have swift || fail_missing
  if have swiftformat; then echo "==> swiftformat --lint"; swiftformat --lint .; else note "swiftformat not installed"; fi
  echo "==> swift test"; swift test "$@"
  ran_any=1
elif [ -f package.json ]; then
  if have npx && [ -f .prettierrc ] || [ -f .prettierrc.json ] || grep -q '"prettier"' package.json 2>/dev/null; then echo "==> prettier --check"; npx prettier --check .; else note "prettier not configured"; fi
  if {{ ls eslint.config.* .eslintrc* 2>/dev/null || true; }} | grep -q .; then echo "==> eslint"; npx eslint .; else note "eslint not configured"; fi
  echo "==> npm test"; npm test -- "$@"
  ran_any=1
elif [ -f pyproject.toml ] || [ -f requirements.txt ] || [ -f setup.py ]; then
  if have ruff; then echo "==> ruff format --check"; ruff format --check .; echo "==> ruff check"; ruff check .; else note "ruff not installed"; fi
  have pytest || have python || fail_missing
  echo "==> pytest"; python -m pytest "$@"
  ran_any=1
fi

if [ "$ran_any" = 0 ]; then
  fail_missing
fi

echo "FACTORY-CHECK PASSED"
"###,
        stack = stack,
    )
}

/// Adapter text for a stack: the embedded first-class script when one
/// exists, otherwise the generic stub parameterized by stack name.
/// Returns the text plus whether it is first-class.
fn adapter_text(stack: &str) -> (String, bool) {
    match adapter_for(stack) {
        Some(adapter) => (adapter.to_string(), true),
        None => (generic_adapter(stack), false),
    }
}

/// Installed stack, detected from the seeded adapter — never assumed.
/// Byte-matches first-class scripts, parses the `# STACK:` marker on
/// templated stubs, and falls back to "rust" for missing or fully custom
/// adapters: seeding never overwrites, so a guess only affects a missing
/// adapter plus the reported stack string.
fn detect_stack(dest: &Path) -> String {
    let body = std::fs::read_to_string(dest.join("scripts/factory-check")).unwrap_or_default();
    for (name, adapter) in [
        ("rust", ADAPTER_RUST),
        ("elixir", ADAPTER_ELIXIR),
        ("node", ADAPTER_NODE),
        ("python", ADAPTER_PYTHON),
        ("go", ADAPTER_GO),
        ("ruby", ADAPTER_RUBY),
        ("php", ADAPTER_PHP),
        ("java", ADAPTER_JAVA),
        ("dotnet", ADAPTER_DOTNET),
        ("swift", ADAPTER_SWIFT),
    ] {
        if body == adapter {
            return name.to_string();
        }
    }
    for line in body.lines() {
        if let Some(name) = line.strip_prefix("# STACK:") {
            let name = name.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    "rust".to_string()
}

fn tool_present(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

struct Installer {
    dest: PathBuf,
    installed: Vec<String>,
    kept: Vec<String>,
    warnings: Vec<String>,
    json: bool,
}

impl Installer {
    fn say(&self, line: &str) {
        if !self.json {
            println!("{line}");
        }
    }

    fn warn(&mut self, line: &str) {
        eprintln!("{line}");
        self.warnings.push(line.to_string());
    }

    /// Write `content` to `rel` unless it already exists. Never overwrites.
    fn seed(&mut self, rel: &str, content: &str) {
        let target = self.dest.join(rel);
        if target.exists() {
            self.kept.push(rel.to_string());
            self.say(&format!("keep: {rel} exists, not overwriting"));
            return;
        }
        if let Some(parent) = target.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.warn(&format!("WARN: cannot create {}: {e}", parent.display()));
                return;
            }
        }
        match std::fs::write(&target, content) {
            Ok(()) => {
                self.installed.push(rel.to_string());
                self.say(&format!("install: {rel}"));
            }
            Err(e) => self.warn(&format!("WARN: cannot write {rel}: {e}")),
        }
    }

    fn seed_executable(&mut self, rel: &str, content: &str) {
        // Only chmod what was newly installed: a kept file is reported as
        // untouched, so its permissions must stay untouched too. (A present
        // but non-executable adapter is --fix territory: doctor R3b.)
        let fresh = !self.dest.join(rel).exists();
        self.seed(rel, content);
        if !fresh {
            return;
        }
        #[cfg(unix)]
        {
            let target = self.dest.join(rel);
            if target.exists() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
            }
        }
    }

    fn ensure_labels(&mut self, gh_ok: bool) {
        if !gh_ok {
            self.say(
                "skip: gh not authenticated — copy .github/ISSUE_TEMPLATE manually and create labels (spec, epic, ticket, ready, running, review, accepted, integrated, blocked, spec-approved)",
            );
            return;
        }
        let mut failed = false;
        for (name, color, desc) in LABELS {
            let created = std::process::Command::new("gh")
                .args([
                    "label",
                    "create",
                    name,
                    "--color",
                    color,
                    "--description",
                    desc,
                ])
                .current_dir(&self.dest)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if created {
                continue;
            }
            let edited = std::process::Command::new("gh")
                .args([
                    "label",
                    "edit",
                    name,
                    "--color",
                    color,
                    "--description",
                    desc,
                ])
                .current_dir(&self.dest)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !edited {
                failed = true;
            }
        }
        if failed {
            self.warn("WARN: some labels could not be ensured via gh — create them from the factory protocol set manually");
        } else {
            self.say("labels: ensured factory label set");
        }
    }

    fn finish_dirs_and_markers(&mut self) {
        for d in [
            "factory/.cache",
            "docs/product",
            "docs/architecture",
            "docs/adr",
            "docs/runbooks",
            "scripts",
        ] {
            let _ = std::fs::create_dir_all(self.dest.join(d));
        }
        // gitignore cache entry (created when absent, appended when present)
        let gi = self.dest.join(".gitignore");
        if gi.is_file() {
            let body = std::fs::read_to_string(&gi).unwrap_or_default();
            if !body.contains("factory/.cache/") {
                let mut body = body;
                if !body.ends_with('\n') {
                    body.push('\n');
                }
                body.push_str("factory/.cache/\n");
                if std::fs::write(&gi, body).is_ok() {
                    self.installed.push(".gitignore".to_string());
                    self.say("install: .gitignore (factory/.cache/)");
                }
            }
        } else if std::fs::write(&gi, "factory/.cache/\n").is_ok() {
            self.installed.push(".gitignore".to_string());
            self.say("install: .gitignore (factory/.cache/)");
        } else {
            self.warn("WARN: cannot create .gitignore");
        }
        // version marker (always records the running binary's version)
        let want = env!("CARGO_PKG_VERSION");
        let vf = self.dest.join(".factory-version");
        let cur = std::fs::read_to_string(&vf).unwrap_or_default();
        if cur.trim() != want {
            if std::fs::write(&vf, format!("{want}\n")).is_ok() {
                self.installed.push(".factory-version".to_string());
                self.say("install: .factory-version");
            }
        } else {
            self.kept.push(".factory-version".to_string());
        }
    }
}

/// Seed every project-local file init owns (prompts, skills, adapter,
/// AGENTS header, glossary, docs, issue templates, dirs + markers).
/// Shared by `init`/`update` and `doctor --fix` so repair re-seeds through
/// init's own path. Seeded files never overwrite: existing files are kept,
/// which is what makes both init and --fix idempotent. Exception: the two
/// markers in finish_dirs_and_markers (.factory-version is always rewritten
/// to the running binary's version; .gitignore gains the cache entry) —
/// both are recorded in `installed` so --fix can report them. No gh calls
/// in here — label management stays in run_init so --fix never touches issues.
fn seed_project_local(
    ins: &mut Installer,
    stack: &str,
    cmd: &str,
    skill_root: &Path,
    global_skills: bool,
) {
    // --- prompts ---
    ins.seed(".pi/prompts/plan.md", PROMPT_PLAN);
    ins.seed(".pi/prompts/build.md", PROMPT_BUILD);
    ins.seed(".pi/prompts/review.md", PROMPT_REVIEW);
    ins.seed(".pi/prompts/handoff.md", PROMPT_HANDOFF);

    // --- skills (hermetic; never overwrite existing) ---
    for (rel, content) in SKILLS {
        let target = skill_root.join(rel);
        let display = if global_skills {
            format!("~/.agents/skills/{rel}")
        } else {
            format!(".agents/skills/{rel}")
        };
        if target.exists() {
            ins.kept.push(display.clone());
            ins.say(&format!("keep: {display} exists, not overwriting"));
            continue;
        }
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&target, content) {
            Ok(()) => {
                ins.installed.push(display.clone());
                ins.say(&format!("install: {display}"));
            }
            Err(e) => ins.warn(&format!("WARN: cannot write {display}: {e}")),
        }
    }

    // --- check adapter for the stack (first-class script, else generic stub) ---
    let (text, first_class) = adapter_text(stack);
    ins.seed_executable("scripts/factory-check", &text);
    if cmd == "init" {
        if first_class {
            ins.say(&format!("check adapter: {stack}"));
        } else {
            ins.say(&format!(
                "check adapter: {stack} (generic stub — edit scripts/factory-check to wire your runner)"
            ));
        }
    }

    // --- AGENTS.md header (marker-based merge) ---
    let agents = ins.dest.join("AGENTS.md");
    match std::fs::read_to_string(&agents) {
        Ok(body) if body.contains("factory:header:start") => {
            ins.kept.push("AGENTS.md".to_string());
            ins.say("keep: AGENTS.md already has factory header");
        }
        Ok(body) => {
            let merged = format!("{AGENTS_HEADER}\n{body}");
            match std::fs::write(&agents, merged) {
                Ok(()) => {
                    ins.installed.push("AGENTS.md".to_string());
                    ins.say("merged: factory header prepended to AGENTS.md (review the join)");
                }
                Err(e) => ins.warn(&format!("WARN: cannot merge AGENTS.md: {e}")),
            }
        }
        Err(_) => ins.seed("AGENTS.md", AGENTS_HEADER),
    }

    // --- glossary + docs skeleton (only if missing) ---
    ins.seed("GLOSSARY.md", GLOSSARY);
    ins.seed("docs/README.md", DOCS_README);

    // --- issue templates (local files; gh only needed for labels) ---
    ins.seed(".github/ISSUE_TEMPLATE/spec.yml", ISSUE_SPEC);
    ins.seed(".github/ISSUE_TEMPLATE/ticket.yml", ISSUE_TICKET);

    ins.finish_dirs_and_markers();
}

// --- bounded guided mode (ticket #49, spec #45) ----------------------------
//
// `factory init --interactive` asks at most two numbered stdin questions
// (stack, skills location) with plain text and no new dependencies. It
// also engages automatically when detection found several stacks and
// stdin is an interactive terminal. Machine-readable mode (--json) and
// piped/non-terminal stdin never prompt and never block: the gate below
// refuses before any read, so scripts and CI cannot hang. Each question
// reads exactly one line with no retry loop; empty, invalid, or EOF
// input falls back to the default (the detection winner for stacks).

/// Stack choices in detection-precedence order plus the generic fallback.
/// Stable: tests address options by number (node is 9, generic is 11).
const INTERACTIVE_STACKS: &[&str] = &[
    "rust", "go", "elixir", "ruby", "java", "php", "dotnet", "swift", "node", "python", "generic",
];

/// Skills-location choices: index 0 is project-local, index 1 is global.
/// The default index follows the `--global-skills` flag.
const INTERACTIVE_SKILLS: &[&str] = &["project (.agents/skills)", "global (~/.agents/skills)"];

/// Parse one choice line into a 0-based option index. Empty, unparseable,
/// or out-of-range input falls back to `default_idx` — prompting never
/// retries, so a question is always exactly one read and one answer.
fn parse_choice_line(line: &str, n_options: usize, default_idx: usize) -> usize {
    let t = line.trim();
    if t.is_empty() {
        return default_idx;
    }
    match t.parse::<usize>() {
        Ok(n) if n >= 1 && n <= n_options => n - 1,
        _ => default_idx,
    }
}

/// True when stdin may be prompted: a real terminal, or the test-only
/// `FACTORY_FORCE_TTY` seam (piped-stdin tests use it to simulate a
/// terminal; the single `read_line` still returns immediately on EOF, so
/// even forced runs can never hang waiting for an answer).
fn stdin_is_interactive() -> bool {
    if std::env::var_os("FACTORY_FORCE_TTY").is_some() {
        return true;
    }
    std::io::IsTerminal::is_terminal(&std::io::stdin())
}

/// Ask one numbered question on stderr, reading exactly one stdin line.
/// Returns the 0-based index of the chosen option (`default_idx` on
/// empty/invalid/EOF). Prompts go to stderr so stdout stays clean for
/// automation; callers gate on `!json && stdin_is_interactive()` first.
fn prompt_numbered(title: &str, options: &[&str], default_idx: usize) -> usize {
    use std::io::Write;
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{title}");
    for (i, opt) in options.iter().enumerate() {
        let marker = if i == default_idx { " (default)" } else { "" };
        let _ = writeln!(err, "  {}) {opt}{marker}", i + 1);
    }
    let _ = write!(err, "Enter a number [{}]: ", default_idx + 1);
    let _ = err.flush();
    drop(err);
    let mut line = String::new();
    match std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line) {
        Ok(_) => parse_choice_line(&line, options.len(), default_idx),
        Err(_) => default_idx,
    }
}

pub fn run_init(
    json: bool,
    stack: Option<&str>,
    mut global_skills: bool,
    pin: Option<&str>,
    cmd: &str,
    interactive: bool,
) -> InitOutput {
    let dest = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // No explicit stack: detect from the project and report the evidence.
    // Explicit `--stack` (even unknown) always wins over detection.
    let (mut stack_owned, detected): (String, Option<DetectedStack>) = match stack {
        Some(s) => (s.to_string(), None),
        None => {
            let d = detect_project_stack(&dest);
            (d.stack.clone(), Some(d))
        }
    };
    // --- bounded guided mode (ticket #49): resolve at most two answers ---
    // Init only, never under --json or piped stdin (the gate refuses
    // before any read, so scripts and CI cannot hang). The stack question
    // runs when flagged, or automatically when detection found several
    // stacks — ambiguity is surfaced, never guessed silently. The skills
    // question runs only when flagged. Explicit `--stack` always wins.
    let mut asked_stack = false;
    let mut asked_skills = false;
    if cmd == "init" && !json && stdin_is_interactive() {
        let ambiguous = detected
            .as_ref()
            .map(|d| d.markers.len() > 1)
            .unwrap_or(false);
        if stack.is_none() && (interactive || ambiguous) {
            let winner = detected
                .as_ref()
                .map(|d| d.stack.as_str())
                .unwrap_or("generic");
            let default_idx = INTERACTIVE_STACKS
                .iter()
                .position(|s| *s == winner)
                .unwrap_or(INTERACTIVE_STACKS.len() - 1);
            let idx = prompt_numbered("Select a stack", INTERACTIVE_STACKS, default_idx);
            asked_stack = true;
            stack_owned = INTERACTIVE_STACKS[idx].to_string();
        }
        if interactive {
            let default_idx = if global_skills { 1 } else { 0 };
            let idx = prompt_numbered(
                "Where should skills be installed?",
                INTERACTIVE_SKILLS,
                default_idx,
            );
            asked_skills = true;
            global_skills = idx == 1;
        }
    }
    let stack: &str = &stack_owned;
    let dest = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut ins = Installer {
        dest: dest.clone(),
        installed: Vec::new(),
        kept: Vec::new(),
        warnings: Vec::new(),
        json,
    };

    // --- prerequisites (warn, don't fail) ---
    let gh_present = tool_present("gh");
    if !gh_present {
        ins.warn(
            "WARN: gh CLI not found — issue templates/labels will be skipped. Install: https://cli.github.com",
        );
    }
    if !tool_present("herdr") {
        ins.warn(
            "WARN: herdr not found — install from https://herdr.dev then run: herdr integration install pi",
        );
    }

    // --- local tree: init's own seeding path (shared with doctor --fix) ---
    let skill_root = if global_skills {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Path::new(&home).join(".agents/skills")
    } else {
        dest.join(".agents/skills")
    };
    seed_project_local(&mut ins, stack, cmd, &skill_root, global_skills);

    // Report what detection found (explicit stacks need no explanation).
    let mut detected_from: Option<String> = None;
    if let Some(d) = detected {
        detected_from = Some(d.evidence.clone());
        if cmd == "init" {
            ins.say(&format!("detected stack: {stack} ({})", d.evidence));
            let others: Vec<&str> = d
                .markers
                .iter()
                .filter(|m| !m.starts_with(&format!("{stack} (")))
                .map(|m| m.as_str())
                .collect();
            if !others.is_empty() {
                ins.say(&format!(
                    "note: also found {}; using {stack} — pass --stack to override",
                    others.join(", ")
                ));
            }
            if d.markers.is_empty() {
                ins.warn(
                    "WARN: no stack markers found — seeded generic check adapter (edit scripts/factory-check to wire your runner)",
                );
            }
        }
    }

    // Guided answers are reported only when a question was actually asked,
    // so unflagged non-interactive output stays byte-for-byte unchanged.
    if asked_stack {
        ins.say(&format!("selected stack: {stack}"));
    }
    if asked_skills {
        ins.say(&format!(
            "skills location: {}",
            if global_skills {
                "global (~/.agents/skills)"
            } else {
                "project (.agents/skills)"
            }
        ));
    }

    // --- labels (best effort via gh) ---
    let authed = gh_present
        && std::process::Command::new("gh")
            .args(["auth", "status"])
            .current_dir(&dest)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    if gh_present && !authed {
        ins.say(
            "skip: gh not authenticated — copy .github/ISSUE_TEMPLATE manually and create labels from the factory protocol set",
        );
    } else {
        ins.ensure_labels(authed);
    }

    if let Some(tag) = pin {
        ins.say(&format!(
            "pin: {tag} acknowledged (this binary carries embedded templates; network refresh is out of scope — nothing is persisted)"
        ));
    }

    let done = cmd;
    ins.say(&format!(
        "factory {done} done ({stack}). Next: fill GLOSSARY.md, file epic issue, /plan."
    ));
    ins.say(
        "init is idempotent and never overwrites existing files; delete a file to force refresh.",
    );

    InitOutput {
        stack: stack.to_string(),
        detected_from,
        installed: ins.installed,
        kept: ins.kept,
        warnings: ins.warnings,
    }
}

#[derive(Serialize)]
struct FreezeOutput<'a> {
    issue: u64,
    label: &'a str,
    hash: &'a str,
}

#[derive(Serialize)]
struct SyncOutput {
    issue: u64,
    hash: String,
    approved: Option<String>,
    ok: bool,
    cache: String,
}

#[derive(Serialize)]
struct VerifyOutput {
    issue: u64,
    hash: String,
    approved: Option<String>,
    ok: bool,
}

#[derive(Serialize)]
struct CheckOutput<'a> {
    passed: bool,
    adapter: &'a str,
    slot: Option<&'a str>,
    code: i32,
}

/// Nearest `scripts/factory-check` walking up from the current directory.
fn discover_adapter() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("scripts").join("factory-check");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub fn run_check(
    json: bool,
    adapter: Option<PathBuf>,
    slot: Option<String>,
    args: Vec<String>,
) -> ! {
    let adapter_path: Option<PathBuf> = adapter
        .or_else(|| std::env::var_os("FACTORY_CHECK_ADAPTER").map(PathBuf::from))
        .or_else(discover_adapter);

    let Some(adapter_path) = adapter_path else {
        eprintln!(
            "factory check: no check adapter found (pass --adapter, set FACTORY_CHECK_ADAPTER, or run under a project with scripts/factory-check)"
        );
        if json {
            println!(
                "{}",
                serde_json::to_string(&CheckOutput {
                    passed: false,
                    adapter: "",
                    slot: slot.as_deref(),
                    code: 1,
                })
                .expect("check JSON serialization")
            );
        }
        std::process::exit(1);
    };

    // Slot isolation: an explicit --slot wins; otherwise the adapter
    // inherits whatever FACTORY_SLOT the caller already set.
    let effective_slot: Option<String> = slot.or_else(|| std::env::var("FACTORY_SLOT").ok());

    let mut child = std::process::Command::new(&adapter_path);
    child.args(&args);
    if let Some(slot) = &effective_slot {
        child.env("FACTORY_SLOT", slot);
    }

    let status = match child.status() {
        Ok(status) => status,
        Err(e) => {
            eprintln!(
                "factory check: cannot execute adapter {}: {e}",
                adapter_path.display()
            );
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&CheckOutput {
                        passed: false,
                        adapter: &adapter_path.to_string_lossy(),
                        slot: effective_slot.as_deref(),
                        code: 1,
                    })
                    .expect("check JSON serialization")
                );
            }
            std::process::exit(1);
        }
    };

    let code = status.code().unwrap_or(1);
    let passed = status.success();
    if json {
        println!(
            "{}",
            serde_json::to_string(&CheckOutput {
                passed,
                adapter: &adapter_path.to_string_lossy(),
                slot: effective_slot.as_deref(),
                code,
            })
            .expect("check JSON serialization")
        );
    }
    std::process::exit(if passed { 0 } else { code.max(1) })
}

// ---------------------------------------------------------------------------
// DOCTOR (ticket #18: diagnose — checks + report + JSON)
//
// Pre-flight check over the catalogue in DOCTOR_ORDER. Every probe reads
// the real environment through the existing shell-out seams (gh via dgh
// with its mise-noise filtering, git/pi binaries, adapter discovery,
// state dir, PATH toolchain) — no API client, no new seam. Doctor is
// read-only: it never writes files, issues, pushes, or starts auth flows;
// failures carry the exact fix command instead. `--fix` (ticket #19, safe
// local repair) applies doctor_fix() first, then reports with the same
// doctor_checks() — so a green post-repair report means the same thing as
// a green plain report. The catalogue lives in this one module
// so later tickets (daemon supervision, policy files) extend DOCTOR_ORDER
// without touching the runner.
// ---------------------------------------------------------------------------

/// Catalogue order. Stable machine names; automation may allowlist them.
const DOCTOR_ORDER: &[&str] = &[
    "gh-installed",
    "gh-auth",
    "gh-repo-access",
    "git-identity",
    "state-dirs",
    "adapter-present",
    "adapter-executable",
    "toolchain",
    "model-runner",
];

#[derive(Serialize, Clone)]
pub struct DoctorCheck {
    name: &'static str,
    ok: bool,
    detail: String,
    hint: String,
}

#[derive(Serialize)]
struct DoctorOutput {
    ok: bool,
    checks: Vec<DoctorCheck>,
    /// Repairs applied by `--fix`, in the order applied. Always present;
    /// empty without `--fix` or when a `--fix` run had nothing to repair.
    fixed: Vec<String>,
}

// ---------------------------------------------------------------------------
// DOCTOR --fix (ticket #19: safe local repair)
//
// Repairs FIRST, then probes with the same doctor_checks() the read-only
// path uses — so a green post-repair report means the same thing as a
// green plain report. Only safe local state is ever touched:
//   R1 dirs: create a missing state dir; restore the write bit when the
//      readonly bit is set. A path blocked by a non-dir file is left
//      failing with its hint — never delete or move user data.
//   R2 worktree metadata: `git worktree prune -n` (dry run) detects stale
//      entries; only when its stdout is non-empty does the real `git
//      worktree prune` run. Local git metadata only — never fetch, push,
//      remotes, branches, or commits.
//   R3 project files: re-seed missing files through init's own seeding
//      path (seed_project_local, default rust stack, never overwrites),
//      plus chmod +x when the discovered adapter exists but is not
//      executable — exactly the hint's own command.
// No new `gh` calls happen here (probes already ran or will run around
// the repairs): no issues, no labels, no auth flows. No toolchain
// installers, ever — those stay report + hint only.
// ---------------------------------------------------------------------------

// Stack re-seed for repair comes from project detection (same as init).
// `--fix` takes no --stack flag (spec: `doctor [--fix] [--json]`).

pub fn doctor_fix() -> Vec<String> {
    let mut fixed = Vec::new();

    // R1: state + cache dirs (create when missing; restore the write bit
    // when the readonly bit is set). A path blocked by a non-dir file is
    // left failing with its hint — never delete or move user data.
    let state = dispatch_state_dir();
    let cache = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("factory")
        .join(".cache");
    for dir in [&state, &cache] {
        match std::fs::metadata(dir) {
            Err(_) => {
                if std::fs::create_dir_all(dir).is_ok() {
                    fixed.push(format!("state-dirs: created {}", dir.display()));
                }
            }
            Ok(m) if m.is_dir() && m.permissions().readonly() => {
                // Restore owner write without widening to world-writable.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = m.permissions().mode() | 0o200;
                    if std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).is_ok()
                    {
                        fixed.push(format!("state-dirs: made writable {}", dir.display()));
                    }
                }
                #[cfg(not(unix))]
                {
                    let mut perms = m.permissions();
                    perms.set_readonly(false);
                    if std::fs::set_permissions(dir, perms).is_ok() {
                        fixed.push(format!("state-dirs: made writable {}", dir.display()));
                    }
                }
            }
            Ok(_) => {}
        }
    }

    // R2: stale worktree metadata (detect via dry run, repair only on signal).
    // The repair entry reflects the real prune call: a failed prune is
    // surfaced, never claimed as done.
    if let Some(dry) = probe_stdout(&git_bin(), &["worktree", "prune", "-n"]) {
        if !dry.is_empty() {
            let first = dry.lines().next().unwrap_or("stale entries");
            match probe_stdout(&git_bin(), &["worktree", "prune"]) {
                Some(_) => fixed.push(format!("worktree-metadata: pruned ({first})")),
                None => fixed.push(
                    "worktree-metadata: prune failed — retry: git worktree prune".to_string(),
                ),
            }
        }
    }

    // R3: missing project files through init's own seeding path.
    let dest = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut ins = Installer {
        dest: dest.clone(),
        installed: Vec::new(),
        kept: Vec::new(),
        warnings: Vec::new(),
        json: true, // quiet: repairs are reported by doctor, not init chatter
    };
    let detected = detect_project_stack(&dest);
    seed_project_local(
        &mut ins,
        &detected.stack,
        "doctor",
        &dest.join(".agents/skills"),
        false,
    );
    for rel in &ins.installed {
        fixed.push(format!("project-files: re-seeded {rel}"));
    }

    // R3b: existing-but-not-executable adapter (fresh seeds are already 755).
    if let Some(p) = doctor_adapter() {
        if p.is_file() && !path_is_executable(&p) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).is_ok() {
                    fixed.push(format!("adapter-executable: chmod +x {}", p.display()));
                }
            }
        }
    }

    fixed
}

fn doctor_pass(name: &'static str, detail: String) -> DoctorCheck {
    DoctorCheck {
        name,
        ok: true,
        detail,
        hint: String::new(),
    }
}

fn doctor_fail(name: &'static str, detail: String, hint: String) -> DoctorCheck {
    DoctorCheck {
        name,
        ok: false,
        detail,
        hint,
    }
}

/// Run `program args`, returning trimmed stdout on exit 0, None otherwise
/// (including spawn failure, e.g. binary missing from PATH).
fn probe_stdout(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Resolve the check adapter exactly as the `check` runner does.
fn doctor_adapter() -> Option<PathBuf> {
    std::env::var_os("FACTORY_CHECK_ADAPTER")
        .map(PathBuf::from)
        .or_else(discover_adapter)
}

#[cfg(unix)]
fn path_is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn path_is_executable(path: &Path) -> bool {
    path.is_file()
}

pub fn doctor_checks() -> Vec<DoctorCheck> {
    let mut checks = Vec::with_capacity(DOCTOR_ORDER.len());

    // gh-installed: the gh shell-out seam must answer (via dgh, which
    // filters version-manager noise like the rest of the gh call sites).
    let gh_version = dgh(&["--version"]).ok();
    checks.push(match gh_version {
        Some(v) => doctor_pass(
            "gh-installed",
            format!(
                "GitHub CLI found ({}).",
                v.split('\n').next().unwrap_or("gh")
            ),
        ),
        None => doctor_fail(
            "gh-installed",
            format!("GitHub CLI not found (looked for `{}` on PATH).", gh_bin()),
            "Install the GitHub CLI: https://cli.github.com — then re-run: factory doctor"
                .to_string(),
        ),
    });

    // gh-auth: login must be live.
    let auth = dgh(&["auth", "status"]).ok();
    checks.push(match auth {
        Some(_) => doctor_pass(
            "gh-auth",
            "GitHub login works — gh commands can authenticate.".to_string(),
        ),
        None => doctor_fail(
            "gh-auth",
            "GitHub login is missing or expired — gh cannot authenticate.".to_string(),
            "Run: gh auth login — then re-run: factory doctor".to_string(),
        ),
    });

    // gh-repo-access: the current repo must be readable via gh.
    let repo = dgh(&["repo", "view", "--json", "nameWithOwner"]).ok();
    checks.push(match repo {
        Some(v) => doctor_pass(
            "gh-repo-access",
            format!(
                "Can read this repo through gh ({}).",
                serde_json::from_str::<serde_json::Value>(&v)
                    .ok()
                    .and_then(|j| j
                        .get("nameWithOwner")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string()))
                    .unwrap_or_else(|| "repo view works".to_string())
            ),
        ),
        None => doctor_fail(
            "gh-repo-access",
            "Cannot read this repo through gh — wrong directory, no access, or login expired."
                .to_string(),
            "Run: gh repo view — to see the cause; for access ask a maintainer, for login run: gh auth login".to_string(),
        ),
    });

    // git-identity: dispatcher commits need name + email.
    let name = probe_stdout(&git_bin(), &["config", "user.name"]).unwrap_or_default();
    let email = probe_stdout(&git_bin(), &["config", "user.email"]).unwrap_or_default();
    checks.push(if !name.is_empty() && !email.is_empty() {
        doctor_pass(
            "git-identity",
            format!("Git identity is set ({name} <{email}>)."),
        )
    } else {
        doctor_fail(
            "git-identity",
            "Git name/email is not configured — dispatcher commits would fail.".to_string(),
            "Run: git config --global user.name \"Your Name\" && git config --global user.email \"you@example.com\""
                .to_string(),
        )
    });

    // state-dirs: state + cache dirs must exist, be dirs, and be writable;
    // worktree metadata must be consistent (no stale entries). Read-only
    // probe: existence + dir + permission bits + prune dry-run, never creates.
    let state = dispatch_state_dir();
    let cache = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("factory")
        .join(".cache");
    let mut dir_problems: Vec<(String, String)> = Vec::new();
    for (label, dir) in [("State", &state), ("Cache", &cache)] {
        match std::fs::metadata(dir) {
            Ok(m) if !m.is_dir() => dir_problems.push((
                format!(
                    "{label} path {} exists but is not a directory.",
                    dir.display()
                ),
                format!(
                    "Run: mv {} {}.bak && mkdir -p {}",
                    dir.display(),
                    dir.display(),
                    dir.display()
                ),
            )),
            Ok(m) if m.permissions().readonly() => dir_problems.push((
                format!("{label} directory {} is not writable.", dir.display()),
                format!("Run: chmod u+rwX {}", dir.display()),
            )),
            Ok(_) => {}
            Err(_) => dir_problems.push((
                format!("{label} directory {} does not exist yet.", dir.display()),
                format!("Run: mkdir -p {}", dir.display()),
            )),
        }
    }
    match probe_stdout(&git_bin(), &["worktree", "prune", "-n"]) {
        Some(dry) if !dry.is_empty() => dir_problems.push((
            format!(
                "Stale worktree metadata: {}.",
                dry.lines().next().unwrap_or("stale entries")
            ),
            "Run: git worktree prune — to clear stale entries (or: factory doctor --fix)"
                .to_string(),
        )),
        Some(_) => {}
        None => dir_problems.push((
            "Worktree metadata could not be verified (git worktree prune -n failed).".to_string(),
            "Run: git worktree prune -n — to see the cause".to_string(),
        )),
    }
    checks.push(if dir_problems.is_empty() {
        doctor_pass(
            "state-dirs",
            format!(
                "State directory {} and cache directory {} exist and are writable; worktree metadata is consistent.",
                state.display(),
                cache.display()
            ),
        )
    } else {
        let detail = dir_problems
            .iter()
            .map(|(d, _)| d.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let hint = dir_problems
            .iter()
            .map(|(_, h)| h.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        doctor_fail("state-dirs", detail, hint)
    });

    // adapter-present: discovered exactly as the `check` runner discovers it.
    let adapter = doctor_adapter();
    checks.push(match &adapter {
        Some(p) if p.is_file() => doctor_pass(
            "adapter-present",
            format!("Check adapter found at {}.", p.display()),
        ),
        Some(p) => doctor_fail(
            "adapter-present",
            format!("Check adapter path {} does not exist.", p.display()),
            "Run: factory init --stack rust — to re-seed scripts/factory-check".to_string(),
        ),
        None => doctor_fail(
            "adapter-present",
            "No check adapter found — verification has nothing to run.".to_string(),
            "Run: factory init --stack rust — to seed scripts/factory-check (or set FACTORY_CHECK_ADAPTER)".to_string(),
        ),
    });

    // adapter-executable: the adapter must run.
    checks.push(match &adapter {
        Some(p) if p.is_file() && path_is_executable(p) => doctor_pass(
            "adapter-executable",
            format!("Check adapter {} is executable.", p.display()),
        ),
        Some(p) if p.is_file() => doctor_fail(
            "adapter-executable",
            format!("Check adapter {} is not executable.", p.display()),
            format!("Run: chmod +x {}", p.display()),
        ),
        _ => doctor_fail(
            "adapter-executable",
            "No check adapter to execute.".to_string(),
            "Run: factory init --stack rust — to seed scripts/factory-check".to_string(),
        ),
    });

    // toolchain: the rust adapter's toolchain must answer via PATH.
    let cargo = probe_stdout("cargo", &["--version"]);
    let rustc = probe_stdout("rustc", &["--version"]);
    checks.push(match (cargo, rustc) {
        (Some(c), Some(r)) => doctor_pass(
            "toolchain",
            format!("Rust toolchain answers ({c}; {r})."),
        ),
        (Some(c), None) => doctor_fail(
            "toolchain",
            format!("cargo answers ({c}) but rustc does not — toolchain incomplete."),
            "Install Rust via: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh — then re-run: factory doctor".to_string(),
        ),
        _ => doctor_fail(
            "toolchain",
            "Rust toolchain (cargo) not found on PATH — checks would fail after model spend."
                .to_string(),
            "Install Rust via: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh — then re-run: factory doctor".to_string(),
        ),
    });

    // model-runner: the configured runner binary must respond.
    let runner =
        probe_stdout(&pi_bin(), &["--version"]).or_else(|| probe_stdout(&pi_bin(), &["--help"]));
    checks.push(match runner {
        Some(v) => doctor_pass(
            "model-runner",
            format!(
                "Model runner answers ({}).",
                v.split('\n').next().unwrap_or(&pi_bin())
            ),
        ),
        None => doctor_fail(
            "model-runner",
            format!(
                "Model runner `{}` does not respond — implement/review steps cannot run.",
                pi_bin()
            ),
            "Install the runner on PATH as `pi` (or point FACTORY_PI_BIN at it) — then re-run: factory doctor".to_string(),
        ),
    });

    debug_assert_eq!(
        checks.iter().map(|c| c.name).collect::<Vec<_>>(),
        DOCTOR_ORDER,
        "doctor catalogue drifted from DOCTOR_ORDER"
    );
    checks
}

/// Exit 0 iff every check passes, else 1 — the green/red answer.
/// With `fix`, safe local repairs apply first and both the human report
/// and JSON list them under `fixed:` / `fixed`.
pub fn cmd_doctor(json: bool, fix: bool) -> i32 {
    let fixed = if fix { doctor_fix() } else { Vec::new() };
    let checks = doctor_checks();
    let ok = checks.iter().all(|c| c.ok);
    if json {
        println!(
            "{}",
            serde_json::to_string(&DoctorOutput { ok, checks, fixed })
                .expect("doctor JSON serialization")
        );
    } else {
        if fix {
            println!("== factory doctor --fix ==");
            if fixed.is_empty() {
                if ok {
                    println!("fix: nothing to repair — already green");
                } else {
                    println!("fix: nothing safe to repair automatically");
                }
            } else {
                for f in &fixed {
                    println!("fixed: {f}");
                }
            }
        } else {
            println!("== factory doctor ==");
        }
        for c in &checks {
            if c.ok {
                println!("[ok] {}: {}", c.name, c.detail);
            } else {
                println!("[FAIL] {}: {}", c.name, c.detail);
                println!("       fix: {}", c.hint);
            }
        }
        if ok {
            println!("doctor: ready — all {} checks pass", checks.len());
        } else {
            let failing: Vec<&str> = checks.iter().filter(|c| !c.ok).map(|c| c.name).collect();
            println!(
                "doctor: blocked — {} of {} checks failing ({}) — apply the fix commands above, then re-run: factory doctor",
                failing.len(),
                checks.len(),
                failing.join(", ")
            );
        }
    }
    i32::from(!ok)
}

/// Valid handoff verdicts (parity with the bash `factory handoff --signal`).
const SIGNALS: &[&str] = &["READY_FOR_REVIEW", "NEEDS_CLARIFICATION", "BLOCKED"];

/// `gh` binary seam: shell out only, never an API client.
/// Overridable via FACTORY_GH_BIN for hermetic tests.
fn gh_bin() -> String {
    std::env::var("FACTORY_GH_BIN").unwrap_or_else(|_| "gh".to_string())
}

/// `git` binary seam (overridable via FACTORY_GIT_BIN for tests).
fn git_bin() -> String {
    std::env::var("FACTORY_GIT_BIN").unwrap_or_else(|_| "git".to_string())
}

fn run_gh_output(args: &[&str]) -> Result<std::process::Output, String> {
    let mut out = Command::new(gh_bin())
        .args(args)
        .output()
        .map_err(|e| format!("failed to run gh {}: {e}", args.join(" ")))?;
    // Version-manager shims (mise) print status lines to stdout, which
    // corrupts `--json` parsing in every consumer below. Strip them once,
    // here, so all callers see clean JSON (mirrors the bash dispatcher's
    // gh() wrapper). Real errors stay on stderr, untouched.
    out.stdout = strip_gh_noise(&out.stdout);
    Ok(out)
}

/// Drop version-manager status lines from gh stdout bytes.
fn strip_gh_noise(raw: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(raw)
        .lines()
        .filter(|l| !l.starts_with("mise "))
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

fn run_git(args: &[&str], cwd: Option<&std::path::Path>) -> Result<std::process::Output, String> {
    let mut cmd = Command::new(git_bin());
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.output()
        .map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))
}

fn validate_signal(signal: &str) -> Result<(), String> {
    if SIGNALS.contains(&signal) {
        Ok(())
    } else {
        Err(format!(
            "invalid --signal {signal:?}: expected one of {}",
            SIGNALS.join("|")
        ))
    }
}

pub fn cmd_handoff(issue: &str, signal: &str, body_file: Option<&PathBuf>, json: bool) -> i32 {
    if let Err(e) = validate_signal(signal) {
        eprintln!("factory handoff: {e}");
        return 2;
    }
    let report = match body_file {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!(
                    "factory handoff: cannot read --body {}: {e}",
                    path.display()
                );
                return 1;
            }
        },
        None => String::new(),
    };
    let body = if report.is_empty() {
        format!("HANDOFF `FACTORY:{signal}`")
    } else {
        format!("{report}\n\nHANDOFF `FACTORY:{signal}`")
    };
    match run_gh_output(&["issue", "comment", issue, "--body", &body]) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            eprintln!(
                "factory handoff: gh issue comment {issue} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return 1;
        }
        Err(e) => {
            eprintln!("factory handoff: {e}");
            return 1;
        }
    }
    if json {
        let value = serde_json::json!({"issue": issue, "signal": format!("FACTORY:{signal}")});
        println!("{value}");
    } else {
        println!();
        println!("FACTORY:{signal}");
    }
    0
}

#[derive(Serialize)]
struct ReconcileIssue {
    number: u64,
    title: String,
    status: String,
    hits: Vec<String>,
}

#[derive(Serialize)]
struct ReconcileOutput {
    base: String,
    issues: Vec<ReconcileIssue>,
    branches: Vec<String>,
    /// Per-ticket outcome figures (ticket #58, spec #56): the identical
    /// figure set the human-readable metrics section renders.
    metrics: Vec<OutcomeFigures>,
}

/// Best-effort fetch (read-only); failures are ignored like the bash parity.
fn best_effort_fetch() {
    let _ = run_git(&["fetch", "origin", "dev", "--quiet"], None);
}

/// Pick the drift base: origin/dev, else origin/main, else HEAD.
fn drift_base() -> String {
    for candidate in ["origin/dev", "origin/main"] {
        if let Ok(out) = run_git(&["rev-parse", "--verify", "--quiet", candidate], None) {
            if out.status.success() {
                return candidate.to_string();
            }
        }
    }
    "HEAD".to_string()
}

pub fn cmd_reconcile(json: bool) -> i32 {
    best_effort_fetch();
    let base = drift_base();
    let list = match run_gh_output(&[
        "issue",
        "list",
        "--state",
        "open",
        "--limit",
        "50",
        "--json",
        "number,title",
    ]) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        Ok(out) => {
            eprintln!(
                "factory reconcile: gh issue list failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return 1;
        }
        Err(e) => {
            eprintln!("factory reconcile: {e}");
            return 1;
        }
    };
    let issues: Vec<serde_json::Value> = match serde_json::from_str(&list) {
        Ok(v) => v,
        Err(_) => {
            // Non-JSON passthrough: still read-only, still a report.
            print!("{list}");
            if !list.ends_with('\n') {
                println!();
            }
            return 0;
        }
    };
    let mut entries: Vec<ReconcileIssue> = Vec::new();
    for item in &issues {
        let number = item.get("number").and_then(|n| n.as_u64()).unwrap_or(0);
        let title = item
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let pattern = format!("#{number}");
        let hits: Vec<String> = run_git(&["log", &base, "--oneline", "--grep", &pattern], None)
            .ok()
            .filter(|out| out.status.success())
            .map(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .take(3)
                    .map(|l| l.to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let status = if hits.is_empty() {
            "open".to_string()
        } else {
            "possibly-landed".to_string()
        };
        entries.push(ReconcileIssue {
            number,
            title,
            status,
            hits,
        });
    }
    let branches: Vec<String> = run_git(&["branch", "-a"], None)
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().trim_start_matches('*').trim().to_string())
                .filter(|l| l.contains("factory/") || l.contains("feat/"))
                .take(20)
                .collect()
        })
        .unwrap_or_default();
    if json {
        let metrics: Vec<OutcomeFigures> = entries
            .iter()
            .map(|e| outcome_figures(&reconcile_history_for(e.number, &base)))
            .collect();
        let out = ReconcileOutput {
            base,
            issues: entries,
            branches,
            metrics,
        };
        println!(
            "{}",
            serde_json::to_string(&out).expect("reconcile JSON serialization")
        );
    } else {
        println!("== open issues vs {base} ==");
        for e in &entries {
            if e.hits.is_empty() {
                println!("#{} open: {}", e.number, e.title);
            } else {
                println!("#{} POSSIBLY-LANDED: {}", e.number, e.title);
                for h in &e.hits {
                    println!("    {h}");
                }
            }
        }
        println!();
        println!("== factory branches ==");
        if branches.is_empty() {
            println!("(none)");
        } else {
            for b in &branches {
                println!("{b}");
            }
        }
        println!();
        println!("== outcome metrics ==");
        for e in &entries {
            println!(
                "{}",
                outcome_human_line(&outcome_figures(&reconcile_history_for(e.number, &base)))
            );
        }
        println!();
        println!("Suggested actions (human decides): close landed issues, delete merged branches.");
    }
    0
}

// ---------------------------------------------------------------------------
// OUTCOME FIGURES (ticket #57, spec #56)
//
// One pure computation seam: assembled loop history in, per-ticket figures
// out. All fetching stays behind the read paths the reconcile view already
// uses; this section performs no network, filesystem, or tracker calls —
// it observes only its argument. Cycle time is issue creation (signal) to
// the landing merge; attempt minutes are per-attempt finish-minus-start.
// Tickets not yet landed yield blanks (None) for landed figures, never
// guesses; attempts with missing timestamps yield blanks for minutes.
// Cost is explicitly absent: `OUTCOME_COST_TEXT` states it plainly, never
// zero-filled, until the session-id join lands as separate instrumentation.
// ---------------------------------------------------------------------------

/// Caveat that ships in the output copy with every autonomy verdict:
/// manual work outside the loop is invisible to the rule.
/// Single source: the rule and its caveat change together or not at all.
pub const OUTCOME_AUTONOMY_CAVEAT: &str = "manual work outside the loop is invisible to the rule";

/// Plain statement carried where a cost figure would go. Spend tracking
/// needs the session-id join (separate, later instrumentation decision),
/// so cost is reported as not-yet-wired, never zero.
pub const OUTCOME_COST_TEXT: &str = "spend tracking not yet wired";

/// A timeline signal reduced to what the autonomy rule observes.
/// Assembled by the caller from issue timeline events; the computation
/// never fetches them itself.
#[derive(Debug, Clone, PartialEq)]
pub enum OutcomeSignal {
    Clarification,
    Blocked,
    Other,
}

/// One dispatch attempt row, assembled by the caller from the ledger.
/// `None` timestamps mean the log has rotated or was never written:
/// minutes read as blank, never interpolated.
#[derive(Debug, Clone, PartialEq)]
pub struct OutcomeAttemptInput {
    pub attempt_no: u64,
    pub started_epoch_secs: Option<i64>,
    pub finished_epoch_secs: Option<i64>,
    /// First-review accept for this attempt, sourced from the persisted
    /// logs and issue-posted reports (never ephemeral temp files).
    pub review_accepted: bool,
}

/// Assembled loop history for one ticket: attempt rows, timeline signals,
/// and merge dates. Plain data in, figures out — no I/O inside.
#[derive(Debug, Clone, PartialEq)]
pub struct OutcomeHistoryInput {
    pub ticket: u64,
    /// Issue creation: the signal end of the signal-to-production span.
    pub signal_epoch_secs: i64,
    /// Landing merge: `None` while the ticket has not landed yet.
    pub landed_epoch_secs: Option<i64>,
    pub attempts: Vec<OutcomeAttemptInput>,
    /// Timeline events reduced to rule-observed signals.
    pub signals: Vec<OutcomeSignal>,
}

/// Per-attempt machine minutes. `None` means the attempt's logs are
/// missing — a gap, not a zero.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutcomeAttemptMinutes {
    pub attempt_no: u64,
    pub minutes: Option<f64>,
}

/// Per-ticket outcome figures. `None` fields are blanks for unlanded or
/// unlogged history, never guesses.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutcomeFigures {
    pub ticket: u64,
    /// Signal-to-production cycle time in days. Blank until landed.
    pub cycle_days: Option<f64>,
    pub attempt_minutes: Vec<OutcomeAttemptMinutes>,
    /// Sum of per-attempt minutes. Blank when any attempt's logs are
    /// missing — a partial sum would read as a confident total.
    pub total_attempt_minutes: Option<f64>,
    pub autonomous: bool,
    pub autonomy_verdict: &'static str,
    pub autonomy_caveat: &'static str,
    pub cost_text: &'static str,
}

/// The single named autonomy predicate (spec #56): one attempt,
/// first-review accept, no clarification or blocked signals. Landing is
/// deliberately not part of the rule: unlanded tickets blank their landed
/// figures while the verdict still follows this predicate exactly.
/// Pure over its argument: no network, filesystem, or tracker calls.
pub fn is_autonomous(history: &OutcomeHistoryInput) -> bool {
    if history.attempts.len() != 1 {
        return false;
    }
    if !history.attempts[0].review_accepted {
        return false;
    }
    !history
        .signals
        .iter()
        .any(|s| matches!(s, OutcomeSignal::Clarification | OutcomeSignal::Blocked))
}

/// Turn assembled loop history into per-ticket outcome figures.
/// Pure over its argument: no network, filesystem, or tracker calls.
pub fn outcome_figures(history: &OutcomeHistoryInput) -> OutcomeFigures {
    let cycle_days = match history.landed_epoch_secs {
        Some(landed) if landed >= history.signal_epoch_secs => {
            Some((landed - history.signal_epoch_secs) as f64 / 86_400.0)
        }
        _ => None,
    };
    let attempt_minutes: Vec<OutcomeAttemptMinutes> = history
        .attempts
        .iter()
        .map(|a| {
            let minutes = match (a.started_epoch_secs, a.finished_epoch_secs) {
                (Some(started), Some(finished)) if finished >= started => {
                    Some((finished - started) as f64 / 60.0)
                }
                _ => None,
            };
            OutcomeAttemptMinutes {
                attempt_no: a.attempt_no,
                minutes,
            }
        })
        .collect();
    let total_attempt_minutes =
        if attempt_minutes.is_empty() || attempt_minutes.iter().any(|m| m.minutes.is_none()) {
            None
        } else {
            Some(
                attempt_minutes
                    .iter()
                    .map(|m| m.minutes.unwrap_or(0.0))
                    .sum(),
            )
        };
    let autonomous = is_autonomous(history);
    OutcomeFigures {
        ticket: history.ticket,
        cycle_days,
        attempt_minutes,
        total_attempt_minutes,
        autonomous,
        autonomy_verdict: if autonomous {
            "autonomous"
        } else {
            "not autonomous"
        },
        autonomy_caveat: OUTCOME_AUTONOMY_CAVEAT,
        cost_text: OUTCOME_COST_TEXT,
    }
}

/// One human-readable line carrying the figures, the verdict, its caveat,
/// and the cost statement together — the copy a future view renders.
/// Pure over its argument.
pub fn outcome_human_line(figures: &OutcomeFigures) -> String {
    let cycle = figures
        .cycle_days
        .map(|d| format!("{d:.1} days"))
        .unwrap_or_else(|| "not yet landed".to_string());
    let minutes = figures
        .total_attempt_minutes
        .map(|m| format!("{m:.1} minutes"))
        .unwrap_or_else(|| "logs missing".to_string());
    format!(
        "Ticket #{}: signal-to-production cycle {}, attempt machine minutes {}. Autonomy: {} ({}). Cost: {}.",
        figures.ticket, cycle, minutes, figures.autonomy_verdict, figures.autonomy_caveat,
        figures.cost_text,
    )
}

// ---------------------------------------------------------------------------
// RECONCILE METRICS SECTION (ticket #58, spec #56)
//
// The reconcile view extended with per-ticket outcome figures, computed
// through #57's pure `outcome_figures` seam. Assembly below is the only
// impure part: it reads the live stores behind the read paths reconcile
// already uses — the dispatch ledger (attempt rows + persisted
// transcripts for first-review accept), the issue timeline (`gh issue
// view`: signal date + clarification/blocked markers), and merge history
// (landing date via the drift grep seam). Every fetch failure degrades to
// a gap, never an error; cost stays not-yet-wired, never zero.
// ---------------------------------------------------------------------------

/// Days since the Unix epoch for a civil date (Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `+HH:MM` / `-HHMM` style zone suffix to seconds east of UTC.
fn parse_tz_offset(s: &str) -> Option<i64> {
    let sign = match s.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return None,
    };
    let digits: String = s[1..].chars().filter(|c| *c != ':').collect();
    if digits.len() != 2 && digits.len() != 4 {
        return None;
    }
    let hh: i64 = digits[..2].parse().ok()?;
    let mm: i64 = if digits.len() == 4 {
        digits[2..].parse().ok()?
    } else {
        0
    };
    if hh > 23 || mm > 59 {
        return None;
    }
    Some(sign * (hh * 3_600 + mm * 60))
}

/// Strict ISO-8601 timestamp (`YYYY-MM-DDTHH:MM:SS[.frac][Z|±offset]`)
/// to epoch seconds. A missing zone reads as a gap (`None`): assuming
/// UTC would print a confident span from an imprecise record.
fn parse_iso8601_epoch(s: &str) -> Option<i64> {
    let (date_part, time_part) = s.split_once('T').or_else(|| s.split_once('t'))?;
    let mut d = date_part.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let m: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    if d.next().is_some() {
        return None;
    }
    let (clock, offset_secs) = if let Some(t) = time_part
        .strip_suffix('Z')
        .or_else(|| time_part.strip_suffix('z'))
    {
        (t, 0)
    } else {
        let i = time_part.find(['+', '-'])?;
        let (c, z) = time_part.split_at(i);
        (c, parse_tz_offset(z)?)
    };
    let mut c = clock.split(':');
    let hh: i64 = c.next()?.parse().ok()?;
    let mm: i64 = c.next()?.parse().ok()?;
    let ss: i64 = c.next()?.split('.').next()?.parse().ok()?;
    if c.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86_400 + hh * 3_600 + mm * 60 + ss - offset_secs)
}

/// Epoch seconds from the shapes the loop's own stores emit: bare
/// integers (ledger `started_at`/`finished_at`) and the ISO-8601
/// `createdAt` shape `gh issue view` returns. Anything else is a gap
/// (`None`), never an error.
fn parse_epoch_secs(raw: &str) -> Option<i64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    parse_iso8601_epoch(s)
}

/// Reduce one `gh issue view --json number,createdAt,labels,comments`
/// document to the signals the autonomy rule observes. Labels reuse the
/// board's existing vocabulary (`blocked`, `needs-info`/`needs-you`);
/// comments match the uppercase signal tokens the loop itself posts to the
/// timeline (`NEEDS_CLARIFICATION`, `BLOCKED`, as in the `FACTORY:`
/// handoff markers). Anything unparseable yields no signals — a gap.
fn outcome_signals_from_view(view: &serde_json::Value) -> Vec<OutcomeSignal> {
    let mut clarification = false;
    let mut blocked = false;
    if let Some(labels) = view.get("labels").and_then(|v| v.as_array()) {
        for label in labels {
            let name = if let Some(s) = label.as_str() {
                s.to_string()
            } else {
                label
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            match name.to_lowercase().as_str() {
                "blocked" => blocked = true,
                "needs-info" | "needs-you" | "clarification" => clarification = true,
                _ => {}
            }
        }
    }
    if let Some(comments) = view.get("comments").and_then(|v| v.as_array()) {
        for comment in comments {
            let body = comment.get("body").and_then(|b| b.as_str()).unwrap_or("");
            if body.contains("NEEDS_CLARIFICATION") {
                clarification = true;
            }
            if body.contains("BLOCKED") {
                blocked = true;
            }
        }
    }
    let mut out = Vec::new();
    if clarification {
        out.push(OutcomeSignal::Clarification);
    }
    if blocked {
        out.push(OutcomeSignal::Blocked);
    }
    out
}

/// Newest commit timestamp on `base` mentioning `#<ticket>` (the same grep
/// seam the drift check uses). No match — or any git failure — reads as
/// unlanded (`None`), never an error.
fn reconcile_landed_epoch(ticket: u64, base: &str) -> Option<i64> {
    let pattern = format!("#{ticket}");
    let out = run_git(&["log", base, "--format=%ct", "--grep", &pattern], None).ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .parse::<i64>()
        .ok()
}

/// Assemble one ticket's loop history from the live stores. First-review
/// accept comes from the persisted transcripts (ledger log plus its
/// sibling review log — never ephemeral temp files), matching the
/// codebase's existing `VERDICT:` read paths. A missing signal date
/// blanks the landing side too: without the signal end, no span is honest.
fn reconcile_history_for(ticket: u64, base: &str) -> OutcomeHistoryInput {
    let state = dispatch_state_dir();
    let ledger = load_ledger(&state);
    let view: Option<serde_json::Value> = run_gh_output(&[
        "issue",
        "view",
        &ticket.to_string(),
        "--json",
        "number,createdAt,labels,comments",
    ])
    .ok()
    .filter(|out| out.status.success())
    .and_then(|out| serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).ok());
    let signal = view
        .as_ref()
        .and_then(|v| v.get("createdAt"))
        .and_then(|c| c.as_str())
        .and_then(parse_epoch_secs);
    let signals = view
        .as_ref()
        .map(outcome_signals_from_view)
        .unwrap_or_default();
    let mut attempts: Vec<OutcomeAttemptInput> = ledger
        .attempts
        .iter()
        .filter(|a| a.ticket == ticket)
        .map(|a| OutcomeAttemptInput {
            attempt_no: a.attempt_no,
            started_epoch_secs: parse_epoch_secs(&a.started_at),
            finished_epoch_secs: parse_epoch_secs(&a.finished_at),
            review_accepted: parse_verdict(&log_read_transcript(&state, a)).as_deref()
                == Some("accept"),
        })
        .collect();
    attempts.sort_by_key(|a| a.attempt_no);
    let landed = match signal {
        Some(_) => reconcile_landed_epoch(ticket, base),
        None => None,
    };
    OutcomeHistoryInput {
        ticket,
        signal_epoch_secs: signal.unwrap_or(0),
        landed_epoch_secs: landed,
        attempts,
        signals,
    }
}

/// Slugify a ticket title into a branch-safe slug (parity with bash `start`).
fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true; // collapse runs + trim leading dashes
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug.chars()
        .take(30)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[derive(Serialize)]
struct StartOutput {
    ticket: u64,
    branch: String,
    worktree: String,
    brief: String,
    base: String,
}

pub fn cmd_start(ticket: &str, worktree_override: Option<&PathBuf>, json: bool) -> i32 {
    let view = match run_gh_output(&["issue", "view", ticket, "--json", "number,title,body"]) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        Ok(out) => {
            eprintln!(
                "factory start: gh issue view {ticket} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return 1;
        }
        Err(e) => {
            eprintln!("factory start: {e}");
            return 1;
        }
    };
    let info: serde_json::Value = match serde_json::from_str(&view) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("factory start: cannot parse gh issue view output: {e}");
            return 1;
        }
    };
    let number = info.get("number").and_then(|n| n.as_u64()).unwrap_or(0);
    let title = info.get("title").and_then(|t| t.as_str()).unwrap_or("");
    let body = info.get("body").and_then(|b| b.as_str()).unwrap_or("");
    let slug = slugify(title);
    let slug = if slug.is_empty() {
        format!("ticket-{number}")
    } else {
        slug
    };
    let branch = format!("factory/T{number}-{slug}");
    let base = run_git(&["rev-parse", "origin/dev"], None)
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            run_git(&["rev-parse", "HEAD"], None)
                .ok()
                .filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "HEAD".to_string());
    let worktree = match worktree_override {
        Some(dir) => dir.clone(),
        None => {
            let repo = std::env::current_dir()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "repo".to_string());
            PathBuf::from(format!("../{repo}-T{number}"))
        }
    };
    if let Some(parent) = worktree.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "factory start: cannot create worktree parent {}: {e}",
                    parent.display()
                );
                return 1;
            }
        }
    }
    match run_git(
        &[
            "worktree",
            "add",
            &worktree.to_string_lossy(),
            "-b",
            &branch,
            &base,
        ],
        None,
    ) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            eprintln!(
                "factory start: git worktree add failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return 1;
        }
        Err(e) => {
            eprintln!("factory start: {e}");
            return 1;
        }
    }
    let brief_path = worktree.join(".factory-brief.md");
    let brief = format!("Ticket issue: #{number}\nBranch: {branch} (base {base})\n\n{body}\n");
    if let Err(e) = std::fs::write(&brief_path, &brief) {
        eprintln!("factory start: cannot write {}: {e}", brief_path.display());
        return 1;
    }
    // Parity with bash: when the build prompt exists, compose .factory-prompt.md.
    let build_prompt = PathBuf::from(".pi/prompts/build.md");
    if build_prompt.is_file() {
        if let Ok(prefix) = std::fs::read_to_string(&build_prompt) {
            let _ = std::fs::write(
                worktree.join(".factory-prompt.md"),
                format!("{prefix}{brief}"),
            );
        }
    }
    if json {
        let out = StartOutput {
            ticket: number,
            branch: branch.clone(),
            worktree: worktree.to_string_lossy().into_owned(),
            brief: brief_path.to_string_lossy().into_owned(),
            base: base.clone(),
        };
        println!(
            "{}",
            serde_json::to_string(&out).expect("start JSON serialization")
        );
    } else {
        println!(
            "worktree: {} | branch: {branch} | brief: {}",
            worktree.to_string_lossy(),
            brief_path.to_string_lossy()
        );
    }
    0
}

// ---------------------------------------------------------------------------
// DISPATCH LOOP (ticket #13)
//
// SAFETY CONTRACT (asserted in tests/cli_dispatch.rs, not just documented):
// The dispatcher NEVER lands and NEVER touches dev/main. There is
// deliberately NO code path in this section that pushes, merges into,
// checks out-and-commits-to, or branch-forces dev or main. Worktrees are
// cut from HEAD (read-only rev-parse) and removed afterwards; review runs
// in a second detached worktree. The human owns the train→dev review and
// the land. `git fetch` / `git push` do not appear here at all.
// ---------------------------------------------------------------------------

#[derive(Serialize, serde::Deserialize, Clone, Debug)]
struct DispatchAttempt {
    id: u64,
    ticket: u64,
    attempt_no: u64,
    base_commit: String,
    #[serde(default)]
    head_commit: String,
    branch: String,
    worktree: String,
    status: String,
    #[serde(default)]
    exit_code: i32,
    log_path: String,
    started_at: String,
    finished_at: String,
    #[serde(default)]
    train_branch: String,
    /// Checkpointed phase (ticket #60, spec #59): plan | implement |
    /// verify | review. `#[serde(default)]` keeps pre-checkpoint ledgers
    /// parsing; missing/empty reads as phase-unknown under the old rules.
    #[serde(default)]
    phase: String,
}

#[derive(Serialize, serde::Deserialize, Default)]
struct DispatchLedger {
    #[serde(default)]
    attempts: Vec<DispatchAttempt>,
    /// Answer delivery keys (`<ticket>:<id>:<hash>`) already handed to a
    /// brief (ticket #44). `#[serde(default)]` keeps pre-answer ledgers
    /// parsing; single-use depends on this surviving restarts.
    #[serde(default)]
    delivered_answers: Vec<String>,
    /// Exactly-once admission keys (ticket #62, spec #59): client-generated
    /// keys bound to the operation fingerprint of the pass that first
    /// carried them. `#[serde(default)]` keeps pre-key ledgers parsing;
    /// records older than the retention horizon read as absent.
    #[serde(default)]
    idempotency_keys: Vec<IdempotencyRecord>,
}

/// One stored exactly-once outcome (ticket #62, spec #59): the key, the
/// operation fingerprint it is bound to, and the admitted attempt facts a
/// repeat replays with a replay marker instead of double-admitting.
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
struct IdempotencyRecord {
    key: String,
    operation: String,
    ticket: u64,
    attempt_no: u64,
    status: String,
    stored_at: String,
}

/// Implementer/reviewer seam: shell out only, never an API client.
/// Overridable via FACTORY_PI_BIN for hermetic tests.
fn pi_bin() -> String {
    std::env::var("FACTORY_PI_BIN").unwrap_or_else(|_| "pi".to_string())
}

/// State root: FACTORY_STATE_DIR, else <cwd>/factory/.state.
/// Read-only state-dir handle (spec #27: handlers read through the
/// ledger module, logs dir, `gh` seam, and git runner — never writable).
pub fn dispatch_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("FACTORY_STATE_DIR") {
        return PathBuf::from(dir);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("factory")
        .join(".state")
}

fn dispatch_max_concurrent() -> usize {
    std::env::var("MAX_CONCURRENT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1)
}

fn dispatch_max_attempts() -> u64 {
    std::env::var("MAX_ATTEMPTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
        .max(1)
}

fn now_stamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// `gh` via the FACTORY_GH_BIN seam, with mise stdout noise filtered.
fn dgh(args: &[&str]) -> Result<String, String> {
    let out = run_gh_output(args)?;
    if !out.status.success() {
        return Err(format!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout
        .lines()
        .filter(|l| !l.starts_with("mise "))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn ledger_path(state: &std::path::Path) -> PathBuf {
    state.join("dispatch.json")
}

fn load_ledger(state: &std::path::Path) -> DispatchLedger {
    std::fs::read_to_string(ledger_path(state))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Atomic ledger write (temp file + rename) so concurrent workers never
/// observe a torn ledger; each attempt also owns its worktree, log, slot,
/// and TMPDIR, so no mutable state is ever shared.
fn save_ledger(state: &std::path::Path, ledger: &DispatchLedger) -> Result<(), String> {
    std::fs::create_dir_all(state)
        .map_err(|e| format!("cannot create {}: {e}", state.display()))?;
    let tmp = state.join("dispatch.json.tmp");
    let text = serde_json::to_string_pretty(ledger)
        .map_err(|e| format!("cannot serialize ledger: {e}"))?;
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write ledger: {e}"))?;
    std::fs::rename(&tmp, ledger_path(state)).map_err(|e| format!("cannot replace ledger: {e}"))?;
    Ok(())
}

// --- replayable attempts, slice 1: phase checkpoints (ticket #60, spec #59)
//
// Each attempt records its phase as it moves (plan, implement, verify,
// review); the checkpoint is committed BEFORE side-effecting work begins,
// never after. A kill mid-tool surfaces as interrupted (resumable) —
// visibly distinct from failed (a negative tool, test, or review verdict,
// which keeps its existing *_failed status byte-for-byte).
//
// Atomicity: the checkpoint rides the same temp-file + rename as every
// ledger write, so a crash during the checkpoint itself leaves either the
// old or the new ledger on disk, never a torn one. The
// FACTORY_CHECKPOINT_CRASH seam below exists only to pin that guarantee
// in tests: it writes tmp and exits before the rename, exactly like a
// kill -9 at that instant.

/// Attempt phases in run order. The checkpoint is committed before each
/// phase's side-effecting work begins.
const ATTEMPT_PHASES: &[&str] = &["plan", "implement", "verify", "review"];

/// Phase on the record, or "unknown" for ledgers written before
/// checkpoints existed (no migration: old rows behave under the old rules).
fn attempt_phase(a: &DispatchAttempt) -> &str {
    if a.phase.is_empty() {
        "unknown"
    } else {
        a.phase.as_str()
    }
}

/// `true` for `{phase}_interrupted`: killed, expired, or stopped mid-tool
/// (resumable). Negative verdicts stay `{stage}_failed` (needs a fix).
fn is_interrupted_status(status: &str) -> bool {
    status.ends_with("_interrupted")
}

/// Resumable machines read: interrupted attempts may run again without
/// consuming the attempt budget. Shared by human and JSON modes.
fn is_resumable(a: &DispatchAttempt) -> bool {
    is_interrupted_status(&a.status)
}

/// Commit the phase checkpoint before side-effecting work begins. Atomic
/// (tmp + rename): a crash between the two leaves the previous ledger.
fn checkpoint_phase(
    state: &std::path::Path,
    ledger: &mut DispatchLedger,
    idx: usize,
    phase: &str,
) -> Result<(), String> {
    debug_assert!(ATTEMPT_PHASES.contains(&phase), "unknown phase {phase}");
    ledger.attempts[idx].phase = phase.to_string();
    std::fs::create_dir_all(state)
        .map_err(|e| format!("cannot create {}: {e}", state.display()))?;
    let tmp = state.join("dispatch.json.tmp");
    let text = serde_json::to_string_pretty(&*ledger)
        .map_err(|e| format!("cannot serialize ledger: {e}"))?;
    std::fs::write(&tmp, &text).map_err(|e| format!("cannot write ledger: {e}"))?;
    // Test-only kill seam: behave exactly like a crash after the tmp
    // write but before the rename (orphaned tmp, previous ledger intact).
    if std::env::var("FACTORY_CHECKPOINT_CRASH").as_deref() == Ok(phase) {
        std::process::exit(1);
    }
    std::fs::rename(&tmp, ledger_path(state)).map_err(|e| format!("cannot replace ledger: {e}"))?;
    Ok(())
}

/// Exactly-once key retention horizon (ticket #62, spec #59): a repeat
/// inside this window replays the stored outcome; older records read as
/// absent so the same key admits anew. Stated in `dispatch once --help`.
const IDEMPOTENCY_KEY_RETENTION_SECS: u64 = 24 * 3600;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `true` while the record still guards its key: stored within the
/// retention horizon. Older records read as absent (admit anew).
fn idempotency_live(r: &IdempotencyRecord) -> bool {
    r.stored_at
        .parse::<u64>()
        .map(|t| now_secs().saturating_sub(t) <= IDEMPOTENCY_KEY_RETENTION_SECS)
        .unwrap_or(false)
}

/// Operation fingerprint a key binds to on first use: the sorted open
/// ticket numbers this pass surveys. The same survey is the same
/// operation (safe to replay); a different survey is a different
/// operation (reusing the key is rejected).
fn idempotency_operation(tickets: &[u64]) -> String {
    let mut sorted = tickets.to_vec();
    sorted.sort_unstable();
    format!(
        "dispatch-once:{}",
        sorted
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Test-only kill seam: when FACTORY_KILL_PHASE names this phase, behave
/// exactly like a kill mid-tool — the checkpointed phase is already on
/// disk, the attempt reads interrupted (resumable), never failed.
fn simulated_kill_phase(phase: &str) -> bool {
    std::env::var("FACTORY_KILL_PHASE").as_deref() == Ok(phase)
}

/// Atomic mkdir lock so two concurrent `dispatch once` processes serialize
/// instead of double-scheduling the same ticket.
struct DispatchLock {
    path: PathBuf,
}

impl DispatchLock {
    fn acquire(path: PathBuf) -> Option<Self> {
        std::fs::create_dir(&path).ok()?;
        Some(Self { path })
    }
}

impl Drop for DispatchLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
    }
}

fn dispatch_active(ledger: &DispatchLedger) -> usize {
    ledger
        .attempts
        .iter()
        .filter(|a| a.status == "impl_running" || a.status == "review_running")
        .count()
}

fn dgh_labels(view: &serde_json::Value) -> Vec<String> {
    view.get("labels")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    if let Some(s) = l.as_str() {
                        Some(s.to_string())
                    } else {
                        l.get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `Depends on: #9, #10` references in a ticket body.
fn depends_on(body: &str) -> Vec<u64> {
    let mut deps = Vec::new();
    let low = body.to_lowercase();
    let mut search = low.as_str();
    while let Some(idx) = search.find("depends on:") {
        search = &search[idx + "depends on:".len()..];
        let line = search.lines().next().unwrap_or("");
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '#' {
                let mut num = String::new();
                while let Some(d) = chars.peek() {
                    if d.is_ascii_digit() {
                        num.push(*d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if let Ok(n) = num.parse::<u64>() {
                    deps.push(n);
                }
            }
        }
        search = &search[line.len().min(search.len())..];
    }
    deps
}

/// `Spec issue: #N` / `Spec: #N` reference in a ticket body (#none → None).
fn linked_spec(body: &str) -> Option<u64> {
    let low = body.to_lowercase();
    let mut idx = 0;
    while idx < low.len() {
        let rest = &low[idx..];
        let found = rest
            .find("spec issue:")
            .map(|i| (i, "spec issue:".len()))
            .or_else(|| rest.find("spec:").map(|i| (i, "spec:".len())));
        let Some((i, len)) = found else {
            break;
        };
        let segment = rest[i + len..].lines().next().unwrap_or("");
        if let Some(h) = segment.find('#') {
            let num: String = segment[h + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = num.parse::<u64>() {
                return Some(n);
            }
        }
        idx += i + len;
    }
    None
}

/// Inline freeze gate for a ticket's linked spec: body hash must match the
/// latest SPEC-APPROVED comment hash (same canonical hash as `freeze`).
fn dispatch_verify_spec(spec: u64) -> Result<(), String> {
    let text = dgh(&[
        "issue",
        "view",
        &spec.to_string(),
        "--json",
        "number,title,body,state,comments",
    ])?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse gh issue view {spec}: {e}"))?;
    let body = v.get("body").and_then(|b| b.as_str()).unwrap_or("");
    let hash = body_hash(body);
    let mut approved: Option<String> = None;
    if let Some(comments) = v.get("comments").and_then(|c| c.as_array()) {
        for c in comments {
            if let Some(b) = c.get("body").and_then(|b| b.as_str()) {
                if let Some(h) = extract_approved_hash(b) {
                    approved = Some(h);
                }
            }
        }
    }
    if approved.as_deref() == Some(hash.as_str()) {
        Ok(())
    } else {
        Err(format!(
            "freeze MISMATCH: #{spec} body={hash} approved={} — re-freeze required",
            approved.as_deref().unwrap_or("none")
        ))
    }
}

fn gh_comment(ticket: u64, body: &str) {
    if let Err(e) = dgh(&["issue", "comment", &ticket.to_string(), "--body", body]) {
        eprintln!("dispatch: gh comment #{ticket} failed: {e}");
    }
}

fn gh_relabel(ticket: u64, remove: Option<&str>, add: &str) {
    let t = ticket.to_string();
    let mut owned = vec!["issue".to_string(), "edit".to_string(), t];
    if let Some(r) = remove {
        owned.push("--remove-label".to_string());
        owned.push(r.to_string());
    }
    owned.push("--add-label".to_string());
    owned.push(add.to_string());
    let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    if let Err(e) = dgh(&refs) {
        eprintln!("dispatch: gh edit #{ticket} failed: {e}");
    }
}

fn append_log(log_path: &std::path::Path, text: &str) {
    let mut existing = std::fs::read_to_string(log_path).unwrap_or_default();
    existing.push_str(text);
    let _ = std::fs::write(log_path, existing);
}

fn log_event(log_path: &std::path::Path, attempt_id: u64, ticket: u64, phase: &str, code: i32) {
    append_log(
        log_path,
        &format!(
            "{}\n",
            serde_json::json!({"attempt": attempt_id, "ticket": ticket, "phase": phase, "event": "done", "code": code, "ts": now_stamp()})
        ),
    );
}

/// Run one phase (implement / verify / review) with slot + private TMPDIR
/// isolation, appending all output to the attempt log.
#[allow(clippy::too_many_arguments)]
fn run_phase(
    program: &str,
    args: &[&str],
    cwd: &std::path::Path,
    slot: &str,
    tmpdir: &std::path::Path,
    log: &std::path::Path,
    phase: &str,
    attempt_id: u64,
    ticket: u64,
) -> (bool, String) {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env("FACTORY_SLOT", slot)
        .env("TMPDIR", tmpdir);
    match cmd.output() {
        Ok(o) => {
            let code = o.status.code().unwrap_or(1);
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            append_log(
                log,
                &format!("--- {phase} {slot} (exit {code}) ---\n{combined}\n"),
            );
            log_event(log, attempt_id, ticket, phase, code);
            (o.status.success(), combined)
        }
        Err(e) => {
            append_log(
                log,
                &format!("--- {phase} failed to spawn {program}: {e} ---\n"),
            );
            log_event(log, attempt_id, ticket, phase, 1);
            (false, String::new())
        }
    }
}

fn git_head(cwd: &std::path::Path) -> String {
    run_git(&["rev-parse", "HEAD"], Some(cwd))
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn remove_worktree(path: &std::path::Path) {
    let s = path.to_string_lossy().to_string();
    let removed = run_git(&["worktree", "remove", "--force", &s], None)
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !removed {
        let _ = std::fs::remove_dir_all(path);
    }
    let _ = run_git(&["worktree", "prune"], None);
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Trailing `VERDICT: accept|changes_requested` wins.
fn parse_verdict(text: &str) -> Option<String> {
    let mut verdict = None;
    for line in text.lines() {
        if line.contains("VERDICT:") {
            if line.contains("changes_requested") {
                verdict = Some("changes_requested".to_string());
            } else if line.contains("accept") {
                verdict = Some("accept".to_string());
            }
        }
    }
    verdict
}

enum TicketOutcome {
    Done,
    Skipped,
}

// --- answer delivery (ticket #44, spec #41) -------------------------------
//
// Proof that a Bea reply reaches running work. The loop reads Bea's
// answers HERE (see the read site in `cmd_dispatch_ticket`, just above the
// brief composition): structured `Answer[id]` lines in the ticket's issue
// comments, snapshotted once per attempt and appended to the brief.
//
// Locked routing rule (spec #41):
// - Structured `Answer[id]` (legacy shapes tolerated, broken markers
//   refused — see `parse_answer_line`).
// - Atomic boundary: the snapshot happens at brief-composition time. An
//   answer Bea posts mid-attempt structurally waits for the next attempt
//   — the running worker never sees it.
// - Single-use: delivery keys persist in the ledger BEFORE implement runs,
//   so a crash or retry never redelivers an answer.
// The same words travel bash-side inside the brief (`ANSWER_ROUTING_RULE`),
// because the worker is a shell-out subprocess that reads only the brief.
// If the tooling ever needs a format change, that is an upstream issue —
// never a local invention.

/// One parsed Bea reply: the question `id` it answers, her `body` words,
/// and whether it arrived in a `legacy` (pre-structured) shape.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedAnswer {
    pub id: String,
    pub body: String,
    pub legacy: bool,
}

/// What one comment line is: a deliverable answer, a broken marker with
/// the reason it was refused, or chatter the loop must ignore.
#[derive(Debug, Clone, PartialEq)]
pub enum AnswerLine {
    Valid(ParsedAnswer),
    Malformed(String),
    NotAnAnswer,
}

fn answer_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parse one comment line. Structured `Answer[id]: body` (exact-case
/// marker) is valid; a case-folded marker or the bracket-less
/// `Answer id: body` shape is valid but flagged legacy; a line that
/// reaches for the marker and misses (empty/bad id, missing colon, empty
/// reply) is malformed with the reason; everything else is chatter. Pure.
pub fn parse_answer_line(line: &str) -> AnswerLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return AnswerLine::NotAnAnswer;
    }
    let lower = trimmed.to_ascii_lowercase();
    let Some(after) = lower.strip_prefix("answer") else {
        return AnswerLine::NotAnAnswer;
    };
    // Byte offset of the marker end; the marker is ASCII so lower and
    // trimmed share indices. Keep the original case for id/body.
    let rest = &trimmed["answer".len()..];
    let marker_exact = trimmed.starts_with("Answer");
    match after.chars().next() {
        // `Answer[id]: body` (or case-folded marker → legacy).
        Some('[') => {
            let legacy = !marker_exact;
            let Some(end) = rest.find(']') else {
                return AnswerLine::Malformed(format!(
                    "malformed Answer line {line:?}: missing `]`"
                ));
            };
            let id = rest[1..end].trim();
            if id.is_empty() {
                return AnswerLine::Malformed(format!("malformed Answer line {line:?}: empty id"));
            }
            if !answer_id_ok(id) {
                return AnswerLine::Malformed(format!(
                    "malformed Answer line {line:?}: id {id:?} must be [A-Za-z0-9_-]+"
                ));
            }
            let after_bracket = rest[end + 1..].trim_start();
            let Some(body) = after_bracket.strip_prefix(':') else {
                return AnswerLine::Malformed(format!(
                    "malformed Answer line {line:?}: missing `:` after id"
                ));
            };
            let body = body.trim();
            if body.is_empty() {
                return AnswerLine::Malformed(format!(
                    "malformed Answer line {line:?}: empty reply"
                ));
            }
            AnswerLine::Valid(ParsedAnswer {
                id: id.to_string(),
                body: body.to_string(),
                legacy,
            })
        }
        // Bare `Answer:` / `answer` with nothing after: a missed marker.
        None | Some(':') => {
            AnswerLine::Malformed(format!("malformed Answer line {line:?}: missing id"))
        }
        // Bracket-less legacy `Answer id: body` (any marker case).
        Some(c) if c.is_whitespace() => {
            let tail = rest.trim_start();
            let token_end = tail.find(|c: char| c.is_whitespace()).unwrap_or(tail.len());
            let token = &tail[..token_end];
            let Some(id) = token.strip_suffix(':') else {
                return AnswerLine::Malformed(format!(
                    "malformed Answer line {line:?}: missing `:` after id"
                ));
            };
            if !answer_id_ok(id) {
                return AnswerLine::Malformed(format!(
                    "malformed Answer line {line:?}: id {id:?} must be [A-Za-z0-9_-]+"
                ));
            }
            let body = tail[token_end..].trim();
            if body.is_empty() {
                return AnswerLine::Malformed(format!(
                    "malformed Answer line {line:?}: empty reply"
                ));
            }
            AnswerLine::Valid(ParsedAnswer {
                id: id.to_string(),
                body: body.to_string(),
                legacy: true,
            })
        }
        // `Answers from Bea`, `answering ...`: chatter, not a marker.
        _ => AnswerLine::NotAnAnswer,
    }
}

/// Scan every line of every comment in order. First occurrence per id
/// wins (one answer per question per snapshot); broken markers accumulate
/// reasons and deliver nothing. Pure.
pub fn collect_answers(comments: &[String]) -> (Vec<ParsedAnswer>, Vec<String>) {
    let mut valid = Vec::new();
    let mut malformed = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for comment in comments {
        for line in comment.lines() {
            match parse_answer_line(line) {
                AnswerLine::Valid(answer) => {
                    if seen.insert(answer.id.clone()) {
                        valid.push(answer);
                    }
                }
                AnswerLine::Malformed(reason) => malformed.push(reason),
                AnswerLine::NotAnAnswer => {}
            }
        }
    }
    (valid, malformed)
}

/// Delivery key for single-use: same id re-answered in new words is a new
/// key (Bea corrected herself); anything else already handed out waits out.
/// Pure.
pub fn answer_delivery_key(ticket: u64, answer: &ParsedAnswer) -> String {
    let hash = body_hash(&answer.body);
    format!(
        "{ticket}:{}:{}",
        answer.id,
        hash.chars().take(16).collect::<String>()
    )
}

/// Answers not yet handed to any brief for this ticket. Pure.
pub fn pending_answers(
    all: &[ParsedAnswer],
    delivered: &[String],
    ticket: u64,
) -> Vec<ParsedAnswer> {
    all.iter()
        .filter(|a| !delivered.contains(&answer_delivery_key(ticket, a)))
        .cloned()
        .collect()
}

/// The bash-side pin of the routing rule: injected into every brief that
/// carries answers, because the worker is a shell-out subprocess that
/// reads only the brief. If the tooling ever needs a format change, file
/// an upstream issue — never invent a new format here.
pub const ANSWER_ROUTING_RULE: &str = "Routing rule (pinned): Bea's replies arrive as structured `Answer[id]` issue comments. The loop snapshots them when composing this brief — anything posted mid-attempt waits for the next attempt (atomic boundary). Each answer is delivered once (single-use): consume it from this brief and never ask for it again. If this rule cannot be followed with the current tooling, stop and report instead of inventing a new format.";

/// The brief section a worker reads: her answers verbatim, then the
/// pinned rule. Pure over inputs so transcripts, stored briefs, and tests
/// agree byte-for-byte.
pub fn render_answers_section(answers: &[ParsedAnswer]) -> String {
    let mut out = String::from("## Answers from Bea\n\n");
    for answer in answers {
        if answer.legacy {
            out.push_str(&format!(
                "- {}: {} (legacy format)\n",
                answer.id, answer.body
            ));
        } else {
            out.push_str(&format!("- {}: {}\n", answer.id, answer.body));
        }
    }
    out.push('\n');
    out.push_str(ANSWER_ROUTING_RULE);
    out.push('\n');
    out
}

fn finish_attempt(
    state: &std::path::Path,
    ledger: &mut DispatchLedger,
    idx: usize,
    status: &str,
    exit_code: i32,
) {
    ledger.attempts[idx].status = status.to_string();
    ledger.attempts[idx].exit_code = exit_code;
    ledger.attempts[idx].finished_at = now_stamp();
    if let Err(e) = save_ledger(state, ledger) {
        eprintln!("dispatch: {e}");
    }
}

/// Mark the checkpointed attempt interrupted (resumable) after a kill
/// mid-tool (ticket #60, spec #59): the phase on the record names where it
/// stopped, the status stays visibly distinct from failed, the ticket keeps
/// its ready label so the next pass resumes, and the worktree is released
/// (resume recreates it from the recorded phase). Counts as attempted.
fn interrupt_attempt(
    state: &std::path::Path,
    ledger: &mut DispatchLedger,
    idx: usize,
    log_path: &std::path::Path,
    worktree: &std::path::Path,
    ticket: u64,
    phase: &str,
) -> TicketOutcome {
    let attempt_no = ledger.attempts[idx].attempt_no;
    let slot = format!("T{ticket}-a{attempt_no}");
    append_log(
        log_path,
        &format!("interrupted during {phase} (slot {slot}) — resumable\n"),
    );
    ledger.attempts[idx].status = format!("{phase}_interrupted");
    ledger.attempts[idx].finished_at = now_stamp();
    if let Err(e) = save_ledger(state, ledger) {
        eprintln!("dispatch: {e}");
    }
    println!("  interrupted during {phase} — resumable next pass");
    gh_comment(
        ticket,
        &format!(
            "DISPATCH: attempt {attempt_no} interrupted during {phase} (resumable) — the next pass resumes from the checkpoint."
        ),
    );
    remove_worktree(worktree);
    TicketOutcome::Done
}

#[allow(clippy::too_many_lines)]
fn cmd_dispatch_ticket(
    state: &std::path::Path,
    ledger: &mut DispatchLedger,
    ticket: u64,
    max_attempts: u64,
    dry_run: bool,
) -> TicketOutcome {
    let view_text = match dgh(&[
        "issue",
        "view",
        &ticket.to_string(),
        "--json",
        "number,title,body,labels,state,comments",
    ]) {
        Ok(t) => t,
        Err(e) => {
            println!("dispatch: #{ticket} unreadable — skip ({e})");
            return TicketOutcome::Skipped;
        }
    };
    let view: serde_json::Value = match serde_json::from_str(&view_text) {
        Ok(v) => v,
        Err(e) => {
            println!("dispatch: #{ticket} unreadable — skip ({e})");
            return TicketOutcome::Skipped;
        }
    };
    let labels = dgh_labels(&view);
    if labels.iter().any(|l| l == "blocked") {
        println!("dispatch: #{ticket} blocked — skip");
        return TicketOutcome::Skipped;
    }
    if !labels.iter().any(|l| l == "ready") {
        println!("dispatch: #{ticket} not ready — skip");
        return TicketOutcome::Skipped;
    }
    if ledger
        .attempts
        .iter()
        .any(|a| a.ticket == ticket && (a.status == "impl_running" || a.status == "review_running"))
    {
        println!("dispatch: #{ticket} already active — skip");
        return TicketOutcome::Skipped;
    }
    let past = ledger
        .attempts
        .iter()
        .filter(|a| {
            a.ticket == ticket && (a.status.ends_with("_done") || a.status.ends_with("_failed"))
        })
        .count() as u64;
    if past >= max_attempts {
        println!("dispatch: #{ticket} exhausted ({past} attempts) — labelling blocked");
        if !dry_run {
            gh_comment(
                ticket,
                &format!(
                    "DISPATCH: attempt budget exhausted ({past}/{max_attempts}). Human decision required."
                ),
            );
            gh_relabel(ticket, Some("ready"), "blocked");
        }
        return TicketOutcome::Skipped;
    }
    let title = view
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let body = view
        .get("body")
        .and_then(|b| b.as_str())
        .unwrap_or("")
        .to_string();
    let mut open_deps = Vec::new();
    for dep in depends_on(&body) {
        let dep_text =
            dgh(&["issue", "view", &dep.to_string(), "--json", "state"]).unwrap_or_default();
        let dep_state = serde_json::from_str::<serde_json::Value>(&dep_text)
            .ok()
            .and_then(|v| {
                v.get("state")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        if dep_state.eq_ignore_ascii_case("open") {
            open_deps.push(dep);
        }
    }
    if !open_deps.is_empty() {
        let list = open_deps
            .iter()
            .map(|d| format!("#{d}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!("dispatch: #{ticket} waiting on open deps: {list} — skip");
        return TicketOutcome::Skipped;
    }
    let spec = linked_spec(&body);
    // Numbering follows the newest record, not the budget count: an
    // interrupted attempt is resumable (outside the done/failed budget)
    // but still occupies its number. Identical to past + 1 when no
    // interrupted row exists.
    let attempt_no = ledger
        .attempts
        .iter()
        .filter(|a| a.ticket == ticket)
        .map(|a| a.attempt_no)
        .max()
        .unwrap_or(0)
        + 1;
    let slot = format!("T{ticket}-a{attempt_no}");
    if dry_run {
        println!(
            "dry-run: would build factory/T{ticket}-<slug> @ {}/worktrees/{slot}, run implement, verify, review",
            state.display()
        );
        return TicketOutcome::Skipped;
    }
    let log_path = state
        .join("logs")
        .join(format!("T{ticket}-a{attempt_no}-impl.log"));
    let id = ledger.attempts.iter().map(|a| a.id).max().unwrap_or(0) + 1;

    let open_train_branch: String = load_open_train(state).map(|t| t.branch).unwrap_or_default();
    // Spec freeze gate: a drifted spec refuses the ticket before any
    // worktree exists. The refusal itself is a logged failed attempt.
    if let Some(s) = spec {
        if let Err(e) = dispatch_verify_spec(s) {
            println!("dispatch: #{ticket} spec #{s} freeze mismatch — needs re-freeze");
            append_log(&log_path, &format!("spec gate refused: {e}\n"));
            ledger.attempts.push(DispatchAttempt {
                id,
                ticket,
                attempt_no,
                base_commit: String::new(),
                head_commit: String::new(),
                branch: String::new(),
                worktree: String::new(),
                status: "impl_failed".to_string(),
                exit_code: 1,
                log_path: log_path.to_string_lossy().into_owned(),
                started_at: now_stamp(),
                finished_at: now_stamp(),
                train_branch: open_train_branch.clone(),
                phase: "unknown".to_string(),
            });
            log_event(&log_path, id, ticket, "spec-gate", 1);
            gh_comment(ticket, &format!("DISPATCH: linked spec #{s} body no longer matches its SPEC-APPROVED hash. Human must re-freeze before this ticket runs."));
            return TicketOutcome::Done;
        }
    }

    // Worktrees base on the open train head when a train is open
    // (read-only rev-parse); otherwise HEAD. Never touches dev/main.
    let base = if open_train_branch.is_empty() {
        let head = git_head(
            std::env::current_dir()
                .as_deref()
                .unwrap_or(std::path::Path::new(".")),
        );
        if head.is_empty() {
            "HEAD".to_string()
        } else {
            head
        }
    } else {
        train_head(&open_train_branch)
    };
    let base = if base.is_empty() {
        "HEAD".to_string()
    } else {
        base
    };
    let slug = slugify(&title);
    let slug = if slug.is_empty() {
        format!("ticket-{ticket}")
    } else {
        slug
    };
    let branch = format!("factory/T{ticket}-{slug}");
    let wt = state.join("worktrees").join(&slot);
    let tmpdir = state.join("tmp").join(&slot);
    let _ = std::fs::create_dir_all(&tmpdir);
    println!("dispatch: #{ticket} '{title}' (attempt {attempt_no})");

    // Reuse an existing ticket branch for retries; never touch dev/main.
    let branch_exists = run_git(&["rev-parse", "--verify", "--quiet", &branch], None)
        .map(|o| o.status.success())
        .unwrap_or(false);
    let wt_s = wt.to_string_lossy().to_string();
    let added = if branch_exists {
        run_git(&["worktree", "add", &wt_s, &branch], None)
    } else {
        run_git(&["worktree", "add", &wt_s, "-b", &branch, &base], None)
    };
    match added {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let msg = format!(
                "dispatch: #{ticket} worktree add failed: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            println!("{msg}");
            append_log(&log_path, &format!("{msg}\n"));
            log_event(&log_path, id, ticket, "worktree-add", 1);
            return TicketOutcome::Skipped;
        }
        Err(e) => {
            println!("dispatch: #{ticket} worktree add failed: {e}");
            return TicketOutcome::Skipped;
        }
    }
    ledger.attempts.push(DispatchAttempt {
        id,
        ticket,
        attempt_no,
        base_commit: base.clone(),
        head_commit: String::new(),
        branch: branch.clone(),
        worktree: wt_s.clone(),
        status: "impl_running".to_string(),
        exit_code: 0,
        log_path: log_path.to_string_lossy().into_owned(),
        started_at: now_stamp(),
        finished_at: String::new(),
        train_branch: open_train_branch.clone(),
        phase: "plan".to_string(),
    });
    let idx = ledger.attempts.len() - 1;
    // Plan checkpoint: committed before any side-effecting work begins.
    if let Err(e) = checkpoint_phase(state, ledger, idx, "plan") {
        eprintln!("dispatch: {e}");
        remove_worktree(&wt);
        return TicketOutcome::Skipped;
    }
    if simulated_kill_phase("plan") {
        return interrupt_attempt(state, ledger, idx, &log_path, &wt, ticket, "plan");
    }

    // --- implement ---
    let spec_label = spec
        .map(|s| s.to_string())
        .unwrap_or_else(|| "none".to_string());
    // Answer delivery (ticket #44, spec #41): the loop reads Bea's routing
    // rule HERE. Structured `Answer[id]` lines in the ticket's issue
    // comments snapshot once, now, into this brief: anything she posts
    // mid-attempt structurally waits for the next attempt (atomic
    // boundary). Delivered keys persist in the ledger BEFORE implement
    // runs, so a crash or retry never redelivers an answer (single-use).
    // The same words travel bash-side inside the brief
    // (`ANSWER_ROUTING_RULE`), because the worker is a shell-out
    // subprocess that reads only the brief.
    let mut brief = format!("Ticket issue: #{ticket}\nSpec issue: #{spec_label}\nBranch: {branch} (base {base})\n\n{body}\n");
    {
        let comment_bodies: Vec<String> = view
            .get("comments")
            .and_then(|c| c.as_array())
            .map(|comments| {
                comments
                    .iter()
                    .filter_map(|c| c.get("body").and_then(|b| b.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let (found, _) = collect_answers(&comment_bodies);
        let pending = pending_answers(&found, &ledger.delivered_answers, ticket);
        if !pending.is_empty() {
            brief.push('\n');
            brief.push_str(&render_answers_section(&pending));
            for answer in &pending {
                let key = answer_delivery_key(ticket, answer);
                if !ledger.delivered_answers.contains(&key) {
                    ledger.delivered_answers.push(key);
                }
            }
            if let Err(e) = save_ledger(state, ledger) {
                eprintln!("dispatch: {e}");
            }
        }
    }
    let _ = std::fs::write(wt.join(".factory-brief.md"), &brief);
    let build_prompt = PathBuf::from(".pi/prompts/build.md");
    if build_prompt.is_file() {
        if let Ok(prefix) = std::fs::read_to_string(&build_prompt) {
            let _ = std::fs::write(wt.join(".factory-prompt.md"), format!("{prefix}{brief}"));
        }
    }
    let prompt =
        std::fs::read_to_string(wt.join(".factory-prompt.md")).unwrap_or_else(|_| brief.clone());
    append_log(
        &log_path,
        &format!("dispatch attempt {slot} implement base={base} branch={branch}\n"),
    );
    // Implement checkpoint: committed before the implementer runs.
    if let Err(e) = checkpoint_phase(state, ledger, idx, "implement") {
        eprintln!("dispatch: {e}");
        remove_worktree(&wt);
        return TicketOutcome::Skipped;
    }
    if simulated_kill_phase("implement") {
        return interrupt_attempt(state, ledger, idx, &log_path, &wt, ticket, "implement");
    }
    println!("  implement: pi -p (slot {slot})");
    let (ok, _) = run_phase(
        &pi_bin(),
        &["-p", &prompt],
        &wt,
        &slot,
        &tmpdir,
        &log_path,
        "implement",
        id,
        ticket,
    );
    if !ok {
        println!("  implement failed — will retry next pass");
        gh_comment(ticket, &format!("DISPATCH: implementer failed (attempt {attempt_no}/{max_attempts}). Log kept in dispatcher state."));
        finish_attempt(state, ledger, idx, "impl_failed", 1);
        remove_worktree(&wt);
        return TicketOutcome::Done;
    }

    // --- verify ---
    println!("  verify: check adapter");
    let adapter = std::env::var_os("FACTORY_CHECK_ADAPTER")
        .map(PathBuf::from)
        .or_else(discover_adapter);
    let Some(adapter) = adapter else {
        append_log(&log_path, "verify: no check adapter found\n");
        gh_comment(
            ticket,
            &format!("DISPATCH: no check adapter found (attempt {attempt_no}/{max_attempts})."),
        );
        finish_attempt(state, ledger, idx, "impl_failed", 1);
        remove_worktree(&wt);
        return TicketOutcome::Done;
    };
    // Verify checkpoint: committed before the check adapter runs.
    if let Err(e) = checkpoint_phase(state, ledger, idx, "verify") {
        eprintln!("dispatch: {e}");
        remove_worktree(&wt);
        return TicketOutcome::Skipped;
    }
    if simulated_kill_phase("verify") {
        return interrupt_attempt(state, ledger, idx, &log_path, &wt, ticket, "verify");
    }
    let (vok, _) = run_phase(
        &adapter.to_string_lossy(),
        &[],
        &wt,
        &slot,
        &tmpdir,
        &log_path,
        "verify",
        id,
        ticket,
    );
    if !vok {
        println!("  checks FAILED — routing back");
        let log_text = std::fs::read_to_string(&log_path).unwrap_or_default();
        gh_comment(ticket, &format!("DISPATCH: factory-check failed (attempt {attempt_no}/{max_attempts}). Tail:\n```\n{}\n```", last_lines(&log_text, 30)));
        finish_attempt(state, ledger, idx, "impl_failed", 1);
        remove_worktree(&wt);
        return TicketOutcome::Done;
    }

    // --- evidence: HANDOFF.md goes to the issue, scaffolding never commits ---
    let mut handoff_text = String::new();
    let handoff_path = wt.join("HANDOFF.md");
    if handoff_path.is_file() {
        handoff_text = std::fs::read_to_string(&handoff_path).unwrap_or_default();
        gh_comment(ticket, &handoff_text);
        println!("  handoff posted to #{ticket}");
    }
    let _ = std::fs::remove_file(wt.join(".factory-brief.md"));
    let _ = std::fs::remove_file(wt.join(".factory-prompt.md"));
    let _ = std::fs::remove_file(&handoff_path);

    // --- head must exist ---
    let cur = git_head(&wt);
    let _ = run_git(&["add", "-A"], Some(&wt));
    let staged_clean = run_git(&["diff", "--cached", "--quiet"], Some(&wt))
        .map(|o| o.status.success())
        .unwrap_or(false);
    let mut head = cur.clone();
    if staged_clean && cur == base {
        if handoff_text.is_empty() {
            println!("  no changes produced — releasing");
            gh_comment(
                ticket,
                "DISPATCH: implementer produced no changes. Human decision required.",
            );
            finish_attempt(state, ledger, idx, "impl_done", 0);
            remove_worktree(&wt);
            return TicketOutcome::Done;
        }
        let _ = run_git(
            &[
                "-c",
                "user.name=factory-dispatch",
                "-c",
                "user.email=factory@local",
                "commit",
                "--allow-empty",
                "-qm",
                &format!("feat(T{ticket}): {title} [handoff-only]"),
            ],
            Some(&wt),
        );
        head = git_head(&wt);
    } else if !staged_clean {
        let _ = run_git(
            &[
                "-c",
                "user.name=factory-dispatch",
                "-c",
                "user.email=factory@local",
                "commit",
                "-qm",
                &format!("feat(T{ticket}): {title}"),
            ],
            Some(&wt),
        );
        head = git_head(&wt);
    }
    ledger.attempts[idx].head_commit = head.clone();
    ledger.attempts[idx].exit_code = 0;
    ledger.attempts[idx].status = "impl_done".to_string();
    if let Err(e) = save_ledger(state, ledger) {
        eprintln!("dispatch: {e}");
    }
    gh_relabel(ticket, Some("ready"), "review");

    // --- review (second detached worktree, reviewer output parsed, posted) ---
    let short = head.chars().take(8).collect::<String>();
    let rwt = state.join("worktrees").join(format!("review-{short}"));
    let rwt_s = rwt.to_string_lossy().to_string();
    if run_git(&["worktree", "add", "--detach", &rwt_s, &head], None)
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        // worktree ready
    } else {
        println!("  review worktree failed — routing back to ready");
        gh_relabel(ticket, Some("review"), "ready");
        finish_attempt(state, ledger, idx, "review_failed", 1);
        remove_worktree(&wt);
        return TicketOutcome::Done;
    }
    ledger.attempts[idx].status = "review_running".to_string();
    if let Err(e) = save_ledger(state, ledger) {
        eprintln!("dispatch: {e}");
    }
    let rlog = state
        .join("logs")
        .join(format!("T{ticket}-a{attempt_no}-review.log"));
    let report_path = state
        .join("logs")
        .join(format!("T{ticket}-a{attempt_no}-report.md"));
    let mut review_brief = format!("Repo worktree: {rwt_s}\nBASE_COMMIT: {base}\nHEAD_COMMIT: {head}\nTicket issue: #{ticket}. Spec issue: #{spec_label} (verify freeze before starting).\nWrite the FULL report to {}. End stdout with: VERDICT: accept|changes_requested\n\n", report_path.display());
    let review_prompt = PathBuf::from(".pi/prompts/review.md");
    if review_prompt.is_file() {
        if let Ok(suffix) = std::fs::read_to_string(&review_prompt) {
            review_brief.push_str(&suffix);
        }
    }
    let _ = std::fs::write(rwt.join(".factory-review-brief.md"), &review_brief);
    // Review checkpoint: committed before the reviewer runs.
    if let Err(e) = checkpoint_phase(state, ledger, idx, "review") {
        eprintln!("dispatch: {e}");
        remove_worktree(&rwt);
        remove_worktree(&wt);
        return TicketOutcome::Skipped;
    }
    if simulated_kill_phase("review") {
        remove_worktree(&rwt);
        return interrupt_attempt(state, ledger, idx, &log_path, &wt, ticket, "review");
    }
    println!("  review: pi -p");
    let (_, rout) = run_phase(
        &pi_bin(),
        &["-p", &review_brief],
        &rwt,
        &slot,
        &tmpdir,
        &rlog,
        "review",
        id,
        ticket,
    );
    let _ = std::fs::remove_file(rwt.join(".factory-review-brief.md"));
    let verdict = parse_verdict(&rout);
    if report_path.is_file() {
        if let Ok(text) = std::fs::read_to_string(&report_path) {
            gh_comment(ticket, &text);
        }
    } else {
        gh_comment(
            ticket,
            &format!(
                "DISPATCH: reviewer verdict: {}. Review log kept in dispatcher state.",
                verdict.as_deref().unwrap_or("none")
            ),
        );
    }
    remove_worktree(&rwt);
    remove_worktree(&wt);
    if verdict.as_deref() == Some("accept") {
        // Accept path: with an open train, auto-merge cleanly or route to
        // blocked on conflict (never forced). With no open train, auto-cut
        // a default-slug train (T35) and merge there instead of waiting;
        // a concurrently opened train is merged into, never duplicated.
        // Never merges into dev/main, never pushes.
        if !open_train_branch.is_empty() {
            match do_train_merge(ticket, &branch, &open_train_branch, state) {
                Ok(short) => {
                    gh_relabel(ticket, Some("review"), "integrated");
                    gh_comment(ticket, &format!("DISPATCH: review accepted; merged into train `{open_train_branch}` ({short}). Human: covered by the train→dev review."));
                    println!("  ACCEPTED + INTEGRATED into {open_train_branch} — #{ticket}");
                    finish_attempt(state, ledger, idx, "review_done", 0);
                }
                Err(e) => {
                    println!("  merge into train failed — routing to blocked (never forced)");
                    gh_relabel(ticket, Some("review"), "blocked");
                    gh_comment(ticket, &format!("DISPATCH: review accepted but merge into train `{open_train_branch}` failed ({e}) — merge aborted, nothing forced. Human: resolve, or `release drop {ticket}`."));
                    finish_attempt(state, ledger, idx, "review_failed", 0);
                }
            }
        } else {
            // Auto-cut (T35): no open train, so cut one (default slug)
            // and merge the accepted ticket into it. A concurrent open
            // refuses the cut — merge into that train instead, never a
            // second train. Any other cut failure falls back to the old
            // accepted-only record (base→head, human owns the land).
            let train = match cut_train(state, None) {
                Ok(t) => {
                    println!("  auto-cut train {t} (no open train)");
                    Some(t)
                }
                Err(e) if e.contains("already open") => {
                    match load_open_train(state) {
                        Some(t) => {
                            println!(
                            "  train opened concurrently ({}) — merging there, never a second train",
                            t.branch
                        );
                            Some(t.branch)
                        }
                        None => {
                            eprintln!("dispatch: auto-cut race ({e}) with no open train — recording accepted");
                            None
                        }
                    }
                }
                Err(e) => {
                    eprintln!("dispatch: auto-cut failed ({e}) — recording accepted");
                    None
                }
            };
            match train {
                Some(t) => {
                    ledger.attempts[idx].train_branch = t.clone();
                    if let Err(e) = save_ledger(state, ledger) {
                        eprintln!("dispatch: {e}");
                    }
                    match do_train_merge(ticket, &branch, &t, state) {
                        Ok(short) => {
                            gh_relabel(ticket, Some("review"), "integrated");
                            gh_comment(ticket, &format!("DISPATCH: review accepted; auto-cut train `{t}` and merged into it ({short}). Human: covered by the train→dev review."));
                            println!("  ACCEPTED + INTEGRATED into {t} (auto-cut) — #{ticket}");
                            finish_attempt(state, ledger, idx, "review_done", 0);
                        }
                        Err(e) => {
                            println!("  merge into auto-cut train failed — routing to blocked (never forced)");
                            gh_relabel(ticket, Some("review"), "blocked");
                            gh_comment(ticket, &format!("DISPATCH: review accepted; auto-cut train `{t}` but merge failed ({e}) — merge aborted, nothing forced. Human: resolve, or `release drop {ticket}`."));
                            finish_attempt(state, ledger, idx, "review_failed", 0);
                        }
                    }
                }
                None => {
                    // Accepted: human owns the land. Record base→head, never merge/push.
                    gh_relabel(ticket, Some("review"), "accepted");
                    gh_comment(ticket, &format!("DISPATCH: review accepted. Base `{base}` → head `{head}` on `{branch}`. Human: verify and land."));
                    println!("  ACCEPTED — #{ticket} awaiting human land");
                    finish_attempt(state, ledger, idx, "review_done", 0);
                }
            }
        }
    } else {
        println!("  changes_requested (or unreadable verdict) — back to ready");
        gh_relabel(ticket, Some("review"), "ready");
        finish_attempt(state, ledger, idx, "review_failed", 0);
    }
    TicketOutcome::Done
}

/// Auto-finish (T35): when the open train's head is an ancestor of dev,
/// the human has merged it — finish without the command (row closed,
/// train worktree removed, branch cleanup printed, branches never
/// deleted). Starts with a read-only `git fetch origin` (best effort:
/// a failed fetch never fails the pass) so a human merge pushed
/// elsewhere is observed. Unmerged trains are left untouched. With
/// `dry_run`, observes and prints but changes nothing.
/// Returns true when a train was finished this pass.
fn maybe_auto_finish(state: &std::path::Path, dry_run: bool) -> bool {
    let Some(train) = load_open_train(state) else {
        return false;
    };
    // Read-only fetch first: observe the human's merge wherever it landed.
    let _ = run_git(&["fetch", "origin"], None);
    if !train_merged_into_dev(&train.branch) {
        return false;
    }
    // Same empty-train guard as `release finish`: an untouched train whose
    // head still equals both its base and dev was never landed.
    let head_now = train_head(&train.branch);
    if head_now.is_empty() {
        return false;
    }
    let dev_head = rev_parse_ok("dev")
        .or_else(|| rev_parse_ok("origin/dev"))
        .unwrap_or_default();
    if head_now == train.base_commit && head_now == dev_head {
        return false;
    }
    if dry_run {
        println!(
            "dry-run: would finish merged train {} (row merged, worktree removed, cleanup printed; branches never deleted)",
            train.branch
        );
        return false;
    }
    match finish_train_closed(state) {
        Ok((branch, _members, cleanup)) => {
            println!("release: train {branch} merged into dev");
            println!("cleanup (human runs these, automation never deletes):");
            for line in &cleanup {
                println!("  {line}");
            }
            true
        }
        Err(e) => {
            eprintln!("dispatch auto-finish: {e}");
            false
        }
    }
}

pub fn cmd_dispatch_once(json: bool, dry_run: bool, idempotency_key: Option<&str>) -> i32 {
    let state = dispatch_state_dir();
    if state.join("STOP").is_file() {
        println!("dispatch: STOP file present — exiting");
        if json {
            println!("{{\"stopped\":true,\"attempted\":0}}");
        }
        return 0;
    }
    for sub in ["logs", "worktrees", "tmp"] {
        if let Err(e) = std::fs::create_dir_all(state.join(sub)) {
            eprintln!("dispatch: cannot create {}: {e}", state.join(sub).display());
            return 1;
        }
    }
    let Some(_lock) = DispatchLock::acquire(state.join(".lock")) else {
        println!("dispatch: another dispatcher holds the lock — exiting");
        return 0;
    };
    let mut ledger = load_ledger(&state);
    // Auto-finish (T35) runs before the ticket survey so a human-merged
    // train closes even on passes with no open tickets. Read-only fetch
    // first, unmerged trains untouched, dry-run changes nothing.
    maybe_auto_finish(&state, dry_run);
    let max_concurrent = dispatch_max_concurrent();
    let max_attempts = dispatch_max_attempts();
    if dispatch_active(&ledger) >= max_concurrent {
        println!(
            "dispatch: {} active, at capacity — exiting",
            dispatch_active(&ledger)
        );
        return 0;
    }
    let list_text = match dgh(&[
        "issue", "list", "--state", "open", "--label", "ticket", "--limit", "30", "--json",
        "number",
    ]) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("dispatch: {e}");
            return 1;
        }
    };
    let list: Vec<serde_json::Value> = serde_json::from_str(&list_text).unwrap_or_default();
    // Exactly-once keys (ticket #62, spec #59): the key binds to this
    // pass's operation fingerprint on first use. A repeat inside the
    // retention window replays the stored outcome (no double-admit, no
    // re-send); reuse across different operations is rejected. Expired
    // records read as absent so the same key admits anew. Dry-run
    // surveys stay side-effect free and record nothing.
    let surveyed: Vec<u64> = list
        .iter()
        .map(|item| {
            item.get("number")
                .and_then(|n| n.as_u64())
                .or_else(|| item.as_u64())
                .unwrap_or(0)
        })
        .filter(|t| *t != 0)
        .collect();
    let operation = idempotency_operation(&surveyed);
    let key: Option<String> = idempotency_key
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string);
    ledger.idempotency_keys.retain(idempotency_live);
    if let Some(ref k) = key {
        if let Some(rec) = ledger
            .idempotency_keys
            .iter()
            .find(|r| r.key == *k)
            .cloned()
        {
            if rec.operation == operation {
                println!(
                    "dispatch: idempotency key '{k}' replayed attempt {} for ticket #{} (status {}) \u{2014} no new attempt admitted",
                    rec.attempt_no, rec.ticket, rec.status
                );
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "stopped": false,
                            "attempted": 0,
                            "idempotency_key": k,
                            "replayed": true,
                            "operation": operation,
                            "ticket": rec.ticket,
                            "attempt_no": rec.attempt_no,
                            "status": rec.status,
                        })
                    );
                }
                return 0;
            }
            println!(
                "dispatch: idempotency key '{k}' already used for a different operation (stored '{}', this pass '{operation}') \u{2014} rejected",
                rec.operation
            );
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "stopped": false,
                        "attempted": 0,
                        "idempotency_key": k,
                        "rejected": true,
                        "error": format!("idempotency key '{k}' already used for a different operation"),
                    })
                );
            }
            return 1;
        }
    }
    if list.is_empty() {
        println!("dispatch: no open tickets");
        return 0;
    }
    let attempts_before = ledger.attempts.len();
    let mut attempted = 0u64;
    for item in &list {
        if dispatch_active(&ledger) >= max_concurrent {
            println!("dispatch: capacity reached");
            break;
        }
        let ticket = item
            .get("number")
            .and_then(|n| n.as_u64())
            .or_else(|| item.as_u64())
            .unwrap_or(0);
        if ticket == 0 {
            continue;
        }
        match cmd_dispatch_ticket(&state, &mut ledger, ticket, max_attempts, dry_run) {
            TicketOutcome::Done => {
                attempted += 1;
                if let Err(e) = save_ledger(&state, &ledger) {
                    eprintln!("dispatch: {e}");
                    return 1;
                }
            }
            TicketOutcome::Skipped => {}
        }
    }
    println!("dispatch: pass complete ({attempted} attempted)");
    if let Some(k) = key {
        if !dry_run {
            let (ticket, attempt_no, status) = ledger.attempts[attempts_before..]
                .first()
                .map(|a| (a.ticket, a.attempt_no, a.status.clone()))
                .unwrap_or((0, 0, "skipped".to_string()));
            ledger.idempotency_keys.retain(|r| r.key != k);
            ledger.idempotency_keys.push(IdempotencyRecord {
                key: k.clone(),
                operation: operation.clone(),
                ticket,
                attempt_no,
                status: status.clone(),
                stored_at: now_stamp(),
            });
            if let Err(e) = save_ledger(&state, &ledger) {
                eprintln!("dispatch: {e}");
                return 1;
            }
            if ticket == 0 {
                println!("dispatch: idempotency key '{k}' admitted no attempts");
            } else {
                println!(
                    "dispatch: idempotency key '{k}' admitted attempt {attempt_no} for ticket #{ticket} (status {status})"
                );
            }
        }
        if json {
            let (ticket, attempt_no, status) = ledger
                .idempotency_keys
                .iter()
                .find(|r| r.key == k)
                .map(|r| (r.ticket, r.attempt_no, r.status.clone()))
                .unwrap_or((0, 0, "skipped".to_string()));
            println!(
                "{}",
                serde_json::json!({
                    "stopped": false,
                    "attempted": attempted,
                    "idempotency_key": k,
                    "replayed": false,
                    "operation": operation,
                    "ticket": ticket,
                    "attempt_no": attempt_no,
                    "status": status,
                })
            );
        }
    } else if json {
        println!(
            "{{\"stopped\":false,\"attempted\":{attempted}}}",
            attempted = attempted
        );
    }
    0
}

pub fn cmd_dispatch_daemon(json: bool, interval: u64) -> i32 {
    let state = dispatch_state_dir();
    println!("dispatch daemon every {interval}s (STOP with: factory dispatch stop)");
    if json {
        println!(r#"{{"daemon":true,"interval":{interval}}}"#);
    }
    loop {
        // Each pass is exactly `dispatch once`: same ledger, same logs,
        // same MAX_CONCURRENT slots. The lock is per-pass (acquired and
        // released inside `cmd_dispatch_once`), so concurrent `once`
        // processes share slots with the daemon exactly as they do today.
        let code = cmd_dispatch_once(json, false, None);
        if code != 0 {
            // Transient failure: report and retry next interval instead of
            // killing autonomous operation (bash `set -e` would abort here;
            // continuing is the load-bearing choice for daemon mode).
            eprintln!("dispatch daemon: pass exited {code} — retrying in {interval}s");
        }
        if state.join("STOP").is_file() {
            println!("dispatch: STOP file present — daemon halting");
            return 0;
        }
        if interval > 0 {
            std::thread::sleep(std::time::Duration::from_secs(interval));
        }
    }
}

/// Strict ledger read shared by the read-only observers (`status`, `log`):
/// unlike the dispatch loop's lenient default-empty load, an observer must
/// say loudly when there is nothing readable to observe. Missing file and
/// corrupt JSON both fail with the ledger path and the cause named. Never
/// writes.
fn load_ledger_strict_for(cmd: &str, state: &std::path::Path) -> Result<DispatchLedger, String> {
    let path = ledger_path(state);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{cmd}: cannot read ledger {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("{cmd}: cannot parse ledger {}: {e}", path.display()))
}

fn load_ledger_strict(state: &std::path::Path) -> Result<DispatchLedger, String> {
    load_ledger_strict_for("status", state)
}

pub fn cmd_status(json: bool) -> i32 {
    let state = dispatch_state_dir();
    let ledger = match load_ledger_strict(&state) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let version = env!("CARGO_PKG_VERSION");
    let dest = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut files_present = Vec::new();
    let mut files_missing = Vec::new();
    for rel in STATUS_PROJECT_FILES {
        if dest.join(rel).is_file() {
            files_present.push(rel.to_string());
        } else {
            files_missing.push(rel.to_string());
        }
    }
    let project_present = files_missing.is_empty();
    let train = load_open_train(&state);
    let stopped = state.join("STOP").is_file();

    // Per-attempt facts: exactly what the dispatcher tracks, with the slot
    // derived the same way the dispatcher derives it (T<ticket>-a<attempt>).
    let attempts: Vec<serde_json::Value> = ledger
        .attempts
        .iter()
        .map(|a| {
            serde_json::json!({
                "ticket": a.ticket,
                "attempt": a.attempt_no,
                "slot": format!("T{}-a{}", a.ticket, a.attempt_no),
                "status": a.status,
                "phase": attempt_phase(a),
                "resumable": is_resumable(a),
                "base": a.base_commit,
                "head": a.head_commit,
                "started": a.started_at,
                "finished": a.finished_at,
            })
        })
        .collect();
    let train_value = match &train {
        None => serde_json::json!({"open": false}),
        Some(t) => {
            let head = train_head(&t.branch);
            let members = member_branches(&ledger, &t.branch);
            serde_json::json!({
                "open": true,
                "train": t.branch,
                "base": t.base_commit,
                "status": t.status,
                "head": head,
                "members": members,
            })
        }
    };

    if json {
        let mut files_map = serde_json::Map::new();
        for rel in STATUS_PROJECT_FILES {
            files_map.insert(
                rel.to_string(),
                serde_json::Value::Bool(!files_missing.iter().any(|m| m == rel)),
            );
        }
        println!(
            "{}",
            serde_json::json!({
                "version": version,
                "project": {
                    "present": project_present,
                    "files": files_map,
                    "missing": files_missing,
                },
                "train": train_value,
                "attempts": attempts,
                "stopped": stopped,
            })
        );
        return 0;
    }

    println!("factory {version}");
    if project_present {
        println!(
            "project: factory project files present ({}).",
            files_present.join(", ")
        );
    } else {
        println!(
            "project: not a fully seeded factory project (missing: {}).",
            files_missing.join(", ")
        );
    }
    match &train {
        None => println!("train: no open train."),
        Some(t) => {
            let head = train_value
                .get("head")
                .and_then(|h| h.as_str())
                .unwrap_or("");
            let members = train_value
                .get("members")
                .and_then(|m| m.as_array())
                .map(|m| m.len())
                .unwrap_or(0);
            let head_words = if head.is_empty() {
                "head not present locally".to_string()
            } else {
                format!("head {}", head.chars().take(8).collect::<String>())
            };
            println!(
                "train: release train {} is open and waiting for your merge ({head_words}, {members} member branches).",
                t.branch
            );
        }
    }
    if ledger.attempts.is_empty() {
        println!("attempts: no attempts yet.");
    } else {
        for a in &ledger.attempts {
            let slot = format!("T{}-a{}", a.ticket, a.attempt_no);
            match a.status.as_str() {
                "impl_running" => println!(
                    "Ticket {} attempt {} is being implemented (slot {}, started {}).",
                    a.ticket, a.attempt_no, slot, a.started_at
                ),
                "review_running" => println!(
                    "Ticket {} attempt {} is being reviewed (slot {}, started {}).",
                    a.ticket, a.attempt_no, slot, a.started_at
                ),
                s if is_interrupted_status(s) => println!(
                    "Ticket {} attempt {} interrupted during {} (slot {}, started {}, resumable).",
                    a.ticket,
                    a.attempt_no,
                    attempt_phase(a),
                    slot,
                    a.started_at
                ),
                other => {
                    if a.finished_at.is_empty() {
                        println!(
                            "Ticket {} attempt {} finished: {other} (slot {}, started {}).",
                            a.ticket, a.attempt_no, slot, a.started_at
                        );
                    } else {
                        println!(
                            "Ticket {} attempt {} finished: {other} (slot {}, started {}, finished {}).",
                            a.ticket, a.attempt_no, slot, a.started_at, a.finished_at
                        );
                    }
                }
            }
        }
    }
    if stopped {
        println!("STOP: dispatch loop is halted (STOP file present).");
    } else {
        println!("STOP: dispatch loop running (no STOP file).");
    }
    0
}

// ---------------------------------------------------------------------------
// LOG (ticket #25, spec #23 slice: ticket history on demand)
//
// Per-attempt entries newest-first with the outcome summary (the story)
// before the transcript tail (the evidence). Transcripts are the same
// logs-dir files the dispatcher writes: the ledger's implement log path
// plus the sibling per-attempt review log, which the ledger does not
// record. Strictly read-only: fs::read only, no writes, no git/gh calls.
// ---------------------------------------------------------------------------

/// Bea-readable one-line outcome for an attempt: the cause before evidence.
/// Shared by human and JSON modes so no fact lives only in prose.
fn log_summary(a: &DispatchAttempt) -> String {
    let slot = format!("T{}-a{}", a.ticket, a.attempt_no);
    match a.status.as_str() {
        "impl_running" => format!(
            "Ticket {} attempt {} is being implemented (slot {}, started {}).",
            a.ticket, a.attempt_no, slot, a.started_at
        ),
        "review_running" => format!(
            "Ticket {} attempt {} is being reviewed (slot {}, started {}).",
            a.ticket, a.attempt_no, slot, a.started_at
        ),
        "impl_done" => format!(
            "Ticket {} attempt {} finished implement, awaiting review (slot {}).",
            a.ticket, a.attempt_no, slot
        ),
        "impl_failed" => format!(
            "Ticket {} attempt {} failed during implement (exit {}).",
            a.ticket, a.attempt_no, a.exit_code
        ),
        "review_done" => format!(
            "Ticket {} attempt {} passed review (slot {}).",
            a.ticket, a.attempt_no, slot
        ),
        "review_failed" => format!(
            "Ticket {} attempt {} failed review: changes requested or merge blocked.",
            a.ticket, a.attempt_no
        ),
        s if is_interrupted_status(s) => format!(
            "Ticket {} attempt {} interrupted during {} (slot {}, resumable).",
            a.ticket,
            a.attempt_no,
            attempt_phase(a),
            slot
        ),
        other => format!(
            "Ticket {} attempt {} finished: {other} (slot {}).",
            a.ticket, a.attempt_no, slot
        ),
    }
}

/// Combined transcript for one attempt: the ledger's implement log plus the
/// sibling review log (same T<ticket>-a<attempt> stem). Missing files read
/// as empty — a running attempt may not have flushed one yet. Read-only.
fn log_read_transcript(state: &std::path::Path, a: &DispatchAttempt) -> String {
    let mut out = String::new();
    if !a.log_path.is_empty() {
        if let Ok(text) = std::fs::read_to_string(Path::new(&a.log_path)) {
            out.push_str(&text);
        }
    }
    let review = state
        .join("logs")
        .join(format!("T{}-a{}-review.log", a.ticket, a.attempt_no));
    if Path::new(&a.log_path) != review {
        if let Ok(text) = std::fs::read_to_string(&review) {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&text);
        }
    }
    out
}

/// Last `tail` lines of `text` (`tail == 0` means full). Returns the shown
/// text plus whether older lines were dropped.
fn log_tail(text: &str, tail: usize) -> (String, bool) {
    if tail == 0 {
        return (text.to_string(), false);
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= tail {
        return (text.to_string(), false);
    }
    (lines[lines.len() - tail..].join("\n") + "\n", true)
}

pub fn cmd_log(ticket_raw: &str, tail: usize, json: bool) -> i32 {
    let ticket = match parse_issue_number(ticket_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("log: {e}");
            return 1;
        }
    };
    let state = dispatch_state_dir();
    let ledger = match load_ledger_strict_for("log", &state) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut entries: Vec<&DispatchAttempt> = ledger
        .attempts
        .iter()
        .filter(|a| a.ticket == ticket)
        .collect();
    entries.sort_by(|a, b| b.attempt_no.cmp(&a.attempt_no).then(b.id.cmp(&a.id)));
    if entries.is_empty() {
        if json {
            println!("{}", serde_json::json!({"ticket": ticket, "attempts": []}));
        } else {
            println!("Ticket {ticket}: no attempts yet.");
        }
        return 0;
    }

    // Read every transcript first so JSON mode carries the same facts.
    let mut rows: Vec<serde_json::Value> = Vec::with_capacity(entries.len());
    for a in &entries {
        let full = log_read_transcript(&state, a);
        let (shown, truncated) = log_tail(&full, tail);
        rows.push(serde_json::json!({
            "ticket": a.ticket,
            "attempt": a.attempt_no,
            "slot": format!("T{}-a{}", a.ticket, a.attempt_no),
            "status": a.status,
            "summary": log_summary(a),
            "phase": attempt_phase(a),
            "resumable": is_resumable(a),
            "base": a.base_commit,
            "head": a.head_commit,
            "started": a.started_at,
            "finished": a.finished_at,
            "exit_code": a.exit_code,
            "transcript": shown,
            "truncated": truncated,
        }));
    }

    if json {
        println!(
            "{}",
            serde_json::json!({"ticket": ticket, "attempts": rows})
        );
        return 0;
    }

    println!(
        "Ticket {ticket} — {} attempt(s) (newest first).",
        rows.len()
    );
    for (i, row) in rows.iter().enumerate() {
        let attempt = row.get("attempt").and_then(|v| v.as_u64()).unwrap_or(0);
        let summary = row.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        let base = row.get("base").and_then(|v| v.as_str()).unwrap_or("");
        let head = row.get("head").and_then(|v| v.as_str()).unwrap_or("");
        let started = row.get("started").and_then(|v| v.as_str()).unwrap_or("");
        let finished = row.get("finished").and_then(|v| v.as_str()).unwrap_or("");
        let transcript = row.get("transcript").and_then(|v| v.as_str()).unwrap_or("");
        let span = if finished.is_empty() {
            format!("started {started}")
        } else {
            format!("started {started}, finished {finished}")
        };
        let head_words = if head.is_empty() {
            "head (none yet)".to_string()
        } else {
            format!("head {head}")
        };
        println!("Attempt {attempt}: {summary}");
        println!("Base {base} -> {head_words}, {span}.");
        if transcript.is_empty() {
            println!("(no transcript yet)");
        } else if tail == 0 {
            println!("Transcript (full):");
            print!("{transcript}");
            if !transcript.ends_with('\n') {
                println!();
            }
        } else {
            println!("Transcript (last {tail} lines):");
            print!("{transcript}");
            if !transcript.ends_with('\n') {
                println!();
            }
        }
        if i + 1 < rows.len() {
            println!("---");
        }
    }
    0
}

pub fn cmd_dispatch_status(json: bool) -> i32 {
    let state = dispatch_state_dir();
    let ledger = load_ledger(&state);
    let worktrees = run_git(&["worktree", "list"], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let stopped = state.join("STOP").is_file();
    if json {
        let wt_lines: Vec<&str> = worktrees.lines().collect();
        println!(
            "{}",
            serde_json::json!({"attempts": ledger.attempts, "worktrees": wt_lines, "stopped": stopped})
        );
    } else {
        println!("== dispatch attempts ==");
        if ledger.attempts.is_empty() {
            println!("(none)");
        }
        for a in &ledger.attempts {
            let base8: String = a.base_commit.chars().take(8).collect();
            let head8: String = a.head_commit.chars().take(8).collect();
            let resumable = if is_resumable(a) { " resumable" } else { "" };
            println!(
                "#{} T{} a{} {} phase={}{} base={} head={} {}",
                a.id,
                a.ticket,
                a.attempt_no,
                a.status,
                attempt_phase(a),
                resumable,
                base8,
                head8,
                a.started_at
            );
        }
        println!();
        println!("== worktrees ==");
        print!("{worktrees}");
        if !worktrees.ends_with('\n') {
            println!();
        }
        if stopped {
            println!("STOPPED (STOP file present)");
        } else {
            println!("running (no STOP file)");
        }
    }
    0
}

pub fn cmd_dispatch_stop(json: bool) -> i32 {
    let state = dispatch_state_dir();
    if let Err(e) = std::fs::create_dir_all(&state) {
        eprintln!("dispatch: cannot create {}: {e}", state.display());
        return 1;
    }
    match std::fs::write(state.join("STOP"), "stopped\n") {
        Ok(()) => {
            println!("stopped");
            if json {
                println!("{{\"stopped\":true}}");
            }
            0
        }
        Err(e) => {
            eprintln!("dispatch: cannot write STOP: {e}");
            1
        }
    }
}

pub fn cmd_dispatch_resume(json: bool) -> i32 {
    let state = dispatch_state_dir();
    let stop = state.join("STOP");
    if stop.is_file() {
        if let Err(e) = std::fs::remove_file(&stop) {
            eprintln!("dispatch: cannot remove STOP: {e}");
            return 1;
        }
    }
    println!("resumed");
    if json {
        println!("{{\"stopped\":false}}");
    }
    0
}

// ---------------------------------------------------------------------------
// HERMETIC SELF-TEST (ticket #15)
//
// The parity oracle: a hermetic proof of the release mechanics — temp bare
// origin, no gh, no network, no tokens, no spend. It drives the real
// `release` subcommands as child processes of this binary against a scratch
// clone, so green here is what parity means. Every step names the mechanics
// it proves; any failure prints FAIL with want/got and the run exits
// non-zero with SELF-TEST FAILURES. A gh-bomb script (records any call,
// exits 1) stands in for `gh`: a clean run proves the mechanics never
// needed it.
// ---------------------------------------------------------------------------

struct SelfTest {
    exe: PathBuf,
    proj: PathBuf,
    state: PathBuf,
    gh_bomb: PathBuf,
    gh_log: PathBuf,
    json: bool,
    checks: Vec<(String, bool)>,
}

impl SelfTest {
    fn pass(&mut self, name: &str) {
        self.checks.push((name.to_string(), true));
        if self.json {
            eprintln!("PASS: {name}");
        } else {
            println!("PASS: {name}");
        }
    }

    fn fail(&mut self, name: &str, want: &str, got: &str) {
        self.checks.push((name.to_string(), false));
        if self.json {
            eprintln!("FAIL: {name} (want [{want}] got [{got}])");
        } else {
            println!("FAIL: {name} (want [{want}] got [{got}])");
        }
    }

    fn check(&mut self, name: &str, ok: bool, want: &str, got: String) {
        if ok {
            self.pass(name);
        } else {
            self.fail(name, want, &got);
        }
    }

    fn base_env(&self, cmd: &mut Command) {
        cmd.env("FACTORY_STATE_DIR", &self.state)
            .env("FACTORY_GH_BIN", &self.gh_bomb)
            .env("GH_BOMB_LOG", &self.gh_log)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "selftest")
            .env("GIT_AUTHOR_EMAIL", "selftest@local")
            .env("GIT_COMMITTER_NAME", "selftest")
            .env("GIT_COMMITTER_EMAIL", "selftest@local")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_HOST")
            .env_remove("GITHUB_API_URL");
    }

    /// Run `factory <args...>` as a child with cwd=proj + hermetic env.
    fn factory(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new(&self.exe);
        cmd.args(args).current_dir(&self.proj);
        self.base_env(&mut cmd);
        cmd.output().expect("self-test child factory must spawn")
    }

    /// Run `git <args...>` in `dir` with hermetic env.
    fn git(&self, dir: &std::path::Path, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new(git_bin());
        cmd.args(args).current_dir(dir);
        self.base_env(&mut cmd);
        cmd.output().expect("self-test git must spawn")
    }
}

fn selftest_text(out: &std::process::Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// One-line failure evidence: exit code + first stderr line.
fn selftest_got(out: &std::process::Output, stderr: &str) -> String {
    let code = out
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or("?".to_string());
    let first = stderr.lines().next().unwrap_or("");
    format!("exit={code} err={first}")
}

/// Parse `release: train <branch> cut from <base>` from start output.
fn selftest_train(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("release: train "))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or("")
        .to_string()
}

/// Ledger with the DispatchLedger shape recording ticket branches on a train.
fn write_selftest_ledger(state: &std::path::Path, entries: &[(u64, &str, &str)]) {
    let _ = std::fs::create_dir_all(state);
    let attempts: Vec<serde_json::Value> = entries
        .iter()
        .enumerate()
        .map(|(i, (ticket, branch, train))| {
            serde_json::json!({
                "id": i as u64 + 1,
                "ticket": ticket,
                "attempt_no": 1,
                "base_commit": "selftest",
                "head_commit": "selftest",
                "branch": branch,
                "worktree": "",
                "status": "review_done",
                "exit_code": 0,
                "log_path": "",
                "started_at": "selftest",
                "finished_at": "selftest",
                "train_branch": train,
            })
        })
        .collect();
    let _ = std::fs::write(
        state.join("dispatch.json"),
        serde_json::json!({"attempts": attempts}).to_string(),
    );
}

#[allow(clippy::too_many_lines)]
pub fn cmd_dispatch_selftest(json: bool) -> i32 {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("factory"));
    let tmp = std::env::temp_dir().join(format!(
        "swefactory-selftest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let origin = tmp.join("origin.git");
    let proj = tmp.join("proj");
    let state = tmp.join("state");
    let gh_bomb = tmp.join("gh-bomb.sh");
    let gh_log = tmp.join("gh-bomb.log");

    // Summary + cleanup: exactly one stdout shape per mode, temp dir always
    // removed before returning.
    let finish = |st: &SelfTest, tmp: &std::path::Path| -> i32 {
        let failed = st.checks.iter().filter(|(_, ok)| !ok).count();
        if json {
            let checks: Vec<serde_json::Value> = st
                .checks
                .iter()
                .map(|(name, ok)| serde_json::json!({"name": name, "ok": ok}))
                .collect();
            println!(
                "{}",
                serde_json::json!({"passed": failed == 0, "checks": checks})
            );
            if failed == 0 {
                eprintln!("SELF-TEST ALL PASS");
            } else {
                eprintln!("SELF-TEST FAILURES ({failed} mechanics broke)");
            }
        } else if failed == 0 {
            println!("SELF-TEST ALL PASS");
        } else {
            println!("SELF-TEST FAILURES ({failed} mechanics broke)");
        }
        let _ = std::fs::remove_dir_all(tmp);
        if failed == 0 {
            0
        } else {
            1
        }
    };

    let mut st = SelfTest {
        exe,
        proj: proj.clone(),
        state: state.clone(),
        gh_bomb: gh_bomb.clone(),
        gh_log: gh_log.clone(),
        json,
        checks: Vec::new(),
    };

    // --- scratch setup: bare origin + clone on dev, pushed locally ---
    // (file remote only: no network can be involved).
    let setup_ok = (|| {
        std::fs::create_dir_all(&proj).ok()?;
        std::fs::create_dir_all(&state).ok()?;
        std::fs::write(
            &gh_bomb,
            "#!/bin/sh\necho \"GH CALLED: $*\" >> \"$GH_BOMB_LOG\"\nexit 1\n",
        )
        .ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh_bomb, std::fs::Permissions::from_mode(0o755)).ok()?;
        }
        let origin_s = origin.to_string_lossy().into_owned();
        let proj_s = proj.to_string_lossy().into_owned();
        if !st
            .git(&tmp, &["init", "--bare", "-q", &origin_s])
            .status
            .success()
        {
            return None;
        }
        if !st
            .git(&tmp, &["init", "-q", "-b", "dev", &proj_s])
            .status
            .success()
        {
            return None;
        }
        std::fs::write(proj.join("seed.txt"), "seed\n").ok()?;
        for args in [&["add", "-A"][..], &["commit", "-qm", "seed selftest"][..]] {
            if !st.git(&proj, args).status.success() {
                return None;
            }
        }
        if !st
            .git(&proj, &["remote", "add", "origin", &origin_s])
            .status
            .success()
        {
            return None;
        }
        if !st
            .git(&proj, &["push", "-q", "origin", "dev"])
            .status
            .success()
        {
            return None;
        }
        Some(())
    })();
    if setup_ok.is_none() {
        st.fail(
            "scratch setup",
            "bare origin + dev pushed locally",
            "git failed",
        );
        return finish(&st, &tmp);
    }

    // --- start cuts the train ---
    let out = st.factory(&["release", "start", "proof"]);
    let (stdout, _stderr) = selftest_text(&out);
    let train = selftest_train(&stdout);
    st.check(
        "start cuts train",
        out.status.success() && train.starts_with("integration/") && train.ends_with("-proof"),
        "integration/<date>-proof",
        if train.is_empty() {
            selftest_got(&out, &stdout)
        } else {
            train.clone()
        },
    );

    // --- a second train is refused while one is open ---
    let out = st.factory(&["release", "start", "again"]);
    let (_stdout, stderr) = selftest_text(&out);
    st.check(
        "second train refused",
        !out.status.success() && stderr.contains("already open"),
        "refused (already open)",
        selftest_got(&out, &stderr),
    );

    // --- status shows the open train ---
    let out = st.factory(&["release", "status"]);
    let (stdout, stderr) = selftest_text(&out);
    st.check(
        "status shows open train",
        out.status.success() && !train.is_empty() && stdout.contains(&train),
        &format!("status listing {train}"),
        selftest_got(&out, &stderr),
    );

    // --- clean merge ---
    st.git(&proj, &["checkout", "-qb", "factory/T1-demo"]);
    let _ = std::fs::write(proj.join("f.txt"), "eleven\n");
    st.git(&proj, &["add", "-A"]);
    st.git(&proj, &["commit", "-qm", "feat(T1): demo"]);
    st.git(&proj, &["checkout", "-q", "dev"]);
    write_selftest_ledger(&state, &[(1, "factory/T1-demo", &train)]);
    let out = st.factory(&["release", "merge", "1"]);
    let (stdout, stderr) = selftest_text(&out);
    st.check(
        "merge clean",
        out.status.success() && stdout.contains("merged"),
        "merged",
        selftest_got(&out, &stderr),
    );

    // --- merge commit present on the train ---
    let log = st.git(&proj, &["log", "--oneline", &train]);
    let (log_out, _) = selftest_text(&log);
    st.check(
        "merge commit present",
        log.status.success() && log_out.contains("Merge ticket #1"),
        "Merge ticket #1 in train log",
        log_out.lines().next().unwrap_or("(empty log)").to_string(),
    );

    // --- drop reverts the merge ---
    let out = st.factory(&["release", "drop", "1"]);
    let (stdout, stderr) = selftest_text(&out);
    st.check(
        "drop reverts",
        out.status.success() && stdout.contains("dropped"),
        "dropped",
        selftest_got(&out, &stderr),
    );

    // --- drop idempotence: a second drop fails loudly, never silently ---
    let out = st.factory(&["release", "drop", "1"]);
    let (_stdout, stderr) = selftest_text(&out);
    st.check(
        "drop-idempotence refusal",
        !out.status.success() && stderr.contains("NOT merged"),
        "refused (NOT merged)",
        selftest_got(&out, &stderr),
    );

    // --- conflict-abort: ticket and train sides collide on one file ---
    st.git(&proj, &["checkout", "-qb", "factory/T2-conf"]);
    let _ = std::fs::write(proj.join("c.txt"), "ticket-side\n");
    st.git(&proj, &["add", "-A"]);
    st.git(&proj, &["commit", "-qm", "ticket side"]);
    st.git(&proj, &["checkout", "-q", "dev"]);
    let twt = state.join("train-worktree");
    if twt.is_dir() {
        let _ = std::fs::write(twt.join("c.txt"), "train-side\n");
        st.git(&twt, &["add", "-A"]);
        st.git(&twt, &["commit", "-qm", "train-side"]);
    }
    let head_before = st.git(&proj, &["rev-parse", &train]);
    let (head_before_s, _) = selftest_text(&head_before);
    let head_before_s = head_before_s.trim().to_string();
    write_selftest_ledger(
        &state,
        &[
            (1, "factory/T1-demo", &train),
            (2, "factory/T2-conf", &train),
        ],
    );
    let out = st.factory(&["release", "merge", "2"]);
    let (_stdout, stderr) = selftest_text(&out);
    st.check(
        "conflict-abort refuses merge",
        !out.status.success() && stderr.contains("conflicted"),
        "refused (conflicted)",
        selftest_got(&out, &stderr),
    );

    // --- the aborted conflict leaves head + worktree clean ---
    let head_after = st.git(&proj, &["rev-parse", &train]);
    let (head_after_s, _) = selftest_text(&head_after);
    let head_after_s = head_after_s.trim().to_string();
    let porcelain = st.git(&twt, &["status", "--porcelain"]);
    let (porcelain_s, _) = selftest_text(&porcelain);
    st.check(
        "train clean after abort",
        !head_before_s.is_empty()
            && head_before_s == head_after_s
            && porcelain.status.success()
            && porcelain_s.trim().is_empty(),
        &format!("head={head_before_s} worktree clean"),
        format!("head={head_after_s} porcelain={}", porcelain_s.trim()),
    );

    // --- status still runs after the abort ---
    let out = st.factory(&["release", "status"]);
    let (_stdout, stderr) = selftest_text(&out);
    st.check(
        "status runs after abort",
        out.status.success(),
        "exit 0",
        selftest_got(&out, &stderr),
    );

    // --- finish refuses an unmerged train: the human must merge first ---
    let out = st.factory(&["release", "finish"]);
    let (_stdout, stderr) = selftest_text(&out);
    st.check(
        "finish unmerged refusal",
        !out.status.success() && stderr.contains("not merged"),
        "refused (not merged)",
        selftest_got(&out, &stderr),
    );

    // --- human merges the train into dev (automation never does this) ---
    st.git(&proj, &["checkout", "-q", "dev"]);
    st.git(
        &proj,
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            "human lands train",
            &train,
        ],
    );
    let out = st.factory(&["release", "finish"]);
    let (stdout, stderr) = selftest_text(&out);
    let train_closed = !state.join("train.json").is_file();
    st.check(
        "finish merged close",
        out.status.success()
            && stdout.contains("merged into dev")
            && stdout.contains("git branch -d")
            && train_closed,
        "merged into dev + cleanup + train closed",
        selftest_got(&out, &stderr),
    );

    // --- finish with no open train is refused ---
    let out = st.factory(&["release", "finish"]);
    let (_stdout, stderr) = selftest_text(&out);
    st.check(
        "finish no-train refusal",
        !out.status.success() && stderr.contains("no open train"),
        "refused (no open train)",
        selftest_got(&out, &stderr),
    );

    // --- abandon with no open train is refused ---
    let out = st.factory(&["release", "abandon"]);
    let (_stdout, stderr) = selftest_text(&out);
    st.check(
        "abandon refuses with no train",
        !out.status.success() && stderr.contains("no open train"),
        "refused (no open train)",
        selftest_got(&out, &stderr),
    );

    // --- a new train starts after the finish ---
    let out = st.factory(&["release", "start", "proof2"]);
    let (stdout, _stderr) = selftest_text(&out);
    let train2 = selftest_train(&stdout);
    st.check(
        "train restarts after finish",
        out.status.success() && train2.starts_with("integration/") && train2.ends_with("-proof2"),
        "integration/<date>-proof2",
        if train2.is_empty() {
            selftest_got(&out, &stdout)
        } else {
            train2.clone()
        },
    );

    // --- abandon closes the unmerged train without a merge check ---
    let out = st.factory(&["release", "abandon"]);
    let (stdout, stderr) = selftest_text(&out);
    let abandoned_closed = !state.join("train.json").is_file();
    st.check(
        "abandon closes unmerged train",
        out.status.success() && stdout.contains("abandoned") && abandoned_closed,
        "abandoned + train closed",
        selftest_got(&out, &stderr),
    );

    // --- abandon prints cleanup but never deletes the branch ---
    let branch = st.git(&proj, &["rev-parse", "--verify", "--quiet", &train2]);
    st.check(
        "abandon never deletes branches",
        !train2.is_empty() && branch.status.success(),
        &format!("{train2} still exists"),
        if train2.is_empty() {
            "(no train2)".to_string()
        } else {
            format!("{train2} deleted")
        },
    );

    // --- a second abandon is refused ---
    let out = st.factory(&["release", "abandon"]);
    let (_stdout, stderr) = selftest_text(&out);
    st.check(
        "second abandon refused",
        !out.status.success() && stderr.contains("no open train"),
        "refused (no open train)",
        selftest_got(&out, &stderr),
    );

    // --- the gh bomb never fired: the mechanics needed no gh ---
    let bomb = std::fs::read_to_string(&gh_log).unwrap_or_default();
    st.check(
        "no gh invoked",
        bomb.trim().is_empty(),
        "gh never called",
        bomb.lines().next().unwrap_or("(empty)").to_string(),
    );

    finish(&st, &tmp)
}

// ---------------------------------------------------------------------------
// RELEASE TRAIN (ticket #14)
//
// SAFETY CONTRACT (asserted in tests/cli_release.rs, not just documented):
// Release automation NEVER touches dev/main and NEVER deletes branches.
// There is deliberately NO code path here that pushes, merges into,
// checks out-and-commits-to, or branch-forces dev or main, and NO code
// path that executes `branch -D/-d` or `worktree remove` on anything but
// the train worktree itself. `git fetch`, `rev-parse`, `merge-base
// --is-ancestor`, `log`, and `status` are read-only. Train merges use
// `--no-ff --no-edit` ONLY with `merge --abort` on conflict — nothing is
// ever forced. `finish`/`abandon` only PRINT branch cleanup for the human;
// the human owns the train→dev merge and the land. No `git push` appears
// in this section at all.
// ---------------------------------------------------------------------------

#[derive(Serialize, serde::Deserialize, Clone, Debug)]
struct TrainState {
    branch: String,
    base_commit: String,
    #[serde(default = "default_train_status")]
    status: String,
    #[serde(default)]
    created_at: String,
}

fn default_train_status() -> String {
    "open".to_string()
}

fn train_path(state: &std::path::Path) -> PathBuf {
    state.join("train.json")
}

fn load_open_train(state: &std::path::Path) -> Option<TrainState> {
    let text = std::fs::read_to_string(train_path(state)).ok()?;
    let train: TrainState = serde_json::from_str(&text).ok()?;
    if train.status == "open" && !train.branch.is_empty() {
        Some(train)
    } else {
        None
    }
}

fn save_train(state: &std::path::Path, train: &TrainState) -> Result<(), String> {
    std::fs::create_dir_all(state)
        .map_err(|e| format!("cannot create {}: {e}", state.display()))?;
    let tmp = state.join("train.json.tmp");
    let text =
        serde_json::to_string_pretty(train).map_err(|e| format!("cannot serialize train: {e}"))?;
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write train: {e}"))?;
    std::fs::rename(&tmp, train_path(state)).map_err(|e| format!("cannot replace train: {e}"))?;
    Ok(())
}

fn clear_train(state: &std::path::Path) {
    let _ = std::fs::remove_file(train_path(state));
    let _ = std::fs::remove_file(state.join("train.json.tmp"));
}

/// YYYYMMDD stamp for `integration/<stamp>-<slug>` (parity with bash
/// `date +%Y%m%d`; hermetic fallback keeps tests deterministic).
fn date_stamp() -> String {
    if let Ok(out) = Command::new("date").arg("+%Y%m%d").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
                return s;
            }
        }
    }
    "19700101".to_string()
}

fn rev_parse_ok(rev: &str) -> Option<String> {
    run_git(&["rev-parse", "--verify", "--quiet", rev], None)
        .ok()
        .filter(|o| o.status.success())?;
    run_git(&["rev-parse", rev], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Read-only base for a new train: origin/dev, else dev, else HEAD.
/// Never checks out anything.
fn resolve_train_base() -> String {
    for candidate in ["origin/dev", "dev"] {
        if let Some(sha) = rev_parse_ok(candidate) {
            return sha;
        }
    }
    rev_parse_ok("HEAD").unwrap_or_else(|| "HEAD".to_string())
}

fn branch_exists(branch: &str) -> bool {
    run_git(&["rev-parse", "--verify", "--quiet", branch], None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn is_ancestor(ancestor: &str, descendant: &str) -> bool {
    run_git(&["merge-base", "--is-ancestor", ancestor, descendant], None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Train worktree lives under the state dir so hermetic tests with an
/// isolated FACTORY_STATE_DIR never collide (never a sibling ../-train).
fn train_worktree_path(state: &std::path::Path) -> PathBuf {
    state.join("train-worktree")
}

fn ensure_train_worktree(state: &std::path::Path, train: &str) -> Result<PathBuf, String> {
    let twt = train_worktree_path(state);
    let listed = run_git(&["worktree", "list", "--porcelain"], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    if listed.contains(&format!("worktree {}", twt.to_string_lossy())) {
        return Ok(twt);
    }
    if twt.exists() {
        return Err(format!(
            "release: {} exists but is not a registered worktree — remove it first",
            twt.display()
        ));
    }
    if let Some(parent) = twt.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("release: cannot create {}: {e}", parent.display()))?;
    }
    match run_git(&["worktree", "add", &twt.to_string_lossy(), train], None) {
        Ok(o) if o.status.success() => Ok(twt),
        Ok(o) => Err(format!(
            "release: git worktree add failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("release: {e}")),
    }
}

fn remove_train_worktree(state: &std::path::Path) {
    let twt = train_worktree_path(state);
    let s = twt.to_string_lossy().to_string();
    let removed = run_git(&["worktree", "remove", "--force", &s], None)
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !removed {
        let _ = std::fs::remove_dir_all(&twt);
    }
    let _ = run_git(&["worktree", "prune"], None);
}

/// Latest recorded branch for a ticket: prefer an attempt already on this
/// train, else the newest attempt with any branch.
fn ticket_branch_for_merge(ledger: &DispatchLedger, ticket: u64, train: &str) -> Option<String> {
    let mut best: Option<&DispatchAttempt> = None;
    for a in &ledger.attempts {
        if a.ticket != ticket || a.branch.is_empty() {
            continue;
        }
        match best {
            None => best = Some(a),
            Some(b) if a.id > b.id => best = Some(a),
            _ => {}
        }
    }
    best?;
    let mut same: Option<&DispatchAttempt> = None;
    for a in &ledger.attempts {
        if a.ticket == ticket && a.train_branch == train && !a.branch.is_empty() {
            match same {
                None => same = Some(a),
                Some(b) if a.id > b.id => same = Some(a),
                _ => {}
            }
        }
    }
    let chosen = same.or(best)?;
    if chosen.branch.is_empty() {
        None
    } else {
        Some(chosen.branch.clone())
    }
}

/// Merge a ticket branch into the train worktree with --no-ff --no-edit
/// ONLY. On conflict the merge is aborted and Err is returned — nothing
/// is ever forced and the train stays untouched.
fn do_train_merge(
    ticket: u64,
    tbranch: &str,
    train: &str,
    state: &std::path::Path,
) -> Result<String, String> {
    let twt = ensure_train_worktree(state, train)?;
    let found = run_git(&["rev-parse", "--verify", "--quiet", tbranch], Some(&twt))
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !found {
        return Err(format!("release: ticket branch {tbranch} not found"));
    }
    let dirty = run_git(&["status", "--porcelain"], Some(&twt))
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(true);
    if dirty {
        return Err(format!(
            "release: train worktree {} is dirty — refusing",
            twt.display()
        ));
    }
    let msg = format!("Merge ticket #{ticket} ({tbranch}) into {train}");
    match run_git(
        &["merge", "--no-ff", "--no-edit", "-m", &msg, tbranch],
        Some(&twt),
    ) {
        Ok(o) if o.status.success() => {}
        Ok(_) => {
            let _ = run_git(&["merge", "--abort"], Some(&twt));
            return Err(format!(
                "release: merge of #{ticket} conflicted — aborted, train untouched"
            ));
        }
        Err(e) => {
            let _ = run_git(&["merge", "--abort"], Some(&twt));
            return Err(format!("release: {e}"));
        }
    }
    let short = run_git(&["rev-parse", "--short", "HEAD"], Some(&twt))
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    Ok(short)
}

/// Newest merge of this ticket without a descendant Revert (a merge whose
/// subject already has a descendant `Revert "<subject>"` was dropped
/// before — keep looking older). Read-only.
fn find_unreverted_merge(
    twt: &std::path::Path,
    ticket: u64,
    train: &str,
) -> Option<(String, String)> {
    let out = run_git(
        &[
            "log",
            "--format=%H",
            "--merges",
            &format!("--grep=Merge ticket #{ticket} "),
            train,
        ],
        Some(twt),
    )
    .ok()
    .filter(|o| o.status.success())
    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    .unwrap_or_default();
    for cand in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let subject = run_git(&["log", "-1", "--format=%s", cand], Some(twt))
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if subject.is_empty() {
            continue;
        }
        let reverts = run_git(
            &[
                "log",
                "--format=%H",
                "--fixed-strings",
                &format!("--grep=Revert \"{subject}\""),
            ],
            Some(twt),
        )
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
        let mut dropped = false;
        for rsha in reverts.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if run_git(&["merge-base", "--is-ancestor", cand, rsha], Some(twt))
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                dropped = true;
                break;
            }
        }
        if dropped {
            continue;
        }
        return Some((cand.to_string(), subject));
    }
    None
}

fn member_branches(ledger: &DispatchLedger, train: &str) -> Vec<String> {
    let mut out = Vec::new();
    for a in &ledger.attempts {
        if a.train_branch == train && !a.branch.is_empty() && !out.contains(&a.branch) {
            out.push(a.branch.clone());
        }
    }
    if out.is_empty() {
        // Back-compat: ledgers written before train_branch existed.
        for a in &ledger.attempts {
            if !a.branch.is_empty() && !out.contains(&a.branch) {
                out.push(a.branch.clone());
            }
        }
    }
    out.sort();
    out
}

fn train_head(train: &str) -> String {
    run_git(&["rev-parse", train], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Cut a new integration train from the dev base and record it open.
/// Shared by `release start` and dispatch auto-cut (T35): same refusal
/// when a train is already open (never two open trains), same refusal
/// when the branch name exists, same dev base. Returns the train branch.
/// Read-write: creates the branch + train.json. Never touches dev/main.
fn cut_train(state: &std::path::Path, slug_opt: Option<&str>) -> Result<String, String> {
    if let Err(e) = std::fs::create_dir_all(state) {
        return Err(format!("release: cannot create {}: {e}", state.display()));
    }
    if let Some(existing) = load_open_train(state) {
        return Err(format!(
            "release: train already open ({}) — merge or abandon it first; never two open trains",
            existing.branch
        ));
    }
    let mut slug = slugify(slug_opt.unwrap_or("train"));
    if slug.is_empty() {
        slug = "train".to_string();
    }
    let stamp = date_stamp();
    let train = format!("integration/{stamp}-{slug}");
    if branch_exists(&train) {
        return Err(format!(
            "release: branch {train} already exists — refusing to reuse"
        ));
    }
    let base = resolve_train_base();
    match run_git(&["branch", &train, &base], None) {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            return Err(format!(
                "release: git branch failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Err(e) => {
            return Err(format!("release: {e}"));
        }
    }
    let record = TrainState {
        branch: train.clone(),
        base_commit: base.clone(),
        status: "open".to_string(),
        created_at: now_stamp(),
    };
    if let Err(e) = save_train(state, &record) {
        return Err(format!("release: {e}"));
    }
    Ok(train)
}

pub fn cmd_release_start(json: bool, slug_opt: Option<&str>) -> i32 {
    let state = dispatch_state_dir();
    let base_hint = resolve_train_base();
    match cut_train(&state, slug_opt) {
        Ok(train) => {
            // Base was resolved inside cut_train; re-read for the report.
            let base = load_open_train(&state)
                .map(|t| t.base_commit)
                .unwrap_or(base_hint);
            if json {
                println!(
                    "{}",
                    serde_json::json!({"train": train, "base": base, "open": true})
                );
            } else {
                println!("release: train {train} cut from {base}");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

pub fn cmd_release_status(json: bool) -> i32 {
    let state = dispatch_state_dir();
    let Some(train) = load_open_train(&state) else {
        if json {
            println!("{{\"open\":false}}");
        } else {
            println!("release: no open train");
        }
        return 0;
    };
    let ledger = load_ledger(&state);
    let members = member_branches(&ledger, &train.branch);
    let head = train_head(&train.branch);
    if json {
        println!(
            "{}",
            serde_json::json!({"open": true, "train": train.branch, "base": train.base_commit, "status": train.status, "head": head, "members": members})
        );
    } else {
        println!(
            "release: open train {} (base {})",
            train.branch,
            train.base_commit.chars().take(8).collect::<String>()
        );
        println!("--- member ticket branches ---");
        if members.is_empty() {
            println!("(none)");
        } else {
            for m in &members {
                println!("{m}");
            }
        }
        println!("--- train head (CI-relevant) ---");
        if head.is_empty() {
            println!("train branch {} not present locally", train.branch);
        } else {
            println!("{head}");
        }
    }
    0
}

pub fn cmd_release_merge(json: bool, ticket_raw: &str) -> i32 {
    let ticket = match parse_issue_number(ticket_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("release: {e}");
            return 1;
        }
    };
    let state = dispatch_state_dir();
    let Some(train) = load_open_train(&state) else {
        eprintln!("release: no open train");
        return 1;
    };
    let ledger = load_ledger(&state);
    let Some(tbranch) = ticket_branch_for_merge(&ledger, ticket, &train.branch) else {
        eprintln!("release: no recorded branch for ticket #{ticket}");
        return 1;
    };
    match do_train_merge(ticket, &tbranch, &train.branch, &state) {
        Ok(short) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"train": train.branch, "ticket": ticket, "branch": tbranch, "head": short, "merged": true})
                );
            } else {
                println!("release: #{ticket} merged into {} ({short})", train.branch);
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

pub fn cmd_release_drop(json: bool, ticket_raw: &str) -> i32 {
    let ticket = match parse_issue_number(ticket_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("release: {e}");
            return 1;
        }
    };
    let state = dispatch_state_dir();
    let Some(train) = load_open_train(&state) else {
        eprintln!("release: no open train");
        return 1;
    };
    let twt = match ensure_train_worktree(&state, &train.branch) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let found = find_unreverted_merge(&twt, ticket, &train.branch);
    let Some((merge_sha, _subject)) = found else {
        eprintln!(
            "release: ticket #{ticket} is NOT merged into {} (or already dropped) — nothing to drop",
            train.branch
        );
        return 1;
    };
    match run_git(&["revert", "-m", "1", "--no-edit", &merge_sha], Some(&twt)) {
        Ok(o) if o.status.success() => {}
        Ok(_) => {
            let _ = run_git(&["revert", "--abort"], Some(&twt));
            eprintln!("release: revert of #{ticket} conflicted — aborted");
            return 1;
        }
        Err(e) => {
            let _ = run_git(&["revert", "--abort"], Some(&twt));
            eprintln!("release: revert failed: {e}");
            return 1;
        }
    }
    let short = run_git(&["rev-parse", "--short", "HEAD"], Some(&twt))
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if json {
        println!(
            "{}",
            serde_json::json!({"train": train.branch, "ticket": ticket, "dropped": true, "revert": short})
        );
    } else {
        println!(
            "release: ticket #{ticket} dropped from {} (revert {short})",
            train.branch
        );
    }
    0
}

/// Human-merge candidates, in order. Both are read-only checks; the human
/// may have merged locally (`dev`) or the checkout may only have the
/// remote (`origin/dev`).
fn train_merged_into_dev(train: &str) -> bool {
    for dev in ["dev", "origin/dev"] {
        if branch_exists(dev) && is_ancestor(train, dev) {
            return true;
        }
    }
    false
}

fn cleanup_lines(train: &str, members: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    for m in members {
        lines.push(format!("git branch -d {m}"));
    }
    lines.push(format!("git branch -d {train}"));
    lines.push("git worktree prune".to_string());
    lines
}

/// Verify the human merged the open train into dev and close it:
/// remove the train worktree, clear the train row, return the branch,
/// its member branches, and the human-run cleanup lines (never executed
/// here — branches are never deleted by automation). Shared by `release
/// finish` and dispatch auto-finish (T35): identical merge predicate
/// (ancestor check + empty-train guard), identical effects.
fn finish_train_closed(
    state: &std::path::Path,
) -> Result<(String, Vec<String>, Vec<String>), String> {
    let train = load_open_train(state).ok_or_else(|| "release: no open train".to_string())?;
    if !train_merged_into_dev(&train.branch) {
        return Err(format!(
            "release: train {} not merged into dev — human must merge first",
            train.branch
        ));
    }
    // An empty train (head == base) is trivially an ancestor but was never
    // landed: refuse only when dev still points at the same commit (no
    // human merge happened). After a human `--no-ff` merge of an empty
    // train, dev has a new merge commit so dev != train and finish succeeds.
    let head_now = train_head(&train.branch);
    if head_now.is_empty() {
        return Err(format!(
            "release: train {} not merged into dev — human must merge first",
            train.branch
        ));
    }
    let dev_head = rev_parse_ok("dev")
        .or_else(|| rev_parse_ok("origin/dev"))
        .unwrap_or_default();
    if head_now == train.base_commit && head_now == dev_head {
        return Err(format!(
            "release: train {} not merged into dev — human must merge first",
            train.branch
        ));
    }
    let ledger = load_ledger(state);
    let members = member_branches(&ledger, &train.branch);
    remove_train_worktree(state);
    clear_train(state);
    let cleanup = cleanup_lines(&train.branch, &members);
    Ok((train.branch, members, cleanup))
}

pub fn cmd_release_finish(json: bool) -> i32 {
    let state = dispatch_state_dir();
    match finish_train_closed(&state) {
        Ok((branch, _members, cleanup)) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"train": branch, "merged": true, "cleanup": cleanup})
                );
            } else {
                println!("release: train {branch} merged into dev");
                println!("cleanup (human runs these, automation never deletes):");
                for line in &cleanup {
                    println!("  {line}");
                }
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

pub fn cmd_release_abandon(json: bool) -> i32 {
    let state = dispatch_state_dir();
    let Some(train) = load_open_train(&state) else {
        eprintln!("release: no open train");
        return 1;
    };
    // No merge check by design: an unmerged train closes cleanly.
    remove_train_worktree(&state);
    clear_train(&state);
    if json {
        println!(
            "{}",
            serde_json::json!({"train": train.branch, "abandoned": true})
        );
    } else {
        println!("release: train {} abandoned (unmerged)", train.branch);
    }
    0
}

/// CLI entry: parse args and dispatch to the command functions below.
/// The binary (`main.rs`) is a thin `exit(run())` wrapper so future Axum
/// handlers can call the same functions in-process.
pub fn run() -> i32 {
    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            print!("{}", version_text(cli.json));
            0
        }
        Commands::Status => cmd_status(cli.json),
        Commands::Log { ticket, tail } => cmd_log(&ticket, tail, cli.json),
        Commands::Init {
            stack,
            global_skills,
            interactive,
        } => {
            let out = run_init(
                cli.json,
                stack.as_deref(),
                global_skills,
                None,
                "init",
                interactive,
            );
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&out).expect("init JSON serialization")
                );
            }
            0
        }
        Commands::Update { pin } => {
            // update refreshes from the same embedded templates; the stack
            // is detected from the installed adapter (never assumed), and
            // seeding never overwrites, so it is a no-op when in place.
            let dest = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let stack = detect_stack(&dest);
            let out = run_init(
                cli.json,
                Some(&stack),
                false,
                pin.as_deref(),
                "update",
                false,
            );
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&out).expect("update JSON serialization")
                );
            }
            0
        }
        Commands::Freeze { issue, label } => cmd_freeze(cli.json, &issue, &label),
        Commands::Sync { issue } => cmd_sync(cli.json, &issue),
        Commands::VerifyFreeze { issue } => cmd_verify_freeze(cli.json, &issue),
        Commands::Check {
            adapter,
            slot,
            args,
        } => run_check(cli.json, adapter, slot, args),
        Commands::Doctor { fix } => cmd_doctor(cli.json, fix),
        Commands::Handoff {
            issue,
            signal,
            body,
        } => cmd_handoff(&issue, &signal, body.as_ref(), cli.json),
        Commands::Reconcile => cmd_reconcile(cli.json),
        Commands::Start { ticket, worktree } => cmd_start(&ticket, worktree.as_ref(), cli.json),
        Commands::Dispatch { action } => match action {
            DispatchAction::Once {
                dry_run,
                idempotency_key,
            } => cmd_dispatch_once(cli.json, dry_run, idempotency_key.as_deref()),
            DispatchAction::Daemon { interval } => cmd_dispatch_daemon(cli.json, interval),
            DispatchAction::Status => cmd_dispatch_status(cli.json),
            DispatchAction::Stop => cmd_dispatch_stop(cli.json),
            DispatchAction::Resume => cmd_dispatch_resume(cli.json),
            DispatchAction::SelfTest => cmd_dispatch_selftest(cli.json),
        },
        Commands::Release { action } => match action {
            ReleaseAction::Start { slug } => cmd_release_start(cli.json, slug.as_deref()),
            ReleaseAction::Status => cmd_release_status(cli.json),
            ReleaseAction::Merge { ticket } => cmd_release_merge(cli.json, &ticket),
            ReleaseAction::Drop { ticket } => cmd_release_drop(cli.json, &ticket),
            ReleaseAction::Finish => cmd_release_finish(cli.json),
            ReleaseAction::Abandon => cmd_release_abandon(cli.json),
        },
        Commands::Land { dry_run, train } => cmd_land(cli.json, dry_run, train),
        Commands::Ui { addr, allow_remote } => cmd_ui(&addr, allow_remote, cli.json),
        Commands::Serve { addr, allow_remote } => cmd_ui(&addr, allow_remote, cli.json),
        Commands::Onboard {
            plain,
            repo,
            new,
            visibility,
            idea,
            approve,
            change,
        } => cmd_onboard(
            cli.json,
            OnboardOpts {
                plain,
                repo,
                new,
                visibility,
                idea,
                approve,
                change,
            },
        ),
    }
}

// ---------------------------------------------------------------------------
// LAND (ticket #21, spec #20: one-command human accept-and-land)
//
// SAFETY CONTRACT: preflight (train resolves → tree clean → head green →
// remote reachable) runs before ANY mutation; the first failure stops
// everything. The sequence then merges train→dev --no-ff, pushes dev,
// closes member tickets with the release note, removes the train
// worktree, and PRINTS branch cleanup for the human (never executed:
// no `branch -d/-D` reaches git). --dry-run runs the same preflight
// and prints the full mutation plan with zero state changes.
// ---------------------------------------------------------------------------

/// True when the current checkout has no staged or unstaged changes.
/// Read-only; the first mutation guard after train resolution.
fn land_working_tree_clean() -> bool {
    run_git(&["status", "--porcelain"], None)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

fn land_check_adapter() -> Option<PathBuf> {
    std::env::var_os("FACTORY_CHECK_ADAPTER")
        .map(PathBuf::from)
        .or_else(discover_adapter)
}

/// Stage the train head in an ephemeral detached worktree so the check
/// runner verifies exactly what would land. Removed after the check
/// (pass or fail) so no state survives preflight.
fn land_temp_worktree(train: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!(
        "swefactory-land-check-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    match run_git(
        &["worktree", "add", "--detach", &dir.to_string_lossy(), train],
        None,
    ) {
        Ok(o) if o.status.success() => Ok(dir),
        Ok(o) => Err(format!(
            "land: cannot stage train head for checks: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("land: {e}")),
    }
}

fn land_drop_temp_worktree(path: &Path) {
    let s = path.to_string_lossy().to_string();
    let _ = run_git(&["worktree", "remove", "--force", &s], None);
    let _ = std::fs::remove_dir_all(path);
    let _ = run_git(&["worktree", "prune"], None);
}

/// Head-green gate: run the check adapter against the train head.
/// The error names the failing adapter, its exit code, and its tail.
fn land_run_checks(train: &str) -> Result<(), String> {
    let Some(adapter) = land_check_adapter() else {
        return Err("land: train head checks failed: no check adapter found (set FACTORY_CHECK_ADAPTER or run under a project with scripts/factory-check)".to_string());
    };
    let twt = land_temp_worktree(train)?;
    let out = Command::new(&adapter).current_dir(&twt).output();
    land_drop_temp_worktree(&twt);
    let out = out.map_err(|e| {
        format!(
            "land: train head checks failed: adapter {} could not run: {e}",
            adapter.display()
        )
    })?;
    if out.status.success() {
        return Ok(());
    }
    let code = out.status.code().unwrap_or(1);
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    Err(format!(
        "land: train head checks failed: adapter {} exited {code}:\n{}",
        adapter.display(),
        last_lines(&combined, 10)
    ))
}

/// Read-only reachability probe: the push must have somewhere to go.
fn land_remote_reachable() -> bool {
    run_git(&["ls-remote", "origin", "HEAD"], None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Distinct member ticket numbers for this train (newest ledger rule
/// first, with the same back-compat fallback `member_branches` uses).
fn member_tickets(ledger: &DispatchLedger, train: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for a in &ledger.attempts {
        if a.train_branch == train && !out.contains(&a.ticket) {
            out.push(a.ticket);
        }
    }
    if out.is_empty() {
        for a in &ledger.attempts {
            if !out.contains(&a.ticket) {
                out.push(a.ticket);
            }
        }
    }
    out.sort();
    out
}

fn land_short(sha: &str) -> String {
    sha.chars().take(8).collect()
}

/// Checkout dev for the release merge. The working tree is clean
/// (preflight), so the switch carries no unrelated work.
fn land_checkout_dev() -> Result<(), String> {
    match run_git(&["checkout", "-q", "dev"], None) {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!(
            "land: cannot checkout dev: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("land: {e}")),
    }
}

/// Merge train→dev --no-ff with the train name in the message.
/// Returns the dev head short sha. On conflict the merge is aborted
/// and nothing is pushed or closed. An already-merged train
/// (re-run after a half-finished land) succeeds as a no-op.
fn land_merge_train(train: &str, msg: &str) -> Result<String, String> {
    match run_git(&["merge", "--no-ff", "--no-edit", "-m", msg, train], None) {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let _ = run_git(&["merge", "--abort"], None);
            return Err(format!(
                "land: merge of {train} into dev failed ({}) — aborted, nothing pushed or closed",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Err(e) => {
            let _ = run_git(&["merge", "--abort"], None);
            return Err(format!("land: {e}"));
        }
    }
    Ok(land_short(
        &run_git(&["rev-parse", "HEAD"], None)
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default(),
    ))
}

fn land_push_dev() -> Result<(), String> {
    match run_git(&["push", "origin", "dev"], None) {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!(
            "land: push of dev to origin failed ({}) — merge stays local, re-run land to complete",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("land: {e}")),
    }
}

/// Close one member ticket with the release note. Already-closed
/// tickets skip cleanly so a re-run after a half-finished land
/// completes closes without duplicates. Returns true when a close ran.
fn land_close_ticket(ticket: u64, note: &str) -> Result<bool, String> {
    let view = dgh(&[
        "issue",
        "view",
        &ticket.to_string(),
        "--json",
        "number,state",
    ])?;
    let v: serde_json::Value = serde_json::from_str(&view)
        .map_err(|e| format!("land: cannot parse gh issue view {ticket}: {e}"))?;
    if v.get("state")
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .eq_ignore_ascii_case("closed")
    {
        return Ok(false);
    }
    dgh(&["issue", "close", &ticket.to_string(), "--comment", note]).map_err(|e| {
        format!("land: close of #{ticket} failed: {e} — merge+push done, re-run land to complete")
    })?;
    Ok(true)
}

/// Release note stamped on every member-ticket close. Names the train
/// and its head (stable across plan and execution: the merge never
/// moves the train), so the dry-run plan shows the exact note.
fn land_release_note(train: &str) -> String {
    format!(
        "Landed {} into dev (train head {}).",
        train,
        land_short(&train_head(train))
    )
}

/// Dispatcher-context detector: land is human-owned and must never run
/// under automation. Dispatch attempt phases export FACTORY_SLOT for
/// isolation (see run_phase), so its presence marks a dispatcher-owned
/// process tree. FACTORY_DISPATCHER / FACTORY_DISPATCH are explicit
/// overrides for future automation callers.
fn land_under_automation() -> bool {
    for key in ["FACTORY_SLOT", "FACTORY_DISPATCHER", "FACTORY_DISPATCH"] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

fn land_refusal() -> String {
    "land: refusing to run under automation — landing is human-owned. The dispatcher must never invoke `factory land`; ask a human to run it."
        .to_string()
}

pub fn cmd_land(json: bool, dry_run: bool, train_opt: Option<String>) -> i32 {
    let state = dispatch_state_dir();

    // Human-owned invariant: automation must never invoke land, not even
    // --dry-run (invoking it IS the approval). Refuse before anything else.
    if land_under_automation() {
        let msg = land_refusal();
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "train": train_opt.clone().unwrap_or_default(),
                    "dry_run": dry_run,
                    "steps": [{"step": "preflight", "status": "refused", "detail": msg}],
                })
            );
        }
        eprintln!("{msg}");
        return 1;
    }

    // Target train: explicit --train wins, else the open train. A named
    // branch outside the open record must still exist locally.
    let open = load_open_train(&state);
    let target: String = match train_opt.clone() {
        Some(b) => b,
        None => match &open {
            Some(t) => t.branch.clone(),
            None => {
                eprintln!("land: no open train — cut one with `release start` first");
                return 1;
            }
        },
    };
    if open.as_ref().map(|t| t.branch.as_str()) != Some(target.as_str()) && !branch_exists(&target)
    {
        eprintln!("land: train branch {target} not found — cut one with `release start` first");
        return 1;
    }
    if let Err(e) = train_land_preflight(&target) {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "train": target,
                    "dry_run": dry_run,
                    "steps": [{"step": "preflight", "status": "failed", "detail": e}],
                })
            );
        }
        eprintln!("{e}");
        return 1;
    }
    let ledger = load_ledger(&state);
    let tickets = member_tickets(&ledger, &target);
    let members = member_branches(&ledger, &target);
    let cleanup = cleanup_lines(&target, &members);
    let merge_msg = format!("Merge {target} into dev");
    let note = land_release_note(&target);
    if dry_run {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "train": target,
                    "dry_run": true,
                    "preflight": "ok",
                    "merge": merge_msg,
                    "push": "origin dev",
                    "close": tickets,
                    "note": note,
                    "cleanup": cleanup,
                    "steps": [
                        {"step": "preflight", "status": "ok"},
                        {"step": "merge", "status": "planned", "detail": merge_msg},
                        {"step": "push", "status": "planned", "detail": "origin dev"},
                        {"step": "closes", "status": "planned", "closed": tickets},
                        {"step": "cleanup", "status": "planned", "cleanup": cleanup},
                    ],
                })
            );
        } else {
            println!("land: dry-run plan for {target} (no changes made)");
            println!("  git checkout dev");
            println!("  git merge --no-ff --no-edit -m \"{merge_msg}\" {target}");
            println!("  git push origin dev");
            for t in &tickets {
                println!("  gh issue close {t} --comment \"{note}\"");
            }
            println!(
                "  remove train worktree {}",
                train_worktree_path(&state).display()
            );
            println!("  cleanup (human runs these, automation never deletes):");
            for line in &cleanup {
                println!("    {line}");
            }
        }
        return 0;
    }
    // --- sequence: merge train→dev --no-ff → push dev → close members ---
    if let Err(e) = land_checkout_dev() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "train": target,
                    "dry_run": false,
                    "steps": [
                        {"step": "preflight", "status": "ok"},
                        {"step": "merge", "status": "failed", "detail": e},
                    ],
                })
            );
        }
        eprintln!("{e}");
        return 1;
    }
    // Merged-elsewhere / retry safety: a train already inside dev needs no
    // new merge commit. Verify (same ancestor check `release finish` uses)
    // and continue to push → closes → cleanup.
    let already_merged = train_merged_into_dev(&target);
    let (dev_short, merge_status) = if already_merged {
        (land_short(&train_head("dev")), "already-merged")
    } else {
        match land_merge_train(&target, &merge_msg) {
            Ok(s) => (s, "ok"),
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "train": target,
                            "dry_run": false,
                            "steps": [
                                {"step": "preflight", "status": "ok"},
                                {"step": "merge", "status": "failed", "detail": e},
                            ],
                        })
                    );
                }
                eprintln!("{e}");
                return 1;
            }
        }
    };
    if let Err(e) = land_push_dev() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "train": target,
                    "dry_run": false,
                    "merge": dev_short,
                    "steps": [
                        {"step": "preflight", "status": "ok"},
                        {"step": "merge", "status": merge_status, "detail": dev_short},
                        {"step": "push", "status": "failed", "detail": e},
                    ],
                })
            );
        }
        eprintln!("{e}");
        return 1;
    }
    let mut closed = Vec::new();
    let mut skipped = Vec::new();
    for t in &tickets {
        match land_close_ticket(*t, &note) {
            Ok(true) => closed.push(*t),
            Ok(false) => skipped.push(*t),
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "train": target,
                            "dry_run": false,
                            "merge": dev_short,
                            "pushed": true,
                            "closed": closed,
                            "skipped": skipped,
                            "steps": [
                                {"step": "preflight", "status": "ok"},
                                {"step": "merge", "status": merge_status, "detail": dev_short},
                                {"step": "push", "status": "ok"},
                                {"step": "closes", "status": "failed", "detail": e, "closed": closed, "skipped": skipped},
                            ],
                        })
                    );
                }
                eprintln!("{e}");
                return 1;
            }
        }
    }
    // Same state path `release finish` uses: drop the train worktree,
    // close the train record, print cleanup (never execute it). A named
    // --train that is not the open train leaves the open record alone.
    remove_train_worktree(&state);
    if open.as_ref().map(|t| t.branch.as_str()) == Some(target.as_str()) {
        clear_train(&state);
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "train": target,
                "dry_run": false,
                "merge": dev_short,
                "pushed": true,
                "closed": closed,
                "skipped": skipped,
                "cleanup": cleanup,
                "steps": [
                    {"step": "preflight", "status": "ok"},
                    {"step": "merge", "status": merge_status, "detail": dev_short},
                    {"step": "push", "status": "ok"},
                    {"step": "closes", "status": "ok", "closed": closed, "skipped": skipped},
                    {"step": "cleanup", "status": "ok", "cleanup": cleanup},
                ],
            })
        );
    } else if already_merged {
        println!("land: {target} already merged into dev ({dev_short}) — verified merged elsewhere, skipping merge");
        println!("land: pushed dev to origin");
        for t in &closed {
            println!("land: closed #{t}");
        }
        for t in &skipped {
            println!("land: #{t} already closed, skipping");
        }
        println!("land: train {target} landed into dev");
        println!("cleanup (human runs these, automation never deletes):");
        for line in &cleanup {
            println!("  {line}");
        }
    } else {
        println!("land: merged {target} into dev ({dev_short})");
        println!("land: pushed dev to origin");
        for t in &closed {
            println!("land: closed #{t}");
        }
        for t in &skipped {
            println!("land: #{t} already closed, skipping");
        }
        println!("land: train {target} landed into dev");
        println!("cleanup (human runs these, automation never deletes):");
        for line in &cleanup {
            println!("  {line}");
        }
    }
    0
}

/// Canonical sha256 of an issue body: exactly one trailing newline, matching
/// the bash parity surface (`printf '%s\n' "$body" | sha256sum`, where the
/// shell has already stripped trailing newlines via command substitution).
fn body_hash(body: &str) -> String {
    let canonical = format!("{}\n", body.trim_end_matches('\n'));
    let digest = Sha256::digest(canonical.as_bytes());
    format!("{digest:x}")
}

fn parse_issue_number(raw: &str) -> Result<u64, String> {
    raw.trim()
        .strip_prefix('#')
        .unwrap_or(raw.trim())
        .parse::<u64>()
        .map_err(|_| format!("invalid issue number: {raw}"))
}

/// Run `gh`, returning filtered stdout. Lines starting with `mise ` are
/// version-manager noise on stdout (the bash shim filters them too); real
/// errors still surface via the exit status on stderr.
fn run_gh(args: &[&str]) -> Result<String, String> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run gh: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh {} failed: {}", args.join(" "), stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let filtered: Vec<&str> = stdout.lines().filter(|l| !l.starts_with("mise ")).collect();
    Ok(filtered.join("\n"))
}

struct IssueView {
    number: u64,
    title: String,
    body: String,
    state: String,
}

fn fetch_issue(number: u64) -> Result<IssueView, String> {
    let out = run_gh(&[
        "issue",
        "view",
        &number.to_string(),
        "--json",
        "number,title,body,state",
    ])?;
    let v: serde_json::Value =
        serde_json::from_str(&out).map_err(|e| format!("gh returned invalid JSON: {e}"))?;
    Ok(IssueView {
        number: v.get("number").and_then(|n| n.as_u64()).unwrap_or(number),
        title: v
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string(),
        body: v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_string(),
        state: v
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// Latest SPEC-APPROVED hash from the issue comments, if any. Mirrors the
/// bash `capture("SPEC-APPROVED `?(?<h>[0-9a-f]{64})`?") | last` semantics.
fn fetch_approved(number: u64) -> Result<Option<String>, String> {
    let out = run_gh(&["issue", "view", &number.to_string(), "--json", "comments"])?;
    let v: serde_json::Value =
        serde_json::from_str(&out).map_err(|e| format!("gh returned invalid JSON: {e}"))?;
    let comments = v
        .get("comments")
        .and_then(|c| c.as_array())
        .ok_or_else(|| "gh comments JSON has no comments array".to_string())?;
    let mut approved: Option<String> = None;
    for comment in comments {
        if let Some(body) = comment.get("body").and_then(|b| b.as_str()) {
            if let Some(hash) = extract_approved_hash(body) {
                approved = Some(hash);
            }
        }
    }
    Ok(approved)
}

/// First 64-hex token at/after the SPEC-APPROVED marker in one comment body.
fn extract_approved_hash(body: &str) -> Option<String> {
    let marker = body.find("SPEC-APPROVED")?;
    let after = &body[marker + "SPEC-APPROVED".len()..];
    let bytes = after.as_bytes();
    let mut i = 0;
    while i + 64 <= bytes.len() {
        let window = &after[i..i + 64];
        if window.bytes().all(|b| b.is_ascii_hexdigit())
            && (i + 64 == bytes.len() || !bytes[i + 64].is_ascii_hexdigit())
            && (i == 0 || !bytes[i - 1].is_ascii_hexdigit())
        {
            return Some(window.to_ascii_lowercase());
        }
        i += 1;
    }
    None
}

fn cache_path(number: u64) -> String {
    format!("factory/.cache/SPEC-{number}.md")
}

fn write_cache(view: &IssueView, hash: &str) -> Result<String, String> {
    let path = cache_path(view.number);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create cache dir: {e}"))?;
    }
    let canonical_body = format!("{}\n", view.body.trim_end_matches('\n'));
    let content = format!(
        "<!-- factory cache: issue #{} hash={} — DO NOT EDIT, regenerate with: factory sync {} -->\n# SPEC-{}: {}\n\n- Issue: #{} (authoritative; this file is a local cache)\n- State: {} | body-sha256: `{}`\n\n{}",
        view.number,
        hash,
        view.number,
        view.number,
        view.title,
        view.number,
        view.state,
        hash,
        canonical_body
    );
    std::fs::write(&path, content).map_err(|e| format!("failed to write {path}: {e}"))?;
    Ok(path)
}

/// Previous cached (hash, body), if a cache file exists. The body is
/// everything after the blank line terminating the header (the `- State:`
/// line); older short headers fall back to everything after the first
/// blank line.
fn read_cache(number: u64) -> Option<(String, String)> {
    let content = std::fs::read_to_string(cache_path(number)).ok()?;
    let header = content.lines().next().unwrap_or("");
    let hash = header
        .split("hash=")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("")
        .to_string();
    let all: Vec<&str> = content.lines().collect();
    let state_idx = all.iter().position(|l| l.starts_with("- State:"));
    let body_start = match state_idx {
        Some(i) => i + 2,
        None => all.iter().position(|l| l.is_empty()).map_or(0, |i| i + 1),
    };
    let body = if body_start < all.len() {
        all[body_start..].join("\n")
    } else {
        String::new()
    };
    Some((hash, body))
}

/// Changed lines between two bodies (`-` old / `+` new), no context.
fn diff_change_lines(old: &str, new: &str) -> Vec<String> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf]
    {
        suf += 1;
    }
    let mut out = Vec::new();
    for line in &a[pre..a.len() - suf] {
        out.push(format!("-{line}"));
    }
    for line in &b[pre..b.len() - suf] {
        out.push(format!("+{line}"));
    }
    out
}

/// Human-readable drift report: both hashes plus what changed, never just
/// "mismatch".
fn drift_report(
    number: u64,
    current_hash: &str,
    approved: Option<&str>,
    previous_body: Option<&str>,
    current_body: &str,
) -> String {
    let mut report = format!(
        "freeze MISMATCH: #{number} body={current_hash} approved={} — re-freeze required",
        approved.unwrap_or("none")
    );
    report.push_str("\n--- previous\n+++ current");
    match previous_body {
        Some(old) => {
            let mut changes = diff_change_lines(old, current_body);
            if changes.is_empty() {
                changes.push("(bodies differ only in trailing whitespace)".to_string());
            }
            if changes.len() > 60 {
                changes.truncate(60);
                changes.push("... (truncated)".to_string());
            }
            for line in changes {
                report.push('\n');
                report.push_str(&line);
            }
        }
        None => {
            report.push_str("\n(no local cache baseline; showing current body)");
            for line in current_body.lines().take(20) {
                report.push('\n');
                report.push_str(line);
            }
        }
    }
    report
}

pub fn cmd_freeze(json: bool, issue_raw: &str, label: &str) -> i32 {
    let number = match parse_issue_number(issue_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("factory freeze: {e}");
            return 1;
        }
    };
    let view = match fetch_issue(number) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("factory freeze: {e}");
            return 1;
        }
    };
    let hash = body_hash(&view.body);
    let comment = format!(
        "SPEC-APPROVED `{hash}`\n\nApproval-time snapshot hash of the issue body. If the body changes after this comment, the hash no longer matches and the spec must be re-frozen before review. Verify with: `factory sync {number}` (compares cached hash)."
    );
    if let Err(e) = run_gh(&["issue", "comment", &number.to_string(), "--body", &comment]) {
        eprintln!("factory freeze: {e}");
        return 1;
    }
    if let Err(e) = run_gh(&["issue", "edit", &number.to_string(), "--add-label", label]) {
        eprintln!("factory freeze: {e}");
        return 1;
    }
    if json {
        println!(
            "{}",
            serde_json::to_string(&FreezeOutput {
                issue: number,
                label,
                hash: &hash,
            })
            .expect("freeze JSON serialization")
        );
    } else {
        println!("frozen: issue #{number} label={label} hash={hash}");
    }
    0
}

pub fn cmd_sync(json: bool, issue_raw: &str) -> i32 {
    let number = match parse_issue_number(issue_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("factory sync: {e}");
            return 1;
        }
    };
    let view = match fetch_issue(number) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("factory sync: {e}");
            return 1;
        }
    };
    let hash = body_hash(&view.body);
    let approved = match fetch_approved(number) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("factory sync: {e}");
            return 1;
        }
    };
    let previous = read_cache(number);
    let cache = match write_cache(&view, &hash) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("factory sync: {e}");
            return 1;
        }
    };
    let ok = approved.as_deref() == Some(hash.as_str());
    if json {
        println!(
            "{}",
            serde_json::to_string(&SyncOutput {
                issue: number,
                hash: hash.clone(),
                approved: approved.clone(),
                ok,
                cache,
            })
            .expect("sync JSON serialization")
        );
    } else {
        println!("cached: {cache} (hash={hash})");
    }
    if ok {
        if !json {
            println!("freeze OK: #{number} hash={hash}");
        }
        0
    } else {
        let previous_body = previous.as_ref().map(|(_, b)| b.as_str());
        eprintln!(
            "{}",
            drift_report(
                number,
                &hash,
                approved.as_deref(),
                previous_body,
                &view.body
            )
        );
        1
    }
}

pub fn cmd_verify_freeze(json: bool, issue_raw: &str) -> i32 {
    let number = match parse_issue_number(issue_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("factory verify-freeze: {e}");
            return 1;
        }
    };
    let view = match fetch_issue(number) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("factory verify-freeze: {e}");
            return 1;
        }
    };
    let hash = body_hash(&view.body);
    let approved = match fetch_approved(number) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("factory verify-freeze: {e}");
            return 1;
        }
    };
    let ok = approved.as_deref() == Some(hash.as_str());
    if json {
        println!(
            "{}",
            serde_json::to_string(&VerifyOutput {
                issue: number,
                hash: hash.clone(),
                approved: approved.clone(),
                ok,
            })
            .expect("verify-freeze JSON serialization")
        );
    } else if ok {
        println!("freeze OK: #{number} hash={hash}");
    }
    if ok {
        0
    } else {
        let previous_body = read_cache(number).map(|(_, b)| b);
        eprintln!(
            "{}",
            drift_report(
                number,
                &hash,
                approved.as_deref(),
                previous_body.as_deref(),
                &view.body
            )
        );
        1
    }
}

#[cfg(test)]
mod guided_tests {
    use super::parse_choice_line;

    #[test]
    fn empty_line_keeps_default() {
        assert_eq!(parse_choice_line("", 11, 8), 8);
        assert_eq!(parse_choice_line("\n", 11, 8), 8);
        assert_eq!(parse_choice_line("   \n", 11, 0), 0);
    }

    #[test]
    fn valid_number_selects_one_based() {
        assert_eq!(parse_choice_line("1\n", 11, 8), 0);
        assert_eq!(parse_choice_line("9", 11, 8), 8);
        assert_eq!(parse_choice_line("11\n", 11, 8), 10);
    }

    #[test]
    fn out_of_range_and_garbage_fall_back_to_default() {
        assert_eq!(parse_choice_line("0\n", 11, 8), 8);
        assert_eq!(parse_choice_line("12\n", 11, 8), 8);
        assert_eq!(parse_choice_line("99\n", 11, 8), 8);
        assert_eq!(parse_choice_line("rust\n", 11, 8), 8);
        assert_eq!(parse_choice_line("-1\n", 11, 8), 8);
    }
}

// --- outcome figures, TDD red (ticket #57, spec #56) ------------------------
// Fixture histories pinning the exact per-ticket figures. These tests fail
// until the pure `outcome_figures` computation below them exists.
#[cfg(test)]
mod outcome_tests {
    use super::{
        is_autonomous, outcome_figures, outcome_human_line, OutcomeAttemptInput,
        OutcomeHistoryInput, OutcomeSignal, OUTCOME_AUTONOMY_CAVEAT, OUTCOME_COST_TEXT,
    };

    fn attempt(
        no: u64,
        started: Option<i64>,
        finished: Option<i64>,
        accepted: bool,
    ) -> OutcomeAttemptInput {
        OutcomeAttemptInput {
            attempt_no: no,
            started_epoch_secs: started,
            finished_epoch_secs: finished,
            review_accepted: accepted,
        }
    }

    fn history(
        ticket: u64,
        landed: Option<i64>,
        attempts: Vec<OutcomeAttemptInput>,
        signals: Vec<OutcomeSignal>,
    ) -> OutcomeHistoryInput {
        OutcomeHistoryInput {
            ticket,
            signal_epoch_secs: 0,
            landed_epoch_secs: landed,
            attempts,
            signals,
        }
    }

    #[test]
    fn single_attempt_accept_pins_exact_figures() {
        let figures = outcome_figures(&history(
            7,
            Some(259_200),
            vec![attempt(1, Some(0), Some(2_700), true)],
            vec![],
        ));
        assert_eq!(figures.ticket, 7);
        assert_eq!(figures.cycle_days, Some(3.0));
        assert_eq!(figures.attempt_minutes.len(), 1);
        assert_eq!(figures.attempt_minutes[0].minutes, Some(45.0));
        assert_eq!(figures.total_attempt_minutes, Some(45.0));
        assert!(figures.autonomous);
        assert!(is_autonomous(&history(
            7,
            Some(259_200),
            vec![attempt(1, Some(0), Some(2_700), true)],
            vec![],
        )));
        assert_eq!(figures.autonomy_verdict, "autonomous");
        assert_eq!(figures.autonomy_caveat, OUTCOME_AUTONOMY_CAVEAT);
        assert_eq!(figures.cost_text, OUTCOME_COST_TEXT);
    }

    #[test]
    fn multi_attempt_is_not_autonomous_but_pins_totals() {
        let figures = outcome_figures(&history(
            8,
            Some(172_800),
            vec![
                attempt(1, Some(0), Some(1_800), false),
                attempt(2, Some(3_600), Some(7_200), true),
            ],
            vec![],
        ));
        assert_eq!(figures.cycle_days, Some(2.0));
        assert_eq!(figures.attempt_minutes.len(), 2);
        assert_eq!(figures.attempt_minutes[0].minutes, Some(30.0));
        assert_eq!(figures.attempt_minutes[1].minutes, Some(60.0));
        assert_eq!(figures.total_attempt_minutes, Some(90.0));
        assert!(!figures.autonomous);
        assert_eq!(figures.autonomy_verdict, "not autonomous");
    }

    #[test]
    fn clarification_signal_present_is_not_autonomous() {
        let input = history(
            9,
            Some(86_400),
            vec![attempt(1, Some(0), Some(1_200), true)],
            vec![OutcomeSignal::Clarification],
        );
        let figures = outcome_figures(&input);
        assert_eq!(figures.cycle_days, Some(1.0));
        assert_eq!(figures.total_attempt_minutes, Some(20.0));
        assert!(!is_autonomous(&input));
        assert!(!figures.autonomous);
    }

    #[test]
    fn blocked_signal_present_is_not_autonomous() {
        let input = history(
            10,
            Some(86_400),
            vec![attempt(1, Some(0), Some(600), true)],
            vec![OutcomeSignal::Blocked],
        );
        assert!(!is_autonomous(&input));
        assert!(!outcome_figures(&input).autonomous);
    }

    #[test]
    fn missing_land_merge_blanks_cycle_but_keeps_minutes() {
        let figures = outcome_figures(&history(
            11,
            None,
            vec![attempt(1, Some(0), Some(900), true)],
            vec![],
        ));
        assert_eq!(figures.cycle_days, None);
        assert_eq!(figures.total_attempt_minutes, Some(15.0));
    }

    #[test]
    fn missing_logs_blank_minutes_never_guess() {
        let figures = outcome_figures(&history(
            12,
            Some(86_400),
            vec![attempt(1, None, None, false)],
            vec![],
        ));
        assert_eq!(figures.cycle_days, Some(1.0));
        assert_eq!(figures.attempt_minutes[0].minutes, None);
        assert_eq!(figures.total_attempt_minutes, None);
        assert!(!figures.autonomous);
    }

    #[test]
    fn human_line_carries_verdict_caveat_and_cost() {
        let figures = outcome_figures(&history(
            7,
            Some(259_200),
            vec![attempt(1, Some(0), Some(2_700), true)],
            vec![],
        ));
        let line = outcome_human_line(&figures);
        assert!(
            line.contains("autonomous"),
            "line names the verdict: {line}"
        );
        assert!(
            line.contains(OUTCOME_AUTONOMY_CAVEAT),
            "line carries the caveat: {line}"
        );
        assert!(line.contains(OUTCOME_COST_TEXT), "line states cost: {line}");
    }

    #[test]
    fn computation_reads_no_environment_or_filesystem() {
        // Purity probe: point every seam override at garbage. A pure
        // computation over its arguments cannot notice.
        std::env::set_var("FACTORY_STATE_DIR", "/nonexistent-t57-purity-probe");
        std::env::set_var("FACTORY_GH_BIN", "/nonexistent-t57-purity-probe");
        let input = history(
            7,
            Some(259_200),
            vec![attempt(1, Some(0), Some(2_700), true)],
            vec![],
        );
        let figures = outcome_figures(&input);
        assert_eq!(figures.cycle_days, Some(3.0));
        assert_eq!(figures.total_attempt_minutes, Some(45.0));
        assert!(figures.autonomous);
        std::env::remove_var("FACTORY_STATE_DIR");
        std::env::remove_var("FACTORY_GH_BIN");
    }
}
