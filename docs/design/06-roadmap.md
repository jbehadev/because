# Roadmap, Risks, Open Questions

## 1. Sequencing principle

The novel part of `because` is summary-first review. The expensive part is
hosting an agent. So the order below puts a **usable, judgeable product in front
of you before the agent host exists**: M1–M3 work entirely over git history with
no conversation at all, which means you can point `cuz` at any existing repo and
see whether reading sections and change summaries actually beats reading diffs.
If it doesn't, you find out in weeks instead of months.

## 2. Milestones

### M0 — Skeleton (no behaviour)
- Cargo workspace, the eight crates from `00-overview.md` §6, CI (fmt, clippy
  `-D warnings`, test, `cargo deny`).
- `because-core`: ids (uuidv7), canonical JSON (RFC 8785), blake3 digests, domain
  types. **Test canonical JSON against fixed vectors** — everything downstream
  depends on two machines agreeing on a hash.
- `because-store`: schema v1 DDL, migration ladder, single-writer actor, read pool.
- Replace the Python `.gitignore` with a Rust one; add `.gitattributes`.
- **Done when:** `cargo test` passes and an empty DB migrates to v1.

### M0.5 — Walking skeleton *(built)*

Not a throwaway prototype: the real `view`/`update` running against a fixture
instead of a store.

- `crates/because-core` — the domain types the UI renders; `crates/because-tui` — `Model`,
  `update`, `view`; `crates/because-cli` — `cuz` plus `cuz render <w>x<h> <script>`,
  which renders frames through ratatui's `TestBackend` and prints them, so the
  UI can be reviewed with no TTY.
- `fixtures/ripgrep.json` — real ripgrep commits with real diffs, hunk ranges,
  post-image content and blame origins, built by `fixtures/build_fixture.py`.
  Summaries and narration are authored, not generated, and live in
  `fixtures/authored.json`; see `fixtures/README.md`.
- Headless tests (`crates/because-tui/tests/`) covering the focus ring, per-change
  cursor retention, the this-input/earlier-input tier split, line resolution
  across inputs, collapsed deletion markers, the provenance markers, clipping
  at five widths, and — in `keys.rs` — which keys type and which command.

**What it settled:**

1. *The mock-versus-real question is a false one.* Because `update` is pure and
   `view` does no I/O, the fixture-driven build and the real build share the
   entire view layer — swapping the fixture loader for a `StorageEngine` changes
   nothing above it. There was nothing to throw away.
2. *Headless rendering works and belongs in CI.* `TestBackend` plus a scripted
   key sequence makes every screen assertable without a terminal. This is worth
   keeping as the main UI test strategy.
3. *Git hunks do not align with semantic sections* — see open question 1 below.
4. *Wrapping has to be owned in one place.* Wrapping text manually and also
   setting ratatui's `Wrap` double-wraps and destroys indentation. The columns
   now wrap once, in `wrap()`/`ellipsize()`.

**Composer and layered keymap** (added after the first skeleton): the Segments
column gained a composer for typing a new segment, and the keymap moved into
`crates/because-tui/src/keymap.rs` as three layers (global, composer, command). 17
key-level tests in `crates/because-tui/tests/keys.rs` drive real `KeyEvent`s through
`dispatch`, because the property under test is which keys type and which keys
command. What building it settled:

5. *The text box chose the modifier.* ratatui-textarea's default map claims
   `Ctrl+←/→` (word movement), `Ctrl+W` (delete word), `Ctrl+C` (copy) and
   `Ctrl+J`; it leaves `Alt+←/→`, `Alt+↑/↓` and `Alt+1…4` untouched. So
   `Ctrl+←/→` was dropped as pane movement altogether and the `Ctrl+w h/l`
   chord went with it. `03-tui.md` §4 has the full collision table.
6. *`tui-textarea` 0.7.0 does not work with ratatui 0.30* — it pins 0.29, and
   the two versions' `Widget` traits do not unify. `ratatui-textarea` 0.9.2 is
   built on `ratatui-core` 0.1 and resolves to a single ratatui.
7. *`Ctrl+Enter` cannot be the submit key* — it is CR, same as `Enter`, without
   keyboard enhancement. `Enter` submits; newline is `Alt+Enter`, `Ctrl+J`, or
   `Shift+Enter` where supported.
8. *Bracketed paste is mandatory*, or a multi-line paste submits its first line.
9. *A typed segment has to be selectable.* The first version appended it to the
   list and it was clipped off the bottom, because the scroll rule protects the
   selection. Selecting what was just queued fixes the scroll and gives the
   Changes column something truthful to say.

