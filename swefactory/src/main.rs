use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;
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

fn run_gh(args: &[&str]) -> Result<std::process::Output, String> {
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
    match run_gh(&["issue", "comment", issue, "--body", &body]) {
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
    let list = match run_gh(&[
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
    let view = match run_gh(&["issue", "view", ticket, "--json", "number,title,body"]) {
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
    }
}
