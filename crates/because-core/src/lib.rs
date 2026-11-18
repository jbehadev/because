//! Domain types for `because`, and the fixture loader for the sample data
//! `cuz` shows without a live agent. Live sessions are kept by `because-store`.
//!
//! Vocabulary (see `docs/design/00-overview.md` §2):
//!
//! - **session** — one conversation with the agent.
//! - **segment** — one input the developer typed, and everything the agent did
//!   in response. Four inputs in a conversation are four segments. (An API
//!   "turn".)
//! - **section** — a labelled part of one segment's response, sliced by the
//!   summarizer so a long response reads as a list.
//! - **change** — one file touched by one segment, with its hunks.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod time;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// A name the session came with. Empty for a session started here: the UI
    /// names it after its first input, and after that input's summary once the
    /// summarizer has written one.
    #[serde(default)]
    pub title: String,
    pub repo: String,
    pub branch: String,
    pub segments: Vec<Segment>,
    /// Where the session came from. Sessions from an earlier run or another
    /// tool are history: shown, not continued.
    #[serde(default)]
    pub origin: Origin,
    /// RFC 3339, UTC. Empty for sample data.
    #[serde(default)]
    pub started_at: String,
    /// The whole session's cost as the tool that ran it recorded it, when that
    /// is known but the per-input costs are not (an imported transcript only
    /// records running totals at some points).
    #[serde(default)]
    pub recorded_cost: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Started in this run (or sample data).
    #[default]
    New,
    /// Run by `cuz live` before, and kept in the store.
    Stored,
    /// A Claude Code session run outside `because`, read from its
    /// transcript.
    Imported,
}

impl Origin {
    /// History is read, not continued: its agent and working tree are gone or
    /// belong to another tool.
    pub fn is_history(self) -> bool {
        self != Origin::New
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub seq: u32,
    /// What the developer typed. Human-authored, verbatim.
    pub input: String,
    /// One-paragraph account of what the agent did. Generated.
    pub summary: String,
    /// The model that wrote `summary` in this session, and what that cost.
    /// Empty for summaries that came with the data.
    #[serde(default)]
    pub summary_by: String,
    #[serde(default)]
    pub summary_cost_usd: f64,
    pub elapsed: String,
    pub cost_usd: f64,
    /// The cost of this one input is not known — only a total covering it and
    /// others was recorded. `cost_usd` is 0 then and is not shown.
    #[serde(default)]
    pub cost_unrecorded: bool,
    /// When the input was sent: RFC 3339, UTC. Empty for sample data.
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub duration_ms: u64,
    pub model: String,
    /// `None` when the input was answered without changing files. In a live
    /// session this is the snapshot commit taken after the input finished.
    pub commit: Option<Commit>,
    pub sections: Vec<Section>,
    pub changes: Vec<FileChange>,
    #[serde(default)]
    pub status: SegStatus,
    /// The agent's final reply, verbatim. Shown where the generated summary
    /// would be when there is none.
    #[serde(default)]
    pub reply: String,
    /// How `commit` was tied to this input: by snapshots taken around it
    /// (empty), or — for imported history — because the commit was made while
    /// it ran (`commit_time`), which is a guess the UI says it is.
    #[serde(default)]
    pub link: String,
    /// Text streaming in for the response in progress. Not persisted.
    #[serde(skip)]
    pub streaming: String,
    /// The file changes came from the store as rows — paths, counts, summaries
    /// — without the code. The runtime re-derives the code from git when the
    /// input is looked at, rather than the store keeping a copy of it.
    #[serde(skip)]
    pub code_unloaded: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegStatus {
    /// The agent is still working on this input.
    Running,
    /// Finished; changes may still be being computed.
    #[default]
    Done,
    /// The agent failed or exited before finishing.
    Failed,
}

/// What the agent reports while working on one input. Produced by the agent
/// host (`because-agent`) from the Claude CLI's stream, consumed by the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// A response to an input began. Sent once per input.
    Started { model: String, session_id: String },
    /// What the agent has just started doing — "thinking", "writing",
    /// "running Bash" — so a long silent stretch still shows signs of life.
    Activity(String),
    /// A fragment of assistant text, for live display.
    TextDelta(String),
    /// A complete assistant text block. Supersedes the deltas that built it.
    Text(String),
    /// Summarized reasoning, when the model returned any.
    Thinking(String),
    /// A tool call. `target` is the path, command or pattern it acts on.
    ToolUse { id: String, name: String, target: String },
    ToolResult { id: String, is_error: bool, text: String },
    /// The response to the input is complete.
    Finished {
        reply: String,
        /// Cumulative for the whole CLI session, not this input. The cost of one
        /// input is the difference between consecutive values.
        cost_total_usd: f64,
        duration_ms: u64,
        is_error: bool,
        permission_denials: usize,
    },
    /// The agent wants to use a tool its permission mode does not allow on its
    /// own. It waits, mid-input, until the developer answers.
    PermissionRequest(PermissionRequest),
    /// The agent withdrew a request before it was answered.
    PermissionCancelled { request_id: String },
    /// The agent took in an input from stdin, as the CLI echoes it back. An
    /// input sent while the agent works arrives here when the agent reads it:
    /// before `Finished` if it joins the running input, after `Finished` if it
    /// starts a response of its own.
    InputTaken(String),
    /// A background task of the agent ended, with the agent's summary of it.
    /// When no input runs, the agent then starts a response of its own: a
    /// `Started` with no input before it.
    TaskEnded(String),
    /// The agent process ended.
    Exited { detail: String },
}

/// A tool call waiting for the developer's approval.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequest {
    /// The agent's id for the request; the answer has to name it.
    pub request_id: String,
    pub tool: String,
    /// What the call acts on, on one line: the path, command or pattern.
    pub target: String,
    /// The call in full, as the developer needs to see it to decide: the
    /// whole command, the file and what goes into it.
    pub detail: String,
    /// The rule the agent suggests for allowing calls like this one for the
    /// rest of the session, e.g. `Bash(cargo test:*)`. `None` when it
    /// suggested none, and only allowing this once is offered.
    pub always: Option<String>,
    /// For `AskUserQuestion`, what the agent asks. The developer answers with
    /// [`Decision::Answers`], not with an approval. Empty for other tools.
    pub questions: Vec<Question>,
}

