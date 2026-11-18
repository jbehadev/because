# Data Model, Storage and Sharing

Implements decision **D2**: SQLite is a local rebuildable cache; the shared
record is append-only JSONL committed to the repository.

## 0. What is built *(`crates/because-store`, schema v1)*

The local store exists; the shared records (§1–§3, §6) do not yet. `cuz live`
writes every input to it as the input changes — sent, finished, its changes
computed, its summary written — and reads it back at start, so earlier
sessions are listed under this run's (`03-tui.md` §5b). Claude Code transcripts
for the repository are imported into it too (`04-agent-host.md`, transcripts).

Where it differs from the plan below, and why:

| Plan | Built | Why |
|---|---|---|
| `.because/cache/slicer.db`, ignored | `<git common dir>/because/slicer.db` | A database in the working tree is swept into every snapshot (`git add -A` into a private index) unless the repository's `.gitignore` is edited. The git directory is never snapshotted, and its common part is shared by every worktree, which is where `cuz live` runs its extra sessions — so there is one store per repository, as intended. It is still a per-clone cache; sharing stays the job of the records |
| text in `blobs`, referenced by digest | text inline (`segments.input`, `steps.body`) | Dedupe by digest matters across machines, which is the records' concern. Inline is simpler until then |
| all of §5 | `sessions`, `segments`, `steps`, `commits`, `file_changes`, `hunks` | The tables something writes today. `sections`, `jobs`, `summary_fts`, `blobs` and `exported` come with the code that fills them, as later steps in the migration ladder |
| `patch_ref = git:<sha>:<path>` | `git:<base>..<sha>:<path>` | An input's changes are a range — before- and after-snapshot, or the parent of its first matched commit and its last — not one commit's diff |
| no cost column | `segments.cost_usd`, NULL when unknown | Claude Code reports cost in dollars, not a usage object to price; a transcript records only running totals, so an input's own cost is sometimes unknown, and NULL says so rather than 0 |

What the store keeps is what nothing else can give back: what was typed, what
the agent said and did, what it cost, what was generated about it, and which
commits each input is tied to. A file change is a row of path, counts and
summary; its hunk text, the code around it and the per-line blame are
re-derived from git when the input is looked at (§4). If the snapshot refs are
deleted and the objects collected, the code view says it cannot load and the
rest of the record stands.

Migrations are the hand-rolled `user_version` ladder from §8: forward-only, one
SQL step per version, and a store newer than the binary is refused with a
message naming both versions.

## 1. Two-tier storage

```
.because/
├── config.toml              committed  — models, redaction rules, plugin config
├── records/
│   ├── 019283af….jsonl      committed  — one shard per session, append-only
│   └── 04bc71de….jsonl      committed
├── blobs/
│   └── 7f/7f3a9c….zst       committed  — content-addressed, only when needed
└── cache/
    ├── slicer.db            IGNORED    — rebuildable from records + git
    ├── slicer.db-wal        IGNORED
    └── slicer.db-shm        IGNORED
```

`records/` and `blobs/` are the durable truth. `cache/slicer.db` is an index:
delete it and `cuz replay` rebuilds it byte-for-byte from the records.

### Why not commit the `.db`

SQLite pages are not line-oriented. Two developers who each summarize one segment
on their own branch produce two `.db` files whose binary contents diverge
everywhere, and git's only options are "take mine" or "take theirs" — one
person's work is silently destroyed on every merge. Committing the log and
treating the DB as derived makes the merge trivial and the loss impossible.

## 2. Content addressing and canonical JSON

Every record's identity is the hash of its own payload:

```
id = blake3_256_hex(canonical_json(payload))
```

`canonical_json` is RFC 8785 (JCS)-style: object keys sorted by code point, no
insignificant whitespace, no trailing zeros in numbers, UTF-8 output. Implement
it once in `because-core` and test it against known vectors; getting it wrong means
the same logical record gets two ids on two machines, which breaks dedupe.

Consequences worth having:

- **Idempotent replay.** Applying a record twice is a no-op.
- **Order-independent merge.** Records carry their own identity, so interleaving
  from a `union` merge is harmless.
- **Free dedupe.** Two people who summarize the same commit with the same model
  and prompt version produce the *same* id and collapse to one row.

