# interrupt Spec

Source: `crates/because-agent/src/lib.rs`
Test: `crates/because-agent/tests/interrupt.rs`

## interrupt_line(request_id)

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| I1 | Request id `because-interrupt-1` | A `control_request` with subtype `interrupt` and that id, as the recording sent it | The CLI ignores the request, and the input does not stop |

## parse_line on the recorded interrupts

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| R1 | `fixtures/claude-stream/interrupt.jsonl`, three inputs | Three `Finished` events: the two interrupted inputs have `is_error` and a cost of 0, the last has neither | A stopped input never ends in the interface |
| R2 | The interrupt while a Bash call waits for approval | A `PermissionRequest`, then a `PermissionCancelled` with the same id, before the second `Finished` | The prompt stays on screen for a call that is gone |
