# `because`

`because` is a TUI with three columns. Use it to review the changes that an
LLM made, and the reasons for these changes. The command is `cuz`.

You do not read a long diff. You read the work. The left column shows the
sessions. The middle column shows your inputs to the agent and the response to
each input. The right column shows the files that each input changed. One key
shows the code.

In the design, a small model writes a summary of each change when the agent
stops. SQLite keeps the summary with the commit, the file, and the patch. Thus
the summary stays after the session ends, and you can share it through the
repository.

## Status

Status: **walking skeleton with a live agent.**

- `cuz` (or `cuz live`) runs Claude Code as the agent in your repository.
- A small model writes a summary of one sentence for each completed input.
- A local SQLite store keeps each session. It also keeps your Claude Code
  sessions for the repository, which `cuz` reads from their transcripts. Thus
  your earlier work is available the next time.
- The full summarizer (sections, summaries for each file, risk) and the shared,
  committed records have a design. They do not exist yet.

## Build and run

```
cargo build --release
cd /path/to/some/git/repo
/path/to/because/target/release/cuz
```

### Prebuilt binaries

The workflow `.github/workflows/build.yml` builds and tests each push on three
runners. The **Artifacts** section of the run contains one file for each
platform:

| Platform | File |
|---|---|
| macOS, Apple Silicon (macOS 11 or later) | `cuz-aarch64-apple-darwin.tar.gz` |
| Linux, x86_64 (static, no glibc dependency) | `cuz-x86_64-unknown-linux-musl.tar.gz` |
| Windows, x86_64 | `cuz-x86_64-pc-windows-msvc.zip` |

When you push a tag `v*`, the workflow also publishes a GitHub Release with
these files and `SHA256SUMS`. To make a release, push a tag:

```
git tag v0.1.0 && git push origin v0.1.0
```

Use the release to install `cuz` without Rust or crates.io. On macOS or Linux,
use the file for your platform:

```
mkdir -p ~/.local/bin
curl -fsSL https://github.com/jbehadev/because/releases/latest/download/cuz-aarch64-apple-darwin.tar.gz \
  | tar -xz -C ~/.local/bin
```

Make sure that `~/.local/bin` is in your `PATH`.

On Windows, use PowerShell:

```
$dir = "$env:LOCALAPPDATA\Programs\cuz"
New-Item -ItemType Directory -Force $dir | Out-Null
Invoke-WebRequest https://github.com/jbehadev/because/releases/latest/download/cuz-x86_64-pc-windows-msvc.zip -OutFile "$env:TEMP\cuz.zip"
Expand-Archive -Force "$env:TEMP\cuz.zip" $dir
```

Then add that directory to your `PATH`. On Windows, `!` runs the command with
`cmd.exe`.

The binaries do not have a code signature. If macOS does not let a downloaded
copy run, remove the download flag with `xattr -d com.apple.quarantine cuz`.
If Windows SmartScreen stops the program, select **More info**, then
**Run anyway**.

## Use

Type an input and push Enter. Claude Code does the work in that repository.
While the agent works, the middle column shows what it does. When the agent
stops, the middle column shows a summary of one sentence (`↳`). The right column
shows the files that the agent changed.

**Enter on an input shows the full input.** This view shows all the text that
you typed and all the text that the agent wrote. The Markdown is rendered, and
you can scroll it. Use these keys:

- `j`/`k`, `Space`/`b`, `g`/`G`: scroll.
- `]`/`[`: go to the next or the previous input.
- `Esc`: go back.

If the composer is empty, Enter does the same for the selected input.

Push Tab to go to the code screen and see the changed files. On a line, push `W`
to see what the agent said when it wrote that line.

If you type an input while the agent works, Enter sends it to the agent, as in
Claude Code. The agent reads your text at its next step, for example after a
tool call. Use this to correct the agent while it works. If the agent reads the
text before the input ends, `cuz` adds the text to that input. If the agent reads
it after the input ends, the text becomes a new input.

