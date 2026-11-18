# live Spec

App: `because-tui`
Test: `crates/because-tui/tests/live.rs`

## One session

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| LV-1 | Type an input and push Enter | One running segment and one `Cmd::Send` | The input does not go to the agent |
| LV-2 | Replay the first recorded input | The segment is `Done`, with the tool call, the text, the reply, and one `Cmd::Finish` | The column does not show what the agent did, or the changes are not computed |
| LV-3 | Replay the two recorded inputs | Each input costs the difference between two totals | Each input shows the cost of the full conversation |
| LV-4 | Type a second input while the first runs | One `Cmd::SendMidTurn`, no new segment, no queue. The echo before `Finished` adds the text to the running input. No `Cmd::Send` after `Finished` | The agent does not get the text until the input ends, or gets it two times |
| LV-5 | The agent reads an input sent while it worked only after `Finished` | A new running segment for that text, and no `Cmd::Send` | The response to the text has no input, and its events are lost |

## Several sessions

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| SS-1 | Start a new session and type an input | The new session is empty, and the input is its own | Inputs go to the wrong conversation |
| SS-2 | Type in session 2 while session 1 works | Both inputs go out at once | One session waits for a different session |
| SS-3 | The two sessions end in a different order | Each `Finished` ends the input of its own session | The reply of one session goes to the other |
| SS-4 | Type a second input in session 2 while it works | `Cmd::SendMidTurn` for session 2 | Text goes to the agent of a different session |
| SS-5 | The same input runs in two sessions | Each costs the same, measured in its own session | A session shows the cost of another session |
| SS-6 | The Sessions column while session 1 works and session 2 shows | The new session, the `⟳` mark, and "1 working elsewhere" | The developer thinks that no agent works |

## Session names

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| NM-1 | A new session, then its first input, then its summary | "new session", then the input, then the summary, with the correct colour | The developer cannot find a session in the list |
| NM-2 | A new session with an input that has not run | The input names the session | The session shows no name |
| NM-3 | A session with a title from the store | It keeps the title | Imported names are lost |
| NM-4 | A multi-line first input | The first line names the session | The name shows line breaks |

## Changes and progress

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| CP-1 | Changes arrive for an input | The response cites the hunk, and the code pane shows the file and what the agent said | The developer cannot find why a line changed |
| CP-2 | An input streams | The frame shows the activity, the tool call, and "working" | A long input looks stuck |
| CP-3 | The agent says nothing at first | "starting claude", then the quiet notice with the log path, then the activity | The developer cannot see why nothing occurs |
| CP-4 | The agent exits with a reason of several lines | The reply keeps the lines, with no empty duration | The reason for a failure is lost |
| CP-5 | Push Enter in the sample view | A notice that there is no agent | The developer thinks that the input went to an agent |
| CP-6 | The agent exits during an input | The input is `Failed`, the notice gives the reason, and nothing runs | The session cannot accept a next input |
| CP-7 | Push Enter when no agent is live | The input waits in the queue, and nothing goes out | The sample view sends input |

## Unread replies

| ID | Scenario | Assertion | Risk if broken |
|----|----------|-----------|----------------|
| UR-1 | A session ends while a different session shows | It is unread, marked `◆`, until the developer selects it | The developer does not see a reply |
| UR-2 | A session works while a different session shows | It is not unread | A running session looks finished |
