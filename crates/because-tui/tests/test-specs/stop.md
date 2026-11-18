# stop Spec

App: `because-tui`
Test: `crates/because-tui/tests/stop.rs`

## Ctrl+C on a running input

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| ST-1 | An input runs, the draft is empty, push Ctrl+C | One `Cmd::Stop` for the session, no quit, and a notice that tells the next Ctrl+C quits | The developer cannot stop an agent that does the wrong work |
| ST-2 | Push Ctrl+C two times while an input runs | The first press stops, the second quits | The developer cannot quit while the agent ends the input |
| ST-3 | An input runs and the draft has text, push Ctrl+C | The draft clears, and no `Cmd::Stop` | A press to clear a draft stops the work of the agent |
| ST-4 | Session 1 runs, session 0 shows and is idle, push Ctrl+C | Quit, and no `Cmd::Stop` | Ctrl+C stops a session that the developer does not see |
| ST-5 | The status line while an input runs | The hint tells that Ctrl+C stops | The developer does not know how to stop the input |

## The end of a stopped input

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| SE-1 | The recorded interrupt `result` arrives after a stop | The input is `Done`, not `Failed`, it shows that the developer stopped it, and its changes are computed | A stopped input shows as a failure, and its edits are not shown |
| SE-2 | A stopped input reports a total cost of 0, then the next input reports its total | The next input costs only its own part | The next input is charged for all earlier inputs |
| SE-4 | The developer sends text, then stops the input before the agent reads the text | The stopped input ends, and the text becomes a new running input when the agent reads it | The text that redirects the agent is lost after a stop |