/// A question that the agent asks the developer, with the options it offers.
/// The developer can also type an answer that is not an option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub question: String,
    /// A short label for the question, for example `Auth method`.
    pub header: String,
    pub options: Vec<QuestionOption>,
    /// The developer can select more than one option.
    pub multi: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

/// The developer's answer to a [`PermissionRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    AllowOnce,
    /// Allow this call, and add the suggested rule for the rest of the session.
    AllowSession,
    /// Allow this call, and switch the session to `auto` permission mode:
    /// from then on Claude Code's classifier decides what runs without asking.
    AllowAuto,
    /// Refuse it. The agent gets an error result and carries on without it.
    Deny,
    /// The answers to the questions of an `AskUserQuestion` call, one for
    /// each question, in the same order.
    Answers(Vec<String>),
}

/// How much of one of the subscription's allowances has been spent.
///
/// Claude Code meters a subscription over rolling windows — a five-hour one and
/// a weekly one — rather than a balance, so "left" is per window and each has
/// its own reset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    /// The key the API used, e.g. `five_hour`.
    pub key: String,
    /// Two or three cells wide: this is shown in a column 26 cells across.
    pub label: String,
    /// Percent of the window's allowance already used, 0–100.
    pub used_pct: f64,
    /// When the window rolls over, as the API gave it. Empty when unknown.
    pub resets_at: String,
}

impl UsageWindow {
    pub fn left_pct(&self) -> f64 {
        (100.0 - self.used_pct).clamp(0.0, 100.0)
    }
}

/// What is left of the Claude subscription's allowance, per window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub windows: Vec<UsageWindow>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub committed_at: String,
    /// What the input's changes are measured against: the snapshot taken
    /// before it, or the parent of the first commit matched to it. Empty in
    /// sample data.
    #[serde(default)]
    pub parent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub seq: u32,
    pub title: String,
    pub intent: String,
    pub summary: String,
    pub confidence: f32,
    /// What the model actually wrote, in order. Verbatim — never generated.
    pub narration: Vec<NarrationStep>,
    pub hunk_refs: Vec<HunkRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrationStep {
    pub kind: StepKind,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Thinking,
    Text,
    ToolCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunkRef {
    pub change_id: String,
    pub hunk_seq: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub id: String,
    pub path: String,
    pub change_type: String,
    pub additions: u32,
    pub deletions: u32,
    pub risk: Risk,
    pub summary: String,
    pub state: String,
    /// First line of `content` in the post-image file (1-based). Small files
    /// are whole; large ones are windowed around the hunks.
    pub content_start_line: u32,
    pub content: Vec<String>,
    pub hunks: Vec<Hunk>,
    /// Post-image line → (short sha, line number in that commit's version),
    /// from `git blame`, for lines introduced by segments of the same session.
    /// This is what lets a later segment's view show which earlier input wrote
    /// a line. See `docs/design/07-provenance.md` §3.
    #[serde(default)]
    pub line_origins: BTreeMap<u32, (String, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    None,
    Behavioral,
    Api,
    Security,
    Perf,
    Data,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::None => "ok",
            Risk::Behavioral => "behavioral",
            Risk::Api => "api",
            Risk::Security => "security",
            Risk::Perf => "perf",
            Risk::Data => "data",
        }
    }
    /// Glyph shown in the changes column. `None` is deliberately quiet.
    pub fn glyph(self) -> &'static str {
        match self {
            Risk::None => "○",
            Risk::Perf => "◈",
            Risk::Api => "◆",
            Risk::Behavioral | Risk::Data => "⚠",
            Risk::Security => "✖",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    pub seq: u32,
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub summary: String,
    pub patch: String,
}

/// Which post-image lines a hunk added, and what it removed.
///
/// Deleted lines do not exist in the post-image, so they are keyed by the
/// post-image line they sat *before* — that is the anchor the code pane renders
/// a collapsed marker at. See `docs/design/03-tui.md` §6.
#[derive(Debug, Default, Clone)]
pub struct HunkLines {
    pub added: Vec<u32>,
    pub deleted_at: BTreeMap<u32, Vec<String>>,
}

impl Hunk {
    pub fn lines(&self) -> HunkLines {
        let mut out = HunkLines::default();
        let mut new_line = self.new_start;
        for raw in self.patch.lines().skip(1) {
            let mut chars = raw.chars();
            match chars.next() {
                Some('+') => {
                    out.added.push(new_line);
                    new_line += 1;
                }
                Some('-') => out
                    .deleted_at
                    .entry(new_line)
                    .or_default()
                    .push(chars.as_str().to_string()),
                Some(' ') => new_line += 1,
                _ => {}
            }
        }
        out
    }

    /// Whether a line of this hunk's post-image falls inside it.
    pub fn contains_new_line(&self, line: u32) -> bool {
        line >= self.new_start && line < self.new_start + self.new_lines.max(1)
    }
}

impl Fixture {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }
}

