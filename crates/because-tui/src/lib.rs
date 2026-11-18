//! The view layer: `Model` + `update` + `view`.
//!
//! `update` is pure over the model and `view` does no I/O, which is what lets
//! this run against a fixture now and a store later without changing either.
//!
//! Columns: Sessions → Segments (the inputs of the selected session) → Changes
//! (the files the selected input touched). See `docs/design/00-overview.md` §2.
//!
//! A session is one conversation. `n` in the Sessions column starts another in
//! the same repository; they share the screen and nothing else, and they run
//! side by side — an agent, a conversation and a working tree each.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use ratatui_textarea::{TextArea, WrapMode};
use because_core::{
    AgentEvent, Origin, Commit, Decision, FileChange, Fixture, Hunk, HunkRef, NarrationStep, PermissionRequest, Risk,
    SegStatus, Section, Segment, Session, StepKind, Usage,
};

pub mod keymap;
mod markdown;
pub use keymap::dispatch;

// ── palette ────────────────────────────────────────────────────────────────
const FG: Color = Color::Rgb(0xd8, 0xd8, 0xd4);
const SOFT: Color = Color::Rgb(0xa8, 0xad, 0xb4);
const DIM: Color = Color::Rgb(0x6b, 0x70, 0x76);
const FRAME: Color = Color::Rgb(0x3a, 0x3f, 0x46);
const SEL: Color = Color::Rgb(0x2b, 0x3a, 0x4a);
const EARLIER_BG: Color = Color::Rgb(0x26, 0x2b, 0x31);
/// Everything a model generated wears this colour. Verbatim text never does.
const GENERATED: Color = Color::Rgb(0x7a, 0xa2, 0xd8);
/// What the developer typed. The third provenance class, alongside generated
/// text and the model's own verbatim words.
const HUMAN: Color = Color::Rgb(0xd8, 0xb0, 0x80);
const THINKING: Color = Color::Rgb(0x9a, 0x9f, 0xa6);
const ADDED: Color = Color::Rgb(0x7f, 0xb3, 0x7f);
const REMOVED: Color = Color::Rgb(0xc8, 0x7f, 0x7f);
/// Work in a tree that is not committed yet, and could be lost with it.
const UNCOMMITTED: Color = Color::Rgb(0xd0, 0xa8, 0x60);

fn risk_color(r: Risk) -> Color {
    match r {
        Risk::None => DIM,
        Risk::Perf => Color::Rgb(0x6f, 0xa8, 0xa8),
        Risk::Api => Color::Rgb(0xa8, 0x8f, 0xc0),
        Risk::Behavioral | Risk::Data => Color::Rgb(0xd0, 0xa8, 0x60),
        Risk::Security => Color::Rgb(0xd0, 0x70, 0x70),
    }
}

// ── model ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Columns,
    Code,
    /// One input in full: everything typed and everything the agent said. The
    /// columns show a summary; this is where the whole text is read.
    Reader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sessions,
    Segments,
    Changes,
    Code,
}

impl Focus {
    pub fn index(self) -> usize {
        match self {
            Focus::Sessions => 0,
            Focus::Segments => 1,
            Focus::Changes => 2,
            Focus::Code => 3,
        }
    }
    fn from_index(i: usize) -> Self {
        [Focus::Sessions, Focus::Segments, Focus::Changes, Focus::Code][i.min(3)]
    }
}

/// The Segments pane holds a list and a composer. Which one has the keyboard
/// decides whether plain keys type or command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegFocus {
    List,
    Composer,
}

/// An input the developer has typed that the agent has not run yet. It is the
/// next segment of its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub session: usize,
    pub text: String,
}

/// A tool call an agent is waiting on the developer to approve.
#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    pub session: usize,
    pub req: PermissionRequest,
    /// When it came to the front of the queue, i.e. on screen. Keys pressed
    /// within `APPROVAL_GRACE` of this are not taken as an answer.
    pub shown: Instant,
    /// The answers so far, when the agent asks questions.
    pub pick: Pick,
}

/// Where the developer is in the questions of an `AskUserQuestion` call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pick {
    /// The question on screen.
    pub at: usize,
    /// The row under the cursor: an option, or "Other" after the last option.
    pub cursor: usize,
    /// The options selected, for a question that allows more than one.
    pub chosen: Vec<bool>,
    /// The text typed for "Other". `None` when the text field is closed.
    pub other: Option<String>,
    /// One answer for each question before `at`.
    pub answers: Vec<String>,
}

/// A key in the question prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKey {
    Up,
    Down,
    /// The option with this index: choose it, or select it when the question
    /// allows more than one.
    Choose(usize),
    /// Select or clear the option under the cursor.
    Toggle,
    /// Answer the question on screen with the cursor, the selection, or the
    /// typed text.
    Enter,
    Type(char),
    Erase,
    /// Close the text field, or decline all the questions.
    Back,
}

/// How long an approval prompt is on screen before a key answers it. The
/// prompt opens over whatever was being typed, and the next keystroke of a
/// word — an `a`, a `d` — should not be what decides.
pub const APPROVAL_GRACE: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Debug, Clone)]
pub enum Msg {
    FocusNext,
    FocusPrev,
    FocusLeft,
    FocusRight,
    FocusAt(usize),
    Up,
    Down,
    Enter,
    Back,
    ToggleRationale,
    ToggleVerbatim,
    ToggleTranscript,
    /// Open the reader on the selected input, or close it.
    Read,
    PageUp,
    PageDown,
    Top,
    Bottom,
    /// The reader moves to the next/previous input.
    NextInput,
    PrevInput,
    /// Next/previous region written by the selected input.
    NextRegion,
    PrevRegion,
    /// Next/previous region written by any input in the session.
    NextAny,
    PrevAny,
    Quit,
    /// Start another session: a fresh conversation in the same repository.
    NewSession,
    /// Try to continue the selected earlier session: resume its conversation
    /// with Claude Code, in the tree it ran in.
    Revive,
    /// The runtime resumed it: it becomes one of this run's sessions. Keyed by
    /// id, since history may have moved while claude was starting.
    Revived { session_id: String },
    /// It could not be resumed, and stays history.
    ReviveFailed { session_id: String, error: String },
    /// Answer the approval prompt on screen.
    Answer(Decision),
    /// A key in the question prompt on screen.
    Pick(PickKey),
    // composer
    Compose,
    LeaveComposer,
    Edit(KeyEvent),
    Paste(String),
    Newline,
    Submit,
    /// Send the draft, and interrupt the running input so that the agent
    /// reads it now (Claude Code's send now). With no input running, `Submit`.
    SendNow,
    /// Ctrl+X, the first key of Ctrl+X Ctrl+S: send now on terminals that
    /// report Ctrl+Enter as Enter.
    ChordX,
    ClearOrQuit,
    // live sessions
    /// The clock, for elapsed time on a running input.
    Tick(Instant),
    /// Something the agent of a session said. Tagged with the session, since
    /// several of them are working at once and an event that arrived while
    /// another session was on screen still belongs to the one that sent it.
    Agent(usize, AgentEvent),
    /// The changes an input made, computed from its snapshots.
    Changes { session: usize, seg: usize, commit: Commit, changes: Vec<FileChange> },
    ChangesFailed { session: usize, seg: usize, error: String },
    /// A small model's one-sentence summary of an input.
    Summary { session: usize, seg: usize, text: String, model: String, cost_usd: f64 },
    SummaryFailed { session: usize, seg: usize, error: String },
    /// The code of an input loaded from the store, re-derived from git. Keyed by
    /// session id rather than position: history moves down when a session is
    /// started, and this may arrive after that.
    Code { session_id: String, seg: usize, changes: Vec<FileChange> },
    CodeFailed { session_id: String, seg: usize, error: String },
    /// How much of the Claude subscription's allowance is left, re-asked every
    /// few minutes. Not tagged with a session: the allowance is the account's,
    /// and every session on screen spends the same one.
    Usage(Usage),
    UsageFailed(String),
    /// Files with changes not yet committed, by working tree, for every tree
    /// of the repository. Re-asked every few seconds; replaces the last answer.
    Uncommitted(BTreeMap<String, usize>),
    /// A shell command that the developer ran (`!`) stopped. `code` is `None`
    /// when a signal stopped it. `error` tells why the command did not start.
    ShellDone { session: usize, seg: usize, output: String, code: Option<i32>, error: Option<String>, duration_ms: u64 },
}

/// Work for the runtime. `update` stays free of I/O by describing effects
/// here; the runtime drains `Model::outbox` after every update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// Snapshot the working tree, then send this input to the agent.
    Send { session: usize, seg: usize, text: String },
    /// The agent finished this input: snapshot again and compute its changes.
    Finish { session: usize, seg: usize },
    /// Ask a small model for a one-sentence summary of this input.
    Summarize { session: usize, seg: usize },
    /// Write this input, and its session, to the store as they are now.
    Persist { session: usize, seg: usize },
    /// Resume this earlier session's conversation in the tree it ran in, and
    /// answer with `Revived` or `ReviveFailed`.
    Revive { session_id: String, repo: String },
    /// Tell a session's agent what the developer decided about a tool call.
    Answer { session: usize, request_id: String, decision: Decision },
    /// Send an input to a session's agent while it works. The agent reads it
    /// at its next step; `AgentEvent::InputTaken` tells where it went.
    SendMidTurn { session: usize, text: String },
    /// The agent started this input on its own, after a background task
    /// ended. Snapshot the working tree, as `Send` does, but send nothing.
    Begin { session: usize, seg: usize },
    /// Stop the input that a session's agent works on. The agent ends it with
    /// `Finished`, and it keeps the conversation. If the input is a shell
    /// command, stop the command; it ends with `Msg::ShellDone`.
    Stop { session: usize },
    /// Snapshot the working tree, then run this shell command in it, and
    /// answer with `Msg::ShellDone`.
    Shell { session: usize, seg: usize, command: String },
}

pub struct Model {
    pub fx: Fixture,
    pub view: View,
    pub focus: Focus,
    pub session: usize,
    pub seg: usize,
    pub change: usize,
    /// Line under the cursor in the code pane, as a post-image line number.
    pub code_cursor: u32,
    /// Retained per change id, so leaving the code pane and coming back is free.
    pub code_cursor_memo: BTreeMap<String, u32>,
    pub rationale: bool,
    pub verbatim: bool,
    pub transcript: bool,
    pub quit: bool,
    /// Retained across pane changes: coming back to Segments returns you to
    /// whichever of list/composer you left.
    pub seg_focus: SegFocus,
    pub composer: TextArea<'static>,
    pub queued: Vec<Queued>,
    /// One-line explanation shown in the status line until the next key, for
    /// when a key cannot do what it normally does (e.g. no code to open).
    pub notice: Option<String>,
    /// An agent is attached: Enter sends inputs rather than only queuing them.
    pub live: bool,
    pub outbox: Vec<Cmd>,
    /// The segment each session is working on, by session index. Sessions run
    /// side by side — a conversation and a working tree each — so this is a map
    /// and not the one input the agent is on.
    pub running: BTreeMap<usize, usize>,
    /// When each session's running input was sent, for its elapsed time.
    pub running_since: BTreeMap<usize, Instant>,
    /// Segments whose changes are being computed.
    pub pending: BTreeSet<(usize, usize)>,
    pub agent_model: Option<String>,
    /// Last cumulative cost reported by each session's agent; the cost of one
    /// input is the delta within its own session. The CLI reports a total for
    /// the conversation it is serving, so a single running total across
    /// sessions would charge an input with what another session had spent.
    pub cost_total: BTreeMap<usize, f64>,
    pub now: Instant,
    /// Sessions whose running input has been sent but whose agent has not said
    /// anything yet.
    pub awaiting_start: BTreeSet<usize>,
    /// What each session's agent last said it started doing: thinking, writing,
    /// a tool.
    pub activity: BTreeMap<usize, String>,
    /// Where the agent's raw output is being logged, to point at when it is quiet.
    pub log_path: Option<String>,
    /// Request a summary when an input finishes. Set by `cuz live`.
    pub summarize: bool,
    /// Ask for inputs to be written to the store as they change. Set by
    /// `cuz live` when it has one.
    pub persist: bool,
    /// Inputs whose summary has been requested and has not arrived.
    pub summarizing: BTreeSet<(usize, usize)>,
    /// First line of the reader shown, unless `reader_follow`.
    pub reader_scroll: u16,
    /// Keep the reader at the end of the text, as it streams in.
    pub reader_follow: bool,
    /// The pane the reader was opened from, to return to.
    pub reader_from: Focus,
    /// Written by `view` each frame: the reader's largest scroll offset and its
    /// page height. Both depend on the terminal size, which `update` never sees.
    pub reader_extent: Cell<(u16, u16)>,
    /// What is left of the subscription's allowance, as last asked. `None`
    /// until the first answer arrives.
    pub usage: Option<Usage>,
    /// Why there is none, when asking failed. Shown in place of the bar rather
    /// than as a notice: it is a standing condition, not an event.
    pub usage_error: Option<String>,
    /// Ids of earlier sessions the runtime is trying to resume.
    pub reviving: BTreeSet<String>,
    /// Tool calls waiting for approval, oldest first, from every session. The
    /// first is the prompt on screen; an agent waits until its own is answered,
    /// so none of them can be put off to later.
    pub approvals: Vec<Approval>,
    /// Files with changes not yet committed, by working tree, as last asked.
    /// A session's work is only safe once it is committed, and a tree nobody
    /// is looking at is the easiest one to forget.
    pub uncommitted: BTreeMap<String, usize>,
    /// Sessions whose agent finished an input while another session was on
    /// screen: a reply nobody has looked at yet. By index, which is stable for
    /// this run's sessions, the only ones with agents. Cleared by selecting it.
    pub unread: BTreeSet<usize>,
    /// Sessions whose running input the developer stopped, until the agent
    /// ends it.
    pub stopping: BTreeSet<usize>,
    /// Inputs sent to each session's agent while it worked, which the agent
    /// has not read yet. Oldest first, as the agent reads them.
    pub mid_turn: BTreeMap<usize, VecDeque<String>>,
    /// Background tasks that ended while no input ran, by session. The next
    /// response that the agent starts on its own shows them as its input.
    pub tasks_ended: BTreeMap<usize, Vec<String>>,
    /// Sessions whose agent started a response with no input, which has no
    /// segment yet. The first step of the response makes the segment: a
    /// response with no steps gets none.
    pub own_turn: BTreeSet<usize>,
    /// Ctrl+X was the last key. `dispatch` reads and clears it with the next
    /// key, so it is a `Cell`.
    pub chord_x: Cell<bool>,
    /// Sessions whose running input is a shell command (`!`), not an input to
    /// the agent.
    pub shells: BTreeSet<usize>,
    /// The shell commands that ran in each session since its last input to
    /// the agent, with their output. The agent reads them with its next
    /// input, as in Claude Code.
    pub shell_context: BTreeMap<usize, Vec<String>>,
}

/// Which lines of a file the code pane marks, from the viewpoint of the
/// selected input.
#[derive(Debug, Default, Clone)]
pub struct Highlight {
    /// Written by the selected input.
    pub selected: BTreeSet<u32>,
    /// Written by an earlier input of the same session → that segment's index.
    pub earlier: BTreeMap<u32, usize>,
    /// Removed by the selected input, keyed by the line they sat before.
    pub deletions: BTreeMap<u32, Vec<String>>,
}

/// Where a line came from: the answer to "why is this line here?".
pub struct Resolution<'a> {
    pub seg_index: usize,
    pub segment: &'a Segment,
    pub short: String,
    /// The line's number in the version of the file that input produced.
    pub orig_line: u32,
    pub change: Option<&'a FileChange>,
    pub hunk: Option<&'a Hunk>,
    pub sections: Vec<&'a Section>,
}

