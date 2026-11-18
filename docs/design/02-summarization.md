# Summarization Pipeline

Summarization is the product, so this is the part to get right. Everything else
is plumbing around it.

## 0. What runs today: one sentence per input *(built, interim)*

The three passes below are not built. Until they are, `cuz live` writes one
sentence per input, which is what the Segments column shows in place of the
reply (`03-tui.md` §5). It exists because the column cannot hold a long reply:
a ~1,000-word answer was cut to six lines there, with nowhere to read the rest.

**How.** When an input finishes, one call through the Claude Code CLI the agent
already runs on, so it needs no API key (`crates/because-agent/src/claude/summary.rs`):

```
claude -p --model haiku --system-prompt <one-sentence instruction> --tools "" \
       --no-session-persistence --output-format json --max-budget-usd 0.05
```

with `MAX_THINKING_TOKENS=0`, the prompt on stdin, and the system temp
directory as the working directory so the repository's CLAUDE.md is not
loaded. The prompt is the input, the tool calls (up to 30), and everything the
agent said; past 15,000 characters the middle is cut and the first 12,000 and
last 3,000 kept. The instruction asks for at most 25 words and the limit is
restated at the end of the prompt; only the first line of the answer is kept.

**When.** For an input that finished without error and either said more than
30 words or used a tool. Shorter tool-free answers fit in the column and are
shown verbatim instead. `cuz live --no-summary` turns it off;
`--summary-model` changes the model.

**Measured** (claude-haiku-4-5-20251001, the same ~1,000-word reply, a scratch
repository with no CLAUDE.md):

| Setup | Cost | Time | Length |
|---|---|---|---|
| thinking on (the CLI default) | $0.005–0.006 | 7–9 s | — |
| `MAX_THINKING_TOKENS=0` | $0.0022 | 2.2 s | ~60 words; ignored the limit |
| same, limit restated at the end of the prompt | $0.0020 | 1.3–1.4 s | 17 words |

In `cuz live` against a real 588-word reply: $0.0013, 1.3 s. A user-level
`~/.claude/CLAUDE.md` and user hooks still apply to these calls; `--bare` would
skip them but only authenticates with an API key, which defeats the point.

**What it is not.** One sentence about the whole input — no sections, no
per-file summaries, no risk. Those are passes A–C. The summary is kept in the
store with the input (`segments.summary`, with the model that wrote it in
`summary_by` and its cost in `summary_cost_usd`; `01-data-model.md` §0), so it
is still there when the session is read as history. It is marked generated (`↳`, generated colour), and the reader (`03-tui.md` §5)
shows the model, cost and `[generated]` tag next to it.

## 1. Trigger

Because `because` hosts the agent (D1), the trigger is in-process and
exact: when the segment's final message arrives with a terminal `stop_reason`, the
agent host writes the segment's `steps` and `file_changes` rows and enqueues jobs.
No file watching, no hook scripts, no polling.

```
segment ends
   └─> store: segments, steps, file_changes (summary_state='pending')
   └─> enqueue section_segment(segment_id)
          └─> on success: enqueue summarize_file(fc_id) × N   (parallel)
                 └─> when all N settle: enqueue rollup_segment(segment_id)
```

The UI subscribes to job completions and repaints affected rows. A segment is
browsable the instant it ends; summaries land underneath it over the next few
seconds.

## 2. Hierarchical, grounded, cheap

Three passes, each small. The thing that makes this work is that **no pass is
asked to read the whole segment**.

### Pass A — sectioning (1 call per segment)

Do *not* hand a model a wall of text and ask it to find sections. Instead build
the candidate structure mechanically first — the segment is already a sequence of
typed steps (text, thinking, tool_use, tool_result) — then ask Haiku only to
group and label them.

Input: the user prompt, plus a compact rendering of the step list where each
step is one line (`#12 tool_use Edit crates/because-store/src/sqlite.rs`), with text
steps truncated to their first ~200 characters. A 60-step segment is ~1.5K tokens
this way instead of ~30K.

Output (structured, `output_config.format`):

```json
{
  "sections": [
    {
      "title": "move retry budget to per-request",
      "intent": "Stop a 429 storm from multiplying retries across calls.",
      "step_start": 4,
      "step_end": 17,
      "confidence": 0.9
    }
  ]
}
```

