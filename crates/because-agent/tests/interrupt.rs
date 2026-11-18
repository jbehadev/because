//! Stopping a running input, against a recording of the real CLI
//! (`fixtures/claude-stream/interrupt.jsonl`, from `scripts/record_interrupt.py`).

use serde_json::Value;
use because_agent::claude::{interrupt_line, parse_line};
use because_core::AgentEvent;

fn recording() -> String {
    std::fs::read_to_string("../../fixtures/claude-stream/interrupt.jsonl").expect("fixture")
}

#[test]
fn the_interrupt_is_the_request_the_recording_sent() {
    let ours: Value = serde_json::from_str(&interrupt_line("because-interrupt-1")).unwrap();
    let sent = recording()
        .lines()
        .filter_map(|l| l.strip_prefix("# sent: "))
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["type"] == "control_request")
        .expect("an interrupt in the recording");
    assert_eq!(ours, sent);
}

#[test]
fn each_interrupted_input_ends_with_an_error_result() {
    let ends: Vec<(bool, f64)> = recording()
        .lines()
        .flat_map(parse_line)
        .filter_map(|e| match e {
            AgentEvent::Finished { is_error, cost_total_usd, .. } => Some((is_error, cost_total_usd)),
            _ => None,
        })
        .collect();
    assert_eq!(ends.len(), 3, "{ends:?}");
    assert_eq!(&ends[..2], [(true, 0.0), (true, 0.0)]);
    assert!(!ends[2].0 && ends[2].1 > 0.0, "the process still takes inputs: {ends:?}");
}

#[test]
fn a_request_that_waits_is_withdrawn() {
    let events: Vec<AgentEvent> = recording().lines().flat_map(parse_line).collect();
    let asked = events.iter().position(|e| matches!(e, AgentEvent::PermissionRequest(_))).expect("a request");
    let AgentEvent::PermissionRequest(req) = &events[asked] else { unreachable!() };
    let withdrawn = events
        .iter()
        .position(|e| matches!(e, AgentEvent::PermissionCancelled { request_id } if *request_id == req.request_id))
        .expect("withdrawn");
    let second_end = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, AgentEvent::Finished { .. }))
        .nth(1)
        .map(|(i, _)| i)
        .unwrap();
    assert!(asked < withdrawn && withdrawn < second_end);
}