impl Model {
    pub fn new(fx: Fixture) -> Self {
        let mut m = Self {
            fx,
            view: View::Columns,
            focus: Focus::Segments,
            session: 0,
            seg: 0,
            change: 0,
            code_cursor: 1,
            code_cursor_memo: BTreeMap::new(),
            rationale: false,
            verbatim: false,
            transcript: false,
            quit: false,
            seg_focus: SegFocus::Composer,
            composer: new_composer(),
            queued: Vec::new(),
            notice: None,
            live: false,
            outbox: Vec::new(),
            running: BTreeMap::new(),
            running_since: BTreeMap::new(),
            pending: BTreeSet::new(),
            agent_model: None,
            cost_total: BTreeMap::new(),
            now: Instant::now(),
            awaiting_start: BTreeSet::new(),
            activity: BTreeMap::new(),
            log_path: None,
            summarize: false,
            persist: false,
            summarizing: BTreeSet::new(),
            reader_scroll: 0,
            reader_follow: false,
            reader_from: Focus::Segments,
            reader_extent: Cell::new((0, 0)),
            usage: None,
            usage_error: None,
            reviving: BTreeSet::new(),
            approvals: Vec::new(),
            uncommitted: BTreeMap::new(),
            unread: BTreeSet::new(),
            stopping: BTreeSet::new(),
            mid_turn: BTreeMap::new(),
            tasks_ended: BTreeMap::new(),
            own_turn: BTreeSet::new(),
            chord_x: Cell::new(false),
            shells: BTreeSet::new(),
            shell_context: BTreeMap::new(),
        };
        // Like returning to a conversation: the most recent input is selected,
        // directly above the composer.
        m.seg = m.segment_count().saturating_sub(1);
        m
    }

    pub fn session(&self) -> &Session {
        &self.fx.sessions[self.session]
    }
    pub fn segments(&self) -> &[Segment] {
        &self.session().segments
    }
    pub fn queued_for_session(&self) -> impl Iterator<Item = &Queued> {
        self.queued.iter().filter(move |q| q.session == self.session)
    }
    /// Segments that ran, then inputs still queued. The selection spans both.
    pub fn segment_count(&self) -> usize {
        self.segments().len() + self.queued_for_session().count()
    }
    pub fn selected_segment(&self) -> Option<&Segment> {
        self.segments().get(self.seg)
    }
    /// Whether a session's agent is working. Each session has one conversation,
    /// so it serves one input at a time — but the others do not wait for it.
    pub fn is_running(&self, session: usize) -> bool {
        self.running.contains_key(&session)
    }
    /// Whether a session's agent works on an input. A shell command that runs
    /// is not work of the agent.
    fn agent_runs(&self, session: usize) -> bool {
        self.is_running(session) && !self.shells.contains(&session)
    }
    /// The session that Ctrl+C stops: the session of the approval prompt on
    /// screen, else the session on screen. `None` when that session has no
    /// running input, or when it stops already.
    pub fn stoppable(&self) -> Option<usize> {
        let s = self.approval().map_or(self.session, |a| a.session);
        (self.is_running(s) && !self.stopping.contains(&s)).then_some(s)
    }
    /// The approval prompt on screen, if one is.
    pub fn approval(&self) -> Option<&Approval> {
        self.approvals.first()
    }
    /// Whether the prompt on screen has been there long enough to answer.
    pub fn approval_ready(&self) -> bool {
        self.approval().is_some_and(|a| self.now.saturating_duration_since(a.shown) >= APPROVAL_GRACE)
    }
    /// The tool call a session's agent is waiting on approval for.
    pub fn waiting_on(&self, session: usize) -> Option<&PermissionRequest> {
        self.approvals.iter().find(|a| a.session == session).map(|a| &a.req)
    }
    pub fn selected_queued(&self) -> Option<&Queued> {
        self.seg
            .checked_sub(self.segments().len())
            .and_then(|i| self.queued_for_session().nth(i))
    }
    pub fn visible_changes(&self) -> Vec<&FileChange> {
        self.selected_segment().map(|s| s.changes.iter().collect()).unwrap_or_default()
    }
    pub fn current_change(&self) -> Option<&FileChange> {
        self.selected_segment()?.changes.get(self.change)
    }

    /// The selected input, when its code has to be loaded before it can be
    /// shown: it came from the store with its files listed but not their code.
    /// The runtime asks git for it; the selection is what decides when, so an
    /// input that is never looked at costs nothing.
    pub fn wants_code(&self) -> Option<(&Session, usize)> {
        let s = self.session();
        s.segments.get(self.seg).filter(|x| x.code_unloaded).map(|_| (s, self.seg))
    }

    /// Whether the code stop exists for the current selection.
    pub fn can_open_code(&self) -> bool {
        self.current_change().is_some()
    }

    /// The input nearest the selection that did change files: earlier first,
    /// since that is usually the work being asked about.
    pub fn nearest_input_with_changes(&self) -> Option<&Segment> {
        let segs = self.segments();
        let here = self.seg.min(segs.len());
        segs[..here]
            .iter()
            .rev()
            .chain(segs[here..].iter())
            .find(|s| !s.changes.is_empty())
    }

    /// Why the code screen cannot open, and what to do instead.
    fn why_no_code(&self) -> String {
        let hint = match self.nearest_input_with_changes() {
            Some(s) => format!(" Input {} did — select it in Segments.", s.seq),
            None => String::new(),
        };
        if self.selected_queued().is_some() {
            format!("No code to open: this input has not run yet.{hint}")
        } else {
            let n = self.selected_segment().map(|s| s.seq).unwrap_or(0);
            format!("No code to open: input {n} changed no files.{hint}")
        }
    }

    /// True when plain keys should type rather than command.
    pub fn composing(&self) -> bool {
        let here = match self.view {
            View::Columns => self.focus == Focus::Segments,
            View::Reader => self.reader_has_composer() && self.reader_from == Focus::Segments,
            View::Code => false,
        };
        here && self.seg_focus == SegFocus::Composer
    }

    /// The reader is on the latest input of a session that takes new ones, so
    /// the composer sits below it: having read the reply, the next input can be
    /// written without going back to the columns for it.
    pub fn reader_has_composer(&self) -> bool {
        self.view == View::Reader && !self.session().origin.is_history() && self.seg + 1 == self.segments().len()
    }
    pub fn draft_is_empty(&self) -> bool {
        self.composer.is_empty()
    }
    pub fn draft(&self) -> String {
        self.composer.lines().join("\n")
    }

    fn clamp(&mut self) {
        self.seg = self.seg.min(self.segment_count().saturating_sub(1));
        self.change = self.change.min(self.visible_changes().len().saturating_sub(1));
    }

    pub fn highlight(&self, change: &FileChange) -> Highlight {
        let mut h = Highlight::default();
        let this = self.selected_segment().and_then(|s| s.commit.as_ref()).map(|c| c.short.as_str());
        for (line, (short, _)) in &change.line_origins {
            if Some(short.as_str()) == this {
                h.selected.insert(*line);
            } else if let Some((idx, _)) = self.session().segment_by_commit(short) {
                h.earlier.insert(*line, idx);
            }
        }
        for hunk in &change.hunks {
            let lines = hunk.lines();
            if change.line_origins.is_empty() {
                // No blame data: fall back to what the hunks say this input added.
                h.selected.extend(lines.added);
            }
            h.deletions.extend(lines.deleted_at);
        }
        h
    }

    /// Resolve a post-image line to the input that wrote it, via the blame
    /// origin, then that input's hunk, then the sections that cite the hunk.
    pub fn resolve_line(&self, change: &FileChange, line: u32) -> Option<Resolution<'_>> {
        let (short, orig) = match change.line_origins.get(&line) {
            Some((s, o)) => (s.clone(), *o),
            None if change.line_origins.is_empty() && self.highlight(change).selected.contains(&line) => {
                (self.selected_segment()?.commit.as_ref()?.short.clone(), line)
            }
            None => return None,
        };
        let (seg_index, segment) = self.session().segment_by_commit(&short)?;
        let origin = segment.change_by_path(&change.path);
        let hunk = origin.and_then(|c| c.hunks.iter().find(|h| h.contains_new_line(orig)));
        let sections = match (origin, hunk) {
            (Some(c), Some(h)) => segment.sections_for_hunk(&c.id, h.seq),
            _ => Vec::new(),
        };
        Some(Resolution { seg_index, segment, short, orig_line: orig, change: origin, hunk, sections })
    }

    fn content_range(&self) -> Option<(u32, u32)> {
        let c = self.current_change()?;
        let start = c.content_start_line;
        Some((start, start + c.content.len().saturating_sub(1) as u32))
    }

    fn enter_code(&mut self) {
        let Some(c) = self.current_change() else {
            self.notice = Some(self.why_no_code());
            return;
        };
        if self.selected_segment().is_some_and(|s| s.code_unloaded) {
            self.notice = Some("Loading this input's code from git — a moment.".into());
            return;
        }
        let id = c.id.clone();
        let first = self.highlight(c).selected.first().copied().unwrap_or(c.content_start_line);
        self.code_cursor = self.code_cursor_memo.get(&id).copied().unwrap_or(first);
        self.view = View::Code;
        self.focus = Focus::Code;
    }

    fn leave_code(&mut self) {
        if let Some(c) = self.current_change() {
            self.code_cursor_memo.insert(c.id.clone(), self.code_cursor);
        }
        self.view = View::Columns;
        self.focus = Focus::Changes;
        self.rationale = false;
    }

    fn open_reader(&mut self) {
        let Some(s) = self.selected_segment() else {
            self.notice = Some(if self.selected_queued().is_some() {
                "Nothing to read yet: this input has not run.".into()
            } else {
                "No input selected.".into()
            });
            return;
        };
        self.reader_follow = s.status == SegStatus::Running;
        if self.view == View::Code {
            self.leave_code();
        }
        self.reader_from = self.focus;
        self.reader_scroll = 0;
        self.view = View::Reader;
    }

    fn close_reader(&mut self) {
        self.view = View::Columns;
        self.focus = self.reader_from;
    }

    /// The reader's first visible line, as last rendered.
    pub fn reader_offset(&self) -> u16 {
        let (max, _) = self.reader_extent.get();
        if self.reader_follow { max } else { self.reader_scroll.min(max) }
    }

    fn scroll_reader(&mut self, d: i32) {
        let (max, _) = self.reader_extent.get();
        let next = (self.reader_offset() as i32 + d).clamp(0, max as i32) as u16;
        self.reader_scroll = next;
        // At the end, stay there as more text arrives, like a terminal.
        self.reader_follow = next >= max && d > 0;
    }

    fn reader_step_input(&mut self, d: i32) {
        let last = self.segments().len().saturating_sub(1) as i32;
        let next = (self.seg as i32 + d).clamp(0, last) as usize;
        if next != self.seg {
            self.seg = next;
            self.change = 0;
            self.reader_scroll = 0;
            self.reader_follow = self.segments()[next].status == SegStatus::Running;
        }
    }
}

/// Replies up to this many words fit in the Segments column as they are, so
/// they are shown verbatim and not summarized.
pub const SHORT_REPLY_WORDS: usize = 30;

/// Whether a finished input gets a summary call: its reply is too long to show
/// in the column, or it used tools, so the reply alone may not say what it did.
pub fn wants_summary(s: &Segment) -> bool {
    s.status == SegStatus::Done
        && !s.is_shell()
        && s.summary.is_empty()
        && (word_count(&s.said()) > SHORT_REPLY_WORDS || s.tool_calls().next().is_some())
}

pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

/// First line of each run of consecutive lines.
fn run_starts(lines: impl IntoIterator<Item = u32>) -> Vec<u32> {
    let mut out = Vec::new();
    let mut prev: Option<u32> = None;
    for l in lines {
        if prev.is_none_or(|p| l != p + 1) {
            out.push(l);
        }
        prev = Some(l);
    }
    out
}

// ── update ─────────────────────────────────────────────────────────────────

pub fn update(m: &mut Model, msg: Msg) {
    apply(m, msg);
    // Whatever is on screen has been seen, however it got there: a key, a new
    // session, a revived one.
    m.unread.remove(&m.session);
}

