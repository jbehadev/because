# Agent Host

Implements decision **D1**: `because` owns the conversation.

## 1. The scope problem, and how to not eat it up front

Hosting the agent means owning the tool loop, tool implementations, permission
gating, context management, streaming, cancellation, retries and rate limits.
That is most of a coding-agent harness, and none of it is the part of
`because` that is actually novel. Building it first means months before
anyone can judge whether summary-first review is any good.

So: put the agent behind a trait with **two implementations**, ship the cheap one
first, and keep the native one as the end state.

```rust
// because-agent/src/lib.rs
#[async_trait]
pub trait AgentBackend: Send + Sync {
    fn id(&self) -> &str;

    /// Start or resume a conversation rooted at `repo_root`.
    async fn open(&self, cfg: SessionConfig) -> Result<Box<dyn AgentSession>>;
}

#[async_trait]
pub trait AgentSession: Send {
    /// Submit a prompt. Events stream until the segment reaches a terminal state.
    async fn send(&mut self, prompt: String) -> Result<()>;
    /// Ordered event stream for the session.
    fn events(&mut self) -> BoxStream<'_, AgentEvent>;
    /// Abort the in-flight segment. Must actually cancel the request.
    async fn cancel(&mut self) -> Result<()>;
}

pub enum AgentEvent {
    SegmentStarted  { segment_id: SegmentId, ts: Timestamp },
    TextDelta    { text: String },
    ThinkingDelta{ text: String },
    ToolStarted  { step_seq: u32, name: String, input: serde_json::Value },
    ToolFinished { step_seq: u32, is_error: bool, result_ref: BlobDigest },
    FileMutated  { path: PathBuf, change: ChangeKind },
    SegmentEnded    { stop_reason: StopReason, usage: Usage },
    Error        { message: String, retryable: bool },
}
```

`because-tui`, `because-store` and `ts-summarize` only ever see `AgentEvent`. Which backend
produced it is invisible to them, and swapping backends is a config line.

### The interface as built *(built)*

The built interface is smaller than the sketch above, and it is synchronous.
It uses threads and a callback, the same as the rest of `cuz`. These items are
in `crates/because-agent/src/lib.rs`:

| Item | Purpose |
|---|---|
| `SessionConfig` | What a session needs from `cuz`: the directory, the session id, the permission mode, the model, resume, the log, and ephemeral. |
| `Agent` | A backend. `start` starts a session or resumes it, and gives each event to a callback. `check_resume` finds a problem before a resume starts. `allowance` gives the account's allowance, or `None` when the agent has none. |
| `AgentSession` | One running session: `send`, `answer` and `interrupt`. Drop it to stop the agent. |
| `summary::Summarizer` | The small model that writes the summaries. The prompt in `summary` is the same for each model. |

`because-cli` uses only these items. It does not use the Claude Code backend
directly, except to make it. All Claude Code code is in
`crates/because-agent/src/claude/`: the stream parser, the process, the summarizer,
the allowance and the transcripts. Thus a new backend is a new module that
implements `Agent`, and one more choice where `cuz live` makes the agent.

