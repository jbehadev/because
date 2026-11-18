//! Key-level tests. These go through `dispatch`, not straight to `update`,
//! because the thing under test is which keys type and which keys command.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use because_core::Fixture;
use because_tui::{Focus, Model, SegFocus, View, dispatch, update};

fn model() -> Model {
    Model::new(Fixture::load("../../fixtures/ripgrep.json").expect("fixture"))
}

fn press(m: &mut Model, code: KeyCode, mods: KeyModifiers) {
    if let Some(msg) = dispatch(m, KeyEvent::new(code, mods)) {
        update(m, msg);
    }
}

fn key(m: &mut Model, code: KeyCode) {
    press(m, code, KeyModifiers::NONE);
}

fn alt(m: &mut Model, code: KeyCode) {
    press(m, code, KeyModifiers::ALT);
}

fn ctrl(m: &mut Model, c: char) {
    press(m, KeyCode::Char(c), KeyModifiers::CONTROL);
}

fn typ(m: &mut Model, text: &str) {
    for c in text.chars() {
        key(m, KeyCode::Char(c));
    }
}

#[test]
fn starts_in_the_composer() {
    let m = model();
    assert!(m.composing(), "the Segments pane should open ready to type");
}

#[test]
fn every_plain_command_key_types_while_composing() {
    let mut m = model();
    let seg = m.seg;
    // Each of these is a command outside the composer.
    typ(&mut m, "1234hjklvtWnNqia[]");
    assert_eq!(m.draft(), "1234hjklvtWnNqia[]");
    assert!(m.composing(), "focus must not have moved");
    assert!(!m.verbatim && !m.transcript && !m.rationale && !m.quit);
    assert_eq!(m.seg, seg, "segment selection must not have moved");
}

#[test]
fn alt_arrows_change_pane_and_the_draft_survives_the_round_trip() {
    let mut m = model();
    typ(&mut m, "add a test");
    alt(&mut m, KeyCode::Right);
    assert_eq!(m.focus, Focus::Changes);
    alt(&mut m, KeyCode::Left);
    assert!(m.composing(), "returns to the composer it left");
    assert_eq!(m.draft(), "add a test");
}

#[test]
fn alt_digits_jump_panes_from_inside_the_composer() {
    let mut m = model();
    typ(&mut m, "x");
    alt(&mut m, KeyCode::Up); // input 3 has changes; input 4 does not
    alt(&mut m, KeyCode::Char('1'));
    assert_eq!(m.focus, Focus::Sessions);
    alt(&mut m, KeyCode::Char('4'));
    assert_eq!((m.view, m.focus), (View::Code, Focus::Code));
    assert_eq!(m.draft(), "x");
}

#[test]
fn alt_up_down_browse_segments_without_touching_the_draft() {
    let mut m = model();
    typ(&mut m, "draft");
    alt(&mut m, KeyCode::Up);
    alt(&mut m, KeyCode::Up);
    assert_eq!(m.seg, 1);
    assert!(m.composing());
    assert_eq!(m.draft(), "draft");
}

#[test]
fn plain_arrows_browse_only_while_the_draft_is_empty() {
    let mut m = model();
    key(&mut m, KeyCode::Up);
    assert_eq!(m.seg, 2, "empty draft: Up moves the selection");
    typ(&mut m, "x");
    key(&mut m, KeyCode::Up);
    assert_eq!(m.seg, 2, "non-empty draft: Up belongs to the editor");
}

#[test]
fn ctrl_arrows_are_word_movement_in_the_composer_not_pane_movement() {
    let mut m = model();
    typ(&mut m, "hello world");
    press(&mut m, KeyCode::Left, KeyModifiers::CONTROL);
    assert!(m.composing(), "Ctrl+Left must not leave the pane");
    assert_eq!(m.composer.cursor().1, 6, "cursor should be at the start of 'world'");
}

#[test]
fn ctrl_w_deletes_a_word_rather_than_starting_a_window_chord() {
    let mut m = model();
    typ(&mut m, "hello world");
    ctrl(&mut m, 'w');
    assert_eq!(m.draft(), "hello ");
    assert!(m.composing());
}

#[test]
fn esc_hands_plain_keys_back_to_commands_and_i_resumes_typing() {
    let mut m = model();
    typ(&mut m, "draft");
    key(&mut m, KeyCode::Esc);
    assert_eq!(m.seg_focus, SegFocus::List);
    key(&mut m, KeyCode::Char('k'));
    assert_eq!(m.seg, 2, "k is a command again");
    key(&mut m, KeyCode::Char('v'));
    assert!(m.verbatim);
    key(&mut m, KeyCode::Char('i'));
    assert!(m.composing());
    assert_eq!(m.draft(), "draft", "the draft was kept while commanding");
}