fn apply(m: &mut Model, msg: Msg) {
    if !matches!(
        msg,
        Msg::Tick(_)
            | Msg::Agent(..)
            | Msg::Changes { .. }
            | Msg::ChangesFailed { .. }
            | Msg::Summary { .. }
            | Msg::SummaryFailed { .. }
            | Msg::Usage(_)
            | Msg::UsageFailed(_)
            | Msg::Uncommitted(_)
            | Msg::Code { .. }
            | Msg::CodeFailed { .. }
            | Msg::Revived { .. }
            | Msg::ReviveFailed { .. }
    ) {
        m.notice = None;
    }
    // The reader takes movement keys for scrolling. Keys that go to a pane
    // close it first, then do what they do from the pane it was opened from.
    if m.view == View::Reader && !m.transcript {
        let page = m.reader_extent.get().1.saturating_sub(2).max(1) as i32;
        match msg {
            Msg::Up => return m.scroll_reader(-1),
            Msg::Down => return m.scroll_reader(1),
            Msg::PageUp => return m.scroll_reader(-page),
            Msg::PageDown => return m.scroll_reader(page),
            Msg::Top => {
                m.reader_scroll = 0;
                m.reader_follow = false;
                return;
            }
            Msg::Bottom => {
                m.reader_follow = true;
                return;
            }
            Msg::NextInput => return m.reader_step_input(1),
            Msg::PrevInput => return m.reader_step_input(-1),
            Msg::Back | Msg::FocusLeft | Msg::Read | Msg::LeaveComposer => return m.close_reader(),
            // On the latest input the composer is here: type into it rather
            // than going back to the columns for it.
            Msg::Compose if m.reader_has_composer() => {
                m.reader_from = Focus::Segments;
                m.seg_focus = SegFocus::Composer;
                return;
            }
            Msg::Enter | Msg::NextRegion | Msg::PrevRegion | Msg::NextAny | Msg::PrevAny | Msg::ToggleRationale => {
                return;
            }
            Msg::FocusNext | Msg::FocusPrev | Msg::FocusRight | Msg::FocusAt(_) | Msg::Compose => m.close_reader(),
            _ => {}
        }
    }
    match msg {
        Msg::Quit => m.quit = true,
        Msg::NewSession => new_session(m),
        Msg::Revive => revive(m),
        Msg::Revived { session_id } => revived(m, &session_id),
        Msg::ReviveFailed { session_id, error } => {
            m.reviving.remove(&session_id);
            let name = m.fx.sessions.iter().position(|s| s.id == session_id).map(|i| session_name(m, i).0);
            m.notice = Some(format!(
                "Could not revive “{}”: {} — it stays read only; n starts a new session.",
                name.unwrap_or(session_id),
                error.lines().find(|l| !l.trim().is_empty()).unwrap_or("claude exited")
            ));
        }
        Msg::Answer(decision) => answer(m, decision),
        Msg::Pick(key) => pick(m, key),
        // Tab cycles the stops that exist. With nothing to open, the code stop
        // is skipped rather than becoming a wall the cycle cannot get past.
        Msg::FocusNext => {
            let next = (m.focus.index() + 1) % 4;
            set_focus(m, if next == 3 && !m.can_open_code() { 0 } else { next });
        }
        Msg::FocusPrev => {
            let prev = (m.focus.index() + 3) % 4;
            set_focus(m, if prev == 3 && !m.can_open_code() { 2 } else { prev });
        }
        Msg::FocusRight => set_focus(m, (m.focus.index() + 1).min(3)),
        Msg::FocusLeft => set_focus(m, m.focus.index().saturating_sub(1)),
        Msg::FocusAt(i) => set_focus(m, i.min(3)),
        // Enter opens what is selected: a session's inputs, an input's full
        // text, a file's code. An input that changed no files has only its
        // text to open, so Enter in Changes reads it.
        Msg::Enter => match m.focus {
            Focus::Sessions => m.focus = Focus::Segments,
            Focus::Segments => m.open_reader(),
            Focus::Changes if m.current_change().is_none() && m.selected_segment().is_some() => m.open_reader(),
            Focus::Changes => m.enter_code(),
            Focus::Code => m.rationale = !m.rationale,
        },
        Msg::Read => m.open_reader(),
        // Outside the reader these have nothing to act on.
        Msg::PageUp | Msg::PageDown | Msg::Top | Msg::Bottom | Msg::NextInput | Msg::PrevInput => {}
        Msg::Back => {
            if m.transcript {
                m.transcript = false;
            } else if m.rationale {
                m.rationale = false;
            } else if m.view == View::Code {
                m.leave_code();
            } else {
                set_focus(m, m.focus.index().saturating_sub(1));
            }
        }
        Msg::Up => step(m, -1),
        Msg::Down => step(m, 1),
        Msg::NextRegion => jump(m, 1, false),
        Msg::PrevRegion => jump(m, -1, false),
        Msg::NextAny => jump(m, 1, true),
        Msg::PrevAny => jump(m, -1, true),
        Msg::ToggleRationale => m.rationale = !m.rationale,
        Msg::ToggleVerbatim => m.verbatim = !m.verbatim,
        Msg::ToggleTranscript => m.transcript = !m.transcript,
        Msg::Compose => {
            if m.view == View::Code {
                m.leave_code();
            }
            m.focus = Focus::Segments;
            m.seg_focus = SegFocus::Composer;
        }
        Msg::LeaveComposer => m.seg_focus = SegFocus::List,
        Msg::Edit(k) => {
            m.composer.input(k);
        }
        Msg::Paste(text) => {
            // Bracketed paste arrives whole, so embedded newlines are inserted
            // as text rather than read as Enter — a pasted stack trace must not
            // submit itself on its first line.
            if m.reader_has_composer() {
                m.reader_from = Focus::Segments;
            }
            if m.focus == Focus::Segments && m.view == View::Columns || m.reader_has_composer() {
                m.seg_focus = SegFocus::Composer;
                m.composer.insert_str(text);
            }
        }
        Msg::Newline => m.composer.insert_newline(),
        Msg::Submit if m.reviving.contains(&m.session().id) => {
            m.notice = Some("Still reviving this session — send again once claude has resumed it.".into());
        }
        Msg::Submit if m.session().origin.is_history() => {
            m.notice = Some(
                "An earlier session is for reading: its agent is gone. Esc then c tries to revive it; \
                 n starts a new session — the draft comes with you."
                    .into(),
            );
        }
        Msg::Submit => {
            let text = m.draft().trim().to_string();
            if text.starts_with('!') {
                run_shell(m, text);
            } else if !text.is_empty() {
                m.composer.clear();
                // Only this session's own agent has to be free: another session
                // working is no reason to hold an input back. Within a session
                // the queue is served in order, so anything already waiting
                // here goes first.
                // A response that the agent started on its own is work too,
                // before its first step makes it an input.
                let working = m.is_running(m.session) || m.own_turn.contains(&m.session);
                let idle = !working && m.queued_for_session().next().is_none();
                // A shell command does not read inputs: the input waits for it.
                let shell = m.shells.contains(&m.session);
                if m.live && idle {
                    start_input(m, m.session, text, true);
                } else if m.live && working && !shell {
                    send_mid_turn(m, text);
                    m.notice = Some("Sent to the agent. It reads your text at its next step. Ctrl+Enter makes it read the text now.".into());
                } else {
                    if !m.live {
                        m.notice = Some(
                            "No agent here: this is sample data. To work with Claude, run `cuz` inside a git repository."
                                .into(),
                        );
                    } else if shell {
                        m.notice = Some("Queued. The agent gets your input when the command stops.".into());
                    }
                    m.queued.push(Queued { session: m.session, text });
                    m.seg = m.segment_count() - 1;
                    m.change = 0;
                }
                // Sent from the reader: back to the columns, where the new
                // input is selected and its progress shows.
                if m.view == View::Reader {
                    m.close_reader();
                }
            }
        }
        Msg::SendNow => send_now(m),
        Msg::ChordX => {
            m.chord_x.set(true);
            m.notice = Some("Ctrl+X: push Ctrl+S to send now.".into());
        }
        Msg::Tick(now) => m.now = now,
        Msg::Agent(session, ev) => on_agent(m, session, ev),
        Msg::Changes { session, seg, commit, changes } => {
            m.pending.remove(&(session, seg));
            if let Some(s) = m.fx.sessions.get_mut(session).and_then(|x| x.segments.get_mut(seg)) {
                // Until there is a summarizer, a live input has one section — its
                // whole response — which cites every hunk the input produced.
                let refs = changes
                    .iter()
                    .flat_map(|c| c.hunks.iter().map(|h| HunkRef { change_id: c.id.clone(), hunk_seq: h.seq }))
                    .collect();
                if let Some(sec) = s.sections.first_mut() {
                    sec.hunk_refs = refs;
                }
                s.commit = Some(commit);
                s.changes = changes;
                persist(m, session, seg);
            }
            m.clamp();
        }
        Msg::ChangesFailed { session, seg, error } => {
            m.pending.remove(&(session, seg));
            m.notice = Some(format!("Could not compute changes for input {}: {error}", seg + 1));
        }
        Msg::Summary { session, seg, text, model, cost_usd } => {
            m.summarizing.remove(&(session, seg));
            if let Some(s) = m.fx.sessions.get_mut(session).and_then(|x| x.segments.get_mut(seg)) {
                s.summary = text;
                s.summary_by = model;
                s.summary_cost_usd = cost_usd;
                persist(m, session, seg);
            }
        }
        Msg::Code { session_id, seg, changes } => {
            if let Some(s) = m.fx.sessions.iter_mut().find(|x| x.id == session_id).and_then(|x| x.segments.get_mut(seg)) {
                // Git gives the code; the store's rows keep what was generated
                // about it, which git cannot.
                let mut changes = changes;
                for c in &mut changes {
                    if let Some(row) = s.changes.iter().find(|r| r.path == c.path) {
                        c.summary = row.summary.clone();
                        c.risk = row.risk;
                        c.state = row.state.clone();
                    }
                }
                let refs = changes
                    .iter()
                    .flat_map(|c| c.hunks.iter().map(|h| HunkRef { change_id: c.id.clone(), hunk_seq: h.seq }))
                    .collect();
                if is_unsectioned(s)
                    && let Some(sec) = s.sections.first_mut()
                {
                    sec.hunk_refs = refs;
                }
                s.changes = changes;
                s.code_unloaded = false;
            }
            m.clamp();
        }
        Msg::CodeFailed { session_id, seg, error } => {
            if let Some(s) = m.fx.sessions.iter_mut().find(|x| x.id == session_id).and_then(|x| x.segments.get_mut(seg)) {
                // Stop asking: the list of files still shows, from the store.
                s.code_unloaded = false;
            }
            m.notice = Some(format!("Could not load the code for input {}: {error}", seg + 1));
        }
        Msg::SummaryFailed { session, seg, error } => {
            m.summarizing.remove(&(session, seg));
            m.notice = Some(format!("No summary for input {}: {error}", seg + 1));
        }
        Msg::Usage(u) => {
            m.usage = Some(u);
            m.usage_error = None;
        }
        // The last good reading stays on screen: one failed poll in a series
        // that has been working says less than the number it would replace.
        // The column has no room for the reason, so the first failure says it
        // once in the status line; the rest are only in the log.
        Msg::UsageFailed(e) => {
            if m.usage.is_none() && m.usage_error.is_none() {
                m.notice = Some(format!("No usage allowance to show: {e}"));
            }
            m.usage_error = Some(e);
        }
        Msg::ShellDone { session, seg, output, code, error, duration_ms } => {
            shell_done(m, session, seg, output, code, error, duration_ms);
        }
        Msg::Uncommitted(n) => m.uncommitted = n,
        // Each press does the first of these that applies: clear the draft,
        // stop the running input, quit. Thus a second press after a stop quits.
        Msg::ClearOrQuit => {
            if m.approval().is_none() && m.composing() && !m.draft_is_empty() {
                m.composer.clear();
            } else if let Some(s) = m.stoppable() {
                stop(m, s);
            } else {
                m.quit = true;
            }
        }
    }
}

/// Start another session on the same repository and select it, ready to type.
///
/// A session is one conversation: the new one knows nothing of what was said in
/// the others, keeps its own inputs, and snapshots its tree under its own refs.
/// It also runs side by side with them — `cuz live` gives it a `claude` process
/// and a working tree of its own when its first input is sent, so a session you
/// never type into costs nothing and two that are typed into do not wait for
/// each other.
fn new_session(m: &mut Model) {
    if m.view == View::Code {
        m.leave_code();
    }
    m.view = View::Columns;
    // The repository this run works in, even when history is on screen: an
    // earlier session's tree may be a worktree that is gone.
    let home = m.fx.sessions.iter().find(|s| !s.origin.is_history()).unwrap_or(m.session());
    let (repo, branch) = (home.repo.clone(), home.branch.clone());
    let n = m.fx.sessions.len() + 1;
    // Above the history, below the sessions of this run: history is kept in
    // one block, newest first, and nothing refers to it by position.
    let at = m.fx.sessions.iter().position(|s| s.origin.is_history()).unwrap_or(m.fx.sessions.len());
    m.fx.sessions.insert(at, Session {
        // Unnamed until something is typed in it: a session is named after its
        // first input, not after the moment it was started.
        title: String::new(),
        id: format!("new-{n}"),
        repo,
        branch,
        segments: Vec::new(),
        ..Default::default()
    });
    m.session = at;
    m.seg = 0;
    m.change = 0;
    m.focus = Focus::Segments;
    m.seg_focus = SegFocus::Composer;
    if !m.live {
        m.notice = Some(
            "New session. This view has no agent: `cuz`, inside a git repository, works with Claude.".into(),
        );
    }
}

/// Ask the runtime to resume the selected earlier session. Nothing moves until
/// it answers: a session that cannot be resumed stays where it was, read only.
fn revive(m: &mut Model) {
    let (id, repo, history) = (m.session().id.clone(), m.session().repo.clone(), m.session().origin.is_history());
    m.notice = Some(if !history {
        "This session is live already — type to continue it.".into()
    } else if !m.live {
        "No agent here: this is sample data. `cuz live`, inside a git repository, can revive a session.".into()
    } else if m.reviving.contains(&id) {
        "Already reviving this session — a moment.".into()
    } else {
        let msg = format!("Reviving “{}”: asking claude to resume it in {repo} …", session_name(m, m.session).0);
        m.outbox.push(Cmd::Revive { session_id: id.clone(), repo });
        m.reviving.insert(id);
        msg
    });
}

/// An earlier session claude has resumed: it joins this run's sessions, at the
/// end of them and above the history, and takes inputs again.
fn revived(m: &mut Model, id: &str) {
    m.reviving.remove(id);
    let Some(from) = m.fx.sessions.iter().position(|s| s.id == id && s.origin.is_history()) else { return };
    let selected = m.session == from;
    let mut s = m.fx.sessions.remove(from);
    s.origin = Origin::New;
    // History is all below this point and none of it is running, so moving one
    // of its sessions up shifts nothing that is keyed by position.
    let at = m.fx.sessions.iter().position(|x| x.origin.is_history()).unwrap_or(m.fx.sessions.len());
    m.fx.sessions.insert(at, s);
    if selected {
        m.session = at;
        m.focus = Focus::Segments;
        m.seg_focus = SegFocus::Composer;
        m.seg = m.segment_count().saturating_sub(1);
        m.change = 0;
    } else if m.session >= at && m.session < from {
        m.session += 1;
    }
    let s = &m.fx.sessions[at];
    // Two agents in one checkout would count each other's edits as their own.
    // The first session always works in the repository it was started in; a
    // later one only once it has a tree, on its first input.
    let shared = m.fx.sessions[..at]
        .iter()
        .enumerate()
        .position(|(i, x)| x.repo == s.repo && (i == 0 || !x.segments.is_empty()))
        .map(|i| format!(" It shares that tree with session {}: each will count the other's edits as its own.", i + 1))
        .unwrap_or_default();
    m.notice = Some(format!("Revived “{}” — claude resumed it in {}. Type to continue.{shared}", session_name(m, at).0, s.repo));
}

/// Answer the prompt on screen and bring up the next one.
fn answer(m: &mut Model, decision: Decision) {
    if m.approvals.is_empty() {
        return;
    }
    let a = m.approvals.remove(0);
    if let Some(next) = m.approvals.first_mut() {
        next.shown = m.now;
    }
    if decision == Decision::AllowSession
        && let Some(rule) = &a.req.always
    {
        m.notice = Some(format!("Allowed for the rest of session {}: {rule}", a.session + 1));
    }
    if decision == Decision::AllowAuto {
        m.notice = Some(format!("Session {} is in auto mode: Claude Code's classifier decides what runs without asking", a.session + 1));
    }
    m.outbox.push(Cmd::Answer { session: a.session, request_id: a.req.request_id, decision });
}

/// A key in the question prompt. The last answer sends all the answers to the
/// agent.
fn pick(m: &mut Model, key: PickKey) {
    let Some(a) = m.approvals.first_mut() else { return };
    let Some(q) = a.req.questions.get(a.pick.at) else { return };
    let p = &mut a.pick;
    let n = q.options.len();
    p.chosen.resize(n, false);
    let selected = |p: &Pick| -> Vec<String> {
        q.options.iter().zip(&p.chosen).filter(|(_, c)| **c).map(|(o, _)| o.label.clone()).collect()
    };
    if let Some(t) = &mut p.other {
        // The text field for "Other" is open.
        match key {
            PickKey::Type(c) => t.push(c),
            PickKey::Erase => {
                t.pop();
            }
            PickKey::Back => p.other = None,
            PickKey::Enter if !t.trim().is_empty() => {
                let mut all = if q.multi { selected(p) } else { Vec::new() };
                all.push(p.other.take().unwrap_or_default().trim().to_string());
                return answered(m, all.join(", "));
            }
            _ => {}
        }
        return;
    }
    match key {
        PickKey::Back => return answer(m, Decision::Deny),
        PickKey::Up => p.cursor = p.cursor.saturating_sub(1),
        PickKey::Down => p.cursor = (p.cursor + 1).min(n),
        PickKey::Type(_) | PickKey::Erase => {}
        PickKey::Choose(i) if i > n => {}
        PickKey::Choose(_) | PickKey::Toggle | PickKey::Enter => {
            let row = if let PickKey::Choose(i) = key { i } else { p.cursor };
            p.cursor = row;
            if row == n {
                p.other = Some(String::new());
            } else if !q.multi {
                let label = q.options[row].label.clone();
                return answered(m, label);
            } else if key == PickKey::Enter {
                let all = selected(p);
                // With nothing selected, Enter takes the option under the cursor.
                let done = if all.is_empty() { q.options[row].label.clone() } else { all.join(", ") };
                return answered(m, done);
            } else {
                p.chosen[row] = !p.chosen[row];
            }
        }
    }
}

/// Keep the answer to the question on screen, and show the next question. The
/// last answer sends all the answers to the agent.
fn answered(m: &mut Model, done: String) {
    let Some(a) = m.approvals.first_mut() else { return };
    let p = &mut a.pick;
    let total = a.req.questions.len();
    let mut answers = std::mem::take(&mut p.answers);
    answers.push(done);
    if answers.len() == total {
        return answer(m, Decision::Answers(answers));
    }
    *p = Pick { at: answers.len(), answers, ..Pick::default() };
}