## 3. Record format

One JSON object per line, newline-terminated (the trailing newline matters —
see §6).

```json
{
  "v": 1,
  "id": "9f2c…",
  "kind": "file_change_summary",
  "ts": "2026-09-22T02:18:04.117Z",
  "actor": {"user": "dev", "tool": "because/0.1.0"},
  "payload": { }
}
```

| Field | Notes |
|---|---|
| `v` | record envelope version; bump only for envelope changes, not payload changes |
| `id` | `blake3(canonical_json(payload))`; **not** over the envelope, so re-exporting with a newer tool version does not change identity |
| `kind` | discriminant; unknown kinds are skipped with a warning, never a hard error |
| `cuz` | RFC 3339 UTC, millisecond precision; used only for display ordering |
| `actor` | provenance — who and what produced this. Never trusted for authorization, only shown |

Record kinds: `session`, `segment`, `section`, `step`, `commit_link`,
`file_change`, `hunk`, `file_change_summary`, `section_summary`, `segment_summary`,
`blob_ref`, `retraction`.

**Forward compatibility rule:** readers must ignore unknown `kind` values and
unknown keys inside a known payload. A teammate on a newer version must not
brick an older checkout.

**`retraction`** exists because you cannot delete a line from an append-only log
that others have already merged. A retraction names a target `id` and a reason;
replay drops the target. This is how a wrong summary or an accidentally captured
secret is removed. (A secret still needs the blob purged and rotated — see
`02-summarization.md` §8.)

## 4. Patches: point at git, don't copy it

A committed change's patch is *already in git*. Copying it into the record store
doubles the repo and invites the two copies to disagree. So `patch_ref` is a
tagged reference:

| Form | When | Resolution |
|---|---|---|
| `git:<sha>:<path>` | the change is in a reachable commit | `because-git` re-derives the patch on demand |
| `blob:<digest>` | working-tree change, or a commit later amended/rebased away | read `.because/blobs/<xx>/<digest>.zst` |

Rules:

- Capture as `blob:` at segment time (the working tree is the only place it exists
  yet), then **promote to `git:` once the change lands in a commit** and garbage
  collect the blob if nothing else references it. `cuz doctor --gc` does this.
- Blobs are zstd level 3, capped at 256 KiB per patch. Over the cap, store stats
  only (`additions`, `deletions`, `binary: true`) and mark the change
  `oversized`; summarize from the file path and stats, not the content.
- Blob paths are content-addressed and therefore **can never conflict** — two
  people writing the same blob write identical bytes to the same path.

## 5. SQLite schema (v1)

`rusqlite` with the `bundled` feature so no system SQLite is required.
Pragmas on every connection: `journal_mode=WAL`, `synchronous=NORMAL`,
`foreign_keys=ON`, `busy_timeout=5000`.

