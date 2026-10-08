# /review-integration — human review of a release train (train → dev)

The open train (`integration/<yyyymmdd>-<slug>`) holds dispatcher-merged
ticket branches (`integrated` label). Merging the train into `dev` is a
HUMAN decision and a human action. Work through this checklist against the
train worktree (or a detached worktree at the train head) and record the
outcome on the train ticket/issue thread.

## 1. Combined gate

- [ ] `scripts/factory-check` passes on the train head (combined tree, not
      just per-ticket greens — two green branches can fail together).
- [ ] CI on the pushed train branch (`integration/**`) is green.

## 2. Membership sanity

- [ ] `factory-dispatch release status` lists exactly the tickets meant for
      this train — no stragglers, no missing members.
- [ ] Every member ticket carries the `integrated` label (not `accepted`;
      `accepted` is the train-less path awaiting human `/land`).

## 3. Overlapping-file scan

- [ ] List files touched by more than one member merge:
      `git log --merges --name-only --pretty=format: <train-base>..<train-head>`
      collapsed per file — overlapping files get a second look even when the
      merge itself was clean.
- [ ] Overlapping migrations: check migration order sanity (timestamps /
      sequence, no two migrations claiming the same version, up/down pairs
      consistent when run in filename order).

## 4. Two-green-branches-fail-together cases

- [ ] Seed/fixture collisions (both tickets adding the "same" record).
- [ ] Config/env additions with the same key and different values.
- [ ] Route, label, or glossary-term duplicates across tickets.
- [ ] Dependency/lockfile changes from two tickets (keep one coherent set).

## 5. Drift check

- [ ] The train was NEVER rebased by automation (dispatcher has no rebase
      path). If the train needs to catch up with `dev`, rebase ONLY on an
      explicit human command, then re-run the combined gate + CI.
- [ ] `git log --merges` on the train shows one merge per member ticket and
      one revert per `release drop` — no force-pushes, no rewritten history.

## Verdict

- Accept: merge train → `dev` yourself (human land), then
  `factory-dispatch release finish` (verifies the merge, marks the
  `releases` row `merged`, removes the train worktree), close member
  tickets.
- Reject a member: `factory-dispatch release drop <ticket>`, re-run the
  combined gate, continue with the reduced train.
- Abandon: `factory-dispatch release abandon` (marks the `releases` row
  `abandoned`, removes the train worktree — a new train can only start
  after the old one is merged or abandoned, never two open).
