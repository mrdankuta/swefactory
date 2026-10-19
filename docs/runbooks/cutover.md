# Cutover runbook: bash SQLite loop → Rust JSON daemon (ticket #88)

One-shot, human-authorized. The importer (`factory dispatch import`) is
the only bridge: it reads the bash dispatcher's `dispatch.db`
(`attempts` + `releases` rows) and writes the Rust JSON ledger
(`dispatch.json` + `train.json`), then archives the `.db`
byte-identical. After the pass the JSON ledger is the only store — no
dual reads, no dual writes, and the binary never links sqlite (the
importer parses the SQLite format in pure Rust, so musl-static stays
dependency-free).

## The window

1. **Stop the bash loop.** Nothing may write the `.db` while the import
   runs (the importer re-hashes the source afterwards and refuses to
   report success if it changed under it, but do not rely on that —
   stop the loop first).
2. **Run the import** from the project root (or anywhere, with
   `FACTORY_STATE_DIR` pointing at the live state):
   `factory dispatch import`
   (`--db PATH` overrides the default `<state>/dispatch.db`).
   Expected: `import: N attempt(s) …`, the open train (or `no open
   train`), and `archived byte-identical … (sha256 …)`.
3. **Verify the read is identical.** On the same state dir, compare what
   `sqlite3` said before with what Rust says now:
   - `sqlite3 <state>/dispatch.db.archived "SELECT id,ticket,attempt_no,status FROM attempts;"`
     against `factory dispatch status` (and `--json`);
   - per ticket, the transcript files the rows point at against
     `factory log <ticket>` (and `--json`, `--tail` as needed).
   Same ids, tickets, attempt numbers, statuses, commits, timestamps —
   same story.
4. **Start the Rust daemon:** `factory dispatch daemon`. The bash loop
   stays stopped; the archived `.db` is the audit trail.

## Guarantees and sharp edges

- **One-shot.** The importer refuses when the state dir already holds a
  `dispatch.json`, a `train.json`, or a `dispatch.db.archived` — a second
  pass can never duplicate rows. To re-cutover, start from a fresh state
  dir.
- **Source never mutated.** The `.db` is opened read-only and hashed
  before and after; the archive is verified byte-for-byte. Keep the
  archive as long as you keep the ledger.
- **Closed releases stay behind.** Only the open train has a live Rust
  equivalent (`train.json`). Closed releases are terminal history: they
  are counted in the report, never carried, and always preserved in the
  byte-identical archive. Nothing is lost; nothing live is dropped.
- **Releases never had two opens.** The bash loop refused a second open
  train, and the importer mirrors its reader (newest open wins) if
  history ever disagrees.
- **Hermetic proof.** `cargo test --test cli_cutover` replays the whole
  pass against checked-in fixture histories — no live state, no network,
  no `sqlite3` at test time.
