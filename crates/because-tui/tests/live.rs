//! A live session, driven by replaying a recorded real Claude CLI session
//! (`fixtures/claude-stream/two-inputs.jsonl`) through the model.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use because_core::{AgentEvent, Commit, FileChange, Fixture, SegStatus, Session, StepKind};
use because_tui::{Cmd, Model, Msg, View, dispatch, session_name, update, view};

fn live() -> Model {
    let fx = Fixture {
        sessions: vec![Session {
            id: "s".into(),
            // As `cuz live` starts it: unnamed until something is typed.
            title: String::new(),
            repo: "/repo".into(),
            branch: "main".into(),
            segments: vec![],
            ..Default::default()
        }],
    };
    let mut m = Model::new(fx);
    m.live = true;
    m
}

/// The recording's events, split per input at each `Finished`.
fn inputs() -> Vec<Vec<AgentEvent>> {
    let all: Vec<AgentEvent> = std::fs::read_to_string("../../fixtures/claude-stream/two-inputs.jsonl")
        .unwrap()
        .lines()
        .flat_map(because_agent::claude::parse_line)
        .collect();
    let mut out = vec![Vec::new()];
    for e in all {
        let done = matches!(e, AgentEvent::Finished { .. });
        out.last_mut().unwrap().push(e);
        if done {
            out.push(Vec::new());
        }
    }
    out.pop();
    out
}

