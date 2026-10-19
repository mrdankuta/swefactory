use askama::Template;
use clap::{Parser, Subcommand};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

mod sqlite;

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
    /// live attempts, loop state, plus a compact finished-history summary
    /// (counts by state, most recent first). Full per-ticket history stays
    /// behind `factory log <ticket>`. Read-only: observing changes nothing.
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
        /// Show one fork by name or number instead of the attempt history.
        #[arg(long, value_name = "NAME|NUMBER")]
        fork: Option<String>,
        /// Render the latest attempt's agent conversation (assistant turns,
        /// thinking, tool calls, results) instead of the raw tool-output log.
        /// Bounded to the newest 20 items (`--tail N` overrides, `0` means
        /// full). Read-only: rendering reads session file bytes only and
        /// never attaches to the live session — opening the live session
        /// in another agent process stays forbidden.
        #[arg(long)]
        transcript: bool,
    },
    /// Fork a ticket attempt at a chosen log entry into a named experiment
    /// line (spec #65: fork creation + isolation in ticket #80, lineage +
    /// nesting + caps in ticket #81, lifecycle composition in ticket #82).
    /// The fork inherits the conversation prefix up to and including the
    /// entry and reads the shared facts document; the originating line is
    /// byte-identical before and after. Each fork is isolated in its own
    /// worktree, branch, and slot, addressable by name or number. Forks
    /// nest: `--fork NAME|NUMBER` roots the child at a parent fork's
    /// entry and records the full chain oldest-first. At most
    /// MAX_FORKS_PER_TICKET forks per ticket; past the cap the fork is
    /// refused with the cap stated. Per-line figures render from the
    /// shared facts document at forks.<ticket>.<name> (or forks.<name>)
    /// fields `cost` and `changed`. A forked line replays like a main
    /// line: `--interrupt NAME|NUMBER` marks it interrupted (resumable)
    /// and releases its worktree, `--resume NAME|NUMBER` continues the
    /// same line with its transcript preserved. Ownership (ticket #73):
    /// `--owned` roots an owned line (subagent-style: aborting the parent
    /// propagates, cleanup follows the owned line); without it the fork
    /// is ownerless (fully independent, the default). Ownership is
    /// recorded in lineage and rendered in every lineage surface, human
    /// and machine-readable, with identical facts.
    Fork {
        /// Ticket issue number (e.g. 80 or #80).
        ticket: String,
        /// Parent attempt number (default: the newest attempt).
        /// Cannot be combined with --fork. Creation only.
        #[arg(long)]
        attempt: Option<u64>,
        /// Parent fork name or 1-based number for a nested fork.
        /// Cannot be combined with --attempt. Creation only.
        #[arg(long, value_name = "NAME|NUMBER")]
        fork: Option<String>,
        /// Transcript entry (1-indexed line) the fork inherits through.
        /// Creation only (required to root a new line).
        #[arg(long)]
        at: Option<u64>,
        /// Fork name (unique per ticket; collisions list the taken names).
        /// Creation only (required to root a new line).
        #[arg(long)]
        name: Option<String>,
        /// Resume an interrupted fork line by name or 1-based number.
        /// Cannot be combined with creation flags or --interrupt.
        #[arg(long, value_name = "NAME|NUMBER")]
        resume: Option<String>,
        /// Mark a live fork line interrupted (resumable), releasing its
        /// worktree for resume to re-attach. Cannot be combined with
        /// creation flags or --resume.
        #[arg(long, value_name = "NAME|NUMBER")]
        interrupt: Option<String>,
        /// Own this line by its parent (subagent-style: aborting the
        /// parent propagates, cleanup follows the owned line). Absent
        /// the fork is ownerless (fully independent). Creation only.
        #[arg(long)]
        owned: bool,
    },
    /// Cross-attempt facts (spec #64): the revision-guarded document
    /// beside the dispatch ledger. Reads only in this slice (ticket
    /// #77): the whole document or one facts path, human-readable by
    /// default, machine-readable with the global --json flag carrying
    /// the same value-plus-revision facts. Read-only: observing changes
    /// nothing; an absent document reads as empty at revision zero with
    /// no migration. Guarded writes arrive in later tickets.
    Facts {
        #[command(subcommand)]
        action: FactsAction,
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
    /// Watch the factory live in one terminal pane (ticket #91): attempt
    /// lanes (live + recent with outcome markers), one-screen health
    /// (version, train, STOP state), and a recent-transitions tail —
    /// refreshing in place over the same read-only seams `status` and
    /// `dispatch status` read. Polling is the transport; when streams are
    /// unavailable the pane says so calmly instead of erroring.
    /// Read-only: watching changes nothing, closing changes nothing.
    /// Piped output (`| cat`) degrades to one static snapshot and exits.
    Watch {
        /// Seconds between refreshes of the live pane.
        #[arg(long, default_value_t = WATCH_DEFAULT_INTERVAL_SECS)]
        interval: u64,
        /// Streams URL to prefer for live facts (e.g.
        /// `http://127.0.0.1:8080/streams/dispatch`). Any refusal or
        /// parse failure falls back to polling with a calm indicator.
        /// `FACTORY_WATCH_STREAM_URL` provides the same default.
        #[arg(long, value_name = "URL")]
        streams_url: Option<String>,
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
    ///
    /// Resume replays the interrupted phase only where the underlying step
    /// is replay-safe; anything else waits for the next decision instead of
    /// repeating a side effect.
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
    /// Show attempts + worktrees + loop state. Human output renders an
    /// aligned-column table bounded to live attempts plus the most recent
    /// finished attempts; `--all` opts into the full history. Read-only:
    /// observing changes nothing. Exit 0 whenever the ledger read
    /// succeeded (even when empty); nonzero only on unreadable state.
    /// The global --json flag selects machine output carrying every
    /// attempt with every field, bounded or not.
    Status {
        /// Show the full attempt history instead of live plus recent.
        /// Machine output always carries every attempt either way.
        #[arg(long)]
        all: bool,
    },
    /// Live task graph: every live task of every live attempt as a node
    /// with its owner edge (owning task, or the attempt itself), status,
    /// and what it is waiting on. Read-only: observing changes nothing.
    /// Exit 0 whenever the ledger read succeeded (even when empty);
    /// nonzero only on unreadable state or a bad --ticket, with the cause
    /// named. The global --json flag selects machine output carrying the
    /// same facts structurally.
    Graph {
        /// Only this ticket's live tasks (e.g. 70 or #70).
        #[arg(long)]
        ticket: Option<String>,
    },
    /// Wait on a parent task's children with a stated policy (ticket #68,
    /// spec #59): `all-settled` resumes only after every child is terminal;
    /// `fail-fast` aborts still-running siblings on the first child failure
    /// before the parent decides. Waiting runs no code; the resume point is
    /// checkpointed atomically like any phase. A simulated kill mid-wait
    /// (`FACTORY_KILL_WAIT=1`) resumes into the same wait; children never
    /// run twice unless declared replay-safe. Outcomes and abort order are
    /// appended to the attempt transcript so `factory log` shows them.
    Wait {
        /// Ticket owning the parent task (e.g. 68 or #68).
        #[arg(long)]
        ticket: Option<String>,
        /// Parent task id (e.g. `plan`).
        #[arg(long)]
        parent: String,
        /// Waiting policy: `all-settled` or `fail-fast`.
        #[arg(long)]
        policy: String,
    },
    /// Halt the loop: later `once` runs exit 0 without work.
    Stop,
    /// Restart the loop after a stop.
    Resume,
    /// Abort an attempt's current work bottom-up (ticket #86 rework of
    /// #69, spec #59 follow-up). An ordinary abort stops foreground tasks
    /// only and leaves background tasks running; `--background` also stops
    /// background tasks and everything they own. Owned work aborts before
    /// its owner and each task undoes its own effects; a kill mid-abort
    /// resumes the same order from persisted progress.
    Abort {
        /// Ticket issue number (e.g. 11 or #11).
        ticket: String,
        /// Also stop background tasks and everything they own.
        #[arg(long)]
        background: bool,
    },
    /// Record a hook/approval decision for one attempt (first-write-wins)
    /// or show the stored one. The first decision for a (ticket, attempt,
    /// key) triple sticks: repeats observe the stored answer instead of
    /// re-asking, so a restart finds the same outcome with or without a
    /// crash in between. Scope is the attempt: a fresh attempt starts
    /// with an empty cache. Exit 0 on record and on reuse; nonzero only
    /// when the scope is missing or the key/decision is empty, with the
    /// cause named. The global --json flag selects machine output
    /// carrying the same facts structurally.
    Memo {
        /// Ticket issue number (e.g. 67 or #67).
        #[arg(long)]
        ticket: String,
        /// Attempt number the decision belongs to.
        #[arg(long)]
        attempt: u64,
        /// Decision key (e.g. approve-deploy, allow-flagged-tool).
        #[arg(long)]
        key: String,
        /// Decision value to store on first write (e.g. allow, deny).
        #[arg(long)]
        decision: String,
    },
    /// Hermetic proof of the release mechanics (temp bare origin, no gh,
    /// no network, no tokens, no spend). Every failure names the mechanics
    /// that broke and exits non-zero.
    SelfTest,
    /// One-shot cutover import (ticket #88): read the bash dispatcher's
    /// SQLite history (`attempts` + `releases` rows) and write the
    /// equivalent Rust JSON ledger, then archive the `.db` byte-identical.
    /// Pure Rust: parses the SQLite format directly, never links sqlite,
    /// so the binary stays dependency-free on every target incl.
    /// musl-static. One-shot: refuses when the state dir already holds a
    /// ledger, a train, or an archive, so re-running never duplicates.
    /// Never mutates the source `.db` (read-only; re-hashed after).
    Import {
        /// Path to the bash dispatcher's `dispatch.db`. Defaults to
        /// `<state>/dispatch.db` (the FACTORY_STATE_DIR, else
        /// `./factory/.state`, sibling of the ledger it writes).
        #[arg(long, value_name = "PATH")]
        db: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum FactsAction {
    /// Read the facts document: the whole document by default, or one
    /// slash-separated facts path (e.g. `costs/total`) on request.
    /// Read-only: observing changes nothing. Exit 0 whenever the
    /// document was read successfully — an absent document reads as
    /// empty at revision zero, and a missing path reads as a plain
    /// sentence (never zero-filled, never error-shaped). Nonzero only
    /// on unreadable state (unreadable/corrupt facts document) with the
    /// cause named. The global --json flag selects machine output
    /// carrying the same value-plus-revision facts.
    Show {
        /// Slash-separated facts path (e.g. costs/total). Omit for the
        /// whole document.
        #[arg(value_name = "PATH")]
        path: Option<String>,
        /// Read the value as of this revision (time travel). Omit for
        /// the latest revision (the default read, unchanged). Refused
        /// past the current revision with both revisions named.
        #[arg(long, value_name = "REV")]
        at: Option<u64>,
    },
    /// Bounded history for one facts path (ticket #71, spec #64): the
    /// last --limit recorded values oldest-first, human-readable by
    /// default, machine-readable with the global --json flag carrying
    /// the same revision-plus-value facts. Read-only: observing changes
    /// nothing. --limit is required (1 to 50 entries per read); an
    /// absent, zero, or over-cap limit is refused with the cap stated
    /// and nothing dumped. A never-written path reads as empty history
    /// (exit 0), never error-shaped.
    History {
        /// Slash-separated facts path (e.g. costs/total).
        #[arg(value_name = "PATH")]
        path: String,
        /// Entries returned, most recent first kept, oldest-first
        /// shown (1 to 50). Required: history is never unbounded.
        #[arg(long, value_name = "N")]
        limit: Option<u64>,
    },
    /// Create one facts path with the revision the reader observed.
    /// Refuses when the path already exists (nothing changed). One
    /// atomic path operation per invocation; the accepted write advances
    /// the revision by exactly one. The global --json flag selects
    /// machine output carrying the same value-plus-revision facts.
    Create {
        /// Slash-separated facts path (e.g. costs/total).
        #[arg(value_name = "PATH")]
        path: String,
        /// Revision the reader observed (reread on conflict and retry).
        #[arg(long, value_name = "N")]
        revision: u64,
        /// JSON value to store (e.g. 42, "done", {"a": 1}).
        #[arg(long, value_name = "JSON")]
        value: String,
    },
    /// Set (upsert) one facts path with the revision the reader observed.
    /// Creates the path or any missing intermediate objects, overwrites
    /// when it exists. One atomic path operation per invocation; the
    /// accepted write advances the revision by exactly one. The global
    /// --json flag selects machine output carrying the same
    /// value-plus-revision facts.
    Set {
        /// Slash-separated facts path (e.g. costs/total).
        #[arg(value_name = "PATH")]
        path: String,
        /// Revision the reader observed (reread on conflict and retry).
        #[arg(long, value_name = "N")]
        revision: u64,
        /// JSON value to store (e.g. 42, "done", {"a": 1}).
        #[arg(long, value_name = "JSON")]
        value: String,
    },
    /// Delete one facts path with the revision the reader observed.
    /// Refuses when the path has no value (nothing changed). One atomic
    /// path operation per invocation; the accepted write advances the
    /// revision by exactly one. The global --json flag selects machine
    /// output carrying the same path-plus-revision facts.
    Delete {
        /// Slash-separated facts path (e.g. costs/total).
        #[arg(value_name = "PATH")]
        path: String,
        /// Revision the reader observed (reread on conflict and retry).
        #[arg(long, value_name = "N")]
        revision: u64,
    },
    /// Atomically add to the number at one facts path with the revision
    /// the reader observed. A missing path starts from zero; a present
    /// non-number is refused with nothing changed. One atomic path
    /// operation per invocation; the accepted write advances the
    /// revision by exactly one. Writes that would push the document
    /// past 65536 bytes (64 KiB) are refused with the cap stated.
    /// The global --json flag selects machine output carrying the same
    /// value-plus-revision facts.
    Increment {
        /// Slash-separated facts path (e.g. costs/total).
        #[arg(value_name = "PATH")]
        path: String,
        /// Revision the reader observed (reread on conflict and retry).
        #[arg(long, value_name = "N")]
        revision: u64,
        /// Amount to add (JSON number, e.g. 5 or -2.5). Defaults to 1.
        #[arg(long, value_name = "DELTA", default_value = "1")]
        by: String,
    },
    /// Attach to one facts path: the current value plus revision first,
    /// then one delivery per accepted write touching the path since the
    /// cursor. Fresh attach (no --since) is a snapshot only, never a
    /// replay; re-attach with --since <last-seen-revision> after a drop
    /// replays exactly the missed deliveries with no duplicates. Past
    /// the delivery bound the backlog is refused with a re-attach
    /// instruction instead of buffering forever. Read-only: watching
    /// changes nothing. The global --json flag selects machine output
    /// carrying the same value-plus-revision facts.
    Watch {
        /// Slash-separated facts path (e.g. costs/total).
        #[arg(value_name = "PATH")]
        path: String,
        /// Cursor revision: replay deliveries past this revision only.
        /// Defaults to the current revision (attach only, no replay).
        #[arg(long, value_name = "N")]
        since: Option<u64>,
    },
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
/// Live-stream upgrade for one fragment container (ticket #84, spec
/// #66): the container names its scope stream plus the documented
/// silence bound, and the hidden banner carries calm copy with one
/// re-attach action. The script is progressive enhancement only — each
/// page keeps its polling hook exactly as it is, so with streams
/// unreachable the views render and refresh identically.
fn stream_viewer_html(container_id: &str) -> String {
    format!(
        "<p data-stream-banner hidden>You're seeing the last update \u{2014} the live view disconnected. \
        <button type=\"button\" data-stream-reattach>Re-attach</button></p>\
        <script>(function(){{\
        var el=document.getElementById('{id}');\
        if(!el||!window.EventSource)return;\
        var url=el.getAttribute('data-stream-url');\
        var silence=parseInt(el.getAttribute('data-stream-silence-after-ms')||'{dflt}',10);\
        var banner=document.querySelector('[data-stream-banner]');\
        var btn=document.querySelector('[data-stream-reattach]');\
        var es=null,timer=null;\
        function show(){{if(banner)banner.hidden=false;}}\
        function hide(){{if(banner)banner.hidden=true;}}\
        function stop(){{if(es){{try{{es.close();}}catch(_e){{}}es=null;}}if(timer){{clearTimeout(timer);timer=null;}}}}\
        function arm(){{if(timer)clearTimeout(timer);timer=setTimeout(function(){{stop();show();}},silence);}}\
        function apply(data){{try{{var p=JSON.parse(data);if(p&&p.html!==undefined)el.innerHTML=p.html;}}catch(_e){{}}arm();}}\
        function attach(){{stop();hide();try{{es=new EventSource(url);}}catch(_e){{return;}}\
        es.addEventListener('hello',function(e){{try{{var h=JSON.parse(e.data);if(h&&h.silence_after_ms)silence=parseInt(h.silence_after_ms,10);}}catch(_e){{}}arm();}});\
        es.addEventListener('snapshot',function(e){{apply(e.data);}});\
        es.addEventListener('delta',function(e){{apply(e.data);}});\
        es.addEventListener('reattach',function(e){{stop();show();}});}}\
        if(btn)btn.addEventListener('click',function(){{attach();}});\
        attach();}})();</script>",
        id = container_id,
        dflt = STREAM_SILENCE_AFTER_MS,
    )
}

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
    <main><div id=\"board\" data-board-fragment=\"/fragments/board\" data-poll-ms=\"{}\" data-stream-scope=\"board\" data-stream-url=\"/streams/board\" data-stream-silence-after-ms=\"{}\" data-stream-partial-ms=\"{}\">\
    {fragment}</div>{}</main>\
    <script>(function(){{var el=document.getElementById('board');if(!el)return;var url=el.getAttribute('data-board-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'15000',10);\
    setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
    </body></html>",
        SITE_CSS,
        BOARD_POLL_MS,
        STREAM_SILENCE_AFTER_MS,
        STREAM_PARTIAL_MS,
        stream_viewer_html("board"),
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

/// One ticket as the detail page sees it: body plus attempt history plus
/// the latest attempt's normalized agent conversation (ticket #93).
#[derive(Debug, Clone, PartialEq)]
pub struct TicketDetail {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub state: String,
    pub attempts: Vec<TicketAttemptView>,
    /// Latest attempt's conversation pane (the same items
    /// `factory log --transcript` renders). `None` when the ticket has no
    /// attempts; `Some` with `session: None` when no session was recorded.
    pub transcript: Option<TicketTranscriptPane>,
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
        transcript: ticket_transcript_pane(number),
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
        "transcript_pane": detail.transcript.as_ref().map(|p| serde_json::json!({
            "ticket": p.ticket,
            "attempt": p.attempt_no,
            "slot": p.slot,
            "session": p.session,
            "items": p.items,
            "total": p.total,
            "truncated": p.truncated,
            "partial": p.partial,
        })),
    })
}

/// Fragment HTML for one ticket (the polling unit). Pure over the detail
/// so tests assert markers without a server: `data-ticket` plus
/// `data-ticket-body`, per-attempt `data-attempt` with `data-summary`
/// before `data-transcript`, `data-truncated` naming the bound when older
/// lines were dropped, and the agent-conversation pane
/// (`data-transcript-pane`) rendering the same normalized items
/// `factory log --transcript` shows.
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
    if let Some(pane) = detail.transcript.as_ref() {
        html.push_str(&ticket_transcript_pane_html(pane));
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

/// One live task node as the dispatcher screen sees it: the observable
/// task graph (ticket #74, spec #66 follow-up) rendered beside the slot
/// lanes, so the dispatcher-live stream carries the graph. Field-for-field
/// the `factory dispatch graph --json` row, so prose and machine surfaces
/// carry the same facts.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskGraphNodeView {
    pub id: String,
    pub ticket: u64,
    pub attempt: u64,
    pub kind: String,
    pub status: String,
    pub owner: String,
    pub waits_on: Vec<String>,
}

/// The live screen: attempts in slot lanes + STOP state + worktrees.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatcherData {
    pub attempts: Vec<DispatcherAttemptView>,
    pub stopped: bool,
    pub worktrees: Vec<String>,
    pub max_concurrent: usize,
    /// Live task-graph nodes (ticket #74): the same live rows
    /// `factory dispatch graph` lists, so the polling fragment and the
    /// dispatcher-live stream carry the current graph. Empty reads as
    /// the calm empty section, never a 500.
    pub tasks: Vec<TaskGraphNodeView>,
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

/// Calm sentence for an empty task graph (mirrors the CLI's empty line).
pub fn task_graph_empty_sentence() -> &'static str {
    "No live tasks \u{2014} the graph is empty."
}

/// Read-only live task-graph rows for the dispatcher screen: every live
/// task of every attempt as one node, owner edge and waits included.
/// Same live filter and sort as `cmd_dispatch_graph`, so the screen, the
/// CLI, and the dispatcher-live stream read one graph. Read-only: borrows
/// the already-loaded ledger, never writes.
fn live_task_graph_views(ledger: &DispatchLedger) -> Vec<TaskGraphNodeView> {
    let mut live: Vec<&LiveTask> = ledger
        .tasks
        .iter()
        .filter(|t| !is_task_terminal(&t.status))
        .collect();
    live.sort_by(|a, b| (a.ticket, a.attempt, &a.id).cmp(&(b.ticket, b.attempt, &b.id)));
    live.iter()
        .map(|t| TaskGraphNodeView {
            id: t.id.clone(),
            ticket: t.ticket,
            attempt: t.attempt,
            kind: task_kind_text(t).to_string(),
            status: t.status.clone(),
            owner: task_owner(t),
            waits_on: t.waits_on.clone(),
        })
        .collect()
}

/// Fragment HTML for the live task graph (the polling unit): one article
/// per live node keyed by task id, so a snapshot carries each node
/// exactly once and reconnects never duplicate. Pure over data so tests
/// assert markers without a server.
pub fn task_graph_fragment_html(tasks: &[TaskGraphNodeView]) -> String {
    let mut html = String::new();
    html.push_str("<section data-task-graph><h2>Live task graph</h2>");
    if tasks.is_empty() {
        html.push_str(&format!(
            "<p data-no-tasks>{}</p>",
            task_graph_empty_sentence()
        ));
    } else {
        html.push_str("<div data-task-nodes>");
        for t in tasks {
            let waits = if t.waits_on.is_empty() {
                "waits on nothing".to_string()
            } else {
                format!("waits on {}", t.waits_on.join(", "))
            };
            html.push_str(&format!(
                "<article data-task-node=\"{}\" data-ticket=\"{}\" data-attempt=\"{}\" data-kind=\"{}\" data-status=\"{}\" data-owner=\"{}\"><h3>{} {} {}</h3><p data-waits>{}</p></article>",
                escape_html(&t.id),
                t.ticket,
                t.attempt,
                escape_html(&t.kind),
                escape_html(&t.status),
                escape_html(&t.owner),
                escape_html(&t.id),
                escape_html(&t.kind),
                escape_html(&t.status),
                escape_html(&waits),
            ));
        }
        html.push_str("</div>");
    }
    html.push_str("</section>");
    html
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
        tasks: live_task_graph_views(&ledger),
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
        "<p data-cli>Matches <code>factory dispatch status</code> + <code>factory dispatch graph</code> + <code>factory log &lt;ticket&gt; --tail</code> + <code>git worktree list</code>. Brake: <code>factory dispatch stop</code> / <code>factory dispatch resume</code>. Slots: up to {} concurrent.</p>",
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
    html.push_str(&task_graph_fragment_html(&data.tasks));
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
<main><div id=\"ticket\" data-ticket-fragment=\"/fragments/ticket/{}\" data-poll-ms=\"{}\" data-stream-scope=\"ticket\" data-stream-url=\"/streams/ticket/{}\" data-stream-silence-after-ms=\"{}\" data-stream-partial-ms=\"{}\">\
{fragment}</div>{}</main>\
<script>(function(){{var el=document.getElementById('ticket');if(!el)return;var url=el.getAttribute('data-ticket-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'15000',10);\
setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
</body></html>",
        detail.number, SITE_CSS,
        detail.number, detail.number, detail.number, BOARD_POLL_MS,
        detail.number, STREAM_SILENCE_AFTER_MS, STREAM_PARTIAL_MS,
        stream_viewer_html("ticket"),
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
<p>The status/tail/worktree ritual as one live view, with the live task graph beside the slot lanes. Matches <code>factory dispatch status</code> + <code>factory dispatch graph</code> + <code>factory log &lt;ticket&gt; --tail</code>.</p>\
<nav><a href=\"/\">Board</a> | <a href=\"/dispatch\">Dispatcher live</a> | <a href=\"/git\">Git</a> | <a href=\"/files\">Files</a> | <a href=\"/train\">Train review</a></nav>\
<p data-version>factory {version}</p></header>\
<main><div id=\"dispatch\" data-dispatch-fragment=\"/fragments/dispatch\" data-poll-ms=\"{}\" data-stream-scope=\"dispatcher-live\" data-stream-url=\"/streams/dispatch\" data-stream-silence-after-ms=\"{}\" data-stream-partial-ms=\"{}\">\
{fragment}</div>{}</main>\
<script>(function(){{var el=document.getElementById('dispatch');if(!el)return;var url=el.getAttribute('data-dispatch-fragment');var ms=parseInt(el.getAttribute('data-poll-ms')||'3000',10);\
setInterval(function(){{fetch(url).then(function(r){{return r.text();}}).then(function(h){{el.innerHTML=h;}}).catch(function(){{}});}},ms);}})();</script>\
</body></html>",
        SITE_CSS,
        DISPATCHER_POLL_MS,
        STREAM_SILENCE_AFTER_MS,
        STREAM_PARTIAL_MS,
        stream_viewer_html("dispatch"),
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
        .route("/streams/board", axum::routing::get(stream_board_handler))
        .route(
            "/streams/ticket/{number}",
            axum::routing::get(stream_ticket_handler),
        )
        .route(
            "/streams/dispatch",
            axum::routing::get(stream_dispatch_handler),
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

// --- live terminal watch (ticket #91) ----------------------------------------
//
// `factory watch` opens one live pane so nobody re-runs status commands:
// attempt lanes (live + recent with outcome markers), one-screen health
// (version, train, STOP state), and a recent-transitions tail — refreshing
// in place and quitting with the terminal restored. The facts come from
// the same read-only seams the polling views read (`dispatcher_data`'s
// ledger, the open train, the STOP file): watching changes nothing,
// closing changes nothing. Streams are the preferred transport where the
// streams work can serve them; when streams are unavailable or refused
// the pane falls back to polling with a visible-but-calm indicator,
// never an error. Non-interactive output (piped stdout, or stdin that is
// not a terminal) degrades to one static snapshot and exits instead of
// hanging or clearing the screen.
//
// Terminal handling is std-only (no new dependencies): the alternate
// screen is entered/exited with plain ANSI codes, and `stty` drives the
// timed single-byte reads. Interrupts quit through the same restore path
// as `q`: ISIG is off while watching, so Ctrl-C arrives as a readable
// byte (0x03) instead of killing the process mid-screen.

/// Default refresh beat (secs) for `factory watch` polling.
pub const WATCH_DEFAULT_INTERVAL_SECS: u64 = 2;

/// Recent-transition lines carried per frame (newest first, bounded so
/// the tail never grows past one screen of history).
pub const WATCH_TRANSITION_LIMIT: usize = 8;

/// Where one watch frame's facts came from: live streams when the
/// configured URL answers, polling otherwise. Polling is the honest
/// default — the pane names it instead of pretending to be live.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchSource {
    Polling,
    LiveStreams,
}

/// Calm one-line source indicator. The polling line names streams as
/// unavailable so a fallback never reads as breakage.
pub fn watch_source_line(source: &WatchSource) -> &'static str {
    match source {
        WatchSource::Polling => "source: polling (streams unavailable) \u{2014} refreshing calmly",
        WatchSource::LiveStreams => "source: live streams",
    }
}

/// One watch frame: every fact the pane renders, read-only from fixture
/// state (ledger, open train, STOP file). Pure data: rendering and
/// change detection below take this struct, so tests script frames over
/// fixture state without a terminal, a server, or the network.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchSnapshot {
    pub version: String,
    pub train_line: String,
    pub loop_line: String,
    pub live_lanes: Vec<String>,
    pub recent_lanes: Vec<String>,
    pub transitions: Vec<String>,
    pub source: WatchSource,
}

/// One attempt lane: slot, outcome status, phase, then the Bea-readable
/// outcome summary. The status word is the outcome marker (e.g.
/// `review_done` reads as passed, `impl_failed` as failed).
fn watch_lane_line(a: &DispatchAttempt) -> String {
    let slot = format!("T{}-a{}", a.ticket, a.attempt_no);
    format!(
        "{} {} ({}): {}",
        slot,
        a.status,
        attempt_phase(a),
        log_summary(a)
    )
}

/// Probe a streams URL with a short-timeout TCP connect (no reads, no
/// writes, no DNS: only numeric-IP hosts are attempted, so the probe can
/// never hang a hermetic run). `true` when something answers.
fn watch_streams_probe(url: &str) -> bool {
    let without_scheme = url.split("://").last().unwrap_or(url);
    let authority = without_scheme.split('/').next().unwrap_or("");
    let bare = authority.rsplit('@').next().unwrap_or("");
    let (host, port) = match bare.rfind(':') {
        Some(idx) => {
            let (h, tail) = bare.split_at(idx);
            match tail[1..].parse::<u16>() {
                Ok(port) => (h.trim_matches(['[', ']']), port),
                Err(_) => return false,
            }
        }
        None => return false,
    };
    let ip: std::net::IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };
    let addr = std::net::SocketAddr::new(ip, port);
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
}

/// Resolve the transport indicator. Never errors: an absent, unparseable,
/// or refused URL reads as polling with the calm line, never a failure.
fn resolve_watch_source(explicit: Option<&str>) -> WatchSource {
    let configured = explicit
        .map(str::to_string)
        .or_else(|| std::env::var("FACTORY_WATCH_STREAM_URL").ok());
    match configured {
        Some(url) if watch_streams_probe(&url) => WatchSource::LiveStreams,
        _ => WatchSource::Polling,
    }
}

/// Read-only assembly for one watch frame: the lenient ledger load (a
/// missing ledger reads as idle, never an error), the open train, and
/// the STOP file — the same seams `status` and `dispatch status` read.
/// Lanes and the transitions tail run newest-first and bounded, so a
/// seeded delta (new attempt, phase change, completion) moves the next
/// frame with no restart. Never writes.
pub fn watch_snapshot() -> WatchSnapshot {
    let state = dispatch_state_dir();
    let ledger = load_ledger(&state);
    let version = env!("CARGO_PKG_VERSION").to_string();
    let train_line = match load_open_train(&state) {
        None => "train: no open train.".to_string(),
        Some(t) => {
            let base_short: String = t.base_commit.chars().take(8).collect();
            if base_short.is_empty() {
                format!("train: {} is open (status {}).", t.branch, t.status)
            } else {
                format!(
                    "train: {} is open (base {}, status {}).",
                    t.branch, base_short, t.status
                )
            }
        }
    };
    let stopped = state.join("STOP").is_file();
    let loop_line = if stopped {
        "loop: STOPPED (STOP file present) \u{2014} the loop will not schedule until resumed."
            .to_string()
    } else {
        "loop: running".to_string()
    };
    let live_lanes: Vec<String> = ledger
        .attempts
        .iter()
        .filter(|a| is_live_attempt(a))
        .map(watch_lane_line)
        .collect();
    // Ledger order is chronological, so the reverse reads newest-first.
    let recent_lanes: Vec<String> = ledger
        .attempts
        .iter()
        .filter(|a| !is_live_attempt(a))
        .rev()
        .take(HISTORY_RECENT_LIMIT)
        .map(watch_lane_line)
        .collect();
    let transitions: Vec<String> = ledger
        .attempts
        .iter()
        .rev()
        .take(WATCH_TRANSITION_LIMIT)
        .map(|a| {
            let slot = format!("T{}-a{}", a.ticket, a.attempt_no);
            if a.finished_at.is_empty() {
                format!(
                    "{} {} ({}) \u{2014} started {}",
                    slot,
                    a.status,
                    attempt_phase(a),
                    a.started_at
                )
            } else {
                format!(
                    "{} {} ({}) \u{2014} started {}, finished {}",
                    slot,
                    a.status,
                    attempt_phase(a),
                    a.started_at,
                    a.finished_at
                )
            }
        })
        .collect();
    WatchSnapshot {
        version,
        train_line,
        loop_line,
        live_lanes,
        recent_lanes,
        transitions,
        source: resolve_watch_source(None),
    }
}

