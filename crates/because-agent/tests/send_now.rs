//! Inputs sent while the agent works, against a recording of the real CLI
//! (`fixtures/claude-stream/send-now.jsonl`, from `scripts/record_send_now.py`).

use because_agent::SessionConfig;
use because_agent::claude::{args, parse_line};
use because_core::AgentEvent;

/// The events of the recording, reduced to the echoes and the ends.
fn marks() -> Vec<String> {
    std::fs::read_to_string("../../fixtures/claude-stream/send-now.jsonl")
        .expect("fixture")
        .lines()
        .flat_map(parse_line)
        .filter_map(|e| match e {
            AgentEvent::InputTaken(t) => Some(t.split_whitespace().last().unwrap_or_default().to_string()),
            AgentEvent::Finished { .. } => Some("end".into()),
            _ => None,
        })
        .collect()
}

#[test]
fn text_read_at_a_tool_call_joins_the_running_input() {
    let m = marks();
    assert_eq!(&m[..3], ["summary.", "PINEAPPLE.", "end"], "{m:?}");
}

#[test]
fn text_read_after_the_last_step_gets_a_response_of_its_own() {
    let m = marks();
    assert_eq!(&m[3..], ["bees.", "end", "MANGO.", "end"], "{m:?}");
}

#[test]
fn the_cli_is_asked_to_echo_each_input() {
    let o = SessionConfig {
        cwd: ".".into(),
        session_id: "s".into(),
        permission_mode: "acceptEdits".into(),
        model: None,
        resume: false,
        log: None,
        ephemeral: false,
    };
    assert!(args(&o).contains(&"--replay-user-messages".to_string()));
}
