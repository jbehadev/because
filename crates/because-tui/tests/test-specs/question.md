# question Spec

App: `because-tui`
Test: `crates/because-tui/tests/question.rs`

## Prompt

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| QU-1 | The agent asks two questions | The frame shows the first question, its header, its options with descriptions, "Other", and "question 1 of 2". It does not show the approval keys | The developer sees raw JSON, or approves a question without an answer |
| QU-2 | Push `2` on a single-select question, then select two options with `Space` and `j` on a multi-select question, then push Enter | One `Cmd::Answer` with `Answers(["Blue", "Small, Medium"])` to session 0, and the prompt closes | The agent gets no answers, or the wrong ones |
| QU-3 | Move with `j` and push Enter on a single-select question | The option under the cursor is the answer | Enter answers with a different option than the one on screen |
| QU-4 | Push Enter on a multi-select question with no option selected | The option under the cursor is the answer | Enter does nothing, and the developer cannot continue |
| QU-5 | Choose "Other", type `Green`, then push Enter | The answer is `Green`. Keys such as `j`, `q` and `1` go into the text, not to the options | The developer cannot type an answer, or letters move the cursor |
| QU-6 | Choose "Other" on a multi-select question with one option selected, and type an answer | The answer is the selected label and the typed text, joined by ", " | The selected options are lost |
| QU-7 | Push Enter with an empty "Other" text field, then Esc, then Esc | Enter does nothing. The first Esc closes the text field. The second Esc sends `Deny` | An empty answer goes to the agent, or Esc cannot get out of the text field |
| QU-8 | Push keys in the grace period | No answer | Keys typed for the composer answer a question by accident |
| QU-9 | A session waits for an answer | The status line shows "waiting for your answer" and the first question | The developer does not see why the input waits |
| QU-10 | Type in the "Other" text field while the input runs | The hint says Enter answers. It does not offer Ctrl+Enter send now | The developer pushes a key that does nothing in the prompt |
