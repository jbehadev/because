//! Permission requests over `--permission-prompt-tool stdio`.
//!
//! These lines follow the Agent SDK's control protocol, not a recording yet:
//! `scripts/record_permission.py` captures one into
//! `fixtures/claude-stream/permission.jsonl`, which should replace them.

use serde_json::{Value, json};
use because_agent::claude::{answer_line, parse_line, permission_detail};
use because_core::{AgentEvent, Decision, PermissionRequest};

const ASK: &str = r#"{"type":"control_request","request_id":"req_1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"cargo test -p because-agent","description":"Run the agent tests"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"cargo test:*"}],"behavior":"allow","destination":"localSettings"}],"tool_use_id":"toolu_1"}}"#;

fn request() -> PermissionRequest {
    match parse_line(ASK).as_slice() {
        [AgentEvent::PermissionRequest(r)] => r.clone(),
        other => panic!("{other:?}"),
    }
}

fn answered(decision: Decision) -> Value {
    let e: Value = serde_json::from_str(ASK).unwrap();
    serde_json::from_str(&answer_line("req_1", &e["request"], decision)).unwrap()
}

#[test]
fn a_request_names_the_tool_and_shows_the_whole_call() {
    let r = request();
    assert_eq!(r.request_id, "req_1");
    assert_eq!(r.tool, "Bash");
    assert_eq!(r.target, "cargo test -p because-agent");
    assert_eq!(r.detail, "cargo test -p because-agent\n\nRun the agent tests");
}

#[test]
fn allow_for_the_session_offers_the_agents_own_rule() {
    assert_eq!(request().always.as_deref(), Some("Bash(cargo test:*)"));
    let none = ASK.replace(r#""permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"cargo test:*"}],"behavior":"allow","destination":"localSettings"}],"#, "");
    match parse_line(&none).as_slice() {
        [AgentEvent::PermissionRequest(r)] => assert_eq!(r.always, None, "nothing suggested, nothing to allow for the session"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn allowing_passes_the_input_back_unchanged() {
    let a = answered(Decision::AllowOnce);
    assert_eq!(a["type"], "control_response");
    assert_eq!(a["response"]["subtype"], "success");
    assert_eq!(a["response"]["request_id"], "req_1");
    let r = &a["response"]["response"];
    assert_eq!(r["behavior"], "allow");
    assert_eq!(r["updatedInput"]["command"], "cargo test -p because-agent");
    assert!(r["updatedPermissions"].is_null(), "once means once");
}

#[test]
fn allowing_for_the_session_keeps_the_rule_out_of_the_settings_file() {
    let r = &answered(Decision::AllowSession)["response"]["response"];
    assert_eq!(r["behavior"], "allow");
    let updates = r["updatedPermissions"].as_array().unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["rules"][0]["ruleContent"], "cargo test:*");
    assert_eq!(updates[0]["destination"], "session", "suggested localSettings, kept to the session");
}

#[test]
fn allowing_in_auto_mode_switches_the_session_only() {
    let r = &answered(Decision::AllowAuto)["response"]["response"];
    assert_eq!(r["behavior"], "allow");
    assert_eq!(r["updatedInput"]["command"], "cargo test -p because-agent");
    let updates = r["updatedPermissions"].as_array().unwrap();
    assert_eq!(updates, &[json!({ "type": "setMode", "mode": "auto", "destination": "session" })]);
}

#[test]
fn denying_tells_the_agent_why() {
    let r = &answered(Decision::Deny)["response"]["response"];
    assert_eq!(r["behavior"], "deny");
    assert!(r["message"].as_str().unwrap().contains("declined"));
}

#[test]
fn a_withdrawn_request_is_reported() {
    let evs = parse_line(r#"{"type":"control_cancel_request","request_id":"req_1"}"#);
    assert_eq!(evs, [AgentEvent::PermissionCancelled { request_id: "req_1".into() }]);
}

#[test]
fn other_control_messages_are_not_permission_requests() {
    assert!(parse_line(r#"{"type":"control_request","request_id":"r","request":{"subtype":"hook_callback"}}"#).is_empty());
    assert!(parse_line(r#"{"type":"control_response","response":{"subtype":"success","request_id":"r"}}"#).is_empty());
}

#[test]
fn a_request_from_a_subagent_still_asks() {
    let sub = ASK.replace(r#""tool_use_id":"toolu_1""#, r#""tool_use_id":"toolu_1","agent_id":"a1""#);
    assert_eq!(parse_line(&sub).len(), 1);
}

#[test]
fn details_show_what_a_write_or_edit_would_do() {
    let w = permission_detail("Write", &json!({ "file_path": "/r/a.txt", "content": "one\ntwo" }));
    assert_eq!(w, "/r/a.txt\n\none\ntwo");
    let e = permission_detail("Edit", &json!({ "file_path": "/r/a.txt", "old_string": "one", "new_string": "uno" }));
    assert_eq!(e, "/r/a.txt\n\n− one\n+ uno");
    let other = permission_detail("mcp__x__y", &json!({ "q": 1 }));
    assert!(other.contains("\"q\": 1"), "{other}");
}

#[test]
fn a_long_call_is_cut_and_says_how_much_is_left() {
    let content: Vec<String> = (1..=100).map(|i| format!("line {i}")).collect();
    let d = permission_detail("Write", &json!({ "file_path": "/r/big.txt", "content": content.join("\n") }));
    assert!(d.lines().count() <= 25, "{}", d.lines().count());
    assert!(d.ends_with("more lines"), "{d}");
}

#[test]
fn the_cli_is_asked_to_send_requests_on_stdio() {
    let o = because_agent::SessionConfig {
        cwd: ".".into(),
        session_id: "s".into(),
        permission_mode: "acceptEdits".into(),
        model: None,
        resume: false,
        log: None,
        ephemeral: false,
    };
    let args = because_agent::claude::args(&o);
    let i = args.iter().position(|a| a == "--permission-prompt-tool").expect("flag");
    assert_eq!(args[i + 1], "stdio");
}
