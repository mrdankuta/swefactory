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
struct Cli {
    /// Emit machine-parseable JSON for every command that produces output.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print the factory version.
    Version,
    /// Seed the factory into the current project (idempotent, never clobbers).
    Init {
        /// Check-adapter stack to install.
        #[arg(long, default_value = "rust")]
        stack: String,
        /// Install skills to $HOME/.agents/skills instead of the project.
        #[arg(long)]
        global_skills: bool,
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
}

#[derive(Subcommand, Debug)]
enum DispatchAction {
    /// Single scheduling pass over ready tickets.
    Once {
        /// Survey only: no worktrees, subprocesses, or writes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show attempts + worktrees + STOP state.
    Status,
    /// Halt the loop: later `once` runs exit 0 without work.
    Stop,
    /// Restart the loop after a stop.
    Resume,
}

#[derive(Serialize)]
struct VersionOutput<'a> {
    version: &'a str,
}

#[derive(Serialize)]
struct InitOutput {
    stack: String,
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
        _ => None,
    }
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
        self.seed(rel, content);
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
        // gitignore cache entry (only when a .gitignore already exists)
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
                    self.say("install: .gitignore (factory/.cache/)");
                }
            }
        }
        // version marker (always records the running binary's version)
        let want = env!("CARGO_PKG_VERSION");
        let vf = self.dest.join(".factory-version");
        let cur = std::fs::read_to_string(&vf).unwrap_or_default();
        if cur.trim() != want {
            if std::fs::write(&vf, format!("{want}\n")).is_ok() {
                self.say("install: .factory-version");
            }
        } else {
            self.kept.push(".factory-version".to_string());
        }
    }
}

fn run_init(
    json: bool,
    stack: &str,
    global_skills: bool,
    pin: Option<&str>,
    cmd: &str,
) -> InitOutput {
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

    // --- prompts ---
    ins.seed(".pi/prompts/plan.md", PROMPT_PLAN);
    ins.seed(".pi/prompts/build.md", PROMPT_BUILD);
    ins.seed(".pi/prompts/review.md", PROMPT_REVIEW);
    ins.seed(".pi/prompts/handoff.md", PROMPT_HANDOFF);

    // --- skills (project-local, hermetic; never overwrite existing) ---
    let skill_root = if global_skills {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Path::new(&home).join(".agents/skills")
    } else {
        dest.join(".agents/skills")
    };
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

    // --- check adapter for the stack ---
    match adapter_for(stack) {
        Some(adapter) => {
            ins.seed_executable("scripts/factory-check", adapter);
            if cmd == "init" {
                ins.say(&format!("check adapter: {stack}"));
            }
        }
        None => {
            ins.warn(&format!(
                "WARN: no check adapter for '{stack}' (expected one of rust, elixir, node, python) — write scripts/factory-check (contract: format→lint→test, non-zero on fail)"
            ));
        }
    }

    // --- AGENTS.md header (marker-based merge) ---
    let agents = dest.join("AGENTS.md");
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

    ins.finish_dirs_and_markers();

    if let Some(tag) = pin {
        ins.say(&format!(
            "pin: {tag} recorded (this binary carries embedded templates; network refresh is out of scope)"
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

fn run_check(json: bool, adapter: Option<PathBuf>, slot: Option<String>, args: Vec<String>) -> ! {
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
    Command::new(gh_bin())
        .args(args)
        .output()
        .map_err(|e| format!("failed to run gh {}: {e}", args.join(" ")))
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

fn cmd_handoff(issue: &str, signal: &str, body_file: Option<&PathBuf>, json: bool) -> i32 {
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

fn cmd_reconcile(json: bool) -> i32 {
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
        let out = ReconcileOutput {
            base,
            issues: entries,
            branches,
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
        println!("Suggested actions (human decides): close landed issues, delete merged branches.");
    }
    0
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

fn cmd_start(ticket: &str, worktree_override: Option<&PathBuf>, json: bool) -> i32 {
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
}

#[derive(Serialize, serde::Deserialize, Default)]
struct DispatchLedger {
    #[serde(default)]
    attempts: Vec<DispatchAttempt>,
}

/// Implementer/reviewer seam: shell out only, never an API client.
/// Overridable via FACTORY_PI_BIN for hermetic tests.
fn pi_bin() -> String {
    std::env::var("FACTORY_PI_BIN").unwrap_or_else(|_| "pi".to_string())
}

/// State root: FACTORY_STATE_DIR, else <cwd>/factory/.state.
fn dispatch_state_dir() -> PathBuf {
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
        "number,title,body,labels,state",
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
    let attempt_no = past + 1;
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
            });
            log_event(&log_path, id, ticket, "spec-gate", 1);
            gh_comment(ticket, &format!("DISPATCH: linked spec #{s} body no longer matches its SPEC-APPROVED hash. Human must re-freeze before this ticket runs."));
            return TicketOutcome::Done;
        }
    }

    let base = git_head(
        std::env::current_dir()
            .as_deref()
            .unwrap_or(std::path::Path::new(".")),
    );
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
    });
    let idx = ledger.attempts.len() - 1;
    if let Err(e) = save_ledger(state, ledger) {
        eprintln!("dispatch: {e}");
    }

    // --- implement ---
    let spec_label = spec
        .map(|s| s.to_string())
        .unwrap_or_else(|| "none".to_string());
    let brief = format!("Ticket issue: #{ticket}\nSpec issue: #{spec_label}\nBranch: {branch} (base {base})\n\n{body}\n");
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
        // Accepted: human owns the land. Record base→head, never merge/push.
        gh_relabel(ticket, Some("review"), "accepted");
        gh_comment(ticket, &format!("DISPATCH: review accepted. Base `{base}` → head `{head}` on `{branch}`. Human: verify and land."));
        println!("  ACCEPTED — #{ticket} awaiting human land");
        finish_attempt(state, ledger, idx, "review_done", 0);
    } else {
        println!("  changes_requested (or unreadable verdict) — back to ready");
        gh_relabel(ticket, Some("review"), "ready");
        finish_attempt(state, ledger, idx, "review_failed", 0);
    }
    TicketOutcome::Done
}

