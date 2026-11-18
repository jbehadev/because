//! The approval prompt: an agent that wants to run a tool its permission mode
//! does not allow on its own waits until the developer answers.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use because_core::{AgentEvent, Decision, Fixture, PermissionRequest, Session};
use because_tui::{APPROVAL_GRACE, Cmd, Model, Msg, dispatch, update, view};

fn live() -> Model {
    let s = |id: &str| Session { id: id.into(), repo: "/repo".into(), branch: "main".into(), ..Default::default() };
    let fx = Fixture { sessions: vec![s("s"), s("t")] };
    let mut m = Model::new(fx);
    m.live = true;
    m
}

fn ask(id: &str, always: Option<&str>) -> AgentEvent {
    AgentEvent::PermissionRequest(PermissionRequest {
        request_id: id.into(),
        tool: "Bash".into(),
        target: "cargo test".into(),
        detail: "cargo test\n\nRun the tests".into(),
        always: always.map(String::from),
        questions: Vec::new(),
    })
}

fn key(m: &mut Model, c: char) -> bool {
    let mods = if c.is_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
    match dispatch(m, KeyEvent::new(KeyCode::Char(c), mods)) {
        Some(msg) => {
            update(m, msg);
            true
        }
        None => false,
    }
}

fn type_and_send(m: &mut Model, text: &str) {
    for c in text.chars() {
        key(m, c);
    }
    let msg = dispatch(m, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
    update(m, msg);
}

/// Let the grace period pass.
fn later(m: &mut Model) {
    let t = m.now + APPROVAL_GRACE + Duration::from_millis(1);
    update(m, Msg::Tick(t));
}

fn frame(m: &Model) -> String {
    let mut t = Terminal::new(TestBackend::new(150, 36)).unwrap();
    t.draw(|f| view(m, f)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(b.area.width as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// An input running, the agent asking about a tool call.
fn asked(always: Option<&str>) -> Model {
    let mut m = live();
    update(&mut m, Msg::Tick(Instant::now()));
    type_and_send(&mut m, "run the tests");
    m.outbox.clear();
    update(&mut m, Msg::Agent(0, ask("req_1", always)));
    m
}

#[test]
fn a_request_puts_the_call_on_screen() {
    let m = asked(Some("Bash(cargo test:*)"));
    let f = frame(&m);
    assert!(f.contains("approve?"), "{f}");
    assert!(f.contains("Run the tests"), "the call in full: {f}");
    assert!(f.contains("allow for this session: Bash(cargo test:*)"), "{f}");
    assert!(f.contains("waiting for your approval"), "the input says why it stopped: {f}");
}

#[test]
fn allowing_answers_the_agent_that_asked() {
    let mut m = asked(None);
    later(&mut m);
    assert!(key(&mut m, 'a'));
    assert_eq!(m.outbox, [Cmd::Answer { session: 0, request_id: "req_1".into(), decision: Decision::AllowOnce }]);
    assert!(m.approval().is_none());
    assert!(!frame(&m).contains("approve?"));
}

#[test]
fn deny_and_allow_for_the_session() {
    let mut m = asked(Some("Bash(cargo test:*)"));
    later(&mut m);
    key(&mut m, 'A');
    assert_eq!(m.outbox.last(), Some(&Cmd::Answer { session: 0, request_id: "req_1".into(), decision: Decision::AllowSession }));
    assert!(m.notice.as_deref().unwrap_or("").contains("Bash(cargo test:*)"));

    update(&mut m, Msg::Agent(0, ask("req_2", None)));
    later(&mut m);
    key(&mut m, 'd');
    assert_eq!(m.outbox.last(), Some(&Cmd::Answer { session: 0, request_id: "req_2".into(), decision: Decision::Deny }));
}

#[test]
fn auto_mode_is_offered_with_or_without_a_rule() {
    for always in [None, Some("Bash(cargo test:*)")] {
        let mut m = asked(always);
        assert!(frame(&m).contains("switch this session to auto mode"), "{always:?}");
        later(&mut m);
        assert!(key(&mut m, 'm'));
        assert_eq!(m.outbox, [Cmd::Answer { session: 0, request_id: "req_1".into(), decision: Decision::AllowAuto }]);
        assert!(m.notice.as_deref().unwrap_or("").contains("auto mode"));
        assert!(m.approval().is_none());
    }
}

#[test]
fn allow_for_the_session_is_not_offered_without_a_rule() {
    let mut m = asked(None);
    later(&mut m);
    assert!(!key(&mut m, 'A'));
    assert!(m.approval().is_some());
    assert!(!frame(&m).contains("allow for this session"));
}

#[test]
fn keys_typed_as_it_opens_do_not_answer_it() {
    let mut m = asked(None);
    // Mid-word in the composer when the prompt came up.
    assert!(!key(&mut m, 'a'));
    assert!(!key(&mut m, 'd'));
    assert!(m.outbox.is_empty());
    assert!(m.approval().is_some());
    assert!(m.draft_is_empty(), "nor are they typed underneath it");
}

#[test]
fn the_prompt_has_the_keyboard() {
    let mut m = asked(None);
    later(&mut m);
    for c in ['q', 'j', 'i', 'n', 'x', '1'] {
        assert!(!key(&mut m, c), "{c}");
    }
    assert!(!m.quit);
    assert_eq!(dispatch(&m, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).map(|_| ()), None);
    // Ctrl+C stops the input that asked, and a second press gets out.
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let msg = dispatch(&m, ctrl_c).unwrap();
    update(&mut m, msg);
    assert_eq!(m.outbox, [Cmd::Stop { session: 0 }]);
    assert!(m.approval().is_none());
    assert!(!m.quit);
    let msg = dispatch(&m, ctrl_c).unwrap();
    update(&mut m, msg);
    assert!(m.quit);
}

#[test]
fn requests_from_several_sessions_wait_their_turn() {
    let mut m = asked(None);
    update(&mut m, Msg::Agent(1, ask("req_b", None)));
    assert!(frame(&m).contains("1 more waiting"));
    later(&mut m);
    key(&mut m, 'a');
    let a = m.approval().expect("the second comes up");
    assert_eq!((a.session, a.req.request_id.as_str()), (1, "req_b"));
    // It has a grace period of its own: the key that answered the first does
    // not answer the second with a repeat.
    assert!(!key(&mut m, 'a'));
    later(&mut m);
    key(&mut m, 'd');
    assert_eq!(m.outbox.last(), Some(&Cmd::Answer { session: 1, request_id: "req_b".into(), decision: Decision::Deny }));
}

#[test]
fn the_same_request_twice_is_one_prompt() {
    let mut m = asked(None);
    update(&mut m, Msg::Agent(0, ask("req_1", None)));
    assert_eq!(m.approvals.len(), 1);
}

#[test]
fn a_withdrawn_request_closes_its_prompt() {
    let mut m = asked(None);
    update(&mut m, Msg::Agent(0, AgentEvent::PermissionCancelled { request_id: "req_1".into() }));
    assert!(m.approval().is_none());
}

#[test]
fn an_agent_that_exits_takes_its_prompts_with_it() {
    let mut m = asked(None);
    update(&mut m, Msg::Agent(1, ask("req_b", None)));
    update(&mut m, Msg::Agent(0, AgentEvent::Exited { detail: "killed".into() }));
    assert_eq!(m.approvals.len(), 1);
    assert_eq!(m.approval().unwrap().session, 1, "the other session's is still up");
}

#[test]
fn the_waiting_session_is_marked() {
    let m = asked(None);
    let f = frame(&m);
    // The Sessions column is the first 33 cells at this width.
    let line: String = f.lines().find(|l| l.contains("1 input")).expect("the session's meta line").chars().take(33).collect();
    assert!(line.contains(" ?"), "{line}");
}