#[test]
fn enter_sends_the_input_as_the_next_segment_of_the_session() {
    let mut m = model();
    assert_eq!(m.segment_count(), 4);
    typ(&mut m, "  add a test for a hidden file on Windows  ");
    key(&mut m, KeyCode::Enter);
    assert!(m.draft_is_empty());
    assert_eq!(m.segment_count(), 5, "four inputs, then a fifth");
    assert_eq!(m.seg, 4, "the new input is selected");
    assert_eq!(m.selected_queued().unwrap().text, "add a test for a hidden file on Windows");
}

#[test]
fn a_queued_input_belongs_to_its_session() {
    let mut m = model();
    typ(&mut m, "follow-up");
    key(&mut m, KeyCode::Enter);
    alt(&mut m, KeyCode::Char('1'));
    key(&mut m, KeyCode::Char('j'));
    assert_eq!(m.session, 1);
    assert_eq!(m.segment_count(), 2, "the other session is unaffected");
}

// ── starting another session ───────────────────────────────────────────────

#[test]
fn n_in_the_sessions_pane_starts_a_session_ready_to_type_into() {
    let mut m = model();
    assert_eq!(m.fx.sessions.len(), 3);
    alt(&mut m, KeyCode::Char('1'));
    key(&mut m, KeyCode::Char('n'));
    assert_eq!(m.fx.sessions.len(), 4);
    assert_eq!(m.session, 3, "the new session is selected");
    assert!(m.segments().is_empty(), "it knows nothing of the other sessions' inputs");
    assert_eq!(m.session().repo, m.fx.sessions[0].repo, "same repository");
    assert!(m.composing(), "and the cursor is already in its composer");
}

#[test]
fn alt_n_starts_one_from_the_composer_and_the_draft_comes_with_it() {
    let mut m = model();
    typ(&mut m, "this deserves its own session");
    alt(&mut m, KeyCode::Char('n'));
    assert_eq!(m.session, 3);
    assert_eq!(m.draft(), "this deserves its own session");
    assert!(m.composing());
}