fn cmd_dispatch_once(json: bool, dry_run: bool) -> i32 {
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
    if list.is_empty() {
        println!("dispatch: no open tickets");
        return 0;
    }
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
    if json {
        println!(
            "{{\"stopped\":false,\"attempted\":{attempted}}}",
            attempted = attempted
        );
    }
    0
}

fn cmd_dispatch_status(json: bool) -> i32 {
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
            println!(
                "#{} T{} a{} {} base={} head={} {}",
                a.id, a.ticket, a.attempt_no, a.status, base8, head8, a.started_at
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

fn cmd_dispatch_stop(json: bool) -> i32 {
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

fn cmd_dispatch_resume(json: bool) -> i32 {
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

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            let version = env!("CARGO_PKG_VERSION");
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&VersionOutput { version })
                        .expect("version JSON serialization")
                );
            } else {
                println!("{version}");
            }
            0
        }
        Commands::Init {
            stack,
            global_skills,
        } => {
            let out = run_init(cli.json, &stack, global_skills, None, "init");
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&out).expect("init JSON serialization")
                );
            }
            0
        }
        Commands::Update { pin } => {
            // update refreshes from the same embedded templates; because
            // seeding never overwrites, it is a no-op when in place.
            let out = run_init(cli.json, "rust", false, pin.as_deref(), "update");
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
        Commands::Handoff {
            issue,
            signal,
            body,
        } => {
            std::process::exit(cmd_handoff(&issue, &signal, body.as_ref(), cli.json));
        }
        Commands::Reconcile => {
            std::process::exit(cmd_reconcile(cli.json));
        }
        Commands::Start { ticket, worktree } => {
            std::process::exit(cmd_start(&ticket, worktree.as_ref(), cli.json));
        }
        Commands::Dispatch { action } => match action {
            DispatchAction::Once { dry_run } => {
                std::process::exit(cmd_dispatch_once(cli.json, dry_run));
            }
            DispatchAction::Status => {
                std::process::exit(cmd_dispatch_status(cli.json));
            }
            DispatchAction::Stop => {
                std::process::exit(cmd_dispatch_stop(cli.json));
            }
            DispatchAction::Resume => {
                std::process::exit(cmd_dispatch_resume(cli.json));
            }
        },
    }
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

fn cmd_freeze(json: bool, issue_raw: &str, label: &str) -> i32 {
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

fn cmd_sync(json: bool, issue_raw: &str) -> i32 {
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

fn cmd_verify_freeze(json: bool, issue_raw: &str) -> i32 {
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
