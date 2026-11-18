//! The allowance bar at the foot of the Sessions column.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use because_core::{Fixture, Session, Usage, UsageWindow};
use because_tui::{Model, Msg, update, view};

fn live() -> Model {
    let fx = Fixture {
        sessions: vec![Session {
            id: "s".into(),
            title: String::new(),
            repo: "/repo".into(),
            branch: "main".into(),
            segments: vec![],
            ..Default::default()
        }],
    };
    let mut m = Model::new(fx);
    m.live = true;
    m
}

fn frame(m: &Model) -> String {
    let mut t = Terminal::new(TestBackend::new(150, 36)).unwrap();
    t.draw(|f| view(m, f)).unwrap();
    let b = t.backend().buffer();
    b.content
        .chunks(b.area.width as usize)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn window(key: &str, label: &str, used: f64) -> UsageWindow {
    UsageWindow { key: key.into(), label: label.into(), used_pct: used, resets_at: String::new() }
}

/// The last row of the Sessions column, so "at the foot of the column" is tested
/// as placement and not only as presence.
fn first_column_tail(out: &str, rows: usize) -> String {
    let mut tail: Vec<String> = out
        .lines()
        .rev()
        .skip(2) // the status line and the column's bottom border
        .take(rows)
        .map(|l| l.chars().take(33).collect::<String>())
        .collect();
    tail.reverse();
    tail.join("\n")
}

#[test]
fn shows_what_is_left_not_what_is_spent() {
    let mut m = live();
    update(&mut m, Msg::Usage(Usage { windows: vec![window("five_hour", "5h", 28.0)] }));
    let tail = first_column_tail(&frame(&m), 2);
    assert!(tail.contains("5h"), "the window is labelled:\n{tail}");
    assert!(tail.contains("72%"), "28% used reads as 72% left:\n{tail}");
    assert!(tail.contains('█') && tail.contains('░'), "and as a bar:\n{tail}");
}

#[test]
fn one_bar_per_window_in_the_order_they_bite() {
    let mut m = live();
    update(
        &mut m,
        Msg::Usage(Usage {
            windows: vec![window("five_hour", "5h", 10.0), window("seven_day", "week", 60.0)],
        }),
    );
    let tail = first_column_tail(&frame(&m), 3);
    let lines: Vec<&str> = tail.lines().collect();
    assert!(lines[0].contains("usage left"), "a heading:\n{tail}");
    assert!(lines[1].contains("5h") && lines[1].contains("90%"), "{tail}");
    assert!(lines[2].contains("week") && lines[2].contains("40%"), "{tail}");
}

/// A fuller bar than the one above it would read as more headroom than there is.
#[test]
fn a_nearly_spent_window_draws_a_nearly_empty_bar() {
    let mut m = live();
    update(&mut m, Msg::Usage(Usage { windows: vec![window("five_hour", "5h", 97.0)] }));
    let tail = first_column_tail(&frame(&m), 2);
    let bar = tail.lines().last().unwrap();
    assert!(bar.contains("3%"), "{tail}");
    assert_eq!(bar.matches('█').count(), 0, "nothing left reads as nothing left:\n{tail}");
}

#[test]
fn says_it_is_checking_before_the_first_answer() {
    assert!(frame(&live()).contains("checking…"));
}

/// Why it failed does not fit the column, so it is said once in full where
/// there is room for it, and the column only has to say that there is no number.
#[test]
fn a_failure_is_explained_once_in_the_status_line() {
    let mut m = live();
    update(&mut m, Msg::UsageFailed("no Claude login to read the allowance with".into()));
    let out = frame(&m);
    assert!(first_column_tail(&out, 2).contains("unavailable"), "{out}");
    assert!(m.notice.as_deref().unwrap_or("").contains("no Claude login"), "{:?}", m.notice);

    // Only once: a poller failing every two minutes must not hold the status
    // line against everything else that wants to say something there.
    m.notice = None;
    update(&mut m, Msg::UsageFailed("still no".into()));
    assert_eq!(m.notice, None);
}

/// A reading that stopped refreshing is better than no reading, as long as the
/// screen admits it is old.
#[test]
fn a_failed_poll_keeps_the_last_number_and_marks_it_stale() {
    let mut m = live();
    update(&mut m, Msg::Usage(Usage { windows: vec![window("five_hour", "5h", 28.0)] }));
    update(&mut m, Msg::UsageFailed("network is down".into()));
    let tail = first_column_tail(&frame(&m), 2);
    assert!(tail.contains("72%"), "the last reading is still there:\n{tail}");
    assert!(tail.contains("stale"), "and marked as old:\n{tail}");
}

/// The fixture view does no I/O, so it has no allowance to report and takes no
/// rows saying so.
#[test]
fn the_fixture_view_has_no_bar() {
    let m = Model::new(Fixture::load("../../fixtures/ripgrep.json").expect("fixture"));
    let out = frame(&m);
    assert!(!out.contains("usage left"), "{out}");
    assert!(!out.contains("checking…"), "{out}");
}

/// The colour of the first filled cell of the bar for a window with `used` spent.
fn bar_color(used: f64) -> ratatui::style::Color {
    let mut m = live();
    update(&mut m, Msg::Usage(Usage { windows: vec![window("five_hour", "5h", used)] }));
    let mut t = Terminal::new(TestBackend::new(150, 36)).unwrap();
    t.draw(|f| view(&m, f)).unwrap();
    let b = t.backend().buffer();
    b.content.iter().find(|c| c.symbol() == "█").expect("a filled cell").fg
}

/// The colour moves with every reading as the window drains, not only at a
/// couple of cliffs, so 80% and 50% left already look different.
#[test]
fn the_bar_changes_colour_as_it_drains() {
    let colors: Vec<_> = [5.0, 20.0, 50.0, 70.0, 85.0].into_iter().map(bar_color).collect();
    for pair in colors.windows(2) {
        assert_ne!(pair[0], pair[1], "{colors:?}");
    }
}
