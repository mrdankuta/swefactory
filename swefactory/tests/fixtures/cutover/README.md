# Cutover fixture histories (ticket #88)

Two checked-in SQLite databases stand in for the bash dispatcher's
`dispatch.db`. They were built once with the `sqlite3` CLI (schema copied
from `factory-v0.4.0/scripts/factory-dispatch`, `db_init`); no test needs
`sqlite3` at test time — the importer is pure Rust and so are the tests.

## `dispatch-fixture.db` — the main history

```sql
CREATE TABLE attempts(
  id INTEGER PRIMARY KEY, ticket INTEGER NOT NULL, attempt_no INTEGER NOT NULL,
  base_commit TEXT, head_commit TEXT, worktree TEXT, branch TEXT,
  status TEXT NOT NULL, exit_code INTEGER, log_path TEXT,
  started_at TEXT DEFAULT (datetime('now')), finished_at TEXT);
CREATE TABLE releases(
  id INTEGER PRIMARY KEY, branch TEXT NOT NULL, base_commit TEXT,
  status TEXT NOT NULL DEFAULT 'open',
  created_at TEXT DEFAULT (datetime('now')));
ALTER TABLE attempts ADD COLUMN train_branch TEXT;
INSERT INTO attempts(id,ticket,attempt_no,base_commit,head_commit,worktree,branch,status,exit_code,log_path,started_at,finished_at,train_branch) VALUES
 (1,101,1,'aaaa…(40)','bbbb…(40)','/tmp/wt/T101-a1','factory/T101-first','impl_done',0,'logs/T101-a1-impl.log','2026-09-01 10:00:00','2026-09-01 10:07:31','integration/20260901-train'),
 (2,101,2,'cccc…(40)',NULL,'/tmp/wt/T101-a2','factory/T101-second','review_failed',1,'logs/T101-a2-impl.log','2026-09-02 11:00:00','2026-09-02 11:09:02',NULL),
 (3,102,1,NULL,NULL,NULL,'factory/T102-first','impl_running',NULL,'logs/T102-a1-impl.log','2026-09-03 12:00:00',NULL,NULL);
INSERT INTO releases(id,branch,base_commit,status,created_at) VALUES
 (1,'integration/20260901-train','dddd…(40)','open','2026-09-01 09:00:00');
```

Row 1 is the full happy path (train-bound, finished). Row 2 is a failed
review with no head commit. Row 3 is a sparse still-running row in the
style of the bash `self-test` inserts (NULL commits, NULL exit, NULL
train) — it must read exactly like a pre-checkpoint Rust row. `log_path`
values are relative on purpose: the tests resolve them against the state
dir, exactly like a bash loop run from its repo root with a relative
`STATE_DIR`. (The `aaaa…`/`bbbb…`/`cccc…`/`dddd…` above are 40 identical
hex characters each — 40 `a`s, 40 `b`s, 40 `c`s, 40 `d`s — matching the
literals asserted in `cli_cutover.rs`.)

## `dispatch-fixture-no-train.db` — attempts, no open train

One finished attempt for ticket 201 and an empty `releases` table: the
import must still carry the attempt while writing no `train.json`.

## Regenerating

Page size 4096, UTF-8, default (rollback-journal) mode — whatever stock
`sqlite3` writes. Keep rows tiny (single b-tree page) so the pure-Rust
reader never meets overflow pages in fixtures; keep timestamps fixed
strings so assertions stay deterministic.