#[test]
fn n_in_the_code_pane_still_moves_between_regions() {
    let mut m = model();
    alt(&mut m, KeyCode::Up); // input 3, which changed files
    alt(&mut m, KeyCode::Char('4'));
    assert_eq!(m.view, View::Code);
    let msg = dispatch(&m, KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert!(matches!(msg, Some(because_tui::Msg::NextRegion)), "plain n keeps its code-pane meaning: {msg:?}");
    key(&mut m, KeyCode::Char('n'));
    assert_eq!(m.fx.sessions.len(), 3, "and starts no session");
}

#[test]
fn enter_on_an_empty_draft_queues_nothing() {
    let mut m = model();
    key(&mut m, KeyCode::Enter);
    typ(&mut m, "   ");
    key(&mut m, KeyCode::Enter);
    assert_eq!(m.queued.len(), 0);
}

#[test]
fn alt_enter_shift_enter_and_ctrl_j_insert_newlines_instead_of_submitting() {
    let mut m = model();
    typ(&mut m, "a");
    alt(&mut m, KeyCode::Enter);
    typ(&mut m, "b");
    press(&mut m, KeyCode::Enter, KeyModifiers::SHIFT);
    typ(&mut m, "c");
    ctrl(&mut m, 'j');
    typ(&mut m, "d");
    assert_eq!(m.composer.lines(), ["a", "b", "c", "d"]);
    assert!(m.queued.is_empty());
}

#[test]
fn a_multi_line_paste_does_not_submit() {
    let mut m = model();
    update(&mut m, because_tui::Msg::Paste("thread 'main' panicked\nat src/lib.rs:12".into()));
    assert_eq!(m.composer.lines().len(), 2);
    assert!(m.queued.is_empty());
}

#[test]
fn ctrl_c_clears_a_draft_before_it_quits_and_the_clear_is_undoable() {
    let mut m = model();
    typ(&mut m, "a long careful prompt");
    ctrl(&mut m, 'c');
    assert!(m.draft_is_empty());
    assert!(!m.quit, "the first Ctrl+C must not throw a draft away and exit");
    ctrl(&mut m, 'u');
    assert_eq!(m.draft(), "a long careful prompt", "Ctrl+U undoes the clear");
    ctrl(&mut m, 'c');
    ctrl(&mut m, 'c');
    assert!(m.quit);
}

#[test]
fn q_ascends_like_esc_and_only_shift_q_quits() {
    let mut m = model();
    let mut e = model();
    for m in [&mut m, &mut e] {
        alt(m, KeyCode::Up);
        alt(m, KeyCode::Char('4'));
        assert_eq!(m.view, View::Code);
    }
    while m.view == View::Code {
        key(&mut m, KeyCode::Char('q'));
        key(&mut e, KeyCode::Esc);
        assert_eq!((m.view, m.focus, m.rationale), (e.view, e.focus, e.rationale));
    }
    assert!(!m.quit, "plain q never quits");
    press(&mut m, KeyCode::Char('Q'), KeyModifiers::SHIFT);
    assert!(m.quit);
}

#[test]
fn i_from_the_code_pane_goes_straight_to_the_composer() {
    let mut m = model();
    alt(&mut m, KeyCode::Up);
    alt(&mut m, KeyCode::Char('4'));
    assert_eq!(m.view, View::Code);
    key(&mut m, KeyCode::Char('i'));
    assert!(m.composing());
    assert_eq!(m.view, View::Columns);
}

#[test]
fn segments_pane_remembers_list_mode_across_pane_changes() {
    let mut m = model();
    key(&mut m, KeyCode::Esc);
    alt(&mut m, KeyCode::Right);
    alt(&mut m, KeyCode::Left);
    assert_eq!(m.seg_focus, SegFocus::List, "coming back does not force you into typing");
    key(&mut m, KeyCode::Char('k'));
    assert_eq!(m.seg, 2);
}

#[test]
fn a_queued_segment_becomes_the_selection_and_has_no_changes_yet() {
    let mut m = model();
    let agent_segments = m.segments().len();
    typ(&mut m, "cover the empty-variable case");
    key(&mut m, KeyCode::Enter);
    assert_eq!(m.seg, agent_segments, "the new segment is selected");
    assert!(m.selected_queued().is_some());
    assert!(m.visible_changes().is_empty());
    // moving right from Changes must not open an empty code pane
    alt(&mut m, KeyCode::Char('3'));
    alt(&mut m, KeyCode::Right);
    assert_eq!(m.view, View::Columns);
    // and the queued segment is reachable by browsing like any other
    alt(&mut m, KeyCode::Char('2'));
    alt(&mut m, KeyCode::Up);
    assert!(m.selected_queued().is_none());
    alt(&mut m, KeyCode::Down);
    assert!(m.selected_queued().is_some());
}

#[test]
fn n_stays_on_this_input_while_brackets_visit_earlier_inputs_too() {
    let mut m = model();
    alt(&mut m, KeyCode::Up); // input 3
    alt(&mut m, KeyCode::Char('4'));
    let c = m.current_change().unwrap().clone();
    let h = m.highlight(&c);
    let mut n_lands = Vec::new();
    for _ in 0..20 {
        key(&mut m, KeyCode::Char('n'));
        n_lands.push(m.code_cursor);
    }
    assert!(n_lands.iter().all(|l| !h.earlier.contains_key(l)), "n never lands on another input's line");
    let mut bracket_lands = Vec::new();
    for _ in 0..40 {
        key(&mut m, KeyCode::Char(']'));
        bracket_lands.push(m.code_cursor);
    }
    assert!(bracket_lands.iter().any(|l| h.earlier.contains_key(l)), "] reaches earlier inputs' lines");
}

// ── arrows on macOS ────────────────────────────────────────────────────────
// macOS Terminal and iTerm2 send Option+←/→ as ESC b / ESC f, which crossterm
// reports as Alt+b / Alt+f — not Alt+Left / Alt+Right. On a Mac there is no
// modifier+arrow chord that is both delivered and free: Option+arrow is word
// movement in every text field, Ctrl+arrow switches Spaces, Cmd never arrives.
// So with nothing typed every arrow form navigates, and with text they edit.

#[test]
fn plain_arrows_change_pane_from_an_empty_composer() {
    let mut m = model();
    key(&mut m, KeyCode::Right);
    assert_eq!(m.focus, Focus::Changes);
    key(&mut m, KeyCode::Left);
    assert!(m.composing(), "back into the composer it left");
    key(&mut m, KeyCode::Left);
    assert_eq!(m.focus, Focus::Sessions);
}

#[test]
fn option_arrows_as_macos_terminals_send_them_move_panes_when_nothing_is_typed() {
    let mut m = model();
    alt(&mut m, KeyCode::Up); // input 3, which has changes
    alt(&mut m, KeyCode::Char('f'));
    assert_eq!(m.focus, Focus::Changes);
    alt(&mut m, KeyCode::Char('f'));
    assert_eq!(m.view, View::Code);
    alt(&mut m, KeyCode::Char('b'));
    assert_eq!((m.view, m.focus), (View::Columns, Focus::Changes));
    alt(&mut m, KeyCode::Char('b'));
    assert!(m.composing());
    alt(&mut m, KeyCode::Char('b'));
    assert_eq!(m.focus, Focus::Sessions);
}

#[test]
fn with_text_in_the_draft_arrows_and_option_arrows_edit_instead() {
    let mut m = model();
    typ(&mut m, "hello world");
    alt(&mut m, KeyCode::Char('b'));
    assert!(m.composing());
    assert_eq!(m.composer.cursor().1, 6, "Option+← is word-left, as in any macOS text field");
    key(&mut m, KeyCode::Left);
    assert_eq!(m.composer.cursor().1, 5);
    key(&mut m, KeyCode::Right);
    assert_eq!(m.composer.cursor().1, 6);
    assert!(m.composing());
}

#[test]
fn tab_moves_panes_while_typing_and_keeps_the_draft() {
    let mut m = model();
    typ(&mut m, "half a thought");
    key(&mut m, KeyCode::Tab);
    assert_eq!(m.focus, Focus::Changes);
    press(&mut m, KeyCode::BackTab, KeyModifiers::SHIFT);
    assert!(m.composing());
    assert_eq!(m.draft(), "half a thought");
}

// ── the code stop exists only when there is code ───────────────────────────
// The app opens on the latest input, which in the fixture changed no files.
// The code stop used to stay in the Tab cycle anyway and trap it: Tab stuck on
// Changes, Shift+Tab stuck on Sessions.

#[test]
fn tab_cycles_past_the_code_stop_when_there_is_nothing_to_open() {
    let mut m = model();
    assert!(!m.can_open_code());
    key(&mut m, KeyCode::Tab);
    assert_eq!(m.focus, Focus::Changes);
    key(&mut m, KeyCode::Tab);
    assert_eq!(m.focus, Focus::Sessions, "wraps instead of sticking on Changes");
    key(&mut m, KeyCode::Tab);
    assert!(m.composing());
}

#[test]
fn shift_tab_cycles_past_the_code_stop_when_there_is_nothing_to_open() {
    let mut m = model();
    press(&mut m, KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(m.focus, Focus::Sessions);
    press(&mut m, KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(m.focus, Focus::Changes, "wraps instead of sticking on Sessions");
}

#[test]
fn tab_still_enters_code_when_the_input_changed_files() {
    let mut m = model();
    key(&mut m, KeyCode::Up); // input 3
    key(&mut m, KeyCode::Tab);
    key(&mut m, KeyCode::Tab);
    assert_eq!(m.view, View::Code);
    key(&mut m, KeyCode::Tab);
    assert_eq!((m.view, m.focus), (View::Columns, Focus::Sessions));
}

#[test]
fn asking_for_code_that_does_not_exist_says_why_and_where_to_go() {
    let mut m = model();
    key(&mut m, KeyCode::Tab);
    key(&mut m, KeyCode::Right);
    assert_eq!(m.focus, Focus::Changes);
    let n = m.notice.clone().expect("a notice");
    assert!(n.contains("input 4 changed no files"), "{n}");
    assert!(n.contains("Input 3 did"), "{n}");
    key(&mut m, KeyCode::Left);
    assert!(m.notice.is_none(), "the notice lasts one key");
}

#[test]
fn segments_stop_at_the_first_and_last_input_instead_of_wrapping() {
    let mut m = model();
    assert_eq!(m.seg, 3, "opens on the latest input");
    key(&mut m, KeyCode::Down);
    assert_eq!(m.seg, 3, "Down on the latest input stays there");
    for _ in 0..10 {
        key(&mut m, KeyCode::Up);
    }
    assert_eq!(m.seg, 0, "Up stops at the first input");
    key(&mut m, KeyCode::Esc);
    key(&mut m, KeyCode::Char('k'));
    assert_eq!(m.seg, 0, "k in list mode stops too");
    alt(&mut m, KeyCode::Up);
    assert_eq!(m.seg, 0, "and Alt+Up");
}

#[test]
fn pressing_past_the_end_keeps_the_selected_change() {
    let mut m = model();
    key(&mut m, KeyCode::Esc);
    for _ in 0..3 {
        key(&mut m, KeyCode::Char('k'));
    }
    assert_eq!(m.seg, 0, "input 1 has two changed files");
    key(&mut m, KeyCode::Char('3'));
    key(&mut m, KeyCode::Char('j'));
    assert_eq!(m.change, 1);
    key(&mut m, KeyCode::Char('2'));
    key(&mut m, KeyCode::Char('k'));
    assert_eq!((m.seg, m.change), (0, 1), "nothing moved, so the file selection stays");
}
