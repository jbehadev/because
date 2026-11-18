# shell Spec

App: `because-tui`
Test: `crates/because-tui/tests/shell.rs`

## A draft that starts with `!`

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| SH-1 | The session is idle, submit `!git status` | One `Cmd::Shell` with the command `git status`, no `Cmd::Send`, and a running input with the `shell` model | The command goes to the agent as a prompt, or it does not run |
| SH-2 | An input of the agent runs, submit `!ls` | No command, the draft stays, and a notice tells why | A command changes the tree while the agent edits it, and the snapshots give the edits to the wrong input |
| SH-3 | Submit `!` with no command | No command, and a notice tells to type a command | An empty shell runs and the input shows nothing useful |
| SH-4 | The status line while the draft starts with `!` | The hint tells that Enter runs the command | The developer thinks that Enter sends the text to the agent |

## The end of a command

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| SE-1 | `ShellDone` with output and exit status 0 | The input is `Done`, its reply is the output, `Cmd::Finish` computes its changes, and no `Cmd::Summarize` | The output is lost, the changes of the command are not shown, or a summary costs money for no reason |
| SE-2 | `ShellDone` with exit status 2 | The reply ends with `exit status 2` | The developer thinks that a failed command passed |
| SE-3 | After a command, submit a usual input | The `Cmd::Send` text has the command and its output in `<bash-input>`/`<bash-stdout>` before the typed text. The input shows only the typed text | The agent does not know what the developer ran, or the input shows text that the developer did not type |
| SE-4 | Send a second input after SE-3 | Its `Cmd::Send` text has no command output | The agent reads the same output again with each input |
| SE-5 | `ShellDone` with an error (the command did not start) | The input is `Failed`, and the agent gets no context | The agent reads a command that never ran |
| SE-6 | An input submitted while the command runs | It waits in the queue, and starts with `Cmd::Send` when the command ends | The input goes to an agent that does not run, and the input is lost |

## Ctrl+C on a running command

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| SC-1 | A command runs, push Ctrl+C | One `Cmd::Stop` for the session, then `ShellDone` with no exit code shows that the developer stopped it | The developer cannot stop a long command, for example a server |
