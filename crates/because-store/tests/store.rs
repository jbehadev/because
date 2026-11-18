use because_core::{Commit, FileChange, Hunk, NarrationStep, Origin, Risk, SegStatus, Section, Segment, Session, StepKind};
use because_store::{Source, Store};

fn session(id: &str) -> Session {
    Session {
        id: id.into(),
        repo: "/repo".into(),
        branch: "main".into(),
        started_at: "2026-10-02T10:00:00.000Z".into(),
        ..Default::default()
    }
}

fn segment(seq: u32) -> Segment {
    Segment {
        id: format!("live-{seq}"),
        seq,
        input: format!("input {seq}\nsecond line"),
        reply: "Done.".into(),
        model: "claude-opus-5".into(),
        status: SegStatus::Done,
        started_at: format!("2026-10-02T10:0{seq}:00.000Z"),
        duration_ms: 37_000,
        elapsed: "0m 37s".into(),
        cost_usd: 0.25,
        sections: vec![Section::whole_response(vec![
            NarrationStep { kind: StepKind::Text, body: "Let me look.".into() },
            NarrationStep { kind: StepKind::ToolCall, body: "Edit src/a.rs".into() },
            NarrationStep { kind: StepKind::Thinking, body: "hmm".into() },
            NarrationStep { kind: StepKind::Text, body: "Done.".into() },
        ])],
        ..Default::default()
    }
}

fn change() -> FileChange {
    FileChange {
        id: "x-f0".into(),
        path: "src/a.rs".into(),
        change_type: "modify".into(),
        additions: 3,
        deletions: 1,
        risk: Risk::Api,
        summary: "Renamed the entry point.".into(),
        state: "unsummarized".into(),
        content_start_line: 1,
        content: vec!["fn main() {}".into()],
        hunks: vec![Hunk {
            seq: 0,
            header: "fn main".into(),
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 4,
            summary: String::new(),
            patch: "@@ -1,2 +1,4 @@\n+x".into(),
        }],
        line_origins: Default::default(),
    }
}

#[test]
fn a_session_round_trips_without_its_code() {
    let mut st = Store::open_in_memory().unwrap();
    let s = session("abc");
    st.save_session(&s, None).unwrap();
    let mut one = segment(1);
    one.summary = "Renamed main.".into();
    one.summary_by = "claude-haiku-4-5".into();
    one.summary_cost_usd = 0.0013;
    one.commit = Some(Commit {
        sha: "b".repeat(40),
        short: "bbbbbbb".into(),
        parent: "a".repeat(40),
        author: "because".into(),
        ..Default::default()
    });
    one.changes = vec![change()];
    st.save_segment("abc", &one).unwrap();
    st.save_segment("abc", &segment(2)).unwrap();

    let got = st.load_sessions().unwrap();
    assert_eq!(got.len(), 1);
    let s = &got[0];
    assert_eq!((s.id.as_str(), s.origin, s.repo.as_str()), ("abc", Origin::Stored, "/repo"));
    assert_eq!(s.segments.len(), 2);
    let a = &s.segments[0];
    assert_eq!(a.input, "input 1\nsecond line");
    assert_eq!((a.summary.as_str(), a.summary_by.as_str()), ("Renamed main.", "claude-haiku-4-5"));
    assert_eq!((a.duration_ms, a.elapsed.as_str(), a.cost_usd), (37_000, "0m 37s", 0.25));
    let kinds: Vec<StepKind> = a.sections[0].narration.iter().map(|n| n.kind).collect();
    assert_eq!(kinds, [StepKind::Text, StepKind::ToolCall, StepKind::Thinking, StepKind::Text]);
    let c = a.commit.as_ref().unwrap();
    assert_eq!((c.short.as_str(), c.parent.as_str()), ("bbbbbbb", "a".repeat(40).as_str()));
    let fc = &a.changes[0];
    assert_eq!((fc.path.as_str(), fc.additions, fc.deletions, fc.risk), ("src/a.rs", 3, 1, Risk::Api));
    assert_eq!(fc.summary, "Renamed the entry point.");
    assert_eq!((fc.hunks.len(), fc.hunks[0].header.as_str(), fc.hunks[0].new_lines), (1, "fn main", 4));
    assert!(fc.content.is_empty() && fc.hunks[0].patch.is_empty(), "the code is git's to keep, not the store's");
    assert!(a.code_unloaded);
    assert!(!s.segments[1].code_unloaded, "nothing to load for an input that changed no files");
}

#[test]
fn saving_an_input_again_replaces_it() {
    let mut st = Store::open_in_memory().unwrap();
    st.save_session(&session("abc"), None).unwrap();
    let mut seg = segment(1);
    seg.status = SegStatus::Running;
    seg.sections = vec![Section::whole_response(vec![])];
    st.save_segment("abc", &seg).unwrap();
    let mut seg = segment(1);
    seg.changes = vec![change()];
    st.save_segment("abc", &seg).unwrap();
    seg.changes = vec![];
    seg.summary = "later".into();
    st.save_segment("abc", &seg).unwrap();
    let got = &st.load_sessions().unwrap()[0].segments;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].sections[0].narration.len(), 4);
    assert!(got[0].changes.is_empty());
    assert_eq!(got[0].summary, "later");
}