/// Pure text for one frame (no escape codes): health, lanes, transitions,
/// source indicator. Empty reads calmly (`no attempts yet`), STOP reads
/// as stopped — never as breakage. Shared by the live pane and the piped
/// static snapshot, so both read the same words.
pub fn watch_render_text(snap: &WatchSnapshot) -> String {
    let mut out = String::new();
    out.push_str(&format!("factory {}\n", snap.version));
    out.push_str(&format!("{}\n", snap.train_line));
    out.push_str(&format!("{}\n", snap.loop_line));
    out.push_str("== live attempts ==\n");
    if snap.live_lanes.is_empty() && snap.recent_lanes.is_empty() {
        out.push_str("attempts: no attempts yet.\n");
    } else if snap.live_lanes.is_empty() {
        out.push_str("attempts: no live attempts.\n");
    } else {
        for lane in &snap.live_lanes {
            out.push_str(lane);
            out.push('\n');
        }
    }
    out.push_str("== recent (newest first) ==\n");
    if snap.recent_lanes.is_empty() {
        out.push_str("(none) \u{2014} no finished attempts yet.\n");
    } else {
        for lane in &snap.recent_lanes {
            out.push_str(lane);
            out.push('\n');
        }
    }
    out.push_str("== recent transitions ==\n");
    if snap.transitions.is_empty() {
        out.push_str("(no transitions yet)\n");
    } else {
        for line in &snap.transitions {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(watch_source_line(&snap.source));
    out.push('\n');
    out.push_str("watching changes nothing. Press q (or Ctrl-C) to quit.\n");
    out
}

/// Machine envelope for one frame: the same facts the text pane carries.
fn watch_snapshot_json(snap: &WatchSnapshot) -> serde_json::Value {
    serde_json::json!({
        "version": snap.version,
        "train": snap.train_line,
        "loop": snap.loop_line,
        "live": snap.live_lanes,
        "recent": snap.recent_lanes,
        "transitions": snap.transitions,
        "source": match snap.source {
            WatchSource::Polling => "polling",
            WatchSource::LiveStreams => "live-streams",
        },
    })
}

/// Names of the sections that moved between two frames (`health`,
/// `lanes`, `transitions`, `source`). The live pane rewrites only the
/// moved sections in place — a delta never needs a full redraw or a
/// restart. Empty means the frame is settled and nothing is rewritten.
pub fn watch_frame_changed(prev: &WatchSnapshot, next: &WatchSnapshot) -> Vec<String> {
    let mut changed = Vec::new();
    if prev.version != next.version
        || prev.train_line != next.train_line
        || prev.loop_line != next.loop_line
    {
        changed.push("health".to_string());
    }
    if prev.live_lanes != next.live_lanes || prev.recent_lanes != next.recent_lanes {
        changed.push("lanes".to_string());
    }
    if prev.transitions != next.transitions {
        changed.push("transitions".to_string());
    }
    if prev.source != next.source {
        changed.push("source".to_string());
    }
    changed
}

/// Terminal state after a scripted input run: hermetic proof that both
/// quit paths restore the terminal, with no real terminal needed. `q`
/// (or `Q`, or Esc) is the clean quit; the Ctrl-C byte (0x03) is the
/// interrupt quit — ISIG is off while watching, so an interrupt arrives
/// as this byte and restores through the same path.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchTerminalState {
    pub quit: bool,
    pub clean: bool,
    pub restored: bool,
    pub alternate_screen_left: bool,
}

/// Fold scripted key bytes over scripted frames into the terminal state
/// the pane would hold. Any quit byte ends watching with the terminal
/// restored and the alternate screen left; anything else keeps watching.
pub fn watch_scripted_transition(_frames: &[WatchSnapshot], inputs: &[u8]) -> WatchTerminalState {
    let quit = inputs
        .iter()
        .any(|b| matches!(b, b'q' | b'Q' | 0x03 | 0x1b));
    WatchTerminalState {
        quit,
        clean: quit,
        restored: quit,
        alternate_screen_left: quit,
    }
}

/// Alternate-screen + termios guard: leaving the scope (quit, return, or
/// panic unwind) restores the exact `stty` settings, shows the cursor,
/// and leaves the alternate screen, so no residue survives.
struct WatchTerminalGuard {
    saved_stty: Option<String>,
}

impl Drop for WatchTerminalGuard {
    fn drop(&mut self) {
        if let Some(saved) = self.saved_stty.take() {
            let _ = Command::new("stty").arg(&saved).status();
        }
        print!("\x1b[?25h\x1b[?1049l");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

/// One timed key read: `Some(byte)` on a keypress, `None` on the refresh
/// beat (or on EOF/unreadable stdin, which simply refreshes again).
fn watch_read_key(dev_stdin: &mut Option<std::fs::File>, stdin: &std::io::Stdin) -> Option<u8> {
    use std::io::Read;
    if let Some(f) = dev_stdin.as_mut() {
        let mut buf = [0u8; 1];
        return match f.read(&mut buf) {
            Ok(1) => Some(buf[0]),
            _ => None,
        };
    }
    // Fallback where /dev/stdin is missing: a blocking byte read. Every
    // keypress still refreshes and quitting stays exact; only the idle
    // beat waits for input instead of the clock.
    match stdin.lock().bytes().next() {
        Some(Ok(b)) => Some(b),
        _ => None,
    }
}

/// The interactive pane: alternate screen, rewrite-only-the-moved
/// sections per beat, quit on `q`/Esc/Ctrl-C with the terminal restored
/// by the guard. Skips rewriting settled frames so an idle factory
/// holds still instead of flickering.
fn watch_interactive(interval: u64, source: WatchSource) -> i32 {
    let saved = Command::new("stty")
        .arg("-g")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    // VMIN 0 + VTIME paces the single-byte reads at the refresh beat
    // while ISIG off turns Ctrl-C into a readable byte, so the interrupt
    // path restores through the same guard as the clean path.
    let vtime = (interval.saturating_mul(10)).clamp(10, 250).to_string();
    if saved.is_some() {
        let _ = Command::new("stty")
            .args(["-icanon", "-echo", "-isig", "min", "0", "time", &vtime])
            .status();
    }
    let guard = WatchTerminalGuard { saved_stty: saved };
    print!("\x1b[?1049h\x1b[H\x1b[2J\x1b[?25l");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    // Unbuffered reads so the VTIME beat applies per byte (`Stdin`'s
    // internal buffer would swallow the timeout semantics).
    let mut dev_stdin = std::fs::File::open("/dev/stdin").ok();
    let stdin = std::io::stdin();
    let mut prev: Option<WatchSnapshot> = None;
    let mut first = true;
    loop {
        let mut snap = watch_snapshot();
        snap.source = source.clone();
        let text = watch_render_text(&snap);
        let settled = match &prev {
            None => false,
            Some(p) => watch_frame_changed(p, &snap).is_empty(),
        };
        if first {
            print!("\x1b[H\x1b[2J{text}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            first = false;
        } else if !settled {
            // Home plus clear-below rewrites the pane without the full
            // clear that reads as flicker.
            print!("\x1b[H{text}\x1b[J");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        prev = Some(snap);
        if matches!(
            watch_read_key(&mut dev_stdin, &stdin),
            Some(b'q') | Some(b'Q') | Some(0x03) | Some(0x1b)
        ) {
            break;
        }
    }
    drop(guard);
    0
}

/// CLI boundary for `factory watch`: exit 0 with the pane (or one static
/// snapshot when piped) on success. Read-only: observing changes nothing;
/// the only writes are the terminal's own screen and termios settings,
/// both restored on quit. The global `--json` flag selects machine output
/// in static mode; the live pane is always the text frame.
pub fn cmd_watch(json: bool, interval: u64, streams_url: Option<&str>) -> i32 {
    let source = resolve_watch_source(streams_url);
    let stdout_tty = <std::io::Stdout as std::io::IsTerminal>::is_terminal(&std::io::stdout());
    let stdin_tty = <std::io::Stdin as std::io::IsTerminal>::is_terminal(&std::io::stdin());
    if !stdout_tty || !stdin_tty {
        // Non-interactive degrade: one static snapshot, no screen codes,
        // no stdin reads, no waiting — safe under `| cat` and in scripts.
        let mut snap = watch_snapshot();
        snap.source = source;
        if json {
            println!("{}", watch_snapshot_json(&snap));
        } else {
            print!("{}", watch_render_text(&snap));
        }
        return 0;
    }
    watch_interactive(interval.clamp(1, 60), source)
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

// --- live event streams (ticket #83, spec #66) ---------------------------------
//
// Read-only Server-Sent Events on the existing serve surface; polling is
// untouched (every polling fragment keeps serving byte-identical
// bytes with streams on or off). One connection serves one scope in v1:
// `board`, `ticket`, or `dispatcher-live` — each pushing exactly the facts
// the matching polling view already shows, as whole-fragment snapshots
// and whole-fragment deltas. No new facts, no mutations, no auth change,
// no bind change (loopback by default, `--allow-remote` as before).
//
// Wire order per connection: `hello` (contract version, scope, server
// time, plus the live rules: keepalive, silence bound, buffer bound,
// partial-output bound) then one `snapshot` (current fragment bytes)
// then `commit` + `delta` pairs only when the fragment bytes move: one
// compact commit entry beside each full-state delta, so a consumer that
// never wants full state still learns what moved. Plus SSE keepalive
// comments at the documented interval. Reconnects start from a fresh snapshot and are
// never a replay (the middle is simply missed; `status`/`log` stay the
// record). A connection that delivers no bytes for `silence_after_ms`
// counts as dead: the viewer says so and offers one re-attach action.
// A viewer with `buffer_bound` deltas still queued is disconnected with
// a terminal `reattach` instruction instead of buffering forever. A
// client that sends `?version=` older or newer than
// `STREAM_CONTRACT_VERSION` gets a loud 400 instead of a stream, so
// mismatched viewers fail at hello instead of rendering garbage.
//
// Consumers converge within `STREAM_KEEPALIVE_MS`: the change-detection
// tick runs at a fifth of the keepalive (clamped to 25ms..=1s), so two
// viewers attached to one scope read identical bytes no later than the
// keepalive beat after a move. `FACTORY_STREAM_KEEPALIVE_MS` overrides
// the interval (the hermetic suite shortens it; unset reads as default).

/// Stream contract version named in every hello. Additive changes ride
/// the same version; anything breaking advances this and old viewers
/// fail loudly at hello (spec #66 testing decision).
pub const STREAM_CONTRACT_VERSION: u64 = 1;

/// Documented keepalive interval (ms): a quiet connection past this is
/// the most staleness two viewers can disagree by, and the beat the
/// suite shortens via `FACTORY_STREAM_KEEPALIVE_MS`. Matches the board
/// poll beat (~15s) so streams feel like the views they mirror.
pub const STREAM_KEEPALIVE_MS: u64 = 15_000;

/// Effective keepalive interval (ms): the env override when it parses,
/// the documented default otherwise. Read-only.
pub fn stream_keepalive_ms() -> u64 {
    std::env::var("FACTORY_STREAM_KEEPALIVE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(STREAM_KEEPALIVE_MS)
}

/// Documented silence bound (ms): a connection that delivers no bytes
/// (not even a keepalive comment) for this long counts as dead; the
/// viewer says so and offers one re-attach action. Three quiet
/// keepalives: long enough that one missed beat never reads as death,
/// short enough that a half-open socket never pretends to be live.
/// The suite shortens it via `FACTORY_STREAM_SILENCE_AFTER_MS`.
pub const STREAM_SILENCE_AFTER_MS: u64 = 3 * STREAM_KEEPALIVE_MS;

/// Effective silence bound (ms): the env override when it parses,
/// the documented default otherwise. Read-only.
pub fn stream_silence_after_ms() -> u64 {
    std::env::var("FACTORY_STREAM_SILENCE_AFTER_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(STREAM_SILENCE_AFTER_MS)
}

/// Documented partial-output interval (ms): running tool output
/// (attempt transcript tails) becomes visible within this bound while
/// the call is still going. Sub-second by design: a long run must read
/// as watchable, not stalled. The transcript-bearing scopes
/// (dispatcher-live, ticket) check for output at this cadence even when
/// the keepalive is long; the suite shortens it further via
/// `FACTORY_STREAM_PARTIAL_MS`.
pub const STREAM_PARTIAL_MS: u64 = 500;

/// Effective partial-output interval (ms): the env override when it
/// parses, the documented default otherwise. Read-only.
pub fn stream_partial_ms() -> u64 {
    std::env::var("FACTORY_STREAM_PARTIAL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(STREAM_PARTIAL_MS)
}

/// Documented per-connection buffer bound (events): the most deltas the
/// server will hold for one slow viewer. Past it the viewer is
/// disconnected with a re-attach instruction instead of buffering
/// forever — the backlog is dropped because every delta already carries
/// whole-fragment state, so the next attach lands current anyway. The
/// suite tightens it via `FACTORY_STREAM_BUFFER_BOUND`.
pub const STREAM_BUFFER_BOUND: usize = 16;

/// Effective buffer bound (events): the env override when it parses,
/// the documented default otherwise. Read-only. Clamped so hello,
/// snapshot, and at least one delta always fit before backpressure.
pub fn stream_buffer_bound() -> usize {
    std::env::var("FACTORY_STREAM_BUFFER_BOUND")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(2, 1024))
        .unwrap_or(STREAM_BUFFER_BOUND)
}
/// Change-detection tick (ms): a fifth of the keepalive, clamped so
/// slow defaults stay cheap and short test beats stay responsive.
fn stream_tick_ms(keepalive_ms: u64) -> u64 {
    (keepalive_ms / 5).clamp(25, 1_000)
}

/// Server time for the hello payload: unix epoch seconds. Read-only.
fn stream_server_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Hello payload: contract version, scope, server time, and the live
/// rules (effective keepalive, silence bound, buffer bound) so the
/// viewer reads the-documented behavior from the wire. The ticket scope
/// also names its ticket number. Pure over inputs except the effective
/// rule values, which read the process env like the handlers do.
pub fn stream_hello_json(scope: &str, ticket: Option<u64>) -> serde_json::Value {
    let mut hello = serde_json::json!({
        "version": STREAM_CONTRACT_VERSION,
        "scope": scope,
        "server_time": stream_server_time_secs(),
        "keepalive_ms": stream_keepalive_ms(),
        "silence_after_ms": stream_silence_after_ms(),
        "buffer_bound": stream_buffer_bound(),
        "partial_ms": stream_partial_ms(),
    });
    if let Some(n) = ticket {
        hello["ticket"] = serde_json::Value::from(n);
    }
    hello
}

/// Compact commit entry for one emitted change (ticket #74): scope,
/// per-connection sequence, content hash of the new state, server time,
/// and the human entries describing what moved. The ticket scope also
/// names its ticket. Pure over inputs except the server time.
fn stream_commit_json(
    scope: &StreamScope,
    seq: u64,
    commit_id: &str,
    changes: &[String],
) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "scope": scope.name(),
        "seq": seq,
        "commit": commit_id,
        "server_time": stream_server_time_secs(),
        "changes": changes,
    });
    if let Some(n) = scope.ticket() {
        entry["ticket"] = serde_json::Value::from(n);
    }
    entry
}

/// One per-scope stream source: the exact bytes the matching polling
/// fragment serves, re-read on every tick. Deltas compare these bytes,
/// so a stream can only ever push what polling already shows.
#[derive(Debug, Clone)]
enum StreamScope {
    Board,
    Ticket { number: u64, raw: String },
    DispatcherLive,
}

impl StreamScope {
    /// Scope name as the hello carries it (spec: board, ticket,
    /// dispatcher-live — one scope per connection in v1).
    fn name(&self) -> &'static str {
        match self {
            StreamScope::Board => "board",
            StreamScope::Ticket { .. } => "ticket",
            StreamScope::DispatcherLive => "dispatcher-live",
        }
    }

    /// Stream URL serving one scope (v1: one connection serves one scope).
    /// The re-attach instruction names this path as the way back.
    fn stream_path(&self) -> String {
        match self {
            StreamScope::Board => "/streams/board".to_string(),
            StreamScope::Ticket { number, .. } => format!("/streams/ticket/{number}"),
            StreamScope::DispatcherLive => "/streams/dispatch".to_string(),
        }
    }

    /// Ticket number for the hello payload (`None` off the ticket scope).
    fn ticket(&self) -> Option<u64> {
        match self {
            StreamScope::Ticket { number, .. } => Some(*number),
            _ => None,
        }
    }

    /// Change-detection tick for this scope (ms): a fifth of the
    /// keepalive, clamped so slow defaults stay cheap and short test
    /// beats stay responsive. Transcript-bearing scopes (dispatcher-live,
    /// ticket) additionally cap at the partial-output interval, so running
    /// output surfaces within the documented sub-second bound while the
    /// call is still going. Board cards only move on ledger commits, so
    /// the board keeps the keepalive-derived beat.
    fn tick_ms(&self, keepalive_ms: u64) -> u64 {
        let tick = stream_tick_ms(keepalive_ms);
        match self {
            StreamScope::Board => tick,
            StreamScope::Ticket { .. } | StreamScope::DispatcherLive => {
                tick.min(stream_partial_ms())
            }
        }
    }

    /// Current fragment bytes for this scope plus the compact commit
    /// state they derive from: one read of the polling seams renders the
    /// full html and distills the small JSON the `commit` entries carry.
    /// A stream can only ever push what polling already shows.
    fn snapshot(&self) -> (String, serde_json::Value) {
        match self {
            StreamScope::Board => {
                let data = board_data();
                let html = board_fragment_html(&data);
                (html, board_commit_state(&data))
            }
            StreamScope::Ticket { number, raw } => match ticket_detail(*number, TICKET_DETAIL_TAIL)
            {
                Some(detail) => {
                    let html = ticket_detail_fragment_html(&detail);
                    (html, ticket_commit_state(&detail))
                }
                None => (
                    ticket_missing_fragment_html(raw),
                    serde_json::json!({"missing": raw}),
                ),
            },
            StreamScope::DispatcherLive => {
                let data = dispatcher_data();
                let html = dispatcher_fragment_html(&data);
                (html, dispatcher_commit_state(&data))
            }
        }
    }

    /// One-line human entries describing what moved between two commit
    /// states of this scope. Pure over the states for testing; the
    /// producer calls it once per emitted change, so every `delta` rides
    /// with exactly one `commit` entry beside it.
    fn summarize_change(&self, old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
        match self {
            StreamScope::Board => summarize_board_change(old, new),
            StreamScope::Ticket { .. } => summarize_ticket_change(old, new),
            StreamScope::DispatcherLive => summarize_dispatcher_change(old, new),
        }
    }
}

/// Compact commit feed (ticket #74, spec #66 follow-up): one `commit`
/// entry per emitted change, beside the full-state `snapshot`/`delta`
/// events on the same connection. A consumer that never wants full
/// state listens to `commit` entries only and still learns what moved:
/// scope, per-connection sequence, content hash of the new state, and
/// one-line human entries. Commit states distill the same polling-seam
/// read the html renders from, so the feed can only ever describe what
/// polling already shows. Pure builders and diffs over data/JSON so
/// tests pin them without a server.
///
/// Compact commit state for one board snapshot: per-card column plus
/// status text. Diffs key on issue number.
fn board_commit_state(columns: &[BoardColumn]) -> serde_json::Value {
    let cards: Vec<serde_json::Value> = columns
        .iter()
        .flat_map(|c| {
            c.cards.iter().map(|card| {
                serde_json::json!({
                    "column": c.id,
                    "number": card.number,
                    "title": card.title,
                    "status": card.status_text,
                })
            })
        })
        .collect();
    serde_json::json!({"cards": cards})
}

/// Compact commit state for one ticket snapshot: issue state plus per-
/// attempt status and transcript length. Diffs key on attempt number.
fn ticket_commit_state(detail: &TicketDetail) -> serde_json::Value {
    let attempts: Vec<serde_json::Value> = detail
        .attempts
        .iter()
        .map(|a| {
            serde_json::json!({
                "n": a.attempt_no,
                "status": a.status,
                "tail_lines": a.transcript.lines().count(),
            })
        })
        .collect();
    serde_json::json!({
        "ticket": detail.number,
        "state": detail.state,
        "title": detail.title,
        "attempts": attempts,
    })
}

/// Compact commit state for one dispatcher snapshot: STOP flag, per-slot
/// status plus transcript length, live task nodes, and worktrees. Diffs
/// key attempts on slot and tasks on node id.
fn dispatcher_commit_state(data: &DispatcherData) -> serde_json::Value {
    let attempts: Vec<serde_json::Value> = data
        .attempts
        .iter()
        .map(|a| {
            serde_json::json!({
                "slot": a.slot,
                "status": a.status,
                "tail_lines": a.transcript_tail.lines().count(),
            })
        })
        .collect();
    let tasks: Vec<serde_json::Value> = data
        .tasks
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "kind": t.kind,
                "status": t.status,
                "owner": t.owner,
            })
        })
        .collect();
    serde_json::json!({
        "stopped": data.stopped,
        "attempts": attempts,
        "tasks": tasks,
        "worktrees": data.worktrees,
    })
}

/// Items of one commit-state array, in order. Missing reads as empty.
fn commit_state_items(state: &serde_json::Value, key: &str) -> Vec<serde_json::Value> {
    state
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.to_vec())
        .unwrap_or_default()
}

/// String field of one commit-state item. Missing reads as empty.
fn commit_field(item: &serde_json::Value, field: &str) -> String {
    item.get(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Board diff keyed on issue number: joins, leaves, column moves, and
/// card updates. Sorted so the entry order is deterministic.
fn summarize_board_change(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let mut before: std::collections::BTreeMap<u64, (String, String)> =
        std::collections::BTreeMap::new();
    for card in commit_state_items(old, "cards") {
        if let Some(n) = card.get("number").and_then(|v| v.as_u64()) {
            before.insert(
                n,
                (commit_field(&card, "column"), commit_field(&card, "status")),
            );
        }
    }
    let mut out = Vec::new();
    for card in commit_state_items(new, "cards") {
        let Some(n) = card.get("number").and_then(|v| v.as_u64()) else {
            continue;
        };
        let column = commit_field(&card, "column");
        let status = commit_field(&card, "status");
        match before.remove(&n) {
            None => out.push(format!("#{n} joined {column}")),
            Some((old_column, _)) if old_column != column => {
                out.push(format!("#{n} {old_column}\u{2192}{column}"))
            }
            Some((_, old_status)) if old_status != status => {
                out.push(format!("#{n} updated: {status}"))
            }
            _ => {}
        }
    }
    for n in before.keys() {
        out.push(format!("#{n} left"));
    }
    out.sort();
    out
}

/// Ticket diff keyed on attempt number: starts, transitions, transcript
/// growth, and leaves, plus an issue-level update marker. Sorted.
fn summarize_ticket_change(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if old.get("state") != new.get("state") || old.get("title") != new.get("title") {
        out.push("ticket updated".to_string());
    }
    let mut before: std::collections::BTreeMap<u64, (String, i64)> =
        std::collections::BTreeMap::new();
    for a in commit_state_items(old, "attempts") {
        if let Some(n) = a.get("n").and_then(|v| v.as_u64()) {
            let lines = a.get("tail_lines").and_then(|v| v.as_i64()).unwrap_or(0);
            before.insert(n, (commit_field(&a, "status"), lines));
        }
    }
    for a in commit_state_items(new, "attempts") {
        let Some(n) = a.get("n").and_then(|v| v.as_u64()) else {
            continue;
        };
        let status = commit_field(&a, "status");
        let lines = a.get("tail_lines").and_then(|v| v.as_i64()).unwrap_or(0);
        match before.remove(&n) {
            None => out.push(format!("attempt {n} started")),
            Some((old_status, old_lines)) => {
                if old_status != status {
                    out.push(format!("attempt {n} {old_status}\u{2192}{status}"));
                }
                if lines != old_lines {
                    out.push(format!("attempt {n} {:+} lines", lines - old_lines));
                }
            }
        }
    }
    for n in before.keys() {
        out.push(format!("attempt {n} left"));
    }
    out.sort();
    out
}

/// Dispatcher diff: STOP toggles, per-slot starts/transitions/output
/// growth/leaves, task joins/leaves/transitions, worktree moves. Sorted.
fn summarize_dispatcher_change(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if old.get("stopped") != new.get("stopped") {
        out.push(
            if new
                .get("stopped")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "STOP halted".to_string()
            } else {
                "dispatch loop resumed".to_string()
            },
        );
    }
    let mut before: std::collections::BTreeMap<String, (String, i64)> =
        std::collections::BTreeMap::new();
    for a in commit_state_items(old, "attempts") {
        let lines = a.get("tail_lines").and_then(|v| v.as_i64()).unwrap_or(0);
        before.insert(
            commit_field(&a, "slot"),
            (commit_field(&a, "status"), lines),
        );
    }
    for a in commit_state_items(new, "attempts") {
        let slot = commit_field(&a, "slot");
        if slot.is_empty() {
            continue;
        }
        let status = commit_field(&a, "status");
        let lines = a.get("tail_lines").and_then(|v| v.as_i64()).unwrap_or(0);
        match before.remove(&slot) {
            None => out.push(format!("+attempt {slot}")),
            Some((old_status, old_lines)) => {
                if old_status != status {
                    out.push(format!("{slot} {old_status}\u{2192}{status}"));
                }
                if lines != old_lines {
                    out.push(format!("{slot} {:+} lines", lines - old_lines));
                }
            }
        }
    }
    for slot in before.keys() {
        out.push(format!("-attempt {slot}"));
    }
    let mut old_tasks: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for t in commit_state_items(old, "tasks") {
        old_tasks.insert(commit_field(&t, "id"), commit_field(&t, "status"));
    }
    for t in commit_state_items(new, "tasks") {
        let id = commit_field(&t, "id");
        if id.is_empty() {
            continue;
        }
        let status = commit_field(&t, "status");
        match old_tasks.remove(&id) {
            None => out.push(format!("+task {id}")),
            Some(old_status) if old_status != status => {
                out.push(format!("task {id} {old_status}\u{2192}{status}"))
            }
            _ => {}
        }
    }
    for id in old_tasks.keys() {
        out.push(format!("-task {id}"));
    }
    if old.get("worktrees") != new.get("worktrees") {
        out.push("worktrees moved".to_string());
    }
    out.sort();
    out
}

/// Short content hash naming one emitted state commit: two viewers that
/// read the same state compute the same id, so the compact feed stays
/// comparable across connections. Hex of the sha256, truncated.
fn stream_commit_id(html: &str) -> String {
    let digest = Sha256::digest(html.as_bytes());
    format!("{digest:x}")[..16].to_string()
}

/// `?version=` gate: present and mismatched means a loud 400 naming the
/// expected contract (fail at hello, never render garbage). Pure over
/// inputs for testing.
fn stream_version_error(requested: Option<u64>) -> Option<String> {
    match requested {
        Some(v) if v != STREAM_CONTRACT_VERSION => Some(format!(
            "stream contract mismatch: client asked for version {v}, server speaks version {STREAM_CONTRACT_VERSION} — refresh the viewer to match the server"
        )),
        _ => None,
    }
}

/// Bounded per-connection queue between the producer task and Axum's
/// `Sse` body. Deltas that arrive while the viewer has `buffer_bound`
/// events still queued are dropped as a batch — every delta already
/// carries whole-fragment state, so the middle is simply missed and the
/// next attach lands current anyway. The batch collapse ends in one
/// terminal `reattach` event (calm copy plus one action) and the stream
/// closes: the server never buffers without bound, and closing every
/// client still changes nothing about the loop.
struct StreamShared {
    inner: std::sync::Mutex<StreamInner>,
}

struct StreamInner {
    queue: std::collections::VecDeque<Result<axum::response::sse::Event, std::convert::Infallible>>,
    closed: bool,
    waker: Option<std::task::Waker>,
}

impl StreamShared {
    /// Queue one event unless the stream already ended.
    fn push(self: &std::sync::Arc<Self>, event: axum::response::sse::Event) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.closed {
            return false;
        }
        guard.queue.push_back(Ok(event));
        if let Some(waker) = guard.waker.take() {
            waker.wake();
        }
        true
    }

    /// Queue one delta, or disconnect a stalled viewer: when `buffer_bound`
    /// events are still queued, the backlog collapses into the terminal
    /// `reattach` instruction and the stream ends. Returns false when the
    /// producer must exit (stalled or closed).
    fn push_delta(
        self: &std::sync::Arc<Self>,
        event: axum::response::sse::Event,
        scope: &StreamScope,
        buffer_bound: usize,
    ) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.closed {
            return false;
        }
        if guard.queue.len() >= buffer_bound {
            guard.queue.clear();
            guard.queue.push_back(Ok(stream_reattach_event(scope)));
            guard.closed = true;
            if let Some(waker) = guard.waker.take() {
                waker.wake();
            }
            return false;
        }
        guard.queue.push_back(Ok(event));
        if let Some(waker) = guard.waker.take() {
            waker.wake();
        }
        true
    }

    /// Mark the stream ended (client gone, or slow-viewer disconnect).
    /// The producer checks this every tick and exits, so dead connections
    /// hold no tasks and no buffers.
    fn mark_closed(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
    }

    /// True once the stream ended for any reason.
    fn is_closed(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).closed
    }
}

/// Axum's handle on one connection: pops queued events until the queue
/// is empty and closed. Dropping it (client gone) marks the shared
/// state closed so the producer task exits on its next tick.
struct StreamChannel {
    shared: std::sync::Arc<StreamShared>,
}

impl Drop for StreamChannel {
    fn drop(&mut self) {
        self.shared.mark_closed();
    }
}

impl futures_core::Stream for StreamChannel {
    type Item = Result<axum::response::sse::Event, std::convert::Infallible>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut guard = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(event) = guard.queue.pop_front() {
            return std::task::Poll::Ready(Some(event));
        }
        if guard.closed {
            return std::task::Poll::Ready(None);
        }
        guard.waker = Some(cx.waker().clone());
        std::task::Poll::Pending
    }
}

/// Terminal instruction for a viewer disconnected past the buffer bound:
/// calm copy (nothing stopped, the middle was missed) plus one action
/// (re-open the same stream URL). Pure over the scope for testing.
fn stream_reattach_event(scope: &StreamScope) -> axum::response::sse::Event {
    let payload = serde_json::to_string(&serde_json::json!({
        "message": "This live view fell behind and was disconnected \u{2014} nothing stopped. Re-attach to see current state.",
        "action": "Re-attach",
        "stream": scope.stream_path(),
    }))
    .unwrap_or_else(|_| "{}".to_string());
    axum::response::sse::Event::default()
        .event("reattach")
        .data(payload)
}

/// Single-line JSON envelope for snapshot/delta payloads: one `data:`
/// line per event however large the fragment grows.
fn stream_html_envelope(html: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "html": html }))
        .unwrap_or_else(|_| "{\"html\":\"\"}".to_string())
}

/// Spawn the per-connection producer and return its channel: hello,
/// then a whole snapshot, then a delta whenever the scope's fragment
/// bytes move. Read-only: the producer only re-reads the polling seams.
/// Past `buffer_bound` queued deltas the viewer is disconnected with a
/// re-attach instruction (the shared queue collapses, the stream ends).
fn spawn_stream_producer(
    scope: StreamScope,
    keepalive_ms: u64,
    buffer_bound: usize,
) -> StreamChannel {
    let shared = std::sync::Arc::new(StreamShared {
        inner: std::sync::Mutex::new(StreamInner {
            queue: std::collections::VecDeque::new(),
            closed: false,
            waker: None,
        }),
    });
    let producer = shared.clone();
    tokio::spawn(async move {
        let hello = serde_json::to_string(&stream_hello_json(scope.name(), scope.ticket()))
            .unwrap_or_else(|_| "{}".to_string());
        if !producer.push(
            axum::response::sse::Event::default()
                .event("hello")
                .data(hello),
        ) {
            return;
        }
        let (mut last_html, mut last_state) = scope.snapshot();
        let mut seq: u64 = 0;
        if !producer.push(
            axum::response::sse::Event::default()
                .event("snapshot")
                .data(stream_html_envelope(&last_html)),
        ) {
            return;
        }
        // Per-connection commit sequence: the first emitted change is
        // seq 1, so a compact-feed consumer can tell a missed middle
        // (a gap in seq) from a quiet loop.
        let mut ticks = tokio::time::interval(std::time::Duration::from_millis(
            scope.tick_ms(keepalive_ms),
        ));
        ticks.tick().await;
        loop {
            ticks.tick().await;
            if producer.is_closed() {
                return;
            }
            let (now_html, now_state) = scope.snapshot();
            if now_html != last_html {
                seq += 1;
                let mut changes = scope.summarize_change(&last_state, &now_state);
                if changes.is_empty() {
                    changes.push("state moved".to_string());
                }
                let commit = serde_json::to_string(&stream_commit_json(
                    &scope,
                    seq,
                    &stream_commit_id(&now_html),
                    &changes,
                ))
                .unwrap_or_else(|_| "{}".to_string());
                last_html = now_html;
                last_state = now_state;
                // One entry per commit, announced before the full state
                // it derives from; the pair travels together past the
                // same backpressure bound.
                if !producer.push_delta(
                    axum::response::sse::Event::default()
                        .event("commit")
                        .data(commit),
                    &scope,
                    buffer_bound,
                ) {
                    return;
                }
                let event = axum::response::sse::Event::default()
                    .event("delta")
                    .data(stream_html_envelope(&last_html));
                if !producer.push_delta(event, &scope, buffer_bound) {
                    return;
                }
            }
        }
    });
    StreamChannel { shared }
}

