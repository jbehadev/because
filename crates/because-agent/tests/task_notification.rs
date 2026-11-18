//! A response that the agent starts on its own after a background task ends,
//! against `fixtures/claude-stream/task-notification.jsonl`. The fixture is
//! written by hand in the shape of a real `cuz live` log.

use because_agent::claude::parse_line;
use because_core::AgentEvent;

fn events() -> Vec<AgentEvent> {
    std::fs::read_to_string("../../fixtures/claude-stream/task-notification.jsonl")
        .expect("fixture")
        .lines()
        .flat_map(parse_line)
        .collect()
}

#[test]
fn each_task_notification_gives_its_summary() {
    let ended: Vec<String> = events()
        .into_iter()
        .filter_map(|e| match e {
            AgentEvent::TaskEnded(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(
        ended,
        [
            "Background command \"Watch the log\" was stopped after reaching its background time limit",
            "Background command \"Build and run the tests\" completed (exit code 0)",
        ]
    );
}

#[test]
fn the_response_of_its_own_follows_the_notices() {
    let marks: Vec<&str> = events()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Started { .. } => Some("start"),
            AgentEvent::Finished { .. } => Some("end"),
            AgentEvent::TaskEnded(_) => Some("task"),
            AgentEvent::ToolUse { .. } | AgentEvent::Text(_) => Some("step"),
            _ => None,
        })
        .collect();
    assert_eq!(marks, ["start", "step", "step", "end", "task", "task", "start", "end", "start", "step", "step", "end"]);
}