#[test]
fn an_input_left_running_loads_as_failed_and_says_why() {
    let mut st = Store::open_in_memory().unwrap();
    st.save_session(&session("abc"), None).unwrap();
    let mut seg = segment(1);
    seg.status = SegStatus::Running;
    seg.reply.clear();
    st.save_segment("abc", &seg).unwrap();
    let got = &st.load_sessions().unwrap()[0].segments[0];
    assert_eq!(got.status, SegStatus::Failed);
    assert!(got.reply.contains("did not finish"), "{}", got.reply);
}

#[test]
fn sessions_come_newest_first_and_empty_ones_are_left_out() {
    let mut st = Store::open_in_memory().unwrap();
    for (id, when) in [("old", "2026-09-01"), ("new", "2026-10-01"), ("empty", "2026-10-02")] {
        let mut s = session(id);
        s.started_at = format!("{when}T00:00:00.000Z");
        st.save_session(&s, None).unwrap();
        if id != "empty" {
            let mut seg = segment(1);
            seg.started_at = s.started_at.clone();
            st.save_segment(id, &seg).unwrap();
            st.save_session(&Session { segments: vec![seg], ..s }, None).unwrap();
        }
    }
    let ids: Vec<String> = st.load_sessions().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(ids, ["new", "old"]);
}

#[test]
fn imported_sessions_keep_their_source_and_unrecorded_costs() {
    let mut st = Store::open_in_memory().unwrap();
    let mut s = session("imp");
    s.origin = Origin::Imported;
    s.title = "Three-column TUI".into();
    s.recorded_cost = Some(67.75);
    let mut a = segment(1);
    a.cost_unrecorded = true;
    a.cost_usd = 0.0;
    a.link = "commit_time".into();
    s.segments = vec![a, segment(2)];
    let src = Source { path: "/x.jsonl".into(), size: 10, mtime: 5 };
    st.replace_imported(&s, &src).unwrap();
    assert_eq!(st.origin_of("imp").unwrap().as_deref(), Some("imported"));
    assert_eq!(st.source_of("imp").unwrap(), Some(src.clone()));
    let got = &st.load_sessions().unwrap()[0];
    assert_eq!((got.origin, got.title.as_str(), got.recorded_cost), (Origin::Imported, "Three-column TUI", Some(67.75)));
    assert!(got.segments[0].cost_unrecorded && !got.segments[1].cost_unrecorded);
    assert_eq!(got.segments[0].link, "commit_time");
    assert_eq!(got.cost(), 67.75, "the recorded total, not the sum of what is known per input");

    // Re-importing replaces the inputs rather than adding to them.
    s.segments.truncate(1);
    st.replace_imported(&s, &Source { size: 11, ..src }).unwrap();
    assert_eq!(st.load_sessions().unwrap()[0].segments.len(), 1);
    assert_eq!(st.origin_of("nope").unwrap(), None);
}

#[test]
fn every_commit_of_an_input_answers_to_it() {
    let mut st = Store::open_in_memory().unwrap();
    st.save_session(&session("abc"), None).unwrap();
    let mut seg = segment(1);
    seg.commit = Some(Commit { sha: "c".repeat(40), short: "ccccccc".into(), parent: "a".repeat(40), ..Default::default() });
    st.save_segment("abc", &seg).unwrap();
    st.add_commit("abc", 1, &Commit { sha: "b".repeat(40), parent: "a".repeat(40), ..Default::default() }).unwrap();
    let mut got = st.session_commits("abc").unwrap();
    got.sort();
    assert_eq!(got, [("bbbbbbb".to_string(), "ccccccc".to_string()), ("ccccccc".into(), "ccccccc".into())]);
}

#[test]
fn the_schema_is_versioned_and_a_newer_one_is_refused() {
    let dir = std::env::temp_dir().join(format!("because-store-test-{}", std::process::id()));
    let path = dir.join("slicer.db");
    let _ = std::fs::remove_dir_all(&dir);
    let st = Store::open(&path).unwrap();
    assert_eq!(st.schema_version().unwrap(), 1);
    drop(st);
    let st = Store::open(&path).unwrap();
    assert_eq!(st.schema_version().unwrap(), 1, "reopening does not re-run migrations");
    drop(st);
    let c = rusqlite::Connection::open(&path).unwrap();
    c.pragma_update(None, "user_version", 99).unwrap();
    drop(c);
    let err = Store::open(&path).err().unwrap().to_string();
    assert!(err.contains("schema version 99"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