**To make the agent read your text now, push Ctrl+Enter** (Claude Code's "send
now"). `cuz` sends the text and interrupts the running input. The agent then
reads the text as a new input. If your terminal sends Ctrl+Enter as Enter, push
Ctrl+X, then Ctrl+S.

> **Caution:** Send now stops the tools that run, for example a long build.
> Claude Code moves these tools to the background. `cuz` cannot do this through
> the CLI.

### Shell commands

**To run a shell command, start the input with `!`**, for example
`!cargo test`. The command runs in the tree of the session, with your `$SHELL`.
It becomes an input of its own, and the reader shows its output. The Changes
column shows the files that the command changed.

As in Claude Code, the agent reads the command and its output with your next
input. The agent gets the last 30,000 characters of the output.

> **Caution:** The command has no terminal. A command that asks for input gets
> end of file. Do not run an editor or a pager.

You can run a command only when the agent of the session does not work. The
command and the agent use the same tree. An input that you send while a command
runs waits until the command stops. To stop a command, push Ctrl+C.

## Sessions

To start a new session, push `n` in the Sessions column, or push `Alt+n` from
any location. A new session is a second conversation in the same repository. It
does not know about the first session. It keeps its own inputs and its own
snapshots.

Sessions run in parallel. Each session starts its own `claude` on its first
input. Thus, while one session works, an input that you type in a different
session goes out immediately. The Sessions column shows `⟳` for each session
where the agent works.

### Worktrees

Two agents must not edit the same checkout. Thus **each session after the first
works in its own git worktree**. `cuz` makes the worktree on the first input of
the session, in `<repo>.sessions/<n>-<id>`, on a new branch.

The worktree starts as a copy of your working tree at that time, with the
uncommitted edits. It does not include ignored files. Thus `cuz` does not copy
build output, and the first build in that session is a full build.

`cuz` shows the paths when you quit. When you do not need a worktree, remove it
with `git worktree remove <path>`.

### Earlier sessions

**Earlier sessions are below the sessions of this run**, under an `── earlier`
rule. The newest session is first. The list contains:

- The sessions that earlier runs of `cuz` hosted.
- The sessions that you ran in Claude Code in this repository.

Each time that `cuz` starts, it reads the Claude Code transcripts in
`~/.claude/projects/`. It reads only the new or changed transcripts. To skip
this step, use `--no-import`.

Earlier sessions are read-only. Enter on an input opens the full input, and Tab
opens its code. If you type in an earlier session, `cuz` tells you that it is
read-only. It keeps your draft for a new session (`n`).

**Push `c` on an earlier session to try to revive it.** `cuz` runs
`claude --resume` in the tree where the session ran. This has two conditions:

- The tree must still exist.
- The Claude Code transcript of the session must still exist.

If one of these is missing, or if `claude` stops a few seconds after it starts,
the status line tells you why. The session then stays read-only. A revived
session moves up into the sessions of this run, and it accepts inputs again.

A revived session can be from the repository where you started `cuz`. Then it
shares that checkout with the first session, and the notice tells you this.
Each of the two sessions counts the edits of the other session as its own.

A transcript does not record the changes that a shell command made. Thus the
files of an imported input are the commits that occurred while the input ran.
`cuz` finds these commits by time and labels them that way. If you make a commit
manually while the agent works, `cuz` counts that commit too.

To do the same import outside the interface, run `cuz import-claude`. It shows
what it found.

## Before you use it

- **It edits your files.** Use a branch that you can discard. The default
  permission mode is `acceptEdits`. In this mode, file edits occur without a
  question. Other calls that need approval, for example a shell command or a
  fetch, stop the agent. A prompt then opens and shows the full call. Use these
  keys:
  - `a`: allow the call one time.
  - `A`: allow calls of this type for the remainder of the session. This uses
    the rule that Claude Code suggests, for example `Bash(cargo test:*)`. `cuz`
    keeps the rule in the session. It does not write the rule to your settings.
  - `m`: allow the call and change the session to `auto` mode. After this, the
    Claude Code classifier makes the decisions.
  - `d`: deny the call. The agent continues without it.

  When the agent asks you a question, a prompt shows the question and its
  options. Push a number or Enter to choose. On a question that allows more
  than one answer, push Space to select options, then Enter. To type a
  different answer, choose "Other". Push Esc to decline the questions.

  Prompts from more than one session wait in a queue. To select a different
  mode, use `--permission-mode`. The `default` mode also asks about edits. The
  `auto` mode lets the Claude Code classifier decide.
- **It uses your Claude Code login and model.** To use a different model, use
  `--model`.
- **The allowance that remains shows at the bottom of the Sessions column.**
  Claude meters a subscription over more than one window: five hours and one
  week. There is one bar for each window. Each bar shows the allowance that is
  *left*. `cuz` gets the value again every two minutes.

  No CLI flag gives this value. Thus `cuz` uses the same endpoint as `/usage`,
  with the OAuth token from your Claude Code login. This token is in the macOS
  Keychain or in `~/.claude/.credentials.json`. The first read can ask you to
  allow keychain access.

  An API-key login has no subscription allowance. Then the bar shows
  `unavailable`, and the status line and the log give the reason. Only the bar
  uses this value. No other function changes.
- **Each summary costs approximately a fifth of a cent.** After an input that
  gave more than 30 words or used a tool, `cuz` makes one
  `claude -p --model haiku` call with thinking off. We measured a cost of
  $0.001–0.002 and a time of approximately 1.3 s. `cuz` shows shorter answers
  without a summary. To stop summaries, use `--no-summary`. To select a
  different model, use `--summary-model`. The log shows the cost of each call.
- **`cuz` does not change your branches or your index.** Before and after each
  input, `cuz` records the working tree as commit objects under
  `refs/because/<session>/`. These commits are not on a branch and not in
  your index. To remove them, run this command:
  `git for-each-ref --format='%(refname)' refs/because | xargs -n1 git update-ref -d`
- **The store is `.git/because/slicer.db`.** It is in the git directory.
  Thus it is never part of your working tree or of a snapshot, and the session
  worktrees share it. It holds the text that you typed, what the agent said and
  did, costs, and summaries.

  `cuz` reads the code from git when you look at an earlier input. If you remove
  the `refs/because/` snapshots (above), you lose the code view of
  earlier sessions, but nothing else. The store is a cache. If you delete it,
  you lose the history of `cuz`. `cuz` imports the Claude Code history again.
- **Ctrl+C stops the running input of the session on screen.** If the
  composer has a draft, the first Ctrl+C clears the draft. If an approval prompt
  shows, Ctrl+C stops the input of that prompt. The agent keeps the
  conversation, and the input stays in the list with its changes. Text that you
  sent while it worked, and that the agent did not read, becomes the next input.
  When no input runs, Ctrl+C quits. Thus a second
  Ctrl+C quits. Esc does not stop an input, because Esc moves out of the
  composer.

## If Claude does not work

- **Make sure that you are in a live session, not in `cuz demo`.** The status
  line starts with `● live`. `cuz demo` shows sample data and has no agent. If
  you push Enter in `cuz demo`, it tells you this.
- **Run `cuz --check` in the repository.** It runs each step that `cuz` needs,
  outside the interface, and shows the result of each step:
  - The repository.
  - The `claude` that it found.
  - The allowance that is left.
  - A git snapshot.
  - The exact command.
  - One small input to Claude.
  - One summary call.

  All of these steps cost approximately two cents. If a step fails,
  `cuz --check` shows the output of `claude`.
- **If you start Claude through a shell alias, other programs cannot see it.**
  To find the real path, run `type claude`. Then give the path to `cuz` with
  `cuz --claude /that/path`.
- **`cuz` logs all `claude` output** to `.git/because/<session>.log`.
  Each run has one log file, which contains all the sessions that the run
  started. Each session in the file starts with the `# claude …` command line
  that identifies it. `cuz` shows the path when you quit. It also shows the path
  on the screen if Claude gives no output for 20 seconds.

## Without a live agent

```
cargo run -p because-cli -- demo              # show the sample sessions (needs a terminal)
cargo run -p because-cli -- render 150x40 "alt-up,alt-3,alt-right,k,W"
                                         # render one frame without a terminal
cargo run -p because-cli -- keys              # show the bytes your terminal sends for each key, and the action
cargo test                               # run 210 tests (no TTY or network necessary)
python3 scripts/pty_keys.py              # test the real binary with real terminal bytes (pip install pyte)
```

## Keys

The arrow keys move between panes until you type in the composer. After that,
the arrow keys edit your text, and **Tab / Shift+Tab** move between panes. On
macOS, Option+←/→ does the same. `Esc` moves out of the composer, so plain keys
are commands again. `i` moves back into the composer.

## Design documents

- [docs/design/](docs/design/): the plan of record.
- [Overview and architecture](docs/design/00-overview.md)
- [Roadmap and milestones](docs/design/06-roadmap.md)
