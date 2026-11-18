//! Headless view tests.
//!
//! The point of keeping `update` pure and `view` free of I/O is that the whole
//! UI can be exercised without a terminal. These run in CI with no TTY.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use because_core::Fixture;
use because_tui::{Focus, Model, Msg, View, column_layout, update, view};

fn model() -> Model {
    Model::new(Fixture::load("../../fixtures/ripgrep.json").expect("fixture"))
}

/// The four-input session, with input `n` (1-based) selected.
fn at_input(n: usize) -> Model {
    let mut m = model();
    m.seg = n - 1;
    m
}

fn frame(m: &Model, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| view(m, f)).unwrap();
    let buf = term.backend().buffer();
    let w = buf.area.width as usize;
    buf.content
        .chunks(w)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply(m: &mut Model, msgs: &[Msg]) {
    for msg in msgs {
        update(m, msg.clone());
    }
}

fn open_code(m: &mut Model) {
    apply(m, &[Msg::FocusAt(2), Msg::FocusRight]);
    assert_eq!(m.view, View::Code, "expected the code pane to open");
}

#[test]
fn four_inputs_make_four_segments() {
    let m = model();
    assert_eq!(m.segments().len(), 4);
    let out = frame(&m, 150, 40);
    for (n, start) in [(1, "before I add"), (2, "now add a variant"), (3, "pathutil still"), (4, "does is_hidden_path")] {
        assert!(out.contains(&format!("{n} › {start}")), "input {n} missing:\n{out}");
    }
}

#[test]
fn starts_on_the_most_recent_input() {
    let m = model();
    assert_eq!(m.seg, 3);
}

#[test]
fn switching_sessions_selects_that_sessions_latest_input() {
    let mut m = model();
    apply(&mut m, &[Msg::FocusAt(0), Msg::Down]);
    assert_eq!(m.session, 1);
    assert_eq!(m.seg, m.segments().len() - 1);
}

#[test]
fn focus_ring_crosses_into_the_code_view_and_back() {
    let mut m = at_input(1);
    open_code(&mut m);
    assert_eq!(m.focus, Focus::Code);
    apply(&mut m, &[Msg::FocusLeft]);
    assert_eq!((m.view, m.focus), (View::Columns, Focus::Changes));
}

#[test]
fn code_cursor_is_retained_per_change() {
    let mut m = at_input(3);
    open_code(&mut m);
    let entered = m.code_cursor;
    apply(&mut m, &[Msg::Down, Msg::Down]);
    assert_eq!(m.code_cursor, entered + 2);
    apply(&mut m, &[Msg::FocusLeft, Msg::FocusRight]);
    assert_eq!(m.code_cursor, entered + 2, "returning should restore the cursor");
}

#[test]
fn a_later_input_shows_which_earlier_input_wrote_each_line() {
    let m = at_input(3);
    let c = m.current_change().unwrap().clone();
    let h = m.highlight(&c);
    assert!(!h.selected.is_empty(), "input 3 wrote lines here");
    let from: std::collections::BTreeSet<usize> = h.earlier.values().copied().collect();
    assert_eq!(from, [0, 1].into(), "lines from inputs 1 and 2 survive in input 3's version");
    assert!(h.earlier.keys().all(|l| !h.selected.contains(l)), "a line has one author");
}

#[test]
fn the_first_input_has_no_earlier_lines() {
    let m = at_input(1);
    for c in m.visible_changes() {
        assert!(m.highlight(c).earlier.is_empty(), "{}", c.path);
    }
}

#[test]
fn why_resolves_a_line_to_the_input_that_wrote_it() {
    let mut m = at_input(3);
    open_code(&mut m);
    let c = m.current_change().unwrap().clone();
    let (&line, _) = m.highlight(&c).earlier.iter().find(|(_, idx)| **idx == 1).expect("a line from input 2");
    let r = m.resolve_line(&c, line).expect("resolves");
    assert_eq!(r.seg_index, 1);
    assert_eq!(r.short, "6e527e9");
    assert!(!r.sections.is_empty(), "the hunk that wrote it is cited by a section");

    m.code_cursor = line;
    apply(&mut m, &[Msg::ToggleRationale]);
    let out = frame(&m, 150, 44);
    assert!(out.contains("via 6e527e9"), "{out}");
    assert!(out.contains("an earlier input"), "{out}");
    assert!(out.contains("› now add a variant"), "the earlier input's text is shown:\n{out}");
}