/// Write text to the running agent. As in Claude Code, the agent reads it at
/// its next step, and does not wait for the input to end.
fn send_mid_turn(m: &mut Model, text: String) {
    m.mid_turn.entry(m.session).or_default().push_back(text.clone());
    m.outbox.push(Cmd::SendMidTurn { session: m.session, text });
}

/// Claude Code's send now: send the draft, then interrupt the running input.
/// The interrupted input ends, and the agent then reads the text as an input
/// of its own. Running tools stop: the CLI has no request to move them to the
/// background, as Claude Code does.
fn send_now(m: &mut Model) {
    let s = m.session;
    if !m.live || !m.agent_runs(s) {
        update(m, Msg::Submit);
        return;
    }
    let text = m.draft().trim().to_string();
    if !text.is_empty() {
        m.composer.clear();
        send_mid_turn(m, text);
    }
    if m.mid_turn.get(&s).is_none_or(|q| q.is_empty()) {
        m.notice = Some("Nothing to send now: type the input first.".into());
        return;
    }
    if !m.stopping.contains(&s) {
        stop(m, s);
    }
    m.notice = Some("Interrupted the agent. It reads your text now.".into());
    if m.view == View::Reader {
        m.close_reader();
    }
}

/// Ask the runtime to stop a session's running input. The input ends when the
/// agent sends `Finished`. Its prompt goes now: the agent withdraws it.
fn stop(m: &mut Model, session: usize) {
    m.stopping.insert(session);
    drop_approvals(m, |a| a.session == session);
    m.outbox.push(Cmd::Stop { session });
    let seg = m.running.get(&session).map_or(0, |i| i + 1);
    m.notice = Some(format!("Stopping input {seg} of session {}. Ctrl+C again quits.", session + 1));
}

/// Ask the runtime to write an input to the store, when there is one.
fn persist(m: &mut Model, session: usize, seg: usize) {
    if m.persist {
        m.outbox.push(Cmd::Persist { session, seg });
    }
}

/// Make an input the next segment and ask the runtime to send it. `follow`
/// moves the selection to it.
fn start_input(m: &mut Model, session: usize, text: String, follow: bool) {
    let idx = open_input(m, session, text.clone(), follow);
    // As in Claude Code, the agent reads the shell commands that ran since its
    // last input before the text that the developer typed.
    let ran = m.shell_context.remove(&session).unwrap_or_default();
    let text = if ran.is_empty() { text } else { format!("{}\n\n{text}", ran.join("\n")) };
    m.outbox.push(Cmd::Send { session, seg: idx, text });
    persist(m, session, idx);
}

/// The output that the agent reads of one shell command, from the end. Claude
/// Code keeps the same quantity.
const SHELL_CONTEXT_CHARS: usize = 30_000;
/// The output that an input keeps of one shell command, from the end.
const SHELL_OUTPUT_CHARS: usize = 200_000;

/// Run a draft that starts with `!` as a shell command in the tree of the
/// session, as in Claude Code. The command becomes an input of its own, with
/// snapshots around it. The agent reads the command and its output with the
/// next input.
fn run_shell(m: &mut Model, text: String) {
    let command = text[1..].trim().to_string();
    let s = m.session;
    let why = if command.is_empty() {
        Some("Type a shell command after the !.")
    } else if !m.live {
        Some("No shell here: this is sample data. To run commands, run `cuz` inside a git repository.")
    } else if m.is_running(s) || m.own_turn.contains(&s) || m.queued_for_session().next().is_some() {
        Some("This session works. Run the command when it stops: the command and the agent use the same tree.")
    } else {
        None
    };
    if let Some(why) = why {
        m.notice = Some(why.into());
        return;
    }
    m.composer.clear();
    // The agent did not read the notices of its background tasks: they stay
    // for its next response.
    let tasks = m.tasks_ended.remove(&s);
    let idx = open_input(m, s, text, true);
    if let Some(t) = tasks {
        m.tasks_ended.insert(s, t);
    }
    m.fx.sessions[s].segments[idx].model = because_core::SHELL.into();
    m.awaiting_start.remove(&s);
    m.shells.insert(s);
    m.outbox.push(Cmd::Shell { session: s, seg: idx, command });
    persist(m, s, idx);
    if m.view == View::Reader {
        m.close_reader();
    }
}

/// A shell command stopped: keep its output, and keep it for the agent too.
fn shell_done(
    m: &mut Model,
    session: usize,
    i: usize,
    output: String,
    code: Option<i32>,
    error: Option<String>,
    duration_ms: u64,
) {
    m.shells.remove(&session);
    let stopped = m.stopping.remove(&session);
    let Some(seg) = m.fx.sessions.get_mut(session).and_then(|x| x.segments.get_mut(i)) else { return };
    seg.elapsed = fmt_duration(duration_ms);
    seg.duration_ms = duration_ms;
    if let Some(e) = error {
        seg.status = SegStatus::Failed;
        seg.reply = format!("Could not run the command: {e}");
        m.notice = Some(seg.reply.clone());
    } else {
        seg.status = SegStatus::Done;
        let end = match code {
            Some(0) => None,
            Some(n) => Some(format!("exit status {n}")),
            None if stopped => Some("■ You stopped this command.".to_string()),
            None => Some("A signal stopped the command.".to_string()),
        };
        let mut reply = tail(output.trim_end(), SHELL_OUTPUT_CHARS);
        if let Some(end) = end {
            if !reply.is_empty() {
                reply.push('\n');
            }
            reply.push_str(&end);
        }
        let command = seg.input[1..].trim();
        m.shell_context.entry(session).or_default().push(format!(
            "<bash-input>{command}</bash-input>\n<bash-stdout>{}</bash-stdout><bash-stderr></bash-stderr>",
            tail(&reply, SHELL_CONTEXT_CHARS)
        ));
        seg.reply = reply;
    }
    input_ended(m, session, i);
}

/// The last `max` characters of `text`, with a note when it is cut.
fn tail(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let kept: String = text.chars().skip(n - max).collect();
    format!("[{} earlier characters removed]\n{kept}", n - max)
}

/// Make an input the next segment, which runs. Returns its index.
fn open_input(m: &mut Model, session: usize, text: String, follow: bool) -> usize {
    let segs = &mut m.fx.sessions[session].segments;
    let seq = segs.len() as u32 + 1;
    segs.push(Segment {
        id: format!("live-{seq}"),
        seq,
        input: text.clone(),
        model: m.agent_model.clone().unwrap_or_else(|| "claude".into()),
        sections: vec![Section::whole_response(Vec::new())],
        status: SegStatus::Running,
        ..Default::default()
    });
    let idx = segs.len() - 1;
    m.running.insert(session, idx);
    m.running_since.insert(session, m.now);
    m.awaiting_start.insert(session);
    m.activity.remove(&session);
    // The agent read the text of this input after it started a response, so
    // the response is not one of its own. It read the notices too.
    m.own_turn.remove(&session);
    m.tasks_ended.remove(&session);
    if follow && session == m.session {
        m.seg = idx;
        m.change = 0;
    }
    idx
}

/// Make the response that the agent started on its own the next segment.
/// Its input tells which background tasks ended. Follow it only if the
/// selection is on the last input of the session.
fn open_own_turn(m: &mut Model, session: usize) {
    let tasks = m.tasks_ended.remove(&session).unwrap_or_default();
    let text = if tasks.is_empty() {
        "The agent continued with no input.".to_string()
    } else {
        format!("The agent continued after a background task ended:\n{}", tasks.join("\n"))
    };
    let count = m.fx.sessions[session].segments.len();
    let follow = session == m.session && m.seg + 1 >= count;
    let idx = open_input(m, session, text, follow);
    m.awaiting_start.remove(&session);
    m.outbox.push(Cmd::Begin { session, seg: idx });
    persist(m, session, idx);
}

/// The agent read an input. An input sent while it worked joins the running
/// input, or, when that input has ended, becomes an input of its own, which
/// runs already. The echo of an input that `start_input` sent has nothing to do.
fn input_taken(m: &mut Model, session: usize, text: String) {
    let queue = m.mid_turn.entry(session).or_default();
    if queue.front() != Some(&text) {
        return;
    }
    queue.pop_front();
    match m.running.get(&session) {
        Some(&i) => {
            let seg = &mut m.fx.sessions[session].segments[i];
            seg.input = format!("{}\n\nAdded while the agent worked:\n{text}", seg.input);
            persist(m, session, i);
        }
        None => {
            let idx = open_input(m, session, text, true);
            persist(m, session, idx);
        }
    }
}

fn on_agent(m: &mut Model, session: usize, ev: AgentEvent) {
    // Handled whether or not an input is running: an agent waiting on an
    // answer is stuck until it gets one, and a prompt left over from an agent
    // that has gone would be answering nobody.
    match ev {
        AgentEvent::PermissionRequest(req) => {
            if !m.approvals.iter().any(|a| a.session == session && a.req.request_id == req.request_id) {
                let shown = m.now;
                m.approvals.push(Approval { session, req, shown, pick: Pick::default() });
            }
            return;
        }
        AgentEvent::PermissionCancelled { request_id } => {
            drop_approvals(m, |a| a.session == session && a.req.request_id == request_id);
            return;
        }
        AgentEvent::InputTaken(text) => {
            input_taken(m, session, text);
            return;
        }
        // While an input runs, the agent reads the notice in that input.
        AgentEvent::TaskEnded(summary) => {
            if !m.agent_runs(session) {
                m.tasks_ended.entry(session).or_default().push(summary);
            }
            return;
        }
        AgentEvent::Finished { .. } => {
            drop_approvals(m, |a| a.session == session);
            m.own_turn.remove(&session);
        }
        // An input that the agent did not read is gone with it.
        AgentEvent::Exited { .. } => {
            drop_approvals(m, |a| a.session == session);
            m.mid_turn.remove(&session);
            m.tasks_ended.remove(&session);
            m.own_turn.remove(&session);
        }
        AgentEvent::Started { ref model, .. } if !m.agent_runs(session) => {
            m.agent_model = Some(model.clone());
            m.own_turn.insert(session);
        }
        AgentEvent::Activity(_)
        | AgentEvent::TextDelta(_)
        | AgentEvent::Text(_)
        | AgentEvent::Thinking(_)
        | AgentEvent::ToolUse { .. }
            // A response of its own waits for the shell command that runs.
            if !m.shells.contains(&session) && m.own_turn.remove(&session) =>
        {
            open_own_turn(m, session);
        }
        _ => {}
    }
    let Some(&i) = m.running.get(&session).filter(|_| m.agent_runs(session)) else {
        // Nothing was waiting on this session's process, so nothing failed: the
        // next input typed here starts it again and resumes the conversation.
        if let AgentEvent::Exited { detail } = ev {
            m.notice = Some(format!(
                "claude exited in session {}: {} — its next input starts it again",
                session + 1,
                detail.lines().next().unwrap_or_default()
            ));
        }
        return;
    };
    let repo = m.fx.sessions[session].repo.clone();
    let seg = &mut m.fx.sessions[session].segments[i];
    let narration = &mut seg.sections[0].narration;
    let mut finished = false;
    match ev {
        AgentEvent::Started { model, .. } => {
            seg.model = model.clone();
            m.agent_model = Some(model);
            m.awaiting_start.remove(&session);
        }
        AgentEvent::Activity(a) => {
            m.awaiting_start.remove(&session);
            m.activity.insert(session, a);
        }
        AgentEvent::TextDelta(t) => seg.streaming.push_str(&t),
        AgentEvent::Text(t) => {
            seg.streaming.clear();
            narration.push(NarrationStep { kind: StepKind::Text, body: t });
        }
        AgentEvent::Thinking(t) => narration.push(NarrationStep { kind: StepKind::Thinking, body: t }),
        AgentEvent::ToolUse { name, target, .. } => {
            let target = target.strip_prefix(&format!("{repo}/")).unwrap_or(&target).to_string();
            let body = if target.is_empty() { name } else { format!("{name} {target}") };
            narration.push(NarrationStep { kind: StepKind::ToolCall, body });
        }
        AgentEvent::ToolResult { is_error: true, text, .. } => {
            if let Some(step) = narration.iter_mut().rev().find(|s| s.kind == StepKind::ToolCall) {
                let why: String = text.lines().next().unwrap_or_default().chars().take(140).collect();
                step.body = format!("{} — failed: {why}", step.body);
            }
        }
        AgentEvent::ToolResult { .. } => {}
        AgentEvent::Finished { reply, cost_total_usd, duration_ms, is_error, permission_denials } => {
            let stopped = m.stopping.remove(&session);
            if permission_denials > 0 {
                narration.push(NarrationStep {
                    kind: StepKind::ToolCall,
                    body: format!("{permission_denials} tool call(s) refused by the permission mode"),
                });
            }
            seg.reply = reply;
            // A stopped input reports a total of 0. Thus the total stays as
            // it was, and the next input is not charged for the earlier ones.
            let spent = m.cost_total.entry(session).or_insert(0.0);
            seg.cost_usd = (cost_total_usd - *spent).max(0.0);
            *spent = spent.max(cost_total_usd);
            seg.elapsed = fmt_duration(duration_ms);
            seg.duration_ms = duration_ms;
            // A stopped input is a usual input: its changes are real, and it
            // gets a summary.
            seg.status = if is_error && !stopped { SegStatus::Failed } else { SegStatus::Done };
            if stopped {
                let body = if m.mid_turn.get(&session).is_some_and(|q| !q.is_empty()) {
                    "■ Interrupted to read the input that you sent."
                } else {
                    "■ You stopped this input."
                };
                narration.push(NarrationStep { kind: StepKind::Text, body: body.into() });
            }
            seg.streaming.clear();
            finished = true;
        }
        AgentEvent::PermissionRequest(_)
        | AgentEvent::PermissionCancelled { .. }
        | AgentEvent::InputTaken(_)
        | AgentEvent::TaskEnded(_) => {
            unreachable!("handled above")
        }
        AgentEvent::Exited { detail } => {
            m.stopping.remove(&session);
            seg.status = SegStatus::Failed;
            let why: Vec<&str> = detail.lines().filter(|l| !l.trim().is_empty()).take(3).collect();
            seg.reply = format!("claude exited before finishing: {}", why.join(" · "));
            if let Some(t) = m.running_since.get(&session) {
                seg.elapsed = fmt_duration(m.now.saturating_duration_since(*t).as_millis() as u64);
            }
            seg.streaming.clear();
            m.notice = Some(seg.reply.clone());
            finished = true;
        }
    }
    if finished {
        input_ended(m, session, i);
    }
}

/// An input ended: compute its changes, ask for its summary, and start the
/// next input of its session.
fn input_ended(m: &mut Model, session: usize, i: usize) {
    if session != m.session {
        m.unread.insert(session);
    }
    m.running.remove(&session);
    m.running_since.remove(&session);
    m.awaiting_start.remove(&session);
    m.activity.remove(&session);
    m.pending.insert((session, i));
    m.outbox.push(Cmd::Finish { session, seg: i });
    persist(m, session, i);
    if m.summarize && wants_summary(&m.fx.sessions[session].segments[i]) {
        m.summarizing.insert((session, i));
        m.outbox.push(Cmd::Summarize { session, seg: i });
    }
    // An input sent while the agent worked, which it did not read before
    // the end, starts a response of its own. The queue waits for it.
    let unread_input = m.mid_turn.get(&session).is_some_and(|q| !q.is_empty());
    // This session's next queued input starts now. Only this session's:
    // every other one has its own agent, which either is working already or
    // has nothing waiting. Follow it only if the selection was on the input
    // that just finished or on the queue below it.
    if !unread_input && let Some(pos) = m.queued.iter().position(|q| q.session == session) {
        let q = m.queued.remove(pos);
        let follow = session == m.session && m.seg >= i;
        start_input(m, session, q.text, follow);
    }
}

