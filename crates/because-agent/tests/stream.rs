//! The parser against a recorded real session: two inputs, a Write and a Bash.

use because_agent::claude::parse_line;
use because_core::AgentEvent;

fn events() -> Vec<AgentEvent> {
    std::fs::read_to_string("../../fixtures/claude-stream/two-inputs.jsonl")
        .expect("fixture")
        .lines()
        .flat_map(parse_line)
        .collect()
}

#[test]
fn each_input_is_bracketed_by_started_and_finished() {
    let kinds: Vec<&str> = events()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Started { .. } => Some("start"),
            AgentEvent::Finished { .. } => Some("finish"),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, ["start", "finish", "start", "finish"]);
}

#[test]
fn tool_calls_name_what_they_act_on() {
    let tools: Vec<(String, String)> = events()
        .into_iter()
        .filter_map(|e| match e {
            AgentEvent::ToolUse { name, target, .. } => Some((name, target)),
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].0, "Write");
    assert!(tools[0].1.ends_with("/repo/hello.txt"), "{}", tools[0].1);
    assert_eq!(tools[1].0, "Bash");
    assert!(tools[1].1.contains("hello.txt"), "{}", tools[1].1);
}

#[test]
fn cost_in_finished_is_cumulative_across_inputs() {
    let costs: Vec<f64> = events()
        .into_iter()
        .filter_map(|e| match e {
            AgentEvent::Finished { cost_total_usd, .. } => Some(cost_total_usd),
            _ => None,
        })
        .collect();
    assert_eq!(costs.len(), 2);
    assert!(costs[1] > costs[0], "the second total includes the first");
    let second_alone = costs[1] - costs[0];
    assert!(second_alone < costs[0], "input 2 alone was cheaper than input 1 ({second_alone:.4} vs {:.4})", costs[0]);
}

#[test]
fn streamed_text_and_complete_blocks_agree() {
    let evs = events();
    let first_finish = evs.iter().position(|e| matches!(e, AgentEvent::Finished { .. })).unwrap();
    let streamed: String = evs[..first_finish]
        .iter()
        .filter_map(|e| if let AgentEvent::TextDelta(t) = e { Some(t.as_str()) } else { None })
        .collect();
    let complete: String = evs[..first_finish]
        .iter()
        .filter_map(|e| if let AgentEvent::Text(t) = e { Some(t.as_str()) } else { None })
        .collect();
    assert!(!streamed.is_empty());
    assert_eq!(streamed, complete);
}

#[test]
fn the_reply_is_the_final_text() {
    let replies: Vec<String> = events()
        .into_iter()
        .filter_map(|e| if let AgentEvent::Finished { reply, .. } = e { Some(reply) } else { None })
        .collect();
    assert!(replies[0].contains("hello.txt"));
    assert_eq!(replies[1], "Done.");
}

#[test]
fn unknown_and_malformed_lines_are_ignored() {
    assert!(parse_line("not json").is_empty());
    assert!(parse_line(r#"{"type":"rate_limit_event"}"#).is_empty());
    assert!(parse_line(r#"{"type":"something_new","x":1}"#).is_empty());
}

#[test]
fn subagent_messages_are_skipped() {
    let line = r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"content":[{"type":"text","text":"inner"}]}}"#;
    assert!(parse_line(line).is_empty());
}

#[test]
fn block_starts_report_what_the_agent_is_doing() {
    let acts: Vec<String> = events()
        .into_iter()
        .filter_map(|e| if let AgentEvent::Activity(a) = e { Some(a) } else { None })
        .collect();
    assert_eq!(acts, ["running Write", "writing", "running Bash", "writing"]);
}
