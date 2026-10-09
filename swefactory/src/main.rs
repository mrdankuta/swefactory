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