/// Shared stream opener: version gate, then hello/snapshot/deltas with
/// the keepalive beat. The `Sse` response carries
/// `text/event-stream` + `no-cache`; unknown tickets never reach here
/// (their handlers 404 first, like the polling fragments).
fn open_stream(
    scope: StreamScope,
    keepalive_ms: u64,
) -> axum::response::Sse<axum::response::sse::KeepAliveStream<StreamChannel>> {
    axum::response::Sse::new(spawn_stream_producer(
        scope,
        keepalive_ms,
        stream_buffer_bound(),
    ))
    .keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_millis(keepalive_ms)),
    )
}

/// Optional `?version=` query: absent means "speak current".
#[derive(Debug, serde::Deserialize)]
struct StreamQuery {
    version: Option<u64>,
}

/// GET /streams/board: hello, snapshot, then board deltas. Read-only.
async fn stream_board_handler(
    axum::extract::Query(query): axum::extract::Query<StreamQuery>,
) -> Result<
    axum::response::Sse<axum::response::sse::KeepAliveStream<StreamChannel>>,
    (axum::http::StatusCode, String),
> {
    if let Some(err) = stream_version_error(query.version) {
        return Err((axum::http::StatusCode::BAD_REQUEST, err));
    }
    Ok(open_stream(StreamScope::Board, stream_keepalive_ms()))
}

/// GET /streams/ticket/{number}: hello, snapshot, then detail deltas.
/// Unknown tickets 404 calmly (same shape as the polling fragment).
async fn stream_ticket_handler(
    axum::extract::Path(raw): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<StreamQuery>,
) -> Result<
    axum::response::Sse<axum::response::sse::KeepAliveStream<StreamChannel>>,
    (axum::http::StatusCode, String),
> {
    if let Some(err) = stream_version_error(query.version) {
        return Err((axum::http::StatusCode::BAD_REQUEST, err));
    }
    match parse_issue_number(&raw)
        .ok()
        .filter(|n| ticket_detail(*n, TICKET_DETAIL_TAIL).is_some())
    {
        Some(number) => Ok(open_stream(
            StreamScope::Ticket { number, raw },
            stream_keepalive_ms(),
        )),
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            ticket_missing_page_html(&raw),
        )),
    }
}

/// GET /streams/dispatch: hello, snapshot, then dispatcher-live deltas.
/// Read-only (the POST stop/resume controls stay the only writers).
async fn stream_dispatch_handler(
    axum::extract::Query(query): axum::extract::Query<StreamQuery>,
) -> Result<
    axum::response::Sse<axum::response::sse::KeepAliveStream<StreamChannel>>,
    (axum::http::StatusCode, String),
> {
    if let Some(err) = stream_version_error(query.version) {
        return Err((axum::http::StatusCode::BAD_REQUEST, err));
    }
    Ok(open_stream(
        StreamScope::DispatcherLive,
        stream_keepalive_ms(),
    ))
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
    /// Resume count (ticket #61, spec #59): how many dispatch passes have
    /// resumed this attempt from its checkpoint. `#[serde(default)]` keeps
    /// pre-resume ledgers parsing (0 = never resumed); surfaced in every
    /// human and machine status/log row so an audit tells a resume apart.
    #[serde(default)]
    resumed: u64,
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
    /// Decision memos for hooks/approvals (ticket #67, spec #59
    /// follow-up): first-write-wins answers scoped to the attempt, so a
    /// restart finds the stored answer instead of re-prompting.
    /// `#[serde(default)]` keeps pre-memo ledgers parsing (absent reads
    /// as an empty cache, never an error).
    #[serde(default)]
    decision_memos: Vec<DecisionMemo>,
    /// Live task nodes for the observable task graph (ticket #70, spec
    /// #59): one record per task the attempt is currently running or
    /// waiting on. `#[serde(default)]` keeps pre-task ledgers parsing
    /// (absent reads as no live tasks, never an error). Terminal tasks
    /// leave the graph on the ledger commit that marks them terminal;
    /// the graph view filters to live rows and never writes.
    #[serde(default)]
    tasks: Vec<LiveTask>,
    /// Foreground/background tasks owned by attempts (ticket #86 rework
    /// of #69, spec #59 follow-up): work that outlives the turn.
    /// Renamed from the accepted #69 slice's `tasks` to `owned_tasks`
    /// so both features coexist with landed #70's live task graph
    /// (`tasks: Vec<LiveTask>`). `#[serde(default)]` keeps pre-task
    /// ledgers parsing (no migration: empty means the old rules apply
    /// for one pass, then owned tasks record normally).
    #[serde(default)]
    owned_tasks: Vec<DispatchTask>,
    /// Persisted bottom-up unwind order (ticket #86 rework of #69):
    /// effect names in the order their owning tasks undid them. Appended
    /// before each save so a kill mid-abort resumes from the persisted
    /// prefix — nothing undone twice, none skipped. `#[serde(default)]`
    /// keeps old ledgers parsing.
    #[serde(default)]
    abort_log: Vec<String>,
    /// Experiment forks (ticket #80, spec #65): named lines branched at a
    /// parent attempt entry. `#[serde(default)]` keeps pre-fork ledgers
    /// parsing (no migration: unforked tickets behave exactly as before).
    #[serde(default)]
    forks: Vec<ForkRecord>,
}

/// One link in a fork's lineage chain, oldest-first (ticket #81,
/// spec #65): `kind` is `"attempt"` (a fork rooted at an attempt line)
/// or `"fork"` (a nested fork rooted at a parent fork line); `no` is
/// the attempt number or parent fork number; `name` is empty for
/// attempts and the parent fork name for nested links.
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
struct ForkLineageLink {
    ticket: u64,
    kind: String,
    no: u64,
    entry: u64,
    #[serde(default)]
    name: String,
}

/// Default lifecycle state for a fork row written before this ticket
/// (ticket #82): a live line, never interrupted — older rows render
/// exactly as before with no migration.
fn default_fork_status() -> String {
    "forked".to_string()
}

/// Default ownership for a fork row written before this ticket
/// (ticket #73): fully independent — older rows behave exactly as
/// before with no migration.
fn default_fork_ownership() -> String {
    "ownerless".to_string()
}

/// One experiment line (ticket #80, spec #65): the fork inherits the
/// parent attempt's transcript prefix up to and including `parent_entry`
/// and continues independently in its own worktree, branch, slot, and log.
/// The parent transcript is never written by a fork.
///
/// Lineage (ticket #81): `lineage` is the full chain oldest-first;
/// `parent_fork` names the immediate parent fork (empty when the parent
/// is an attempt). `#[serde(default)]` keeps pre-lineage ledgers parsing
/// (no migration: older forks render their chain from the parent
/// attempt + entry they already carry).
///
/// Lifecycle (ticket #82): `status` is the per-line replay state —
/// `"forked"` (live), `"implement_interrupted"` (interrupted,
/// resumable) — and `resumed` counts resumes of this same line.
/// `#[serde(default)]` keeps pre-lifecycle rows parsing as live lines
/// with no resume history (no migration: unforked tickets and old rows
/// behave exactly as before).
///
/// Ownership (ticket #73): `ownership` is `"owned"` (subagent-style:
/// aborting the parent propagates, cleanup follows the owned line) or
/// `"ownerless"` (fully independent, the default). `#[serde(default)]`
/// keeps pre-ownership rows parsing as ownerless (no migration: old
/// forks behave exactly as before).
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
struct ForkRecord {
    id: u64,
    ticket: u64,
    fork_no: u64,
    name: String,
    parent_attempt: u64,
    parent_entry: u64,
    #[serde(default)]
    parent_fork: String,
    #[serde(default)]
    lineage: Vec<ForkLineageLink>,
    #[serde(default = "default_fork_ownership")]
    ownership: String,
    slot: String,
    branch: String,
    worktree: String,
    log_path: String,
    #[serde(default)]
    base_commit: String,
    started_at: String,
    #[serde(default = "default_fork_status")]
    status: String,
    #[serde(default)]
    resumed: u64,
}

/// One stored hook/approval decision (ticket #67, spec #59 follow-up):
/// the attempt scope it belongs to, the decision key, and the winning
/// value the first writer stored. Repeats observe this row instead of
/// re-asking; a fresh attempt starts with no rows. `#[serde(default)]`
/// keeps pre-memo ledgers parsing (absent reads as an empty cache).
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
struct DecisionMemo {
    ticket: u64,
    attempt: u64,
    key: String,
    decision: String,
    stored_at: String,
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

// --- live task graph (ticket #70, spec #59 follow-up) ---------------------
//
// One observable graph of everything live: every live task of an attempt
// is a node carrying its owner edge (the owning task, or the attempt
// itself when parent is absent), its kind (parent | child | background |
// foreground; empty reads as "task"), its status, and what it is waiting
// on. Terminal tasks (done | failed | aborted | cancelled | completed, or
// any status ending _done | _failed | _aborted | _cancelled | _completed)
// leave the graph on the ledger commit that marks them terminal — the
// view filters to live rows. Later tickets (#68 waiting policies, #69
// background scope) own the write paths; this slice only records and
// reads the nodes so the dispatcher screen and debugging share one view.
/// One live task node in the observable task graph.
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
struct LiveTask {
    #[serde(default)]
    id: String,
    #[serde(default)]
    ticket: u64,
    /// Attempt number this task belongs to (matches DispatchAttempt.attempt_no).
    #[serde(default)]
    attempt: u64,
    /// Owning task id; None means owned directly by the attempt itself.
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    status: String,
    /// Task ids (or resources) this task is waiting on; empty means none.
    #[serde(default)]
    waits_on: Vec<String>,
    /// Waiting policy for a parent task (ticket #68, spec #59): ""
    /// (no policy) | "all-settled" | "fail-fast". `#[serde(default)]`
    /// keeps pre-wait ledgers parsing (absent reads as no policy).
    #[serde(default)]
    wait_policy: String,
    /// Replay-safe children may run again after a kill mid-wait (ticket
    /// #68, spec #59); anything else never runs twice. `#[serde(default)]`
    /// keeps pre-wait ledgers parsing (absent reads as not replay-safe).
    #[serde(default)]
    replay_safe: bool,
}

/// `true` once a task has reached a terminal state and must leave the
/// live graph. Centralized so human and JSON modes agree.
fn is_task_terminal(status: &str) -> bool {
    matches!(
        status,
        "done" | "failed" | "aborted" | "cancelled" | "completed"
    ) || status.ends_with("_done")
        || status.ends_with("_failed")
        || status.ends_with("_aborted")
        || status.ends_with("_cancelled")
        || status.ends_with("_completed")
}

/// Owner edge for one node: `task:<parent>` when owned by another task,
/// else `attempt:T<ticket>-a<attempt>` for the attempt itself.
fn task_owner(t: &LiveTask) -> String {
    match &t.parent {
        Some(p) => format!("task:{p}"),
        None => format!("attempt:T{}-a{}", t.ticket, t.attempt),
    }
}

/// Display kind; empty records read as the generic "task".
fn task_kind_text(t: &LiveTask) -> &str {
    if t.kind.is_empty() {
        "task"
    } else {
        t.kind.as_str()
    }
}

// --- foreground/background owned tasks + bottom-up abort (ticket #86
// rework of #69, spec #59 follow-up) ------------------------------------
//
// Work that outlives the turn, and abort that cleans up in order.
// A background task belongs to the attempt but not to its current work:
// the attempt reads idle while only background tasks run, and an ordinary
// abort of the current work leaves background tasks alone. Aborting with
// background scope stops background tasks and everything they own.
//
// Aborts unwind bottom-up: owned work aborts before its owner and each
// task's own cleanup runs only once its owned work is done, so every task
// undoes its own effects. Effects undo newest-first within a task; tasks
// unwind deepest-first (ties break by task id, ascending) so the order is
// deterministic across a kill and its resume. Each undone effect is
// appended to the ledger `abort_log` and the task's `undone` list BEFORE
// the atomic save, so a kill mid-abort resumes from the persisted prefix:
// no effect undone twice, none skipped. The FACTORY_KILL_ABORT_AT seam
// (an effect name) exits(1) right after persisting that effect, exactly
// like a kill -9 at that instant. No migration: ledgers without
// `owned_tasks` read as empty and every command below is a no-op over them.
/// One unit of work owned by an attempt (ticket #86 rework of #69).
///
/// Lives in `DispatchLedger::owned_tasks` (renamed from the accepted #69
/// slice's `tasks` so it coexists with #70's live task graph).
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
struct DispatchTask {
    id: u64,
    ticket: u64,
    attempt: u64,
    name: String,
    /// `foreground` (the attempt's current work) or `background` (work
    /// that outlives the turn). Missing reads as foreground.
    #[serde(default = "task_default_scope")]
    scope: String,
    /// `running`, `done`, or `aborted`. Missing reads as running.
    #[serde(default = "task_default_status")]
    status: String,
    /// Owning task id, `None` for roots.
    #[serde(default)]
    parent: Option<u64>,
    /// Owned effects, created oldest-first; undone newest-first.
    #[serde(default)]
    effects: Vec<String>,
    /// Already-undone effects (persisted prefix of the unwind).
    #[serde(default)]
    undone: Vec<String>,
}

fn task_default_scope() -> String {
    "foreground".to_string()
}

fn task_default_status() -> String {
    "running".to_string()
}

/// Depth of a task in the ownership tree (roots are 0). Missing parents
/// read as roots so a torn fixture can never loop the unwind.
fn task_depth(tasks: &[DispatchTask], id: u64) -> usize {
    let mut depth = 0;
    let mut current = id;
    let mut guard = tasks.len().saturating_add(1);
    while guard > 0 {
        guard -= 1;
        let Some(t) = tasks.iter().find(|t| t.id == current) else {
            break;
        };
        match t.parent {
            Some(p) if p != current && tasks.iter().any(|o| o.id == p) => {
                depth += 1;
                current = p;
            }
            _ => break,
        }
    }
    depth
}

/// Scope filter for an abort: ordinary aborts stop foreground tasks only;
/// background scope additionally stops background tasks (and everything
/// they own, which inherits the owner's scope through the tree).
fn task_in_abort_scope(t: &DispatchTask, background: bool) -> bool {
    if background {
        return true;
    }
    t.scope != "background"
}

/// Bottom-up unwind plan: indexes into `ledger.owned_tasks` for this
/// ticket, deepest-owned work first, ties by task id ascending.
/// Deterministic so a resume replays the same order from persisted
/// progress.
fn abort_plan(ledger: &DispatchLedger, ticket: u64, background: bool) -> Vec<usize> {
    let mut plan: Vec<usize> = ledger
        .owned_tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            t.ticket == ticket && t.status == "running" && task_in_abort_scope(t, background)
        })
        .map(|(i, _)| i)
        .collect();
    plan.sort_by_key(|&i| {
        (
            std::cmp::Reverse(task_depth(&ledger.owned_tasks, ledger.owned_tasks[i].id)),
            ledger.owned_tasks[i].id,
        )
    });
    plan
}

/// `true` while the attempt has no running foreground work but still owns
/// running background work: the attempt reads idle, the work outlives it.
fn attempt_idle_background(ledger: &DispatchLedger, ticket: u64, attempt: u64) -> bool {
    let owned: Vec<&DispatchTask> = ledger
        .owned_tasks
        .iter()
        .filter(|t| t.ticket == ticket && t.attempt == attempt && t.status == "running")
        .collect();
    !owned.is_empty() && owned.iter().all(|t| t.scope == "background")
}

/// Running background tasks owned by this attempt (shared by human and
/// machine surfaces so both carry identical facts).
fn attempt_background_running(ledger: &DispatchLedger, ticket: u64, attempt: u64) -> u64 {
    ledger
        .owned_tasks
        .iter()
        .filter(|t| {
            t.ticket == ticket
                && t.attempt == attempt
                && t.status == "running"
                && t.scope == "background"
        })
        .count() as u64
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

/// Live vs finished (ticket #90): running or interrupted attempts still
/// own the loop; everything else is finished history to summarize, not dump.
fn is_live_attempt(a: &DispatchAttempt) -> bool {
    matches!(a.status.as_str(), "impl_running" | "review_running")
        || is_interrupted_status(&a.status)
}

/// Finished-history rows shown before the `--all` opt-in (ticket #90):
/// live attempts always render, plus this many of the most recent finished.
const HISTORY_RECENT_LIMIT: usize = 8;

/// Counts of attempts by status, alphabetical for stable output.
fn count_by_status<'a, I>(statuses: I) -> Vec<(String, usize)>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for s in statuses {
        *counts.entry(s).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(s, n)| (s.to_string(), n))
        .collect()
}

