//! The summary call, against a recorded real result
//! (`fixtures/claude-summary/result.json`).

use because_agent::claude::summary::{args, parse_result};
use because_agent::summary::{SYSTEM, prompt};

#[test]
fn a_recorded_result_parses() {
    let out = std::fs::read_to_string("../../fixtures/claude-summary/result.json").unwrap();
    let s = parse_result(&out).unwrap();
    assert!(s.text.starts_with("Thought_slicer preserves model narration"), "{}", s.text);
    assert_eq!(s.model, "claude-haiku-4-5-20251001");
    assert!((s.cost_usd - 0.002026).abs() < 1e-9);
    assert_eq!(s.duration_ms, 1281);
}

#[test]
fn errors_and_empty_results_are_errors() {
    let err = parse_result(r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"result":""}"#);
    assert!(err.unwrap_err().to_string().contains("error_max_budget_usd"));
    let empty = parse_result(r#"{"type":"result","subtype":"success","is_error":false,"result":"  "}"#);
    assert!(empty.unwrap_err().to_string().contains("empty"));
    assert!(parse_result("Invalid API key · Please run /login").is_err());
}

#[test]
fn only_the_first_line_is_kept() {
    let s = parse_result(r#"{"type":"result","subtype":"success","is_error":false,"result":"One line.\n\nAnd more."}"#);
    assert_eq!(s.unwrap().text, "One line.");
}

#[test]
fn the_prompt_restates_the_limit_last_and_cuts_the_middle_of_long_text() {
    let said = format!("{}{}{}", "a".repeat(12_000), "MIDDLE".repeat(1_000), "z".repeat(3_000));
    let p = prompt("do the thing", &["Edit src/a.rs", "Bash cargo test"], &said);
    assert!(p.starts_with("Input the developer typed:\ndo the thing\n"));
    assert!(p.contains("Tools the agent ran: \n- Edit src/a.rs\n- Bash cargo test\n"));
    assert!(!p.contains("MIDDLE"), "the middle of a long reply is cut");
    assert!(p.contains("[… 6000 characters cut …]"));
    assert!(p.contains(&"z".repeat(3_000)), "the end is kept: it is usually the conclusion");
    assert!(p.ends_with("Summarize the exchange above in ONE sentence of at most 25 words."));
    assert!(prompt("q", &[], "a").contains("Tools the agent ran: (none)"));
}

#[test]
fn the_call_has_no_tools_no_persistence_and_a_budget() {
    let a = args("haiku");
    let after = |flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].as_str());
    assert_eq!(after("--model"), Some("haiku"));
    assert_eq!(after("--tools"), Some(""));
    assert_eq!(after("--system-prompt"), Some(SYSTEM));
    assert_eq!(after("--output-format"), Some("json"));
    assert!(a.iter().any(|x| x == "--no-session-persistence"));
    assert!(after("--max-budget-usd").is_some());
}
