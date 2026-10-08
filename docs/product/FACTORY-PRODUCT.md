# Factory Product Vision — production-grade software, consumer-simple UX

> Status: vision / pre-spec. Authoritative decisions live in GitHub issues once the repo is created.
> Source snapshot: `upstream/` (pi-factory @ `dc477a7`, v0.5.1 worktree) + `factory-v0.4.0/` (installed instance in booksbuddy).
> Session origin: fork of Orca session `9cff49ef…`, agent `pi`, Oct 2026.

## 1. The bet

**Factory is not a dev tool. It is a quality engine with two faces:**
production-grade software engineering practices underneath, an interface so
simple a non-technical builder can ship with it — while a senior engineer
still trusts and reaches for it.

* Builder says what they want in plain language → working, reviewed,
  tested software comes out.
* Every step the factory takes is the step a great team would take:
  spec → frozen approval → small tickets → isolated builds → independent
  review → green checks → integrated release train → human-owned land.
* The rigor is invisible by default, auditable on demand.

Positioning: **not another IDE, not another model.** The orchestration +
quality layer that sits under any agent (Pi core headless, Claude, Codex…)
and inside any surface (CLI, web, Orca ADE, mobile). Orca won the ADE;
factory owns the *method*.

## 2. Who it's for

### Builder Bea (non-technical founder / operator)
* Has an idea, no repo habits. Connects GitHub (or Linear), picks a name,
  describes the idea in one or two sentences.
* Sees cards move: `Ideas → Building → Needs you → Done`. "Needs you" is
  always a concrete question with options, never a stack trace.
