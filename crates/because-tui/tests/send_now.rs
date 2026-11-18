//! Text sent while the agent works, driven by a recording of the real CLI
//! (`fixtures/claude-stream/send-now.jsonl`). Each line that the recording sent
//! is sent again here at the same point, as the developer would.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use because_core::{AgentEvent, Decision, Fixture, SegStatus, Session};
use because_tui::{Cmd, Model, Msg, dispatch, update};

fn live() -> Model {
    let s = Session { id: "s".into(), repo: "/repo".into(), branch: "main".into(), ..Default::default() };
    let mut m = Model::new(Fixture { sessions: vec![s] });
    m.live = true;
    m
}

fn replay() -> (Model, Vec<Cmd>) {
    let mut m = live();
    let mut cmds = Vec::new();
    let text = std::fs::read_to_string("../../fixtures/claude-stream/send-now.jsonl").unwrap();
    for line in text.lines() {
        match line.strip_prefix("# sent: ").map(|l| serde_json::from_str::<Value>(l).unwrap()) {
            Some(v) if v["type"] == "user" => {
                update(&mut m, Msg::Paste(v["message"]["content"].as_str().unwrap().into()));
                update(&mut m, Msg::Submit);
            }
            Some(_) => update(&mut m, Msg::Answer(Decision::AllowOnce)),
            None => {
                for e in because_agent::claude::parse_line(line) {
                    update(&mut m, Msg::Agent(0, e));
                }
            }
        }
        cmds.append(&mut m.outbox);
    }
    (m, cmds)
}

#[test]
fn each_text_lands_on_the_input_the_agent_read_it_in() {
    let (m, _) = replay();
    let segs: Vec<(&str, SegStatus)> =
        m.segments().iter().map(|s| (s.input.split_whitespace().last().unwrap(), s.status)).collect();
    assert_eq!(segs, [("PINEAPPLE.", SegStatus::Done), ("bees.", SegStatus::Done), ("MANGO.", SegStatus::Done)]);
    assert!(m.segments()[0].input.contains("Added while the agent worked:"));
    assert!(!m.is_running(0));
}

#[test]
fn each_text_goes_out_one_time() {
    let (_, cmds) = replay();
    let sends = cmds.iter().filter(|c| matches!(c, Cmd::Send { .. })).count();
    let now: Vec<&str> = cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::SendMidTurn { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(sends, 2);
    assert_eq!(now.len(), 2, "{now:?}");
}

// ── send now ───────────────────────────────────────────────────────────────

fn press(m: &mut Model, code: KeyCode, mods: KeyModifiers) {
    if let Some(msg) = dispatch(m, KeyEvent::new(code, mods)) {
        update(m, msg);
    }
}

fn type_text(m: &mut Model, text: &str) {
    for c in text.chars() {
        press(m, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

/// An input runs, "other way" is in the draft, and the outbox is empty.
fn drafted() -> Model {
    let mut m = live();
    type_text(&mut m, "first");
    press(&mut m, KeyCode::Enter, KeyModifiers::NONE);
    type_text(&mut m, "other way");
    m.outbox.clear();
    m
}

fn sent_now() -> [Cmd; 2] {
    [Cmd::SendMidTurn { session: 0, text: "other way".into() }, Cmd::Stop { session: 0 }]
}

#[test]
fn ctrl_enter_sends_the_draft_and_interrupts() {
    let mut m = drafted();
    press(&mut m, KeyCode::Enter, KeyModifiers::CONTROL);
    assert_eq!(m.outbox, sent_now());
    assert!(m.draft_is_empty());
}

#[test]
fn ctrl_x_ctrl_s_does_the_same() {
    let mut m = drafted();
    press(&mut m, KeyCode::Char('x'), KeyModifiers::CONTROL);
    press(&mut m, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert_eq!(m.outbox, sent_now());
}

#[test]
fn a_key_after_ctrl_x_is_not_lost() {
    let mut m = drafted();
    press(&mut m, KeyCode::Char('x'), KeyModifiers::CONTROL);
    press(&mut m, KeyCode::Char('!'), KeyModifiers::NONE);
    assert_eq!(m.draft(), "other way!");
    assert!(m.outbox.is_empty());
    // The chord is over: Ctrl+S alone does nothing.
    press(&mut m, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(m.outbox.is_empty());
}

#[test]
fn with_nothing_to_send_nothing_stops() {
    let mut m = live();
    type_text(&mut m, "first");
    press(&mut m, KeyCode::Enter, KeyModifiers::NONE);
    m.outbox.clear();
    press(&mut m, KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(m.outbox.is_empty());
    assert!(m.notice.as_deref().unwrap_or("").contains("Nothing to send"), "{:?}", m.notice);
}

#[test]
fn with_the_agent_idle_ctrl_enter_is_enter() {
    let mut m = live();
    type_text(&mut m, "first");
    press(&mut m, KeyCode::Enter, KeyModifiers::CONTROL);
    assert_eq!(m.outbox[0], Cmd::Send { session: 0, seg: 0, text: "first".into() });
}

#[test]
fn the_interrupted_input_says_why_and_the_text_runs_next() {
    let mut m = drafted();
    press(&mut m, KeyCode::Enter, KeyModifiers::CONTROL);
    let end = AgentEvent::Finished { reply: String::new(), cost_total_usd: 0.0, duration_ms: 900, is_error: true, permission_denials: 0 };
    update(&mut m, Msg::Agent(0, end));
    assert_eq!(m.segments()[0].status, SegStatus::Done);
    assert!(m.segments()[0].said().contains("Interrupted to read the input that you sent"), "{}", m.segments()[0].said());
    update(&mut m, Msg::Agent(0, AgentEvent::InputTaken("other way".into())));
    assert_eq!(m.segments().len(), 2);
    assert!(m.is_running(0));
}