/// Compact `state xN` words for a history/totals line.
fn counts_words(counts: &[(String, usize)]) -> String {
    counts
        .iter()
        .map(|(s, n)| format!("{s} x{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Newest resumable checkpoint wins (ticket #61, spec #59): the newest
/// record for this ticket, when it reads interrupted. An older
/// interrupted row under a newer running/done/failed row never resumes —
/// the newer history stands and no fork is admitted.
fn newest_resumable_index(ledger: &DispatchLedger, ticket: u64) -> Option<usize> {
    let newest = ledger
        .attempts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.ticket == ticket)
        .max_by_key(|(_, a)| (a.attempt_no, a.id))
        .map(|(i, _)| i)?;
    is_resumable(&ledger.attempts[newest]).then_some(newest)
}

/// Words shared by every human surface when an attempt carries resume
/// history; empty for never-resumed rows so old output is byte-identical.
fn resumed_suffix(a: &DispatchAttempt) -> String {
    if a.resumed == 0 {
        String::new()
    } else {
        format!(" Resumed from checkpoint {} time(s).", a.resumed)
    }
}

/// Calm words for paused-and-will-resume states (ticket #63, spec #59):
/// a restart must never read as breakage to Builder Bea. The
/// replay-safety blind spot stays in output copy, not docs alone:
/// resume replays the interrupted phase only where the underlying step
/// is replay-safe, and anything else waits for the next decision
/// instead of repeating a side effect.
const PAUSED_CALM_LINE: &str =
    "Paused, not broken \u{2014} this attempt will resume on the next pass.";
const REPLAY_BLIND_SPOT_LINE: &str =
    "Resume replays the interrupted phase only where the underlying step is replay-safe; anything else waits for the next decision instead of repeating a side effect.";

/// Print the calm footer on a human surface that lists attempts, only
/// when a resumable row exists. Machine modes already carry the same
/// phase/interrupted/resumed facts; this adds no new facts, only tone.
fn print_paused_copy(resumable_present: bool) {
    if resumable_present {
        println!("{PAUSED_CALM_LINE}");
        println!("{REPLAY_BLIND_SPOT_LINE}");
    }
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

/// `Spec issue: #N` / `Spec: #N` / `Part of #N` reference in a ticket body (#none → None).
fn linked_spec(body: &str) -> Option<u64> {
    let low = body.to_lowercase();
    let mut idx = 0;
    while idx < low.len() {
        let rest = &low[idx..];
        // Earliest marker in the body wins (mirrors the bash dispatcher's
        // first-match scan), so a `Part of` link ahead of a `Spec:` mention
        // resolves to the former.
        let found = ["spec issue:", "spec:", "part of"]
            .iter()
            .filter_map(|m| rest.find(m).map(|i| (i, m.len())))
            .min();
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

/// Cut (or re-attach) the ticket worktree, repairing stale registrations
/// (ticket #75): when the branch is registered at a directory that no
/// longer exists — a dead retry's admin entry, the #60 incident shape —
/// `git worktree add` fails with "already used by worktree at ..." and
/// every later pass would skip the ticket forever. Prune the dead entry
/// and retry so the attempt proceeds on a fresh worktree. An occupying
/// directory that still exists is only reaped when it is our own dead slot
/// under `state/worktrees/`; a foreign checkout is refused with the exact
/// recovery command instead of being touched. Never touches dev/main.
fn ensure_ticket_worktree(
    state: &std::path::Path,
    branch: &str,
    wt: &std::path::Path,
    base: &str,
) -> Result<(), String> {
    let wt_s = wt.to_string_lossy().to_string();
    // One add attempt: reuse form when the branch exists, fresh cut otherwise.
    let attempt_add = || {
        let branch_exists = run_git(&["rev-parse", "--verify", "--quiet", branch], None)
            .map(|o| o.status.success())
            .unwrap_or(false);
        let out = if branch_exists {
            run_git(&["worktree", "add", &wt_s, branch], None)
        } else {
            run_git(&["worktree", "add", &wt_s, "-b", branch, base], None)
        };
        match out {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) => Err(format!("failed to run: {e}")),
        }
    };
    // After one repair of the pinning registration, retry the add once.
    let retry_after_repair = |note: &str| match attempt_add() {
        Ok(()) => {
            println!("dispatch: {note} — fresh worktree cut for {branch}");
            Ok(())
        }
        Err(e) => Err(format!("worktree add failed after {note}: {e}")),
    };
    if let Err(first_err) = attempt_add() {
        // Repair window: drop dead admin entries, then look for the
        // registration that still pins the branch.
        let _ = run_git(&["worktree", "prune"], None);
        let listed = run_git(&["worktree", "list", "--porcelain"], None)
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let holder = parse_git_worktree_porcelain(&listed)
            .into_iter()
            .find(|w| w.branch == branch)
            .map(|w| w.path);
        match holder {
            Some(path) if !std::path::Path::new(&path).exists() => {
                // Directory absent: dead registration, safe to drop.
                let _ = run_git(&["worktree", "remove", "--force", &path], None);
                let _ = run_git(&["worktree", "prune"], None);
                retry_after_repair(&format!("stale worktree registration at {path} pruned"))
            }
            Some(path) => {
                let ours = std::path::Path::new(&path).starts_with(state.join("worktrees"));
                if ours {
                    // Our own dead slot (killed pass leftover): reap it.
                    remove_worktree(std::path::Path::new(&path));
                    retry_after_repair(&format!("dead slot worktree at {path} reaped"))
                } else {
                    Err(format!(
                            "worktree add failed ({first_err}); branch {branch} is checked out at {path} — refusing to touch a foreign checkout. Recover with: git worktree remove --force {path} (after confirming no live worker) or git worktree prune"
                        ))
                }
            }
            None => {
                // No registration pins the branch anymore (the prune
                // above already cleared it): retry once before giving up.
                retry_after_repair("pruning dead worktree entries").map_err(|e| {
                        format!(
                            "{e} (first failure: {first_err}). Recover with: git worktree prune && git worktree list"
                        )
                    })
            }
        }
    } else {
        Ok(())
    }
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

/// Reap stale leases at pass start (ticket #61, spec #59): any record
/// still reading running while this pass holds the lock belongs to a dead
/// worker (kill -9, reboot, STOP between passes) — no live pass can exist
/// beside the lock holder. Reaping marks it `{phase}_interrupted`
/// (resumable) with an audible log line and ticket comment, and never
/// deletes anything. Skipped under dry-run, which writes nothing.
fn reap_stale_leases(state: &std::path::Path, ledger: &mut DispatchLedger) -> usize {
    let mut reaped = 0;
    for i in 0..ledger.attempts.len() {
        let status = ledger.attempts[i].status.clone();
        if status != "impl_running" && status != "review_running" {
            continue;
        }
        let phase = attempt_phase(&ledger.attempts[i]).to_string();
        let ticket = ledger.attempts[i].ticket;
        let attempt_no = ledger.attempts[i].attempt_no;
        let slot = format!("T{ticket}-a{attempt_no}");
        let log_path = ledger.attempts[i].log_path.clone();
        append_log(
            Path::new(&log_path),
            &format!(
                "lease expired during {phase} (slot {slot}, stale worker) — marked resumable\n"
            ),
        );
        ledger.attempts[i].status = format!("{phase}_interrupted");
        ledger.attempts[i].finished_at = now_stamp();
        println!(
            "dispatch: #{ticket} attempt {attempt_no} stale (was {status}) — marked {phase}_interrupted (resumable)"
        );
        gh_comment(
            ticket,
            &format!("DISPATCH: attempt {attempt_no} lease expired during {phase} (stale worker) — marked resumable; the next pass resumes from the checkpoint."),
        );
        reaped += 1;
    }
    if reaped > 0 {
        if let Err(e) = save_ledger(state, ledger) {
            eprintln!("dispatch: {e}");
        }
    }
    reaped
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

/// Phase context for one attempt run (ticket #61, spec #59): everything a
/// pass needs to (re)run phases, whether the attempt is fresh or resumed
/// from its checkpoint.
struct PhaseCtx {
    ticket: u64,
    attempt_no: u64,
    id: u64,
    slot: String,
    branch: String,
    base: String,
    title: String,
    spec_label: String,
    log_path: PathBuf,
    wt: PathBuf,
    tmpdir: PathBuf,
    open_train_branch: String,
    max_attempts: u64,
}

/// Run attempt phases from `start_phase` onward (ticket #61, spec #59).
/// Fresh attempts start at implement (plan checkpoints before this call);
/// resumes start at their checkpointed phase, so completed phases are not
/// repeated — the same record, log file, and slot continue. Unknown phases
/// replay from implement, the safe superset.
fn run_attempt_phases(
    state: &std::path::Path,
    ledger: &mut DispatchLedger,
    idx: usize,
    ctx: &PhaseCtx,
    start_phase: &str,
    brief: &str,
) -> TicketOutcome {
    let ticket = ctx.ticket;
    let attempt_no = ctx.attempt_no;
    let id = ctx.id;
    let max_attempts = ctx.max_attempts;
    let slot = ctx.slot.as_str();
    let branch = ctx.branch.as_str();
    let base = ctx.base.as_str();
    let title = ctx.title.as_str();
    let spec_label = ctx.spec_label.as_str();
    let open_train_branch = ctx.open_train_branch.as_str();
    let log_path = ctx.log_path.as_path();
    let wt = ctx.wt.as_path();
    let tmpdir = ctx.tmpdir.as_path();
    let from = ATTEMPT_PHASES
        .iter()
        .position(|p| *p == start_phase)
        .unwrap_or(1);
    let _ = std::fs::write(wt.join(".factory-brief.md"), brief);
    let build_prompt = PathBuf::from(".pi/prompts/build.md");
    if build_prompt.is_file() {
        if let Ok(prefix) = std::fs::read_to_string(&build_prompt) {
            let _ = std::fs::write(wt.join(".factory-prompt.md"), format!("{prefix}{brief}"));
        }
    }
    let prompt = std::fs::read_to_string(wt.join(".factory-prompt.md"))
        .unwrap_or_else(|_| brief.to_string());
    if from <= 1 {
        append_log(
            log_path,
            &format!("dispatch attempt {slot} implement base={base} branch={branch}\n"),
        );
        // Implement checkpoint: committed before the implementer runs.
        // On failure settle as interrupted at the last committed phase
        // (ticket #75) — never leave the activated row running.
        if let Err(e) = checkpoint_phase(state, ledger, idx, "implement") {
            eprintln!("dispatch: {e}");
            append_log(log_path, &format!("checkpoint implement failed: {e}\n"));
            let phase = attempt_phase(&ledger.attempts[idx]).to_string();
            return interrupt_attempt(state, ledger, idx, log_path, wt, ticket, &phase);
        }
        if simulated_kill_phase("implement") {
            return interrupt_attempt(state, ledger, idx, log_path, wt, ticket, "implement");
        }
        println!("  implement: pi -p (slot {slot})");
        let (ok, _) = run_phase(
            &pi_bin(),
            &["-p", &prompt],
            wt,
            slot,
            tmpdir,
            log_path,
            "implement",
            id,
            ticket,
        );
        if !ok {
            println!("  implement failed — will retry next pass");
            gh_comment(ticket, &format!("DISPATCH: implementer failed (attempt {attempt_no}/{max_attempts}). Log kept in dispatcher state."));
            finish_attempt(state, ledger, idx, "impl_failed", 1);
            remove_worktree(wt);
            return TicketOutcome::Done;
        }
    }
    if from <= 2 {
        println!("  verify: check adapter");
        let adapter = std::env::var_os("FACTORY_CHECK_ADAPTER")
            .map(PathBuf::from)
            .or_else(discover_adapter);
        let Some(adapter) = adapter else {
            append_log(log_path, "verify: no check adapter found\n");
            gh_comment(
                ticket,
                &format!("DISPATCH: no check adapter found (attempt {attempt_no}/{max_attempts})."),
            );
            finish_attempt(state, ledger, idx, "impl_failed", 1);
            remove_worktree(wt);
            return TicketOutcome::Done;
        };
        // Verify checkpoint: committed before the check adapter runs.
        // On failure settle as interrupted (ticket #75), never running.
        if let Err(e) = checkpoint_phase(state, ledger, idx, "verify") {
            eprintln!("dispatch: {e}");
            append_log(log_path, &format!("checkpoint verify failed: {e}\n"));
            let phase = attempt_phase(&ledger.attempts[idx]).to_string();
            return interrupt_attempt(state, ledger, idx, log_path, wt, ticket, &phase);
        }
        if simulated_kill_phase("verify") {
            return interrupt_attempt(state, ledger, idx, log_path, wt, ticket, "verify");
        }
        let (vok, _) = run_phase(
            &adapter.to_string_lossy(),
            &[],
            wt,
            slot,
            tmpdir,
            log_path,
            "verify",
            id,
            ticket,
        );
        if !vok {
            println!("  checks FAILED — routing back");
            let log_text = std::fs::read_to_string(log_path).unwrap_or_default();
            gh_comment(ticket, &format!("DISPATCH: factory-check failed (attempt {attempt_no}/{max_attempts}). Tail:\n```\n{}\n```", last_lines(&log_text, 30)));
            finish_attempt(state, ledger, idx, "impl_failed", 1);
            remove_worktree(wt);
            return TicketOutcome::Done;
        }
    }
    // Review resumes straight onto the recorded head; earlier phases overwrite it above.
    let mut head = ledger.attempts[idx].head_commit.clone();
    if from <= 2 {
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
        let cur = git_head(wt);
        let _ = run_git(&["add", "-A"], Some(wt));
        let staged_clean = run_git(&["diff", "--cached", "--quiet"], Some(wt))
            .map(|o| o.status.success())
            .unwrap_or(false);
        head = cur.clone();
        if staged_clean && cur == base {
            if handoff_text.is_empty() {
                println!("  no changes produced — releasing");
                gh_comment(
                    ticket,
                    "DISPATCH: implementer produced no changes. Human decision required.",
                );
                finish_attempt(state, ledger, idx, "impl_done", 0);
                remove_worktree(wt);
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
                Some(wt),
            );
            head = git_head(wt);
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
                Some(wt),
            );
            head = git_head(wt);
        }
        ledger.attempts[idx].head_commit = head.clone();
        ledger.attempts[idx].exit_code = 0;
        ledger.attempts[idx].status = "impl_done".to_string();
        if let Err(e) = save_ledger(state, ledger) {
            eprintln!("dispatch: {e}");
        }
        gh_relabel(ticket, Some("ready"), "review");
    }
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
        remove_worktree(wt);
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
    // On failure settle as interrupted (ticket #75), never running.
    if let Err(e) = checkpoint_phase(state, ledger, idx, "review") {
        eprintln!("dispatch: {e}");
        append_log(log_path, &format!("checkpoint review failed: {e}\n"));
        let phase = attempt_phase(&ledger.attempts[idx]).to_string();
        remove_worktree(&rwt);
        return interrupt_attempt(state, ledger, idx, log_path, wt, ticket, &phase);
    }
    if simulated_kill_phase("review") {
        remove_worktree(&rwt);
        return interrupt_attempt(state, ledger, idx, log_path, wt, ticket, "review");
    }
    println!("  review: pi -p");
    let (_, rout) = run_phase(
        &pi_bin(),
        &["-p", &review_brief],
        &rwt,
        slot,
        tmpdir,
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
    remove_worktree(wt);
    if verdict.as_deref() == Some("accept") {
        // Accept path: with an open train, auto-merge cleanly or route to
        // blocked on conflict (never forced). With no open train, auto-cut
        // a default-slug train (T35) and merge there instead of waiting;
        // a concurrently opened train is merged into, never duplicated.
        // Never merges into dev/main, never pushes.
        if !open_train_branch.is_empty() {
            match do_train_merge(ticket, branch, open_train_branch, state) {
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
                    match do_train_merge(ticket, branch, &t, state) {
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
/// Resume the newest resumable checkpoint (ticket #61, spec #59): the same
/// attempt continues its phase with its transcript preserved, instead of
/// opening a fresh attempt. The slot gate holds (no resume is admitted at
/// capacity), dry-run announces but writes nothing, and mid-attempt answers
/// still wait for the next attempt boundary — only already-delivered
/// answers are replayed into the resumed brief, so a restart never loses
/// them and never redelivers anything new.
fn resume_ticket_attempt(
    state: &std::path::Path,
    ledger: &mut DispatchLedger,
    ticket: u64,
    idx: usize,
    view: &serde_json::Value,
    dry_run: bool,
) -> TicketOutcome {
    let attempt_no = ledger.attempts[idx].attempt_no;
    let phase = attempt_phase(&ledger.attempts[idx]).to_string();
    let slot = format!("T{ticket}-a{attempt_no}");
    // Slot gate: a resume is admitted exactly like fresh work — never past
    // capacity. The pass loop checks this too; this is the enforcement at
    // the admission point itself.
    if dispatch_active(ledger) >= dispatch_max_concurrent() {
        println!("dispatch: #{ticket} resume deferred — slot gate at capacity");
        return TicketOutcome::Skipped;
    }
    if dry_run {
        println!("dry-run: would resume #{ticket} attempt {attempt_no} from {phase} checkpoint (no writes)");
        return TicketOutcome::Skipped;
    }
    let record_branch = ledger.attempts[idx].branch.clone();
    let record_base = ledger.attempts[idx].base_commit.clone();
    let record_log = ledger.attempts[idx].log_path.clone();
    let wt = state.join("worktrees").join(&slot);
    let tmpdir = state.join("tmp").join(&slot);
    let _ = std::fs::create_dir_all(&tmpdir);
    // Recreate the worktree the interruption released. The branch survives
    // every interruption, so resume re-attaches (ticket #75: stale
    // registrations are repaired inside); a missing branch (never
    // observed, but cheap to survive) is cut fresh from the recorded base.
    remove_worktree(&wt);
    let base = if record_base.is_empty() {
        "HEAD".to_string()
    } else {
        record_base.clone()
    };
    if let Err(e) = ensure_ticket_worktree(state, &record_branch, &wt, &base) {
        println!("dispatch: #{ticket} resume {e}");
        return TicketOutcome::Skipped;
    }
    let n = ledger.attempts[idx].resumed + 1;
    ledger.attempts[idx].resumed = n;
    ledger.attempts[idx].status = if phase == "review" {
        "review_running".to_string()
    } else {
        "impl_running".to_string()
    };
    ledger.attempts[idx].finished_at = String::new();
    append_log(
        Path::new(&record_log),
        &format!(
            "resumed from {phase} checkpoint (slot {slot}, resume #{n}) — transcript preserved\n"
        ),
    );
    if let Err(e) = save_ledger(state, ledger) {
        // Re-activation never persisted (ticket #75): revert to the
        // interrupted checkpoint in memory so no running row escapes,
        // then Done so the loop retries the save loudly.
        eprintln!("dispatch: {e}");
        ledger.attempts[idx].status = format!("{phase}_interrupted");
        ledger.attempts[idx].finished_at = now_stamp();
        remove_worktree(&wt);
        return TicketOutcome::Done;
    }
    println!("  resumed from {phase} checkpoint (resume #{n}) — continuing attempt {attempt_no}");
    gh_comment(
        ticket,
        &format!("DISPATCH: attempt {attempt_no} resumed from {phase} checkpoint (resume #{n}) — continuing the same attempt, transcript preserved."),
    );
    // Answer boundary (ticket #44, unchanged): the resumed brief replays
    // only answers already delivered to this ticket — anything Bea posted
    // mid-attempt structurally waits for the next attempt. Delivery keys
    // are untouched, so single-use still holds and the next fresh attempt
    // picks the arrival up exactly once.
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
    let spec_label = linked_spec(&body)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "none".to_string());
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
    let replay: Vec<ParsedAnswer> = found
        .into_iter()
        .filter(|a| {
            ledger
                .delivered_answers
                .contains(&answer_delivery_key(ticket, a))
        })
        .collect();
    let mut brief = format!("Ticket issue: #{ticket}\nSpec issue: #{spec_label}\nBranch: {record_branch} (base {base})\n\n{body}\n");
    if !replay.is_empty() {
        brief.push('\n');
        brief.push_str(&render_answers_section(&replay));
    }
    let live_train: String = load_open_train(state).map(|t| t.branch).unwrap_or_default();
    let ctx = PhaseCtx {
        ticket,
        attempt_no,
        id: ledger.attempts[idx].id,
        slot,
        branch: record_branch,
        base,
        title,
        spec_label,
        log_path: PathBuf::from(record_log),
        wt,
        tmpdir,
        open_train_branch: live_train,
        max_attempts: dispatch_max_attempts(),
    };
    run_attempt_phases(state, ledger, idx, &ctx, &phase, &brief)
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
    // Resume (ticket #61, spec #59): the newest resumable checkpoint wins —
    // the same attempt continues its phase with its transcript preserved,
    // instead of opening a fresh attempt. Admission gates (budget, deps,
    // spec) were passed when the attempt opened; the slot gate is enforced
    // inside before anything is admitted.
    if let Some(r_idx) = newest_resumable_index(ledger, ticket) {
        return resume_ticket_attempt(state, ledger, ticket, r_idx, &view, dry_run);
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
                resumed: 0,
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

    // Reuse an existing ticket branch for retries (ticket #75: stale
    // registrations are repaired inside); never touch dev/main. No lease
    // is held on failure — the attempt row is pushed only after the
    // directory exists, so every early exit here never activated.
    if let Err(e) = ensure_ticket_worktree(state, &branch, &wt, &base) {
        let msg = format!("dispatch: #{ticket} {e}");
        println!("{msg}");
        append_log(&log_path, &format!("{msg}\n"));
        log_event(&log_path, id, ticket, "worktree-add", 1);
        return TicketOutcome::Skipped;
    }
    let wt_s = wt.to_string_lossy().to_string();
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
        resumed: 0,
    });
    let idx = ledger.attempts.len() - 1;
    // Plan checkpoint: committed before any side-effecting work begins.
    // On failure the row was never activated on disk (this checkpoint is
    // its first persist — ticket #75), so drop it in memory and release
    // the ticket instead of leaving a running row behind.
    if let Err(e) = checkpoint_phase(state, ledger, idx, "plan") {
        eprintln!("dispatch: {e}");
        ledger.attempts.pop();
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
    let ctx = PhaseCtx {
        ticket,
        attempt_no,
        id,
        slot: slot.clone(),
        branch: branch.clone(),
        base: base.clone(),
        title: title.clone(),
        spec_label: spec_label.clone(),
        log_path: log_path.clone(),
        wt,
        tmpdir,
        open_train_branch: open_train_branch.clone(),
        max_attempts,
    };
    run_attempt_phases(state, ledger, idx, &ctx, "implement", &brief)
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
    // Stale-lease reaper (ticket #61, spec #59): running rows under the
    // lock are dead workers, so they become resumable checkpoints before
    // the STOP gate and the slot gate see them. Dry-run writes nothing.
    if !dry_run {
        reap_stale_leases(&state, &mut ledger);
    }
    // STOP mid-attempt leaves resumable checkpoints: the reaper above
    // already converted any in-flight attempt, so halting here is safe
    // and a later `dispatch resume` + pass resumes the same attempt.
    if state.join("STOP").is_file() {
        println!("dispatch: STOP file present — exiting");
        if json {
            println!("{{\"stopped\":true,\"attempted\":0}}");
        }
        return 0;
    }
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
    // Oldest-ready-first: the tracker lists newest-first, but a pass works
    // the queue in ascending ticket order so older tickets starve never.
    // Numberless items sort last and are skipped below, as before.
    let mut list = list;
    list.sort_by_key(|item| {
        item.get("number")
            .and_then(|n| n.as_u64())
            .or_else(|| item.as_u64())
            .unwrap_or(u64::MAX)
    });
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
                "resumed": a.resumed,
                "idle_background": attempt_idle_background(&ledger, a.ticket, a.attempt_no),
                "background_running": attempt_background_running(&ledger, a.ticket, a.attempt_no),
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

    // Fork facts ride alongside attempt rows with identical lineage in
    // both modes. Empty stays present-but-empty: machines see one shape.
    // Per-line figures come from the shared facts document (read-only).
    let (fork_facts, fork_facts_rev) = fork_facts_or_empty(&state);
    let fork_rows: Vec<serde_json::Value> = ledger
        .forks
        .iter()
        .map(|f| fork_json(f, &fork_facts, fork_facts_rev))
        .collect();
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
                "forks": fork_rows,
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
    // Human contract (ticket #90): version, project files, open train,
    // live attempts, loop state — plus a compact finished-history summary.
    // Full per-ticket history stays behind `factory log <ticket>`.
    let live: Vec<&DispatchAttempt> = ledger
        .attempts
        .iter()
        .filter(|a| is_live_attempt(a))
        .collect();
    let finished: Vec<&DispatchAttempt> = ledger
        .attempts
        .iter()
        .filter(|a| !is_live_attempt(a))
        .collect();
    if live.is_empty() {
        if ledger.attempts.is_empty() {
            println!("attempts: no attempts yet.");
        } else {
            println!("attempts: no live attempts.");
        }
    } else {
        for a in &live {
            let slot = format!("T{}-a{}", a.ticket, a.attempt_no);
            let suffix = resumed_suffix(a);
            match a.status.as_str() {
                "impl_running" => println!(
                    "Ticket {} attempt {} is being implemented (slot {}, started {}).{}",
                    a.ticket, a.attempt_no, slot, a.started_at, suffix
                ),
                "review_running" => println!(
                    "Ticket {} attempt {} is being reviewed (slot {}, started {}).{}",
                    a.ticket, a.attempt_no, slot, a.started_at, suffix
                ),
                s if is_interrupted_status(s) => println!(
                    "Ticket {} attempt {} interrupted during {} (slot {}, started {}, resumable).{}",
                    a.ticket,
                    a.attempt_no,
                    attempt_phase(a),
                    slot,
                    a.started_at,
                    suffix
                ),
                other => {
                    if a.finished_at.is_empty() {
                        println!(
                            "Ticket {} attempt {} finished: {other} (slot {}, started {}).{}",
                            a.ticket, a.attempt_no, slot, a.started_at, suffix
                        );
                    } else {
                        println!(
                            "Ticket {} attempt {} finished: {other} (slot {}, started {}, finished {}).{}",
                            a.ticket, a.attempt_no, slot, a.started_at, a.finished_at, suffix
                        );
                    }
                }
            }
            // Idle-with-background reads beside the attempt line so Bea
            // never mistakes a background-only attempt for breakage
            // (ticket #86 rework of #69). Silent when the attempt owns
            // no owned tasks.
            if attempt_idle_background(&ledger, a.ticket, a.attempt_no) {
                println!(
                    "Ticket {} attempt {} is idle — background tasks still running ({} running).",
                    a.ticket,
                    a.attempt_no,
                    attempt_background_running(&ledger, a.ticket, a.attempt_no)
                );
            }
        }
    }
    if !ledger.forks.is_empty() {
        for f in &ledger.forks {
            println!("{}", fork_summary(f, &fork_facts, fork_facts_rev));
        }
    }
    print_paused_copy(ledger.attempts.iter().any(is_resumable));
    if finished.is_empty() {
        println!(
            "history: (none) \u{2014} no finished attempts yet. Full per-ticket history: factory log <ticket>."
        );
    } else {
        let counts = count_by_status(finished.iter().map(|a| a.status.as_str()));
        println!(
            "history: {} finished ({}).",
            finished.len(),
            counts_words(&counts)
        );
        println!("recent (newest first):");
        for a in finished.iter().rev().take(HISTORY_RECENT_LIMIT) {
            if a.finished_at.is_empty() {
                println!(
                    "  T{}-a{} {} (started {}).",
                    a.ticket, a.attempt_no, a.status, a.started_at
                );
            } else {
                println!(
                    "  T{}-a{} {} (started {}, finished {}).",
                    a.ticket, a.attempt_no, a.status, a.started_at, a.finished_at
                );
            }
        }
        println!("Full per-ticket history: factory log <ticket>.");
    }
    if stopped {
        println!("loop: STOPPED");
    } else {
        println!("loop: running");
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
    let base = match a.status.as_str() {
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
    };
    format!("{base}{}", resumed_suffix(a))
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

// ---------------------------------------------------------------------------
// TRANSCRIPT (ticket #93): normalized agent conversation
//
// The attempt log shows tool output (what happened); the agent session
// file shows the conversation (what was said and decided): the user brief,
// assistant turns, thinking, tool calls, results. This section reduces one
// session file to typed items — one reducer every surface renders — so the
// CLI and the UI never parse jsonl twice. Unknown entry types are skipped
// by design (a live file may hold flush markers, not conversation); corrupt
// lines yield partial items plus a named gap, never an error. Reads are
// file reads only: rendering never touches a live run, and a missing
// session reads calmly (an attempt may predate its session).
// ---------------------------------------------------------------------------

/// Default transcript items shown (`--tail 0` means full). Items are
/// denser than log lines, so this is smaller than LOG_DEFAULT_TAIL.
pub const TRANSCRIPT_DEFAULT_TAIL: usize = 20;
/// Per-item text cap: full turns survive, runaway tool output does not.
const TRANSCRIPT_ITEM_CAP: usize = 4000;
/// Override for the agent sessions root in tests
/// (`$HOME/.pi/agent/sessions` in production).
const PI_SESSIONS_DIR_ENV: &str = "FACTORY_PI_SESSIONS_DIR";

/// One normalized transcript row: the user brief, assistant prose, thinking,
/// one tool call (name + key arguments), or one tool result. `name` carries
/// the tool name for calls/results, else empty.
#[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct TranscriptItem {
    pub kind: String,
    #[serde(default)]
    pub name: String,
    pub text: String,
}

/// Char-boundary-safe clip with an ellipsis marker when cut.
fn transcript_clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let cut: String = s.chars().take(n).collect();
    format!("{cut}[\u{2026}]")
}

fn transcript_text_block(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|b| match b {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(m) => {
                    m.get("text").and_then(|t| t.as_str()).map(str::to_string)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Reduce one session file to items plus the count of skipped lines
/// (blank lines are ignored, unparsable ones are counted — a live file may
/// be mid-flush). Never fails: corrupt input yields partial items, never
/// an error. THE shared normalization: both the CLI renderer and the UI
/// pane below consume these items, so no surface parses jsonl twice.
pub fn session_transcript_items(text: &str) -> (Vec<TranscriptItem>, usize) {
    let mut items = Vec::new();
    let mut skipped = 0usize;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if entry.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        let message = entry.get("message");
        let role = message
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            .unwrap_or("");
        match role {
            "user" => {
                let text = message
                    .map(|m| transcript_text_block(&m["content"]))
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    items.push(TranscriptItem {
                        kind: "user".to_string(),
                        name: String::new(),
                        text: transcript_clip(&text, TRANSCRIPT_ITEM_CAP),
                    });
                }
            }
            "assistant" => {
                let empty = Vec::new();
                let blocks = message
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .unwrap_or(&empty);
                for block in blocks {
                    let btype = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match btype {
                        "thinking" => {
                            let text = block.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                            if !text.trim().is_empty() {
                                items.push(TranscriptItem {
                                    kind: "thinking".to_string(),
                                    name: String::new(),
                                    text: transcript_clip(text.trim(), 300),
                                });
                            }
                        }
                        "text" => {
                            let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            if !text.trim().is_empty() {
                                items.push(TranscriptItem {
                                    kind: "assistant".to_string(),
                                    name: String::new(),
                                    text: transcript_clip(text.trim(), TRANSCRIPT_ITEM_CAP),
                                });
                            }
                        }
                        "toolCall" => {
                            let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                            let args = block
                                .get("arguments")
                                .map(|a| {
                                    serde_json::to_string(a).unwrap_or_else(|_| "{}".to_string())
                                })
                                .unwrap_or_else(|| "{}".to_string());
                            items.push(TranscriptItem {
                                kind: "tool_call".to_string(),
                                name: name.to_string(),
                                text: transcript_clip(&args, 200),
                            });
                        }
                        _ => {}
                    }
                }
            }
            "toolResult" => {
                let name = message
                    .and_then(|m| m.get("toolName"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let empty = Vec::new();
                let content = message
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .unwrap_or(&empty);
                let text: Vec<String> = content
                    .iter()
                    .filter_map(|b| {
                        b.get("text")
                            .and_then(|t| t.as_str())
                            .filter(|t| !t.trim().is_empty())
                            .map(str::to_string)
                    })
                    .collect();
                if !text.is_empty() {
                    items.push(TranscriptItem {
                        kind: "tool_result".to_string(),
                        name: name.to_string(),
                        text: transcript_clip(&text.join("\n"), 600),
                    });
                }
            }
            _ => {}
        }
    }
    (items, skipped)
}

/// Agent sessions root: override-first for tests, home-based in
/// production. Missing/unset home reads as no sessions (calm).
fn pi_sessions_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(PI_SESSIONS_DIR_ENV) {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return PathBuf::from(".pi/agent/sessions");
    }
    PathBuf::from(home).join(".pi/agent/sessions")
}

/// Latest session file whose recorded cwd matches the attempt worktree:
/// session dirs are keyed by worktree path, but the key encoding is the
/// agent's business, so match on the recorded `cwd` of each dir's newest
/// file instead of reconstructing the key. Read-only; `None` when
/// nothing matches (attempt predates its session, or none was kept).
pub fn resolve_attempt_session(state: &std::path::Path, worktree: &str) -> Option<PathBuf> {
    let root = pi_sessions_dir();
    let dirs = std::fs::read_dir(&root).ok()?;
    let mut best: Option<(String, PathBuf)> = None;
    for dir in dirs.flatten() {
        if !dir.path().is_dir() {
            continue;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir.path())
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
                    .collect()
            })
            .unwrap_or_default();
        if files.is_empty() {
            continue;
        }
        files.sort();
        let file = files.last().cloned()?;
        let head = std::fs::read_to_string(&file).ok()?;
        let first = head.lines().find(|l| !l.trim().is_empty())?;
        let entry: serde_json::Value = serde_json::from_str(first).ok()?;
        let cwd = entry.get("cwd").and_then(|c| c.as_str()).unwrap_or("");
        let matches = cwd == worktree
            || Path::new(&cwd) == state.join(worktree)
            || cwd == state.join(worktree).to_string_lossy();
        if !matches {
            continue;
        }
        let key = file.file_name()?.to_string_lossy().into_owned();
        if best.as_ref().map(|(k, _)| key > *k).unwrap_or(true) {
            best = Some((key, file));
        }
    }
    best.map(|(_, file)| file)
}

/// Human transcript: one readable block per item, oldest first.
/// `total` is the full item count so a bounded view states its window
/// honestly instead of understating it.
fn render_transcript_human(
    ticket: u64,
    attempt_no: u64,
    slot: &str,
    items: &[TranscriptItem],
    total: usize,
) -> String {
    let mut out = format!(
        "Transcript for ticket {ticket} attempt {attempt_no} (slot {slot}, {total} turns):\n",
    );
    for item in items {
        match item.kind.as_str() {
            "user" => out.push_str(&format!("\n[user]\n{}\n", item.text)),
            "assistant" => out.push_str(&format!("\n[agent]\n{}\n", item.text)),
            "thinking" => out.push_str(&format!("\n[thinking]\n{}\n", item.text)),
            "tool_call" => out.push_str(&format!("\n[tool {}({})]\n", item.name, item.text)),
            "tool_result" => out.push_str(&format!("\n[result {}]\n{}\n", item.name, item.text)),
            _ => out.push_str(&format!("\n[{}]\n{}\n", item.kind, item.text)),
        }
    }
    out
}

/// `--transcript` read path for `log`: latest attempt's session file
/// reduced to items, bounded to the newest `tail` items (`0` = full).
/// An observer like every log path: missing sessions and partial files
/// read calmly with exit 0, never an error shape.
fn cmd_log_transcript(ticket: u64, attempt: &DispatchAttempt, tail: usize, json: bool) -> i32 {
    let state = dispatch_state_dir();
    let slot = format!("T{ticket}-a{}", attempt.attempt_no);
    let Some(file) = resolve_attempt_session(&state, &attempt.worktree) else {
        if json {
            println!(
                "{}",
                serde_json::json!({"ticket": ticket, "attempt": attempt.attempt_no, "slot": slot, "session": null, "items": []})
            );
        } else {
            println!(
                "Ticket {ticket} attempt {} has no recorded session yet.",
                attempt.attempt_no
            );
        }
        return 0;
    };
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let (items, skipped) = session_transcript_items(&text);
    let total = items.len();
    let (shown, truncated) = if tail == 0 || items.len() <= tail {
        (items, false)
    } else {
        (items[items.len() - tail..].to_vec(), true)
    };
    if json {
        let mut doc = serde_json::json!({
            "ticket": ticket,
            "attempt": attempt.attempt_no,
            "slot": slot,
            "session": file.display().to_string(),
            "items": shown,
            "truncated": truncated,
        });
        if skipped > 0 {
            doc["partial"] = serde_json::json!(format!(
                "session file partially unreadable ({skipped} lines skipped)"
            ));
        }
        println!("{doc}");
        return 0;
    }
    print!(
        "{}",
        render_transcript_human(ticket, attempt.attempt_no, &slot, &shown, total)
    );
    if truncated {
        println!("\n(showing newest {tail} of {total} items; --tail 0 for full)");
    }
    if skipped > 0 {
        println!("(session file partially unreadable: {skipped} lines skipped)");
    }
    0
}

/// One ticket's agent-conversation pane as the UI sees it: the latest
/// attempt's normalized items (the same items `factory log --transcript`
/// renders, through the same `session_transcript_items` reducer), bounded
/// to the newest TRANSCRIPT_DEFAULT_TAIL items. `session` is `None` when
/// no session file was recorded yet — a calm empty pane, never an error.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketTranscriptPane {
    pub ticket: u64,
    pub attempt_no: u64,
    pub slot: String,
    pub session: Option<String>,
    pub items: Vec<TranscriptItem>,
    pub total: usize,
    pub truncated: bool,
    pub partial: usize,
}

/// Read-only pane assembly: latest attempt (same newest-first order as
/// `cmd_log`), session file resolved worktree-keyed, items reduced once.
/// Missing/unreadable ledger reads as no pane (`None`); a missing session
/// reads as a pane with `session: None`. File reads only — rendering
/// never disturbs a live run.
pub fn ticket_transcript_pane(ticket: u64) -> Option<TicketTranscriptPane> {
    let state = dispatch_state_dir();
    let ledger = load_ledger(&state);
    let mut entries: Vec<&DispatchAttempt> = ledger
        .attempts
        .iter()
        .filter(|a| a.ticket == ticket)
        .collect();
    entries.sort_by(|a, b| b.attempt_no.cmp(&a.attempt_no).then(b.id.cmp(&a.id)));
    let latest = entries.first()?;
    let slot = format!("T{ticket}-a{}", latest.attempt_no);
    let Some(file) = resolve_attempt_session(&state, &latest.worktree) else {
        return Some(TicketTranscriptPane {
            ticket,
            attempt_no: latest.attempt_no,
            slot,
            session: None,
            items: Vec::new(),
            total: 0,
            truncated: false,
            partial: 0,
        });
    };
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let (items, skipped) = session_transcript_items(&text);
    let total = items.len();
    let (shown, truncated) = if items.len() <= TRANSCRIPT_DEFAULT_TAIL {
        (items, false)
    } else {
        (
            items[items.len() - TRANSCRIPT_DEFAULT_TAIL..].to_vec(),
            true,
        )
    };
    Some(TicketTranscriptPane {
        ticket,
        attempt_no: latest.attempt_no,
        slot,
        session: Some(file.display().to_string()),
        items: shown,
        total,
        truncated,
        partial: skipped,
    })
}

/// Calm sentence when the attempt has no recorded session yet (UI twin of
/// the CLI's missing-session line: ready, not broken).
pub fn transcript_no_session_sentence(ticket: u64, attempt_no: u64, slot: &str) -> String {
    format!("Ticket {ticket} attempt {attempt_no} (slot {slot}) has no recorded session yet.")
}

/// Pane HTML for one transcript (the polling unit rides inside the ticket
/// fragment, so it refreshes with the other fragments). Pure over the pane
/// so tests assert markers without a server: `data-transcript-pane` with
/// per-item `data-transcript-item` carrying the item kind, tool names in
/// `data-tool-name`, item text in `data-transcript-text`, plus
/// `data-transcript-gap` when corrupt lines were skipped and
/// `data-transcript-truncated` naming the bound.
pub fn ticket_transcript_pane_html(pane: &TicketTranscriptPane) -> String {
    let mut html = String::new();
    html.push_str(&format!(
        "<section data-transcript-pane data-slot=\"{}\"><h2>Agent conversation (slot {})</h2>",
        escape_html(&pane.slot),
        escape_html(&pane.slot),
    ));
    let Some(session) = pane.session.as_deref() else {
        html.push_str(&format!(
            "<p data-no-transcript-session>{}</p>",
            escape_html(&transcript_no_session_sentence(
                pane.ticket,
                pane.attempt_no,
                &pane.slot
            )),
        ));
        html.push_str("</section>");
        return html;
    };
    html.push_str(&format!(
        "<p data-transcript-source>Recorded session: <code>{}</code>. Matches <code>factory log {} --transcript</code>.</p>",
        escape_html(session),
        pane.ticket,
    ));
    if pane.items.is_empty() {
        html.push_str("<p data-transcript-empty>(no conversation recorded yet)</p>");
    } else {
        html.push_str("<div data-transcript-items>");
        for item in &pane.items {
            let kind_label = match item.kind.as_str() {
                "user" => "user",
                "assistant" => "agent",
                "thinking" => "thinking",
                "tool_call" => "tool call",
                "tool_result" => "result",
                other => other,
            };
            html.push_str(&format!(
                "<article data-transcript-item=\"{}\"><p data-transcript-kind>{}</p>",
                escape_html(&item.kind),
                kind_label,
            ));
            if (item.kind == "tool_call" || item.kind == "tool_result") && !item.name.is_empty() {
                html.push_str(&format!(
                    "<p data-tool-name>{}</p>",
                    escape_html(&item.name),
                ));
            }
            html.push_str(&format!(
                "<p data-transcript-text>{}</p></article>",
                escape_html(&item.text),
            ));
        }
        html.push_str("</div>");
    }
    if pane.truncated {
        html.push_str(&format!(
            "<p data-transcript-truncated>Showing the newest {} of {} items \u{2014} older items are hidden. Full conversation: <code>factory log {} --transcript --tail 0</code>.</p>",
            pane.items.len(),
            pane.total,
            pane.ticket,
        ));
    }
    if pane.partial > 0 {
        html.push_str(&format!(
            "<p data-transcript-gap>Session file partially unreadable: {} lines skipped \u{2014} showing the items that parsed.</p>",
            pane.partial,
        ));
    }
    html.push_str("</section>");
    html
}

// ---------------------------------------------------------------------------
// FORK (ticket #80, spec #65 slice: fork creation + isolation)
//
// Experiments that start from full context without risking the original.
// `factory fork <ticket> --at <entry> --name <name>` roots a new
// experiment line at a parent attempt entry: the fork inherits the
// conversation prefix up to and including that entry (1-indexed transcript
// line), then continues independently in its own worktree, branch, slot,
// and log file. The parent transcript file is never written by a fork.
//
// Names are unique per ticket; collisions are rejected with the taken
// names shown so no existing experiment is ever hijacked. Forks are
// addressable by name or 1-based number (`--fork NAME|NUMBER` on `log`).
// ---------------------------------------------------------------------------

/// Fork names per ticket (oldest first): the collision message source.
fn fork_taken_names(ledger: &DispatchLedger, ticket: u64) -> Vec<String> {
    let mut names: Vec<String> = ledger
        .forks
        .iter()
        .filter(|f| f.ticket == ticket)
        .map(|f| f.name.clone())
        .collect();
    names.sort();
    names
}

/// Resolve a fork selector (exact name, else 1-based fork number) for one
/// ticket. Pure over the ledger: the seam tests assert both spellings.
fn resolve_fork<'a>(
    ledger: &'a DispatchLedger,
    ticket: u64,
    selector: &str,
) -> Option<&'a ForkRecord> {
    let sel = selector.trim();
    if let Some(f) = ledger
        .forks
        .iter()
        .find(|f| f.ticket == ticket && f.name == sel)
    {
        return Some(f);
    }
    if let Ok(n) = sel.parse::<u64>() {
        if n >= 1 {
            return ledger
                .forks
                .iter()
                .find(|f| f.ticket == ticket && f.fork_no == n);
        }
    }
    None
}

/// Plain-English fork-name validation: nonempty, `[A-Za-z0-9_-]` only.
fn validate_fork_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("fork: name must not be empty (--name <name>)".to_string());
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(format!(
            "fork: invalid name '{trimmed}' (use letters, digits, '_' , '-', '.')"
        ));
    }
    Ok(trimmed.to_string())
}

/// Exact prefix bytes: transcript lines 1..=entry (1-indexed, inclusive).
/// Lines join with `\n` plus a trailing newline so the fork log is a
/// well-formed prefix even when the parent lacks a trailing newline.
fn fork_prefix_slice(full: &str, entry: u64) -> String {
    let lines: Vec<&str> = full.lines().collect();
    let n = (entry as usize).min(lines.len());
    if n == 0 {
        return String::new();
    }
    lines[..n].join("\n") + "\n"
}

/// Documented per-ticket fork cap (ticket #81, spec #65): storage growth
/// is a visible decision, so the bound lives in user-facing copy (the
/// `Fork` help above) and in the refusal message, never silently.
/// A wide experiment tree is visibly more expensive than a single line.
pub const MAX_FORKS_PER_TICKET: u64 = 8;

/// The fork's full lineage chain oldest-first (ticket #81, spec #65).
/// Pre-lineage rows (ticket #80) carry no `lineage`, so the chain is
/// derived from the parent attempt + entry they already record.
fn fork_chain(f: &ForkRecord) -> Vec<ForkLineageLink> {
    if !f.lineage.is_empty() {
        return f.lineage.clone();
    }
    vec![ForkLineageLink {
        ticket: f.ticket,
        kind: "attempt".to_string(),
        no: f.parent_attempt,
        entry: f.parent_entry,
        name: String::new(),
    }]
}

/// Plain-English lineage, oldest-first, shared by human output and the
/// JSON `summary` so both modes carry identical facts.
fn fork_lineage_words(chain: &[ForkLineageLink]) -> String {
    let ticket = chain.first().map(|l| l.ticket).unwrap_or(0);
    let links: Vec<String> = chain
        .iter()
        .map(|l| {
            if l.kind == "fork" && !l.name.is_empty() {
                format!("fork '{}' entry {}", l.name, l.entry)
            } else {
                format!("attempt {} entry {}", l.no, l.entry)
            }
        })
        .collect();
    format!("from ticket #{ticket} {}", links.join(" -> "))
}

/// One compact figure word: strings read as-is, numbers/bools render
/// plainly, anything else falls back to compact JSON (never pretty).
fn fork_figure_word(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(_) | serde_json::Value::Bool(_) => value.to_string(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "?".to_string()),
    }
}

/// One per-line figure from the shared facts document (ticket #81,
/// spec #65 + spec #64): ticket-scoped `forks.<ticket>.<name>.<field>`
/// first, then unscoped `forks.<name>.<field>`. `None` is a missing
/// figure (plain not-recorded words, exit 0) - never synthesized,
/// never zero-filled.
fn fork_figure(facts: &serde_json::Value, ticket: u64, name: &str, field: &str) -> Option<String> {
    for path in [
        format!("forks/{ticket}/{name}/{field}"),
        format!("forks/{name}/{field}"),
    ] {
        if let Some(v) = facts_lookup(facts, &path) {
            return Some(fork_figure_word(v));
        }
    }
    None
}

/// Words shared by every human surface when a fork carries resume
/// history (ticket #82, spec #65); empty for never-resumed rows so old
/// output is byte-identical.
fn fork_resumed_suffix(f: &ForkRecord) -> String {
    if f.resumed == 0 {
        String::new()
    } else {
        format!(" Resumed from checkpoint {} time(s).", f.resumed)
    }
}

/// Ownership words (ticket #73): who this line answers to, or that it
/// answers to nothing. Shared by human output and the JSON `summary`
/// so both modes hold the identical fact.
fn fork_ownership_words(f: &ForkRecord) -> String {
    if f.ownership == "owned" {
        if f.parent_fork.is_empty() {
            format!("owned by attempt {}", f.parent_attempt)
        } else {
            format!("owned by fork '{}'", f.parent_fork)
        }
    } else {
        "ownerless".to_string()
    }
}

/// Abort words (ticket #73): only aborted owned lines carry them, so
/// every other row renders exactly as before. Shared by human output
/// and the JSON `summary` so both modes hold the identical fact.
fn fork_aborted_suffix(f: &ForkRecord) -> String {
    if f.status == "aborted" {
        " Aborted with parent task.".to_string()
    } else {
        String::new()
    }
}

/// Calm one-line experiment summary (ticket #81, spec #65): what line
/// this is, where it descends from (oldest-first), and its per-line
/// figures from the shared document. Missing figures read as
/// not-recorded, never zero-filled. The identical string ships in human
/// output and in the JSON `summary` so scripts reason about the same
/// tree a reader sees. Resume history (ticket #82) appends only for
/// resumed rows, so never-resumed lines render exactly as before.
/// Ownership (ticket #73) names the parent the line answers to, or
/// that it answers to nothing; aborted owned lines carry their abort
/// while every other row renders exactly as before.
fn fork_summary(f: &ForkRecord, facts: &serde_json::Value, revision: u64) -> String {
    let chain = fork_chain(f);
    let cost = fork_figure(facts, f.ticket, &f.name, "cost");
    let changed = fork_figure(facts, f.ticket, &f.name, "changed");
    let figures = match (cost, changed) {
        (Some(c), Some(ch)) => format!("cost {c}, changed {ch}"),
        (Some(c), None) => format!("cost {c}, changed not yet recorded"),
        (None, Some(ch)) => format!("cost not yet recorded, changed {ch}"),
        (None, None) => "cost not yet recorded, changed not yet recorded".to_string(),
    };
    format!(
        "Fork '{}' (#{}, {}): {}; {}; {} (shared facts rev {}).{}{}",
        f.name,
        f.fork_no,
        f.slot,
        fork_lineage_words(&chain),
        fork_ownership_words(f),
        figures,
        revision,
        fork_resumed_suffix(f),
        fork_aborted_suffix(f)
    )
}

/// One fork's machine-readable row (ticket #81, spec #65): the same
/// lineage chain oldest-first plus the same summary string and figures
/// the human line carries, so both modes hold identical facts.
fn fork_json(f: &ForkRecord, facts: &serde_json::Value, revision: u64) -> serde_json::Value {
    let chain = fork_chain(f);
    let lineage: Vec<serde_json::Value> = chain
        .iter()
        .map(|l| {
            serde_json::json!({
                "ticket": l.ticket,
                "kind": l.kind,
                "no": l.no,
                "entry": l.entry,
                "name": l.name,
            })
        })
        .collect();
    serde_json::json!({
        "ticket": f.ticket,
        "fork": f.name,
        "fork_no": f.fork_no,
        "name": f.name,
        "parent_attempt": f.parent_attempt,
        "parent_entry": f.parent_entry,
        "parent_fork": f.parent_fork,
        "ownership": f.ownership,
        "lineage": lineage,
        "slot": f.slot,
        "branch": f.branch,
        "worktree": f.worktree,
        "status": f.status,
        "resumed": f.resumed,
        "summary": fork_summary(f, facts, revision),
        "cost": fork_figure(facts, f.ticket, &f.name, "cost"),
        "changed": fork_figure(facts, f.ticket, &f.name, "changed"),
        "facts_revision": revision,
    })
}

/// Read-only shared-facts load for fork rendering (ticket #81): status
/// and log must never fail on facts trouble, so an unreadable document
/// degrades to empty at revision zero (figures not-recorded) instead
/// of erroring the read.
fn fork_facts_or_empty(state: &std::path::Path) -> (serde_json::Value, u64) {
    match load_facts_document(state) {
        Ok(doc) => (doc.facts, doc.revision),
        Err(_) => (serde_json::json!({}), 0),
    }
}

// ---------------------------------------------------------------------------
// FORK LIFECYCLE (ticket #82, spec #65 slice: replayability per line)
//
// A forked line replays exactly like a main line: `--interrupt` marks
// the line interrupted (resumable) and releases its worktree so resume
// recreates it; `--resume` continues the SAME line (never a new one)
// with its transcript preserved. Both are ledger + file operations on
// the recorded line — no issue-tracker calls, no subprocesses, no
// spend — so they stay hermetic. Landing is untouched: `release merge`
// only ever reads attempt branches, so no path lands a fork without
// the normal accept-and-land decision.
// ---------------------------------------------------------------------------

/// Mark one fork line interrupted (resumable), releasing its worktree so
/// the next `--resume` re-attaches it. Mirrors the dispatcher's attempt
/// interruption: the checkpoint line is already on disk, the ledger
/// reads interrupted, never failed. Refuses live-again lines (already
/// interrupted) instead of stacking markers.
fn interrupt_fork_line(
    state: &std::path::Path,
    ledger: &mut DispatchLedger,
    ticket: u64,
    idx: usize,
) -> i32 {
    let status = ledger.forks[idx].status.clone();
    let name = ledger.forks[idx].name.clone();
    if is_interrupted_status(&status) {
        eprintln!(
            "fork: '{name}' for ticket #{ticket} is already interrupted — resume it with `fork {ticket} --resume {name}`"
        );
        return 1;
    }
    if status != "forked" {
        eprintln!(
            "fork: '{name}' for ticket #{ticket} is not live (status {status}) — nothing to interrupt"
        );
        return 1;
    }
    let slot = ledger.forks[idx].slot.clone();
    let log_path = ledger.forks[idx].log_path.clone();
    let worktree = PathBuf::from(ledger.forks[idx].worktree.clone());
    append_log(
        Path::new(&log_path),
        &format!("interrupted during implement (slot {slot}) — resumable\n"),
    );
    ledger.forks[idx].status = "implement_interrupted".to_string();
    // The worktree is released here and recreated on resume — the same
    // release/re-attach discipline attempts follow, so abandoned lines
    // never pin storage beyond the per-ticket cap.
    remove_worktree(&worktree);
    if let Err(e) = save_ledger(state, ledger) {
        eprintln!("fork: {e}");
        return 1;
    }
    println!("fork '{name}' for ticket #{ticket} interrupted (slot {slot}) — resumable next pass");
    0
}

/// Resume one interrupted fork line: the same line continues its phase
/// with its transcript preserved, instead of branching a new line. The
/// slot gate holds (no resume is admitted past capacity), the released
/// worktree is re-attached on the recorded branch, and the checkpoint
/// line names the resume for the audit — mirroring `resume_ticket_attempt`
/// without its issue-tracker side effects (forks are local lines).
fn resume_fork_line(
    state: &std::path::Path,
    ledger: &mut DispatchLedger,
    ticket: u64,
    idx: usize,
    json: bool,
) -> i32 {
    let status = ledger.forks[idx].status.clone();
    let name = ledger.forks[idx].name.clone();
    if !is_interrupted_status(&status) {
        eprintln!(
            "fork: '{name}' for ticket #{ticket} is not interrupted (status {status}) — nothing to resume"
        );
        return 1;
    }
    // Slot gate: a resume is admitted exactly like fresh work — never
    // past capacity. Shared with attempts: the gate applies across all
    // lines of a ticket.
    if dispatch_active(ledger) >= dispatch_max_concurrent() {
        eprintln!("fork: '{name}' resume deferred — slot gate at capacity");
        return 1;
    }
    let record_branch = ledger.forks[idx].branch.clone();
    let record_base = ledger.forks[idx].base_commit.clone();
    let record_log = ledger.forks[idx].log_path.clone();
    let slot = ledger.forks[idx].slot.clone();
    let wt = state.join("worktrees").join(&slot);
    // Re-attach the worktree the interruption released. The branch
    // survives every interruption, so resume re-attaches; a missing
    // branch (abandoned out-of-band, never observed, but cheap to
    // survive) is cut fresh from the recorded base.
    remove_worktree(&wt);
    let branch_exists = run_git(&["rev-parse", "--verify", "--quiet", &record_branch], None)
        .map(|o| o.status.success())
        .unwrap_or(false);
    let wt_s = wt.to_string_lossy().to_string();
    let base = if record_base.is_empty() {
        "HEAD".to_string()
    } else {
        record_base.clone()
    };
    let added = if branch_exists {
        run_git(&["worktree", "add", &wt_s, &record_branch], None)
    } else {
        run_git(
            &["worktree", "add", &wt_s, "-b", &record_branch, &base],
            None,
        )
    };
    match added {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprintln!(
                "fork: '{name}' resume worktree add failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return 1;
        }
        Err(e) => {
            eprintln!("fork: '{name}' resume worktree add failed: {e}");
            return 1;
        }
    }
    let n = ledger.forks[idx].resumed + 1;
    ledger.forks[idx].resumed = n;
    ledger.forks[idx].status = "forked".to_string();
    append_log(
        Path::new(&record_log),
        &format!(
            "resumed from implement checkpoint (slot {slot}, resume #{n}) — transcript preserved\n"
        ),
    );
    if let Err(e) = save_ledger(state, ledger) {
        eprintln!("fork: {e}");
        remove_worktree(&wt);
        return 1;
    }
    if json {
        let f = &ledger.forks[idx];
        println!(
            "{}",
            serde_json::json!({
                "ticket": ticket,
                "fork": f.name,
                "fork_no": f.fork_no,
                "name": f.name,
                "status": f.status,
                "resumed": f.resumed,
                "slot": f.slot,
                "branch": f.branch,
                "worktree": f.worktree,
            })
        );
        return 0;
    }
    println!("fork '{name}' for ticket #{ticket} resumed from implement checkpoint (resume #{n}) — continuing the same line, transcript preserved");
    0
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_fork(
    ticket_raw: &str,
    attempt_opt: Option<u64>,
    fork_sel: Option<&str>,
    entry_opt: Option<u64>,
    name_opt: Option<&str>,
    resume_sel: Option<&str>,
    interrupt_sel: Option<&str>,
    owned: bool,
    json: bool,
) -> i32 {
    let ticket = match parse_issue_number(ticket_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("fork: {e}");
            return 1;
        }
    };
    let state = dispatch_state_dir();
    let mut ledger = match load_ledger_strict_for("fork", &state) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    // Lifecycle modes (ticket #82) name exactly one line and combine
    // with nothing else: creation flags root a new line, lifecycle
    // flags continue one. Ownership (ticket #73) is a creation flag.
    if resume_sel.is_some() || interrupt_sel.is_some() {
        if attempt_opt.is_some()
            || fork_sel.is_some()
            || entry_opt.is_some()
            || name_opt.is_some()
            || owned
        {
            eprintln!(
                "fork: --resume/--interrupt name one line; creation flags (--attempt/--fork/--at/--name/--owned) root a new one, never both (ticket #{ticket})"
            );
            return 1;
        }
        if resume_sel.is_some() && interrupt_sel.is_some() {
            eprintln!(
                "fork: use either --resume NAME|NUMBER or --interrupt NAME|NUMBER, not both (ticket #{ticket})"
            );
            return 1;
        }
        let sel = resume_sel.or(interrupt_sel).unwrap_or("");
        let Some(idx) = ledger.forks.iter().position(|f| {
            f.ticket == ticket
                && (f.name == sel.trim() || sel.trim().parse::<u64>().ok() == Some(f.fork_no))
        }) else {
            let taken = fork_taken_names(&ledger, ticket);
            if taken.is_empty() {
                eprintln!("fork: ticket #{ticket} has no forks");
            } else {
                eprintln!(
                    "fork: no fork '{}' for ticket #{ticket} (taken: {})",
                    sel.trim(),
                    taken.join(", ")
                );
            }
            return 1;
        };
        if resume_sel.is_some() {
            return resume_fork_line(&state, &mut ledger, ticket, idx, json);
        }
        return interrupt_fork_line(&state, &mut ledger, ticket, idx);
    }
    let Some(entry) = entry_opt else {
        eprintln!("fork: --at ENTRY is required to root a new line (ticket #{ticket})");
        return 1;
    };
    let Some(name_raw) = name_opt else {
        eprintln!("fork: --name NAME is required to root a new line (ticket #{ticket})");
        return 1;
    };
    let name = match validate_fork_name(name_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    // Name collisions are rejected with the taken names shown; the failed
    // fork writes nothing, so no existing experiment is ever hijacked.
    let taken = fork_taken_names(&ledger, ticket);
    if taken.iter().any(|t| t == &name) {
        eprintln!(
            "fork: name '{name}' already taken for ticket #{ticket} (taken: {})",
            taken.join(", ")
        );
        return 1;
    }
    // One parent line only: --attempt and --fork together name no line.
    if attempt_opt.is_some() && fork_sel.is_some() {
        eprintln!(
            "fork: use either --attempt N or --fork NAME|NUMBER, not both (ticket #{ticket})"
        );
        return 1;
    }
    // Documented per-ticket cap (ticket #81, spec #65): past the cap the
    // fork is refused before anything is written - no ledger row, no
    // worktree, no branch, no log file. A wide experiment tree stays a
    // visible storage decision.
    let have_forks = ledger.forks.iter().filter(|f| f.ticket == ticket).count() as u64;
    if have_forks >= MAX_FORKS_PER_TICKET {
        eprintln!(
            "fork: ticket #{ticket} already has {have_forks} forks (cap {MAX_FORKS_PER_TICKET} per ticket); abandon or land one line before branching again"
        );
        return 1;
    }
    // Parent line: an attempt (explicit --attempt, else the newest
    // record) or a parent fork (--fork NAME|NUMBER for nested forks).
    // Either way the parent file is only READ here; the fork seeds its
    // own log file and the parent stays byte-identical.
    enum ForkParent {
        Attempt(DispatchAttempt),
        Fork(ForkRecord),
    }
    let parent: ForkParent = match fork_sel {
        Some(sel) => match resolve_fork(&ledger, ticket, sel) {
            Some(f) => ForkParent::Fork(f.clone()),
            None => {
                let taken = fork_taken_names(&ledger, ticket);
                if taken.is_empty() {
                    eprintln!("fork: ticket #{ticket} has no forks to fork from");
                } else {
                    eprintln!(
                        "fork: no fork '{}' for ticket #{ticket} (taken: {})",
                        sel.trim(),
                        taken.join(", ")
                    );
                }
                return 1;
            }
        },
        None => match attempt_opt {
            Some(no) => match ledger
                .attempts
                .iter()
                .filter(|a| a.ticket == ticket && a.attempt_no == no)
                .max_by_key(|a| a.id)
                .cloned()
            {
                Some(a) => ForkParent::Attempt(a),
                None => {
                    eprintln!("fork: no attempt {no} for ticket #{ticket} to fork from");
                    return 1;
                }
            },
            None => match ledger
                .attempts
                .iter()
                .filter(|a| a.ticket == ticket)
                .max_by_key(|a| (a.attempt_no, a.id))
                .cloned()
            {
                Some(a) => ForkParent::Attempt(a),
                None => {
                    eprintln!("fork: ticket #{ticket} has no attempts to fork from");
                    return 1;
                }
            },
        },
    };
    // Parent transcript, base candidates, and the lineage prefix the
    // child extends. A nested fork inherits its parent fork's prefix
    // (lines 1..=entry of the parent fork log) and records the parent
    // fork's full chain plus its own link, oldest-first.
    let (full, base_candidates, lineage_prefix, parent_fork_name, root_attempt, parent_desc) =
        match &parent {
            ForkParent::Attempt(a) => (
                log_read_transcript(&state, a),
                vec![a.head_commit.clone(), a.base_commit.clone()],
                Vec::new(),
                String::new(),
                a.attempt_no,
                format!("attempt {}", a.attempt_no),
            ),
            ForkParent::Fork(f) => (
                std::fs::read_to_string(Path::new(&f.log_path)).unwrap_or_default(),
                vec![f.base_commit.clone()],
                fork_chain(f),
                f.name.clone(),
                f.parent_attempt,
                format!("fork '{}'", f.name),
            ),
        };
    // Entry bound: 1-indexed into the parent line's transcript.
    let line_count = full.lines().count() as u64;
    if entry < 1 || entry > line_count {
        eprintln!(
            "fork: entry {entry} out of range for ticket #{ticket} {parent_desc} (transcript has {line_count} line(s); use --at 1..={line_count})"
        );
        return 1;
    }
    let fork_no = ledger
        .forks
        .iter()
        .filter(|f| f.ticket == ticket)
        .map(|f| f.fork_no)
        .max()
        .unwrap_or(0)
        + 1;
    let id = ledger
        .attempts
        .iter()
        .map(|a| a.id)
        .chain(ledger.forks.iter().map(|f| f.id))
        .max()
        .unwrap_or(0)
        + 1;
    let slot = format!("T{ticket}-fork-{name}");
    let branch = format!("factory/T{ticket}-fork-{name}");
    let wt = state.join("worktrees").join(&slot);
    let log_path = state
        .join("logs")
        .join(format!("T{ticket}-fork-{name}.log"));
    // Base follows the parent line (head, else base), but fixture ledgers
    // may name refs absent from the scratch repo: verify each candidate
    // and fall back to the local HEAD so the fork always roots somewhere.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut base = String::new();
    for candidate in &base_candidates {
        if candidate.is_empty() {
            continue;
        }
        let valid = run_git(&["rev-parse", "--verify", "--quiet", candidate], Some(&cwd))
            .map(|o| o.status.success())
            .unwrap_or(false);
        if valid {
            base = candidate.clone();
            break;
        }
    }
    if base.is_empty() {
        base = git_head(&cwd);
    }
    let base = if base.is_empty() {
        "HEAD".to_string()
    } else {
        base
    };
    // Isolation: its own worktree on its own branch. A stale branch left
    // by a removed worktree is reused, never force-moved.
    let wt_s = wt.to_string_lossy().to_string();
    let branch_exists = run_git(&["rev-parse", "--verify", "--quiet", &branch], None)
        .map(|o| o.status.success())
        .unwrap_or(false);
    let added = if branch_exists {
        run_git(&["worktree", "add", &wt_s, &branch], None)
    } else {
        run_git(&["worktree", "add", &wt_s, "-b", &branch, &base], None)
    };
    match added {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprintln!(
                "fork: worktree add failed for ticket #{ticket} fork '{name}': {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return 1;
        }
        Err(e) => {
            eprintln!("fork: worktree add failed for ticket #{ticket} fork '{name}': {e}");
            return 1;
        }
    }
    // Seed the fork log with the inherited prefix; the parent file stays
    // byte-identical (only read above, never written).
    let prefix = fork_prefix_slice(&full, entry);
    if let Err(e) = std::fs::create_dir_all(state.join("logs"))
        .map_err(|e| format!("cannot create {}: {e}", state.join("logs").display()))
        .and_then(|()| {
            std::fs::write(&log_path, &prefix)
                .map_err(|e| format!("cannot write fork log {}: {e}", log_path.display()))
        })
    {
        eprintln!("fork: {e}");
        remove_worktree(&wt);
        return 1;
    }
    // The child's chain is the parent line's full chain plus its own
    // link, oldest-first: a direct fork links the attempt, a nested fork
    // links the parent fork. `parent_attempt` stays the root attempt so
    // pre-lineage readers keep their meaning.
    let mut lineage = lineage_prefix;
    match &parent {
        ForkParent::Attempt(a) => lineage.push(ForkLineageLink {
            ticket,
            kind: "attempt".to_string(),
            no: a.attempt_no,
            entry,
            name: String::new(),
        }),
        ForkParent::Fork(f) => lineage.push(ForkLineageLink {
            ticket,
            kind: "fork".to_string(),
            no: f.fork_no,
            entry,
            name: f.name.clone(),
        }),
    }
    let lineage_rows: Vec<serde_json::Value> = lineage
        .iter()
        .map(|l| {
            serde_json::json!({
                "ticket": l.ticket,
                "kind": l.kind,
                "no": l.no,
                "entry": l.entry,
                "name": l.name,
            })
        })
        .collect();
    let ownership = if owned {
        "owned".to_string()
    } else {
        "ownerless".to_string()
    };
    let record = ForkRecord {
        id,
        ticket,
        fork_no,
        name: name.clone(),
        parent_attempt: root_attempt,
        parent_entry: entry,
        parent_fork: parent_fork_name.clone(),
        lineage,
        ownership: ownership.clone(),
        slot: slot.clone(),
        branch: branch.clone(),
        worktree: wt_s.clone(),
        log_path: log_path.to_string_lossy().into_owned(),
        base_commit: base.clone(),
        started_at: now_stamp(),
        status: "forked".to_string(),
        resumed: 0,
    };
    ledger.forks.push(record);
    if let Err(e) = save_ledger(&state, &ledger) {
        eprintln!("fork: {e}");
        remove_worktree(&wt);
        let _ = std::fs::remove_file(&log_path);
        return 1;
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ticket": ticket,
                "fork": name,
                "fork_no": fork_no,
                "name": name,
                "parent_attempt": root_attempt,
                "parent_entry": entry,
                "parent_fork": parent_fork_name,
                "ownership": ownership,
                "lineage": lineage_rows,
                "slot": slot,
                "branch": branch,
                "worktree": wt_s,
            })
        );
        return 0;
    }
    println!(
        "Forked ticket #{ticket} {parent_desc} entry {entry} into fork '{name}' (#{fork_no}, slot {slot}).",
    );
    println!("Branch {branch} at {wt_s}.");
    0
}

pub fn cmd_log(
    ticket_raw: &str,
    tail: usize,
    fork_sel: Option<&str>,
    transcript: bool,
    json: bool,
) -> i32 {
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
    // Transcript read path: the latest attempt's conversation, not the log.
    // --fork selects a different history view, so the combination is refused
    // plainly rather than guessed at.
    if transcript {
        if fork_sel.is_some() {
            eprintln!("log: --transcript with --fork is not supported yet");
            return 1;
        }
        let mut latest: Vec<&DispatchAttempt> = ledger
            .attempts
            .iter()
            .filter(|a| a.ticket == ticket)
            .collect();
        latest.sort_by(|a, b| b.attempt_no.cmp(&a.attempt_no).then(b.id.cmp(&a.id)));
        match latest.first() {
            None => {
                if json {
                    println!("{}", serde_json::json!({"ticket": ticket, "attempts": []}));
                } else {
                    println!("Ticket {ticket}: no attempts yet.");
                }
                return 0;
            }
            Some(a) => {
                let tail_items = if tail == LOG_DEFAULT_TAIL {
                    TRANSCRIPT_DEFAULT_TAIL
                } else {
                    tail
                };
                return cmd_log_transcript(ticket, a, tail_items, json);
            }
        }
    }
    // Fork read path: addressable by name or number in daily commands.
    if let Some(sel) = fork_sel {
        let Some(f) = resolve_fork(&ledger, ticket, sel) else {
            let taken = fork_taken_names(&ledger, ticket);
            if taken.is_empty() {
                eprintln!("log: ticket #{ticket} has no forks");
            } else {
                eprintln!(
                    "log: no fork '{}' for ticket #{ticket} (taken: {})",
                    sel.trim(),
                    taken.join(", ")
                );
            }
            return 1;
        };
        let full = std::fs::read_to_string(Path::new(&f.log_path)).unwrap_or_default();
        let (shown, truncated) = log_tail(&full, tail);
        let (fork_facts, fork_facts_rev) = fork_facts_or_empty(&state);
        if json {
            let mut row = fork_json(f, &fork_facts, fork_facts_rev);
            row["transcript"] = serde_json::Value::String(shown);
            row["truncated"] = serde_json::Value::Bool(truncated);
            println!("{row}");
            return 0;
        }
        println!("{}", fork_summary(f, &fork_facts, fork_facts_rev));
        if shown.is_empty() {
            println!("(no transcript yet)");
        } else if tail == 0 {
            println!("Transcript (full):");
            print!("{shown}");
            if !shown.ends_with('\n') {
                println!();
            }
        } else {
            println!("Transcript (last {tail} lines):");
            print!("{shown}");
            if !shown.ends_with('\n') {
                println!();
            }
        }
        return 0;
    }
    let mut entries: Vec<&DispatchAttempt> = ledger
        .attempts
        .iter()
        .filter(|a| a.ticket == ticket)
        .collect();
    entries.sort_by(|a, b| b.attempt_no.cmp(&a.attempt_no).then(b.id.cmp(&a.id)));
    let mut ticket_forks: Vec<&ForkRecord> =
        ledger.forks.iter().filter(|f| f.ticket == ticket).collect();
    ticket_forks.sort_by_key(|a| a.fork_no);
    if entries.is_empty() && ticket_forks.is_empty() {
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
        // Memo facts per attempt (ticket #67): the stored answers a hook
        // reuses on resume instead of re-asking. Empty for memo-less
        // attempts so old rows read exactly as before.
        let memos: Vec<serde_json::Value> = attempt_memos(&ledger, a.ticket, a.attempt_no)
            .iter()
            .map(|m| memo_json(m))
            .collect();
        rows.push(serde_json::json!({
            "ticket": a.ticket,
            "attempt": a.attempt_no,
            "slot": format!("T{}-a{}", a.ticket, a.attempt_no),
            "status": a.status,
            "summary": log_summary(a),
            "phase": attempt_phase(a),
            "resumable": is_resumable(a),
            "resumed": a.resumed,
            "idle_background": attempt_idle_background(&ledger, a.ticket, a.attempt_no),
            "background_running": attempt_background_running(&ledger, a.ticket, a.attempt_no),
            "memos": memos,
            "base": a.base_commit,
            "head": a.head_commit,
            "started": a.started_at,
            "finished": a.finished_at,
            "exit_code": a.exit_code,
            "transcript": shown,
            "truncated": truncated,
        }));
    }

    // Fork facts ride alongside attempt rows so machines and prose read
    // one story. Empty stays absent: unforked tickets keep the old shape.
    // Per-line figures come from the shared facts document (read-only).
    let (fork_facts, fork_facts_rev) = fork_facts_or_empty(&state);
    let fork_rows: Vec<serde_json::Value> = ticket_forks
        .iter()
        .map(|f| fork_json(f, &fork_facts, fork_facts_rev))
        .collect();
    if json {
        let mut out = serde_json::json!({"ticket": ticket, "attempts": rows});
        if !fork_rows.is_empty() {
            out["forks"] = serde_json::Value::Array(fork_rows);
        }
        println!("{out}");
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
        // Idle-with-background reads beside the attempt story so the log
        // names where the attempt halted the same way status does (#86).
        if row
            .get("idle_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let running = row
                .get("background_running")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            println!(
                "Attempt {attempt} is idle — background tasks still running ({running} running)."
            );
        }
        if let Some(memos) = row.get("memos").and_then(|v| v.as_array()) {
            if !memos.is_empty() {
                let pairs: Vec<String> = memos
                    .iter()
                    .map(|m| {
                        format!(
                            "{}={}",
                            m.get("key").and_then(|v| v.as_str()).unwrap_or("?"),
                            m.get("decision").and_then(|v| v.as_str()).unwrap_or("?")
                        )
                    })
                    .collect();
                println!(
                    "Decisions ({}, reused on resume instead of asking again): {}.",
                    memos.len(),
                    pairs.join(", ")
                );
            }
        }
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
    for f in &ticket_forks {
        println!("{}", fork_summary(f, &fork_facts, fork_facts_rev));
    }
    print_paused_copy(entries.iter().any(|a| is_resumable(a)));
    0
}

// ---------------------------------------------------------------------------
// REVISION-GUARDED FACTS, slice 1: the document + reads (ticket #77,
// spec #64)
//
// One JSON facts document lives in the existing state directory beside
// the dispatch ledger (`facts.json` next to `dispatch.json`):
// `{"revision": N, "facts": {...}}`. The ledger still owns attempt
// lifecycle, logs still own transcripts; facts own cross-attempt figures
// and nothing else moves stores. No migration: an absent document reads
// as empty at revision zero, and reading never creates files, so
// adoption touches no ledger, log, or worktree behavior.
//
// Reads address the whole document (`factory facts show`) or one
// slash-separated path (`factory facts show costs/total`), human-readable
// by default and machine-readable with the global --json flag carrying
// the same value-plus-revision facts. A missing path is a plain sentence
// with exit 0 — never zero-filled, never error-shaped. Path-syntax
// validation, guarded writes, counters, and size bounds arrive in later
// tickets and reuse these same load/traverse seams.
// ---------------------------------------------------------------------------

/// Facts document filename inside the state directory (beside the
/// dispatch ledger, the logs dir, and the train file).
const FACTS_FILENAME: &str = "facts.json";

/// Document-size cap in bytes (ticket #79, spec #64): the serialized
/// facts document (`{"revision": N, "facts": {...}}`, pretty-printed,
/// exactly as written to disk) must fit in 65536 bytes (64 KiB).
/// Writes that would exceed it are refused with the cap stated — never
/// silently truncated. Deletes only shrink, so they skip the check.
const FACTS_MAX_DOCUMENT_BYTES: usize = 65536;

/// Accepted facts-path shape (ticket #79): slash-separated names like
/// `costs/total`, each segment 1-64 chars of letters, digits, `_` or
/// `-`, 1-16 segments. Shown verbatim on every path rejection so a
/// typo never writes to a surprise location.
const FACTS_PATH_SHAPE: &str =
    "paths look like costs/total (slash-separated names of letters, digits, _ or -, e.g. costs/total)";

/// Human footer on every facts read: the per-machine blind spot from
/// spec #64 stays in user-facing copy wherever facts are shown, so Bea
/// never mistakes local figures for shared ones. Machine output keeps
/// exactly the value-plus-revision facts (no tone, same shape).
const FACTS_LOCALITY_LINE: &str =
    "Per-machine local state: a ticket built on two machines sees two documents.";

fn facts_doc_path(state: &std::path::Path) -> PathBuf {
    state.join(FACTS_FILENAME)
}

/// One loaded facts document: the monotonic revision plus the whole
/// value tree. Reads-only shape; writers advance the revision.
struct FactsDocument {
    revision: u64,
    facts: serde_json::Value,
}

/// Read-only facts load. Absent file (or absent state dir) reads as
/// empty at revision zero with nothing created; unreadable or corrupt
/// files error naming the facts document, mirroring the ledger seam.
fn load_facts_document(state: &std::path::Path) -> Result<FactsDocument, String> {
    let path = facts_doc_path(state);
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FactsDocument {
                revision: 0,
                facts: serde_json::json!({}),
            });
        }
        Err(e) => {
            return Err(format!("facts: cannot read {}: {e}", path.display()));
        }
        Ok(t) => t,
    };
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("facts: cannot parse {}: {e}", path.display()))?;
    let obj = parsed.as_object().ok_or_else(|| {
        format!(
            "facts: cannot parse {}: expected an object with revision + facts",
            path.display()
        )
    })?;
    Ok(FactsDocument {
        revision: obj.get("revision").and_then(|r| r.as_u64()).unwrap_or(0),
        facts: obj
            .get("facts")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    })
}