* Approves specs in plain English ("here's what we'll build, in your
  words — [Approve] [Change something]"), previews each feature as a link
  + screenshot, accepts with one tap.
* Trust signals, not logs: green ticks for tests, "reviewed by independent
  check", cost per feature ("$1.20"), what changed in one paragraph.

### Engineer Eli (senior, owns production)
* Trusts factory because the protocol is strict: frozen spec hashes,
  separate review worktree, `factory-check` gate (format → lint → test),
  `integration/*` release trains, human-owned land. Nothing merges on vibes.
* Uses the same engine via CLI with `--json`, custom stack adapters,
  policy files (protected paths, tenant-boundary rules), prompt + factory
  version pinning, reproducible reruns.
* Reviews trains, not threads: one diff per ticket, review verdict quoted,
  CI on train head, one-command `land` / `drop`.

## 3. What we learned from Orca (`stablyai/orca`, MIT, ~76k★)

Researched Oct 2026 via web (repo + `onorca.dev/docs/cli/*`). Adapt, don't clone:

| Orca pattern | Factory equivalent (do this) |
|---|---|
| ADE, not IDE — orchestration outlasts models, BYO agent (27 agents incl. Pi), no proxy | `AgentRunner` trait. Pi core **headless** (no TUI) is one runner; Claude/Codex later. Never tie the product to Herdr panes or Pi TUI. |
| `orca-cli` is the API: `worktree create/ps/rm`, `terminal send/wait --for tui-idle`, `snapshot/click/fill`, `emulator`, `automations`, `artifacts share` — every UI action scriptable, agents call the same CLI with `--json` | One `factory` binary, `--json` everywhere, for humans *and* agents. Kill the Herdr-pane dance. `factory status --json`, `factory log 98 --tail`, `factory train status --json`. |
| Tasks native: GitHub/Linear/Jira/GitLab → "open worktree from task", review PR in-app. Open issue #8376 asks for a generic task-source API | `TaskStore` trait. GitHub Issues first (we have it), Linear second, Jira third. Same ticket shape (`ready/running/review/integrated/blocked`, `Depends on:`, `Spec:`), pluggable backend. Non-technical users see cards, never issue numbers. |
| Review loop: annotate diff lines → send back to agent, checks inline, compare 5 parallel attempts, merge winner | Keep `review.md → VERDICT: accept\|changes_requested → train merge --no-ff`, but render as side-by-side diffs + plain-English verdict + one-click "send back". |
| Terminals survive restarts, SSH worktrees on beefy boxes, mobile companion ("agent needs you", steer from phone), cron automations | Supervised daemon (`factoryd`), not a bash loop. Remote workers for fan-out, local supervision, push on `needs-you`, scheduled `rebase train on origin/dev`. |
| Ships desktop + CLI on PATH + headless `orca serve` + mobile; skills registry (`npx skills add … --skill orca-cli`) | Ship one static binary + `factory serve`; publish a `factory` skill so *any* orchestrator (Orca, Herdr, plain terminal) can run the method. Don't rebuild Electron — integrate. |
| `snapshot/click/fill` browser loop + device profiles + design-mode (click UI → HTML/CSS+screenshot into prompt) | Reuse for acceptance verification: "does the login button work? prove it with a screenshot", not just unit tests. |

Explicit non-goals (v1): no new editor, no new model, no new issue tracker.
Be the quality workflow that runs *inside* the tools teams already use.

## 4. Architecture (Rust)

```
┌─────────────────────────────────────────────────┐
│ Surfaces (all call the same CLI --json)         │
│  web UI (thin) · factory CLI · Orca skill ·     │
│  mobile push (needs-you only)                   │
└──────────────────┬──────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────┐
│ factory (single Rust binary, clap)              │
│  init · doctor · plan · dispatch · train ·      │
│  status · log · land · skills · serve           │
└──────────────────┬──────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────┐
│ factoryd (supervised daemon, tokio)             │
│  scheduler │ worktree mgr │ gate runner │       │
│  train merger │ cost/policy guard │ audit log   │
│  TaskStore: github → linear → jira              │
│  AgentRunner: pi-headless → claude → codex      │
│  Verifier: browser snapshot/click/fill, previews│
└─────────────────────────────────────────────────┘
```

Why Rust over Go: single static binary for mac/win/linux, tokio daemon
with real locking/leases (kills the sqlite+`timeout pi -p` race at
`MAX_CONCURRENT>1`), strong CLI story (clap), matches Orca-native
ecosystem expectations. Go remains the fallback if hiring dictates.

Key contracts (carried over from v0.4.0, hardened):
* Spec freeze = sha256 of canonical body (`SPEC-APPROVED <hash>`); `verify-freeze`
  shows *what changed*, not just MISMATCH.
* Attempt ledger: atomic transitions
  `ready → impl_running → impl_done → review_running → review_done|failed`,
  heartbeat + stale-lease reaper, per-attempt structured logs, worktree GC.
* Dispatcher never lands, never touches `dev/main` — trains live on
  `integration/*` only; human owns train→dev. (Keep the v0.4.0 safety
  comment as a compile-time invariant + `self-test`.)
* Cost guardrails: per-ticket $ cap + token budget, not just `TIMEOUT_IMPL`.
* Reproducibility: freeze hash + prompt SHA + factory version + base commit
  recorded per attempt; `factory rerun 98 --attempt 2` replays exactly.
* Policy engine: protected paths, secret redaction, tenant-boundary rules
  from the project's `AGENTS.md` header (machine-readable, not prose-only).

## 5. UX sketch (consumer-simple)

* Onboarding (5 min): connect GitHub/Linear → pick repo (or "start fresh")
  → describe idea → factory drafts epic/spec/tickets → you approve in
  plain English. `factory doctor --fix` repairs `gh auth`, worktrees, model auth.
* Daily: kanban `Ideas → Building → Needs you → Done`. Each card: one-line
  status, preview link, cost, "what changed" paragraph. `Needs you` is a
  question with buttons; answering routes back to the agent (Orca mobile-companion pattern).
* Accept/land: diff summary + checks + reviewer verdict in plain English →
  [Accept & land] does merge + issue close + worktree cleanup, undoable via `drop` (revert).
* Pro escape hatch: `Advanced` reveals branches, hashes, train head, full
  logs, `factory land --checklist`, custom `factory-check` adapters.

## 6. Robustness roadmap

* v0.5 (now): Rust binary parity with bash (`init/doctor/status/log`,
  `dispatch once`, `train start/status/merge/drop`), `--json` everywhere,
  `self-test` in CI. gh-only, pi-headless-only.
* v1.0: `factoryd` supervised daemon, atomic ledger, remote workers,
  cost caps, `TaskStore: linear`, browser acceptance checks, metrics
  (lead time, review pass rate, $/ticket). `factory ui` local web.
* v2.0: `TaskStore: jira`, hosted dispatcher option (you run the daemon,
  user keeps repo+issues), team RBAC/SSO, audit export, organization
  policy packs (e.g. "Elixir+Oban multi-tenant" from `AGENTS.md` as a pack).

## 7. Open questions

1. Target wedge: solo builders first (simplest onboarding) or small teams with Linear (clearer willingness to pay)?
2. Orca relationship: skill/plugin inside Orca vs standalone web UI first?
3. Fresh-repo scaffolding (`new-project-init.sh` exists in v0.4.0 snapshot) — how opinionated should greenfield generation be?
4. Pricing: open-core CLI (free) + paid hosted dispatch + team packs?
