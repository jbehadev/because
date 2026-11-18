# task_notification Spec

App: `because-tui`
Test: `crates/because-tui/tests/task_notification.rs`

## Replay of a response that the agent starts on its own

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| TN-1 | Send one input, then replay `fixtures/claude-stream/task-notification.jsonl` | Two inputs, both `Done`. The second input names the two tasks that ended, and has the tool call and the reply of the agent | The developer does not see what the agent did after a background task ended |
| TN-2 | The same replay | One `Cmd::Send`, and one `Cmd::Begin` for the second input. The response with no steps makes no input | The changes of the response are not measured, the agent gets an input two times, or an empty input shows |
| TN-3 | The selection is on the last input when the agent starts a response of its own | The selection moves to the new input | The developer does not see that the agent works |
| TN-4 | The agent sent `Started` with no input, and no step came yet. The developer pushes Enter with text | `Cmd::SendMidTurn`, not `Cmd::Send` | The text starts a second response while the agent works, and the events of the two responses mix |
| TN-5 | An input runs, and a `TaskEnded` arrives | The input text does not change, and no input is added | A notice that the agent reads in the running input shows two times |
| TN-6 | The agent sent `Started` with no input, then the echo of text that the developer sent while it worked | The text becomes the input, and the response goes to it, not to a second input | One response shows as two inputs |
