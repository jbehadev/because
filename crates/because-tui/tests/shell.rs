//! A draft that starts with `!` runs as a shell command in the tree of the
//! session, as in Claude Code. The agent reads the command and its output with
//! the next input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use because_core::{Fixture, SegStatus, Session};
use because_tui::{Cmd, Model, Msg, dispatch, keymap, update};

fn live() -> Model {
    let s = Session { id: "s".into(), repo: "/repo".into(), branch: "main".into(), ..Default::default() };
    let mut m = Model::new(Fixture { sessions: vec![s] });
    m.live = true;
    m
}

fn press(m: &mut Model, k: KeyEvent) {
    if let Some(msg) = dispatch(m, k) {
        update(m, msg);
    }
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

fn done(m: &mut Model, output: &str, code: Option<i32>) {
    let seg = m.segments().len() - 1;
    update(m, Msg::ShellDone { session: 0, seg, output: output.into(), code, error: None, duration_ms: 40 });
}

fn sent(m: &Model) -> Vec<String> {
    m.outbox
        .iter()
        .filter_map(|c| match c {
            Cmd::Send { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_bang_draft_runs_as_a_command() {
    let mut m = live();
    say(&mut m, "!git status");
    assert_eq!(m.outbox.iter().filter(|c| matches!(c, Cmd::Shell { .. })).count(), 1);
    assert!(m.outbox.contains(&Cmd::Shell { session: 0, seg: 0, command: "git status".into() }), "{:?}", m.outbox);
    assert!(sent(&m).is_empty());
    let s = &m.segments()[0];
    assert!(s.is_shell());
    assert_eq!(s.status, SegStatus::Running);
    assert_eq!(s.input, "!git status");
}

#[test]
fn a_command_waits_for_the_agent() {
    let mut m = live();
    say(&mut m, "write an essay");
    m.outbox.clear();
    say(&mut m, "!ls");
    assert!(m.outbox.is_empty(), "{:?}", m.outbox);
    assert_eq!(m.draft(), "!ls");
    assert!(m.notice.as_deref().unwrap_or("").contains("Run the command when it stops"), "{:?}", m.notice);
}

#[test]
fn a_bang_with_no_command_does_nothing() {
    let mut m = live();
    say(&mut m, "!");
    assert!(m.outbox.is_empty());
    assert!(m.segments().is_empty());
    assert!(m.notice.as_deref().unwrap_or("").contains("Type a shell command"), "{:?}", m.notice);
}

#[test]
fn the_hint_tells_that_enter_runs_the_command() {
    let mut m = live();
    type_text(&mut m, "!make");
    assert!(keymap::hint(&m).contains("Enter runs the command"), "{}", keymap::hint(&m));
    let mut m = live();
    type_text(&mut m, "make");
    assert!(!keymap::hint(&m).contains("Enter runs the command"));
}

#[test]
fn the_output_becomes_the_reply() {
    let mut m = live();
    m.summarize = true;
    say(&mut m, "!echo hi");
    m.outbox.clear();
    done(&mut m, "hi\n", Some(0));
    let s = &m.segments()[0];
    assert_eq!(s.status, SegStatus::Done);
    assert_eq!(s.reply, "hi");
    assert!(m.outbox.contains(&Cmd::Finish { session: 0, seg: 0 }), "{:?}", m.outbox);
    assert!(!m.outbox.iter().any(|c| matches!(c, Cmd::Summarize { .. })), "{:?}", m.outbox);
    assert!(!m.is_running(0));
}

#[test]
fn a_failed_command_tells_its_exit_status() {
    let mut m = live();
    say(&mut m, "!false");
    done(&mut m, "oops", Some(2));
    assert_eq!(m.segments()[0].reply, "oops\nexit status 2");
}

#[test]
fn the_agent_reads_the_command_with_the_next_input_only() {
    let mut m = live();
    say(&mut m, "!cargo test");
    done(&mut m, "1 failed", Some(101));
    m.outbox.clear();
    say(&mut m, "fix the test");
    let text = &sent(&m)[0];
    assert!(
        text.starts_with("<bash-input>cargo test</bash-input>\n<bash-stdout>1 failed\nexit status 101</bash-stdout>"),
        "{text}"
    );
    assert!(text.ends_with("\n\nfix the test"), "{text}");
    assert_eq!(m.segments()[1].input, "fix the test");

    // The agent read it once.
    update(
        &mut m,
        Msg::Agent(0, because_core::AgentEvent::Finished {
            reply: "ok".into(),
            cost_total_usd: 0.0,
            duration_ms: 1,
            is_error: false,
            permission_denials: 0,
        }),
    );
    assert!(!m.is_running(0));
    m.outbox.clear();
    say(&mut m, "thanks");
    assert_eq!(sent(&m), ["thanks"]);
}

#[test]
fn a_command_that_did_not_start_fails() {
    let mut m = live();
    say(&mut m, "!nope");
    update(&mut m, Msg::ShellDone {
        session: 0,
        seg: 0,
        output: String::new(),
        code: None,
        error: Some("no such shell".into()),
        duration_ms: 0,
    });
    assert_eq!(m.segments()[0].status, SegStatus::Failed);
    m.outbox.clear();
    say(&mut m, "hello");
    assert_eq!(sent(&m), ["hello"]);
}

#[test]
fn an_input_waits_for_the_command() {
    let mut m = live();
    say(&mut m, "!sleep 5");
    m.outbox.clear();
    say(&mut m, "then this");
    assert!(sent(&m).is_empty());
    assert!(!m.outbox.iter().any(|c| matches!(c, Cmd::SendMidTurn { .. })), "{:?}", m.outbox);
    assert_eq!(m.queued.len(), 1);
    done(&mut m, "", Some(0));
    let text = sent(&m);
    assert_eq!(text.len(), 1);
    assert!(text[0].ends_with("then this"), "{}", text[0]);
    assert!(m.queued.is_empty());
}

#[test]
fn ctrl_c_stops_the_command() {
    let mut m = live();
    say(&mut m, "!npm run dev");
    m.outbox.clear();
    press(&mut m, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(m.outbox, [Cmd::Stop { session: 0 }]);
    assert!(!m.quit);
    done(&mut m, "listening on 3000", None);
    assert!(m.segments()[0].reply.ends_with("You stopped this command."), "{}", m.segments()[0].reply);
}
