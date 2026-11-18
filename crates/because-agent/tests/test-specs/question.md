# question Spec

Source: `crates/because-agent/src/lib.rs`
Test: `crates/because-agent/tests/question.rs`

## parse_line on the recorded question

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| Q1 | `fixtures/claude-stream/question.jsonl`, an `AskUserQuestion` call with two questions | One `PermissionRequest` with the two questions, their headers, options and descriptions. Only the second question allows more than one option | The prompt shows the wrong options, or a multi-select question takes only one answer |
| Q2 | The same request | Its `target` is the text of the first question | The status line shows an empty target |
| Q3 | A `Bash` request | `questions` is empty | A tool call shows as a question, and its approval is lost |

## answer_line for questions

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| Q4 | `Decision::Answers` for the recorded request, as the recording answered it | The same `updatedInput` that the recording sent: the questions, and `answers` keyed by question text | The agent gets "The user did not answer the questions." |
| Q5 | `Decision::Deny` for the recorded request | `deny`, with a message that says the developer declined to answer | The agent thinks a tool call failed, not that the developer declined |

## The recording

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| Q6 | The tool result in the recording | `ToolResult` that contains both answers | The fixture does not prove that the CLI reads `answers` |