**Segments are inputs** (the decision recorded in open question 7): the middle
column lists the inputs of a conversation, and the fixture was rebuilt around a
real four-input sequence on `pathutil.rs` (`626b895` → `6e527e9` → `c55818f`,
plus a question answered without changes). It is built by
`fixtures/build_fixture.py` from git, with all authored text isolated in
`fixtures/authored.json`. What it settled:

10. *Earlier inputs' lines have to be found by blame.* Each input's hunks are
    numbered against its own version of the file; by input 3 they point at the
    wrong lines. The fixture now carries per-line origins from `git blame`, and
    the code pane labels each surviving line with the input that wrote it. Of
    45 lines input 1 wrote, 33 survive input 2 and 25 survive input 3. This
    pulls the anchoring work from `07-provenance.md` into M1.
11. *`W` must resolve the line, not the selection.* Viewing input 3, a line can
    belong to input 2; the panel follows the cursor line through blame to the
    input, hunk, sections and narration that produced it.
12. *A real render caught what the tests did not.* Wrapping in the Segments
    column was off by one against a 7-cell prefix and clipped a character at
    120 columns; it fit by accident at 150, the width the tests used. A test now
    rebuilds the input text from rendered cells at five widths, and was
    confirmed to fail without the fix.
13. *Unit tests passed while a Mac terminal failed.* The key tests fed the
    keymap ideal events like `Alt+Left`; Terminal.app and iTerm2 send
    Option+←/→ as `ESC b` / `ESC f`, which arrive as `Alt+b` / `Alt+f`, and the
    TUI opens in the composer, where plain ←/→ moved a cursor through an empty
    draft. Reported from a real Mac. Fixed by letting every arrow form navigate
    while the draft is empty (`03-tui.md` §4), and caught from now on at the
    byte level by `scripts/pty_keys.py`, which runs the real binary in a
    pseudo-terminal — confirmed to fail on the old keymap. `cuz keys` was built
    so the next terminal-specific report comes with data.
14. *A reply is not a summary, and the column cannot hold one.* In real use a
    ~1,000-word answer filled the selected segment's six lines and the rest was
    unreachable. The Segments column now shows a one-sentence summary from
    Haiku through the Claude Code CLI (about $0.001–0.002 and 1.3 s per input
    with thinking off; 7–9 s with it on), and a full-screen reader holds the
    whole input and response (`03-tui.md` §5, `02-summarization.md` §0). This
    brings a thin slice of M2 forward; passes A–C are still unbuilt.
15. *Sessions were lost on quit, and the store went in the git directory.*
    `because-store` now keeps every input `cuz live` runs and imports Claude Code's
    transcripts, so earlier sessions are listed under the current ones
    (`03-tui.md` §5b). The plan's `.because/cache/` would have been swept
    into every snapshot; the store went into the git common directory instead,
    which every worktree shares (`01-data-model.md` §0). Imported inputs are tied
    to commits by time: 16 of 17 right on this repository's own history, the
    miss being a commit the developer made by hand mid-input — which is why the
    UI labels those links.

### M1 — Read-only TUI over git (no LLM)
- `because-git`: walk a rev range, extract per-file changes and hunks, re-derive
  patches from `git:<sha>:<path>` refs.
- `cuz index <rev-range>` populates `commits`, `file_changes`, `hunks`.
- `because-tui`: three columns, the four-stop focus ring, responsive breakpoints,
  keymap-driven help. Left column shows commits (there are no sessions yet),
  middle shows files, right shows hunks.
- **Code pane** (`03-tui.md` §6): post-image file view with this-input/earlier-input
  gutter tiers, collapsed deletion markers, `u` full-context diff toggle,
  revision selector, sticky header, syntect snapshot cache. All of this is
  git-only, so it lands here rather than waiting on the agent — and it is the
  half of the UX that decides whether summary-first review is usable.
- `cuz keys` key probe — built early, in M0.5, after the Mac arrow failure
  below.
- **Anchoring** (`07-provenance.md` §3): `line_runs` maintained incrementally
  during `cuz index`, blame fallback with per-`(commit,path)` caching and
  prefetch on code-pane entry, `W` rationale panel, `cuz why <path>:<line>`.
  Git-only, so it is testable against this repo's own history before any
  narration exists to attach — at this stage the panel shows commit, summary
  and hunk, and the narration region is empty.
- **Done when:** `cuz index HEAD~200..HEAD && cuz` browses 200 commits of a real
  repo at the performance targets in `03-tui.md` §8, with no network calls, and
  the code pane opens on a 20k-line file in under 100 ms.

### M2 — Summarization
- `ts-summarize`: Haiku client (raw HTTP), structured outputs, the three passes,
  durable job queue, bounded concurrency, backoff, cost accounting.
- Redaction gate (`02-summarization.md` §8) **before** the first API call is ever
  made, not after.
