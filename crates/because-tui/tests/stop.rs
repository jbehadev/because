//! Ctrl+C stops the running input of the session on screen. The agent ends
//! the input as the recording shows (`fixtures/claude-stream/interrupt.jsonl`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use because_core::{AgentEvent, Fixture, SegStatus, Session};
use because_tui::{Cmd, Model, Msg, dispatch, update, view};

fn live() -> Model {
    let s = |id: &str| Session { id: id.into(), repo: "/repo".into(), branch: "main".into(), ..Default::default() };
    let mut m = Model::new(Fixture { sessions: vec![s("s"), s("t")] });
    m.live = true;
    m
}

fn press(m: &mut Model, k: KeyEvent) {
    if let Some(msg) = dispatch(m, k) {
        update(m, msg);
    }
}

fn ctrl_c(m: &mut Model) {
    press(m, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
}

fn type_text(m: &mut Model, text: &str) {
    for c in text.chars() {
        press(m, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn say(m: &mut Model, text: &str) {
    type_text(m, text);
    press(m, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

/// The first input of the recording: interrupted when the first text arrived.
fn interrupted() -> Vec<AgentEvent> {
    let text = std::fs::read_to_string("../../fixtures/claude-stream/interrupt.jsonl").unwrap();
    let mut out = Vec::new();
    for e in text.lines().flat_map(because_agent::claude::parse_line) {
        let done = matches!(e, AgentEvent::Finished { .. });
        out.push(e);
        if done {
            break;
        }
    }
    out
}

fn finished(cost_total_usd: f64) -> AgentEvent {
    AgentEvent::Finished { reply: "ok".into(), cost_total_usd, duration_ms: 1000, is_error: false, permission_denials: 0 }
}

fn frame(m: &Model) -> String {
    let mut t = Terminal::new(TestBackend::new(150, 36)).unwrap();
    t.draw(|f| view(m, f)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(b.area.width as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// Session 0 runs an input; the outbox is empty.
fn running() -> Model {
    let mut m = live();
    say(&mut m, "write an essay");
    m.outbox.clear();
    m
}

#[test]
fn ctrl_c_stops_the_running_input() {
    let mut m = running();
    ctrl_c(&mut m);
    assert_eq!(m.outbox, [Cmd::Stop { session: 0 }]);
    assert!(!m.quit);
    assert!(m.notice.as_deref().unwrap_or("").contains("Ctrl+C again quits"), "{:?}", m.notice);
}

#[test]
fn a_second_ctrl_c_quits() {
    let mut m = running();
    ctrl_c(&mut m);
    ctrl_c(&mut m);
    assert_eq!(m.outbox, [Cmd::Stop { session: 0 }], "one stop only");
    assert!(m.quit);
}

#[test]
fn a_draft_is_cleared_before_anything_stops() {
    let mut m = running();
    type_text(&mut m, "next");
    ctrl_c(&mut m);
    assert!(m.draft_is_empty());
    assert!(m.outbox.is_empty());
    assert!(!m.quit);
}

#[test]
fn ctrl_c_does_not_stop_a_session_that_is_not_on_screen() {
    let mut m = live();
    m.session = 1;
    say(&mut m, "work elsewhere");
    m.session = 0;
    m.outbox.clear();
    ctrl_c(&mut m);
    assert!(m.outbox.is_empty());
    assert!(m.quit);
}

#[test]
fn the_hint_says_how_to_stop() {
    let m = running();
    assert!(frame(&m).contains("Ctrl+C stops"), "{}", frame(&m));
}

#[test]
fn a_stopped_input_ends_as_a_usual_input() {
    let mut m = running();
    ctrl_c(&mut m);
    m.outbox.clear();
    for e in interrupted() {
        update(&mut m, Msg::Agent(0, e));
    }
    let s = &m.segments()[0];
    assert_eq!(s.status, SegStatus::Done);
    assert!(s.said().contains("You stopped this input"), "{}", s.said());
    assert!(!m.is_running(0));
    assert!(m.outbox.contains(&Cmd::Finish { session: 0, seg: 0 }), "its changes are computed: {:?}", m.outbox);
}

#[test]
fn the_input_after_a_stop_costs_only_its_own_part() {
    let mut m = running();
    update(&mut m, Msg::Agent(0, finished(0.5)));
    say(&mut m, "second");
    ctrl_c(&mut m);
    for e in interrupted() {
        update(&mut m, Msg::Agent(0, e));
    }
    say(&mut m, "third");
    update(&mut m, Msg::Agent(0, finished(0.75)));
    let costs: Vec<f64> = m.segments().iter().map(|s| s.cost_usd).collect();
    assert_eq!(costs, [0.5, 0.0, 0.25]);
}

#[test]
fn text_sent_before_a_stop_becomes_the_next_input() {
    let mut m = running();
    say(&mut m, "do it the other way");
    ctrl_c(&mut m);
    for e in interrupted() {
        update(&mut m, Msg::Agent(0, e));
    }
    assert_eq!(m.segments()[0].status, SegStatus::Done);
    update(&mut m, Msg::Agent(0, AgentEvent::InputTaken("do it the other way".into())));
    assert_eq!(m.segments().len(), 2);
    assert!(m.is_running(0));
    assert_eq!(m.segments()[1].input, "do it the other way");
}