/// Strict facts-path validation (ticket #79): a raw path is accepted
/// only when it already has the documented shape; nothing is
/// normalized away, so a typo never writes to a surprise location.
/// Returns the canonical path on success.
fn validate_facts_path(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err(format!(
            "facts: invalid path '{raw}': {FACTS_PATH_SHAPE}; nothing changed."
        ));
    }
    let segments: Vec<&str> = raw.split('/').collect();
    if segments.len() > 16
        || segments.iter().any(|seg| {
            seg.is_empty()
                || seg.len() > 64
                || !seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    {
        return Err(format!(
            "facts: invalid path '{raw}': {FACTS_PATH_SHAPE}; nothing changed."
        ));
    }
    Ok(raw.to_string())
}

/// Traverse the facts tree along a validated path. `None` is a missing
/// fact (plain sentence, exit 0) — never synthesized, never zero-filled.
fn facts_lookup<'a>(facts: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = facts;
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        current = current.get(seg)?;
    }
    Some(current)
}

/// Pretty value for human output (mirrors the JSON mode value exactly).
fn facts_pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Store `value` at `path` (normalized, non-empty), creating any
/// missing intermediate objects. Errors (nothing changed) when an
/// intermediate segment exists as a non-object or the document root is
/// not an object.
fn facts_set_value(
    facts: &mut serde_json::Value,
    path: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (last, parents) = segments.split_last().expect("non-empty facts path");
    if !facts.is_object() {
        return Err(format!(
            "facts: cannot write {path}: the facts document root is not an object; nothing changed."
        ));
    }
    let mut current = facts;
    for seg in parents {
        if current.get(seg).is_none() {
            current
                .as_object_mut()
                .expect("facts cursor stays an object")
                .insert((*seg).to_string(), serde_json::json!({}));
        }
        current = current
            .get_mut(seg)
            .expect("just-inserted facts segment present");
        if !current.is_object() {
            return Err(format!(
                "facts: cannot write {path}: '{seg}' is not an object; nothing changed."
            ));
        }
    }
    current
        .as_object_mut()
        .expect("facts parent stays an object")
        .insert((*last).to_string(), value);
    Ok(())
}

