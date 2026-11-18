# task_notification Spec

Source: `crates/because-agent/src/claude/mod.rs`
Test: `crates/because-agent/tests/task_notification.rs`

## parse_line on a response that the agent starts on its own

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| TN1 | `fixtures/claude-stream/task-notification.jsonl`, two `system/task_notification` lines after a `result` | Two `TaskEnded` events, each with the `summary` of its line | `cuz` cannot tell why the agent started to work with no input |
| TN2 | The same fixture, from the end of the first input | The order is `Finished`, `TaskEnded`, `TaskEnded`, `Started`, `Finished` (no steps), `Started`, steps, `Finished` | The interface cannot find where the response of the agent starts and ends |