#[test]
fn a_hunk_shared_by_several_sections_resolves_to_all_of_them() {
    // c55818f is a single hunk at -U3 that contains three sections' work.
    let m = at_input(3);
    let c = m.current_change().unwrap().clone();
    let line = *m.highlight(&c).selected.first().unwrap();
    assert_eq!(m.resolve_line(&c, line).unwrap().sections.len(), 3);
}

#[test]
fn lines_older_than_the_session_resolve_to_nothing() {
    let mut m = at_input(3);
    open_code(&mut m);
    let c = m.current_change().unwrap().clone();
    assert!(m.resolve_line(&c, 1).is_none(), "line 1 predates every input in the session");
    m.code_cursor = 1;
    apply(&mut m, &[Msg::ToggleRationale]);
    let out = frame(&m, 150, 44);
    assert!(out.contains("not changed by any input in this session"), "{out}");
}

#[test]
fn an_input_answered_without_changes_has_no_code_pane() {
    let mut m = at_input(4);
    assert!(m.visible_changes().is_empty());
    apply(&mut m, &[Msg::FocusAt(2), Msg::FocusRight]);
    assert_eq!(m.view, View::Columns, "no change, so nothing to open");
    assert!(frame(&m, 150, 40).contains("no file changes"));
}

#[test]
fn code_pane_marks_this_input_earlier_inputs_and_deletions() {
    let mut m = at_input(3);
    open_code(&mut m);
    let out = frame(&m, 150, 44);
    assert!(out.contains("as of input 3"), "revision must be named:\n{out}");
    assert!(out.contains(" ▌"), "this input's lines:\n{out}");
    assert!(out.contains("2┊"), "input 2's surviving lines:\n{out}");
    assert!(out.contains(" ⌄ −"), "collapsed deletion marker:\n{out}");
}

#[test]
fn rationale_panel_separates_generated_from_verbatim_and_human() {
    let mut m = at_input(2);
    open_code(&mut m);
    apply(&mut m, &[Msg::ToggleRationale]);
    let out = frame(&m, 150, 44);
    assert!(out.contains("[generated]"), "generated text must be marked:\n{out}");
    assert!(out.contains("❝ "), "the model's own words must be marked:\n{out}");
    assert!(out.contains("input      ›"), "the developer's input must be shown:\n{out}");
    assert!(out.contains("(this input)"), "{out}");
}

#[test]
fn every_input_in_every_session_renders_without_panicking() {
    let mut m = model();
    for s in 0..m.fx.sessions.len() {
        m.session = s;
        for i in 0..m.segments().len() {
            m.seg = i;
            m.view = View::Columns;
            m.focus = Focus::Segments;
            for w in [80u16, 100, 150] {
                let _ = frame(&m, w, 30);
            }
            for c in 0..m.visible_changes().len() {
                m.change = c;
                open_code(&mut m);
                apply(&mut m, &[Msg::ToggleRationale]);
                for w in [80u16, 150] {
                    let _ = frame(&m, w, 40);
                }
                apply(&mut m, &[Msg::Back, Msg::Back]);
            }
            m.change = 0;
        }
    }
}

/// Rebuild the selected input's text from the rendered Segments column and
/// check nothing was clipped against the border. A long input is cut to a few
/// lines on purpose, and then it must end in an ellipsis.
#[test]
fn the_selected_input_is_never_clipped_at_any_width() {
    for w in [50u16, 64, 80, 90, 100, 120, 150, 200] {
        let m = at_input(2);
        let out = frame(&m, w, 40);
        let rows: Vec<Vec<char>> = out.lines().map(|l| l.chars().collect()).collect();
        let top: String = rows[0].iter().collect();
        let x0 = top.find("┌ 2 Segments").map(|b| top[..b].chars().count()).expect("column");
        let x1 = x0 + rows[0][x0..].iter().position(|&c| c == '┐').unwrap();
        let cells: Vec<String> = rows.iter().map(|r| r[x0 + 1..x1].iter().collect()).collect();
        let first = cells.iter().position(|c| c.starts_with("▸  2 › ")).expect("selected row");
        let mut text = vec![cells[first]["▸  2 › ".len()..].trim_end().to_string()];
        for c in &cells[first + 1..] {
            if c.starts_with("       ") && !c.trim_start().starts_with('↳') {
                text.push(c.trim().to_string());
            } else {
                break;
            }
        }
        let got = text.join(" ");
        let want = m.selected_segment().unwrap().input.split_whitespace().collect::<Vec<_>>().join(" ");
        match got.strip_suffix('…') {
            Some(cut) => assert!(want.starts_with(cut.trim_end()), "clipped at width {w}: {got:?}"),
            None => assert_eq!(got, want, "clipped at width {w}"),
        }
        if w >= 150 {
            assert_eq!(got, want, "an 18-word input fits in four lines at width {w}");
        }
    }
}

