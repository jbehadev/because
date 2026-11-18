//! Long responses: the Segments column shows a summary, and the reader shows
//! everything. The case that prompted it was a ~1,000-word story, cut to six
//! lines in the column with nowhere to read the rest.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use because_core::{AgentEvent, Commit, Fixture, Session};
use because_tui::{Cmd, Focus, Model, Msg, SegFocus, View, dispatch, update, view};

fn live() -> Model {
    let fx = Fixture {
        sessions: vec![Session {
            id: "s".into(),
            title: "live · repo".into(),
            repo: "/repo".into(),
            branch: "main".into(),
            segments: vec![],
            ..Default::default()
        }],
    };
    let mut m = Model::new(fx);
    m.live = true;
    m.summarize = true;
    m
}

fn key(m: &mut Model, code: KeyCode) {
    if let Some(msg) = dispatch(m, KeyEvent::new(code, KeyModifiers::NONE)) {
        update(m, msg);
    }
}

fn say(m: &mut Model, text: &str) {
    for c in text.chars() {
        key(m, KeyCode::Char(c));
    }
    key(m, KeyCode::Enter);
}

/// About 1,000 words of Markdown, ending in a line that can be looked for.
fn story() -> String {
    let mut s = String::from("# The Lighthouse\n\n");
    for p in 1..=24 {
        s.push_str(&format!(
            "Paragraph {p}. The keeper climbed the stairs again, counted the ships on the horizon, \
             trimmed the wick, and wrote the weather in the log the way her mother had taught her, \
             one careful line at a time, while the gulls argued below.\n\n"
        ));
    }
    s.push_str("- first list item\n- second list item\n\nThe last line of the story.");
    s
}

/// The agent's side of an input, stopping where the runtime's snapshot diff
/// would begin: the input is finished and its changes are still being computed.
fn respond(m: &mut Model, reply: &str, tools: &[&str]) {
    update(m, Msg::Agent(0, AgentEvent::Started { model: "claude-opus-5".into(), session_id: "s".into() }));
    for t in tools {
        update(m, Msg::Agent(0, AgentEvent::ToolUse { id: "t".into(), name: (*t).into(), target: "src/a.rs".into() }));
    }
    update(m, Msg::Agent(0, AgentEvent::Text(reply.into())));
    let done = AgentEvent::Finished {
        reply: reply.into(),
        cost_total_usd: 0.2,
        duration_ms: 37_000,
        is_error: false,
        permission_denials: 0,
    };
    update(m, Msg::Agent(0, done));
}

fn answer(m: &mut Model, reply: &str, tools: &[&str]) {
    respond(m, reply, tools);
    settle(m);
}

/// The runtime's snapshot diff, landing on the last input: nothing changed.
fn settle(m: &mut Model) {
    let seg = m.segments().len() - 1;
    let commit = Commit {
        sha: "0".repeat(40),
        short: "0000000".into(),
        subject: String::new(),
        author: String::new(),
        committed_at: String::new(),
        ..Default::default()
    };
    update(m, Msg::Changes { session: 0, seg, commit, changes: vec![] });
}

