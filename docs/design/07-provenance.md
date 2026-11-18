# Retaining the Generation, and Anchoring It to Code

Two requirements, related but separable:

1. **Keep what the model actually wrote** — its prose and reasoning carry context
   that no generated summary preserves.
2. **Attach that context to the code it produced**, so it is reachable from a
   line of source rather than only from a session list.

The second is the harder one, because code moves.

## 1. What is worth keeping, and the rule that decides

The governing rule:

> **Store bytes for anything that cannot be reconstructed. Store a pointer for
> anything that can.**

Applied:

| Content | Reconstructible? | Stored as |
|---|---|---|
| user prompt | no | bytes |
| assistant prose between tool calls | no | bytes |
| summarized thinking | no | bytes |
| `bash` stdout/stderr | no | bytes |
| tool call metadata (name, path, flags) | no | bytes (small) |
| `edit_file` / `write_file` inputs (the new code) | **yes** — it is the patch | pointer to the patch |
| `read_file` results | **yes** — `git show <sha>:<path>` | pointer |
| `grep`/`glob` results | mostly — re-runnable at that commit | pointer + digest |
| file patches | yes — in git | `git:<sha>:<path>` (see `01-data-model.md` §4) |

That single rule collapses most of the volume problem. The expensive content is
tool results, and tool results are mostly re-derivable; the valuable content is
the model's own words, and those are small.

### Thinking is only ever a summary

Worth stating plainly because it shapes expectations: the API never returns raw
chain of thought. With `thinking.display: "summarized"` you get a readable
summary of the reasoning; with `"omitted"` — **the default on Opus 5** — the
thinking blocks arrive with empty text and there is nothing to save.

So `[agent] thinking_display = "summarized"` is not a UI preference, it is the
difference between capturing reasoning and capturing nothing. Set it explicitly.
(Whether the CLI backend surfaces summarized thinking in its event stream needs
verifying against a real stream before M4 relies on it.)

Storing thinking blocks is also operationally required: they must be echoed back
unchanged when continuing a conversation on the same model, so the agent host
needs them regardless of whether the UI shows them.

## 2. Narration: attaching prose to a change, mechanically

Because `because` hosts the agent, the steps of a segment are an ordered list
with known types. That makes the attachment deterministic — no model call, no
guessing, no hallucination risk:

> For a `tool_use` step **T** that mutated files, the **narration window** is the
> maximal contiguous run of `text` and `thinking` steps immediately preceding
> **T**, stopping at the previous `tool_use` or `tool_result`.

That run is, literally, what the model said it was about to do, immediately
before doing it. It is the highest-value context in the segment and it costs
nothing to capture.

```
step 11  thinking  "the budget is per-call, which multiplies under a 429 storm…"  ┐
step 12  text      "I'll move the budget to the request scope and drop the…"      ├ narration window
step 13  tool_use  edit_file because-agent/src/retry.rs                                ┘ ← the mutation
step 14  tool_result ok
```

Edge cases that need a decision, not a default:

- **Empty window.** Models often batch several edits with no commentary between
  them. Fall back to the nearest preceding non-empty window in the same section
  and mark the link `inherited`, so the UI can say the prose is adjacent rather
  than specific.
- **Parallel tool calls** in one assistant message share one window; mark the
  links `shared`. Do not duplicate the text per file.
- **Retries.** A failed edit followed by a corrected one produces two windows;
  keep both and mark the first `superseded`. The reasoning in a failed attempt is
  often the most informative thing in the segment.

### Three tiers of "why", each cheaper than the last to produce

| Tier | Source | Cost | Trust |
|---|---|---|---|
| summary | Haiku, generated | ~$0.03/segment | interpreted — can be wrong |
| **narration** | the model's own words, mechanically linked | **free** | verbatim — cannot be wrong about itself |
| full transcript | every step of the segment | free, bulky | verbatim |

The narration tier is the one this document adds, and it is the one that most
directly answers "keep the context". It is also the honest fallback when a
summary is stale or failed: the prose still stands.

**The UI must never blur generated and verbatim text.** A Haiku summary and an
Opus explanation look alike on screen and have completely different warranties.
Verbatim spans render with a `❝` marker and the normal foreground; generated
spans render in the accent colour used for summaries elsewhere. If a reader
cannot tell which is which at a glance, the trust model of the whole tool is
broken.

## 3. Anchoring to code that moves

The goal: put the cursor on any line, ask "why is this here", and get the
section, the summary, and the narration that produced it.

### Resolution path

```
(path, line) at HEAD
  → blame                → (commit_sha, orig_path, orig_line)
  → file_changes         → the change for (commit_sha, orig_path)
  → hunks                → the hunk whose [new_start, new_start+new_lines) holds orig_line
  → narration_links      → the steps
  → sections, segments      → title, intent, prompt
```