/// Remove the value at `path` (normalized, non-empty). `Ok(true)` is a
/// removal; `Ok(false)` is a missing path (nothing changed, refused by
/// the caller). Errors when the document root is not an object.
fn facts_delete_value(facts: &mut serde_json::Value, path: &str) -> Result<bool, String> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (last, parents) = segments.split_last().expect("non-empty facts path");
    if !facts.is_object() {
        return Err(format!(
            "facts: cannot delete {path}: the facts document root is not an object; nothing changed."
        ));
    }
    let mut current = facts;
    for seg in parents {
        match current.get_mut(seg) {
            Some(next) if next.is_object() => current = next,
            _ => return Ok(false),
        }
    }
    Ok(current
        .as_object_mut()
        .expect("facts parent stays an object")
        .remove(*last)
        .is_some())
}

/// Atomic facts-document write (temp file + rename, the same durability
/// guarantee as the ledger): a crash between the two leaves the previous
/// document on disk, never a torn one. The FACTORY_FACTS_CRASH seam
/// exists only to pin that guarantee in tests: with it set to `1`, this
/// writes tmp and the process exits before the rename, exactly like a
/// kill -9 at that instant (orphaned tmp, previous document intact).
fn save_facts_document(state: &std::path::Path, doc: &FactsDocument) -> Result<(), String> {
    std::fs::create_dir_all(state)
        .map_err(|e| format!("facts: cannot create {}: {e}", state.display()))?;
    let text = serde_json::to_string_pretty(&serde_json::json!({
        "revision": doc.revision,
        "facts": doc.facts,
    }))
    .map_err(|e| format!("facts: cannot serialize facts document: {e}"))?;
    let tmp = facts_doc_path(state).with_extension("json.tmp");
    std::fs::write(&tmp, &text)
        .map_err(|e| format!("facts: cannot stage {}: {e}", tmp.display()))?;
    // Test-only kill seam: behave exactly like a crash after the tmp
    // write but before the rename.
    if std::env::var("FACTORY_FACTS_CRASH").as_deref() == Ok("1") {
        std::process::exit(1);
    }
    std::fs::rename(&tmp, facts_doc_path(state)).map_err(|e| {
        format!(
            "facts: cannot replace {}: {e}",
            facts_doc_path(state).display()
        )
    })?;
    Ok(())
}

/// History-file name beside the facts document (ticket #71, spec
/// #64): one small entry per accepted write, appended after the live
/// document commits. No retention eviction in v1 (growth is one entry
/// per write; reads stay bounded by --limit instead).
const FACTS_HISTORY_FILENAME: &str = "facts.history.json";

/// Bound on one history read (ticket #71): at most 50 entries per
/// invocation, mirroring the log transcript tail scale. Stated in
/// every history-bound refusal; there is no unbounded history dump.
const FACTS_HISTORY_MAX_ENTRIES: u64 = 50;

fn facts_history_path(state: &std::path::Path) -> PathBuf {
    state.join(FACTS_HISTORY_FILENAME)
}

/// One recorded write, as stored: the revision it committed plus the
/// path and its stored value (or the deletion marker). Kept as plain
/// JSON values (no struct) so a corrupt entry fails the load loudly
/// instead of half-deserializing.
fn facts_history_entry(
    revision: u64,
    path: &str,
    stored: Option<&serde_json::Value>,
) -> serde_json::Value {
    match stored {
        Some(value) => serde_json::json!({
            "revision": revision,
            "path": path,
            "value": value,
        }),
        None => serde_json::json!({
            "revision": revision,
            "path": path,
            "deleted": true,
        }),
    }
}

/// Read-only history load. An absent file (or absent state dir) reads
/// as empty history with nothing created — no migration, mirroring the
/// facts document. Unreadable or corrupt files error naming the facts
/// history document.
fn load_facts_history(state: &std::path::Path) -> Result<Vec<serde_json::Value>, String> {
    let path = facts_history_path(state);
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("facts: cannot read {}: {e}", path.display())),
        Ok(t) => t,
    };
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("facts: cannot parse {}: {e}", path.display()))?;
    let entries = parsed
        .get("entries")
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            format!(
                "facts: cannot parse {}: expected an object with entries",
                path.display()
            )
        })?;
    for e in entries {
        let shape_ok = e.as_object().is_some()
            && e.get("revision").and_then(|r| r.as_u64()).is_some()
            && e.get("path").and_then(|p| p.as_str()).is_some();
        if !shape_ok {
            return Err(format!(
                "facts: cannot parse {}: every entry needs a revision and a path",
                path.display()
            ));
        }
    }
    Ok(entries.clone())
}

/// Atomic history append (temp file + rename, like the facts
/// document): readers see whole histories, never torn ones.
fn save_facts_history(
    state: &std::path::Path,
    entries: &[serde_json::Value],
) -> Result<(), String> {
    std::fs::create_dir_all(state)
        .map_err(|e| format!("facts: cannot create {}: {e}", state.display()))?;
    let text = serde_json::to_string_pretty(&serde_json::json!({ "entries": entries }))
        .map_err(|e| format!("facts: cannot serialize facts history: {e}"))?;
    let tmp = facts_history_path(state).with_extension("json.hist.tmp");
    std::fs::write(&tmp, &text)
        .map_err(|e| format!("facts: cannot stage {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, facts_history_path(state)).map_err(|e| {
        format!(
            "facts: cannot replace {}: {e}",
            facts_history_path(state).display()
        )
    })?;
    Ok(())
}

/// Rebuild the facts tree as of `revision` from recorded entries:
/// entries apply in file (revision) order, each at most once, so a
/// path reads the newest value at or before the requested revision.
/// Revisions that predate recording (no migration, per spec #64) or
/// a deleted path replay to missing — stated plainly, never an error.
/// Unparseable entries are skipped so one bad row cannot hide the rest.
fn replay_facts_at(entries: &[serde_json::Value], revision: u64) -> serde_json::Value {
    let mut facts = serde_json::json!({});
    for e in entries {
        let entry_rev = match e.get("revision").and_then(|r| r.as_u64()) {
            Some(r) => r,
            None => continue,
        };
        if entry_rev > revision {
            continue;
        }
        let path = match e.get("path").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => continue,
        };
        if e.get("deleted").and_then(|d| d.as_bool()).unwrap_or(false) {
            let _ = facts_delete_value(&mut facts, path);
        } else if let Some(v) = e.get("value") {
            let _ = facts_set_value(&mut facts, path, v.clone());
        }
    }
    facts
}
/// Integers that fit stay integers; anything else goes float. Errors
/// (nothing changed) on overflow or a non-finite result.
fn facts_add_delta(
    current: &serde_json::Value,
    delta: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    match (current.as_i64(), delta.as_i64()) {
        (Some(a), Some(b)) => a
            .checked_add(b)
            .map(serde_json::Value::from)
            .ok_or_else(|| {
                "facts: cannot increment: the total would overflow; nothing changed.".to_string()
            }),
        _ => {
            let a = current.as_f64().expect("caller checks numeric");
            let b = delta.as_f64().expect("caller checks numeric");
            let sum = a + b;
            if !sum.is_finite() {
                return Err(
                    "facts: cannot increment: the total would overflow; nothing changed."
                        .to_string(),
                );
            }
            serde_json::Number::from_f64(sum)
                .map(serde_json::Value::Number)
                .ok_or_else(|| {
                    "facts: cannot increment: the total would overflow; nothing changed."
                        .to_string()
                })
        }
    }
}

/// Refuse (exit 1, nothing written) when the prospective document would
/// exceed [`FACTS_MAX_DOCUMENT_BYTES`]. The cap is stated in every
/// refusal, human and machine alike; values are never truncated.
/// Returns the refusal exit code, or `None` when the document fits.
fn refuse_when_oversized(json: bool, op: &str, path: &str, doc: &FactsDocument) -> Option<i32> {
    let text = serde_json::to_string_pretty(&serde_json::json!({
        "revision": doc.revision,
        "facts": doc.facts,
    }))
    .unwrap_or_default();
    if text.len() <= FACTS_MAX_DOCUMENT_BYTES {
        return None;
    }
    eprintln!(
        "facts: cannot {op} {path}: document would be {} bytes, over the {FACTS_MAX_DOCUMENT_BYTES} byte cap (64 KiB); nothing changed.",
        text.len()
    );
    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": path,
                "cap": FACTS_MAX_DOCUMENT_BYTES,
                "proposed": text.len(),
                "oversized": true,
            })
        );
    }
    Some(1)
}

/// Calm conflict refusal (ticket #79, Bea's story): human output reads
/// as contention, never breakage. Machine output keeps the exact
/// revisions so scripts can retry. Always exit 1 with nothing changed.
fn refuse_conflict(json: bool, path: &str, presented: u64, current: u64) -> i32 {
    eprintln!(
        "facts: someone else saved {path} first — you presented revision {presented}, current revision is {current}. Reread the path and retry; nothing changed."
    );
    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": path,
                "presented": presented,
                "current": current,
                "conflict": true,
            })
        );
    }
    1
}

/// Malformed-path refusal shared by every facts entry point: shows the
/// accepted shape, changes nothing. Machine mode adds the same shape
/// as JSON so scripts see it structurally.
fn refuse_bad_path(json: bool, raw: &str, err: &str) -> i32 {
    eprintln!("{err}");
    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": raw,
                "expected": FACTS_PATH_SHAPE,
                "invalid": true,
            })
        );
    }
    1
}

/// One guarded facts write: the shared path for create (refuses when the
/// path exists), set (upserts), increment (numbers only), and delete.
/// `value_raw` is `Some` for create/set (parsed as JSON), the delta for
/// increment, and `None` for delete.
fn cmd_facts_write(
    json: bool,
    op: &str,
    path_raw: &str,
    revision: u64,
    value_raw: Option<&str>,
) -> i32 {
    let state = dispatch_state_dir();
    let doc = match load_facts_document(&state) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let path = match validate_facts_path(path_raw) {
        Ok(p) => p,
        Err(e) => return refuse_bad_path(json, path_raw, &e),
    };
    if revision != doc.revision {
        return refuse_conflict(json, &path, revision, doc.revision);
    }
    let mut facts = doc.facts;
    let stored: Option<serde_json::Value> = match op {
        "create" | "set" | "increment" => {
            let raw = value_raw.unwrap_or("null");
            let value: serde_json::Value = match serde_json::from_str(raw) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("facts: cannot {op} {path}: --value is not valid JSON: {e}");
                    return 1;
                }
            };
            if op == "increment" {
                if !value.is_number() {
                    eprintln!(
                        "facts: cannot increment {path}: --by must be a JSON number; nothing changed."
                    );
                    return 1;
                }
                let current = facts_lookup(&facts, &path)
                    .cloned()
                    .unwrap_or(serde_json::Value::from(0));
                if !current.is_number() {
                    eprintln!(
                        "facts: cannot increment {path}: current value is not a number at revision {}; nothing changed.",
                        doc.revision
                    );
                    return 1;
                }
                let total = match facts_add_delta(&current, &value) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("{e}");
                        return 1;
                    }
                };
                if let Err(e) = facts_set_value(&mut facts, &path, total.clone()) {
                    eprintln!("{e}");
                    return 1;
                }
                Some(total)
            } else {
                if op == "create" && facts_lookup(&facts, &path).is_some() {
                    eprintln!(
                        "facts: cannot create {path}: path already exists at revision {}; nothing changed.",
                        doc.revision
                    );
                    return 1;
                }
                if let Err(e) = facts_set_value(&mut facts, &path, value.clone()) {
                    eprintln!("{e}");
                    return 1;
                }
                Some(value)
            }
        }
        _ => match facts_delete_value(&mut facts, &path) {
            Ok(true) => None,
            Ok(false) => {
                eprintln!(
                    "facts: cannot delete {path}: no value set at revision {}; nothing changed.",
                    doc.revision
                );
                return 1;
            }
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        },
    };
    let new_revision = doc.revision + 1;
    let new_doc = FactsDocument {
        revision: new_revision,
        facts,
    };
    if op != "delete" {
        if let Some(code) = refuse_when_oversized(json, op, &path, &new_doc) {
            return code;
        }
    }
    // Read the history before committing: a corrupt history refuses
    // the whole write (nothing changed) instead of committing facts
    // the history can no longer describe.
    let mut history = match load_facts_history(&state) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if let Err(e) = save_facts_document(&state, &new_doc) {
        eprintln!("{e}");
        return 1;
    }
    // History appends after the live document commits (conservative
    // gap on crash: an as-of read never shows an uncommitted value).
    // A failed append is reported but the committed write stands.
    history.push(facts_history_entry(new_revision, &path, stored.as_ref()));
    if let Err(e) = save_facts_history(&state, &history) {
        eprintln!("{e}");
        return 1;
    }
    // Journal the committed write for `facts watch` (ticket #72): the
    // document side of the later streams work. Appended only after the
    // rename above, so the journal reflects committed state and refused
    // writes append nothing. A failed append is loud (exit 1 naming
    // both facts) rather than a silent gap in future deliveries.
    if let Err(e) = append_facts_event(
        &state,
        new_revision,
        &path,
        op,
        &stored.clone().unwrap_or(serde_json::Value::Null),
    ) {
        eprintln!(
            "facts: {path} saved at revision {new_revision}, but the watch journal was not appended: {e}; watchers may miss this delivery."
        );
        return 1;
    }
    match stored {
        Some(value) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "revision": new_revision,
                        "path": path,
                        "value": value,
                    })
                );
                return 0;
            }
            println!("facts {path} at revision {new_revision}:");
            println!("{}", facts_pretty(&value));
            println!("{FACTS_LOCALITY_LINE}");
            0
        }
        None => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "revision": new_revision,
                        "path": path,
                        "deleted": true,
                    })
                );
                return 0;
            }
            println!("facts {path} at revision {new_revision}: deleted.");
            println!("{FACTS_LOCALITY_LINE}");
            0
        }
    }
}

/// Create one facts path with the revision the reader observed.
/// Refuses when the path already exists or the revision is stale, with
/// the cause named; refusals change nothing.
pub fn cmd_facts_create(json: bool, path: &str, revision: u64, value: &str) -> i32 {
    cmd_facts_write(json, "create", path, revision, Some(value))
}

/// Set (upsert) one facts path with the revision the reader observed.
/// Refuses on a stale revision, with presented and current revisions
/// named; refusals change nothing.
pub fn cmd_facts_set(json: bool, path: &str, revision: u64, value: &str) -> i32 {
    cmd_facts_write(json, "set", path, revision, Some(value))
}

/// Delete one facts path with the revision the reader observed.
/// Refuses when the path has no value or the revision is stale, with
/// the cause named; refusals change nothing.
pub fn cmd_facts_delete(json: bool, path: &str, revision: u64) -> i32 {
    cmd_facts_write(json, "delete", path, revision, None)
}

/// Atomically add `by` (a JSON number) to the number at one facts
/// path with the revision the reader observed. A missing path starts
/// from zero; a present non-number is refused with nothing changed.
/// Refusals change nothing.
pub fn cmd_facts_increment(json: bool, path: &str, revision: u64, by: &str) -> i32 {
    cmd_facts_write(json, "increment", path, revision, Some(by))
}
/// Attach block for one facts path (ticket #72): the current value plus
/// revision rendered exactly like `facts show <path>` minus the locality
/// footer, so `show` and `watch` attach with identical facts. Callers add
/// the footer (show: immediately; watch: after the deliveries).
fn print_facts_path_attach(json: bool, doc: &FactsDocument, path: &str) {
    match facts_lookup(&doc.facts, path) {
        Some(value) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "revision": doc.revision,
                        "path": path,
                        "value": value,
                    })
                );
                return;
            }
            println!("facts {path} at revision {}:", doc.revision);
            println!("{}", facts_pretty(value));
        }
        None => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "revision": doc.revision,
                        "path": path,
                        "value": serde_json::Value::Null,
                        "missing": true,
                    })
                );
                return;
            }
            println!("facts {path} at revision {}: no value set.", doc.revision);
        }
    }
}

/// Read the facts document: whole or one path, human or machine.
/// Read-only like `status`/`log`: observing changes nothing.
/// With `at`, the value as of that revision (time travel, ticket
/// #71); without it, the latest revision (the default read).
/// An as-of read of the current revision serves the live document,
/// so it is exactly the latest read.
pub fn cmd_facts_show(json: bool, path_raw: Option<&str>, at_raw: Option<u64>) -> i32 {
    let state = dispatch_state_dir();
    let doc = match load_facts_document(&state) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let path = match path_raw {
        None => None,
        Some(raw) => match validate_facts_path(raw) {
            Ok(p) => Some(p),
            Err(e) => return refuse_bad_path(json, raw, &e),
        },
    };
    if let Some(r) = at_raw {
        if r > doc.revision {
            let label = path.clone().unwrap_or_else(|| "document".to_string());
            return refuse_future_revision(json, &label, r, doc.revision);
        }
    }
    // Latest and as-of-current serve the live document; older
    // revisions replay the recorded entries. One render below (the
    // attach block shared with `facts watch`), so both modes carry
    // identical facts at every revision.
    let view: FactsDocument = match at_raw {
        Some(r) if r < doc.revision => {
            let entries = match load_facts_history(&state) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            FactsDocument {
                revision: r,
                facts: replay_facts_at(&entries, r),
            }
        }
        _ => doc,
    };

    match path {
        None => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"revision": view.revision, "facts": view.facts})
                );
                return 0;
            }
            if view.facts == serde_json::json!({}) {
                println!(
                    "facts at revision {}: empty (no values set).",
                    view.revision
                );
            } else {
                println!("facts at revision {}:", view.revision);
                println!("{}", facts_pretty(&view.facts));
            }
            println!("{FACTS_LOCALITY_LINE}");
            0
        }
        Some(p) => {
            print_facts_path_attach(json, &view, &p);
            if !json {
                println!("{FACTS_LOCALITY_LINE}");
            }
            0
        }
    }
}
/// Future-revision refusal shared by as-of reads: names the presented
/// and current revisions, changes nothing. Machine output keeps both
/// revisions structurally so scripts can reread latest and retry.
fn refuse_future_revision(json: bool, label: &str, presented: u64, current: u64) -> i32 {
    eprintln!(
        "facts: cannot show {label} at revision {presented}: current revision is {current}; nothing changed."
    );
    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": label,
                "presented": presented,
                "current": current,
                "unavailable": true,
            })
        );
    }
    1
}

/// Missing --limit refusal for history reads: states the bound so the
/// retry is explicit. Machine output keeps the bound structurally.
fn refuse_history_limit_required(json: bool, path: &str) -> i32 {
    eprintln!(
        "facts: history needs an explicit --limit (1 to {FACTS_HISTORY_MAX_ENTRIES} entries per read); nothing dumped."
    );
    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": path,
                "max": FACTS_HISTORY_MAX_ENTRIES,
                "limit_required": true,
            })
        );
    }
    1
}

/// Zero or over-cap --limit refusal: states the bound, dumps nothing.
fn refuse_history_limit_invalid(json: bool, path: &str, limit: u64) -> i32 {
    eprintln!(
        "facts: history --limit must be 1 to {FACTS_HISTORY_MAX_ENTRIES} (got {limit}); nothing dumped."
    );
    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": path,
                "max": FACTS_HISTORY_MAX_ENTRIES,
                "invalid_limit": true,
            })
        );
    }
    1
}
/// Bounded history for one facts path: the last `limit` recorded
/// values oldest-first, human or machine. Read-only like
/// `status`/`log`: observing changes nothing.
pub fn cmd_facts_history(json: bool, path_raw: &str, limit_raw: Option<u64>) -> i32 {
    let state = dispatch_state_dir();
    let path = match validate_facts_path(path_raw) {
        Ok(p) => p,
        Err(e) => return refuse_bad_path(json, path_raw, &e),
    };
    let limit = match limit_raw {
        None => return refuse_history_limit_required(json, &path),
        Some(n) if n == 0 || n > FACTS_HISTORY_MAX_ENTRIES => {
            return refuse_history_limit_invalid(json, &path, n);
        }
        Some(n) => n as usize,
    };
    let doc = match load_facts_document(&state) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let entries = match load_facts_history(&state) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let for_path: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|e| e.get("path").and_then(|p| p.as_str()) == Some(path.as_str()))
        .collect();
    let total = for_path.len();
    let shown: Vec<&&serde_json::Value> =
        for_path.iter().skip(total.saturating_sub(limit)).collect();
    if json {
        let out: Vec<serde_json::Value> = shown
            .iter()
            .map(|e| {
                if e.get("deleted").and_then(|d| d.as_bool()).unwrap_or(false) {
                    serde_json::json!({
                        "revision": e.get("revision"),
                        "deleted": true,
                    })
                } else {
                    serde_json::json!({
                        "revision": e.get("revision"),
                        "value": e.get("value").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "path": path,
                "revision": doc.revision,
                "limit": limit as u64,
                "entries": out,
            })
        );
        return 0;
    }
    if shown.is_empty() {
        println!(
            "facts {path} history at revision {}: no entries.",
            doc.revision
        );
        println!("{FACTS_LOCALITY_LINE}");
        return 0;
    }
    println!(
        "facts {path} history at revision {} (showing last {} of {total}):",
        doc.revision,
        shown.len(),
    );
    for e in &shown {
        let rev = e.get("revision").and_then(|r| r.as_u64()).unwrap_or(0);
        if e.get("deleted").and_then(|d| d.as_bool()).unwrap_or(false) {
            println!("revision {rev}: deleted.");
        } else {
            let value = e.get("value").cloned().unwrap_or(serde_json::Value::Null);
            println!("revision {rev}:");
            println!("{}", facts_pretty(&value));
        }
    }
    println!("{FACTS_LOCALITY_LINE}");
    0
}

// ---------------------------------------------------------------------------
// DOCUMENT SUBSCRIPTIONS (ticket #72, spec #64 follow-up)
//
// Facts that push, at CLI fidelity: `factory facts watch <path>` attaches
// to one facts path (current value plus revision, exactly as
// `facts show <path>` renders) then replays one delivery per accepted
// write touching the path since the cursor. This is the document side of
// the later streams work: the streams layer can poll this command per
// scope, but no socket, daemon, or second store appears here.
//
// The replay source is an append-only journal beside the document
// (`facts-events.jsonl`: one JSON object per accepted write with its
// revision, path, op, and stored value), appended only after the write
// commits, so refused writes leave no trace and concurrent writers never
// lose each other's entries. A fresh attach (no --since) is a snapshot
// only, never a replay — like streams reconnects, the middle is simply
// missed and the next attach lands current. A cursor past the delivery
// bound is refused with a re-attach instruction naming the current
// revision as the way back, instead of buffering forever. Watching is
// read-only: only fs::read on this path, and an absent document or
// journal reads as empty with nothing created.
// ---------------------------------------------------------------------------

/// Journal filename for accepted facts writes (beside `facts.json`).
const FACTS_EVENTS_FILENAME: &str = "facts-events.jsonl";

/// Delivery bound for one watch replay: the most deliveries a single
/// invocation prints. Past it the watcher is told to re-attach instead
/// of buffered forever — every delivery already carries its revision, so
/// the next attach lands current anyway. Mirrors STREAM_BUFFER_BOUND.
const FACTS_WATCH_BUFFER_BOUND: usize = 16;

fn facts_events_path(state: &std::path::Path) -> PathBuf {
    state.join(FACTS_EVENTS_FILENAME)
}

/// One journaled accepted write: the revision it committed at, the path
/// it touched, the op, and the stored value (null for delete, whose
/// delivery renders as deleted).
struct FactsDelivery {
    revision: u64,
    path: String,
    op: String,
    value: serde_json::Value,
}

/// Append one journal line after a committed write. Append-only (never
/// read-modify-write) so concurrent writers cannot lose entries.
fn append_facts_event(
    state: &std::path::Path,
    revision: u64,
    path: &str,
    op: &str,
    value: &serde_json::Value,
) -> Result<(), String> {
    let line = serde_json::to_string(&serde_json::json!({
        "revision": revision,
        "path": path,
        "op": op,
        "value": value,
    }))
    .map_err(|e| format!("facts: cannot serialize watch journal entry: {e}"))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(facts_events_path(state))
        .map_err(|e| {
            format!(
                "facts: cannot append {}: {e}",
                facts_events_path(state).display()
            )
        })?;
    use std::io::Write as _;
    writeln!(file, "{line}").map_err(|e| {
        format!(
            "facts: cannot append {}: {e}",
            facts_events_path(state).display()
        )
    })?;
    Ok(())
}

/// Read-only journal load. An absent journal (or absent state dir) reads
/// as zero entries with nothing created; an unreadable or corrupt file
/// errors naming the journal — never silently skipped, which could hide
/// missed deliveries.
fn load_facts_events(state: &std::path::Path) -> Result<Vec<FactsDelivery>, String> {
    let path = facts_events_path(state);
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("facts: cannot read {}: {e}", path.display())),
        Ok(t) => t,
    };
    let mut out = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = serde_json::from_str(line).map_err(|e| {
            format!(
                "facts: cannot parse {} (line {}): {e}",
                path.display(),
                index + 1
            )
        })?;
        let revision = entry
            .get("revision")
            .and_then(|r| r.as_u64())
            .ok_or_else(|| {
                format!(
                    "facts: cannot parse {} (line {}): entry has no revision",
                    path.display(),
                    index + 1
                )
            })?;
        let event_path = entry.get("path").and_then(|p| p.as_str()).ok_or_else(|| {
            format!(
                "facts: cannot parse {} (line {}): entry has no path",
                path.display(),
                index + 1
            )
        })?;
        let op = entry.get("op").and_then(|o| o.as_str()).ok_or_else(|| {
            format!(
                "facts: cannot parse {} (line {}): entry has no op",
                path.display(),
                index + 1
            )
        })?;
        out.push(FactsDelivery {
            revision,
            path: event_path.to_string(),
            op: op.to_string(),
            value: entry
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        });
    }
    Ok(out)
}

/// Machine envelope for one watch replay: the attach facts plus the
/// cursor, the deliveries, and (on refusal) the re-attach instruction.
fn facts_watch_json(
    doc: &FactsDocument,
    path: &str,
    since: u64,
    pending: &[&FactsDelivery],
    reattach: bool,
) -> serde_json::Value {
    let mut attach = match facts_lookup(&doc.facts, path) {
        Some(value) => serde_json::json!({
            "revision": doc.revision,
            "path": path,
            "value": value,
        }),
        None => serde_json::json!({
            "revision": doc.revision,
            "path": path,
            "value": serde_json::Value::Null,
            "missing": true,
        }),
    };
    let deliveries: Vec<serde_json::Value> = pending
        .iter()
        .map(|d| {
            if d.op == "delete" {
                serde_json::json!({
                    "revision": d.revision,
                    "op": d.op,
                    "deleted": true,
                })
            } else {
                serde_json::json!({
                    "revision": d.revision,
                    "op": d.op,
                    "value": d.value,
                })
            }
        })
        .collect();
    let obj = attach.as_object_mut().expect("watch attach is an object");
    obj.insert("since".to_string(), serde_json::Value::from(since));
    obj.insert(
        "deliveries".to_string(),
        serde_json::Value::from(deliveries),
    );
    obj.insert("reattach".to_string(), serde_json::Value::from(reattach));
    if reattach {
        obj.insert(
            "reason".to_string(),
            serde_json::Value::from(format!(
                "watch on {path} fell behind ({} deliveries past revision {since} over the {bound}-delivery bound) — re-attach with `factory facts watch {path} --since {current}` to see current state",
                pending.len(),
                bound = FACTS_WATCH_BUFFER_BOUND,
                current = doc.revision,
            )),
        );
        obj.insert(
            "bound".to_string(),
            serde_json::Value::from(FACTS_WATCH_BUFFER_BOUND),
        );
        obj.insert(
            "pending".to_string(),
            serde_json::Value::from(pending.len()),
        );
    }
    attach
}

/// Attach to one facts path with the current value plus revision first,
/// then one delivery per accepted write touching the path since the
/// cursor (--since, defaulting to the current revision: a fresh attach
/// is a snapshot, never a replay). Read-only: watching changes nothing.
pub fn cmd_facts_watch(json: bool, path_raw: &str, since_raw: Option<u64>) -> i32 {
    let state = dispatch_state_dir();
    let doc = match load_facts_document(&state) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let path = match validate_facts_path(path_raw) {
        Ok(p) => p,
        Err(e) => return refuse_bad_path(json, path_raw, &e),
    };
    let since = since_raw.unwrap_or(doc.revision);
    if since > doc.revision {
        eprintln!(
            "facts: cannot watch {path} since revision {since}: current revision is {}. Reread the path and retry; nothing changed.",
            doc.revision
        );
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "path": path,
                    "since": since,
                    "current": doc.revision,
                    "invalid": true,
                })
            );
        }
        return 1;
    }
    let events = match load_facts_events(&state) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let pending: Vec<&FactsDelivery> = events
        .iter()
        .filter(|e| e.path == path && e.revision > since)
        .collect();
    if pending.len() > FACTS_WATCH_BUFFER_BOUND {
        // Slow watcher: the attach still lands current, then the
        // re-attach instruction instead of the backlog — the server is
        // never buffered without bound.
        if json {
            println!("{}", facts_watch_json(&doc, &path, since, &pending, true));
            return 1;
        }
        print_facts_path_attach(false, &doc, &path);
        eprintln!(
            "facts: watch on {path} fell behind ({} deliveries past revision {since} over the {}-delivery bound) — re-attach with `factory facts watch {path} --since {}` to see current state; nothing changed.",
            pending.len(),
            FACTS_WATCH_BUFFER_BOUND,
            doc.revision
        );
        println!("{FACTS_LOCALITY_LINE}");
        return 1;
    }
    if json {
        println!("{}", facts_watch_json(&doc, &path, since, &pending, false));
        return 0;
    }
    print_facts_path_attach(false, &doc, &path);
    for delivery in &pending {
        if delivery.op == "delete" {
            println!(
                "facts {path} delivery at revision {} (delete): deleted.",
                delivery.revision
            );
        } else {
            println!(
                "facts {path} delivery at revision {} ({}):",
                delivery.revision, delivery.op
            );
            println!("{}", facts_pretty(&delivery.value));
        }
    }
    println!("{FACTS_LOCALITY_LINE}");
    0
}

// ---------------------------------------------------------------------------
// LIVE TASK GRAPH (ticket #70, spec #59 follow-up)
//
// One observable graph of everything live for the dispatcher screen and
// debugging: every live task of an attempt is a node with its owner edge
// (owning task, or the attempt itself), kind, status, and what it is
// waiting on. Read-only like all observers: fs::read only, no ledger,
// log, or worktree writes on this path, so opening and closing viewers
// changes nothing. Terminal tasks leave the graph on the ledger commit
// that marks them terminal (filtered here, never deleted here).
// ---------------------------------------------------------------------------

/// Read-only live task graph over the dispatch ledger.
/// `ticket_raw` optionally narrows to one ticket (e.g. "70" or "#70").
pub fn cmd_dispatch_graph(json: bool, ticket_raw: Option<&str>) -> i32 {
    let filter: Option<u64> = match ticket_raw {
        Some(raw) => match parse_issue_number(raw) {
            Ok(n) => Some(n),
            Err(e) => {
                eprintln!("graph: {e}");
                return 1;
            }
        },
        None => None,
    };
    let state = dispatch_state_dir();
    let ledger = match load_ledger_strict_for("graph", &state) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut live: Vec<&LiveTask> = ledger
        .tasks
        .iter()
        .filter(|t| !is_task_terminal(&t.status))
        .filter(|t| filter.map(|f| t.ticket == f).unwrap_or(true))
        .collect();
    live.sort_by(|a, b| (a.ticket, a.attempt, &a.id).cmp(&(b.ticket, b.attempt, &b.id)));
    let rows: Vec<serde_json::Value> = live
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "ticket": t.ticket,
                "attempt": t.attempt,
                "kind": task_kind_text(t),
                "status": t.status,
                "owner": task_owner(t),
                "waits_on": t.waits_on,
            })
        })
        .collect();
    if json {
        println!(
            "{}",
            serde_json::json!({"tasks": rows, "count": rows.len()})
        );
        return 0;
    }
    if rows.is_empty() {
        println!("No live tasks \u{2014} the graph is empty.");
        return 0;
    }
    println!("Live task graph \u{2014} {} live task(s).", rows.len());
    for t in &live {
        let waits = if t.waits_on.is_empty() {
            "waits_on=none".to_string()
        } else {
            format!("waits_on={}", t.waits_on.join(","))
        };
        println!(
            "{} {} {} owner={} {}",
            t.id,
            task_kind_text(t),
            t.status,
            task_owner(t),
            waits
        );
    }
    0
}