fn frame(m: &Model, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| view(m, f)).unwrap();
    let b = t.backend().buffer();
    b.content.chunks(b.area.width as usize).map(|r| r.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_long_reply_is_summarized_and_the_column_does_not_hold_it() {
    let mut m = live();
    say(&mut m, "write me a 1000 word story");
    answer(&mut m, &story(), &[]);
    assert!(m.outbox.contains(&Cmd::Summarize { session: 0, seg: 0 }), "{:?}", m.outbox);
    let out = frame(&m, 150, 36);
    assert!(out.contains("summarizing…"), "{out}");
    assert!(!out.contains("Paragraph 3"), "the reply is not shown while its summary is on the way:\n{out}");

    update(
        &mut m,
        Msg::Summary {
            session: 0,
            seg: 0,
            text: "Wrote a short story about a lighthouse keeper's routine.".into(),
            model: "claude-haiku-4-5".into(),
            cost_usd: 0.002,
        },
    );
    let out = frame(&m, 150, 36);
    assert!(out.contains("↳ Wrote a short story about a lighthouse keeper's"), "{out}");
    assert!(out.contains("Enter reads all 1,"), "{out}");
    assert!(!out.contains("Paragraph 2"), "{out}");
}

#[test]
fn the_reader_says_the_changes_are_still_being_computed() {
    let mut m = live();
    say(&mut m, "fix the bug");
    respond(&mut m, "Done.", &["Edit"]);
    update(&mut m, Msg::Read);
    let out = frame(&m, 100, 30);
    assert!(out.contains("computing changes…"), "the reader tracks the diff, not only the summary:\n{out}");

    settle(&mut m);
    let out = frame(&m, 100, 30);
    assert!(!out.contains("computing changes…"), "{out}");
}

#[test]
fn a_short_answer_is_shown_as_it_is_and_not_summarized() {
    let mut m = live();
    say(&mut m, "what is 2+2?");
    answer(&mut m, "4.", &[]);
    assert!(!m.outbox.iter().any(|c| matches!(c, Cmd::Summarize { .. })), "{:?}", m.outbox);
    assert!(frame(&m, 150, 36).contains("❝ 4."));
}

#[test]
fn work_with_tools_is_summarized_even_when_the_reply_is_short() {
    let mut m = live();
    say(&mut m, "fix the bug");
    answer(&mut m, "Done.", &["Edit"]);
    assert!(m.outbox.contains(&Cmd::Summarize { session: 0, seg: 0 }));
}

#[test]
fn without_a_summarizer_no_summary_is_requested() {
    let mut m = live();
    m.summarize = false;
    say(&mut m, "write me a 1000 word story");
    answer(&mut m, &story(), &[]);
    assert!(!m.outbox.iter().any(|c| matches!(c, Cmd::Summarize { .. })));
}

#[test]
fn when_the_summary_fails_the_column_shows_two_lines_of_the_reply_and_says_why() {
    let mut m = live();
    say(&mut m, "write me a 1000 word story");
    answer(&mut m, &story(), &[]);
    update(&mut m, Msg::SummaryFailed { session: 0, seg: 0, error: "claude printed nothing".into() });
    let out = frame(&m, 150, 36);
    assert!(out.contains("No summary for input 1: claude printed nothing"), "{out}");
    assert!(out.contains("❝ The Lighthouse Paragraph 1."), "markup is dropped from the preview:\n{out}");
    assert!(!out.contains("Paragraph 2."), "two lines at most:\n{out}");
}

#[test]
fn enter_on_an_empty_draft_reads_the_whole_response_and_esc_comes_back() {
    let mut m = live();
    say(&mut m, "write me a 1000 word story");
    answer(&mut m, &story(), &[]);
    assert!(m.composing() && m.draft_is_empty());
    key(&mut m, KeyCode::Enter);
    assert_eq!(m.view, View::Reader);

    let out = frame(&m, 120, 30);
    assert!(out.contains("› write me a 1000 word story"), "{out}");
    assert!(out.contains("The Lighthouse"), "{out}");
    assert!(!out.contains("# The Lighthouse"), "headings are rendered, not shown as markup:\n{out}");
    assert!(!out.contains("The last line of the story."), "{out}");

    // The composer is below the reader and has the keyboard, so End and not G.
    key(&mut m, KeyCode::End);
    let out = frame(&m, 120, 30);
    assert!(out.contains("The last line of the story."), "{out}");
    assert!(out.contains("• second list item"), "{out}");
    assert!(out.contains(" end "), "{out}");

    key(&mut m, KeyCode::Esc);
    assert_eq!(m.view, View::Columns);
    assert_eq!((m.focus, m.seg_focus), (Focus::Segments, SegFocus::Composer));
}

#[test]
fn the_reader_scrolls_by_line_and_page_and_stops_at_both_ends() {
    let mut m = live();
    say(&mut m, "write me a 1000 word story");
    answer(&mut m, &story(), &[]);
    key(&mut m, KeyCode::Esc);
    update(&mut m, Msg::Read);
    let _ = frame(&m, 100, 30);
    let (max, _) = m.reader_extent.get();
    assert!(max > 30, "a 1,000-word story is several pages at 100 columns: max {max}");

    key(&mut m, KeyCode::Char('j'));
    key(&mut m, KeyCode::Down);
    assert_eq!(m.reader_offset(), 2);
    key(&mut m, KeyCode::Char(' '));
    let after_page = m.reader_offset();
    assert!(after_page > 20, "{after_page}");
    key(&mut m, KeyCode::Char('b'));
    assert_eq!(m.reader_offset(), 2);
    key(&mut m, KeyCode::Up);
    key(&mut m, KeyCode::Up);
    key(&mut m, KeyCode::Up);
    assert_eq!(m.reader_offset(), 0, "stops at the top");
    for _ in 0..200 {
        key(&mut m, KeyCode::Char('j'));
    }
    assert_eq!(m.reader_offset(), max, "stops at the end");
    key(&mut m, KeyCode::Char('k'));
    assert_eq!(m.reader_offset(), max - 1, "one line up from the end, not from past it");
    key(&mut m, KeyCode::Char('g'));
    assert_eq!(m.reader_offset(), 0);
}

#[test]
fn brackets_move_the_reader_between_inputs() {
    let mut m = live();
    say(&mut m, "first question");
    answer(&mut m, "first answer", &[]);
    say(&mut m, "second question");
    answer(&mut m, "second answer", &[]);
    key(&mut m, KeyCode::Esc);
    update(&mut m, Msg::Read);
    assert!(frame(&m, 100, 30).contains("second answer"));
    key(&mut m, KeyCode::Char('['));
    let out = frame(&m, 100, 30);
    assert!(out.contains("first answer") && out.contains("input 1/2"), "{out}");
    key(&mut m, KeyCode::Char('['));
    assert_eq!(m.seg, 0, "stops at the first input");
    key(&mut m, KeyCode::Char(']'));
    assert_eq!(m.seg, 1);
}

#[test]
fn enter_in_changes_reads_an_input_that_changed_no_files() {
    let mut m = live();
    say(&mut m, "what is 2+2?");
    answer(&mut m, "4.", &[]);
    update(&mut m, Msg::FocusAt(2));
    assert!(frame(&m, 150, 36).contains("reads the answer in full"));
    key(&mut m, KeyCode::Enter);
    assert_eq!(m.view, View::Reader);
    key(&mut m, KeyCode::Esc);
    assert_eq!((m.view, m.focus), (View::Columns, Focus::Changes));
}

#[test]
fn moving_to_another_pane_closes_the_reader() {
    let mut m = live();
    say(&mut m, "what is 2+2?");
    answer(&mut m, "4.", &[]);
    update(&mut m, Msg::Read);
    key(&mut m, KeyCode::Tab);
    assert_eq!((m.view, m.focus), (View::Columns, Focus::Changes));
    update(&mut m, Msg::Read);
    key(&mut m, KeyCode::Char('h'));
    assert_eq!((m.view, m.focus), (View::Columns, Focus::Changes));
}

#[test]
fn alt_r_reads_from_the_composer_without_losing_the_draft() {
    let mut m = live();
    say(&mut m, "what is 2+2?");
    answer(&mut m, "4.", &[]);
    for c in "half a thought".chars() {
        key(&mut m, KeyCode::Char(c));
    }
    let alt_r = dispatch(&m, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT)).unwrap();
    update(&mut m, alt_r);
    assert_eq!(m.view, View::Reader);
    key(&mut m, KeyCode::Esc);
    assert_eq!(m.draft(), "half a thought");
    assert!(m.composing());
}