Every arrow but the first is an index lookup. `git blame --porcelain` emits
`<sha> <orig_line> <final_line> [<count>]` per region, which gives the original
line number directly, and `-C -M` follows copies and renames. Verified against
git locally.

### Blame is too slow for the query path — measured

| history depth | whole-file blame | single line (`-L n,n`) |
|---|---|---|
| 401 commits | 134 ms | 109 ms |
| 1601 commits | 429 ms | 189 ms |

Measured on a synthetic 1500-line file with dense per-commit edits. Two things
fall out, and the first one killed the obvious mitigation:

1. **`-L` scoping barely helps.** Blame's cost is walking history, not counting
   lines — a single line costs nearly as much as the whole file. Scoping the
   viewport is not the answer.
2. **It scales with history depth**, roughly linearly. At 10k commits on a
   frequently-touched file this is seconds, against a 16 ms interactive budget.

Consequences, in order of when to build them:

- **Never blame on the UI thread.** Spawn it, show a throbber, deliver the result
  as an `AppEvent`.
- **Cache by `(commit_sha, path)`.** Blame output for an immutable commit is
  itself immutable, so it can be persisted in SQLite and never invalidated.
  Since `-L` does not help, blame the whole file once and cache the full map.
- **Prefetch on code-pane entry**, so by the time the user presses `W` the answer
  is usually already there.
- **Then remove blame from the query path entirely** with the line-run index
  below. Blame stays as the fallback for files and commits that were never
  indexed.

### The line-run index

`cuz index` already walks commits and already has every hunk. That is enough to
maintain line provenance forward, incrementally, instead of reconstructing it
backwards on demand:

```sql
CREATE TABLE line_runs (
    commit_sha      TEXT    NOT NULL,   -- the file's state AT this commit
    path            TEXT    NOT NULL,
    start_line      INTEGER NOT NULL,
    end_line        INTEGER NOT NULL,
    file_change_id  TEXT    NOT NULL REFERENCES file_changes(id) ON DELETE CASCADE,
    PRIMARY KEY (commit_sha, path, start_line)
);
```

Maintenance per commit: take the parent's runs, drop the ranges its hunks
deleted, insert a run for the added lines attributed to this commit's
`file_change`, shift the runs after each hunk by the hunk's line delta, then
coalesce adjacent runs with the same `file_change_id`. O(changed lines), once,
at index time.

Runs coalesce well — a 1500-line file touched by 50 changes holds on the order of
50–200 rows, not 1500. Query becomes a single indexed range lookup.

Only HEAD's runs are maintained by default; keeping them for every commit is
possible and not worth the space. On a branch switch, replay from the merge-base
(cheap — O(commits since merge-base)) while serving the previous HEAD's runs, so
the switch does not stall.

### What anchoring cannot do

Worth writing down so the UI does not overpromise:

- A line that was **moved** without being modified blames to its move commit, so
  the narration returned is about the move, not the original intent. `-C -M`
  mitigates this and does not eliminate it.
- **Reformatting** (a formatter run, a mass re-indent) reattributes whole files.
  Support `blame.ignoreRevsFile` / `--ignore-rev` and read the repo's existing
  `.git-blame-ignore-revs` if present — teams that already maintain one get this
  for free.
- A **deleted** line has no anchor in the current file at all; it is reachable
  only through the segment or the diff.

The panel should say which commit it resolved through, so a reader can see when
the answer is about a move rather than the logic.

## 4. Schema additions

```sql
-- what the model wrote, linked to what it changed
CREATE TABLE narration_links (
    file_change_id  TEXT NOT NULL REFERENCES file_changes(id) ON DELETE CASCADE,
    step_id         TEXT NOT NULL REFERENCES steps(id)        ON DELETE CASCADE,
    relation        TEXT NOT NULL,   -- narration|tool_call|tool_result
    qualifier       TEXT,            -- inherited|shared|superseded|NULL
    seq             INTEGER NOT NULL,
    PRIMARY KEY (file_change_id, step_id, relation)
);
CREATE INDEX narration_by_step ON narration_links(step_id);
```

`steps` gains three columns:

```sql
ALTER TABLE steps ADD COLUMN retention   TEXT NOT NULL DEFAULT 'narration'; -- narration|local|dropped
ALTER TABLE steps ADD COLUMN redacted    INTEGER NOT NULL DEFAULT 0;
ALTER TABLE steps ADD COLUMN body_origin TEXT;   -- 'bytes' | 'git:<sha>:<path>' | 'patch:<file_change_id>'
```

`body_origin` is where the re-derivability rule lives: a step whose body is
reconstructible stores the pointer and no bytes.

The FTS table gains narration, so search covers what the model said and not only
what Haiku wrote about it. `/` searches summaries by default; `/!` widens to
narration, because searching prose the model wrote is a different intent from
searching summaries and mixing them by default makes both noisier.

