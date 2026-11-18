//! Questions from the agent (`AskUserQuestion`), against a recording of the
//! real CLI (`fixtures/claude-stream/question.jsonl`, from
//! `scripts/record_question.py`).

use serde_json::Value;
use because_agent::claude::{answer_line, parse_line};
use because_core::{AgentEvent, Decision, PermissionRequest, QuestionOption};

fn recording() -> String {
    std::fs::read_to_string("../../fixtures/claude-stream/question.jsonl").expect("fixture")
}

/// The request line as the CLI sent it.
fn request_line() -> String {
    recording().lines().find(|l| l.contains("\"can_use_tool\"")).expect("a question in the recording").to_string()
}

fn request() -> PermissionRequest {
    match parse_line(&request_line()).as_slice() {
        [AgentEvent::PermissionRequest(r)] => r.clone(),
        other => panic!("{other:?}"),
    }
}

/// The answer that the recording sent, which the CLI took.
fn sent_answer() -> Value {
    recording()
        .lines()
        .filter_map(|l| l.strip_prefix("# sent: "))
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["type"] == "control_response")
        .expect("an answer in the recording")
}

fn answered(decision: Decision) -> Value {
    let e: Value = serde_json::from_str(&request_line()).unwrap();
    serde_json::from_str(&answer_line(e["request_id"].as_str().unwrap(), &e["request"], decision)).unwrap()
}

#[test]
fn a_question_arrives_with_its_options() {
    let r = request();
    assert_eq!(r.tool, "AskUserQuestion");
    let [color, sizes] = r.questions.as_slice() else { panic!("{:?}", r.questions) };
    assert_eq!((color.question.as_str(), color.header.as_str(), color.multi), ("Which color?", "Color", false));
    assert_eq!(color.options[1], QuestionOption { label: "Blue".into(), description: "Blue".into() });
    assert_eq!((sizes.header.as_str(), sizes.multi), ("Sizes", true));
    let labels: Vec<&str> = sizes.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["Small", "Medium", "Large"]);
}

#[test]
fn the_target_is_the_first_question() {
    assert_eq!(request().target, "Which color?");
}

#[test]
fn a_tool_call_has_no_questions() {
    let bash = r#"{"type":"control_request","request_id":"req_1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}"#;
    match parse_line(bash).as_slice() {
        [AgentEvent::PermissionRequest(r)] => assert!(r.questions.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_answers_are_what_the_recording_sent() {
    let ours = answered(Decision::Answers(vec!["Blue".into(), "Small, Medium".into()]));
    assert_eq!(ours, sent_answer());
    assert_eq!(ours["response"]["response"]["updatedInput"]["answers"]["Which sizes?"], "Small, Medium");
}

#[test]
fn declining_tells_the_agent_that_the_developer_did_not_answer() {
    let r = &answered(Decision::Deny)["response"]["response"];
    assert_eq!(r["behavior"], "deny");
    assert!(r["message"].as_str().unwrap().contains("declined to answer"), "{r}");
}

#[test]
fn the_agent_got_the_answers() {
    let text = recording()
        .lines()
        .flat_map(parse_line)
        .find_map(|e| match e {
            AgentEvent::ToolResult { text, .. } => Some(text),
            _ => None,
        })
        .expect("a tool result");
    assert!(text.contains("\"Which color?\"=\"Blue\""), "{text}");
    assert!(text.contains("\"Which sizes?\"=\"Small, Medium\""), "{text}");
}