#[test]
fn the_reader_follows_an_input_while_it_streams() {
    let mut m = live();
    say(&mut m, "write me a 1000 word story");
    update(&mut m, Msg::Agent(0, AgentEvent::Started { model: "claude-opus-5".into(), session_id: "s".into() }));
    update(&mut m, Msg::Agent(0, AgentEvent::TextDelta(story())));
    update(&mut m, Msg::Read);
    let out = frame(&m, 100, 30);
    assert!(out.contains("The last line of the story."), "opens at the end of a running input:\n{out}");
    key(&mut m, KeyCode::Home);
    assert!(!frame(&m, 100, 30).contains("The last line of the story."));
}

#[test]
fn a_queued_input_has_nothing_to_read_yet() {
    let mut m = live();
    // Only the sample view queues: a live agent gets the text at once.
    m.live = false;
    say(&mut m, "first");
    say(&mut m, "second, queued");
    assert!(m.selected_queued().is_some());
    update(&mut m, Msg::Read);
    assert_eq!(m.view, View::Columns);
    assert_eq!(m.notice.as_deref(), Some("Nothing to read yet: this input has not run."));
}

#[test]
fn the_generated_tag_never_runs_off_the_edge() {
    for w in [60u16, 80, 100, 150] {
        let mut m = live();
        say(&mut m, "write me a 1000 word story");
        answer(&mut m, &story(), &[]);
        let text = "Agent wrote a 600-word story about a lighthouse keeper named Ansel who saves a fishing boat during a storm using his signal lamp and foghorn.";
        update(&mut m, Msg::Summary { session: 0, seg: 0, text: text.into(), model: "claude-haiku-4-5-20251001".into(), cost_usd: 0.0013 });
        update(&mut m, Msg::Read);
        let out = frame(&m, w, 30);
        assert!(out.contains("[generated · claude-haiku-4-5-20251001 · $0.0013]"), "width {w}:\n{out}");
    }
}