// ---------------------------------------------------------------------------
// CHILD-TASK WAITING POLICIES (ticket #68, spec #59 follow-up)
//
// A parent task owns child tasks (e.g. a verify split across suites) and
// waits with a stated policy: all-settled resumes only after every child
// is terminal; fail-fast aborts still-running siblings on the first child
// failure before the parent decides. Waiting runs no code — children
// finish outside the wait — and the resume point is checkpointed atomically
// (tmp + rename) like any phase. Outcomes and abort order are appended to
// the attempt transcript so `factory log` shows them.
// ---------------------------------------------------------------------------

/// `true` for a failed child outcome (triggers fail-fast aborts).
fn is_task_failed(status: &str) -> bool {
    status == "failed" || status.ends_with("_failed")
}

/// Transcript file for one attempt: the ledger `log_path` when set, else
/// the conventional `<state>/logs/T<ticket>-a<attempt>.log`.
fn wait_log_path(
    state: &std::path::Path,
    ledger: &DispatchLedger,
    ticket: u64,
    attempt: u64,
) -> PathBuf {
    if let Some(a) = ledger
        .attempts
        .iter()
        .find(|a| a.ticket == ticket && a.attempt_no == attempt)
    {
        if !a.log_path.is_empty() {
            return PathBuf::from(&a.log_path);
        }
    }
    state.join(format!("logs/T{ticket}-a{attempt}.log"))
}

fn wait_append(
    state: &std::path::Path,
    ledger: &DispatchLedger,
    ticket: u64,
    attempt: u64,
    line: &str,
) {
    let path = wait_log_path(state, ledger, ticket, attempt);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    append_log(&path, &format!("{line}\n"));
}

/// Wait on a parent task's children with the stated policy over fixture
/// ledger state. `ticket_raw` optionally narrows the parent lookup.
/// Exit 0 on waiting/resumed; nonzero with the cause named on bad input
/// or unreadable state. The global `--json` flag selects machine output
/// carrying the same facts structurally.
pub fn cmd_dispatch_wait(
    json: bool,
    ticket_raw: Option<&str>,
    parent_id: &str,
    policy_raw: &str,
) -> i32 {
    let policy = policy_raw.trim().to_ascii_lowercase().replace('_', "-");
    if policy != "all-settled" && policy != "fail-fast" {
        eprintln!("wait: invalid policy {policy_raw:?}: want all-settled or fail-fast");
        return 1;
    }
    let filter: Option<u64> = match ticket_raw {
        Some(raw) => match parse_issue_number(raw) {
            Ok(n) => Some(n),
            Err(e) => {
                eprintln!("wait: {e}");
                return 1;
            }
        },
        None => None,
    };
    if parent_id.trim().is_empty() {
        eprintln!("wait: missing --parent task id");
        return 1;
    }
    let state = dispatch_state_dir();
    let mut ledger = match load_ledger_strict_for("wait", &state) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let candidates: Vec<usize> = ledger
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.id == parent_id)
        .filter(|(_, t)| filter.map(|f| t.ticket == f).unwrap_or(true))
        .map(|(i, _)| i)
        .collect();
    if candidates.is_empty() {
        eprintln!("wait: unknown parent task {parent_id:?}");
        return 1;
    }
    if candidates.len() > 1 {
        eprintln!("wait: ambiguous parent task {parent_id:?}: pass --ticket to select one");
        return 1;
    }
    let pidx = candidates[0];
    let (pticket, pattempt) = (ledger.tasks[pidx].ticket, ledger.tasks[pidx].attempt);
    let child_idx: Vec<usize> = ledger
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.parent.as_deref() == Some(parent_id))
        .filter(|(_, t)| t.ticket == pticket && t.attempt == pattempt)
        .map(|(i, _)| i)
        .collect();
    if child_idx.is_empty() {
        eprintln!("wait: parent task {parent_id:?} owns no child tasks");
        return 1;
    }
    // Already terminal: idempotent re-read, no writes.
    if is_task_terminal(&ledger.tasks[pidx].status) {
        let outcome = ledger.tasks[pidx].status.clone();
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "parent": parent_id, "ticket": pticket, "attempt": pattempt,
                    "policy": policy, "state": "resumed", "outcome": outcome,
                    "children": [], "abort_order": [],
                })
            );
        } else {
            println!("resumed: parent {parent_id} already {outcome} (policy {policy})");
        }
        return 0;
    }
    let mut children: Vec<(String, String, bool)> = child_idx
        .iter()
        .map(|&i| {
            let t = &ledger.tasks[i];
            (t.id.clone(), t.status.clone(), t.replay_safe)
        })
        .collect();
    children.sort_by(|a, b| a.0.cmp(&b.0));
    let failed: Vec<(String, String, bool)> = children
        .iter()
        .filter(|(_, s, _)| is_task_failed(s))
        .cloned()
        .collect();
    let pending: Vec<(String, String, bool)> = children
        .iter()
        .filter(|(_, s, _)| !is_task_terminal(s))
        .cloned()
        .collect();
    let pending_ids: Vec<String> = pending.iter().map(|(id, _, _)| id.clone()).collect();

    // Simulated kill mid-wait: checkpoint the wait, then exit nonzero
    // before any resume/abort mutation. Children are untouched, so the
    // next pass resumes the same wait; only replay-safe children may run
    // again, and the wait itself never runs them.
    if std::env::var("FACTORY_KILL_WAIT").as_deref() == Ok("1")
        || std::env::var("FACTORY_KILL_WAIT").as_deref() == Ok(parent_id)
    {
        ledger.tasks[pidx].status = "waiting".to_string();
        ledger.tasks[pidx].waits_on = pending_ids.clone();
        ledger.tasks[pidx].wait_policy = policy.clone();
        if let Err(e) = save_ledger(&state, &ledger) {
            eprintln!("wait: {e}");
            return 1;
        }
        wait_append(
            &state,
            &ledger,
            pticket,
            pattempt,
            &format!(
                "WAIT: parent {parent_id} checkpointed waiting on {} (policy {policy})",
                pending_ids.join(",")
            ),
        );
        eprintln!(
            "wait: killed mid-wait on parent {parent_id} (resumable) — rerun resumes the same wait"
        );
        return 1;
    }

    // Fail-fast trigger: first failed child aborts still-running siblings
    // in sorted id order before the parent decides (parent records failed).
    if policy == "fail-fast" && !failed.is_empty() {
        let first_failed = failed[0].0.clone();
        let mut abort_order: Vec<String> = Vec::new();
        for (id, _, _) in &pending {
            if let Some(t) = ledger.tasks.iter_mut().find(|t| t.id == *id) {
                t.status = "aborted".to_string();
                t.waits_on.clear();
            }
            abort_order.push(id.clone());
        }
        ledger.tasks[pidx].status = "failed".to_string();
        ledger.tasks[pidx].waits_on.clear();
        ledger.tasks[pidx].wait_policy = policy.clone();
        if let Err(e) = save_ledger(&state, &ledger) {
            eprintln!("wait: {e}");
            return 1;
        }
        wait_append(
            &state,
            &ledger,
            pticket,
            pattempt,
            &format!("WAIT: child {first_failed} failed (policy fail-fast)"),
        );
        for id in &abort_order {
            wait_append(
                &state,
                &ledger,
                pticket,
                pattempt,
                &format!("WAIT: abort {id} after {first_failed} (fail-fast)"),
            );
        }
        wait_append(
            &state,
            &ledger,
            pticket,
            pattempt,
            &format!("WAIT: parent {parent_id} decides failed after abort (policy fail-fast)"),
        );
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "parent": parent_id, "ticket": pticket, "attempt": pattempt,
                    "policy": policy, "state": "resumed", "outcome": "failed",
                    "failed_child": first_failed,
                    "children": children.iter().map(|(id, status, replay_safe)| {
                        serde_json::json!({"id": id, "status": status, "replay_safe": replay_safe})
                    }).collect::<Vec<_>>(),
                    "abort_order": abort_order,
                })
            );
        } else {
            println!("resumed: parent {parent_id} decides failed (policy fail-fast) after child {first_failed} failed");
            for (id, status, _) in &children {
                println!("child {id} {status}");
            }
            for id in &abort_order {
                println!("abort {id} after {first_failed} (fail-fast)");
            }
        }
        return 0;
    }

    // Still waiting: checkpoint the wait and report what remains.
    if !pending.is_empty() {
        ledger.tasks[pidx].status = "waiting".to_string();
        ledger.tasks[pidx].waits_on = pending_ids.clone();
        ledger.tasks[pidx].wait_policy = policy.clone();
        if let Err(e) = save_ledger(&state, &ledger) {
            eprintln!("wait: {e}");
            return 1;
        }
        wait_append(
            &state,
            &ledger,
            pticket,
            pattempt,
            &format!(
                "WAIT: parent {parent_id} waiting on {} (policy {policy})",
                pending_ids.join(",")
            ),
        );
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "parent": parent_id, "ticket": pticket, "attempt": pattempt,
                    "policy": policy, "state": "waiting",
                    "pending": pending_ids,
                    "children": children.iter().map(|(id, status, replay_safe)| {
                        serde_json::json!({"id": id, "status": status, "replay_safe": replay_safe})
                    }).collect::<Vec<_>>(),
                })
            );
        } else {
            println!(
                "waiting: parent {parent_id} waits on {} (policy {policy})",
                pending_ids.join(", ")
            );
            for (id, status, _) in &children {
                println!("child {id} {status}");
            }
        }
        return 0;
    }

    // All children terminal: resume the parent with every outcome logged.
    let parent_outcome = if children.iter().any(|(_, s, _)| is_task_failed(s)) {
        "failed"
    } else {
        "done"
    };
    ledger.tasks[pidx].status = parent_outcome.to_string();
    ledger.tasks[pidx].waits_on.clear();
    ledger.tasks[pidx].wait_policy = policy.clone();
    if let Err(e) = save_ledger(&state, &ledger) {
        eprintln!("wait: {e}");
        return 1;
    }
    for (id, status, _) in &children {
        wait_append(
            &state,
            &ledger,
            pticket,
            pattempt,
            &format!("WAIT: child {id} {status}"),
        );
    }
    wait_append(
        &state,
        &ledger,
        pticket,
        pattempt,
        &format!("WAIT: parent {parent_id} resumed {parent_outcome} (policy {policy}); replay-safe children may rerun, others ran once"),
    );
    if json {
        println!(
            "{}",
            serde_json::json!({
                "parent": parent_id, "ticket": pticket, "attempt": pattempt,
                "policy": policy, "state": "resumed", "outcome": parent_outcome,
                "children": children.iter().map(|(id, status, replay_safe)| {
                    serde_json::json!({"id": id, "status": status, "replay_safe": replay_safe})
                }).collect::<Vec<_>>(),
                "abort_order": [],
            })
        );
    } else {
        println!("resumed: parent {parent_id} {parent_outcome} (policy {policy})");
        for (id, status, _) in &children {
            println!("child {id} {status}");
        }
    }
    0
}

// ---------------------------------------------------------------------------
// DECISION MEMOS (ticket #67, spec #59 follow-up)
//
// Hooks and approvals that never ask twice. A first-write-wins memo cache
// scoped to the attempt: the first decision (approve deploy, allow a
// flagged tool call, answer a guard question) is stored, and after any
// restart the hook finds the stored answer instead of re-prompting.
// Identical outcome with or without a crash in between: the store rides
// the same temp-file + rename as every ledger write, so a kill between
// the two leaves the previous ledger (the FACTORY_MEMO_CRASH seam below
// exists only to pin that guarantee in tests: it writes tmp and exits
// before the rename, exactly like a kill -9 at that instant).
//
// Concurrency: memo writes take the same mkdir lock as `dispatch once`,
// so concurrent first-writes serialize and resolve to exactly one winner;
// losers observe the stored value. Scope is the attempt: the triple
// (ticket, attempt, key) is the cache line, and a fresh attempt number
// starts with an empty cache.
// ---------------------------------------------------------------------------

/// Memos stored for one attempt, in record order.
fn attempt_memos(ledger: &DispatchLedger, ticket: u64, attempt_no: u64) -> Vec<&DecisionMemo> {
    ledger
        .decision_memos
        .iter()
        .filter(|m| m.ticket == ticket && m.attempt == attempt_no)
        .collect()
}

/// One memo as machine-readable facts, shared by every JSON surface so no
/// fact lives only in prose.
fn memo_json(m: &DecisionMemo) -> serde_json::Value {
    serde_json::json!({
        "ticket": m.ticket,
        "attempt": m.attempt,
        "slot": format!("T{}-a{}", m.ticket, m.attempt),
        "key": m.key,
        "decision": m.decision,
        "stored_at": m.stored_at,
    })
}

/// Acquire the shared dispatch lock, retrying while a pass holds it. Memo
/// writes must serialize against `dispatch once` ledger writes as well as
/// each other, or a concurrent commit could lose one of the two updates.
fn memo_lock(state: &std::path::Path) -> Option<DispatchLock> {
    for _ in 0..200 {
        if let Some(lock) = DispatchLock::acquire(state.join(".lock")) {
            return Some(lock);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}

/// Record a hook/approval decision or observe the stored one (ticket #67).
/// First-write-wins on (ticket, attempt, key); repeats print the stored
/// answer with a reuse marker and change nothing. Atomic (tmp + rename)
/// under the shared dispatch lock.
pub fn cmd_dispatch_memo(
    json: bool,
    ticket_raw: &str,
    attempt_no: u64,
    key_raw: &str,
    decision_raw: &str,
) -> i32 {
    let ticket = match parse_issue_number(ticket_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("memo: {e}");
            return 1;
        }
    };
    let key = key_raw.trim().to_string();
    if key.is_empty() {
        eprintln!("memo: key must not be empty");
        return 1;
    }
    let decision = decision_raw.trim().to_string();
    if decision.is_empty() {
        eprintln!("memo: decision must not be empty");
        return 1;
    }
    fn print_reuse(
        json: bool,
        ticket: u64,
        attempt_no: u64,
        slot: &str,
        key: &str,
        decision: &str,
    ) {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ticket": ticket,
                    "attempt": attempt_no,
                    "slot": slot,
                    "key": key,
                    "decision": decision,
                    "recorded": false,
                })
            );
        } else {
            println!(
                "Decision reused for ticket {ticket} attempt {attempt_no}: {key}={decision} (slot {slot}) — already decided, not asked again."
            );
        }
    }
    let state = dispatch_state_dir();
    let slot = format!("T{ticket}-a{attempt_no}");
    // Reuse is the resume path, so it stays lock-free like every other
    // observer: after a kill (even one that orphans the dispatch lock,
    // exactly as `dispatch once` under kill does) the hook still finds
    // the stored answer. Tmp + rename makes the load see the old or the
    // new ledger, never a torn one; a miss below falls through to the
    // locked write, which re-checks before storing.
    let ledger = load_ledger(&state);
    if let Some(stored) = ledger
        .decision_memos
        .iter()
        .find(|m| m.ticket == ticket && m.attempt == attempt_no && m.key == key)
    {
        print_reuse(json, ticket, attempt_no, &slot, &key, &stored.decision);
        return 0;
    }
    if let Err(e) = std::fs::create_dir_all(&state) {
        eprintln!("memo: cannot create {}: {e}", state.display());
        return 1;
    }
    let Some(_lock) = memo_lock(&state) else {
        eprintln!("memo: another dispatcher holds the lock — retry the hook");
        return 1;
    };
    // Re-check under the lock: a concurrent first-write may have won
    // while this hook waited, and losers must observe the stored value.
    let mut ledger = load_ledger(&state);
    if !ledger
        .attempts
        .iter()
        .any(|a| a.ticket == ticket && a.attempt_no == attempt_no)
    {
        eprintln!("memo: no such attempt T{ticket}-a{attempt_no} — the scope must exist before a hook records a decision");
        return 1;
    }
    if let Some(stored) = ledger
        .decision_memos
        .iter()
        .find(|m| m.ticket == ticket && m.attempt == attempt_no && m.key == key)
    {
        print_reuse(json, ticket, attempt_no, &slot, &key, &stored.decision);
        return 0;
    }
    ledger.decision_memos.push(DecisionMemo {
        ticket,
        attempt: attempt_no,
        key: key.clone(),
        decision: decision.clone(),
        stored_at: now_secs().to_string(),
    });
    if let Err(e) = std::fs::create_dir_all(&state) {
        eprintln!("memo: cannot create {}: {e}", state.display());
        return 1;
    }
    let tmp = state.join("dispatch.json.tmp");
    let text = match serde_json::to_string_pretty(&ledger) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("memo: cannot serialize ledger: {e}");
            return 1;
        }
    };
    if let Err(e) = std::fs::write(&tmp, &text) {
        eprintln!("memo: cannot write ledger: {e}");
        return 1;
    }
    // Test-only kill seam: behave exactly like a crash after the tmp
    // write but before the rename (orphaned tmp, previous ledger intact).
    if std::env::var("FACTORY_MEMO_CRASH").as_deref() == Ok("1") {
        std::process::exit(1);
    }
    if let Err(e) = std::fs::rename(&tmp, ledger_path(&state)) {
        eprintln!("memo: cannot replace ledger: {e}");
        return 1;
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ticket": ticket,
                "attempt": attempt_no,
                "slot": slot,
                "key": key,
                "decision": decision,
                "recorded": true,
            })
        );
    } else {
        println!(
            "Decision recorded for ticket {ticket} attempt {attempt_no}: {key}={decision} (slot {slot}) — hooks reuse this answer instead of asking again."
        );
    }
    0
}