Validated on receipt: ranges must be in order, non-overlapping, and cover every
step. If validation fails twice, fall back to a mechanical sectioning (split
on runs of consecutive tool calls to the same file) so the middle column is
never empty.

### Pass B — file change summaries (1 call per file, parallel)

One call per changed file, each returning both the per-hunk labels and the file
rollup — one round trip, two levels of detail.

Input, in this order:
1. the user prompt (verbatim — this is what makes the "why" possible);
2. the title and intent of the section this change belongs to;
3. the file path and language;
4. the unified diff for that file, hunk by hunk, with `@@` headers kept.

Output:

```json
{
  "file_summary": "Retry budget is now computed once per request instead of per call, and the ceiling moved from 5 to 3.",
  "risk": "behavioral",
  "hunks": [
    {"seq": 0, "summary": "replaced per-call counter with a request-scoped budget"},
    {"seq": 1, "summary": "lowered the ceiling and added a debug log on exhaustion"}
  ],
  "confidence": 0.86
}
```

`risk` is one of `none|behavioral|api|security|perf|data` and drives colour in
the right-hand column. It is the single highest-value field for a reviewer
scanning fast, so it is worth a line in the prompt defining each value rather
than letting the model guess.

### Pass C — segment rollup (1 call per segment)

Input is the file summaries and section titles from A and B — **not** any diff
text. A map-reduce, so cost is bounded by the number of files, not their size.

## 3. Model configuration

Summarizer: `claude-haiku-4-5` (200K context, $1/MTok in, $5/MTok out).

Haiku 4.5 specifics that differ from the current Opus/Sonnet generation and will
bite if assumed away:

- **Thinking takes `budget_tokens`**, not `{type:"adaptive"}`. For these three
  passes, run with no `thinking` block at all — they are extraction and
  compression tasks, and latency matters more than depth.
- **`output_config.effort` errors on Haiku 4.5.** Do not send it.
- **Structured outputs are supported** on Haiku 4.5 (`output_config.format`, and
  `strict: true` on tools with `additionalProperties: false` + `required`). Use
  them; a schema-invalid summary is a parse branch you do not want.
- **Minimum cacheable prefix is 4096 tokens** on Haiku 4.5 — far higher than the
  512 on Opus 5. A 600-token system prompt **will not cache**, silently. See §5.

Escalation knob, off by default: if a file's diff exceeds `escalate_lines` (say
400) or Haiku reports `confidence < 0.5`, re-run that one file on
`claude-sonnet-5`. Config, not automatic, and never a silent downgrade in the
other direction.

`prompt_ver` is written on every generated row. Bump it whenever a prompt
template changes so `cuz summarize --regenerate --prompt-ver=old` can find and
redo the affected rows.

## 4. Cost and latency envelope

Worked example — one segment touching 8 files, ~120 changed lines each (~38 KiB of
patch text, ~11K tokens):

| Pass | Calls | Input tokens | Output tokens |
|---|---|---|---|
| A sectioning | 1 | ~2,000 | ~600 |
| B file summaries | 8 | ~13,000 total | ~1,600 total |
| C segment rollup | 1 | ~1,500 | ~400 |
| **Total** | **10** | **~16,500** | **~2,600** |

At Haiku 4.5 rates: 16,500 × $1/1M = **$0.017** in, 2,600 × $5/1M = **$0.013**
out → **≈ $0.03 per segment**. Fifty segments a day is ~$1.50/day/developer, roughly
1–2% of what the Opus segments themselves cost. Summarization is not the expensive
part of this system, which is the argument for doing it on every segment rather
than on request.

Latency: pass A ~1–2s, pass B 8 calls in parallel ~2–4s, pass C ~1s. First
summaries visible ~2s after the segment ends, all of them by ~6s. Cap concurrency
at 8 in-flight summarizer calls to stay clear of rate limits, with a token
bucket and exponential backoff on 429.

Show the running cost in the UI (segment-level and session-level, from
`usage_json`). A tool that spends the user's money should say how much.

## 5. Prompt caching: measure before believing

The 4096-token minimum on Haiku 4.5 means the obvious move — `cache_control` on
a short system prompt — does nothing and reports nothing. Options:

- **Don't bother for passes A and C.** Their prefixes are small and volatile.
- **For pass B, cache only if the prefix genuinely clears 4096 tokens** — e.g.
  system prompt + repo conventions (`CLAUDE.md`, style notes) + the file's prior
  summary history as a stable prefix, with the diff after the last breakpoint.
- **Verify with `usage.cache_read_input_tokens`.** If it is zero across repeated
  calls, caching is not happening; the API does not warn. Wire this into
  `cuz doctor` so a silent regression is visible.

Render order is `tools` → `system` → `messages`, and any byte change in the
prefix invalidates everything after it. Keep timestamps, per-request ids and the
diff itself *after* the last breakpoint.

## 6. Backfill over history

`cuz summarize <rev-range> --batch` uses the Message Batches API: submit one
request per file change, poll `processing_status` until `ended`, stream results
and key them by `custom_id` (**results come back in arbitrary order** — never by
position). 50% cheaper and no rate-limit management, at the cost of latency.
This is the right tool for "summarize the last 2,000 commits on import" and the
wrong tool for a live segment.

Backfilled commits have no prompt and no sections, so pass B runs without the
prompt/section grounding and the summary is necessarily weaker — a "what"
without a "why". Mark those rows so the UI can be honest about it
(`prompt_ver` suffixed `-nogrounding`, or an explicit `grounded` flag).

## 7. Failure handling

Every job: 3 attempts, exponential backoff with jitter, then `state='dead'` with
`last_error` preserved. A dead job renders as `summary_state='failed'` with the
error reachable in the UI. **A failed summary never blocks browsing** — the
patch is always there, which is the whole point of not making the summary the
only record.

Typed error handling, most-specific first, rather than one broad catch:
`NotFound` → `RateLimit` (backoff, respect `retry-after`) → other 4xx (do not
retry, mark dead) → 5xx / connection (retry).

Check `stop_reason` before reading `content` on every response.

## 8. Redaction before anything leaves the machine

Diffs are sent to an API. Some diffs contain secrets. This is a hard gate in
front of every request, not a setting:

1. **Path exclusion** — `.becauseignore` (gitignore syntax, via the
   `ignore` crate) plus built-in defaults: `.env*`, `*.pem`, `*.key`, `*.p12`,
   `id_rsa*`, `*.keystore`, `secrets/**`, `**/credentials*`.
2. **Content scan** — regex set over the patch text for high-signal shapes: AWS
   keys (`AKIA[0-9A-Z]{16}`), private-key PEM headers, bearer/authorization
   headers, `sk-ant-…` and similar API-key prefixes, high-entropy base64 runs
   over ~40 chars in an assignment position.
3. **On match** — the file change is stored locally with `summary_state =
   'skipped'` and reason `redacted`. **No request is made.** The UI says the file
   was skipped and why. Do not "mask and send" by default: a masking bug sends
   the secret, and the failure mode is unrecoverable.
4. **Opt-in override** per path, recorded in `config.toml` so it is reviewable in
   a diff like any other config.

**This gate is only half of it.** The rules above guard content going *out* to
the API. Narration (`07-provenance.md` §6) creates an inbound flow — model prose
committed into the repository — where path-based exclusion cannot help, because
prose has no path. The content scan has to run there too, and arguably stricter:
a committed artifact is more exposed than an API call.

Secrets that reach the record store are worse than ones that don't, because
records get committed and pushed. If a blob with a secret is already committed:
`retraction` record, purge the blob file, rotate the credential. Say that in the
docs — there is no version of this where rotation is optional.

## 9. Evaluating summary quality

Without a measurement you cannot tell a prompt change from a prompt regression.
Minimum viable eval, worth building at milestone 2:

- 30–50 hand-picked file changes from this repo's own history, each with a
  human-written reference summary and the correct `risk` value;
- score `risk` as exact-match accuracy (objective, cheap to check);
- score summary text with an LLM judge on `claude-opus-5` against a rubric:
  names the actual behavioural change, does not restate the diff line by line,
  does not invent a rationale absent from the prompt;
- run it on every prompt-template change and record the score next to
  `prompt_ver`.

The "does not invent a rationale" criterion matters most. A confidently wrong
"why" is worse than no "why", because the entire premise of the tool is that the
reader can trust the middle and right columns enough to skip the code.