/// Take prompts off the queue unanswered. If the one on screen goes, the
/// next one starts its grace period now.
fn drop_approvals(m: &mut Model, gone: impl Fn(&Approval) -> bool) {
    let front = m.approvals.first().map(|a| (a.session, a.req.request_id.clone()));
    m.approvals.retain(|a| !gone(a));
    let now = m.now;
    if let Some(next) = m.approvals.first_mut()
        && front != Some((next.session, next.req.request_id.clone()))
    {
        next.shown = now;
    }
}

/// A live input before a summarizer has sliced it: one section holding the
/// whole response.
fn is_unsectioned(s: &Segment) -> bool {
    s.sections.len() == 1 && s.sections[0].title == "response"
}

use because_core::time::{self, fmt_duration};

fn new_composer() -> TextArea<'static> {
    let mut ta = TextArea::default();
    ta.set_placeholder_text("type the next input · Enter to send");
    ta.set_placeholder_style(Style::default().fg(DIM));
    ta.set_cursor_line_style(Style::default());
    // Wraps long lines onto more rows instead of a horizontal scroll.
    ta.set_wrap_mode(WrapMode::WordOrGlyph);
    ta
}

/// The height of the composer for `width` columns: its wrapped rows and two
/// borders. It grows from three rows to eight rows, and then it scrolls.
fn composer_height(m: &Model, width: u16) -> u16 {
    let inner = width.saturating_sub(2) as usize;
    let rows: usize = m.composer.lines().iter().map(|l| wrapped_rows(l, inner)).sum();
    (rows.min(u16::MAX as usize) as u16 + 2).clamp(3, 8)
}

/// The number of screen rows that `line` uses in `width` columns. This
/// follows the `WordOrGlyph` rule of `ratatui-textarea`: break at word
/// bounds, and break a word that is wider than a row at a grapheme.
fn wrapped_rows(line: &str, width: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let width = width.max(1);
    let gw = |g: &str| if g == "\t" { 4 } else { g.width() };
    // `rows` counts the closed rows. `used` is the width of the open row.
    let mut rows = 0;
    let mut used = 0;
    for word in line.split_word_bounds() {
        let w: usize = word.graphemes(true).map(gw).sum();
        if used + w <= width {
            used += w;
            continue;
        }
        if used > 0 {
            rows += 1;
            used = 0;
            if w <= width {
                used = w;
                continue;
            }
        }
        // A word wider than a row breaks at graphemes. The last part closes
        // its row, so the next word starts on a new row.
        rows += 1;
        let mut part = 0;
        for g in word.graphemes(true) {
            let w = gw(g);
            if part > 0 && part + w > width {
                rows += 1;
                part = 0;
            }
            part += w;
        }
    }
    (rows + usize::from(used > 0)).max(1)
}

fn set_focus(m: &mut Model, i: usize) {
    let next = Focus::from_index(i);
    match (m.view, next) {
        (View::Columns, Focus::Code) => m.enter_code(),
        (View::Code, f) if f != Focus::Code => {
            m.leave_code();
            m.focus = f;
        }
        _ => m.focus = next,
    }
}

fn step(m: &mut Model, d: i32) {
    let bump = |cur: usize, len: usize| -> usize {
        if len == 0 { 0 } else { ((cur as i32 + d).rem_euclid(len as i32)) as usize }
    };
    match m.focus {
        // Stops at either end, like Segments: with history below this run's
        // sessions, wrapping from the oldest back to the top was a jump across
        // the whole list.
        Focus::Sessions => {
            let last = m.fx.sessions.len().saturating_sub(1) as i32;
            let next = (m.session as i32 + d).clamp(0, last) as usize;
            if next != m.session {
                m.session = next;
                m.seg = m.segment_count().saturating_sub(1);
                m.change = 0;
            }
        }
        // Segments are a timeline, oldest at the top: stop at either end rather
        // than jumping from the latest input to the first.
        Focus::Segments => {
            let last = m.segment_count().saturating_sub(1) as i32;
            let next = (m.seg as i32 + d).clamp(0, last) as usize;
            if next != m.seg {
                m.seg = next;
                m.change = 0;
            }
        }
        Focus::Changes => m.change = bump(m.change, m.visible_changes().len()),
        Focus::Code => {
            if let Some((lo, hi)) = m.content_range() {
                m.code_cursor = (m.code_cursor as i64 + d as i64).clamp(lo as i64, hi as i64) as u32;
            }
        }
    }
    m.clamp();
}

fn jump(m: &mut Model, d: i32, any: bool) {
    let Some(c) = m.current_change() else { return };
    let h = m.highlight(c);
    let mut marks: BTreeSet<u32> = run_starts(h.selected.iter().copied()).into_iter().collect();
    marks.extend(h.deletions.keys().copied());
    if any {
        marks.extend(run_starts(h.earlier.keys().copied()));
    }
    let marks: Vec<u32> = marks.into_iter().collect();
    if marks.is_empty() {
        return;
    }
    let cur = m.code_cursor;
    m.code_cursor = if d > 0 {
        marks.iter().copied().find(|&l| l > cur).unwrap_or(marks[0])
    } else {
        marks.iter().rev().copied().find(|&l| l < cur).unwrap_or(marks[marks.len() - 1])
    };
}

// ── view ───────────────────────────────────────────────────────────────────

pub fn view(m: &Model, f: &mut Frame) {
    let area = f.area();
    let [body, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    match m.view {
        View::Columns => columns(m, f, body),
        View::Code => code_pane(m, f, body),
        View::Reader => reader(m, f, body),
    }
    f.render_widget(status_line(m), status);
    if m.transcript {
        transcript(m, f, area);
    }
    if let Some(a) = m.approval() {
        if a.req.questions.is_empty() {
            approval(m, a, f, body);
        } else {
            question(m, a, f, body);
        }
    }
}

/// The approval prompt: which session, which tool, the call in full, and the
/// keys that answer it. Over everything else, since the agent is stopped
/// until it is answered.
fn approval(m: &Model, a: &Approval, f: &mut Frame, area: Rect) {
    let w = area.width.saturating_sub(8).clamp(20, 100);
    let inner = w.saturating_sub(4) as usize;
    let dim = Style::default().fg(DIM);
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{} ", a.req.tool), Style::default().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(ellipsize(&a.req.target, inner.saturating_sub(a.req.tool.chars().count() + 1)), Style::default().fg(SOFT)),
    ])];
    lines.push(Line::default());
    // Verbatim, line for line: this is the command that will run, and a
    // reflowed one would not be.
    for l in a.req.detail.lines() {
        let mut rest: Vec<char> = l.chars().collect();
        if rest.is_empty() {
            lines.push(Line::default());
        }
        while !rest.is_empty() {
            let take = rest.len().min(inner);
            lines.push(Line::from(Span::styled(rest.drain(..take).collect::<String>(), Style::default().fg(FG))));
        }
    }
    lines.push(Line::default());
    let ready = m.approval_ready();
    let key = |k: &str| Span::styled(k.to_string(), Style::default().fg(if ready { HUMAN } else { DIM }).add_modifier(Modifier::BOLD));
    lines.push(Line::from(vec![key("a"), Span::styled(" allow once", dim)]));
    if let Some(rule) = &a.req.always {
        lines.push(Line::from(vec![
            key("A"),
            Span::styled(" allow for this session: ", dim),
            Span::styled(ellipsize(rule, inner.saturating_sub(26)), Style::default().fg(SOFT)),
        ]));
    }
    lines.push(Line::from(vec![key("m"), Span::styled(" allow, and switch this session to auto mode", dim)]));
    lines.push(Line::from(vec![key("d"), Span::styled(" deny — the agent is told, and carries on without it", dim)]));
    prompt_box(m, a, f, area, w, lines, "approve?".into());
}

/// The question prompt: the question on screen, its options, and "Other" for
/// an answer that is not an option. One question at a time, in order.
fn question(m: &Model, a: &Approval, f: &mut Frame, area: Rect) {
    let w = area.width.saturating_sub(8).clamp(20, 100);
    let inner = w.saturating_sub(4) as usize;
    let dim = Style::default().fg(DIM);
    let p = &a.pick;
    let Some(q) = a.req.questions.get(p.at) else { return };
    let mut lines = Vec::new();
    if !q.header.is_empty() {
        lines.push(Line::from(Span::styled(q.header.clone(), Style::default().fg(HUMAN).add_modifier(Modifier::BOLD))));
    }
    for l in wrap(&q.question, inner) {
        lines.push(Line::from(Span::styled(l, Style::default().fg(FG).add_modifier(Modifier::BOLD))));
    }
    if q.multi {
        lines.push(Line::from(Span::styled("Select one or more.", dim)));
    }
    lines.push(Line::default());
    let ready = m.approval_ready();
    let typing = p.other.is_some();
    let row = |i: usize, label: &str, chosen: bool| {
        let here = i == p.cursor && !typing;
        let mark = match (q.multi, chosen) {
            (true, true) => "[x] ",
            (true, false) => "[ ] ",
            (false, _) => "",
        };
        let style = if here { Style::default().fg(FG).bg(SEL).add_modifier(Modifier::BOLD) } else { Style::default().fg(FG) };
        Line::from(vec![
            Span::styled(if here { "› " } else { "  " }, Style::default().fg(HUMAN)),
            Span::styled(format!("{}. ", i + 1), Style::default().fg(if ready { HUMAN } else { DIM }).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{mark}{}", ellipsize(label, inner.saturating_sub(9))), style),
        ])
    };
    for (i, o) in q.options.iter().enumerate() {
        lines.push(row(i, &o.label, p.chosen.get(i).copied().unwrap_or(false)));
        // Some agents repeat the label as the description.
        let about = if o.description == o.label { "" } else { &o.description };
        for l in wrap(about, inner.saturating_sub(5)) {
            lines.push(Line::from(Span::styled(format!("     {l}"), dim)));
        }
    }
    let n = q.options.len();
    match &p.other {
        // The text field, with a block for the cursor.
        Some(t) => lines.push(Line::from(vec![
            Span::styled("› ", Style::default().fg(HUMAN)),
            Span::styled(format!("{}. ", n + 1), Style::default().fg(HUMAN).add_modifier(Modifier::BOLD)),
            Span::styled(ellipsize(t, inner.saturating_sub(7)), Style::default().fg(FG)),
            Span::styled("█", Style::default().fg(HUMAN)),
        ])),
        None => lines.push(row(n, "Other: type an answer", false)),
    }
    lines.push(Line::default());
    let esc = if typing { "Esc returns to the options" } else { "Esc declines — the agent is told, and carries on without the answers" };
    lines.push(Line::from(Span::styled(esc, dim)));
    let count = a.req.questions.len();
    let title = if count > 1 { format!("question {} of {count}", p.at + 1) } else { "question".into() };
    prompt_box(m, a, f, area, w, lines, title);
}

/// The frame of a prompt: centered over the body, with the session in the
/// title and the number of prompts that wait after it.
fn prompt_box(m: &Model, a: &Approval, f: &mut Frame, area: Rect, w: u16, lines: Vec<Line>, what: String) {
    let dim = Style::default().fg(DIM);
    let more = m.approvals.len() - 1;
    let bottom = if more > 0 { format!(" {more} more waiting ") } else { String::new() };
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = Rect::new(area.x + (area.width.saturating_sub(w)) / 2, area.y + (area.height.saturating_sub(h)) / 2, w.min(area.width), h);
    let (name, _) = session_name(m, a.session);
    let room = (w as usize).saturating_sub(22 + what.chars().count());
    let title = format!(" session {} · {} — {what} ", a.session + 1, ellipsize(&name, room));
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .padding(Padding::horizontal(1))
                .border_style(Style::default().fg(HUMAN))
                .title(Span::styled(title, Style::default().fg(FG).add_modifier(Modifier::BOLD)))
                .title_bottom(Span::styled(bottom, dim).into_right_aligned_line()),
        ),
        r,
    );
}

fn pane<'a>(title: &'a str, focused: bool, n: usize) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { GENERATED } else { FRAME }))
        .title(Span::styled(
            format!(" {n} {title} "),
            Style::default()
                .fg(if focused { FG } else { DIM })
                .add_modifier(if focused { Modifier::BOLD } else { Modifier::empty() }),
        ))
}

/// Which of the three columns are on screen, and how wide, for a body
/// `width` cells wide with `focus` where it is.
///
/// Segments is the column to keep readable: it is what the screen is for, and
/// it wraps where the others cut. So as the terminal narrows — a smaller
/// window, or a larger font — Changes gives up width before Segments does,
/// then the columns go two at a time, following the focus, and below that
/// only the focused column is shown. The numbers in the titles stay, so
/// `1`–`3` and `h`/`l` still reach a column that is off screen.
pub fn column_layout(width: u16, focus: Focus) -> Vec<(Focus, Constraint)> {
    use Constraint::{Fill, Length, Percentage};
    if width >= 140 {
        vec![(Focus::Sessions, Percentage(22)), (Focus::Segments, Percentage(40)), (Focus::Changes, Percentage(38))]
    } else if width >= 100 {
        vec![(Focus::Sessions, Length(26)), (Focus::Segments, Fill(55)), (Focus::Changes, Fill(45))]
    } else if width >= 60 {
        if focus == Focus::Sessions {
            // Sessions has the focus, so its names get more than the 26 they
            // are cut to beside two other columns.
            vec![(Focus::Sessions, Length((width * 2 / 5).max(26))), (Focus::Segments, Fill(1))]
        } else {
            vec![(Focus::Segments, Fill(55)), (Focus::Changes, Fill(45))]
        }
    } else {
        let only = if matches!(focus, Focus::Sessions | Focus::Changes) { focus } else { Focus::Segments };
        vec![(only, Fill(1))]
    }
}

fn columns(m: &Model, f: &mut Frame, area: Rect) {
    let cols = column_layout(area.width, m.focus);
    let areas = Layout::horizontal(cols.iter().map(|&(_, c)| c)).split(area);
    for (&(col, _), &r) in cols.iter().zip(areas.iter()) {
        match col {
            Focus::Sessions => sessions_column(m, f, r),
            Focus::Segments => segments_column(m, f, r),
            _ => changes_column(m, f, r),
        }
    }
}

fn sessions_column(m: &Model, f: &mut Frame, a: Rect) {
    // The allowance sits at the foot of the column rather than at the end of
    // the list, so it stays put as sessions are added and the list grows past
    // the bottom of the pane.
    let block = pane("Sessions", m.focus == Focus::Sessions, 1);
    let inner = block.inner(a);
    f.render_widget(block, a);
    let bar = usage_panel(m, inner.width as usize);
    let [list_a, usage_a] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(bar.len() as u16)]).areas(inner);
    let (list, sel_line, total) = sessions(m, inner.width);
    // Keep the selected session's name and meta line in view; with history,
    // the list is longer than the column.
    let visible = list_a.height as usize;
    let offset = if sel_line + 3 > visible { (sel_line + 3 - visible).min(total.saturating_sub(visible)) } else { 0 };
    f.render_widget(list.scroll((offset as u16, 0)), list_a);
    if !bar.is_empty() {
        f.render_widget(Paragraph::new(bar), usage_a);
    }
}

