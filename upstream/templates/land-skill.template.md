---
name: land
development: false
description: >-
  Lands the current thread's reviewed changes onto origin/dev for the
  BooksBuddy Phoenix app. Invoke only when the user has explicitly requested
  landing (Land Changes button, /land, or equivalent merge-request language) —
  never for review, preparation, passing checks, or skill installation.
metadata:
  delta-action: land
---

# Land on origin/dev

You are executing an explicit landing request. Do not ask whether the user
wants to merge — the invocation itself is the approval. Keep the repo's
checks and safeguards; stop only for genuine blockers or unresolved scope.

Context: BooksBuddy (`acctmgt_app`, Elixir/Phoenix) lands as linear
Conventional Commits pushed directly to `origin/dev`. There are no CI
workflows, no PR process, and no branch protection on this repo — local
verification is the gate. The agent works in a Delta worktree (a clone);
`local` is the user's checkout (never push there for publication), `origin`
is GitHub (`https://github.com/rarefytech/booksbuddy.git`).

All terminal commands run in the thread's Delta worktree (pass its UUID via
the terminal `worktree` parameter). Prefix any git command that might open
an editor with `GIT_EDITOR=true `.

## 1. Preflight

- `git --no-optional-locks status -sb` and `git --no-optional-locks diff --stat`:
  confirm the change set is the intended, reviewed scope. Unrelated or
  exploratory files do not land — leave them uncommitted and say so.
- Never stage secrets (API keys, tokens, credentials, `.env` files). If a
  secret is in the diff, stop and report.
- `git fetch origin` and compare with `origin/dev`. Note divergence now;
you will reconcile at publication.

## 2. Verify (repo gate)

- Run `mix precommit` (repo rule, `AGENTS.md` Project guidelines; alias
  defined in `mix.exs` `aliases/0`: `compile --warning-as-errors`,
  `deps.unlock --unused`, `format`, `test`). Fix failures; do not land
  around them.
- `mix ci` (same source: adds `format --check-formatted`, `credo --strict`,
  `dialyzer`, `ex_dna`) is optional unless the change touches risky areas
  (auth, migrations, money/tax math) — then run it too.

## 3. Commit

- Follow the installed `commit` skill's conventions: inspect status/diff,
  stage specific files only, Conventional Commit `type(scope): subject`
  (e.g. `feat(deliverables): ...`), imperative subject, body explaining why
  plus any migrations or follow-ups.
- If there is nothing worth committing or the diff is not understood, stop
  and report — that is not a landing.

## 4. Publish to origin/dev

- Push the `dev` branch to `origin` with plain `git push origin dev`.
  Never use `--force`, never rewrite history on `dev`, never push to the
  `local` backlink for publication.
- If the push is rejected (diverged): `git pull --rebase origin dev` and
  retry the push.
- Conflict preference for this project is AUTOMATIC resolution: resolve
  only when the merged result is clear (e.g. non-overlapping hunks on
  unrelated files — complete the rebase, re-run `mix precommit`, continue).
  Still stop and report without resolving when changes overlap ambiguously,
  any side's intent is unclear, verification fails after resolution, or the
  operation would drop anyone's work.

## 5. Confirm the landing

- `git --no-optional-locks status -sb` is clean and `origin/dev` contains
  the new commit (`git --no-optional-locks log --oneline -3 origin/dev`).
- Prepared commits, a pushed topic branch, or passing checks alone are not
  success. Pending, failing, or unverifiable checks must not be treated as
  success.

## 6. Report the outcome

- In a subthread where `report_subthread_status` is available, report
  there; otherwise report directly in conversation.
- `success` only after step 5 verifies the commit on `origin/dev`.
  `failure` for any blocker, with what is needed to unblock.
- Keep `title` to a few sentence-case words and `description` to one short
  line with verified links only (short SHA; no invented URLs):
  - success: title `Landed on dev`, e.g. description `` `abc1234` on
    origin/dev · precommit clean. ``
  - failure: e.g. title `Push blocked` / `Merge conflicts`, description
    names the commit state and what is required.
- Failure is not terminal: continue safe recovery when permitted and
  report the updated outcome after re-verification.
