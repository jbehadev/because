# send_now Spec

App: `because-tui`
Test: `crates/because-tui/tests/send_now.rs`

## Replay of the recorded CLI session

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| SN-1 | Replay `fixtures/claude-stream/send-now.jsonl`, and send each input of the recording at the same point | Three inputs, all `Done`: the first has the added text, the second and the third are their own inputs | Text sent during a response shows on the wrong input, or its response is lost |
| SN-2 | The same replay | Two `Cmd::Send` and two `Cmd::SendMidTurn`, and each text goes out one time | The agent gets an input two times |

## Send now (Ctrl+Enter, or Ctrl+X Ctrl+S)

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| SN-3 | An input runs, the draft has text, push Ctrl+Enter | `Cmd::SendMidTurn` with the text, then `Cmd::Stop`, and the draft clears | The agent does not read the correction until its current step ends |
| SN-4 | Push Ctrl+X, then Ctrl+S, in the same state | The same as SN-3 | Terminals that do not report Ctrl+Enter have no send-now key |
| SN-5 | Push Ctrl+X, then a letter | The letter goes into the draft, and nothing is sent | The chord takes a key from the developer |
| SN-6 | An input runs and the draft is empty, and no text waits | Nothing is sent or stopped, and a notice tells why | A press with nothing to send stops the work of the agent |
| SN-7 | No input runs, the draft has text, push Ctrl+Enter | The same as Enter: one `Cmd::Send` | Ctrl+Enter does nothing when the agent is idle |
| SN-8 | The interrupted input ends, then the agent reads the text | The interrupted input is `Done` and tells that it stopped for the new input; the text becomes a running input | The developer cannot see why the input stopped |