fn segments_column(m: &Model, f: &mut Frame, b: Rect) {
    // Where the inputs on show came from, said once, where there is room.
    let seg_title = match (m.session().origin, m.verbatim) {
        (Origin::Imported, _) => "Segments · from Claude Code",
        (Origin::Stored, _) => "Segments · earlier session",
        (Origin::New, true) => "Segments · verbatim",
        (Origin::New, false) => "Segments",
    };
    let composer_h = composer_height(m, b.width);
    let [list_area, composer_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(composer_h)]).areas(b);
    let focused = m.focus == Focus::Segments;
    let (list, sel_line, total) = segments(m, b.width.saturating_sub(2));
    // Prefer showing the tail (the latest input sits above the composer), but
    // never scroll the selected segment's first line off the top.
    let visible = list_area.height.saturating_sub(1) as usize;
    let offset = total.saturating_sub(visible).min(sel_line) as u16;
    f.render_widget(
        list.scroll((offset, 0)).block(
            pane(seg_title, focused && m.seg_focus == SegFocus::List, 2)
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(if focused { GENERATED } else { FRAME })),
        ),
        list_area,
    );
    composer(m, f, composer_area);
}

fn changes_column(m: &Model, f: &mut Frame, c: Rect) {
    f.render_widget(changes(m, c.width.saturating_sub(4)).block(pane("Changes", m.focus == Focus::Changes, 3)), c);
}

/// What the first column calls a session, and the colour that says where the
/// name came from — `None` for a name that was neither typed nor generated.
///
/// A session started here is unnamed until something is typed in it. The first
/// input names it, verbatim and in the human colour; once the summarizer has
/// written that input's summary the name becomes the summary, which is the
/// first account of the session that says what happened rather than what was
/// asked. A session that came with a title keeps it.
pub fn session_name(m: &Model, i: usize) -> (String, Option<Color>) {
    let s = &m.fx.sessions[i];
    if !s.title.is_empty() {
        // An imported session's title was written by Claude Code's model.
        let from = (s.origin == Origin::Imported).then_some(GENERATED);
        return (s.title.clone(), from);
    }
    if let Some(seg) = s.segments.first() {
        return if seg.summary.is_empty() {
            (first_line(&seg.input), Some(HUMAN))
        } else {
            (first_line(&seg.summary), Some(GENERATED))
        };
    }
    // Typed but not sent yet: the input is waiting for the agent, and the
    // session it belongs to is still the one being named.
    match m.queued.iter().find(|q| q.session == i) {
        Some(q) => (first_line(&q.text), Some(HUMAN)),
        None => ("new session".to_string(), None),
    }
}

/// The first line with anything on it, for a name that has one line to fit in.
fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_string()
}

/// Returns the list, the line the selected session starts on, and the total
/// line count, so the caller can keep the selection on screen.
///
/// This run's sessions come first, then — under a rule — history, newest
/// first: sessions from earlier runs and ones imported from Claude Code.
fn sessions(m: &Model, width: u16) -> (Paragraph<'static>, usize, usize) {
    let mut lines = Vec::new();
    let mut sel_line = 0;
    let w = width as usize;
    let new_session_line = |lines: &mut Vec<Line<'static>>| {
        lines.push(Line::from(vec![
            Span::styled("  + ", Style::default().fg(GENERATED)),
            Span::styled("n new session", Style::default().fg(DIM)),
        ]));
        lines.push(Line::from(""));
    };
    let mut in_history = false;
    for (i, s) in m.fx.sessions.iter().enumerate() {
        if s.origin.is_history() && !in_history {
            in_history = true;
            new_session_line(&mut lines);
            let fill = w.saturating_sub(11);
            lines.push(Line::from(Span::styled(format!("── earlier {}", "─".repeat(fill)), Style::default().fg(DIM))));
        }
        let on = i == m.session;
        if on {
            sel_line = lines.len();
        }
        let queued = m.queued.iter().filter(|q| q.session == i).count();
        let (name, from) = session_name(m, i);
        let title = ellipsize(&name, w.saturating_sub(2));
        lines.push(Line::from(vec![
            Span::styled(if on { "▸ " } else { "  " }, Style::default().fg(GENERATED)),
            Span::styled(
                title,
                if on {
                    Style::default().fg(from.unwrap_or(FG)).bg(SEL).add_modifier(Modifier::BOLD)
                } else if m.unread.contains(&i) {
                    Style::default().fg(from.unwrap_or(FG)).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(from.unwrap_or(SOFT))
                },
            ),
        ]));
        let n = s.segments.len() + queued;
        let cost = if s.recorded_cost.is_none() && s.segments.iter().any(|x| x.cost_unrecorded) {
            "$—".to_string()
        } else {
            format!("${:.2}", s.cost())
        };
        // History leads with its date: in a narrow column the end of this
        // line is what gets cut.
        let date = if s.origin.is_history() { format!("{} · ", time::short_date(&s.started_at)) } else { String::new() };
        let mut meta = format!("  {date}{n} input{} · {cost}", if n == 1 { "" } else { "s" });
        if queued > 0 {
            meta.push_str(&format!(" · {queued} queued"));
        }
        // Files not committed in the tree this session works in. Marked on the
        // first session listed for that tree only: several can share one —
        // history from the repository's own checkout, a session not sent yet —
        // and the count is the tree's, not each session's.
        let dirty = match m.uncommitted.get(&s.repo) {
            Some(&n) if n > 0 && !m.fx.sessions[..i].iter().any(|x| x.repo == s.repo) => format!(" ±{n}"),
            _ => String::new(),
        };
        // Which session the agent is in, so work in a session you are not
        // looking at is still visible. The cells these need come off the meta
        // line, which is the one thing here that can be cut without losing a
        // name.
        let mut marks = dirty.chars().count() + 2;
        let unread = m.unread.contains(&i);
        if unread {
            marks += 2;
        }
        let mut meta = vec![Span::styled(ellipsize(&meta, w.saturating_sub(marks)), Style::default().fg(DIM))];
        if !dirty.is_empty() {
            meta.push(Span::styled(dirty, Style::default().fg(UNCOMMITTED)));
        }
        // A reply written while you were elsewhere.
        if unread {
            meta.push(Span::styled(" ◆", Style::default().fg(GENERATED).add_modifier(Modifier::BOLD)));
        }
        if m.waiting_on(i).is_some() {
            meta.push(Span::styled(" ?", Style::default().fg(HUMAN).add_modifier(Modifier::BOLD)));
        } else if m.is_running(i) {
            meta.push(Span::styled(" ⟳", Style::default().fg(GENERATED).add_modifier(Modifier::BOLD)));
        }
        lines.push(Line::from(meta));
        lines.push(Line::from(""));
    }
    if !in_history {
        new_session_line(&mut lines);
    }
    let repo = m.session().repo.rsplit('/').next().unwrap_or_default().to_string();
    lines.push(Line::from(Span::styled(
        ellipsize(&format!("  {repo} · {}", m.session().branch), w),
        Style::default().fg(DIM),
    )));
    // Every tree of the repository, including ones no session here worked in.
    let trees = m.uncommitted.values().filter(|&&n| n > 0).count();
    if trees > 0 {
        lines.push(Line::from(Span::styled(
            ellipsize(&format!("  ± {trees} tree{} not committed", if trees == 1 { "" } else { "s" }), w),
            Style::default().fg(UNCOMMITTED),
        )));
    }
    let total = lines.len();
    (Paragraph::new(lines), sel_line, total)
}

/// Widest label the windows use, so the bars line up under each other.
const USAGE_LABEL_W: usize = 4;

/// Colour by how much is left: the bar should be readable without the number.
/// It fades green → amber → red as the window drains, so the colour moves with
/// every reading instead of jumping only at a couple of thresholds.
fn usage_color(left_pct: f64) -> Color {
    // (percent left, colour), highest first. Below the last stop it stays red.
    const STOPS: [(f64, (u8, u8, u8)); 4] = [
        (100.0, (0x7f, 0xb3, 0x7f)), // ADDED
        (60.0, (0xb0, 0xb8, 0x6a)),
        (30.0, (0xd0, 0xa8, 0x60)),
        (10.0, (0xc8, 0x7f, 0x7f)), // REMOVED
    ];
    let p = left_pct.clamp(0.0, 100.0);
    let rgb = |(r, g, b): (u8, u8, u8)| Color::Rgb(r, g, b);
    for pair in STOPS.windows(2) {
        let ((hi, a), (lo, b)) = (pair[0], pair[1]);
        if p >= lo {
            let t = (p - lo) / (hi - lo);
            let mix = |x: u8, y: u8| (y as f64 + (x as f64 - y as f64) * t).round() as u8;
            return rgb((mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2)));
        }
    }
    rgb(STOPS[STOPS.len() - 1].1)
}

/// The allowance at the foot of the Sessions column: one bar per window the
/// endpoint reports, showing what is **left** rather than what is spent, since
/// that is the number you act on.
///
/// Empty — and so taking no rows — outside a live session: the fixture view
/// does no I/O, and an allowance nothing here is spending would be noise.
fn usage_panel(m: &Model, width: usize) -> Vec<Line<'static>> {
    if !m.live {
        return Vec::new();
    }
    let dim = Style::default().fg(DIM);
    let head = |label: &str| {
        let fill = width.saturating_sub(label.chars().count() + 4);
        Line::from(Span::styled(format!("── {label} {}", "─".repeat(fill)), dim))
    };
    let Some(u) = &m.usage else {
        // Two cells of a 24-wide column cannot hold why. The reason went to the
        // status line when it first failed, and to the log every time.
        let body = if m.usage_error.is_some() { "  unavailable" } else { "  checking…" };
        return vec![head("usage"), Line::from(Span::styled(body, dim))];
    };
    // "  5h   " + bar + " 100%" — the bar takes whatever the fixed parts leave.
    let bar_w = width.saturating_sub(USAGE_LABEL_W + 9).clamp(4, 16);
    let mut lines = vec![head(if m.usage_error.is_some() { "usage left · stale" } else { "usage left" })];
    for w in &u.windows {
        let left = w.left_pct();
        let filled = ((left / 100.0) * bar_w as f64).round() as usize;
        let color = usage_color(left);
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<USAGE_LABEL_W$} ", w.label), dim),
            Span::styled("█".repeat(filled.min(bar_w)), Style::default().fg(color)),
            Span::styled("░".repeat(bar_w.saturating_sub(filled)), Style::default().fg(FRAME)),
            Span::styled(format!(" {:>3}%", left.round()), Style::default().fg(color)),
        ]));
    }
    lines
}

fn narration_lines(sec: &Section, indent: &str, width: usize, lines: &mut Vec<Line<'static>>) {
    for step in &sec.narration {
        let (label, style) = match step.kind {
            StepKind::Thinking => ("❝ thinking ", Style::default().fg(THINKING)),
            StepKind::Text => ("❝ said     ", Style::default().fg(FG)),
            StepKind::ToolCall => ("  ⟩ ran    ", Style::default().fg(DIM)),
        };
        for (i, chunk) in wrap(&step.body, width.saturating_sub(indent.len() + 11)).into_iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(indent.to_string(), Style::default()),
                Span::styled(if i == 0 { label } else { "           " }, Style::default().fg(DIM)),
                Span::styled(chunk, style),
            ]));
        }
    }
}

/// Lines of a long input shown in the column when it is selected.
const INPUT_LINES_SELECTED: usize = 4;
/// Tool calls listed under the selected input before the rest are counted.
const TOOLS_SHOWN: usize = 4;

