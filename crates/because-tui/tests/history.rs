//! Earlier sessions — from the store, or imported from Claude Code — shown
//! under this run's own, read but not continued.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use because_core::{
    AgentEvent, Commit, FileChange, Fixture, NarrationStep, Origin, Risk, SegStatus, Section, Segment, Session, StepKind,
};
use because_tui::{Cmd, Focus, Model, Msg, View, dispatch, update, view};

fn change(path: &str, content: bool) -> FileChange {
    FileChange {
        id: format!("{path}-id"),
        path: path.into(),
        change_type: "modify".into(),
        additions: 2,
        deletions: 1,
        risk: Risk::None,
        summary: if content { String::new() } else { "Stored summary.".into() },
        state: "unsummarized".into(),
        content_start_line: 1,
        content: if content { vec!["fn a() {}".into(), "fn b() {}".into()] } else { vec![] },
        hunks: vec![],
        line_origins: Default::default(),
    }
}

fn earlier(id: &str, origin: Origin, title: &str, started: &str) -> Session {
    Session {
        id: id.into(),
        title: title.into(),
        repo: "/repo".into(),
        branch: "main".into(),
        origin,
        started_at: started.into(),
        segments: vec![Segment {
            seq: 1,
            input: format!("what {id} was asked"),
            reply: format!("what {id} answered"),
            model: "claude-opus-5".into(),
            status: SegStatus::Done,
            started_at: started.into(),
            elapsed: "1m 05s".into(),
            cost_unrecorded: origin == Origin::Imported,
            sections: vec![Section::whole_response(vec![NarrationStep {
                kind: StepKind::Text,
                body: format!("what {id} answered"),
            }])],
            commit: Some(Commit { sha: "c".repeat(40), short: "ccccccc".into(), parent: "b".repeat(40), ..Default::default() }),
            link: if origin == Origin::Imported { "commit_time".into() } else { String::new() },
            changes: vec![change("src/a.rs", false)],
            code_unloaded: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn model() -> Model {
    let fx = Fixture {
        sessions: vec![
            Session { id: "now".into(), repo: "/repo".into(), branch: "main".into(), ..Default::default() },
            earlier("stored-1", Origin::Stored, "", "2026-10-01T09:00:00.000Z"),
            earlier("imported-1", Origin::Imported, "Three-column TUI", "2025-09-22T02:18:34.950Z"),
        ],
    };
    let mut m = Model::new(fx);
    m.live = true;
    m
}

fn frame(m: &Model, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| view(m, f)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(b.area.width as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn key(m: &mut Model, code: KeyCode) {
    if let Some(msg) = dispatch(m, KeyEvent::new(code, KeyModifiers::NONE)) {
        update(m, msg);
    }
}

#[test]
fn history_is_listed_under_this_runs_sessions_with_dates() {
    let m = model();
    let out = frame(&m, 150, 40);
    let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s:?} missing:\n{out}"));
    assert!(at("n new session") < at("── earlier"), "new sessions start above the history");
    assert!(at("── earlier") < at("what stored-1 was asked"));
    assert!(at("Oct 1 · 1 input · $0.00") > 0);
    assert!(at("Three-column TUI") > at("what stored-1 was asked"), "newest first, as loaded");
    assert!(out.contains("Sep 22 2025 · 1 input · $—"), "an imported input's own cost is unknown:\n{out}");
    let mut m = m;
    m.session = 2;
    assert!(frame(&m, 150, 40).contains("Segments · from Claude Code"));
    m.session = 1;
    assert!(frame(&m, 150, 40).contains("Segments · earlier session"));
}

#[test]
fn an_earlier_session_is_read_only_and_keeps_the_draft() {
    let mut m = model();
    update(&mut m, Msg::FocusAt(0));
    key(&mut m, KeyCode::Down);
    assert_eq!(m.session, 1);
    assert!(frame(&m, 150, 40).contains("earlier session · read only"));
    update(&mut m, Msg::Compose);
    for c in "carry on".chars() {
        key(&mut m, KeyCode::Char(c));
    }
    key(&mut m, KeyCode::Enter);
    assert!(m.outbox.is_empty(), "nothing is sent: {:?}", m.outbox);
    assert_eq!(m.draft(), "carry on");
    assert!(m.notice.as_deref().unwrap().contains("n starts a new session"));
    // n from here makes a session above the history, where the draft can go.
    update(&mut m, Msg::NewSession);
    assert_eq!(m.session, 1);
    assert_eq!(m.fx.sessions[1].origin, Origin::New);
    assert_eq!(m.fx.sessions[2].id, "stored-1", "history moved down, in order");
    assert_eq!(m.fx.sessions[1].repo, "/repo");
}

#[test]
fn selecting_an_earlier_input_asks_for_its_code_and_keeps_what_the_store_had() {
    let mut m = model();
    assert!(m.wants_code().is_none(), "this run's empty session needs nothing");
    m.session = 2;
    m.seg = 0;
    let (s, seg) = m.wants_code().unwrap();
    assert_eq!((s.id.as_str(), seg), ("imported-1", 0));
    m.change = 0;
    m.focus = Focus::Changes;
    let out = frame(&m, 150, 40);
    assert!(out.contains("loading the code from git"), "{out}");
    assert!(out.contains("Matched by time"), "{out}");
    key(&mut m, KeyCode::Enter);
    assert_eq!(m.view, View::Columns, "no code to show yet");
    assert!(m.notice.as_deref().unwrap().contains("Loading"));

    // A session started meanwhile moves the history down; the code still lands
    // on the input it was asked for.
    update(&mut m, Msg::NewSession);
    let id = "imported-1".to_string();
    update(&mut m, Msg::Code { session_id: id.clone(), seg: 0, changes: vec![change("src/a.rs", true)] });
    let s = m.fx.sessions.iter().find(|s| s.id == id).unwrap();
    let c = &s.segments[0].changes[0];
    assert_eq!(c.content.len(), 2);
    assert_eq!(c.summary, "Stored summary.", "generated text comes from the store, not git");
    assert!(!s.segments[0].code_unloaded);
}

#[test]
fn code_that_cannot_be_loaded_is_said_once() {
    let mut m = model();
    m.session = 1;
    update(&mut m, Msg::CodeFailed { session_id: "stored-1".into(), seg: 0, error: "bad object cccccc".into() });
    assert!(m.wants_code().is_none(), "not asked again");
    assert!(m.notice.as_deref().unwrap().contains("bad object"));
}

#[test]
fn inputs_are_written_to_the_store_as_they_change_when_there_is_one() {
    let mut m = model();
    m.persist = true;
    update(&mut m, Msg::Compose);
    for c in "hi".chars() {
        key(&mut m, KeyCode::Char(c));
    }
    key(&mut m, KeyCode::Enter);
    assert_eq!(m.outbox, [Cmd::Send { session: 0, seg: 0, text: "hi".into() }, Cmd::Persist { session: 0, seg: 0 }]);
    m.outbox.clear();
    update(
        &mut m,
        Msg::Agent(
            0,
            AgentEvent::Finished {
                reply: "hello".into(),
                cost_total_usd: 0.1,
                duration_ms: 1200,
                is_error: false,
                permission_denials: 0,
            },
        ),
    );
    assert_eq!(m.outbox, [Cmd::Finish { session: 0, seg: 0 }, Cmd::Persist { session: 0, seg: 0 }]);
    assert_eq!(m.segments()[0].duration_ms, 1200);
    m.outbox.clear();
    update(&mut m, Msg::Changes { session: 0, seg: 0, commit: Commit::default(), changes: vec![] });
    assert_eq!(m.outbox, [Cmd::Persist { session: 0, seg: 0 }]);
}

#[test]
fn a_long_history_scrolls_to_keep_the_selection_in_view() {
    let mut m = model();
    for i in 0..30 {
        m.fx.sessions.push(earlier(&format!("old-{i}"), Origin::Stored, "", "2026-09-01T00:00:00.000Z"));
    }
    update(&mut m, Msg::FocusAt(0));
    for _ in 0..40 {
        key(&mut m, KeyCode::Down);
    }
    assert_eq!(m.fx.sessions[m.session].id, "old-29", "stops at the oldest rather than wrapping to the top");
    let out = frame(&m, 150, 30);
    assert!(out.contains("what old-29 was asked"), "{out}");
}

#[test]
fn c_on_an_earlier_session_asks_the_runtime_to_revive_it() {
    let mut m = model();
    update(&mut m, Msg::FocusAt(0));
    key(&mut m, KeyCode::Down);
    key(&mut m, KeyCode::Down);
    assert_eq!(m.fx.sessions[m.session].id, "imported-1");
    key(&mut m, KeyCode::Char('c'));
    assert_eq!(m.outbox, [Cmd::Revive { session_id: "imported-1".into(), repo: "/repo".into() }]);
    assert!(m.notice.as_deref().unwrap().contains("Reviving “Three-column TUI”"), "{:?}", m.notice);
    // Asked once, however often it is pressed, and nothing is sent meanwhile.
    m.outbox.clear();
    key(&mut m, KeyCode::Char('c'));
    assert!(m.outbox.is_empty());
    assert_eq!(m.fx.sessions[2].origin, Origin::Imported, "nothing moves until the runtime answers");
    // On a session of this run, c does nothing.
    update(&mut m, Msg::FocusAt(0));
    m.session = 0;
    assert!(dispatch(&m, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)).is_none());
}

#[test]
fn a_revived_session_joins_this_runs_and_takes_inputs() {
    let mut m = model();
    m.session = 2;
    update(&mut m, Msg::Revive);
    update(&mut m, Msg::Revived { session_id: "imported-1".into() });
    assert!(m.reviving.is_empty());
    let ids: Vec<&str> = m.fx.sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["now", "imported-1", "stored-1"], "moved above the history, which keeps its order");
    assert_eq!((m.session, m.fx.sessions[1].origin), (1, Origin::New), "and still selected");
    assert!(m.notice.as_deref().unwrap().contains("Revived"), "{:?}", m.notice);
    let out = frame(&m, 150, 40);
    assert!(out.find("Three-column TUI").unwrap() < out.find("── earlier").unwrap(), "{out}");
    // Its next input is its second, sent to its own agent.
    m.outbox.clear();
    m.composer.insert_str("carry on");
    update(&mut m, Msg::Submit);
    assert_eq!(m.outbox[0], Cmd::Send { session: 1, seg: 1, text: "carry on".into() });
    assert_eq!(m.fx.sessions[1].segments[1].seq, 2);
}

#[test]
fn a_session_that_cannot_be_revived_stays_read_only_and_says_why() {
    let mut m = model();
    m.session = 1;
    update(&mut m, Msg::Revive);
    update(&mut m, Msg::ReviveFailed {
        session_id: "stored-1".into(),
        error: "No conversation found with session ID: stored-1".into(),
    });
    let notice = m.notice.clone().unwrap();
    assert!(notice.contains("Could not revive") && notice.contains("No conversation found"), "{notice}");
    assert_eq!(m.fx.sessions[1].origin, Origin::Stored);
    assert!(m.reviving.is_empty(), "it can be tried again");
    m.composer.insert_str("carry on");
    m.outbox.clear();
    update(&mut m, Msg::Submit);
    assert!(m.outbox.is_empty());
}