/// The numbered pane titles across the top row, in screen order.
fn titles(out: &str) -> Vec<&'static str> {
    let top = out.lines().next().unwrap_or_default();
    ["1 Sessions", "2 Segments", "3 Changes"]
        .into_iter()
        .filter_map(|t| top.find(&format!("┌ {t}")).map(|x| (x, t)))
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// Narrowing the terminal — or making its font larger — takes width from
/// Changes and then whole columns, never from Segments first.
#[test]
fn columns_give_way_to_segments_as_the_terminal_narrows() {
    let mut m = at_input(4);
    m.focus = Focus::Segments;
    assert_eq!(titles(&frame(&m, 150, 30)), ["1 Sessions", "2 Segments", "3 Changes"]);
    assert_eq!(titles(&frame(&m, 110, 30)), ["1 Sessions", "2 Segments", "3 Changes"]);
    assert_eq!(titles(&frame(&m, 80, 30)), ["2 Segments", "3 Changes"]);
    assert_eq!(titles(&frame(&m, 50, 30)), ["2 Segments"]);

    m.focus = Focus::Changes;
    assert_eq!(titles(&frame(&m, 80, 30)), ["2 Segments", "3 Changes"]);
    assert_eq!(titles(&frame(&m, 50, 30)), ["3 Changes"]);

    m.focus = Focus::Sessions;
    assert_eq!(titles(&frame(&m, 80, 30)), ["1 Sessions", "2 Segments"]);
    assert_eq!(titles(&frame(&m, 50, 30)), ["1 Sessions"]);
}

/// Whatever the width, the focused column is on screen and Segments is at
/// least as wide as Changes beside it.
#[test]
fn the_focused_column_is_always_shown_and_segments_is_never_the_narrowest() {
    for w in 30u16..=220 {
        for focus in [Focus::Sessions, Focus::Segments, Focus::Changes] {
            let cols = column_layout(w, focus);
            assert!(cols.iter().any(|&(c, _)| c == focus), "{focus:?} hidden at width {w}");
            let areas = ratatui::layout::Layout::horizontal(cols.iter().map(|&(_, c)| c))
                .split(ratatui::layout::Rect::new(0, 0, w, 1));
            let width_of = |f: Focus| cols.iter().position(|&(c, _)| c == f).map(|i| areas[i].width);
            assert_eq!(areas.iter().map(|r| r.width).sum::<u16>(), w, "columns fill width {w}");
            if let (Some(seg), Some(chg)) = (width_of(Focus::Segments), width_of(Focus::Changes)) {
                assert!(seg >= chg, "Segments {seg} narrower than Changes {chg} at width {w}");
            }
        }
    }
}

/// Moving through the columns at a narrow width brings the hidden ones back.
#[test]
fn a_column_off_screen_is_one_key_away() {
    let mut m = at_input(4);
    m.focus = Focus::Segments;
    assert!(!frame(&m, 80, 30).contains("1 Sessions"));
    apply(&mut m, &[Msg::FocusAt(0)]);
    let out = frame(&m, 80, 30);
    assert!(out.contains("1 Sessions") && out.contains("ripgrep · master"), "{out}");
    apply(&mut m, &[Msg::FocusAt(2)]);
    assert!(titles(&frame(&m, 80, 30)).contains(&"3 Changes"));
}

/// With the focus on Sessions at a narrow width, its names get more room
/// than the 26 cells they have beside two other columns.
#[test]
fn a_focused_sessions_column_widens_when_it_stands_beside_one_other() {
    let mut m = at_input(4);
    m.focus = Focus::Sessions;
    let (name, _) = because_tui::session_name(&m, 1);
    let shown = |out: &str| out.lines().filter(|l| l.contains(&name[..12])).map(|l| l.split('│').nth(1).unwrap_or("").trim().to_string()).next();
    let wide = shown(&frame(&m, 95, 30)).expect("session listed");
    let three = shown(&frame(&m, 110, 30)).expect("session listed");
    assert!(wide.chars().count() > three.chars().count(), "{wide:?} vs {three:?}");
}

#[test]
fn a_long_next_input_wraps_onto_more_rows_instead_of_a_horizontal_scroll() {
    let mut m = model();
    // Each word is unique, so the test can find each one on the screen.
    let words: Vec<String> = (1..=30).map(|i| format!("word{i:02}")).collect();
    update(&mut m, Msg::Paste(words.join(" ")));
    let out = frame(&m, 150, 40);
    for w in &words {
        assert!(out.contains(w.as_str()), "{w} is not on the screen:\n{out}");
    }
}