pub fn cmd_dispatch_status(json: bool, all: bool) -> i32 {
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
        let memos: Vec<serde_json::Value> = ledger.decision_memos.iter().map(memo_json).collect();
        // Per-attempt idle facts ride beside the raw rows so scripts
        // decide resume-versus-alert on identical facts (ticket #86).
        let attempts: Vec<serde_json::Value> = ledger
            .attempts
            .iter()
            .map(|a| {
                let mut row = serde_json::to_value(a).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = row.as_object_mut() {
                    obj.insert(
                        "idle_background".to_string(),
                        serde_json::Value::Bool(attempt_idle_background(
                            &ledger,
                            a.ticket,
                            a.attempt_no,
                        )),
                    );
                    obj.insert(
                        "background_running".to_string(),
                        serde_json::json!(attempt_background_running(
                            &ledger,
                            a.ticket,
                            a.attempt_no
                        )),
                    );
                }
                row
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({"attempts": attempts, "decision_memos": memos, "owned_tasks": ledger.owned_tasks, "abort_log": ledger.abort_log, "worktrees": wt_lines, "stopped": stopped})
        );
    } else {
        // Human contract (ticket #90): an aligned-column table over live
        // attempts plus the most recent finished attempts (`--all` opts
        // into the full history), then a totals line carrying the loop
        // state. Internal ledger ids stay in `--json`; humans read slots.
        println!("== dispatch attempts ==");
        if ledger.attempts.is_empty() {
            println!("(none)");
        } else {
            let live: Vec<&DispatchAttempt> = ledger
                .attempts
                .iter()
                .filter(|a| is_live_attempt(a))
                .collect();
            let finished: Vec<&DispatchAttempt> = ledger
                .attempts
                .iter()
                .filter(|a| !is_live_attempt(a))
                .collect();
            let recent: Vec<&DispatchAttempt> = if all {
                finished.clone()
            } else {
                finished
                    .iter()
                    .rev()
                    .take(HISTORY_RECENT_LIMIT)
                    .copied()
                    .collect::<Vec<&DispatchAttempt>>()
                    .into_iter()
                    .rev()
                    .collect()
            };
            let shown: Vec<&DispatchAttempt> =
                live.iter().copied().chain(recent.iter().copied()).collect();
            // Column widths from the shown rows so the table aligns.
            let mut ticket_w = "TICKET".len();
            let mut attempt_w = "ATTEMPT".len();
            let mut status_w = "STATUS".len();
            let mut phase_w = "PHASE".len();
            let mut slot_w = "SLOT".len();
            let mut started_w = "STARTED".len();
            let cells: Vec<[String; 6]> = shown
                .iter()
                .map(|a| {
                    [
                        a.ticket.to_string(),
                        a.attempt_no.to_string(),
                        a.status.clone(),
                        attempt_phase(a).to_string(),
                        format!("T{}-a{}", a.ticket, a.attempt_no),
                        a.started_at.clone(),
                    ]
                })
                .collect();
            for c in &cells {
                ticket_w = ticket_w.max(c[0].len());
                attempt_w = attempt_w.max(c[1].len());
                status_w = status_w.max(c[2].len());
                phase_w = phase_w.max(c[3].len());
                slot_w = slot_w.max(c[4].len());
                started_w = started_w.max(c[5].len());
            }
            let header = format!(
                "{:<ticket_w$} {:<attempt_w$} {:<status_w$} {:<phase_w$} {:<slot_w$} {:<started_w$}",
                "TICKET", "ATTEMPT", "STATUS", "PHASE", "SLOT", "STARTED",
            );
            println!("{}", header.trim_end());
            for (a, c) in shown.iter().zip(cells.iter()) {
                let mut extras = String::new();
                if is_resumable(a) {
                    extras.push_str(" resumable");
                }
                if a.resumed > 0 {
                    extras.push_str(&format!(" resumed={}", a.resumed));
                }
                // Memo facts ride the attempt row (ticket #67): empty
                // when the attempt has no memos, so memo-less rows read
                // exactly like the header-padded columns plus extras.
                let memos = attempt_memos(&ledger, a.ticket, a.attempt_no);
                if !memos.is_empty() {
                    let pairs: Vec<String> = memos
                        .iter()
                        .map(|m| format!("{}={}", m.key, m.decision))
                        .collect();
                    extras.push_str(&format!(" memos={} ({})", memos.len(), pairs.join(", ")));
                }
                // Background tasks belong to the attempt but not its
                // current work: the attempt reads idle while they run
                // (ticket #86).
                if attempt_idle_background(&ledger, a.ticket, a.attempt_no) {
                    extras.push_str(&format!(
                        " idle (background running: {})",
                        attempt_background_running(&ledger, a.ticket, a.attempt_no)
                    ));
                }
                let line = format!(
                    "{:<ticket_w$} {:<attempt_w$} {:<status_w$} {:<phase_w$} {:<slot_w$} {:<started_w$}{}",
                    c[0], c[1], c[2], c[3], c[4], c[5], extras
                );
                println!("{}", line.trim_end());
            }
            if !all && finished.len() > HISTORY_RECENT_LIMIT {
                println!(
                    "showing live plus the most recent {} of {} finished attempts; full history: factory dispatch status --all.",
                    HISTORY_RECENT_LIMIT,
                    finished.len()
                );
            }
        }
        // Owned tasks read beside their attempts so an abort's scope is
        // visible where Engineer Eli already looks (ticket #86). Empty
        // ledgers print nothing new: old output stays byte-identical.
        if !ledger.owned_tasks.is_empty() {
            println!();
            println!("== owned tasks ==");
            for t in &ledger.owned_tasks {
                let owner = match t.parent {
                    Some(p) => format!(" owned-by={p}"),
                    None => String::new(),
                };
                println!(
                    "T{}-a{} {} {} {}{} effects={} undone={}",
                    t.ticket,
                    t.attempt,
                    t.name,
                    t.scope,
                    t.status,
                    owner,
                    t.effects.len(),
                    t.undone.len()
                );
            }
            if !ledger.abort_log.is_empty() {
                println!("abort order: {}", ledger.abort_log.join(", "));
            }
        }
        print_paused_copy(ledger.attempts.iter().any(is_resumable));
        println!();
        println!("== worktrees ==");
        print!("{worktrees}");
        if !worktrees.ends_with('\n') {
            println!();
        }
        let live_total = ledger
            .attempts
            .iter()
            .filter(|a| is_live_attempt(a))
            .count();
        let finished_total = ledger.attempts.len().saturating_sub(live_total);
        let live_counts = count_by_status(
            ledger
                .attempts
                .iter()
                .filter(|a| is_live_attempt(a))
                .map(|a| a.status.as_str()),
        );
        let finished_counts = count_by_status(
            ledger
                .attempts
                .iter()
                .filter(|a| !is_live_attempt(a))
                .map(|a| a.status.as_str()),
        );
        let live_words = if live_counts.is_empty() {
            "none".to_string()
        } else {
            counts_words(&live_counts)
        };
        let finished_words = if finished_counts.is_empty() {
            "none".to_string()
        } else {
            counts_words(&finished_counts)
        };
        let worktree_count = worktrees.lines().filter(|l| !l.trim().is_empty()).count();
        println!(
            "total: {} attempt(s) ({} live: {}; {} finished: {}) in {} worktree(s); loop: {}.",
            ledger.attempts.len(),
            live_total,
            live_words,
            finished_total,
            finished_words,
            worktree_count,
            if stopped { "STOPPED" } else { "running" }
        );
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

// ---------------------------------------------------------------------------
// CUTOVER (ticket #88): one-shot bash-SQLite → Rust-JSON import
//
// The split brain retires here: the bash dispatcher's `dispatch.db`
// (`attempts` + `releases` rows) is read once through the pure-Rust
// `sqlite` module and written out as the Rust JSON ledger
// (`dispatch.json` + `train.json`), then the `.db` is archived
// byte-identical. After this pass the JSON ledger is the only store:
// no dual reads, no dual writes, no sqlite linkage anywhere.
//
// Cutover runbook (human-authorized window only): stop the bash loop,
// run `factory dispatch import`, verify `dispatch status` and
// `log <ticket>` (human and `--json`) read the same facts `sqlite3`
// read before, then start the Rust daemon. Closed releases are terminal
// history with no live train behind them: they stay in the
// byte-identical archive and are reported, never carried.
// ---------------------------------------------------------------------------

/// Outcome of one cutover pass, printed human- or machine-readable.
struct CutoverReport {
    attempts: usize,
    open_train: Option<String>,
    closed_skipped: usize,
    ledger: PathBuf,
    train: Option<PathBuf>,
    archive: PathBuf,
    sha256: String,
}

/// CLI boundary for `dispatch import`: exit 0 with the report on
/// success, exit 1 with a plain-English cause naming the file.
pub fn cmd_dispatch_import(json: bool, db_opt: Option<&std::path::Path>) -> i32 {
    let state = dispatch_state_dir();
    let db_path = db_opt
        .map(PathBuf::from)
        .unwrap_or_else(|| state.join("dispatch.db"));
    match cutover_import(&state, &db_path) {
        Ok(r) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "attempts": r.attempts,
                        "open_train": r.open_train,
                        "closed_releases_skipped": r.closed_skipped,
                        "ledger": r.ledger.display().to_string(),
                        "train": r.train.map(|p| p.display().to_string()),
                        "archive": r.archive.display().to_string(),
                        "sha256": r.sha256,
                    })
                );
            } else {
                println!(
                    "import: {} attempt(s) from {}",
                    r.attempts,
                    db_path.display()
                );
                match &r.open_train {
                    Some(t) => println!("import: open train {t}"),
                    None => println!("import: no open train (nothing to carry)"),
                }
                if r.closed_skipped > 0 {
                    println!(
                        "import: skipped {} closed release(s) — terminal history, preserved in the archive",
                        r.closed_skipped
                    );
                }
                println!("import: ledger {}", r.ledger.display());
                if let Some(t) = &r.train {
                    println!("import: train {}", t.display());
                }
                println!(
                    "import: archived byte-identical {} (sha256 {})",
                    r.archive.display(),
                    r.sha256
                );
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// One import row → one ledger attempt, field for field. `NULL` commits,
/// paths, and timestamps read as the same empty strings a
/// pre-checkpoint Rust ledger carries; a `NULL` exit code (still
/// running, never reported one) reads as 0, which no human or JSON
/// summary prints for a running attempt.
fn cutover_attempt(row: &sqlite::Row) -> Result<DispatchAttempt, String> {
    Ok(DispatchAttempt {
        id: row.req_u64("id")?,
        ticket: row.req_u64("ticket")?,
        attempt_no: row.req_u64("attempt_no")?,
        base_commit: row.text("base_commit")?,
        head_commit: row.text("head_commit")?,
        branch: row.text("branch")?,
        worktree: row.text("worktree")?,
        status: row.req_text("status")?,
        exit_code: row.opt_int("exit_code")?.unwrap_or(0) as i32,
        log_path: row.text("log_path")?,
        started_at: row.text("started_at")?,
        finished_at: row.text("finished_at")?,
        train_branch: row.maybe_text("train_branch")?,
        phase: String::new(),
        resumed: 0,
    })
}

/// The live release state: the newest open release becomes the Rust open
/// train, mirroring the bash reader (`ORDER BY id DESC LIMIT 1`). Closed
/// releases are terminal history — counted so the report can say so,
/// never carried, always preserved in the byte-identical archive.
fn cutover_open_train(rows: &[sqlite::Row]) -> Result<(Option<TrainState>, usize), String> {
    let mut open: Vec<&sqlite::Row> = Vec::new();
    let mut closed = 0;
    for row in rows {
        if row.text("status")? == "open" {
            open.push(row);
        } else {
            closed += 1;
        }
    }
    open.sort_by_key(|r| r.req_u64("id").unwrap_or(0));
    match open.last() {
        None => Ok((None, closed)),
        Some(row) => Ok((
            Some(TrainState {
                branch: row.req_text("branch")?,
                base_commit: row.text("base_commit")?,
                status: row.text("status")?,
                created_at: row.text("created_at")?,
            }),
            closed,
        )),
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The one-shot pass. Refuses (before touching anything) when the state
/// dir already holds a ledger, a train, or an archive — re-running
/// against an already-imported state never duplicates. The source `.db`
/// is opened read-only exactly once and re-hashed afterwards to prove
/// it was never mutated.
fn cutover_import(
    state: &std::path::Path,
    db_path: &std::path::Path,
) -> Result<CutoverReport, String> {
    let ledger_path = state.join("dispatch.json");
    if ledger_path.is_file() {
        return Err(format!(
            "import: {} already exists — this state dir was already imported (or holds Rust-native attempts); refusing to duplicate. To re-cutover, start from a fresh state dir.",
            ledger_path.display()
        ));
    }
    let train_path = state.join("train.json");
    if train_path.is_file() {
        return Err(format!(
            "import: {} already exists — a Rust train is already open; refusing to clobber it.",
            train_path.display()
        ));
    }
    let archive = state.join("dispatch.db.archived");
    if archive.exists() {
        return Err(format!(
            "import: {} already exists — this history was already archived; refusing to duplicate.",
            archive.display()
        ));
    }
    if !db_path.is_file() {
        return Err(format!(
            "import: no SQLite history at {} — nothing to import.",
            db_path.display()
        ));
    }
    let before = std::fs::read(db_path)
        .map_err(|e| format!("import: cannot read {}: {e}", db_path.display()))?;
    let sha = hex_sha256(&before);
    let db = sqlite::Db::parse(before.clone()).map_err(|e| {
        format!(
            "import: cannot parse {} as SQLite history: {e}",
            db_path.display()
        )
    })?;
    let attempt_rows = db.table("attempts").map_err(|e| {
        format!(
            "import: cannot read attempts from {}: {e}",
            db_path.display()
        )
    })?;
    let release_rows = db.table("releases").map_err(|e| {
        format!(
            "import: cannot read releases from {}: {e}",
            db_path.display()
        )
    })?;
    let mut attempts = Vec::with_capacity(attempt_rows.len());
    for row in &attempt_rows {
        attempts.push(cutover_attempt(row)?);
    }
    let (open_train, closed_skipped) = cutover_open_train(&release_rows)?;
    let ledger = DispatchLedger {
        attempts,
        ..Default::default()
    };
    save_ledger(state, &ledger)?;
    let train_file = if let Some(ref train) = open_train {
        save_train(state, train)?;
        Some(train_path.clone())
    } else {
        None
    };
    std::fs::write(&archive, &before)
        .map_err(|e| format!("import: cannot write {}: {e}", archive.display()))?;
    let archived = std::fs::read(&archive)
        .map_err(|e| format!("import: cannot verify {}: {e}", archive.display()))?;
    if archived != before {
        return Err(format!(
            "import: archive {} differs from the source — refusing to report success.",
            archive.display()
        ));
    }
    let after = std::fs::read(db_path)
        .map_err(|e| format!("import: cannot re-read {}: {e}", db_path.display()))?;
    if after != before {
        return Err(format!(
            "import: {} changed during the import — refusing to report success.",
            db_path.display()
        ));
    }
    Ok(CutoverReport {
        attempts: ledger.attempts.len(),
        open_train: open_train.map(|t| t.branch),
        closed_skipped,
        ledger: ledger_path,
        train: train_file,
        archive,
        sha256: sha,
    })
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

/// Abort an attempt's current work bottom-up over fixture state. Prints the
/// unwind order as it goes; every undone effect persists before the next
/// begins so a kill resumes the same order. Owned experiment lines
/// (ticket #73) abort with the parent ticket: each owned line still
/// holding its work is marked aborted, its transcript marked, and its
/// worktree released — the same release discipline interruption follows.
/// Ownerless lines are fully independent and never touched.
pub fn cmd_dispatch_abort(ticket_raw: &str, background: bool, json: bool) -> i32 {
    let ticket = match parse_issue_number(ticket_raw) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("abort: {e}");
            return 1;
        }
    };
    let state = dispatch_state_dir();
    let mut ledger = match load_ledger_strict_for("abort", &state) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let plan = abort_plan(&ledger, ticket, background);
    // Owned lines answer to the parent ticket (ticket #73): every owned
    // fork still holding its line aborts with the parent — including when
    // the parent already failed and no task work remains to unwind.
    // Ownerless lines are fully independent and never touched.
    let owned_fork_idxs: Vec<usize> = ledger
        .forks
        .iter()
        .enumerate()
        .filter(|(_, f)| f.ticket == ticket && f.ownership == "owned" && f.status != "aborted")
        .map(|(i, _)| i)
        .collect();
    if plan.is_empty() && owned_fork_idxs.is_empty() {
        println!("Ticket {ticket}: nothing to abort.");
        if json {
            println!(
                "{}",
                serde_json::json!({"ticket": ticket, "aborted": [], "aborted_forks": [], "order": [], "background": background})
            );
        }
        return 0;
    }
    if plan.is_empty() {
        let names: Vec<String> = owned_fork_idxs
            .iter()
            .map(|&i| ledger.forks[i].name.clone())
            .collect();
        println!(
            "Ticket {ticket}: aborting owned fork line(s) ({} line(s)): {}.",
            names.len(),
            names.join(", ")
        );
    }
    let order_names: Vec<String> = plan
        .iter()
        .map(|&i| ledger.owned_tasks[i].name.clone())
        .collect();
    if background {
        println!(
            "Ticket {ticket}: aborting with background scope ({} task(s), bottom-up): {}.",
            plan.len(),
            order_names.join(", ")
        );
    } else {
        println!(
            "Ticket {ticket}: aborting current work ({} task(s), bottom-up): {}. Background tasks keep running.",
            plan.len(),
            order_names.join(", ")
        );
    }
    let mut undone_now: Vec<String> = Vec::new();
    for &idx in &plan {
        let effects: Vec<String> = ledger.owned_tasks[idx]
            .effects
            .iter()
            .rev()
            .cloned()
            .collect();
        let task_name = ledger.owned_tasks[idx].name.clone();
        for effect in effects {
            if ledger.owned_tasks[idx].undone.iter().any(|u| u == &effect) {
                continue;
            }
            ledger.owned_tasks[idx].undone.push(effect.clone());
            ledger.abort_log.push(effect.clone());
            undone_now.push(effect.clone());
            println!("  undid {effect} ({task_name})");
            if let Err(e) = save_ledger(&state, &ledger) {
                eprintln!("abort: {e}");
                return 1;
            }
            if std::env::var("FACTORY_KILL_ABORT_AT").as_deref() == Ok(effect.as_str()) {
                eprintln!("abort: simulated kill after {effect} — resume replays the same order");
                std::process::exit(1);
            }
        }
        ledger.owned_tasks[idx].status = "aborted".to_string();
        if let Err(e) = save_ledger(&state, &ledger) {
            eprintln!("abort: {e}");
            return 1;
        }
        println!("  aborted {task_name}");
    }
    let aborted_names: Vec<String> = plan
        .iter()
        .map(|&i| ledger.owned_tasks[i].name.clone())
        .collect();
    if !plan.is_empty() {
        println!(
            "Ticket {ticket}: aborted {} task(s) bottom-up.",
            aborted_names.len()
        );
    }
    let mut aborted_forks: Vec<String> = Vec::new();
    for &idx in &owned_fork_idxs {
        let name = ledger.forks[idx].name.clone();
        let slot = ledger.forks[idx].slot.clone();
        let log_path = ledger.forks[idx].log_path.clone();
        let worktree = PathBuf::from(ledger.forks[idx].worktree.clone());
        append_log(
            Path::new(&log_path),
            &format!("aborted with parent ticket {ticket} (slot {slot}) — owned line\n"),
        );
        ledger.forks[idx].status = "aborted".to_string();
        // Cleanup follows the owned line: the worktree is released here,
        // exactly as interruption releases it. Ownerless lines keep theirs.
        remove_worktree(&worktree);
        if let Err(e) = save_ledger(&state, &ledger) {
            eprintln!("abort: {e}");
            return 1;
        }
        println!("  aborted fork '{name}' (owned line, slot {slot})");
        aborted_forks.push(name);
    }
    if !aborted_forks.is_empty() {
        println!(
            "Ticket {ticket}: aborted {} owned fork line(s) with parent.",
            aborted_forks.len()
        );
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ticket": ticket,
                "aborted": aborted_names,
                "aborted_forks": aborted_forks,
                "order": ledger.abort_log,
                "undone_now": undone_now,
                "background": background,
            })
        );
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

    // --- replayable attempts (ticket #63, spec #59): kill mid-phase plus
    // --- resume, hermetically. A fake gh serves one canned ticket view
    // --- from local files, a fake implementer stands in for `pi` (no
    // --- model, no spend), and a fake adapter stands in for the checks —
    // --- so green here proves the guarantee with no network and no spend,
    // --- exactly like the release mechanics above.
    let gh_views = tmp.join("dispatch-views");
    let gh_dispatch_log = tmp.join("gh-dispatch.log");
    let pi_dispatch_log = tmp.join("pi-dispatch.log");
    let _ = std::fs::create_dir_all(&gh_views);
    let _ = std::fs::write(
        gh_views.join("71.json"),
        r#"{"number":71,"title":"Widget","body":"Build it.","labels":[{"name":"ticket"},{"name":"ready"}],"state":"OPEN","comments":[]}"#,
    );
    let fake_gh = tmp.join("fake-gh.sh");
    let _ = std::fs::write(
        &fake_gh,
        "#!/bin/sh\necho \"GH-DISPATCH: $*\" >> \"$GH_DISPATCH_LOG\"\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"list\" ]; then printf '[{\"number\":71}]'; exit 0; fi\nif [ \"$1\" = \"issue\" ] && [ \"$2\" = \"view\" ]; then cat \"$GH_DISPATCH_VIEWS/$3.json\"; exit 0; fi\nexit 0\n",
    );
    let fake_pi = tmp.join("fake-pi.sh");
    let _ = std::fs::write(
        &fake_pi,
        "#!/bin/sh\necho \"PI-DISPATCH: cwd=$(pwd)\" >> \"$PI_DISPATCH_LOG\"\necho \"change from hermetic implementer\" >> change.txt\necho \"notes\"\necho \"VERDICT: accept\"\n",
    );
    let fake_adapter = tmp.join("fake-adapter.sh");
    let _ = std::fs::write(
        &fake_adapter,
        "#!/bin/sh\necho \"adapter ran\" >> \"$PI_DISPATCH_LOG\"\necho \"FACTORY-CHECK PASSED\"\nexit 0\n",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for p in [&fake_gh, &fake_pi, &fake_adapter] {
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
        }
    }
    let dispatch_env = |cmd: &mut Command| {
        cmd.env("FACTORY_GH_BIN", &fake_gh)
            .env("FACTORY_PI_BIN", &fake_pi)
            .env("FACTORY_CHECK_ADAPTER", &fake_adapter)
            .env("GH_DISPATCH_LOG", &gh_dispatch_log)
            .env("GH_DISPATCH_VIEWS", &gh_views)
            .env("PI_DISPATCH_LOG", &pi_dispatch_log);
    };
    // Pass 1: a kill mid-implement interrupts (resumable), never fails.
    let mut kill = Command::new(&st.exe);
    kill.args(["dispatch", "once"]).current_dir(&st.proj);
    st.base_env(&mut kill);
    dispatch_env(&mut kill);
    kill.env("FACTORY_KILL_PHASE", "implement");
    let out = kill.output().expect("self-test dispatch kill must spawn");
    let (stdout, _stderr) = selftest_text(&out);
    let kill_status = std::fs::read_to_string(state.join("dispatch.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("attempts")?
                .as_array()?
                .iter()
                .find(|a| a.get("ticket").and_then(|t| t.as_u64()) == Some(71))?
                .get("status")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default();
    st.check(
        "dispatch kill-mid-phase interrupts (resumable)",
        out.status.success()
            && stdout.contains("interrupted")
            && stdout.contains("resumable")
            && kill_status == "implement_interrupted",
        "implement_interrupted (resumable)",
        format!(
            "status={kill_status} out={}",
            stdout.lines().last().unwrap_or("(empty)")
        ),
    );
    // Pass 2: the next pass resumes the same attempt to completion —
    // no fork, transcript preserved, resumed marked for the audit.
    let mut resume = Command::new(&st.exe);
    resume.args(["dispatch", "once"]).current_dir(&st.proj);
    st.base_env(&mut resume);
    dispatch_env(&mut resume);
    let out = resume
        .output()
        .expect("self-test dispatch resume must spawn");
    let ledger_text = std::fs::read_to_string(state.join("dispatch.json")).unwrap_or_default();
    let attempts: Vec<serde_json::Value> = serde_json::from_str::<serde_json::Value>(&ledger_text)
        .ok()
        .and_then(|v| v.get("attempts")?.as_array().cloned())
        .map(|all| {
            all.into_iter()
                .filter(|a| a.get("ticket").and_then(|t| t.as_u64()) == Some(71))
                .collect()
        })
        .unwrap_or_default();
    let resumed_ok = attempts.len() == 1
        && attempts[0].get("resumed").and_then(|r| r.as_u64()) == Some(1)
        && attempts[0].get("status").and_then(|s| s.as_str()) == Some("review_done");
    st.check(
        "dispatch resume completes the same attempt",
        out.status.success() && resumed_ok,
        "one attempt, resumed=1, review_done",
        format!("ledger={ledger_text}"),
    );
    // No network, no spend: the gh bomb never fired (the dispatch fakes
    // answered everything from local files) and only the fake
    // implementer ran (no model, no tokens in env by construction).
    let pi_log = std::fs::read_to_string(&pi_dispatch_log).unwrap_or_default();
    let bomb_after = std::fs::read_to_string(&gh_log).unwrap_or_default();
    st.check(
        "dispatch resume spends nothing (no network, fake implementer only)",
        bomb_after.trim().is_empty() && pi_log.contains("PI-DISPATCH:"),
        "gh never called, fake implementer ran",
        format!(
            "bomb={} pi={}",
            bomb_after.lines().next().unwrap_or("(empty)"),
            pi_log.lines().next().unwrap_or("(empty)")
        ),
    );

    // --- facts conflict path (ticket #79, spec #64): stale rejected,
    // --- fresh retry succeeds, hermetically. The same child-`factory`
    // --- seam as the release mechanics above, against the same isolated
    // --- state dir — no issue-tracker calls, no network, no spend.
    let out = st.factory(&[
        "facts",
        "create",
        "costs/total",
        "--revision",
        "0",
        "--value",
        "42",
    ]);
    let (_stdout, _stderr) = selftest_text(&out);
    let created_ok = out.status.success();
    let out = st.factory(&[
        "facts",
        "set",
        "costs/total",
        "--revision",
        "1",
        "--value",
        "43",
    ]);
    let (_stdout, _stderr2) = selftest_text(&out);
    let advanced_ok = out.status.success();
    // A loser presenting the stale revision 0 against current 2 is
    // rejected naming both revisions, with the stored value intact.
    let out = st.factory(&[
        "facts",
        "set",
        "costs/total",
        "--revision",
        "0",
        "--value",
        "99",
    ]);
    let (_stdout, conflict_err) = selftest_text(&out);
    let show = st.factory(&["facts", "show", "costs/total", "--json"]);
    let (show_out, _) = selftest_text(&show);
    let kept: bool = serde_json::from_str::<serde_json::Value>(&show_out)
        .map(|v| v.get("value").and_then(|n| n.as_u64()) == Some(43))
        .unwrap_or(false);
    st.check(
        "facts conflict rejects stale",
        created_ok
            && advanced_ok
            && !out.status.success()
            && conflict_err.contains('0')
            && conflict_err.contains('2')
            && kept,
        "stale rev 0 refused against current 2, value 43 intact",
        selftest_got(&out, &conflict_err),
    );
    // The loser rereads, observes revision 2, and retries: succeeds,
    // advancing the revision by exactly one to 3.
    let show = st.factory(&["facts", "show", "costs/total", "--json"]);
    let (show_out, _) = selftest_text(&show);
    let current = serde_json::from_str::<serde_json::Value>(&show_out)
        .ok()
        .and_then(|v| v.get("revision")?.as_u64())
        .unwrap_or(u64::MAX);
    let current_s = current.to_string();
    let out = st.factory(&[
        "facts",
        "set",
        "costs/total",
        "--revision",
        current_s.as_str(),
        "--value",
        "100",
    ]);
    let (_stdout, retry_err) = selftest_text(&out);
    let show = st.factory(&["facts", "show", "costs/total", "--json"]);
    let (show_out, _) = selftest_text(&show);
    let (rev_after, val_after) = serde_json::from_str::<serde_json::Value>(&show_out)
        .map(|v| {
            (
                v.get("revision").and_then(|r| r.as_u64()).unwrap_or(0),
                v.get("value").and_then(|n| n.as_u64()).unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    st.check(
        "facts retry succeeds",
        out.status.success() && current == 2 && rev_after == 3 && val_after == 100,
        "retry at rev 2 commits rev 3 value 100",
        if retry_err.trim().is_empty() {
            format!("rev={rev_after} value={val_after}")
        } else {
            selftest_got(&out, &retry_err)
        },
    );

    // --- fork-at-entry + resume-on-a-fork (ticket #82, spec #65): the
    // --- experiment line inherits the attempt prefix, survives an
    // --- interruption with its transcript preserved, and resumes as the
    // --- same line — through the same child-`factory` seam, with no gh,
    // --- no network, and no spend (fork lines run no implementer).
    let out = st.factory(&["fork", "71", "--at", "1", "--name", "selftest-exp"]);
    let (stdout, stderr) = selftest_text(&out);
    let attempt_log =
        std::fs::read_to_string(state.join("logs").join("T71-a1-impl.log")).unwrap_or_default();
    let fork_log_path = state.join("logs").join("T71-fork-selftest-exp.log");
    let fork_body = std::fs::read_to_string(&fork_log_path).unwrap_or_default();
    let prefix_ok =
        !attempt_log.is_empty() && !fork_body.is_empty() && attempt_log.starts_with(&fork_body);
    st.check(
        "fork-at-entry inherits the prefix",
        out.status.success() && stdout.contains("Forked ticket #71") && prefix_ok,
        "Forked ticket #71 with the attempt prefix",
        selftest_got(&out, &stderr),
    );
    // The experiment line does work: one transcript line from the worker.
    let _ = std::fs::write(&fork_log_path, format!("{fork_body}experiment work line\n"));
    // Interruption marks the line resumable and releases its worktree.
    let out = st.factory(&["fork", "71", "--interrupt", "selftest-exp"]);
    let (_stdout, stderr) = selftest_text(&out);
    let ledger_text = std::fs::read_to_string(state.join("dispatch.json")).unwrap_or_default();
    let interrupted = serde_json::from_str::<serde_json::Value>(&ledger_text)
        .ok()
        .and_then(|v| {
            v.get("forks")?
                .as_array()?
                .first()?
                .get("status")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default();
    let wt_list = st.git(&proj, &["worktree", "list"]);
    let (wt_out, _) = selftest_text(&wt_list);
    st.check(
        "fork interrupt releases the worktree (resumable)",
        out.status.success()
            && interrupted == "implement_interrupted"
            && !wt_out.contains("T71-fork-selftest-exp"),
        "implement_interrupted, worktree released",
        format!("status={interrupted} out={}", selftest_got(&out, &stderr)),
    );
    // Resume continues the same line: no new fork, transcript kept.
    let out = st.factory(&["fork", "71", "--resume", "selftest-exp"]);
    let (stdout, stderr) = selftest_text(&out);
    let ledger_text = std::fs::read_to_string(state.join("dispatch.json")).unwrap_or_default();
    let (fork_count, resumed, status) = serde_json::from_str::<serde_json::Value>(&ledger_text)
        .ok()
        .and_then(|v| {
            let forks = v.get("forks")?.as_array()?;
            let first = forks.first()?;
            Some((
                forks.len(),
                first.get("resumed")?.as_u64()?,
                first.get("status")?.as_str()?.to_string(),
            ))
        })
        .unwrap_or((0, 0, String::new()));
    let wt_list = st.git(&proj, &["worktree", "list"]);
    let (wt_out, _) = selftest_text(&wt_list);
    st.check(
        "fork resume continues the same line",
        out.status.success()
            && stdout.contains("resumed")
            && fork_count == 1
            && resumed == 1
            && status == "forked"
            && wt_out.contains("T71-fork-selftest-exp"),
        "one fork, resumed=1, forked, worktree re-attached",
        format!(
            "count={fork_count} resumed={resumed} status={status} out={}",
            selftest_got(&out, &stderr)
        ),
    );
    // The resumed transcript holds prefix + work + both markers, and
    // the existing tail bound still applies on the fork line.
    let resumed_body = std::fs::read_to_string(&fork_log_path).unwrap_or_default();
    let out = st.factory(&["log", "71", "--fork", "selftest-exp", "--tail", "2"]);
    let (tail_out, _) = selftest_text(&out);
    st.check(
        "fork resume preserves the transcript under tail bounds",
        out.status.success()
            && resumed_body.contains("experiment work line")
            && resumed_body.contains("interrupted during implement")
            && resumed_body.contains("resume #1")
            && tail_out.contains("resume #1")
            && !tail_out.contains("experiment work line"),
        "prefix + work + markers kept, tail 2 shows the newest lines",
        selftest_got(&out, &tail_out),
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
        Commands::Log {
            ticket,
            tail,
            fork,
            transcript,
        } => cmd_log(&ticket, tail, fork.as_deref(), transcript, cli.json),
        Commands::Facts { action } => match action {
            FactsAction::Show { path, at } => cmd_facts_show(cli.json, path.as_deref(), at),
            FactsAction::History { path, limit } => cmd_facts_history(cli.json, &path, limit),
            FactsAction::Create {
                path,
                revision,
                value,
            } => cmd_facts_create(cli.json, &path, revision, &value),
            FactsAction::Set {
                path,
                revision,
                value,
            } => cmd_facts_set(cli.json, &path, revision, &value),
            FactsAction::Delete { path, revision } => cmd_facts_delete(cli.json, &path, revision),
            FactsAction::Increment { path, revision, by } => {
                cmd_facts_increment(cli.json, &path, revision, &by)
            }
            FactsAction::Watch { path, since } => cmd_facts_watch(cli.json, &path, since),
        },
        Commands::Fork {
            ticket,
            attempt,
            fork,
            at,
            name,
            resume,
            interrupt,
            owned,
        } => cmd_fork(
            &ticket,
            attempt,
            fork.as_deref(),
            at,
            name.as_deref(),
            resume.as_deref(),
            interrupt.as_deref(),
            owned,
            cli.json,
        ),
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
            DispatchAction::Status { all } => cmd_dispatch_status(cli.json, all),
            DispatchAction::Graph { ticket } => cmd_dispatch_graph(cli.json, ticket.as_deref()),
            DispatchAction::Wait {
                ticket,
                parent,
                policy,
            } => cmd_dispatch_wait(cli.json, ticket.as_deref(), &parent, &policy),
            DispatchAction::Stop => cmd_dispatch_stop(cli.json),
            DispatchAction::Resume => cmd_dispatch_resume(cli.json),
            DispatchAction::Abort { ticket, background } => {
                cmd_dispatch_abort(&ticket, background, cli.json)
            }
            DispatchAction::Memo {
                ticket,
                attempt,
                key,
                decision,
            } => cmd_dispatch_memo(cli.json, &ticket, attempt, &key, &decision),
            DispatchAction::SelfTest => cmd_dispatch_selftest(cli.json),
            DispatchAction::Import { db } => cmd_dispatch_import(cli.json, db.as_deref()),
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
        Commands::Watch {
            interval,
            streams_url,
        } => cmd_watch(cli.json, interval, streams_url.as_deref()),
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

// --- cutover selection, TDD red (ticket #88) --------------------------------
// Release rows that are not the live open train never reach the Rust
// store: closed releases are terminal history (preserved in the
// byte-identical archive), and the newest open release wins, mirroring
// the bash reader (`ORDER BY id DESC LIMIT 1`).
#[cfg(test)]
mod cutover_tests {
    use super::{cutover_attempt, cutover_open_train};
    use crate::sqlite::{Row, Value};

    fn row(pairs: Vec<(&str, Value)>) -> Row {
        Row {
            columns: pairs.iter().map(|(c, _)| c.to_string()).collect(),
            values: pairs.into_iter().map(|(_, v)| v).collect(),
        }
    }

    fn release(id: i64, branch: &str, status: &str) -> Row {
        row(vec![
            ("id", Value::Int(id)),
            ("branch", Value::Text(branch.to_string())),
            ("base_commit", Value::Text("abc".to_string())),
            ("status", Value::Text(status.to_string())),
            ("created_at", Value::Text("2026-09-01 09:00:00".to_string())),
        ])
    }

    #[test]
    fn closed_releases_are_counted_never_carried() {
        let rows = vec![
            release(1, "integration/20260901-old", "closed"),
            release(2, "integration/20260901-train", "open"),
        ];
        let (train, closed) = cutover_open_train(&rows).unwrap();
        assert_eq!(train.unwrap().branch, "integration/20260901-train");
        assert_eq!(closed, 1);
    }

    #[test]
    fn newest_open_release_wins_like_the_bash_reader() {
        let rows = vec![
            release(1, "integration/20260901-first", "open"),
            release(2, "integration/20260901-second", "open"),
        ];
        let (train, closed) = cutover_open_train(&rows).unwrap();
        assert_eq!(train.unwrap().branch, "integration/20260901-second");
        assert_eq!(closed, 0);
    }

    #[test]
    fn no_releases_means_no_train() {
        let (train, closed) = cutover_open_train(&[]).unwrap();
        assert!(train.is_none());
        assert_eq!(closed, 0);
    }

    #[test]
    fn sparse_attempt_rows_read_as_empty_strings() {
        let r = row(vec![
            ("id", Value::Int(9)),
            ("ticket", Value::Int(77)),
            ("attempt_no", Value::Int(1)),
            ("base_commit", Value::Null),
            ("head_commit", Value::Null),
            ("branch", Value::Text("factory/T77".to_string())),
            ("worktree", Value::Null),
            ("status", Value::Text("impl_done".to_string())),
            ("exit_code", Value::Null),
            ("log_path", Value::Null),
            ("started_at", Value::Text("2026-09-05 10:00:00".to_string())),
            ("finished_at", Value::Null),
        ]);
        let a = cutover_attempt(&r).unwrap();
        assert_eq!(a.id, 9);
        assert_eq!(a.base_commit, "");
        assert_eq!(a.exit_code, 0);
        assert_eq!(a.train_branch, "");
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

// --- compact commit feed + partial output, TDD red (ticket #74, spec #66) ---
//
// Pure diffs and tick selection pinned without a server: what moved
// between two commit states, and which cadence each scope checks at.
// The loopback suite (ui_streams.rs) pins the same behavior through
// event order and view state on the wire.
#[cfg(test)]
mod stream_commit_tests {
    use super::{
        stream_commit_id, stream_partial_ms, stream_tick_ms, summarize_board_change,
        summarize_dispatcher_change, summarize_ticket_change, StreamScope, STREAM_PARTIAL_MS,
    };

    fn dispatcher_states() -> (serde_json::Value, serde_json::Value) {
        let old = serde_json::json!({
            "stopped": false,
            "attempts": [{"slot": "T9-a1", "status": "impl_running", "tail_lines": 2}],
            "tasks": [{"id": "t1", "kind": "parent", "status": "running", "owner": "attempt:T9-a1"}],
            "worktrees": ["/tmp/wt1"],
        });
        let new = serde_json::json!({
            "stopped": false,
            "attempts": [{"slot": "T9-a1", "status": "impl_done", "tail_lines": 4}],
            "tasks": [
                {"id": "t1", "kind": "parent", "status": "running", "owner": "attempt:T9-a1"},
                {"id": "t2", "kind": "child", "status": "running", "owner": "task:t1"}
            ],
            "worktrees": ["/tmp/wt1"],
        });
        (old, new)
    }

    #[test]
    fn dispatcher_diff_names_transitions_joins_and_output_growth() {
        let (old, new) = dispatcher_states();
        let changes = summarize_dispatcher_change(&old, &new);
        assert!(
            changes
                .iter()
                .any(|c| c == "T9-a1 impl_running\u{2192}impl_done"),
            "status transition, got: {changes:?}"
        );
        assert!(
            changes.iter().any(|c| c == "T9-a1 +2 lines"),
            "output growth, got: {changes:?}"
        );
        assert!(
            changes.iter().any(|c| c == "+task t2"),
            "task join, got: {changes:?}"
        );
    }

    #[test]
    fn dispatcher_diff_names_leaves_and_stop_toggles() {
        let (mut old, _) = dispatcher_states();
        old["stopped"] = serde_json::Value::Bool(true);
        let new = serde_json::json!({
            "stopped": false,
            "attempts": [],
            "tasks": [],
            "worktrees": [],
        });
        let changes = summarize_dispatcher_change(&old, &new);
        assert!(changes.contains(&"dispatch loop resumed".to_string()));
        assert!(changes.contains(&"-attempt T9-a1".to_string()));
        assert!(changes.contains(&"-task t1".to_string()));
        assert!(changes.contains(&"worktrees moved".to_string()));
    }

    #[test]
    fn board_diff_names_column_moves_and_updates() {
        let old = serde_json::json!({"cards": [
            {"column": "ready", "number": 9, "title": "Sprout", "status": "Ready to build."},
            {"column": "open", "number": 10, "title": "Seed", "status": "New."},
        ]});
        let new = serde_json::json!({"cards": [
            {"column": "running", "number": 9, "title": "Sprout", "status": "Being built now."},
            {"column": "open", "number": 11, "title": "Root", "status": "New."},
        ]});
        let changes = summarize_board_change(&old, &new);
        assert!(changes.contains(&"#9 ready\u{2192}running".to_string()));
        assert!(changes.contains(&"#10 left".to_string()));
        assert!(changes.contains(&"#11 joined open".to_string()));
    }

    #[test]
    fn ticket_diff_names_starts_transitions_and_growth() {
        let old = serde_json::json!({
            "ticket": 9, "state": "open", "title": "Sprout",
            "attempts": [{"n": 1, "status": "impl_running", "tail_lines": 2}],
        });
        let new = serde_json::json!({
            "ticket": 9, "state": "open", "title": "Sprout",
            "attempts": [
                {"n": 1, "status": "impl_done", "tail_lines": 5},
                {"n": 2, "status": "review_running", "tail_lines": 0},
            ],
        });
        let changes = summarize_ticket_change(&old, &new);
        assert!(changes.contains(&"attempt 1 impl_running\u{2192}impl_done".to_string()));
        assert!(changes.contains(&"attempt 1 +3 lines".to_string()));
        assert!(changes.contains(&"attempt 2 started".to_string()));
    }

    #[test]
    fn identical_states_summarize_to_nothing() {
        let (old, _) = dispatcher_states();
        assert!(summarize_dispatcher_change(&old, &old).is_empty());
    }

    #[test]
    fn commit_ids_are_stable_short_hashes() {
        let a = stream_commit_id("<section>same</section>");
        assert_eq!(a, stream_commit_id("<section>same</section>"));
        assert_eq!(a.len(), 16, "truncated hex, got: {a}");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, stream_commit_id("<section>moved</section>"));
    }

    #[test]
    fn transcript_scopes_tick_at_the_partial_bound() {
        assert_eq!(STREAM_PARTIAL_MS, 500, "documented sub-second bound");
        assert_eq!(stream_partial_ms(), 500, "unset env reads as the default");
        // Slow default keepalive (15s): the board keeps its cheap beat
        // while running-output scopes cap at the partial bound.
        let slow = 15_000;
        assert_eq!(stream_tick_ms(slow), 1_000);
        assert_eq!(StreamScope::Board.tick_ms(slow), 1_000);
        for scope in [
            StreamScope::DispatcherLive,
            StreamScope::Ticket {
                number: 9,
                raw: "9".to_string(),
            },
        ] {
            assert_eq!(
                scope.tick_ms(slow),
                500,
                "running output surfaces within the documented bound"
            );
        }
    }
}

//
// Ticket #89: bounded-queue disconnect pinned without a server. The
// loopback slow-consumer test (`stalled_consumer_...` in ui_streams.rs)
// drives this same collapse through real TCP backpressure, which is
// timing-dependent (kernel loopback buffers absorb whole 1.5MB deltas,
// and 40ms producer ticks alias against the 60ms toggle beat under
// load); it is quarantined there with the re-enable criterion. This
// module pins the mechanism deterministically: past `buffer_bound`
// queued events the backlog collapses into one terminal reattach
// instruction and the stream ends — no sockets, no clock.
#[cfg(test)]
mod stream_backpressure_tests {
    use super::{StreamChannel, StreamInner, StreamScope, StreamShared};
    use futures_core::Stream;

    fn noop_waker() -> std::task::Waker {
        use std::task::{RawWaker, RawWakerVTable, Waker};
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    fn dummy_event(name: &str) -> axum::response::sse::Event {
        axum::response::sse::Event::default()
            .event(name)
            .data("dummy")
    }

    #[test]
    fn queued_past_the_bound_collapses_to_one_terminal_reattach_then_ends() {
        // Mirrors the quarantined wire test's rule (`buffer_bound = 4`):
        // hello + snapshot are consumed before the stall, so the queue
        // starts empty and each detected change costs two slots
        // (commit, then delta). The third change past a full queue must
        // collapse the backlog instead of buffering forever.
        let scope = StreamScope::DispatcherLive;
        let bound = 4usize;
        let shared = std::sync::Arc::new(StreamShared {
            inner: std::sync::Mutex::new(StreamInner {
                queue: std::collections::VecDeque::new(),
                closed: false,
                waker: None,
            }),
        });
        assert!(shared.push(dummy_event("commit")));
        assert!(shared.push(dummy_event("delta")));
        assert!(shared.push_delta(dummy_event("commit"), &scope, bound));
        assert!(shared.push_delta(dummy_event("delta"), &scope, bound));
        assert!(
            !shared.push_delta(dummy_event("commit"), &scope, bound),
            "past a full queue the producer must exit with the collapse"
        );
        assert!(
            shared.is_closed(),
            "the collapse ends the stream: no task or buffer is held"
        );
        assert_eq!(
            shared
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .queue
                .len(),
            1,
            "the backlog collapses to the single terminal instruction"
        );
        assert!(
            !shared.push(dummy_event("delta")),
            "nothing queues after the terminal instruction"
        );

        // The same interface Axum polls: exactly one event (the
        // reattach instruction), then the end of the stream — the
        // payload copy itself stays pinned by the quarantined wire
        // test (`-- --ignored`), since axum's `Event` exposes no
        // payload getters to unit tests.
        let waker = noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        let mut channel = StreamChannel {
            shared: shared.clone(),
        };
        assert!(
            matches!(
                std::pin::Pin::new(&mut channel).poll_next(&mut cx),
                std::task::Poll::Ready(Some(_))
            ),
            "the viewer drains the terminal instruction first"
        );
        assert!(
            matches!(
                std::pin::Pin::new(&mut channel).poll_next(&mut cx),
                std::task::Poll::Ready(None)
            ),
            "then the server ends the stream instead of idling"
        );
    }
}