- `cuz summarize <rev-range> --batch` for history backfill via the Batches API.
- Eval set: 30–50 labelled changes, `risk` accuracy + LLM-judge rubric.
- FTS widened to narration (`/!`) and the generated-vs-verbatim trust markers,
  so a summary is never mistaken for something the model actually said.
- **Done when:** summaries appear for a backfilled range, `cuz doctor` reports
  cache-hit and cost, and the eval produces a number you can regress against.

### M3 — Shared records
- *Started:* the local half — `because-store`, schema v1, written by `cuz live` and
  the transcript importer (`01-data-model.md` §0). The records below are not.
- `ts-record`: envelope, kinds, export/import/replay, retractions,
  blob promotion (`blob:` → `git:`) and GC.
- Property test: arbitrary interleavings of two record sets replay to identical
  DBs.
- Staleness detection and requeue-from-UI.
- Retention tiers (`07-provenance.md` §5) in the record format, and the
  **inbound** redaction gate over narration bodies — the direction that writes
  model prose into the repository rather than sending it to an API.
- **Done when:** two clones can each summarize different work, merge with
  `merge=union`, and both see the union of summaries with correct provenance and
  no duplicates.

### M4 — Agent host via the Claude CLI *(running: `cuz live`)*

Built so far: `crates/because-agent` (stream parser and process wrapper, tested by
replaying a recorded real session), `crates/because-git` (snapshots under
`refs/because/`, diffs and blame between them, tested against real
temporary repositories), live segments in the TUI (streaming text, tool calls,
queued inputs, per-input cost from each session's own cumulative totals), and
`cuz live`. Verified
end to end in a pseudo-terminal against the real CLI: two inputs, the second
queued behind the first, the code screen labelling input 1's lines in input 2's
view, and `W` showing what the agent said. Since then: a one-sentence summary
per input and the reader, verified the same way against a real 588-word reply
(finding 14). Then **several sessions at once** (`03-tui.md` §5a): `n` starts a
conversation of its own, and because two agents cannot share a checkout without
making the snapshots meaningless, each session after the first runs in its own
git worktree, branched from a snapshot of the main tree. What that leaves open
is how the work in a session worktree comes back — merging two sessions'
changes to one file is a review problem, not a git one. Since then: the store
(`01-data-model.md` §0), so every input is kept and earlier sessions — this
tool's and Claude Code's own, imported from transcripts — are listed as
history; the **permission modal** over `--permission-prompt-tool stdio`
(`03-tui.md` §5c), tested against the Agent SDK's control-protocol shapes
rather than a recording; **reviving** an earlier session with `claude --resume`
(`03-tui.md` §5b); and the subscription allowance at the foot of the Sessions
column (`04-agent-host.md`). Real cancellation is built: Ctrl+C sends the
CLI's `interrupt` request (`04-agent-host.md`). Still to do from the list below:
narration capture. Snapshots happen
per input, not per tool call, so one input is one section until there is a
summarizer or `structuredPatch` is used.

Original plan:
- `AgentBackend`/`AgentSession` traits; `ClaudeCliBackend` over
  `claude -p --output-format stream-json --input-format stream-json`.
- Event translation tested against committed CLI output fixtures.
- Live segment streaming into the middle column, prompt input pane, permission
  modal, real cancellation.
- Sessions fill the left column and each input becomes a segment in the middle
  one; sectioning now runs with real inputs, so summaries get their "why".
- Narration capture (`07-provenance.md` §2): the mutation funnel records the run
  of text and thinking steps preceding each edit, `narration_links` fills in,
  and the rationale panel built in M1 starts returning verbatim reasoning. The
  `v` verbatim toggle and the `t` transcript overlay land here.
- **Done when:** a full prompt → segment → sections → change summaries loop runs
  inside the TUI.

### M5 — Native Messages API backend
- `MessagesApiBackend`: SSE streaming, the tool loop, the six tools, strict
  schemas, prompt caching with verified cache reads, compaction or context
  editing for long sessions.
- Switchable via `[agent] backend = "messages-api"`; the CLI backend stays as a
  fallback and as a differential test oracle (same prompt, both backends,
  compare the resulting change sets).
- **Done when:** a multi-hour session survives past the context limit and the
  CLI dependency is optional.

### M6 — Plugins
- Freeze the trait signatures against the ABI rules; add a conformance test
  suite any `StorageEngine` must pass.
- `plugins-wasm` feature with an extism host, capability grants, fuel metering.
- A second storage engine (even a trivial JSONL-only one) to prove the seam.
- MCP tool plugins via `rmcp`.
- **Done when:** a storage engine outside the workspace passes the conformance
  suite unmodified.

## 3. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| **Summaries are not trusted, so nobody skips the diff** — the entire premise fails | highest | M1–M3 test this before the agent host is built. The eval in M2 makes "is it any good" a number. Never hide the patch; summary-first, not summary-only. |
| Hosting the agent is a large, unglamorous build | high | Two backends behind one trait; CLI backend first (M4), native later (M5). Nothing is thrown away. |
| CLI stream-json schema is not a stable contract | medium | Pin a CLI version, parse permissively (ignore unknown event kinds), test against committed fixtures so upgrades fail loudly in CI. |
| Secrets in diffs reach a hosted API, or worse, get committed in records | high | Hard redaction gate before the trait boundary; skip-not-mask by default; documented retraction + rotation procedure. |
| **Narration committed to the repo leaks a secret the model quoted** | high | Inbound content scan over every narration body; a hit keeps the body local-only and commits a placeholder. Prose has no path, so path exclusion cannot help — `07-provenance.md` §6. |
| Record store grows without bound | medium | `narration` tier by default (~13 MB/month for a team of five, vs ~350 MB at `full`); `cuz doctor` warns past a threshold; per-session shards can be `git rm`'d without breaking replay. |
| Anchoring misattributes after a move or reformat | medium | `-C -M`, honour `.git-blame-ignore-revs`, and name the resolved commit in the panel so a move is visible as a move rather than passed off as intent. |
| Stale summaries after rebase mislead a reader | medium | `summary_of` vs `patch_digest`, explicit `stale` state, visible marker, cheap requeue. |
| Cost surprises | low | ~$0.03/segment measured (`02-summarization.md` §4); live cost meter; Batch API for backfill. |
| SQLite write contention | low | Single writer actor; WAL; no retry-on-busy logic needed. |
| `ratatui` breaking releases | low | Pin exact versions; UI logic is in pure `update`/`Model` so only `view` is exposed to churn. |

## 4. Open questions

1. **Section ↔ change attribution — now with two pieces of evidence.** In
   ripgrep `241b87b`, git's hunk 1 spans new-file lines 579–634 and contains
   work belonging to two sections. In `c55818f` it is starker: at `-U3` the
   whole change is **one hunk** (`-83,110 +83,89`) holding three sections'
   work, so every line it wrote resolves to all three in the rationale panel.
   Git splits on textual adjacency; sections split on intent. So a hunk is not
   the atom either — attribution wants line ranges. Options, in increasing cost:
   attach sections at hunk granularity and show all that cite a hunk (what the
   skeleton does, and says so on screen); split hunks at section boundaries at
   index time; or record line ranges per section when narration is captured,
   which the agent host can do exactly because it sees each edit separately.
   Leaning toward the third: the mutation funnel (`04-agent-host.md` §2) already
   knows which edit call produced which lines, before git merges them into
   hunks.
2. **Uncommitted work.** Should the left column show a synthetic "working tree"
   node above the commits, so a segment's changes are visible before they are
   committed? Probably yes, and it needs a `notify`-based watcher for edits made
   outside `because`.
3. **Cross-segment narrative.** Reviewing a whole branch means reading N segment
   summaries. A branch-level rollup pass (Sonnet, on demand) may be the actual
   killer feature for PR review. Defer, but leave room in the schema.
4. **Multi-repo.** The schema keys on `repo_root`. Monorepo-adjacent teams with
   several repos per session would need a first-class repo table. Not v1.
5. **Prompt-template distribution.** If prompts live in the binary, a team
   cannot fix a bad summary style without a release. If they live in
   `.because/config.toml`, they are reviewable and versioned — but then
   `prompt_ver` needs to be a content hash of the template, not a label.
   Leaning toward templates in config, hashed.
6. **Narration for segments that change no files — resolved.** An input
   answered in conversation is a segment like any other: it appears in the
   middle column, its Changes column says there are no file changes, and `v`
   shows the answer. The fixture has two. Still open: a segment whose edits a
   later input fully reverted keeps its narration, but `line_origins` will show
   none of its lines surviving — worth a visible marker so "we tried this and
   backed it out" is findable.
7. **What is a segment? — decided.** A segment is one input, as in Claude
   Code: four inputs in a conversation are four segments, in order. Submitting
   the composer sends the next input and adds it as the next segment of the
   session (queued until the agent is idle). The model-sliced parts of a
   response are **sections**, one level down. This replaced an earlier model in
   which the middle column listed sections of a single response; the schema,
   traits, code and these docs were renamed accordingly (`turn` → segment,
   old `segment` → section).
8. **Do sections need to be editable by hand?** A developer correcting a wrong
   section title is valuable provenance, and the record store already supports
   corrections as new records. Cheap to add, easy to defer.

## 5. What "done" looks like for v1

A developer on a team clones a repo, runs `cuz`, and can answer "what changed in
the last week and why" by reading two columns of prose, dropping into the diff
only where a summary raised a flag — with the summaries written by whichever
teammate's agent made the change, shared through git, and re-derivable from
nothing but the repo.