fn say(m: &mut Model, text: &str) {
    for c in text.chars() {
        if let Some(msg) = dispatch(m, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) {
            update(m, msg);
        }
    }
    let msg = dispatch(m, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
    update(m, msg);
}

fn replay(m: &mut Model, events: &[AgentEvent]) {
    replay_in(m, 0, events);
}

/// The same, for the agent of a session that is not on screen.
fn replay_in(m: &mut Model, session: usize, events: &[AgentEvent]) {
    for e in events {
        update(m, Msg::Agent(session, e.clone()));
    }
}

fn frame(m: &Model) -> String {
    let mut t = Terminal::new(TestBackend::new(150, 36)).unwrap();
    t.draw(|f| view(m, f)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(b.area.width as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[test]
fn enter_starts_an_input_and_asks_the_runtime_to_send_it() {
    let mut m = live();
    say(&mut m, "Create a file named hello.txt");
    assert_eq!(m.segments().len(), 1);
    assert_eq!(m.segments()[0].status, SegStatus::Running);
    assert_eq!(m.outbox, [Cmd::Send { session: 0, seg: 0, text: "Create a file named hello.txt".into() }]);
}

#[test]
fn a_real_response_fills_the_segment() {
    let mut m = live();
    say(&mut m, "Create a file named hello.txt containing the single line: hi.");
    m.outbox.clear();
    replay(&mut m, &inputs()[0]);
    let s = &m.segments()[0];
    assert_eq!(s.status, SegStatus::Done);
    let steps: Vec<(StepKind, &str)> = s.sections[0].narration.iter().map(|n| (n.kind, n.body.as_str())).collect();
    assert_eq!(steps[0], (StepKind::ToolCall, "Write hello.txt"), "repo prefix stripped");
    assert!(steps.iter().any(|(k, b)| *k == StepKind::Text && b.contains("hello.txt")));
    assert!(s.reply.contains("hello.txt"));
    assert_eq!(m.outbox, [Cmd::Finish { session: 0, seg: 0 }], "snapshot after it finishes");
    assert!(m.pending.contains(&(0, 0)));
    assert!(m.agent_model.as_deref().unwrap_or("").starts_with("claude-"));
}

#[test]
fn each_input_costs_the_difference_between_running_totals() {
    let mut m = live();
    let evs = inputs();
    say(&mut m, "first");
    replay(&mut m, &evs[0]);
    say(&mut m, "second");
    replay(&mut m, &evs[1]);
    let (a, b) = (m.segments()[0].cost_usd, m.segments()[1].cost_usd);
    assert!((a - 0.0297).abs() < 0.0001, "{a}");
    assert!((b - 0.0089).abs() < 0.0001, "input 2 alone, not the running total: {b}");
}

#[test]
fn an_input_typed_while_the_agent_works_goes_to_it_at_once() {
    let mut m = live();
    say(&mut m, "first");
    m.outbox.clear();
    say(&mut m, "second");
    assert_eq!(m.outbox, [Cmd::SendMidTurn { session: 0, text: "second".into() }]);
    assert!(m.queued.is_empty());
    assert_eq!(m.segments().len(), 1);
    m.outbox.clear();
    // The agent reads it at a tool boundary, before the input ends.
    update(&mut m, Msg::Agent(0, AgentEvent::InputTaken("first".into())));
    update(&mut m, Msg::Agent(0, AgentEvent::InputTaken("second".into())));
    assert_eq!(m.segments()[0].input, "first\n\nAdded while the agent worked:\nsecond");
    replay(&mut m, &inputs()[0]);
    assert_eq!(m.segments().len(), 1, "one response for the two");
    assert!(!m.outbox.iter().any(|c| matches!(c, Cmd::Send { .. })), "{:?}", m.outbox);
    assert!(!m.is_running(0));
}

#[test]
fn an_input_the_agent_reads_after_the_end_is_an_input_of_its_own() {
    let mut m = live();
    say(&mut m, "first");
    say(&mut m, "second");
    replay(&mut m, &inputs()[0]);
    m.outbox.clear();
    update(&mut m, Msg::Agent(0, AgentEvent::InputTaken("second".into())));
    assert_eq!(m.segments().len(), 2);
    assert_eq!((m.segments()[1].input.as_str(), m.segments()[1].status), ("second", SegStatus::Running));
    assert!(!m.outbox.iter().any(|c| matches!(c, Cmd::Send { .. })), "it went out already: {:?}", m.outbox);
    replay(&mut m, &inputs()[1]);
    assert_eq!(m.segments()[1].status, SegStatus::Done);
}

// ── several sessions ───────────────────────────────────────────────────────

#[test]
fn a_new_session_is_empty_and_its_inputs_are_its_own() {
    let mut m = live();
    say(&mut m, "first");
    replay(&mut m, &inputs()[0]);
    m.outbox.clear();
    update(&mut m, Msg::NewSession);
    assert_eq!(m.fx.sessions.len(), 2);
    assert_eq!(m.session, 1);
    assert!(m.segments().is_empty(), "a session knows nothing of what was said in another");
    say(&mut m, "second");
    assert_eq!(m.outbox, [Cmd::Send { session: 1, seg: 0, text: "second".into() }], "sent as session 2's own input");
    assert_eq!(m.running.get(&1), Some(&0));
    assert_eq!(m.fx.sessions[0].segments.len(), 1, "the first session is left as it was");
}

#[test]
fn an_input_in_another_session_goes_out_while_the_first_is_still_working() {
    let mut m = live();
    say(&mut m, "first");
    update(&mut m, Msg::NewSession);
    m.outbox.clear();
    say(&mut m, "in the new session");
    assert_eq!(
        m.outbox,
        [Cmd::Send { session: 1, seg: 0, text: "in the new session".into() }],
        "a session has its own agent: it does not wait for another session's input"
    );
    assert!(m.queued.is_empty(), "nothing is waiting: {:?}", m.queued);
    assert_eq!(m.running.len(), 2, "both sessions are working");
    assert_eq!((m.running.get(&0), m.running.get(&1)), (Some(&0), Some(&0)));
}

#[test]
fn each_session_answers_its_own_input_in_its_own_order() {
    let mut m = live();
    say(&mut m, "first");
    update(&mut m, Msg::NewSession);
    say(&mut m, "in the new session");
    // Session 2 finishes first. Only its input is affected.
    m.outbox.clear();
    replay_in(&mut m, 1, &inputs()[0]);
    assert_eq!(m.fx.sessions[1].segments[0].status, SegStatus::Done);
    assert_eq!(m.fx.sessions[0].segments[0].status, SegStatus::Running, "the other session is still working");
    assert_eq!(m.outbox, [Cmd::Finish { session: 1, seg: 0 }]);
    assert_eq!(m.running.keys().copied().collect::<Vec<_>>(), [0]);

    m.outbox.clear();
    replay_in(&mut m, 0, &inputs()[1]);
    assert_eq!(m.fx.sessions[0].segments[0].status, SegStatus::Done);
    assert_eq!(m.outbox, [Cmd::Finish { session: 0, seg: 0 }]);
    assert!(m.running.is_empty());
}

#[test]
fn a_second_input_in_the_same_session_goes_to_its_own_agent() {
    let mut m = live();
    update(&mut m, Msg::NewSession);
    say(&mut m, "first");
    m.outbox.clear();
    say(&mut m, "second");
    assert_eq!(m.outbox, [Cmd::SendMidTurn { session: 1, text: "second".into() }]);
}

#[test]
fn what_an_input_cost_is_measured_against_its_own_session() {
    let mut m = live();
    say(&mut m, "first");
    update(&mut m, Msg::NewSession);
    say(&mut m, "in the new session");
    // Each claude reports a total for its own conversation, so session 2's
    // first input is charged $0.0297 and not $0.0297 minus session 1's total.
    replay(&mut m, &inputs()[0]);
    replay_in(&mut m, 1, &inputs()[0]);
    let (a, b) = (m.fx.sessions[0].segments[0].cost_usd, m.fx.sessions[1].segments[0].cost_usd);
    assert!((a - b).abs() < 0.0001, "the same input costs the same in either session: {a} vs {b}");
    assert!(b > 0.0, "session 2 is not charged nothing because session 1 had spent more: {b}");
}

#[test]
fn the_first_column_lists_the_sessions_and_marks_the_one_the_agent_is_in() {
    let mut m = live();
    say(&mut m, "first");
    update(&mut m, Msg::NewSession);
    let out = frame(&m);
    assert!(out.contains("▸ new session"), "the new session is in the first column, selected and unnamed:\n{out}");
    assert!(out.contains("+ n new session"), "and how to start another:\n{out}");
    let running = out.lines().find(|l| l.contains("1 input")).unwrap_or_default();
    assert!(running.contains("⟳"), "the session being worked in is marked even from another: {running:?}");
    assert!(out.contains("● live · 1 working elsewhere"), "and the status line does not read as idle:\n{out}");
}

#[test]
fn a_session_is_named_by_its_first_input_then_by_its_summary() {
    let mut m = live();
    assert_eq!(session_name(&m, 0), ("new session".into(), None), "nothing has been typed in it yet");
    let out = frame(&m);
    assert!(out.contains("▸ new session"), "and the first column says so:\n{out}");

    say(&mut m, "make hello.txt");
    let (name, from) = session_name(&m, 0);
    assert_eq!(name, "make hello.txt", "typing names the session after the input");
    assert!(from.is_some(), "in the colour of what the developer typed, not the neutral one");

    replay(&mut m, &inputs()[0]);
    update(
        &mut m,
        Msg::Summary {
            session: 0,
            seg: 0,
            text: "Created hello.txt containing one line.".into(),
            model: "claude-haiku-4-5".into(),
            cost_usd: 0.002,
        },
    );
    let (name, from) = session_name(&m, 0);
    assert_eq!(name, "Created hello.txt containing one line.", "the summary of input 1 takes over");
    assert!(from.is_some(), "marked as generated");
}

#[test]
fn a_new_session_is_named_by_an_input_that_has_not_run_yet() {
    let mut m = live();
    // The sample view, where nothing runs at all: an input waits and the
    // session it belongs to is still the one being named.
    m.live = false;
    update(&mut m, Msg::NewSession);
    say(&mut m, "look at the retry budget");
    assert!(m.segments().is_empty(), "nothing has run here");
    assert_eq!(session_name(&m, 1).0, "look at the retry budget", "an input that has not run names its session too");
}

#[test]
fn a_session_that_came_with_a_title_keeps_it() {
    let mut m = live();
    m.fx.sessions[0].title = "hidden-path checks".into();
    say(&mut m, "make hello.txt");
    assert_eq!(session_name(&m, 0), ("hidden-path checks".into(), None));
}

#[test]
fn a_multi_line_input_names_the_session_with_its_first_line() {
    let mut m = live();
    say(&mut m, "fix the retry budget");
    m.fx.sessions[0].segments[0].input = "\n\nfix the retry budget\n\nit is per-attempt, not per-request".into();
    assert_eq!(session_name(&m, 0).0, "fix the retry budget");
}

#[test]
fn changes_attach_to_the_input_and_open_in_the_code_pane() {
    let mut m = live();
    say(&mut m, "make hello");
    replay(&mut m, &inputs()[0]);
    let hunk = because_core::Hunk {
        seq: 0, header: String::new(), old_start: 0, old_lines: 0, new_start: 1, new_lines: 1,
        summary: String::new(), patch: "@@ -0,0 +1 @@\n+hi".into(),
    };
    let change = FileChange {
        id: "abc1234-f0".into(), path: "hello.txt".into(), change_type: "add".into(), additions: 1, deletions: 0,
        risk: because_core::Risk::None, summary: String::new(), state: "unsummarized".into(), content_start_line: 1,
        content: vec!["hi".into()], hunks: vec![hunk], line_origins: [(1, ("abc1234".to_string(), 1))].into(),
    };
    let commit = Commit { sha: "abc1234".repeat(6)[..40].to_string(), short: "abc1234".into(), subject: String::new(), author: String::new(), committed_at: String::new(), ..Default::default() };
    update(&mut m, Msg::Changes { session: 0, seg: 0, commit, changes: vec![change] });
    assert!(m.pending.is_empty());
    assert_eq!(m.segments()[0].sections[0].hunk_refs.len(), 1, "the response cites the hunk");
    update(&mut m, Msg::FocusAt(2));
    update(&mut m, Msg::FocusRight);
    assert_eq!(m.view, View::Code);
    update(&mut m, Msg::ToggleRationale);
    let out = frame(&m);
    assert!(out.contains("hello.txt"), "{out}");
    assert!(out.contains("❝ said"), "W shows what the agent said:\n{out}");
}

#[test]
fn a_running_input_shows_progress_while_it_streams() {
    let mut m = live();
    say(&mut m, "make hello");
    let evs = &inputs()[0];
    let upto = evs.iter().position(|e| matches!(e, AgentEvent::TextDelta(_))).unwrap() + 1;
    replay(&mut m, &evs[..upto]);
    let out = frame(&m);
    assert!(out.contains("⟳ writing"), "says what the agent is doing:\n{out}");
    assert!(out.contains("⟩ Write hello.txt"), "{out}");
    assert!(out.contains("● working"), "{out}");
}

#[test]
fn before_the_agent_says_anything_it_reads_as_starting_then_as_quiet() {
    let mut m = live();
    m.log_path = Some("/repo/.git/because/s.log".into());
    say(&mut m, "make hello");
    assert!(frame(&m).contains("⟳ starting claude"));
    let later = m.now + std::time::Duration::from_secs(25);
    update(&mut m, Msg::Tick(later));
    let out = frame(&m);
    assert!(out.contains("no response from claude after 25s"), "{out}");
    assert!(out.contains("log: /repo/.git/because/s.log"), "{out}");
    update(&mut m, Msg::Agent(0, AgentEvent::Activity("thinking".into())));
    assert!(frame(&m).contains("⟳ thinking"), "any sign of life clears it");
}

#[test]
fn the_reason_an_agent_exited_keeps_several_lines() {
    let mut m = live();
    say(&mut m, "make hello");
    update(&mut m, Msg::Agent(0, AgentEvent::Exited { detail: "error: unknown option '--x'\n\n(Did you mean --y?)".into() }));
    assert_eq!(m.segments()[0].reply, "claude exited before finishing: error: unknown option '--x' · (Did you mean --y?)");
    // its changes come back empty: say it stopped, not that it was answered
    let commit = Commit { sha: "a".repeat(40), short: "aaaaaaa".into(), subject: String::new(), author: String::new(), committed_at: String::new(), ..Default::default() };
    update(&mut m, Msg::Changes { session: 0, seg: 0, commit, changes: vec![] });
    let out = frame(&m);
    assert!(out.contains("stopped before finishing"), "{out}");
    assert!(!out.contains("answered in conversation"), "{out}");
    assert!(!out.contains("failed ·  ·"), "no empty duration:\n{out}");
}

#[test]
fn enter_in_the_sample_view_says_there_is_no_agent() {
    let mut m = live();
    m.live = false;
    say(&mut m, "anything");
    assert!(m.notice.as_deref().unwrap_or("").contains("run `cuz`"), "{:?}", m.notice);
    assert!(frame(&m).contains("not sent · no agent in this view"));
}

#[test]
fn the_agent_exiting_mid_input_marks_it_failed_and_says_why() {
    let mut m = live();
    say(&mut m, "make hello");
    update(&mut m, Msg::Agent(0, AgentEvent::Exited { detail: "Invalid API key".into() }));
    let s = &m.segments()[0];
    assert_eq!(s.status, SegStatus::Failed);
    assert!(m.notice.as_deref().unwrap_or("").contains("Invalid API key"));
    assert!(m.running.is_empty(), "the next input can start");
}

#[test]
fn outside_a_live_session_enter_only_queues() {
    let mut m = live();
    m.live = false;
    say(&mut m, "anything");
    assert!(m.outbox.is_empty());
    assert_eq!(m.queued.len(), 1);
}

// ── unread replies ─────────────────────────────────────────────────────────

#[test]
fn a_reply_written_while_another_session_is_on_screen_is_unread_until_you_go_there() {
    let mut m = live();
    say(&mut m, "first");
    update(&mut m, Msg::NewSession);
    say(&mut m, "in the new session");

    replay_in(&mut m, 0, &inputs()[0]);
    replay_in(&mut m, 1, &inputs()[0]);
    assert_eq!(m.unread.iter().copied().collect::<Vec<_>>(), [0], "the one on screen was seen as it was written");
    assert_eq!(frame(&m).matches('◆').count(), 1, "{}", frame(&m));

    update(&mut m, Msg::FocusAt(0));
    update(&mut m, Msg::Up);
    assert_eq!(m.session, 0);
    assert!(m.unread.is_empty(), "selecting it is reading it");
    assert!(!frame(&m).contains('◆'));
}

#[test]
fn a_reply_still_running_is_not_unread_yet() {
    let mut m = live();
    say(&mut m, "first");
    update(&mut m, Msg::NewSession);
    let events = &inputs()[0];
    replay_in(&mut m, 0, &events[..events.len() - 1]);
    assert!(m.unread.is_empty(), "it shows as working, not as waiting to be read");
}