```sql
CREATE TABLE schema_meta (
    version     INTEGER NOT NULL,
    applied_at  TEXT    NOT NULL
);

-- ── identity of a working session ───────────────────────────────────────────
CREATE TABLE sessions (
    id            TEXT PRIMARY KEY,          -- uuidv7, sortable by creation
    repo_root     TEXT NOT NULL,
    branch_start  TEXT,
    title         TEXT,                      -- generated after segment 1
    model         TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    ended_at      TEXT,
    status        TEXT NOT NULL              -- active|idle|ended|failed
);

-- ── one input and the agent's full response: the middle column ─────────────
CREATE TABLE segments (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    prompt_ref    TEXT NOT NULL,             -- blob digest of the user prompt
    started_at    TEXT NOT NULL,
    ended_at      TEXT,
    model         TEXT NOT NULL,
    stop_reason   TEXT,
    usage_json    TEXT,                      -- full usage object incl. cache + cost
    summary       TEXT,                      -- one-paragraph rollup
    summary_state TEXT NOT NULL DEFAULT 'pending',
    UNIQUE (session_id, seq)
);

-- ── mechanical structure of a segment: the raw steps, before interpretation ───
CREATE TABLE steps (
    id            TEXT PRIMARY KEY,
    segment_id       TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    kind          TEXT NOT NULL,             -- text|thinking|tool_use|tool_result
    tool_name     TEXT,
    body_ref      TEXT,                      -- blob digest
    ts            TEXT NOT NULL,
    UNIQUE (segment_id, seq)
);

-- ── labelled parts of one segment's response, sliced by the summarizer ─────
CREATE TABLE sections (
    id            TEXT PRIMARY KEY,
    segment_id       TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    title         TEXT NOT NULL,             -- "switch retries to per-request"
    intent        TEXT,                      -- one sentence: what this achieves
    step_start    INTEGER NOT NULL,          -- inclusive index into steps.seq
    step_end      INTEGER NOT NULL,          -- inclusive
    summary       TEXT,
    confidence    REAL,                      -- 0..1 self-reported by sectioner
    model         TEXT,
    prompt_ver    TEXT,                      -- which prompt template produced this
    created_at    TEXT NOT NULL,
    UNIQUE (segment_id, seq)
);

-- ── git commits, linked to the segment that produced them when known ──────────
CREATE TABLE commits (
    sha           TEXT PRIMARY KEY,
    parent_sha    TEXT,
    author        TEXT,
    committed_at  TEXT,
    subject       TEXT,
    segment_id       TEXT REFERENCES segments(id) ON DELETE SET NULL
);

-- ── the unit of review: one file, in one change event ──────────────────────
CREATE TABLE file_changes (
    id              TEXT PRIMARY KEY,
    segment_id         TEXT REFERENCES segments(id) ON DELETE CASCADE,
    commit_sha      TEXT REFERENCES commits(sha) ON DELETE SET NULL,
    path            TEXT NOT NULL,
    old_path        TEXT,                    -- set on rename
    change_type     TEXT NOT NULL,           -- add|modify|delete|rename|mode|binary
    additions       INTEGER NOT NULL DEFAULT 0,
    deletions       INTEGER NOT NULL DEFAULT 0,
    patch_ref       TEXT,                    -- git:<sha>:<path> | blob:<digest>
    patch_digest    TEXT,                    -- digest of the patch text as summarized
    oversized       INTEGER NOT NULL DEFAULT 0,
    summary         TEXT,
    risk            TEXT,                    -- none|behavioral|api|security|perf|data
    summary_of      TEXT,                    -- patch_digest the summary was derived from
    summary_state   TEXT NOT NULL DEFAULT 'pending', -- pending|running|ok|failed|stale|skipped
    model           TEXT,
    prompt_ver      TEXT,
    created_at      TEXT NOT NULL,
    CHECK (segment_id IS NOT NULL OR commit_sha IS NOT NULL)
);
CREATE INDEX file_changes_path      ON file_changes(path);
CREATE INDEX file_changes_segment   ON file_changes(segment_id);
CREATE INDEX file_changes_commit    ON file_changes(commit_sha);
CREATE INDEX file_changes_state     ON file_changes(summary_state)
       WHERE summary_state IN ('pending','running','failed');

-- ── hunk-level labels, so a 400-line diff reads as a list ──────────────────
CREATE TABLE hunks (
    id              TEXT PRIMARY KEY,
    file_change_id  TEXT NOT NULL REFERENCES file_changes(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    old_start       INTEGER NOT NULL,
    old_lines       INTEGER NOT NULL,
    new_start       INTEGER NOT NULL,
    new_lines       INTEGER NOT NULL,
    header          TEXT,                    -- the @@ … @@ context, e.g. fn name
    summary         TEXT,
    UNIQUE (file_change_id, seq)
);

-- ── which changes belong to which slice of the segment ────────────────────────
CREATE TABLE section_file_changes (
    section_id      TEXT NOT NULL REFERENCES sections(id) ON DELETE CASCADE,
    file_change_id  TEXT NOT NULL REFERENCES file_changes(id) ON DELETE CASCADE,
    PRIMARY KEY (section_id, file_change_id)
);

-- ── content-addressed body store, mirrors .because/blobs ─────────────
CREATE TABLE blobs (
    digest      TEXT PRIMARY KEY,
    size        INTEGER NOT NULL,
    encoding    TEXT NOT NULL,               -- zstd|raw
    data        BLOB NOT NULL
);

-- ── durable work queue; survives a crash mid-summarization ─────────────────
CREATE TABLE jobs (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,             -- section_segment|summarize_file|rollup_segment
    target_id     TEXT NOT NULL,
    payload_json  TEXT,
    state         TEXT NOT NULL,             -- queued|running|done|failed|dead
    attempts      INTEGER NOT NULL DEFAULT 0,
    not_before    TEXT,                      -- backoff
    last_error    TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX jobs_runnable ON jobs(state, not_before);

-- ── records already exported, so `cuz export` is incremental ────────────────
CREATE TABLE exported (
    record_id   TEXT PRIMARY KEY,
    shard       TEXT NOT NULL,
    exported_at TEXT NOT NULL
);

-- ── search over summaries, not code ────────────────────────────────────────
CREATE VIRTUAL TABLE summary_fts USING fts5(
    kind, ref_id UNINDEXED, path, title, body,
    tokenize = 'unicode61 remove_diacritics 2'
);
```

