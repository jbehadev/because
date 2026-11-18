# Fixtures

`ripgrep.json` is the sample data `cuz demo` and `cuz render` show and most of
the TUI tests run against — `cuz live` reads the store instead: three sessions,
seven segments, built from real
[BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) history.
The ripgrep code in this file is used under the Unlicense, which is one of the
two ripgrep licenses.

| Session | Inputs | Commits |
|---|---|---|
| hidden-path checks for incremental walking | 4 | `626b895` → `6e527e9` → `c55818f`, then a question answered without changes |
| global gitignore ignored on NixOS | 2 | `241b87b`, then a question |
| globset allocation cleanup | 1 | `068a63e` |

The first session is the one to look at: three consecutive inputs editing the
same file, which is what exercises the earlier-input tier (`N┊`) and line
resolution in the code pane.

## What is real, and what is authored

Split by file, so the boundary is visible in the repository and not only in
this README:

| File | Contents | Source |
|---|---|---|
| `build_fixture.py` | the extractor | — |
| `authored.json` | inputs, summaries, sections, narration, risks, timings, costs | **written by hand** |
| `ripgrep.json` | the merged fixture the skeleton loads | **generated** by `build_fixture.py` |

Derived from git by the builder: commit metadata, paths, addition/deletion
counts, hunk ranges and headers, unified diff text, post-image content (whole
files up to 400 lines, otherwise ±25 lines around the hunks), and per-line
origins from `git blame` at each input's commit, restricted to the session's
commits.

**Nothing in `authored.json` is generated.** There was no API key in the
environment this was built in, so none of it is measured Haiku output. That
hides two things:

- Real narration contains false starts, abandoned approaches and commentary on
  work that was later reverted. This narration is tidier than that, and the
  `superseded` qualifier in `07-provenance.md` §2 is not exercised.
- Summary length and specificity here are the design target, not evidence that
  Haiku produces them. M2's eval set answers that.

Replace `authored.json`'s summaries with generated ones as soon as there is an
API key: same schema, same renderer, and the difference is the first real
signal on summary quality.

## Regenerating

```
git clone --depth 150 https://github.com/BurntSushi/ripgrep /tmp/ripgrep
python3 fixtures/build_fixture.py --repo /tmp/ripgrep
```

The builder validates every authored hunk reference (`"path#n"`) against the
hunks it extracts and exits on a mismatch. Hunk numbering depends on diff
context size (`-U3`): at `-U2`, `6e527e9` has five hunks; at `-U3` it has
three, and `c55818f` collapses to one. A silent mismatch would attach sections
to the wrong code.

The `scripts/record_*.py` scripts record real CLI sessions into
`claude-stream/`. Each script runs `scripts/scrub_fixture.py` on its output.
The scrub replaces private paths with `/repo` and `/home/user`. It also cuts the
lists in the `init` event to three entries. The scrub stops with an error if a
private string remains. Examine a new recording before you commit it.

## `claude-stream/two-inputs.jsonl`

A real Claude Code CLI session in `--input-format stream-json
--output-format stream-json --include-partial-messages` mode: two inputs in a
scratch repository, the first creating a file with Write, the second appending
to it with Bash. It is what `crates/because-agent` is tested against and what the
live-session tests in `crates/because-tui/tests/live.rs` replay.

Scrubbed before committing: filesystem paths (including Claude Code's
dash-joined project-directory form) replaced with `/repo` and `/home/user`; the
environment-specific lists in the `init` event (tools, skills, slash commands,
plugins, agents) trimmed to three entries; and the `partial_json` fragments of
streamed tool inputs blanked, because file paths arrive split across them and
cannot be scrubbed reliably. The parser reads tool inputs from the complete
`assistant` blocks, which are intact.

There is no recording of a permission prompt yet. `crates/because-agent/tests/permission.rs`
tests against hand-written lines in the Agent SDK's control-protocol shape;
`scripts/record_permission.py` records a real exchange (one allowed and one
denied Bash call, about a cent on haiku) to `claude-stream/permission.jsonl`,
which should replace them.

## `claude-stream/task-notification.jsonl`

Written by hand. It has the shape of a real `cuz live` log, with short text and
no private paths. An input starts a background command and ends. Then two
tasks end, and the CLI starts a response with no steps and a response with two
steps. `crates/because-agent/tests/task_notification.rs` and
`crates/because-tui/tests/task_notification.rs` replay it.

## `claude-summary/result.json`

The real output of one summary call (`claude -p --model haiku --output-format
json`, thinking off; `docs/design/02-summarization.md` §0) on a ~1,000-word
reply. `crates/because-agent/tests/summary.rs` parses it. It holds no paths; the
session id and uuid are random and are left as recorded.

## `claude-transcript/two-inputs.jsonl`

A real Claude Code transcript (`~/.claude/projects/<dir>/<session>.jsonl`) from
two `claude -p` inputs in a scratch repository — the first creating a file with
Write, the second appending to it with Bash — which is what
`crates/because-agent/tests/transcript.rs` and the import test in
`crates/because-cli/src/import.rs` read. The interactive-session records the parser
also handles (`origin`, `isMeta`, compaction summaries, slash commands, titles)
are spliced into a copy of it in the tests rather than recorded.

Scrubbed before committing: paths replaced with `/repo` and `/home/user`,
including the dash-joined directory form; `attachment`, `queue-operation` and
`atis-latch` records removed, since they list the environment (tools, skills,
settings) and the parser skips them.
