# send_now Spec

Source: `crates/because-agent/src/lib.rs`
Test: `crates/because-agent/tests/send_now.rs`

## parse_line on the recorded inputs sent while the agent works

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| N1 | `fixtures/claude-stream/send-now.jsonl`, an input sent at a tool call | Its `InputTaken` comes before the one `Finished` of that input | `cuz` cannot tell that the text joined the running input |
| N2 | The same recording, an input sent while the final text streams | Its `InputTaken` comes after a `Finished`, and a second `Finished` follows | The response to the text has no input in the interface |
| N3 | `Options::args` | The arguments contain `--replay-user-messages` | No echo arrives, and text sent during a response never shows |
