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
    Land {
        /// Preview every mutation without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ReleaseAction {
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
enum DispatchAction {
    /// Single scheduling pass over ready tickets.
    Once {
        /// Survey only: no worktrees, subprocesses, or writes.
        #[arg(long)]
        dry_run: bool,
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

/// Seed every project-local file init owns (prompts, skills, adapter,
/// AGENTS header, glossary, docs, issue templates, dirs + markers).
/// Shared by `init`/`update` and `doctor --fix` so repair re-seeds through
/// init's own path. Never overwrites: existing files are kept, which is
/// what makes both init and --fix idempotent. No gh calls in here — label
/// management stays in run_init so --fix never touches issues.
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

    // --- local tree: init's own seeding path (shared with doctor --fix) ---
    let skill_root = if global_skills {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Path::new(&home).join(".agents/skills")
    } else {
        dest.join(".agents/skills")
    };
    seed_project_local(&mut ins, stack, cmd, &skill_root, global_skills);

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

// ---------------------------------------------------------------------------
// DOCTOR (ticket #18: diagnose — checks + report + JSON)
//
// Pre-flight check over the catalogue in DOCTOR_ORDER. Every probe reads
// the real environment through the existing shell-out seams (gh/git/pi
// binaries, adapter discovery, state dir, PATH toolchain) — no API
// client, no new seam. Doctor is read-only: it never writes files,
// issues, pushes, or starts auth flows; failures carry the exact fix
// command instead. `--fix` (safe local repair) is a separate ticket and
// deliberately has no flag here. The catalogue lives in this one module
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
struct DoctorCheck {
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

/// Default stack re-seeded when the adapter is missing entirely.
/// `--fix` takes no --stack flag (spec: `doctor [--fix] [--json]`).
const DOCTOR_FIX_STACK: &str = "rust";

fn doctor_fix() -> Vec<String> {
    let mut fixed = Vec::new();

    // R1: state dir.
    let state = dispatch_state_dir();
    match std::fs::metadata(&state) {
        Err(_) => {
            if std::fs::create_dir_all(&state).is_ok() {
                fixed.push(format!("state-dirs: created {}", state.display()));
            }
        }
        Ok(m) if m.is_dir() && m.permissions().readonly() => {
            // Restore owner write without widening to world-writable.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = m.permissions().mode() | 0o200;
                if std::fs::set_permissions(&state, std::fs::Permissions::from_mode(mode)).is_ok() {
                    fixed.push(format!("state-dirs: made writable {}", state.display()));
                }
            }
            #[cfg(not(unix))]
            {
                let mut perms = m.permissions();
                perms.set_readonly(false);
                if std::fs::set_permissions(&state, perms).is_ok() {
                    fixed.push(format!("state-dirs: made writable {}", state.display()));
                }
            }
        }
        Ok(_) => {}
    }

    // R2: stale worktree metadata (detect via dry run, repair only on signal).
    if let Some(dry) = probe_stdout(&git_bin(), &["worktree", "prune", "-n"]) {
        if !dry.is_empty() {
            let _ = probe_stdout(&git_bin(), &["worktree", "prune"]);
            let first = dry.lines().next().unwrap_or("stale entries");
            fixed.push(format!("worktree-metadata: pruned ({first})"));
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
    seed_project_local(
        &mut ins,
        DOCTOR_FIX_STACK,
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

fn doctor_checks() -> Vec<DoctorCheck> {
    let mut checks = Vec::with_capacity(DOCTOR_ORDER.len());

    // gh-installed: the gh shell-out seam must answer.
    let gh_version = probe_stdout(&gh_bin(), &["--version"]);
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
    let auth = probe_stdout(&gh_bin(), &["auth", "status"]);
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
    let repo = probe_stdout(&gh_bin(), &["repo", "view", "--json", "nameWithOwner"]);
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

    // state-dirs: state/cache dir must exist, be a dir, and be writable.
    // Read-only probe: existence + dir + permission bits, never creates.
    let state = dispatch_state_dir();
    checks.push(match std::fs::metadata(&state) {
        Ok(m) if !m.is_dir() => doctor_fail(
            "state-dirs",
            format!(
                "State path {} exists but is not a directory.",
                state.display()
            ),
            format!(
                "Run: mv {} {}.bak && mkdir -p {}",
                state.display(),
                state.display(),
                state.display()
            ),
        ),
        Ok(m) if m.permissions().readonly() => doctor_fail(
            "state-dirs",
            format!("State directory {} is not writable.", state.display()),
            format!("Run: chmod u+rwX {}", state.display()),
        ),
        Ok(_) => doctor_pass(
            "state-dirs",
            format!(
                "State directory {} exists and is writable.",
                state.display()
            ),
        ),
        Err(_) => doctor_fail(
            "state-dirs",
            format!("State directory {} does not exist yet.", state.display()),
            format!("Run: mkdir -p {}", state.display()),
        ),
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
fn cmd_doctor(json: bool, fix: bool) -> i32 {
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
    #[serde(default)]
    train_branch: String,
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
        // Accept path: with an open train, auto-merge cleanly or route to
        // blocked on conflict (never forced). Without a train the human
        // owns the land. Never merges into dev/main, never pushes.
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
            // Accepted: human owns the land. Record base→head, never merge/push.
            gh_relabel(ticket, Some("review"), "accepted");
            gh_comment(ticket, &format!("DISPATCH: review accepted. Base `{base}` → head `{head}` on `{branch}`. Human: verify and land."));
            println!("  ACCEPTED — #{ticket} awaiting human land");
            finish_attempt(state, ledger, idx, "review_done", 0);
        }
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

fn cmd_dispatch_daemon(json: bool, interval: u64) -> i32 {
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
        let code = cmd_dispatch_once(json, false);
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
fn cmd_dispatch_selftest(json: bool) -> i32 {
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

fn cmd_release_start(json: bool, slug_opt: Option<&str>) -> i32 {
    let state = dispatch_state_dir();
    if let Err(e) = std::fs::create_dir_all(&state) {
        eprintln!("release: cannot create {}: {e}", state.display());
        return 1;
    }
    if let Some(existing) = load_open_train(&state) {
        eprintln!(
            "release: train already open ({}) — merge or abandon it first; never two open trains",
            existing.branch
        );
        return 1;
    }
    let mut slug = slugify(slug_opt.unwrap_or("train"));
    if slug.is_empty() {
        slug = "train".to_string();
    }
    let stamp = date_stamp();
    let train = format!("integration/{stamp}-{slug}");
    if branch_exists(&train) {
        eprintln!("release: branch {train} already exists — refusing to reuse");
        return 1;
    }
    let base = resolve_train_base();
    match run_git(&["branch", &train, &base], None) {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprintln!(
                "release: git branch failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return 1;
        }
        Err(e) => {
            eprintln!("release: {e}");
            return 1;
        }
    }
    let record = TrainState {
        branch: train.clone(),
        base_commit: base.clone(),
        status: "open".to_string(),
        created_at: now_stamp(),
    };
    if let Err(e) = save_train(&state, &record) {
        eprintln!("release: {e}");
        return 1;
    }
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

fn cmd_release_status(json: bool) -> i32 {
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

fn cmd_release_merge(json: bool, ticket_raw: &str) -> i32 {
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

fn cmd_release_drop(json: bool, ticket_raw: &str) -> i32 {
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

fn cmd_release_finish(json: bool) -> i32 {
    let state = dispatch_state_dir();
    let Some(train) = load_open_train(&state) else {
        eprintln!("release: no open train");
        return 1;
    };
    if !train_merged_into_dev(&train.branch) {
        eprintln!(
            "release: train {} not merged into dev — human must merge first",
            train.branch
        );
        return 1;
    }
    // An empty train (head == base) is trivially an ancestor but was never
    // landed: refuse only when dev still points at the same commit (no
    // human merge happened). After a human `--no-ff` merge of an empty
    // train, dev has a new merge commit so dev != train and finish succeeds.
    let head_now = train_head(&train.branch);
    if head_now.is_empty() {
        eprintln!(
            "release: train {} not merged into dev — human must merge first",
            train.branch
        );
        return 1;
    }
    let dev_head = rev_parse_ok("dev")
        .or_else(|| rev_parse_ok("origin/dev"))
        .unwrap_or_default();
    if head_now == train.base_commit && head_now == dev_head {
        eprintln!(
            "release: train {} not merged into dev — human must merge first",
            train.branch
        );
        return 1;
    }
    let ledger = load_ledger(&state);
    let members = member_branches(&ledger, &train.branch);
    remove_train_worktree(&state);
    clear_train(&state);
    let cleanup = cleanup_lines(&train.branch, &members);
    if json {
        println!(
            "{}",
            serde_json::json!({"train": train.branch, "merged": true, "cleanup": cleanup})
        );
    } else {
        println!("release: train {} merged into dev", train.branch);
        println!("cleanup (human runs these, automation never deletes):");
        for line in &cleanup {
            println!("  {line}");
        }
    }
    0
}

fn cmd_release_abandon(json: bool) -> i32 {
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
        Commands::Doctor { fix } => {
            std::process::exit(cmd_doctor(cli.json, fix));
        }
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
            DispatchAction::Daemon { interval } => {
                std::process::exit(cmd_dispatch_daemon(cli.json, interval));
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
            DispatchAction::SelfTest => {
                std::process::exit(cmd_dispatch_selftest(cli.json));
            }
        },
        Commands::Release { action } => match action {
            ReleaseAction::Start { slug } => {
                std::process::exit(cmd_release_start(cli.json, slug.as_deref()));
            }
            ReleaseAction::Status => {
                std::process::exit(cmd_release_status(cli.json));
            }
            ReleaseAction::Merge { ticket } => {
                std::process::exit(cmd_release_merge(cli.json, &ticket));
            }
            ReleaseAction::Drop { ticket } => {
                std::process::exit(cmd_release_drop(cli.json, &ticket));
            }
            ReleaseAction::Finish => {
                std::process::exit(cmd_release_finish(cli.json));
            }
            ReleaseAction::Abandon => {
                std::process::exit(cmd_release_abandon(cli.json));
            }
        },
        Commands::Land { dry_run } => {
            std::process::exit(cmd_land(cli.json, dry_run));
        }
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

fn cmd_land(json: bool, dry_run: bool) -> i32 {
    let state = dispatch_state_dir();
    let Some(train) = load_open_train(&state) else {
        eprintln!("land: no open train — cut one with `release start` first");
        return 1;
    };
    if !land_working_tree_clean() {
        eprintln!(
            "land: working tree is dirty — commit or stash before landing {}",
            train.branch
        );
        return 1;
    }
    if let Err(e) = land_run_checks(&train.branch) {
        eprintln!("{e}");
        return 1;
    }
    if !land_remote_reachable() {
        eprintln!("land: remote origin unreachable — refusing before any mutation");
        return 1;
    }
    let ledger = load_ledger(&state);
    let tickets = member_tickets(&ledger, &train.branch);
    let members = member_branches(&ledger, &train.branch);
    let cleanup = cleanup_lines(&train.branch, &members);
    let merge_msg = format!("Merge {} into dev", train.branch);
    let note = land_release_note(&train.branch);
    if dry_run {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "train": train.branch,
                    "dry_run": true,
                    "preflight": "ok",
                    "merge": merge_msg,
                    "push": "origin dev",
                    "close": tickets,
                    "note": note,
                    "cleanup": cleanup,
                })
            );
        } else {
            println!("land: dry-run plan for {} (no changes made)", train.branch);
            println!("  git checkout dev");
            println!(
                "  git merge --no-ff --no-edit -m \"{merge_msg}\" {}",
                train.branch
            );
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
        eprintln!("{e}");
        return 1;
    }
    let dev_short = match land_merge_train(&train.branch, &merge_msg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if let Err(e) = land_push_dev() {
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
                eprintln!("{e}");
                return 1;
            }
        }
    }
    // Same state path `release finish` uses: drop the train worktree,
    // close the train record, print cleanup (never execute it).
    remove_train_worktree(&state);
    clear_train(&state);
    if json {
        println!(
            "{}",
            serde_json::json!({"train": train.branch, "dry_run": false, "merge": dev_short, "pushed": true, "closed": closed, "skipped": skipped, "cleanup": cleanup})
        );
    } else {
        println!("land: merged {} into dev ({dev_short})", train.branch);
        println!("land: pushed dev to origin");
        for t in &closed {
            println!("land: closed #{t}");
        }
        for t in &skipped {
            println!("land: #{t} already closed, skipping");
        }
        println!("land: train {} landed into dev", train.branch);
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
