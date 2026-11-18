//! The mark for work not committed yet, in the Sessions column.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use because_core::{Fixture, Origin, Session};
use because_tui::{Model, Msg, update, view};

fn session(id: &str, repo: &str, origin: Origin) -> Session {
    Session { id: id.into(), title: id.into(), repo: repo.into(), branch: "main".into(), origin, ..Default::default() }
}

fn model() -> Model {
    let fx = Fixture {
        sessions: vec![
            session("first", "/repo", Origin::New),
            session("second", "/repo.sessions/2-abcdef", Origin::New),
            session("earlier", "/repo", Origin::Stored),
        ],
    };
    let mut m = Model::new(fx);
    m.live = true;
    m
}

/// The Sessions column, row by row.
fn column(m: &Model) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(150, 36)).unwrap();
    t.draw(|f| view(m, f)).unwrap();
    let b = t.backend().buffer();
    b.content
        .chunks(b.area.width as usize)
        .map(|r| r.iter().take(33).map(|c| c.symbol()).collect::<String>())
        .collect()
}

/// The meta line under a session's name.
fn meta_of(rows: &[String], name: &str) -> String {
    let i = rows.iter().position(|r| r.contains(name)).unwrap_or_else(|| panic!("{name} not listed:\n{}", rows.join("\n")));
    rows[i + 1].clone()
}

#[test]
fn nothing_is_marked_before_the_first_answer() {
    let rows = column(&model());
    assert!(!rows.iter().any(|r| r.contains('±')), "{}", rows.join("\n"));
}

#[test]
fn a_tree_with_uncommitted_files_is_marked_once_on_its_first_session() {
    let mut m = model();
    let n = [("/repo", 3), ("/repo.sessions/2-abcdef", 0), ("/repo.sessions/3-fedcba", 2)];
    update(&mut m, Msg::Uncommitted(n.iter().map(|(d, n)| (d.to_string(), *n)).collect()));
    let rows = column(&m);
    let all = rows.join("\n");
    assert!(meta_of(&rows, "first").contains("±3"), "{all}");
    assert!(!meta_of(&rows, "second").contains('±'), "a clean tree is not marked:\n{all}");
    assert!(!meta_of(&rows, "earlier").contains('±'), "the same tree is not counted twice:\n{all}");
    assert!(all.contains("± 2 trees not committed"), "every tree counts at the foot, listed or not:\n{all}");
}

#[test]
fn committing_clears_the_mark() {
    let mut m = model();
    update(&mut m, Msg::Uncommitted([("/repo".to_string(), 1)].into()));
    update(&mut m, Msg::Uncommitted([("/repo".to_string(), 0)].into()));
    let rows = column(&m);
    assert!(!rows.iter().any(|r| r.contains('±')), "{}", rows.join("\n"));
}
