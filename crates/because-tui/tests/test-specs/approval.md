# approval Spec

App: `because-tui`
Test: `crates/because-tui/tests/approval.rs`

## Prompt

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| AP-1 | The agent asks about a Bash call with a suggested rule | The frame shows the prompt, the full call, the rule, and why the input waits | The developer approves a call that they cannot see |
| AP-2 | Push `a` after the grace period | One `Cmd::Answer` with `AllowOnce` goes to session 0, and the prompt closes | The agent waits forever, or a different call is approved |
| AP-3 | Push `A`, then `d` on a second request | `AllowSession` with a notice that names the rule, then `Deny` | The developer allows more than they think, or a deny allows |
| AP-4 | Push `m`, with and without a suggested rule | `AllowAuto`, a notice about auto mode, and the prompt closes | The session changes mode without a record on screen |
| AP-5 | Push `A` when the agent suggests no rule | The key does nothing, and the frame does not offer it | A session allowance with no rule allows all calls |
| AP-6 | Type `a` and `d` in the grace period | No answer, and no text in the draft | Keys typed for the composer approve a call by accident |
| AP-7 | Push plain keys and Tab while the prompt shows | No key has an effect. Ctrl+C stops the input of the prompt: one `Cmd::Stop`, and the prompt closes. A second Ctrl+C quits | The prompt closes without an answer, or Ctrl+C cannot get the developer out |
| AP-8 | Two sessions ask | The second prompt shows after the first answer, with its own grace period | One key press answers two calls |
| AP-9 | The same request arrives two times | One prompt | The developer answers a call two times |
| AP-10 | The agent withdraws its request | The prompt closes | The developer answers a request that is gone |
| AP-11 | The agent of session 0 exits while session 1 also asks | Only the prompt of session 1 stays | A prompt answers an agent that is gone |
| AP-12 | A session waits for approval | Its line in the Sessions column shows `?` | The developer does not see which session waits |