Transcript import is not part of the interface yet. It reads only Claude Code
transcripts (`because-cli`'s `import`).

### Backend 1 — `ClaudeCliBackend` (milestone 4, ships first)

Spawn the Claude Code CLI in streaming JSON mode and translate its event stream
into `AgentEvent`. Flags verified against `claude --help` on 2026-09-22:

```
claude -p \
  --output-format stream-json \
  --input-format stream-json \
  --include-partial-messages \
  --replay-user-messages \
  --session-id <uuid> \
  --permission-mode manual
```

`--input-format stream-json` gives a persistent stdin channel, so each input is a new segment in the same session,
`--include-partial-messages` gives token-level deltas for the live view,
`--session-id` lets `because` choose the id so its `sessions.id` matches,
and `--resume <session-id>` reattaches after a restart. `--permission-mode` has
`manual`, `acceptEdits`, `auto`, `dontAsk`, `bypassPermissions`, `plan`;
`--restricted` drops the command-running tools entirely.

What this buys on day one: a working agent with tools, permissions, context
compaction and MCP already solved, so the summarization UX can be validated for
real. What it costs: a subprocess dependency, a JSON event schema that is not
contractually stable (pin a CLI version, and treat unknown event types as
ignorable), and less precise attribution of file mutations than the native loop.

Write the translation layer against recorded fixtures — capture real CLI output
once, commit it as test data, and parse it in unit tests. That keeps the schema
risk visible: when a CLI upgrade changes the stream, a test fails instead of a
user's session breaking.

### What the CLI stream actually contains *(built; verified against a recording)*

`crates/because-agent` implements this backend; `cuz live` runs it. The protocol
facts below come from a recorded real session, committed as
`fixtures/claude-stream/two-inputs.jsonl` and replayed by the tests — not from
documentation, since the stream is not a published contract:

- **One process serves the session.** Each input is one stdin line,
  `{"type":"user","message":{"role":"user","content":"…"}}`; the response ends
  with a `result` event. `--verbose` is required for stream-json output in
  print mode.
- **Every input begins with its own `system/init`**, carrying the model.
- **Live text** arrives as `stream_event` / `content_block_delta` /
  `text_delta`; complete blocks then arrive as `assistant` messages. The tests
  check the deltas concatenate to exactly the complete text.
- **`total_cost_usd` in `result` is cumulative for the process**, not per input:
  in the recording, input 2's usage was smaller than input 1's but its total was
  higher ($0.0386 vs $0.0297). Per-input cost is the difference between
  consecutive totals; showing the raw value would overstate every input after
  the first.
- **Tool results for Claude Code's own edit tools carry `structuredPatch`**, the
  exact hunks of that edit. Shell edits do not. `cuz live` currently ignores it
  and takes changes from git snapshots taken before and after each input
  (`crates/because-git`), which covers both; `structuredPatch` is the route to
  per-edit attribution later.
- **Thinking blocks are often empty** (signature only), as in imported
  transcripts; the parser keeps only non-empty ones.
- `result.permission_denials` lists tools the permission mode refused.

Permission prompts *(built; not yet checked against a recording)*: the CLI
runs with `--permission-prompt-tool stdio`, so a tool call its mode does not
allow arrives on stdout as `{"type":"control_request","request_id",
"request":{"subtype":"can_use_tool","tool_name","input",
"permission_suggestions"}}` and waits. The answer goes to stdin as
`{"type":"control_response","response":{"subtype":"success","request_id",
"response":{…}}}` with `behavior` `allow` (and `updatedInput`, the input echoed
back; for allow-for-session, `updatedPermissions`: the suggestions with
`destination` rewritten to `session`; for allow-and-switch-to-auto,
`updatedPermissions: [{"type":"setMode","mode":"auto","destination":"session"}]`)
or `deny` (and `message`). A
`control_cancel_request` withdraws one; any other control request is answered
with an error so the CLI is not left waiting. The shapes are the Agent SDK's;
`scripts/record_permission.py` records a real exchange to
`fixtures/claude-stream/permission.jsonl` to test against.

Questions from the agent use the same request. `AskUserQuestion` arrives as a
`can_use_tool` request with `requires_user_interaction` set, and its `input`
holds `questions`. Each question has a `question`, a `header`, `options` with a
`label` and a `description`, and `multiSelect`. To answer, send `allow` with
`updatedInput`: the input, plus `answers`, an object from question text to
answer. An answer to a multi-select question joins its labels with ", ". If
`answers` is empty, the agent gets "The user did not answer the questions."
`scripts/record_question.py` records a real exchange to
`fixtures/claude-stream/question.jsonl`.

How the TUI shows these prompts and takes the answer: `03-tui.md` §5c.

**Resuming.** A session's `claude` is started with `--session-id` on its first
input and with `--resume <id>` whenever it has to be started again — after it
exits, or to revive an earlier session (`03-tui.md` §5b). `cuz --check` adds
`--no-session-persistence`, so its throwaway conversation leaves no transcript.

**Input during a response** *(built; checked against the CLI)*. A user
message written to stdin while the agent works does not wait for the end. The
CLI adds it to the running turn at the next tool boundary, and the turn ends
with one `result`. If no tool call remains, the message starts a new turn after
the `result`, with a second `init`. No event marks the difference. Thus `cuz`
runs the CLI with `--replay-user-messages`: the CLI echoes each input
(`"isReplay": true`) when it reads it, and the position of the echo shows the
turn (`03-tui.md` §5d).

**Interrupting** *(built; checked against a recording)*. To stop the input
that runs, `cuz` writes `{"type":"control_request","request_id",
"request":{"subtype":"interrupt"}}` to stdin. The CLI withdraws a permission
request that waits (`control_cancel_request`). Then it ends the input with a
`result` that has `subtype` `error_during_execution`, `is_error` set, and
`total_cost_usd` 0. The process stays, and it accepts the next input. Thus a
stopped input keeps the last total, so the next input is not charged for the
earlier ones. `scripts/record_interrupt.py` records
`fixtures/claude-stream/interrupt.jsonl`.

**Background tasks** *(built; checked against a `cuz live` log)*. The agent
can run a command or a monitor in the background, and end its response before
the task ends. When the task ends, the CLI writes `{"type":"system",
"subtype":"task_notification","task_id","status","summary"}`. If an input
runs, the agent reads the notice at its next step. If no input runs, the CLI
starts a response of its own: a `system/init`, the steps, and a `result` with
`origin.kind` `"task-notification"`. No user message comes before this
response. Some of these responses have no steps (`num_turns` 0).

The parser gives `TaskEnded` with the `summary`. When no input runs, `cuz`
keeps the summaries. The first step of the next response makes a segment, and
its input tells which tasks ended. A response with no steps makes no segment.
`Cmd::Begin` takes the "before" snapshot of the segment, as `Cmd::Send` does
for an input. `fixtures/claude-stream/task-notification.jsonl` has this
sequence. It is written by hand in the shape of a real log.

Not built yet: per-tool-call attribution.

### The subscription allowance *(built)*

The Sessions column shows how much of the subscription's allowance is left
(`03-tui.md` §5b, the allowance). The CLI has no flag for it — `/usage` is an
interactive command — so `crates/because-agent/src/claude/usage.rs` asks what `/usage`
asks: `GET https://api.anthropic.com/api/oauth/usage` with
`anthropic-beta: oauth-2025-04-20` and the OAuth access token Claude Code
already stored (the macOS Keychain, or `~/.claude/.credentials.json`), through
`curl`, so the crate gains no HTTP dependency. Every top-level object in the
reply that carries a `utilization` becomes a window, so a window added later
appears without a change here and unknown fields are ignored. It is not a
published contract: each step fails to an error the UI shows — an API-key
login has no OAuth token and no allowance — rather than to a wrong number. The
raw reply goes to the log on every poll, so when the shape changes the log
says how. `cuz --check` makes the same request and reports it.

### Claude Code transcripts *(built: import as history)*

Sessions run in Claude Code itself are read from its transcripts,
`~/.claude/projects/<dir>/<session-id>.jsonl` (or under `$CLAUDE_CONFIG_DIR`),
where `<dir>` is the path Claude Code was started in with every character that
is not a letter or digit replaced by `-`. `crates/because-agent/src/claude/transcript.rs`
parses one; `because-cli`'s import ties inputs to commits and writes the store. It
runs at the start of `cuz live` (`--no-import` skips it) and as `cuz import-claude`,
which prints what it did.

Checked against Claude Code 2.1 transcripts — an interactive session of 2,935
records and `claude -p` sessions — and tested against a recorded, scrubbed one
(`fixtures/claude-transcript/two-inputs.jsonl`):

| Record | Read as |
|---|---|
| `user`, typed by a person | an input. Interactive sessions mark these `origin.kind: "human"` and that is believed when present; `claude -p` writes no `origin`, so text Claude Code adds itself is recognised instead: `isMeta`, compaction summaries, slash-command and shell-escape markup, interruption notices, sidechain (subagent) records |
| `user` with `tool_result` blocks | not an input; an `is_error` result marks its tool call failed, matched by `tool_use_id` |
| `assistant` | text, non-empty thinking and tool calls of the latest input, in order; the model from the first message |
| `cost-state` | the session's running cost, written at some turn ends. An input gets its own cost only when one total falls after it and before the next input; otherwise its cost is unknown, never guessed. 5 of 17 inputs in the interactive session had their own |
| `ai-title` | the session's title (the last one) |
| anything else | skipped; so are lines that are not JSON, as the last line of a transcript being written can be |

Assistant records carry no input id, so they are grouped by order: everything
after an input and before the next answers it. Checked against Claude Code's
own `parentUuid` chains in the interactive session: all 935 assistant records
grouped the same way.

What a transcript cannot give is which files an input changed — a shell
command's edits leave no trace in it, and in this repository's own history none
of the agent's file writes went through the edit tools. So an input is tied to
the commits made, on any branch, between when it was sent and two seconds after
the last record answering it, and the UI says that is how it knows (`03-tui.md`
§5b). A transcript is read again only when its size or modification time has
changed, and a session `cuz live` hosted is skipped: the store has it from the
inside. `cuz live --check` runs with `--no-session-persistence`, so its
throwaway conversation does not come back as history.

Claude Code deletes transcripts after 30 days by default (`cleanupPeriodDays`);
what has been imported stays in the store.

### Backend 2 — `MessagesApiBackend` (milestone 5, the end state)

Rust has no official Anthropic SDK, so this is `reqwest` 0.13 + `serde_json`
against `POST https://api.anthropic.com/v1/messages`.

Headers: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type:
application/json`.

Request shape for the main model:

```json
{
  "model": "claude-opus-5",
  "max_tokens": 64000,
  "stream": true,
  "thinking": {"type": "adaptive", "display": "summarized"},
  "output_config": {"effort": "xhigh"},
  "system": [{"type": "text", "text": "…", "cache_control": {"type": "ephemeral"}}],
  "tools": [ … ],
  "messages": [ … ]
}
```

API details that are easy to get wrong and expensive to debug:

- **Thinking is adaptive, not budgeted.** `budget_tokens` is rejected with a 400
  on Opus 5. Thinking is *on by default* — omitting the block runs adaptive.
  `display: "summarized"` is what makes reasoning visible in the middle column;
  the default is `"omitted"`, which streams empty thinking blocks and looks like
  a long hang. It is also the difference between capturing the model's reasoning
  and capturing nothing — the raw chain of thought is never returned on any
  model, so the summary is the only reasoning there is to keep
  (`07-provenance.md` §1).
- **`effort` lives inside `output_config`**, not at the top level. `xhigh` is the
  right default for coding work.
- **Stream anything with a large `max_tokens`.** SSE via `eventsource-stream`
  0.2.3 or `reqwest-eventsource` 0.6.0 over `bytes_stream()`.
- **Return *all* `tool_result` blocks in one user message.** Splitting parallel
  tool results across multiple messages teaches the model to stop making
  parallel calls — a silent, gradual quality loss with no error.
- **Echo `response.content` back verbatim**, including thinking blocks, when
  continuing on the same model. Extracting just the text loses state.
- **Handle `stop_reason` before reading `content`**: `tool_use` → run tools and
  loop; `pause_turn` → resend as-is to continue; `refusal` → surface it, do not
  parse content; `max_tokens` → the response is truncated.
- **No assistant prefill.** It is a 400 on Opus 5.
- **Parse tool inputs with `serde_json`, never string matching.** JSON escaping
  of tool inputs varies by model.
- **Caching:** Opus 5's minimum cacheable prefix is 512 tokens. Order is `tools`
  → `system` → `messages`; put the stable system prompt and tool definitions
  first with a breakpoint after them, and keep anything volatile (timestamps,
  ids) after it. Verify with `usage.cache_read_input_tokens` — zero across
  repeated segments means a silent invalidator.
- **Context management** for long sessions: server-side compaction (beta
  `compact-2026-01-12`, via `client.beta`-equivalent endpoint) or context editing
  (`clear_tool_uses_20250919`) to drop old tool results. Compaction requires
  appending `response.content` unmodified, since the compaction blocks in it are
  what the API uses to replace history on the next request. Drop either of these
  and a long session dies at the context limit.

## 2. Tools

Minimum viable set, each with `strict: true`, `additionalProperties: false` and
an explicit `required` list:

| Tool | Notes |
|---|---|
| `read_file` | byte-range/line-range args; refuse paths outside `repo_root` |
| `write_file` | full content replace; records the pre-image for the diff |
| `edit_file` | exact-string replace with a uniqueness check, like `str_replace` |
| `glob` | via the `ignore` crate, honours `.gitignore` |
| `grep` | ripgrep semantics; shell out to `rg` if present, else `grep` crate |
| `bash` | the one that needs real gating — see §3 |

Every mutation goes through one funnel that (a) captures the pre-image, (b)
applies the change, (c) computes the patch, (d) emits `FileMutated`, and (e)
records the narration window — the run of text and thinking steps immediately
preceding this tool call (`07-provenance.md` §2). That is what makes both file
attribution and rationale attribution exact rather than inferred.

Path containment is enforced on the canonicalized path *after* symlink
resolution, not on the string. `../` checks on raw strings are not a security
control.

Later, MCP servers can be exposed as tools via `rmcp` 3.4.0 (the Rust MCP SDK),
which also gives the plugin system a ready transport for tool plugins — see
`05-plugins.md` §5.

## 3. Permissions

Per-tool policy resolved in order: session override → `config.toml` → default.

| Tool class | Default |
|---|---|
| read-only (`read_file`, `glob`, `grep`) | allow |
| in-repo mutation (`write_file`, `edit_file`) | allow, recorded |
| `bash` | ask, with the exact command shown |
| anything touching a path outside `repo_root` | deny |

The permission prompt is a modal that blocks the segment, with `a` (allow once),
`A` (allow for session), `m` (allow, and switch the session to `auto` mode), `d` (deny, returns an error `tool_result` so the model
can adapt), and an editable command line. `bypass` exists as a flag and prints a
warning banner that stays up for the session.

With the CLI backend the policy is Claude Code's: `--permission-mode` (default
`acceptEdits`) decides what runs unasked, and only what it would ask about
reaches the modal. The modal is built with `a`, `A`, `m` and `d`
(`03-tui.md` §5c); the editable command line and the `bypass` banner are not.

Denials return `tool_result` with `is_error: true` rather than aborting the segment.
Dropping the result entirely leaves the conversation malformed.

## 4. Segment lifecycle and cancellation

```
prompt submitted
  → segments row (started_at, prompt blob)              [store]
  → request; stream deltas                           [ui: middle column live]
  → stop_reason=tool_use → run tools in parallel
      → mutations captured, file_changes rows written [store]
      → all tool_results in ONE user message → loop
  → terminal stop_reason
  → segments.ended_at, usage_json                       [store]
  → enqueue sectioning + per-file summaries        [summarizer]
```

With the CLI as the agent, Ctrl+C sends the `interrupt` control request (§1),
and the CLI stops its own tools. The stopped input becomes `Done`, not
`Failed`. For the planned direct-API host: `Esc` must cancel for real: hold a `tokio_util::sync::CancellationToken` per
segment, drop the HTTP stream on cancel, and kill child processes in the bash tool's
process *group* (not just the direct child, or `sh -c` leaves orphans). Then
write the segment with `stop_reason='cancelled'` — a cancelled segment is still a
segment and its partial changes are real, so it gets summarized like any other.

## 5. Key handling

API key resolution, in order: `ANTHROPIC_API_KEY` → `ANTHROPIC_AUTH_TOKEN` →
OAuth profile under `~/.config/anthropic/` (what `ant auth login` writes).
An unset `ANTHROPIC_API_KEY` does **not** mean there are no credentials, so
`cuz doctor` should report which source resolved rather than just "missing key".

Wrap the key in `secrecy::SecretString` (0.10.3) so it cannot be accidentally
logged or included in a `Debug` dump, and never write it to
`.because/config.toml` — that file gets committed.
