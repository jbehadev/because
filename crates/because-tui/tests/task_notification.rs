//! A response that the agent starts on its own after a background task ends,
//! replayed from `fixtures/claude-stream/task-notification.jsonl`.

use because_core::{AgentEvent, Fixture, SegStatus, Session, StepKind};
use because_tui::{Cmd, Model, Msg, update};

fn live() -> Model {
    let s = Session { id: "s".into(), repo: "/repo".into(), branch: "main".into(), ..Default::default() };
    let mut m = Model::new(Fixture { sessions: vec![s] });
    m.live = true;
    m
}

fn submit(m: &mut Model, text: &str) {
    update(m, Msg::Paste(text.into()));
    update(m, Msg::Submit);
}

fn feed(m: &mut Model, e: AgentEvent) {
    update(m, Msg::Agent(0, e));
}

fn replay() -> (Model, Vec<Cmd>) {
    let mut m = live();
    submit(&mut m, "Build the tests, and tell me when they pass.");
    let mut cmds = std::mem::take(&mut m.outbox);
    let text = std::fs::read_to_string("../../fixtures/claude-stream/task-notification.jsonl").unwrap();
    for line in text.lines() {
        for e in because_agent::claude::parse_line(line) {
            feed(&mut m, e);
        }
        cmds.append(&mut m.outbox);
    }
    (m, cmds)
}

fn started() -> AgentEvent {
    AgentEvent::Started { model: "claude-opus-5-5".into(), session_id: "s".into() }
}

#[test]
fn the_response_of_its_own_becomes_an_input() {
    let (m, _) = replay();
    let segs = m.segments();
    assert_eq!(segs.len(), 2);
    assert!(segs.iter().all(|s| s.status == SegStatus::Done));
    let own = &segs[1];
    assert!(own.input.contains("\"Watch the log\" was stopped"), "{}", own.input);
    assert!(own.input.contains("\"Build and run the tests\" completed"), "{}", own.input);
    let steps = &own.sections[0].narration;
    assert!(steps.iter().any(|s| s.kind == StepKind::ToolCall && s.body.starts_with("Bash")), "{steps:?}");
    assert_eq!(own.reply, "The tests pass: 59 passed, 0 failed.");
    assert!(!m.is_running(0));
}

#[test]
fn it_is_measured_and_not_sent() {
    let (_, cmds) = replay();
    let sends = cmds.iter().filter(|c| matches!(c, Cmd::Send { .. })).count();
    let begins: Vec<usize> = cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Begin { session: 0, seg } => Some(*seg),
            _ => None,
        })
        .collect();
    assert_eq!(sends, 1);
    assert_eq!(begins, [1]);
}

#[test]
fn the_selection_follows_from_the_last_input() {
    let (m, _) = replay();
    assert_eq!(m.seg, 1);
}

#[test]
fn text_typed_before_the_first_step_joins_the_response() {
    let mut m = live();
    feed(&mut m, started());
    m.outbox.clear();
    submit(&mut m, "also run clippy");
    assert_eq!(m.outbox, [Cmd::SendMidTurn { session: 0, text: "also run clippy".into() }]);
}

#[test]
fn a_notice_read_in_the_running_input_adds_nothing() {
    let mut m = live();
    submit(&mut m, "first");
    feed(&mut m, started());
    feed(&mut m, AgentEvent::TaskEnded("Background command \"x\" completed (exit code 0)".into()));
    assert_eq!(m.segments().len(), 1);
    assert_eq!(m.segments()[0].input, "first");
    assert!(m.tasks_ended.is_empty());
}

#[test]
fn text_read_after_the_start_takes_the_response() {
    let mut m = live();
    m.mid_turn.entry(0).or_default().push_back("then the docs".into());
    feed(&mut m, started());
    feed(&mut m, AgentEvent::InputTaken("then the docs".into()));
    feed(&mut m, AgentEvent::Text("Done with the docs.".into()));
    assert_eq!(m.segments().len(), 1);
    assert_eq!(m.segments()[0].input, "then the docs");
    assert_eq!(m.segments()[0].sections[0].narration.len(), 1);
}