/// Returns the list, the line the selected segment starts on, and the total
/// line count, so the caller can scroll without losing the selection.
fn segments(m: &Model, width: u16) -> (Paragraph<'static>, usize, usize) {
    let w = width as usize;
    // Room left after the fixed-width prefixes: "▸ 12 › " is 7 cells, the
    // summary's "       ↳ " is 9. Wrapping to anything wider clips the last
    // character against the border.
    let body = w.saturating_sub(7);
    let summary_w = w.saturating_sub(9);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut sel_line = 0;
    if m.segments().is_empty() && m.queued_for_session().next().is_none() {
        let text = if m.live {
            format!("No inputs yet. Type below and press Enter — Claude works in {}.", m.session().repo)
        } else {
            "No inputs in this session.".to_string()
        };
        for chunk in wrap(&text, w.saturating_sub(4)) {
            lines.push(Line::from(Span::styled(format!("  {chunk}"), Style::default().fg(DIM))));
        }
    }
    for (i, s) in m.segments().iter().enumerate() {
        let on = i == m.seg;
        if on {
            sel_line = lines.len();
        }
        // The input: what the developer typed. Four lines when selected, two
        // otherwise; the reader has all of it.
        let input: Vec<String> = s.input.lines().flat_map(|l| wrap(l, body)).collect();
        let shown = input.len().min(if on { INPUT_LINES_SELECTED } else { 2 });
        for (k, chunk) in input.iter().take(shown).enumerate() {
            let last_cut = k + 1 == shown && input.len() > shown;
            let text = if last_cut { ellipsize(&input[k..].join(" "), body) } else { chunk.clone() };
            let lead = match (k, on) {
                (0, true) => format!("▸ {:>2} › ", s.seq),
                (0, false) => format!("  {:>2} › ", s.seq),
                _ => "       ".to_string(),
            };
            lines.push(Line::from(vec![
                Span::styled(lead, Style::default().fg(if on { GENERATED } else { DIM })),
                Span::styled(
                    text,
                    Style::default().fg(HUMAN).add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }),
                ),
            ]));
        }
        // What the agent did: live progress, a generated summary, or — until
        // there is a summarizer — the agent's own reply, marked as verbatim.
        let tools = s.sections.iter().flat_map(|x| &x.narration).filter(|n| n.kind == StepKind::ToolCall);
        if s.status == SegStatus::Running {
            let secs = m.running_since.get(&m.session).map(|t| m.now.saturating_duration_since(*t).as_secs()).unwrap_or(0);
            let waiting = m.awaiting_start.contains(&m.session);
            let status = if s.is_shell() {
                format!("⟳ running the command · {secs}s")
            } else if let Some(r) = m.waiting_on(m.session).filter(|r| !r.questions.is_empty()) {
                format!("? waiting for your answer · {}", r.target)
            } else if let Some(r) = m.waiting_on(m.session) {
                format!("? waiting for your approval · {} {}", r.tool, r.target)
            } else if waiting && secs >= 20 {
                format!("⟳ no response from claude after {secs}s — the log may say why")
            } else if waiting {
                format!("⟳ starting claude · {secs}s")
            } else {
                format!(
                    "⟳ {} · {secs}s · {} tool call(s)",
                    m.activity.get(&m.session).map(String::as_str).unwrap_or("working"),
                    tools.clone().count()
                )
            };
            lines.push(Line::from(Span::styled(
                ellipsize(&format!("       {status}"), w),
                Style::default().fg(GENERATED).add_modifier(Modifier::BOLD),
            )));
            if waiting && secs >= 20
                && let Some(log) = &m.log_path
            {
                lines.push(Line::from(Span::styled(ellipsize(&format!("         log: {log}"), w), Style::default().fg(DIM))));
            }
            let last_text = s.sections.iter().flat_map(|x| &x.narration).rev().find(|n| n.kind == StepKind::Text);
            let live = if s.streaming.trim().is_empty() { last_text.map(|n| n.body.as_str()).unwrap_or("") } else { &s.streaming };
            let tail: Vec<String> = live.lines().flat_map(|l| wrap(l, summary_w)).collect();
            for (k, chunk) in tail.iter().skip(tail.len().saturating_sub(3)).enumerate() {
                lines.push(Line::from(vec![
                    Span::styled(if k == 0 { "       ❝ " } else { "         " }, Style::default().fg(DIM)),
                    Span::styled(chunk.clone(), Style::default().fg(FG)),
                ]));
            }
            if let Some(t) = tools.clone().next_back() {
                lines.push(Line::from(Span::styled(
                    ellipsize(&format!("       ⟩ {}", t.body), w),
                    Style::default().fg(DIM),
                )));
            }
        } else {
            // A generated summary when there is one. Otherwise the start of the
            // agent's own reply, marked verbatim and cut to two lines: the
            // column is for scanning, and the reader is for reading.
            let (mark, text, style, cap) = if !s.summary.is_empty() {
                ("       ↳ ", s.summary.clone(), Style::default().fg(GENERATED), 3)
            } else if m.summarizing.contains(&(m.session, i)) {
                ("       ↳ ", "summarizing…".to_string(), Style::default().fg(GENERATED).add_modifier(Modifier::DIM), 1)
            } else if s.is_shell() {
                // Output of a command: not what an agent said.
                ("       ⟩ ", s.reply.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "), Style::default().fg(SOFT), 2)
            } else {
                let reply = if s.reply.trim().is_empty() { s.said() } else { s.reply.clone() };
                ("       ❝ ", plain_preview(&reply), Style::default().fg(FG), 2)
            };
            let all = wrap(&text, summary_w);
            let n = all.len().min(if on { cap } else { 1 });
            for (k, chunk) in all.iter().take(n).enumerate() {
                let chunk = if k + 1 == n && all.len() > n { ellipsize(&all[k..].join(" "), summary_w) } else { chunk.clone() };
                lines.push(Line::from(vec![
                    Span::styled(if k == 0 { mark } else { "         " }, style),
                    Span::styled(chunk, style),
                ]));
            }
        }
        let pending = m.pending.contains(&(m.session, i));
        let meta = match &s.commit {
            _ if s.status == SegStatus::Running => String::new(),
            _ if pending => format!("         computing changes… · {} · {}", s.elapsed, cost_label(s)),
            _ if s.status == SegStatus::Failed => format!("         failed · {} · {}", s.elapsed, cost_label(s)),
            Some(_) if s.changes.is_empty() => format!("         no file changes · {} · {}", s.elapsed, cost_label(s)),
            Some(c) => format!(
                "         {} file{} · {} · {} · {}{}",
                s.changes.len(),
                if s.changes.len() == 1 { "" } else { "s" },
                s.elapsed,
                cost_label(s),
                c.short,
                // A commit matched by when it was made, not seen being made.
                if s.link == "commit_time" { " by time" } else { "" }
            ),
            None => format!("         no file changes · {} · {}", s.elapsed, cost_label(s)),
        };
        if !meta.is_empty() {
            lines.push(Line::from(Span::styled(ellipsize(&meta, w), Style::default().fg(DIM))));
            // The selected input says how much more there is, and how to read
            // it when Enter would open it from here.
            let words = word_count(&s.said());
            if on && (words > SHORT_REPLY_WORDS || input.len() > shown) {
                let enter_reads = m.focus == Focus::Segments && (m.seg_focus == SegFocus::List || m.draft_is_empty());
                let how = if enter_reads { "Enter reads" } else { "r reads" };
                let what = if words > SHORT_REPLY_WORDS { format!("all {} words", fmt_count(words)) } else { "it in full".into() };
                lines.push(Line::from(Span::styled(ellipsize(&format!("         {how} {what}"), w), Style::default().fg(DIM))));
            }
        }
        // Sections: the response to this input, broken down.
        let unsectioned = is_unsectioned(s);
        if on && unsectioned && !m.verbatim && s.status != SegStatus::Running {
            let n = tools.clone().count();
            let cap = if n > TOOLS_SHOWN + 1 { TOOLS_SHOWN } else { n };
            for t in tools.clone().take(cap) {
                lines.push(Line::from(Span::styled(
                    ellipsize(&format!("         ┊ ⟩ {}", t.body), w),
                    Style::default().fg(DIM),
                )));
            }
            if n > cap {
                lines.push(Line::from(Span::styled(
                    format!("         ┊ … and {} more tool calls", n - cap),
                    Style::default().fg(DIM),
                )));
            }
        } else if on && (!s.changes.is_empty() || m.verbatim) {
            for sec in &s.sections {
                lines.push(Line::from(vec![
                    Span::styled(format!("         ┊ {} ", sec.seq + 1), Style::default().fg(DIM)),
                    Span::styled(
                        // 13 = width of the "         ┊ N " prefix
                        ellipsize(&sec.title, w.saturating_sub(13 + 1)),
                        Style::default().fg(GENERATED),
                    ),
                ]));
                if m.verbatim {
                    narration_lines(sec, "           ", w, &mut lines);
                }
            }
        }
        lines.push(Line::from(""));
    }
    let ran = m.segments().len();
    for (qi, q) in m.queued_for_session().enumerate() {
        let on = m.seg == ran + qi;
        if on {
            sel_line = lines.len();
        }
        let seq = ran + qi + 1;
        for (k, chunk) in q.text.lines().flat_map(|l| wrap(l, body)).enumerate() {
            let lead = match (k, on) {
                (0, true) => format!("▸ {seq:>2} › "),
                (0, false) => format!("  {seq:>2} › "),
                _ => "       ".to_string(),
            };
            lines.push(Line::from(vec![
                Span::styled(lead, Style::default().fg(if on { GENERATED } else { DIM })),
                Span::styled(
                    chunk,
                    Style::default().fg(HUMAN).add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }),
                ),
            ]));
        }
        lines.push(Line::from(Span::styled(
            if m.live {
                "         queued · runs when the agent is idle"
            } else {
                "         not sent · no agent in this view (run `cuz`)"
            },
            Style::default().fg(DIM),
        )));
        lines.push(Line::from(""));
    }
    let total = lines.len();
    (Paragraph::new(lines), sel_line, total)
}

fn composer(m: &Model, f: &mut Frame, area: Rect) {
    let focused = m.focus == Focus::Segments || m.view == View::Reader;
    let typing = m.composing();
    let mut ta = m.composer.clone();
    let history = m.session().origin.is_history();
    let title = if history {
        ta.set_placeholder_text(if m.reviving.contains(&m.session().id) {
            "reviving: asking claude to resume this session …"
        } else {
            "an earlier session: read only · Esc c revives it · n starts a new one"
        });
        Span::styled(" ✎ earlier session · read only ", Style::default().fg(DIM))
    } else if typing {
        Span::styled(" ✎ next input ", Style::default().fg(HUMAN).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" ✎ next input · i to type ", Style::default().fg(DIM))
    };
    // Joins onto the list above it instead of drawing a second box.
    let set = border::Set { top_left: "├", top_right: "┤", ..border::PLAIN };
    ta.set_block(
        Block::default()
            .borders(Borders::ALL)
            .border_set(set)
            .border_style(Style::default().fg(if focused { GENERATED } else { FRAME }))
            .title(title),
    );
    ta.set_style(Style::default().fg(if typing { HUMAN } else { DIM }));
    ta.set_cursor_style(if typing { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() });
    f.render_widget(&ta, area);
}

fn changes(m: &Model, width: u16) -> Paragraph<'static> {
    let w = width as usize;
    let mut lines = Vec::new();
    let note = |lines: &mut Vec<Line<'static>>, head: &str, body: &str| {
        lines.push(Line::from(Span::styled(format!("  {head}"), Style::default().fg(SOFT))));
        for chunk in wrap(body, w.saturating_sub(2)) {
            lines.push(Line::from(Span::styled(format!("  {chunk}"), Style::default().fg(DIM))));
        }
    };
    if m.selected_queued().is_some() {
        let body = if m.live {
            "Nothing has run yet, so there are no changes to show. When the agent takes this input, the files it touches appear here."
        } else {
            "This view shows sample data and has no agent attached, so this input will not run. `cuz`, inside a git repository, sends inputs to Claude."
        };
        note(&mut lines, "✎ queued input", body);
        return Paragraph::new(lines);
    }
    let Some(seg) = m.selected_segment() else { return Paragraph::new(lines) };
    if seg.status == SegStatus::Running {
        note(&mut lines, "⟳ the agent is working", "Changed files appear here when this input finishes.");
        return Paragraph::new(lines);
    }
    if m.pending.contains(&(m.session, m.seg)) {
        note(&mut lines, "computing changes…", "Snapshotting the working tree and diffing it against the state before this input.");
        return Paragraph::new(lines);
    }
    if seg.changes.is_empty() && seg.status == SegStatus::Failed {
        note(
            &mut lines,
            "stopped before finishing",
            "The agent stopped before it finished this input, and it had changed no files. The reason is under the input.",
        );
        return Paragraph::new(lines);
    }
    if seg.changes.is_empty() {
        let next = match m.nearest_input_with_changes() {
            Some(s) => format!(" There is no code to open for it; input {} changed files.", s.seq),
            None => String::new(),
        };
        let what = if seg.is_shell() {
            "This command changed no files. Enter reads its output in full."
        } else {
            "This input was answered in conversation. Enter reads the answer in full."
        };
        note(&mut lines, "no file changes", &format!("{what}{next}"));
        return Paragraph::new(lines);
    }
    if seg.link == "commit_time" {
        let short = seg.commit.as_ref().map(|c| c.short.as_str()).unwrap_or_default();
        for chunk in wrap(
            &format!(
                "Matched by time: what changed in the commits made while this input ran, up to {short}. \
                 A commit made by hand meanwhile would be counted too."
            ),
            w.saturating_sub(2),
        ) {
            lines.push(Line::from(Span::styled(format!("  {chunk}"), Style::default().fg(DIM))));
        }
        lines.push(Line::from(""));
    }
    for (i, c) in seg.changes.iter().enumerate() {
        let on = i == m.change;
        let rc = risk_color(c.risk);
        lines.push(Line::from(vec![
            Span::styled(if on { "▸ " } else { "  " }, Style::default().fg(GENERATED)),
            Span::styled(format!("{} ", c.risk.glyph()), Style::default().fg(rc)),
            Span::styled(
                ellipsize(&c.path, w.saturating_sub(4)),
                Style::default()
                    .fg(if on { FG } else { SOFT })
                    .add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }),
            ),
        ]));
        if c.summary.is_empty() {
            lines.push(Line::from(Span::styled("    not summarized yet", Style::default().fg(DIM))));
        }
        for chunk in wrap(&c.summary, w.saturating_sub(4)) {
            lines.push(Line::from(Span::styled(format!("    {chunk}"), Style::default().fg(GENERATED))));
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!("    {} hunk{}   ", c.hunks.len(), if c.hunks.len() == 1 { "" } else { "s" }),
                Style::default().fg(DIM),
            ),
            Span::styled(format!("+{}", c.additions), Style::default().fg(ADDED)),
            Span::styled(format!(" −{}", c.deletions), Style::default().fg(REMOVED)),
            if c.state == "unsummarized" {
                Span::styled("   risk not assessed", Style::default().fg(DIM))
            } else {
                Span::styled(format!("   {}", c.risk.label()), Style::default().fg(rc))
            },
        ]));
        if on && seg.code_unloaded {
            lines.push(Line::from(Span::styled("      loading the code from git…", Style::default().fg(DIM))));
        }
        if on {
            for h in &c.hunks {
                let text = if h.summary.is_empty() {
                    format!("      ▸ @@ -{},{} +{},{} {}", h.old_start, h.old_lines, h.new_start, h.new_lines, h.header)
                } else {
                    format!("      ▸ @@{} — {}", h.header, h.summary)
                };
                lines.push(Line::from(Span::styled(
                    ellipsize(&text, w),
                    Style::default().fg(Color::Rgb(0x8a, 0x90, 0x98)),
                )));
            }
        }
        lines.push(Line::from(""));
    }
    Paragraph::new(lines)
}

fn code_pane(m: &Model, f: &mut Frame, area: Rect) {
    let (Some(seg), Some(c)) = (m.selected_segment(), m.current_change()) else { return };
    let (body, rat) = if m.rationale {
        let [b, r] = Layout::vertical([Constraint::Min(8), Constraint::Percentage(45)]).areas(area);
        (b, Some(r))
    } else {
        (area, None)
    };
    let [head, code] = Layout::vertical([Constraint::Length(4), Constraint::Min(1)]).areas(body);
    f.render_widget(sticky_header(m, seg, c, head.width), head);

    let h = m.highlight(c);
    let start = c.content_start_line;
    let mut lines = Vec::new();
    let mut cursor_row = 0usize;
    for (i, text) in c.content.iter().enumerate() {
        let ln = start + i as u32;
        if let Some(removed) = h.deletions.get(&ln) {
            lines.push(Line::from(Span::styled(
                format!("    ⌄ −{} ─ {} ", removed.len(), removed[0].trim().chars().take(48).collect::<String>()),
                Style::default().fg(REMOVED),
            )));
        }
        let at_cursor = ln == m.code_cursor;
        if at_cursor {
            cursor_row = lines.len();
        }
        let (tier, tstyle, lstyle) = if h.selected.contains(&ln) {
            (" ▌".to_string(), Style::default().fg(GENERATED), Style::default().fg(FG).bg(SEL))
        } else if let Some(idx) = h.earlier.get(&ln) {
            let n = m.segments()[*idx].seq;
            let tag = if n < 10 { format!("{n}┊") } else { "+┊".into() };
            (tag, Style::default().fg(HUMAN), Style::default().fg(SOFT).bg(EARLIER_BG))
        } else {
            ("  ".into(), Style::default(), Style::default().fg(Color::Rgb(0x8a, 0x90, 0x98)))
        };
        lines.push(Line::from(vec![
            Span::styled(if at_cursor { "›" } else { " " }, Style::default().fg(FG).add_modifier(Modifier::BOLD)),
            Span::styled(tier, tstyle),
            Span::styled(
                format!("{ln:>5} │ "),
                if at_cursor { Style::default().fg(FG).add_modifier(Modifier::BOLD) } else { Style::default().fg(DIM) },
            ),
            Span::styled(text.clone(), lstyle),
        ]));
    }
    // Keep the cursor about a third of the way down.
    let inner = code.height.saturating_sub(2) as usize;
    let scroll = cursor_row.saturating_sub(inner / 3) as u16;
    let short = seg.commit.as_ref().map(|c| c.short.as_str()).unwrap_or("");
    let title = format!(" 4 {} ── as of input {} · {} ", c.path, seg.seq, short);
    f.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(GENERATED))
                .title(Span::styled(title, Style::default().fg(FG).add_modifier(Modifier::BOLD)))
                .title_bottom(Span::styled(
                    format!(" ▌ this input · N┊ written by input N · {} +{} −{} ", c.risk.label(), c.additions, c.deletions),
                    Style::default().fg(DIM),
                )),
        ),
        code,
    );
    if let Some(r) = rat {
        rationale(m, f, r, c);
    }
}

fn sticky_header(m: &Model, seg: &Segment, c: &FileChange, width: u16) -> Paragraph<'static> {
    let w = width as usize;
    let total = m.segments().len();
    let at = match m.resolve_line(c, m.code_cursor) {
        Some(r) if r.sections.is_empty() || is_unsectioned(r.segment) => format!("input {}", r.segment.seq),
        Some(r) => format!(
            "input {} · {}",
            r.segment.seq,
            r.sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>().join(" · ")
        ),
        None => "not changed in this session".into(),
    };
    Paragraph::new(vec![
        Line::from(vec![
            Span::styled(format!("input {}/{total} › ", seg.seq), Style::default().fg(DIM)),
            Span::styled(ellipsize(&seg.input.split_whitespace().collect::<Vec<_>>().join(" "), w.saturating_sub(14)), Style::default().fg(HUMAN).add_modifier(Modifier::BOLD)),
        ]),
        if c.summary.is_empty() {
            Line::from(Span::styled("not summarized yet — W shows what the agent said", Style::default().fg(DIM)))
        } else {
            Line::from(vec![
                Span::styled(ellipsize(&c.summary, w.saturating_sub(13)), Style::default().fg(GENERATED)),
                Span::styled(" [generated]", Style::default().fg(GENERATED).add_modifier(Modifier::DIM)),
            ])
        },
        Line::from(Span::styled(
            ellipsize(&format!("line {} ← {at}", m.code_cursor), w),
            Style::default().fg(SOFT),
        )),
        Line::from(Span::styled(
            "j/k move · W why · n/N this input · ]/[ any input · Esc back",
            Style::default().fg(DIM),
        )),
    ])
}