impl Session {
    pub fn cost(&self) -> f64 {
        // `+ 0.0`: an empty f64 sum is -0.0, which prints as "$-0.00".
        self.recorded_cost.unwrap_or_else(|| self.segments.iter().map(|s| s.cost_usd).sum::<f64>() + 0.0)
    }

    /// The segment whose commit wrote a line, by short sha.
    pub fn segment_by_commit(&self, short: &str) -> Option<(usize, &Segment)> {
        self.segments
            .iter()
            .enumerate()
            .find(|(_, s)| s.commit.as_ref().is_some_and(|c| c.short == short))
    }
}

/// The `model` of an input that the developer ran as a shell command (`!`).
/// No model answered it: the shell did, and `reply` is the output.
pub const SHELL: &str = "shell";

impl Segment {
    /// The developer ran this input as a shell command, not as an input to the
    /// agent.
    pub fn is_shell(&self) -> bool {
        self.model == SHELL
    }

    /// Everything the agent said in response, verbatim, in order: every text
    /// block, not only the final reply. Falls back to the reply when the
    /// narration holds no text (a failure, or data without narration).
    pub fn said(&self) -> String {
        let text: Vec<&str> = self
            .sections
            .iter()
            .flat_map(|s| &s.narration)
            .filter(|n| n.kind == StepKind::Text)
            .map(|n| n.body.as_str())
            .collect();
        if text.is_empty() { self.reply.clone() } else { text.join("\n\n") }
    }

    /// The tool calls the agent made, as their one-line display.
    pub fn tool_calls(&self) -> impl Iterator<Item = &str> {
        self.sections.iter().flat_map(|s| &s.narration).filter(|n| n.kind == StepKind::ToolCall).map(|n| n.body.as_str())
    }

    pub fn change(&self, id: &str) -> Option<&FileChange> {
        self.changes.iter().find(|c| c.id == id)
    }

    pub fn change_by_path(&self, path: &str) -> Option<&FileChange> {
        self.changes.iter().find(|c| c.path == path)
    }

    /// Sections that reference a given hunk. More than one means the hunk is
    /// coarser than the work it contains — see `06-roadmap.md`, open question 1.
    pub fn sections_for_hunk(&self, change_id: &str, hunk_seq: u32) -> Vec<&Section> {
        self.sections
            .iter()
            .filter(|s| s.hunk_refs.iter().any(|r| r.change_id == change_id && r.hunk_seq == hunk_seq))
            .collect()
    }
}

impl Section {
    /// The one section a response has before a sectioner has sliced it: all
    /// of it, citing every hunk the input produced.
    pub fn whole_response(narration: Vec<NarrationStep>) -> Self {
        Section {
            seq: 0,
            title: "response".into(),
            intent: String::new(),
            summary: String::new(),
            confidence: 0.0,
            narration,
            hunk_refs: Vec::new(),
        }
    }

    pub fn hunks_in<'a>(&self, change: &'a FileChange) -> Vec<&'a Hunk> {
        self.hunk_refs
            .iter()
            .filter(|r| r.change_id == change.id)
            .filter_map(|r| change.hunks.iter().find(|h| h.seq == r.hunk_seq))
            .collect()
    }
}
