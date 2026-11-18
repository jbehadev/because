//! Reading Claude Code transcripts, against a recorded real one
//! (`fixtures/claude-transcript/two-inputs.jsonl`: two `claude -p` inputs, the
//! first using Write, the second Bash) and variants of it with the records an
//! interactive session adds.

use because_agent::claude::transcript::parse;
use because_core::{Origin, StepKind};

fn fixture() -> String {
    std::fs::read_to_string("../../fixtures/claude-transcript/two-inputs.jsonl").unwrap()
}

#[test]
fn inputs_become_segments_with_what_the_agent_said_and_did() {
    let t = parse(&fixture()).unwrap();
    let s = &t.session;
    assert_eq!(s.id, "8062b589-4cab-4e08-86e4-484ac35ec56e");
    assert_eq!((s.origin, s.repo.as_str()), (Origin::Imported, "/repo"));
    assert_eq!(s.started_at, "2026-10-02T17:02:54.014Z");
    assert_eq!(s.segments.len(), 2);

    let a = &s.segments[0];
    assert!(a.input.starts_with("Create a file notes.txt"), "{}", a.input);
    let steps: Vec<(StepKind, &str)> = a.sections[0].narration.iter().map(|n| (n.kind, n.body.as_str())).collect();
    assert_eq!(steps[0], (StepKind::ToolCall, "Write notes.txt"), "path made relative to the session's directory");
    assert_eq!(steps[1].0, StepKind::Text);
    assert!(a.reply.starts_with("I created `notes.txt`"));
    assert_eq!(a.model, "claude-sonnet-5-5");
    assert_eq!(a.started_at, "2026-10-02T17:02:54.014Z");
    // From the input to the last record answering it.
    assert_eq!(a.duration_ms, 3_496);

    let b = &s.segments[1];
    assert!(b.sections[0].narration[0].body.starts_with("Bash echo"), "{:?}", b.sections[0].narration);
    assert_eq!(b.reply, "Done.");
}

#[test]
fn a_running_total_after_each_input_gives_each_its_own_cost() {
    let s = parse(&fixture()).unwrap().session;
    let (a, b) = (&s.segments[0], &s.segments[1]);
    assert!(!a.cost_unrecorded && !b.cost_unrecorded);
    assert!((a.cost_usd - 0.0296692).abs() < 1e-9, "{}", a.cost_usd);
    assert!((b.cost_usd - (0.0393462 - 0.0296692)).abs() < 1e-9, "{}", b.cost_usd);
    assert!((s.recorded_cost.unwrap() - 0.0393462).abs() < 1e-9);
}

#[test]
fn a_total_covering_two_inputs_is_not_split_between_them() {
    // Drop the running total written after the first input: the next one then
    // covers both, and neither input's own cost is known.
    let mut seen = false;
    let text: String = fixture()
        .lines()
        .filter(|l| {
            let first = l.contains("\"type\": \"cost-state\"") && !seen;
            seen |= first;
            !first
        })
        .map(|l| format!("{l}\n"))
        .collect();
    let s = parse(&text).unwrap().session;
    assert!(s.segments.iter().all(|x| x.cost_unrecorded));
    assert!((s.cost() - 0.0393462).abs() < 1e-9, "the session total still is");
}

#[test]
fn the_windows_cover_each_input_from_sent_to_last_answer() {
    let t = parse(&fixture()).unwrap();
    let ms = |s: &str| because_core::time::parse_ms(s).unwrap();
    assert_eq!(t.windows[0], (ms("2026-10-02T17:02:54.014Z"), ms("2026-10-02T17:02:57.510Z")));
    assert_eq!(t.windows[1].0, ms("2026-10-02T17:02:58.901Z"));
}

/// Records an interactive session has that `claude -p` does not, spliced in.
fn interactive() -> String {
    let user = |extra: &str, content: &str| {
        format!(
            r#"{{"type":"user","sessionId":"8062b589-4cab-4e08-86e4-484ac35ec56e","cwd":"/repo","timestamp":"2026-10-02T17:03:10.000Z","promptId":"p9","uuid":"u9",{extra}"message":{{"role":"user","content":{content}}}}}"#
        )
    };
    let mut lines: Vec<String> = fixture().lines().map(String::from).collect();
    lines.extend([
        user("", r#""<command-name>/clear</command-name>""#),
        user(r#""isMeta":true,"#, r#"[{"type":"text","text":"Base directory for this skill: /x"}]"#),
        user(r#""isCompactSummary":true,"#, r#""This session is being continued from a previous conversation""#),
        user("", r#"[{"type":"text","text":"[Request interrupted by user]"}]"#),
        user(r#""origin":{"kind":"task-notification"},"#, r#""a background task finished""#),
        user(r#""isSidechain":true,"#, r#""a subagent's prompt""#),
        user(r#""origin":{"kind":"human"},"#, r#"[{"type":"image","source":{}},{"type":"text","text":"what is in this screenshot?"}]"#),
        r#"{"type":"ai-title","aiTitle":"Notes file with alpha and beta","sessionId":"8062b589-4cab-4e08-86e4-484ac35ec56e"}"#.into(),
        r#"{"type":"some-future-record","whatever":1}"#.into(),
        "not json at all".into(),
    ]);
    lines.join("\n")
}

#[test]
fn only_what_a_person_typed_becomes_an_input() {
    let s = parse(&interactive()).unwrap().session;
    let inputs: Vec<&str> = s.segments.iter().map(|x| x.input.as_str()).collect();
    assert_eq!(inputs.len(), 3, "{inputs:?}");
    assert_eq!(inputs[2], "[image]\nwhat is in this screenshot?");
    assert_eq!(s.segments[2].id, "p9", "Claude Code's own id for the input");
    assert_eq!(s.title, "Notes file with alpha and beta");
}

#[test]
fn a_failed_tool_call_says_so() {
    let text = fixture().replace(
        r#""content": "(Bash completed with no output)", "is_error": false"#,
        r#""content": "Exit code 1\nbash: notes.txt: Permission denied", "is_error": true"#,
    );
    assert_ne!(text, fixture(), "the fixture's Bash result is where this test expects it");
    let s = parse(&text).unwrap().session;
    let bash = &s.segments[1].sections[0].narration[0].body;
    assert!(bash.ends_with("— failed: Exit code 1"), "{bash}");
}

#[test]
fn a_transcript_with_nothing_typed_is_not_a_session() {
    assert!(parse(r#"{"type":"ai-title","aiTitle":"x","sessionId":"s"}"#).is_none());
    assert!(parse("").is_none());
}