## 5. Retention tiers

Local storage always keeps full fidelity. The tier controls only what is
**committed** to `.because/records/`.

| Tier | Contents | Est. per segment, zstd |
|---|---|---|
| `summaries` | generated summaries only | ~1 KB |
| `narration` *(default)* | + prompt, assistant prose, summarized thinking, tool call metadata | ~2–3 KB |
| `full` | + tool result bodies | ~30–150 KB |

For a five-person team at 50 segments/day each (250 segments/day):

- `narration` → ~600 KB/day, ~13 MB/month
- `full` → ~15 MB/day, ~350 MB/month

Hence the default. `full` exists for teams who want it and should carry a warning
in `cuz doctor` once the record store passes a size threshold.

Because records shard per session, a team that wants to drop old narration can
`git rm` old shards without breaking replay — the local DB keeps them, and the
summaries in other shards still resolve. Pruning is a repo-hygiene decision, not
a schema one.

## 6. Redaction runs in both directions

`02-summarization.md` §8 gates content **going out** to the API. Narration
creates a second, more dangerous flow: model prose gets **committed to the
repository**, where it is pushed, mirrored and cloned.

The model quotes what it read. If it read a config file with a live credential,
its explanation may contain that credential verbatim — and unlike a diff, prose
has no structure to exclude by path.

So the same redaction rules run over every narration body before it is written to
a record:

- path-based exclusion cannot help here (prose has no path), so the **content
  scan is the only gate** and must run on every narration step;
- a hit marks `steps.redacted = 1`, the body stays local-only (`retention =
  'local'`), and the record carries the link with a placeholder rather than the
  text;
- the UI shows "narration withheld: matched a credential pattern" with the local
  body still readable by the person on that machine.

This is a case where the committed artifact is more exposed than the API call,
so if anything the inbound gate deserves the stricter setting.

## 7. Surfacing it

### Code pane

`W` on any line opens the **rationale panel** for that line. Rendered by the
skeleton, viewing input 3 of a four-input session with the cursor on a line that
input 2 wrote:

```
┌ why is line 85 here? ────────────────────────────────────────────────────────────────────────────────┐
│input      › now add a variant that takes a plain Path. incremental checking won't have a DirEntry to…│
│summary      New is_hidden_path for callers without a DirEntry — on Windows it costs a                │
│             metadata() call per check. The leading-dot test is extracted into                        │
│             is_hidden_path_only and shared with is_hidden_entry. [generated]                         │
│section      share the leading-dot check                                                              │
│  ❝ said     Both functions need the same leading-dot test. Pulling it out as is_hidden_path_only and │
│             having is_hidden_entry call it, so there is one definition.                              │
│    ⟩ ran    edit_file crates/ignore/src/pathutil.rs                                                  │
└ via 6e527e9 · input 2 of 4 (an earlier input) · its line 85 ─────────────────────────────────────────┘
```

The line is resolved through blame, not through the selected input: it may have
been written by an earlier one, and here it was. `via 6e527e9 · input 2 of 4
(an earlier input) · its line 85` names the commit, the input, and the line's
number in *that* input's version of the file. `[generated]`, `›` and `❝` are the
three provenance markers from §2.

### Segments column

The middle column lists inputs. `v` expands the selected input's sections to
show the **verbatim** narration under each — the model's own words for that part
of the response — in place of the generated section titles alone.

### Transcript overlay

`t` opens the whole session's transcript as a modal — every input, and under
each its sections and narration, in order — scrolled to the selected input. It
is the conversation as it happened, with the generated section labels marked as
such. Deliberately not a fifth stop in the focus ring: the ring stays at four
(`03-tui.md` §3), and the transcript is a drill-down, not a peer.

### From outside the TUI

```
cuz why <path>:<line>        # rationale for one line, plain text
cuz why <path>:<line> --json # same, structured, for editor integration
```

Non-interactive access matters more than it looks: it makes the context reachable
from vim, emacs or a review script without opening the TUI, and it is the
smallest possible integration surface for anyone who wants to build on this.

## 8. Where this lands in the roadmap

- **M1** — `line_runs` maintenance during `cuz index`, blame fallback with caching
  and prefetch, `cuz why`. All git-only; no agent and no LLM needed. Anchoring is
  testable from day one against this repo's own history.
- **M2** — search over narration; trust markers in the UI.
- **M3** — retention tiers in the record format; the inbound redaction gate.
- **M4** — narration capture proper, once the agent host produces real steps.
  Until then `narration_links` is populated for imported history as empty, and
  the rationale panel shows summary and prompt only.

The ordering matters: the anchoring machinery is independent of the agent, so it
can be built and proven before there is any narration to attach to it.