fn rationale(m: &Model, f: &mut Frame, area: Rect, c: &FileChange) {
    let w = area.width.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    let line = m.code_cursor;
    let (title_bottom, resolved) = match m.resolve_line(c, line) {
        None => {
            lines.push(Line::from(Span::styled(
                format!("line {line} was not changed by any input in this session."),
                Style::default().fg(SOFT),
            )));
            lines.push(Line::from(Span::styled(
                "Blame would name the commit that did change it; that commit is outside this session.",
                Style::default().fg(DIM),
            )));
            (format!(" line {line} · outside this session "), false)
        }
        Some(r) => {
            let total = m.segments().len();
            let same = Some(r.seg_index) == Some(m.seg);
            lines.push(Line::from(vec![
                Span::styled("input      › ", Style::default().fg(DIM)),
                Span::styled(ellipsize(&r.segment.input, w.saturating_sub(13)), Style::default().fg(HUMAN)),
            ]));
            if let Some(oc) = r.change.filter(|c| !c.summary.is_empty()) {
                for (k, chunk) in wrap(&oc.summary, w.saturating_sub(24)).into_iter().enumerate() {
                    lines.push(Line::from(vec![
                        Span::styled(if k == 0 { "summary      " } else { "             " }, Style::default().fg(DIM)),
                        Span::styled(chunk, Style::default().fg(GENERATED)),
                    ]));
                }
                if let Some(l) = lines.last_mut() {
                    l.push_span(Span::styled(" [generated]", Style::default().fg(GENERATED).add_modifier(Modifier::DIM)));
                }
            }
            if r.sections.len() > 1 {
                lines.push(Line::from(Span::styled(
                    format!(
                        "             {} sections cite this one hunk — it is coarser than the work inside it",
                        r.sections.len()
                    ),
                    Style::default().fg(DIM),
                )));
            }
            for sec in &r.sections {
                if !is_unsectioned(r.segment) {
                    lines.push(Line::from(vec![
                        Span::styled("section      ", Style::default().fg(DIM)),
                        Span::styled(sec.title.clone(), Style::default().fg(GENERATED)),
                    ]));
                }
                narration_lines(sec, "  ", w, &mut lines);
            }
            let who = if same { "this input" } else { "an earlier input" };
            (
                format!(" via {} · input {} of {total} ({who}) · its line {} ", r.short, r.segment.seq, r.orig_line),
                true,
            )
        }
    };
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if resolved { FRAME } else { DIM }))
                .title(Span::styled(
                    format!(" why is line {line} here? "),
                    Style::default().fg(FG).add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Span::styled(title_bottom, Style::default().fg(DIM))),
        ),
        area,
    );
}

fn reader(m: &Model, f: &mut Frame, area: Rect) {
    let Some(s) = m.selected_segment() else { return };
    let with_composer = m.reader_has_composer();
    let composer_h = if with_composer { composer_height(m, area.width) } else { 0 };
    let [area, composer_area] = Layout::vertical([Constraint::Min(3), Constraint::Length(composer_h)]).areas(area);
    // Borders and one column of padding each side.
    let lines = reader_lines(m, s, area.width.saturating_sub(4) as usize);
    // With the composer below, its top edge is the reader's bottom one.
    let page = area.height.saturating_sub(if with_composer { 1 } else { 2 });
    let max = (lines.len() as u16).saturating_sub(page);
    m.reader_extent.set((max, page));
    let off = m.reader_offset();
    let total = m.segments().len();
    let place = if max == 0 {
        "all".to_string()
    } else if off >= max {
        "end".to_string()
    } else {
        format!("{}%", (off as usize * 100) / max as usize)
    };
    let place = Span::styled(format!(" {place} "), Style::default().fg(DIM)).into_right_aligned_line();
    let block = Block::default()
        .padding(Padding::horizontal(1))
        .border_style(Style::default().fg(GENERATED))
        .title(Span::styled(
            format!(" input {}/{total} · in full ", s.seq),
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ));
    let block = if with_composer {
        block.borders(Borders::TOP | Borders::LEFT | Borders::RIGHT).title_top(place)
    } else {
        block.borders(Borders::ALL).title_bottom(place)
    };
    f.render_widget(Paragraph::new(lines).scroll((off, 0)).block(block), area);
    if with_composer {
        composer(m, f, composer_area);
    }
}

/// The reader's text: the input, the summary, then the response as the agent
/// gave it, tool calls in place, and the files it changed.
fn reader_lines(m: &Model, s: &Segment, w: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(DIM);
    let theme = markdown::Theme {
        text: Style::default().fg(FG),
        code: Style::default().fg(Color::Rgb(0xb8, 0xc4, 0xa8)),
        dim,
    };
    let mut lines = Vec::new();
    let mut meta = vec![s.model.clone()];
    if !s.elapsed.is_empty() {
        meta.push(s.elapsed.clone());
    }
    if !s.started_at.is_empty() {
        meta.push(format!("{} {}", time::short_date(&s.started_at), s.started_at.get(11..16).unwrap_or_default()));
    }
    meta.push(cost_label(s));
    let words = word_count(&s.said());
    if words > 0 {
        meta.push(format!("{} word{}", fmt_count(words), if words == 1 { "" } else { "s" }));
    }
    if let Some(c) = &s.commit {
        meta.push(if s.link == "commit_time" { format!("{} (by commit time)", c.short) } else { c.short.clone() });
    }
    lines.push(Line::from(Span::styled(ellipsize(&meta.join(" · "), w), dim)));
    lines.push(Line::default());

    // What the developer typed, line for line.
    for (k, l) in s.input.lines().enumerate() {
        let chunks = wrap(l, w.saturating_sub(2));
        if chunks.is_empty() {
            lines.push(Line::default());
        }
        for (j, chunk) in chunks.into_iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(if k == 0 && j == 0 { "› " } else { "  " }, Style::default().fg(HUMAN)),
                Span::styled(chunk, Style::default().fg(HUMAN).add_modifier(Modifier::BOLD)),
            ]));
        }
    }
    lines.push(Line::default());

    if !s.summary.is_empty() {
        let tag = if s.summary_by.is_empty() {
            " [generated]".to_string()
        } else {
            format!(" [generated · {} · ${:.4}]", s.summary_by, s.summary_cost_usd)
        };
        for (k, chunk) in wrap(&s.summary, w.saturating_sub(2)).into_iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(if k == 0 { "↳ " } else { "  " }, Style::default().fg(GENERATED)),
                Span::styled(chunk, Style::default().fg(GENERATED)),
            ]));
        }
        // The tag goes after the sentence, or under it when it would not fit.
        let tag = Span::styled(tag, Style::default().fg(GENERATED).add_modifier(Modifier::DIM));
        match lines.last_mut() {
            Some(l) if l.width() + tag.width() <= w => l.push_span(tag),
            _ => lines.push(Line::from(vec![Span::raw("  "), Span::styled(tag.content.trim_start().to_string(), tag.style)])),
        }
        lines.push(Line::default());
    } else if m.summarizing.contains(&(m.session, m.seg)) {
        lines.push(Line::from(Span::styled("↳ summarizing…", Style::default().fg(GENERATED))));
        lines.push(Line::default());
    }

    let rule = |label: &str| {
        let fill = w.saturating_sub(label.chars().count() + 4);
        Line::from(Span::styled(format!("── {label} {}", "─".repeat(fill)), dim))
    };
    lines.push(rule(if s.is_shell() { "⟩ output of the command" } else { "❝ response, verbatim" }));
    lines.push(Line::default());
    let sectioned = !is_unsectioned(s);
    let mut said_any = false;
    for sec in &s.sections {
        if sectioned {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", sec.title), Style::default().fg(GENERATED).add_modifier(Modifier::BOLD)),
                Span::styled("[generated label]", Style::default().fg(GENERATED).add_modifier(Modifier::DIM)),
            ]));
        }
        for step in &sec.narration {
            match step.kind {
                StepKind::Text => {
                    said_any = true;
                    lines.extend(markdown::render(&step.body, w, &theme));
                    lines.push(Line::default());
                }
                StepKind::ToolCall => {
                    for (k, chunk) in wrap(&step.body, w.saturating_sub(2)).into_iter().enumerate() {
                        lines.push(Line::from(Span::styled(format!("{}{chunk}", if k == 0 { "⟩ " } else { "  " }), dim)));
                    }
                }
                StepKind::Thinking => {
                    lines.push(Line::from(Span::styled("thinking", Style::default().fg(THINKING).add_modifier(Modifier::BOLD))));
                    let t = markdown::Theme { text: Style::default().fg(THINKING).add_modifier(Modifier::ITALIC), ..theme };
                    lines.extend(markdown::render(&step.body, w, &t));
                    lines.push(Line::default());
                }
            }
        }
        if sectioned {
            lines.push(Line::default());
        }
    }
    if s.is_shell() {
        // Output is not Markdown: show it line for line.
        for l in s.reply.lines() {
            let chunks = wrap(l, w);
            if chunks.is_empty() {
                lines.push(Line::default());
            }
            lines.extend(chunks.into_iter().map(|c| Line::from(Span::styled(c, theme.code))));
        }
        lines.push(Line::default());
    } else if !said_any && !s.reply.trim().is_empty() {
        lines.extend(markdown::render(&s.reply, w, &theme));
        lines.push(Line::default());
    }
    if s.status == SegStatus::Running {
        if !s.streaming.trim().is_empty() {
            lines.extend(markdown::render(&s.streaming, w, &theme));
        }
        lines.push(Line::from(Span::styled(
            format!("⟳ {}", m.activity.get(&m.session).map(String::as_str).unwrap_or("working")),
            Style::default().fg(GENERATED).add_modifier(Modifier::BOLD),
        )));
    }
    // The runtime is still diffing the working tree. Saying so beats an empty
    // space that reads as "this input changed nothing", the same way the column
    // and the Changes pane say it.
    if m.pending.contains(&(m.session, m.seg)) {
        lines.push(Line::default());
        lines.push(rule("files changed"));
        lines.push(Line::from(Span::styled("computing changes…", Style::default().fg(SOFT))));
    } else if !s.changes.is_empty() {
        lines.push(Line::default());
        lines.push(rule("files changed"));
        for c in &s.changes {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", c.risk.glyph()), Style::default().fg(risk_color(c.risk))),
                Span::styled(ellipsize(&c.path, w.saturating_sub(16)), Style::default().fg(SOFT)),
                Span::styled(format!("  +{}", c.additions), Style::default().fg(ADDED)),
                Span::styled(format!(" −{}", c.deletions), Style::default().fg(REMOVED)),
            ]));
        }
    }
    while lines.last().is_some_and(|l| l.width() == 0) {
        lines.pop();
    }
    lines
}

/// One paragraph of plain text from Markdown, for a two-line preview: markup
/// characters at line starts and code fences are dropped.
fn plain_preview(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("```"))
        .map(|l| l.trim_start_matches(['#', '>', '-', '*', '+', ' ']))
        .collect::<Vec<_>>()
        .join(" ")
        .replace("**", "")
        .replace('`', "")
}

/// What an input cost, or that it was not recorded on its own.
fn cost_label(s: &Segment) -> String {
    if s.cost_unrecorded { "$—".into() } else { format!("${:.2}", s.cost_usd) }
}

/// 1042 → "1,042".
fn fmt_count(n: usize) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn transcript(m: &Model, f: &mut Frame, area: Rect) {
    let w = area.width.saturating_sub(8);
    let h = area.height.saturating_sub(4);
    let r = Rect::new(area.x + 4, area.y + 2, w, h);
    let inner = w.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    let mut sel_row = 0usize;
    for (i, s) in m.segments().iter().enumerate() {
        if i == m.seg {
            sel_row = lines.len();
        }
        for (k, chunk) in s.input.lines().flat_map(|l| wrap(l, inner.saturating_sub(6))).enumerate() {
            lines.push(Line::from(vec![
                Span::styled(if k == 0 { format!("{:>2} › ", s.seq) } else { "     ".into() }, Style::default().fg(DIM)),
                Span::styled(chunk, Style::default().fg(HUMAN).add_modifier(Modifier::BOLD)),
            ]));
        }
        for sec in &s.sections {
            lines.push(Line::from(vec![
                Span::styled(format!("     ── {} ", sec.title), Style::default().fg(GENERATED)),
                Span::styled("[generated label]", Style::default().fg(GENERATED).add_modifier(Modifier::DIM)),
            ]));
            narration_lines(sec, "     ", inner, &mut lines);
        }
        lines.push(Line::from(""));
    }
    for (qi, q) in m.queued_for_session().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(format!("{:>2} › ", m.segments().len() + qi + 1), Style::default().fg(DIM)),
            Span::styled(q.text.clone(), Style::default().fg(HUMAN)),
            Span::styled("   queued", Style::default().fg(DIM)),
        ]));
    }
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).scroll((sel_row as u16, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(GENERATED))
                .title(Span::styled(
                    format!(" {} — transcript, verbatim ", session_name(m, m.session).0),
                    Style::default().fg(FG).add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Span::styled(" Esc close ", Style::default().fg(DIM))),
        ),
        r,
    );
}

fn status_line(m: &Model) -> Paragraph<'static> {
    // A notice is the most important thing on screen while it lasts, so it
    // gets the whole line rather than whatever the cost fields leave over.
    if let Some(n) = &m.notice {
        return Paragraph::new(Line::from(Span::styled(
            format!(" {n}"),
            Style::default().fg(HUMAN).add_modifier(Modifier::BOLD),
        )));
    }
    let (model, cost, commit) = match m.selected_segment() {
        Some(s) => (
            s.model.clone(),
            format!(" input {} ", cost_label(s)),
            s.commit.as_ref().map(|c| c.short.clone()).unwrap_or_else(|| "no commit".into()),
        ),
        None => ("claude-opus-5".into(), " queued ".into(), "—".into()),
    };
    let model = if m.live { m.agent_model.clone().unwrap_or_else(|| "claude".into()) } else { model };
    // Working here, or working elsewhere: with sessions running side by side,
    // "live" on its own would read as idle while two agents were busy.
    let live = match (m.live, m.is_running(m.session), m.running.len()) {
        (false, ..) => Span::raw(""),
        (true, true, _) => Span::styled(" ● working ", Style::default().fg(GENERATED).add_modifier(Modifier::BOLD)),
        (true, false, 0) => Span::styled(" ● live ", Style::default().fg(ADDED)),
        (true, false, n) => Span::styled(
            format!(" ● live · {n} working elsewhere "),
            Style::default().fg(ADDED),
        ),
    };
    Paragraph::new(Line::from(vec![
        live,
        Span::styled(format!(" {model} "), Style::default().fg(FG).bg(FRAME)),
        Span::styled(cost, Style::default().fg(DIM)),
        Span::styled(format!("· session ${:.2} ", m.session().cost()), Style::default().fg(DIM)),
        Span::styled(format!("· {commit} "), Style::default().fg(DIM)),
        Span::styled(format!("· {}", keymap::hint(m)), Style::default().fg(DIM)),
    ]))
}

/// Truncate with an ellipsis. Grapheme-aware in the real implementation; this
/// counts chars, which is enough for the fixture.
fn ellipsize(s: &str, width: usize) -> String {
    if s.chars().count() <= width || width < 2 {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Greedy wrap. Real implementation should be grapheme-aware; this is enough to
/// prove the layout.
fn wrap(s: &str, width: usize) -> Vec<String> {
    if width < 8 {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > width {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
