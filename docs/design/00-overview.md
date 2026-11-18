# `because` — Overview and Architecture

> Status: the plan of record, partly built. Six of the eight crates in §6 exist
> and `cuz` runs a live session with Claude Code as the agent; what is built and
> where it departs from this plan is recorded in each document's "built"
> sections and in `06-roadmap.md` §2. Revise these documents in place as
> decisions change.

## 1. Thesis

Reviewing an agent's work by reading its diff does not scale. One input can send
the agent across twenty files and several hundred lines, and the diff carries no
record of *why* any of it happened. The intent lives in what the developer typed
and in the model's reasoning, both of which are gone once the conversation
scrolls away.

`because` keeps that intent and attaches it to the code. Every input the
developer types becomes a segment; each segment's response is sliced into
labelled sections; every file change gets a summary generated *with the input
and the section in context*; and all of it is stored so it can be read later,
by a different person, on a different machine.

Comments in source are the usual place people try to put this, and they rot and
annoy. A sidecar store does not touch the code.

## 2. Vocabulary and the three columns

| Term | Means | Example |
|---|---|---|
| **session** | one conversation with the agent | "hidden-path checks for incremental walking" |
| **segment** | one input the developer typed, and everything the agent did in response to it. Four inputs are four segments. The Messages API calls this a turn. | `› now add a variant that takes a plain Path` |
| **section** | a labelled part of one segment's response, sliced by the summarizer so a long response reads as a list | "share the leading-dot check" |
| **change** | one file touched by one segment, with its hunks | `crates/ignore/src/pathutil.rs  +59 −14` |
| **narration** | the model's own words before an edit — verbatim, never generated | `❝ Both functions need the same leading-dot test…` |

An earlier revision of these docs used "turn" for a segment and "segment" for a
section. The schema, the traits and the code all use the terms above.

| Column | Shows | Grain |
|---|---|---|
| Left — **Sessions** | conversations, with input count and cost | one row per session |
| Middle — **Segments** | the selected session's inputs, in order: what you typed, a generated `↳` summary of what the agent did, and — for the selected one — its sections. The composer for the next input sits at the bottom. | one row per input |
| Right — **Changes** | files the selected input touched, summary first, patch on demand | one row per file, expandable to hunks |

The middle column reads like the conversation it came from, which is the point:
four inputs in Claude Code are four segments here, in the same order, each with
what it did attached. An input answered without touching files is still a
segment — its Changes column says so, and `v` shows the answer.

Selection cascades left to right. The right column is *summary-first*: you see
"switched the retry budget from per-call to per-request, so a 429 storm can no
longer multiply" and press `d` if you actually want the 40 lines.

Behind the three columns is a fourth stop: a **full-screen code pane** showing
the file *as of the selected input*, with that input's lines marked `▌` and
lines written by earlier inputs of the same session marked `N┊` with the input
number. Focus moves across all four stops with one gesture (`Alt+←/→`, `Tab`, or
`Alt+1`–`4`), switching views implicitly at the boundary. See `03-tui.md`
§3-§6. The middle column also holds a composer for typing the next
segment, which is why navigation lives on Alt chords rather than plain keys —
see `03-tui.md` §4.

Underneath both is the **generation itself**, retained verbatim: the prose and
summarized reasoning the model wrote immediately before each edit, linked to the
change it produced, and reachable from any line of code via `W` or `cuz why` —
including lines an earlier input wrote. Summaries are interpreted and can be
wrong; the narration is what was actually said. See `07-provenance.md`.

## 3. Non-goals (v1)

- Not a git client. No staging, committing, or branch management beyond reading.
- Not a code review tool with threads and approvals.
- Not a multi-agent orchestrator.
- No web UI, no server component, no hosted accounts.

## 4. Decisions of record

| # | Decision | Rationale |
|---|---|---|
| D1 | **`because` hosts the agent loop itself.** It owns the conversation and calls the Messages API directly. | First-class access to segments, tool calls and file mutations; no reverse-engineering another tool's on-disk format. Cost: the tool loop, permissions, context management and rate limiting are all ours. See `04-agent-host.md` for how milestone 1 de-risks this. |
| D2 | **SQLite is a local, rebuildable cache. The shared record is append-only JSONL committed to the repo.** | SQLite is a binary blob git cannot merge; two branches summarizing different work would conflict irreconcilably. Content-addressed JSONL lines merge cleanly and replay deterministically. See `01-data-model.md`. |
| D3 | **Plugins are Rust traits now, with ABI-representable signatures so a WASM host can be added later.** | Zero runtime cost and full type safety today; no interface rework when sandboxed plugins arrive. Constraint: owned data across the boundary, no lifetimes or generics in the trait. See `05-plugins.md`. |
| D4 | **Rust + ratatui.** | See `03-tui.md` §1 for the build-on-top-of-something survey. |

