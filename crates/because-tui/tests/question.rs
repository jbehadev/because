//! The question prompt: the agent asks the developer questions with
//! `AskUserQuestion`, and waits until the developer answers them.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use because_core::{AgentEvent, Decision, Fixture, PermissionRequest, Question, QuestionOption, Session};
use because_tui::{APPROVAL_GRACE, Cmd, Model, Msg, dispatch, update, view};

fn live() -> Model {
    let s = Session { id: "s".into(), repo: "/repo".into(), branch: "main".into(), ..Default::default() };
    let mut m = Model::new(Fixture { sessions: vec![s] });
    m.live = true;
    m
}

fn q(question: &str, header: &str, labels: &[&str], multi: bool) -> Question {
    Question {
        question: question.into(),
        header: header.into(),
        options: labels.iter().map(|l| QuestionOption { label: l.to_string(), description: format!("About {l}") }).collect(),
        multi,
    }
}

fn ask(questions: Vec<Question>) -> AgentEvent {
    AgentEvent::PermissionRequest(PermissionRequest {
        request_id: "req_1".into(),
        tool: "AskUserQuestion".into(),
        target: questions[0].question.clone(),
        detail: String::new(),
        always: None,
        questions,
    })
}

fn color() -> Question {
    q("Which color?", "Color", &["Red", "Blue"], false)
}

fn sizes() -> Question {
    q("Which sizes?", "Sizes", &["Small", "Medium", "Large"], true)
}

fn press(m: &mut Model, code: KeyCode) -> bool {
    match dispatch(m, KeyEvent::new(code, KeyModifiers::NONE)) {
        Some(msg) => {
            update(m, msg);
            true
        }
        None => false,
    }
}

fn keys(m: &mut Model, s: &str) {
    for c in s.chars() {
        press(m, KeyCode::Char(c));
    }
}

/// Let the grace period pass.
fn later(m: &mut Model) {
    let t = m.now + APPROVAL_GRACE + Duration::from_millis(1);
    update(m, Msg::Tick(t));
}

fn frame(m: &Model) -> String {
    let mut t = Terminal::new(TestBackend::new(150, 40)).unwrap();
    t.draw(|f| view(m, f)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(b.area.width as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// An input running, the agent asking these questions.
fn asked(questions: Vec<Question>) -> Model {
    let mut m = live();
    update(&mut m, Msg::Tick(Instant::now()));
    keys(&mut m, "pick a color");
    press(&mut m, KeyCode::Enter);
    m.outbox.clear();
    update(&mut m, Msg::Agent(0, ask(questions)));
    m
}

fn answers(m: &Model) -> Option<Vec<String>> {
    match m.outbox.last() {
        Some(Cmd::Answer { session: 0, request_id, decision: Decision::Answers(a) }) if request_id == "req_1" => Some(a.clone()),
        _ => None,
    }
}

#[test]
fn the_first_question_is_on_screen() {
    let m = asked(vec![color(), sizes()]);
    let f = frame(&m);
    for text in ["question 1 of 2", "Color", "Which color?", "1. Red", "2. Blue", "About Blue", "3. Other"] {
        assert!(f.contains(text), "{text}: {f}");
    }
    assert!(!f.contains("allow once"), "{f}");
}

#[test]
fn answers_to_both_questions_go_to_the_agent() {
    let mut m = asked(vec![color(), sizes()]);
    later(&mut m);
    keys(&mut m, "2");
    assert!(frame(&m).contains("question 2 of 2"));
    assert!(m.outbox.is_empty(), "nothing goes before the last answer");
    // Small, then Medium.
    keys(&mut m, " j ");
    assert!(frame(&m).contains("[x] Medium"));
    press(&mut m, KeyCode::Enter);
    assert_eq!(answers(&m), Some(vec!["Blue".into(), "Small, Medium".into()]));
    assert!(m.approval().is_none());
}

#[test]
fn enter_takes_the_option_under_the_cursor() {
    let mut m = asked(vec![color()]);
    later(&mut m);
    keys(&mut m, "j");
    press(&mut m, KeyCode::Enter);
    assert_eq!(answers(&m), Some(vec!["Blue".into()]));
}

#[test]
fn enter_with_nothing_selected_takes_the_option_under_the_cursor() {
    let mut m = asked(vec![sizes()]);
    later(&mut m);
    keys(&mut m, "jj");
    press(&mut m, KeyCode::Enter);
    assert_eq!(answers(&m), Some(vec!["Large".into()]));
}

#[test]
fn other_takes_a_typed_answer() {
    let mut m = asked(vec![color()]);
    later(&mut m);
    keys(&mut m, "3");
    keys(&mut m, "Green jq1");
    assert!(frame(&m).contains("Green jq1"));
    press(&mut m, KeyCode::Backspace);
    press(&mut m, KeyCode::Backspace);
    press(&mut m, KeyCode::Backspace);
    press(&mut m, KeyCode::Enter);
    assert_eq!(answers(&m), Some(vec!["Green".into()]));
}

#[test]
fn other_adds_to_the_selection() {
    let mut m = asked(vec![sizes()]);
    later(&mut m);
    keys(&mut m, "1");
    keys(&mut m, "4");
    keys(&mut m, "Tiny");
    press(&mut m, KeyCode::Enter);
    assert_eq!(answers(&m), Some(vec!["Small, Tiny".into()]));
}

#[test]
fn esc_closes_the_text_then_declines() {
    let mut m = asked(vec![color()]);
    later(&mut m);
    keys(&mut m, "3");
    press(&mut m, KeyCode::Enter);
    assert!(m.outbox.is_empty(), "an empty answer does not go");
    press(&mut m, KeyCode::Esc);
    assert!(m.approval().is_some_and(|a| a.pick.other.is_none()));
    press(&mut m, KeyCode::Esc);
    assert_eq!(m.outbox, [Cmd::Answer { session: 0, request_id: "req_1".into(), decision: Decision::Deny }]);
    assert!(m.approval().is_none());
}

#[test]
fn keys_typed_as_it_opens_do_not_answer_it() {
    let mut m = asked(vec![color()]);
    assert!(!press(&mut m, KeyCode::Char('1')));
    assert!(!press(&mut m, KeyCode::Enter));
    assert!(m.outbox.is_empty());
    assert!(m.approval().is_some());
}

#[test]
fn the_status_line_says_why_the_input_waits() {
    let m = asked(vec![color()]);
    let f = frame(&m);
    assert!(f.contains("waiting for your answer · Which color?"), "{f}");
}

#[test]
fn the_hint_for_the_text_field_does_not_offer_send_now() {
    let mut m = asked(vec![color()]);
    later(&mut m);
    keys(&mut m, "3");
    let f = frame(&m);
    let h = f.lines().last().unwrap();
    assert!(h.contains("Enter answers"), "{h}");
    assert!(!h.contains("send now"), "{h}");
}
