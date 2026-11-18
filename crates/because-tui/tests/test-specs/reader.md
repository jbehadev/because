# reader Spec

App: `because-tui`
Test: `crates/because-tui/tests/reader.rs`

## Summaries in the column

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| SM-1 | A long reply | A summary request, "summarizing…", then the summary and "Enter reads all" | A long reply fills the column |
| SM-2 | The reader on an input while its changes are computed | "computing changes…", which goes when the changes arrive | The reader shows no code with no reason |
| SM-3 | A short answer | No summary request, and the answer shows | Money is spent on a summary that is not necessary |
| SM-4 | A short reply after tool calls | A summary request | The column does not tell what the tools did |
| SM-5 | No summarizer | No summary request | `--no-summary` does not stop the cost |
| SM-6 | The summary fails | Two lines of the reply, without markup, and the reason | The column is empty with no reason |

## The reader

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| RD-1 | Enter on an empty draft | The reader opens with the rendered input and reply, and Esc goes back | The developer cannot read the full response |
| RD-2 | Scroll by line and by page | The offset moves, and stops at the two ends | Text is out of reach |
| RD-3 | `]` and `[` | The reader moves between inputs and stops at the first | The developer cannot move through the session |
| RD-4 | Enter in Changes on an input with no files | The reader opens, and Esc goes back to Changes | Enter does nothing there |
| RD-5 | Move to a different pane | The reader closes | The reader covers the pane that the developer selected |
| RD-6 | Alt+r with a draft | The reader opens, and the draft stays | The draft is lost |
| RD-7 | The reader on a running input | It follows the end until the developer scrolls | The developer does not see new text |
| RD-8 | Read a queued input, in the sample view | No reader, and a notice | The reader shows an empty input |
| RD-9 | The generated tag at four widths | The tag is complete | The model and cost are cut |
| RD-10 | The reader on the latest input, from the composer | The composer stays, the page keys scroll, and Enter sends | The developer must leave the reader to answer |
| RD-11 | The reader from the list | Keys are commands until `i` | Plain keys go into the draft by accident |
| RD-12 | The reader on an earlier input | No composer, until the latest input | A reply to an earlier input looks possible |