## 5. Architecture

```
                        ┌──────────────────────────────────────────┐
                        │  `because` (single process, tokio)  │
                        │                                          │
  keyboard ────────────>│  ui thread ── owns Terminal, renders      │
                        │      │  ▲                                │
                        │   Msg│  │AppEvent                        │
                        │      ▼  │                                │
                        │  ┌─────────────┐                         │
                        │  │  app core   │  Model + update()       │
                        │  └──┬───┬───┬──┘                         │
                        │     │   │   └──────────────┐             │
                        │     │   │                  │             │
                        │  ┌──▼──┐ ┌▼──────────┐ ┌───▼─────────┐   │
                        │  │agent│ │summarizer │ │ store actor │   │
                        │  │ host│ │ job pool  │ │(single wtr) │   │
                        │  └──┬──┘ └────┬──────┘ └───┬─────────┘   │
                        └─────┼─────────┼────────────┼─────────────┘
                              │         │            │
                     Messages API   Messages API   SQLite (WAL)
                     (opus-5)       (haiku-4-5)      +
                              │                    .because/
                         tools ▼                    records/*.jsonl
                     fs, bash, git                  blobs/**
```

Four concerns, deliberately separated:

1. **ui** — owns the `Terminal`, does nothing but render a `Model` and translate
   key events into `Msg`. No I/O, no locks held across a frame.
2. **agent host** — drives one conversation segment at a time; streams deltas out as
   `AppEvent`s so the middle column fills in live.
3. **summarizer job pool** — bounded set of tokio tasks draining a durable job
   queue. Never blocks the UI or the agent; rows appear as `pending` and fill in.
4. **store actor** — the *only* writer to SQLite. All writes are messages to this
   task. Avoids `SQLITE_BUSY` entirely rather than retrying it. Readers use
   separate read-only connections.

## 6. Crate layout

```
because/
├── Cargo.toml                  # workspace
├── crates/
│   ├── because-core/                # domain types, IDs, canonical JSON, digests
│   ├── because-store/               # StorageEngine trait + sqlite impl + migrations
│   ├── ts-record/              # JSONL record format, export/import/replay
│   ├── because-git/                 # diff extraction, patch re-derivation, staleness
│   ├── because-agent/               # AgentBackend trait + api/cli backends + tools
│   ├── ts-summarize/           # Sectioner/Summarizer traits + haiku impls + jobs
│   ├── because-tui/                 # ratatui widgets, layout, event loop
│   └── because-cli/                 # `cuz` binary: tui, index, export, import, doctor
└── docs/design/
```

Built so far: `because-core`, `because-store`, `because-git`, `because-agent`, `because-tui` and
`because-cli`, each covering the part of its role that the walking skeleton and
`cuz live` need. `because-agent` has only the Claude Code CLI backend, plus the
one-sentence summary call (`02-summarization.md` §0) and the allowance query
(`04-agent-host.md`) that will move once `ts-summarize` exists. `ts-record` and
`ts-summarize` do not exist yet.

`because-core` depends on nothing but serde and hashing. Nothing depends on `because-tui`.
`because-store`, `ts-summarize` and `because-agent` are all trait-first so they can be
swapped or faked in tests without a network or a terminal.

## 7. `cuz` CLI surface

| Command | Purpose |
|---|---|
| `cuz` / `cuz live` | (built) the three-column TUI on a live session in the current repository, with the Claude Code CLI as the agent |
| `cuz --check` / `cuz live --check` | (built) every step `cuz live` depends on, run once outside the TUI and reported line by line |
| `cuz import-claude` | (built) read this repository's Claude Code transcripts into the store and report what was found; `cuz live` does the same at start |
| `cuz demo` | (built) the TUI on the sample fixture, with no agent |
| `cuz render <w>x<h> [script]` | (built) replay a key script against the fixture and print the frame, for review with no TTY |
| `cuz index [<rev-range>]` | ingest git history into the local DB (no LLM) |
| `cuz summarize <rev-range> [--batch]` | backfill summaries; `--batch` uses the Batch API at 50% cost |
| `cuz export` | write pending records to `.because/records/` |
| `cuz import` / `cuz replay` | rebuild the local DB from committed records |
| `cuz doctor` | check API key, git repo, schema version, stale summaries, redaction rules |
| `cuz keys` | key probe (built): how each press arrives from this terminal and what it does in each mode |
| `cuz why <path>:<line> [--json]` | rationale for one line outside the TUI — section, summary, narration, prompt |

A working `cuz index` + `cuz tui` with no LLM in the loop is milestone 1. It is
worth being able to run the whole product offline over an existing repo.
