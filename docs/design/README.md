# `because` design docs

Read in order:

| Doc | Contents |
|---|---|
| [00-overview.md](00-overview.md) | thesis, the three columns, decisions of record, architecture, crate layout, CLI surface |
| [01-data-model.md](01-data-model.md) | SQLite schema v1, content addressing, JSONL record format, git merge semantics, staleness, migrations |
| [02-summarization.md](02-summarization.md) | the three-pass pipeline, model config, cost/latency envelope, caching caveats, redaction, quality eval |
| [03-tui.md](03-tui.md) | framework survey, event loop, the four-stop focus ring, navigation keys and terminal portability, columns layout, several sessions, history and reviving, the allowance bar, approval prompts, the reader, the full-screen code pane, diff rendering, perf targets |
| [04-agent-host.md](04-agent-host.md) | `AgentBackend` trait, CLI backend and what its stream contains, permission prompts, Claude Code transcripts, the subscription allowance, Messages API backend, tools, permissions, segment lifecycle |
| [05-plugins.md](05-plugins.md) | extension traits, ABI-representable rules, registry and config, the WASM path, MCP |
| [06-roadmap.md](06-roadmap.md) | milestones M0–M6, risks, open questions |
| [07-provenance.md](07-provenance.md) | retaining what the model generated, narration links, anchoring context to lines that move, retention tiers, inbound redaction |

Decisions of record are listed in [00-overview.md §4](00-overview.md#4-decisions-of-record).
Amend these documents in place when a decision changes; the git history is the
decision log.