Notes on the shape:

- `steps` (mechanical) is kept separate from `sections` (interpreted). A wrong or
  stale sectioning can always be thrown away and regenerated, because the raw
  structure survives independently. Never let the model's output be the only
  copy of a fact.
- `summary_of` next to `patch_digest` is the staleness mechanism (§7).
- `prompt_ver` on every generated row means you can find and regenerate
  everything produced by a prompt template you later decide was bad.
- No `cost` column: derive it from `usage_json` and a price table, so a pricing
  change does not require rewriting history.
- `narration_links` and `line_runs`, plus three added `steps` columns, are
  specified in `07-provenance.md` §4. They are part of schema v1; they live in
  that document because they only make sense alongside the anchoring design.

The DDL above was executed against SQLite 3.45 and creates 12 tables, 5 indexes
and the FTS5 virtual table; the `file_changes` `CHECK` was verified to reject a
row with neither a `segment_id` nor a `commit_sha`.

**FTS5 escaping is not optional.** Raw user input passed to `MATCH` is parsed as
FTS5 query syntax, and summaries are full of the characters that syntax uses:

```
MATCH 'per-request'    -> OperationalError: no such column: request
MATCH '"per-request"'  -> 1 hit
```

So the `/` search in the TUI must wrap each user term in double quotes (doubling
any embedded `"`), or offer an explicit "raw query" mode for people who want
`NEAR`, `OR` and prefix matching. A naive passthrough turns every hyphenated
search into an error dialog.

## 6. Merge semantics

`.gitattributes` in the repo root:

```
.because/records/*.jsonl  merge=union -diff
.because/blobs/**         binary -diff -merge
.because/cache/**         export-ignore
```

`merge=union` concatenates both sides' added lines instead of conflicting. This
is safe **only** because:

1. every line is a complete, standalone JSON object;
2. every line ends with `\n`, so union never fuses two records into one line;
3. records are identified by content hash, so duplicates collapse at replay;
4. records are never edited in place — corrections are new records, deletions
   are `retraction` records.

Violating any of those four turns union merge into silent corruption, so they
belong in a test: a property test that takes two record sets, interleaves them
arbitrarily, replays, and asserts the resulting DB is identical either way.

Sharding one file per session keeps concurrent writes on separate branches in
separate files most of the time, so union merge is a backstop rather than the
common path.

## 7. Staleness

A summary describes a specific patch. Rebases, amends and follow-up edits change
the patch underneath it.

```
if resolve(patch_ref).digest() != row.summary_of {
    row.summary_state = 'stale'
}
```

Stale rows render with a dim marker and the summary still visible — a stale
summary is usually still roughly right and is better than nothing — plus a
keybinding to requeue. `cuz doctor` reports the stale count. Requeueing a stale
row is cheap because the patch is re-derived from git rather than stored.

## 8. Migrations

`refinery` with embedded SQL migrations, or a hand-rolled `user_version` ladder
(the schema is small enough that hand-rolled is defensible and drops a
dependency). Either way:

- forward-only, one file per version, never edited after release;
- `cuz` refuses to run against a DB with a *newer* schema version than the binary
  knows, and says which version it needs;
- because the DB is a cache, the escape hatch for any migration that turns out
  to be wrong is `rm -rf .because/cache && cuz replay`. Test that path in
  CI; it is the real disaster recovery story.