#[test]
fn the_reader_on_the_latest_input_has_the_composer_and_sends_from_it() {
    let mut m = live();
    say(&mut m, "write me a 1000 word story");
    answer(&mut m, &story(), &[]);
    key(&mut m, KeyCode::Enter);
    assert_eq!(m.view, View::Reader);
    assert!(m.composing(), "opened from the composer, the composer keeps the keyboard");
    let out = frame(&m, 120, 30);
    assert!(out.contains("✎ next input"), "{out}");

    key(&mut m, KeyCode::PageDown);
    assert!(m.reader_offset() > 0, "the page keys scroll the reader above");
    for c in "now make it shorter".chars() {
        key(&mut m, KeyCode::Char(c));
    }
    assert_eq!(m.view, View::Reader);
    assert!(frame(&m, 120, 30).contains("now make it shorter"));

    key(&mut m, KeyCode::Enter);
    assert_eq!(m.view, View::Columns, "sending goes back to the columns");
    assert!(m.composing() && m.draft_is_empty());
    assert_eq!(m.segments().len(), 2);
    assert_eq!(m.seg, 1, "the new input is selected");
    assert!(m.outbox.contains(&Cmd::Send { session: 0, seg: 1, text: "now make it shorter".into() }), "{:?}", m.outbox);
}

#[test]
fn from_the_list_the_reader_commands_until_i_moves_into_the_composer() {
    let mut m = live();
    say(&mut m, "write me a 1000 word story");
    answer(&mut m, &story(), &[]);
    key(&mut m, KeyCode::Esc);
    key(&mut m, KeyCode::Enter);
    assert_eq!(m.view, View::Reader);
    assert!(!m.composing());
    assert!(frame(&m, 120, 30).contains("✎ next input · i to type"));
    key(&mut m, KeyCode::Char('j'));
    assert_eq!(m.reader_offset(), 1, "j scrolls");

    key(&mut m, KeyCode::Char('i'));
    assert_eq!(m.view, View::Reader, "i types into the composer here");
    assert!(m.composing());
    key(&mut m, KeyCode::Char('j'));
    assert_eq!(m.draft(), "j");
    key(&mut m, KeyCode::Esc);
    assert_eq!(m.view, View::Columns);
    assert_eq!(m.draft(), "j", "the draft comes back with you");
}

#[test]
fn an_earlier_input_in_the_reader_has_no_composer() {
    let mut m = live();
    say(&mut m, "first question");
    answer(&mut m, "first answer", &[]);
    say(&mut m, "second question");
    answer(&mut m, "second answer", &[]);
    key(&mut m, KeyCode::Up);
    key(&mut m, KeyCode::Enter);
    assert_eq!((m.view, m.seg), (View::Reader, 0));
    assert!(!m.reader_has_composer() && !m.composing());
    assert!(!frame(&m, 100, 30).contains("next input"));
    key(&mut m, KeyCode::Char(']'));
    assert!(m.reader_has_composer(), "stepping onto the latest input brings it");
}
