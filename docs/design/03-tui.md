# TUI: Framework Choice, Event Loop, Layout

## 1. "Can we build on top of another TUI?"

Yes, and the answer is **ratatui** — but it is worth being precise about what
that does and does not save.

There are two different things you might mean by "build on another TUI":

**(a) Reuse a widget/terminal framework.** This is the real win and ratatui is
the answer. It is the maintained successor to `tui-rs` and provides layout
solving (constraint-based), a widget set (`List`, `Table`, `Paragraph`,
`Scrollbar`, `Block`), double-buffered diffed rendering, and a backend
abstraction over crossterm. It is what `gitui`, `bottom`, `atuin`, `oxker` and
`television` are built on. Not using it means writing a layout solver and a
damage-tracking renderer, which is weeks of work and a bug farm.

**(b) Fork an existing application and bend it into shape.** This is where the
saving usually fails to materialise. Surveying the plausible candidates:

| Candidate | Why not |
|---|---|
| `gitui` | Closest UI shape (panes, diff view, syntax highlighting) but its model is "operate on a git repo"; sessions, segments, sections and an agent loop are not extensions of that model, they replace it. You would keep the diff widget and discard the rest — so take the idea, not the fork. |
| `zellij` plugin (WASM) | It is a terminal multiplexer. You would get pane management you don't need and inherit a plugin ABI constraint on the part of the system that most needs to evolve. |
| `cursive` | Callback/closure-driven with an internal event loop that fights an async agent stream. Less active than ratatui. |
| `helix` / editor forks | Enormous surface area for the one thing you'd want (a good diff/text view). |

So: **ratatui + crossterm from a clean start, reusing widget crates
aggressively.** What you actually avoid writing:

| Need | Crate | Version at time of writing |
|---|---|---|
| framework | `ratatui` | 0.30.2 |
| terminal backend | `crossterm` | 0.29.0 |
| composer (multiline, undo) | `ratatui-textarea` | 0.9.2 — **not** `tui-textarea` 0.7.0, which pins ratatui 0.29; alongside 0.30 cargo resolves two ratatui versions and its `Widget` impl will not render into our `Frame`. ratatui-textarea is built on `ratatui-core` 0.1, the same core as ratatui 0.30 |
| scrollable long diffs | `tui-scrollview` | 0.6.7 |
| spinners for pending summaries | `throbber-widgets-tui` | 0.11.1 |
| agent/tool output with ANSI colour | `ansi-to-tui` | 8.0.1 |
| syntax highlighting | `syntect` 5.3.0 + `two-face` 0.5.2 | bat's grammar/theme set |
| diffing | `imara-diff` 0.2.0 | Histogram diff, fast; `similar` 3.2.0 if you want word-level intra-line |
| git | `gix` 0.87.1 | pure Rust, no libgit2 link; `git2` 0.21 as fallback if a gix API is missing |
| panic/error reports | `color-eyre` 0.6.5 | must install a hook that restores the terminal first |

Pin exact versions in `Cargo.lock` and check for newer releases at
implementation time — ratatui in particular has had breaking releases.

## 2. Event loop

One thread owns the `Terminal`. Everything else talks to it over channels.

```rust
// because-tui/src/app.rs
pub enum Msg {
    Key(KeyEvent),
    Resize(u16, u16),
    Tick,
    Agent(AgentEvent),     // streamed text delta, tool started, segment ended
    Job(JobEvent),         // summary landed / failed for target id
    Store(StoreEvent),     // rows changed
}

pub fn update(model: &mut Model, msg: Msg) -> Vec<Cmd>   // pure; returns work to do
pub fn view(model: &Model, frame: &mut Frame)            // pure; no I/O, no locks
```

Elm-shaped on purpose: `update` is synchronous and pure over the `Model` and
returns `Cmd`s (spawn a request, enqueue a job, write to the store) that the
runtime executes. It makes the whole UI testable without a terminal — feed a
`Vec<Msg>`, assert on the `Model` — and keeps async out of rendering, which is
where async in TUIs goes wrong.

The runtime:

```rust
loop {
    tokio::select! {
        Some(Ok(ev)) = term_events.next() => dispatch(Msg::from(ev)),
        Some(ev)     = app_rx.recv()      => dispatch(ev),
        _            = render_tick.tick(), if model.dirty => { term.draw(|f| view(&model, f))?; }
    }
}
```

- `crossterm::event::EventStream` for keys, so input is just another async
  source instead of a polling thread.
- **Render only when dirty**, capped at ~30 fps. An agent streaming tokens can
  emit hundreds of deltas per second; coalesce them into the model and let the
  tick decide when to paint. Without this, the TUI burns a core echoing text.
- Terminal restoration on *every* exit path: `Drop` guard plus a `color-eyre`
  panic hook that leaves raw mode and the alternate screen before printing. A
  panic that leaves the terminal in raw mode is the single most user-hostile TUI
  bug.


## 3. Views and the focus ring

Three views, four focus stops. The code pane is full-screen, so it cannot be
focused at the same time as the columns — which means the focus ring spans a
view change.

```rust
pub enum View  { Columns, Code, Reader }
pub enum Focus { Sessions, Segments, Changes, Code }
```

The reader (§5) is not a focus stop. It is opened on purpose — `Enter` on an
input, `r`, or `Alt+r` from the composer — and closes back to the pane it was
opened from with `Esc`. A key that moves to a pane (`Tab`, `1`–`4`, `Alt+→`,
`i`) closes it and moves from that pane.

```
Focus:   Sessions ⇄ Segments ⇄ Changes ⇄ Code
View:    └──────── Columns ───────────┘   └Code┘
```

Moving focus right off `Changes` enters the code pane and switches the view;
moving left off `Code` returns to `Changes` and switches back. The mode change
is a consequence of movement, so there is one gesture to learn rather than two.

State is retained on both sides of the boundary: leaving the code pane and
coming back restores scroll position, revision selection and expanded
deletions, keyed by `file_change.id`. Bouncing between a summary and its code
must be free, because that is the core loop of using this tool.

## 4. Keys

### The constraint: the Segments pane is where you type

The Segments column holds a composer for typing a new segment (§5). That one
fact drives the whole keymap: **while the composer has focus, every plain
printable key is text.** `j`, `k`, `1`–`4`, `v`, `t`, `q` all have to type
themselves, so none of them can be the only way to navigate or toggle. Anything
structural has to live on a key that a text box does not need.

### What the composer already owns

The composer is `ratatui-textarea` (see §1 for why not `tui-textarea`). Its
default keymap, read from the 0.9.2 source, claims more than it looks like it
would:

| Key | ratatui-textarea uses it for | Consequence |
|---|---|---|
| `Ctrl+←/→` | word left / right | **cannot be pane movement** |
| `Ctrl+↑/↓` | paragraph up / down | cannot be list movement |
| `Ctrl+W` | delete word | the `Ctrl+w h/l` window chord is gone |
| `Ctrl+C` | copy | taken back for clear/quit (below) |
| `Ctrl+J` | delete to line head | taken back for newline (below) |
| `Tab` | insert tab | taken back for focus; a literal tab in a prompt is expendable |
| `Alt+v` | page up | taken back for verbatim; `PageUp` still scrolls |
| `Alt+b/f/h/d/n/p/</>/[/]` | word/paragraph movement and deletion | left alone |
| **`Alt+←/→`, `Alt+↑/↓`, `Alt+1…4`** | **nothing** | **free — this is the navigation layer** |

So the answer to "which modifier" turned out to be decided by the text box, not
by preference: Alt+arrows are the only arrow chords a text editor leaves alone —
**on Linux and Windows.** On macOS the terminal decides first, and Option+←/→
never arrives as Alt+←/→ at all; see "macOS" below.

### The Ctrl+arrow question, settled

An earlier revision kept `Ctrl+←/→` as a synonym for pane movement. It is now
dropped entirely, for three independent reasons:

1. In the composer it is word movement, in ratatui-textarea and in every text
   field on Linux and Windows. Stealing it breaks muscle memory.
2. Keeping it outside the composer only would make it mean "change pane" in
   three panes and "jump a word" in the fourth — the kind of mode-dependence
   that sends the cursor somewhere unexpected.
3. macOS binds it to Mission Control by default and Terminal.app does not emit
   it (the original objection).

### Three layers

Dispatch lives in `crates/because-tui/src/keymap.rs`. While an approval prompt is
up (§5c) it takes every key but `Ctrl+C`, which stops the input of that
prompt (and quits on a second press). Otherwise it
checks, in order:

**1. Global — works in every pane, including while typing.**

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | next / previous focus stop |
| `Alt+←` / `Alt+→` | focus left / right, across into the code pane |
| `Alt+↑` / `Alt+↓` | move the selection in the focused list — in the composer, the segment list, so you can browse without losing the draft |
| `Alt+1` … `Alt+4` | jump to a stop |
| `Alt+v` / `Alt+t` / `Alt+w` / `Alt+r` | verbatim / transcript / why / read — the same as plain `v` `t` `W` `r` |
| `Alt+n` | start another session (§5a). The one Alt key that is not its plain key everywhere: plain `n` keeps its code-pane meaning |
| `Ctrl+C` | clear the draft if there is one, else stop the running input of this session, else quit |
| `Ctrl+Enter`, `Ctrl+X` `Ctrl+S` | send now: send the draft and interrupt the running input (§5d). With no input running, the same as `Enter` |

**2. Composer — while typing.** Everything not listed goes to the editor, so
`Ctrl+A/E`, `Ctrl+←/→`, `Ctrl+W`, `Ctrl+K`, `Ctrl+U` (undo), `Backspace` and the
rest behave as they do in any text field.

| Key | Action |
|---|---|
| `Enter` | queue the segment; with nothing typed, read the selected input in full |
| `Alt+Enter` | newline — arrives as `ESC CR`, which every terminal can send |
| `Shift+Enter` | newline, where keyboard enhancement is active |
| `Ctrl+J` | newline — in raw mode LF is distinct from CR |
| `Esc` | leave the composer; plain keys become commands; the draft is kept |
| `←` `→` `↑` `↓`, and Option+←/→ | **while the draft is empty**: navigate — ←/→ change pane, ↑/↓ browse segments. There is nothing to edit, so nothing is lost. **Once there is text**: the editor's (cursor and word movement) |

**3. Command — everywhere else.**

| Key | Action |
|---|---|
| `i` / `a` | go to the composer, from any pane including the code pane |
| `1`–`4`, `h`/`l`, `←`/`→` | focus |
| `j`/`k`, `↑`/`↓` | move selection. The Segments and Sessions lists stop at their first and last entries rather than wrapping: they are timelines, and jumping from the latest input to the first was confusing in use — with history under this run's sessions, a wrap would cross the whole list |
| `Enter` / `Esc` or `q` | open / ascend. `Enter` opens what is selected: a session's inputs, an input in the reader, a file in the code pane — or, for an input that changed no files, the reader |
| `v` `t` `W` `r` `/` `f` `R` `d` `Q` | as in the global actions table below |
| `n` | start another session (Sessions column) |
| `c` | revive the selected earlier session (§5b); free everywhere else |
| `n` / `N` | next / previous region (code pane) |

**In the reader** the command keys scroll instead: `j`/`k`, `Space`/`b`,
`g`/`G`, PageUp/PageDown, Home/End, and `]`/`[` for the next/previous input
(§5).

User-facing summary, and the status line says which mode is active:
*Alt moves and toggles from anywhere. Plain keys type when you are typing and
command when you are not. Esc switches.*

### Why Enter submits, and what an earlier revision got wrong

An earlier revision said "`Ctrl+Enter` sends". That does not work: in the legacy
encoding `Ctrl+Enter` and `Enter` both send CR, and crossterm reports them
identically unless keyboard enhancement is on. So:

- `Enter` submits, because it is the one thing every terminal sends reliably.
- Newline has three routes because none is universal on its own: `Alt+Enter`
  (ESC-prefixed, works everywhere Alt works), `Ctrl+J` (LF — crossterm's parser
  only maps `\n` to Enter outside raw mode, so in raw mode it is distinct), and
  `Shift+Enter` where the terminal supports the kitty keyboard protocol.
- **Bracketed paste is mandatory.** Without it a pasted newline is
  indistinguishable from `Enter`, and pasting a stack trace would submit its
  first line. `cuz` enables it on startup and inserts `Event::Paste` as text.

### Ctrl+C clears, then stops, then quits

`Ctrl+C` with a non-empty draft clears the draft. With an empty draft, it stops
the running input of the session on screen. When no input runs, it quits. Thus
the second press after a stop quits, as in Claude Code. A session that is not on
screen does not stop: the developer cannot see what they stop. `Esc` does not
stop an input, because `Esc` is the key that leaves the composer.

The
clear goes through ratatui-textarea's undo history (`TextArea::clear` is
implemented as a cursor jump plus `delete_str`), so `Ctrl+U` brings a cleared
draft back. The failure this prevents is losing a long, careful prompt to a
reflexive `Ctrl+C`.

### macOS: no arrow chord is free, so empty means navigate

This section was rewritten after the Alt+arrow scheme failed on a Mac. The cause
is in the terminal, not the keymap, and it is worth knowing precisely:

| Chord on a Mac | What happens | Usable for panes? |
|---|---|---|
| Option+←/→ | Terminal.app and iTerm2 send `ESC b` / `ESC f` — the shell word-movement codes, because Option+arrow is word movement in every macOS text field. crossterm reports `Alt+b` / `Alt+f`, not `Alt+Left`. | only when nothing is typed |
| Ctrl+←/→ | switches Spaces in Mission Control; the terminal never sees it | no |
| Cmd+←/→ | taken by the terminal app | no |
| Shift+←/→ | text selection | no |

So on macOS there is no modifier+arrow chord that is both delivered and free
while typing. The keymap follows the rule that already applied to ↑/↓: **with
an empty draft, every form of arrow navigates** — plain arrows, `Alt+←/→` in its
CSI form, and `Alt+b`/`Alt+f` as Mac terminals send Option+arrows — because
there is no text for them to edit. Once the draft has text, they edit it, and
**Tab / Shift+Tab** is the pane key that works on every terminal while typing.
Outside the composer, `Alt+b`/`Alt+f` always move panes.

Verified by running the real binary in a pseudo-terminal and sending the exact
bytes (`scripts/pty_keys.py`). Before this change, plain → and `ESC f` from the
empty composer did nothing, while the Rust key tests passed — they had fed the
keymap an ideal `Alt+Left`, not the `ESC b` a Mac sends.

One encoding cannot be supported: some terminals send Alt+arrow as `ESC ESC [C`.
crossterm reads `ESC ESC` as a single Esc, so the key arrives as Esc, `[`, `C`
and leaves the composer. The pty script keeps this as an executable expectation,
so a crossterm change that fixes it will show up.

Option+*letter* has a separate problem: Terminal.app ("Use Option as Meta key"
off) and iTerm2 (Left Option key: Normal) compose characters by default —
`Option+v` produces `√`, `Option+t` produces `†`. Inside the composer that types
a symbol into the draft. Every Alt binding therefore has a non-Alt route: Tab
for panes, `Esc` then the plain key for anything else. Alt is faster, never
required. The fixes, for anyone who wants Alt letters: Terminal.app *Settings →
Profiles → Keyboard → Use Option as Meta key*; iTerm2 *Profiles → Keys → Left
Option key → Esc+*; Ghostty `macos-option-as-alt = true`.

### The Esc/Alt ambiguity

Terminals encode Alt+X as ESC followed by X. crossterm disambiguates by whether
the next byte is already buffered when ESC is read: a lone ESC is reported only
if nothing follows in the same read. At human typing speed `Esc` then `v` are
separate reads. Over a slow link, or with keys buffered, they can coalesce into
`Alt+v`.

Making `Alt+X` perform the same action as plain `X` keeps most coalescing
harmless — the toggle still happens. What differs is that the composer is not
left, so the *next* key types rather than commands. Low frequency, recoverable
with backspace; worth knowing rather than engineering around.

### Global actions

| Key | Action |
|---|---|
| `i` | go to the composer from anywhere |
| `/` | FTS search over summaries; `n`/`N` cycles hits. Terms must be quoted before reaching `MATCH` — see `01-data-model.md` §5 |
| `/!` | same search, widened to include narration — what the model actually wrote |
| `v` / `Alt+v` | toggle the Segments column between generated labels and verbatim model text |
| `t` / `Alt+t` | the whole session's transcript — every input and its narration — as a modal |
| `r` / `Alt+r` | read the selected input in full (§5, reader); `Enter` does the same in the Segments list, in an empty composer, and in Changes when the input changed no files |
| `f` | filter changes by risk level |
| `d` | toggle summary ⇄ patch for the selected change (inline, in the Changes column) |
| `R` | requeue the summary for the selection (stale or failed) |
| `Esc` | ascend or leave the composer. It does not stop an input: while an agent works, Esc must still leave the composer |
| `?` | help overlay, generated from the keymap |
| `Q` / `Ctrl+c` | quit. Each `Ctrl+c` does the first of these that applies: clear a draft, stop the running input of the session on screen (or of the approval prompt), quit. Plain `q` ascends like `Esc`, so a stray press never exits |

`d` and the code pane are different tools on purpose: `d` is a quick look at the
patch without leaving the list, the code pane is for reading the change in the
context of the whole file.

`n`/`N` is overloaded — search-hit cycling when a search is active,
highlighted-region cycling in the code pane otherwise — and resolved by `Focus`
plus a `search_active` flag.

### Making it diagnosable

At startup `cuz` calls `crossterm::terminal::supports_keyboard_enhancement()` and,
when true, pushes `DISAMBIGUATE_ESCAPE_CODES` (kitty, foot, WezTerm, Ghostty,
partially Windows Terminal), popping it on exit.

**`cuz keys`** (built) is the probe. It runs under the same terminal settings as
the TUI and prints, for each press, how it arrived and what it does in each of
the three contexts — empty composer, while typing, other panes:

```
arrives as             empty composer     while typing       other panes
Alt+b                  pane ←             edit text          pane ←
                       ↳ macOS Terminal and iTerm2 send Option+← like this (ESC b)
√                      edit text          edit text          (nothing)
                       ↳ Option is typing symbols instead of sending Alt. …
Tab                    next pane          next pane          next pane
```

It also says whether keyboard enhancement is on, which decides whether
Shift+Enter is distinguishable from Enter. "The shortcut does nothing on my
machine" becomes a one-command answer. It will move under `cuz doctor` when that
command exists.

### Mouse

`EnableMouseCapture` behind a config flag, default on: click a column or row to
focus it, wheel scrolls the focused pane, click a gutter marker in the code pane
to jump to that change. `m` toggles capture off and on, because capture takes
terminal text selection away — and with `Ctrl+C` no longer copying in the
composer, terminal selection is the copy path.

## 5. Columns view

Rendered by the skeleton (`cuz render 120x26 "alt-up,alt-up"`), not drawn:

```
┌ 1 Sessions ────────────┐┌ 2 Segments ─────────────────────────────────┐┌ 3 Changes ──────────────────────────────────┐
│▸ hidden-path checks fo…││▸  2 › now add a variant that takes a plain  ││▸ ◈ crates/ignore/src/pathutil.rs            │
│  4 inputs · $1.31      ││       Path. incremental checking won't have ││    New is_hidden_path for callers without   │
│                        ││       a DirEntry to pass in                 ││    a DirEntry — on Windows it costs a       │
│  global gitignore igno…││       ↳ Added is_hidden_path(&Path). On     ││    metadata() call per check. The           │
│  2 inputs · $0.78      ││         Windows it stats the path to read   ││    leading-dot test is extracted into       │
│                        ││         the HIDDEN attribute; elsewhere it …││    is_hidden_path_only and shared with      │
│  globset allocation cl…││         1 file · 4m 20s · $0.52 · 6e527e9   ││    is_hidden_entry.                         │
│  1 input · $0.38       ││         Enter reads all 62 words            ││    3 hunks   +59 −14   perf                 │
│                        ││         ┊ 1 add is_hidden_path for callers… ││      ▸ @@use std::{ffi::OsStr, path::Path…  │
│  + n new session       ││         ┊ 2 share the leading-dot check     ││      ▸ @@use crate::walk::DirEntry; — is_…  │
│                        ││                                             ││      ▸ @@pub(crate) fn is_hidden_entry(de…  │
│  ripgrep · master      ││   3 › pathutil still has leftovers from     ││                                             │
│                        ││       before the refactor — tidy it up      ││                                             │
│                        ││       ↳ Collapsed the remaining cfg-duplica…││                                             │
│                        ││         1 file · 3m 48s · $0.44 · c55818f   ││                                             │
│                        ││                                             ││                                             │
│                        ││   4 › does is_hidden_path do anything       ││                                             │
│                        ││       different on Windows? the commit says…││                                             │
│                        ││       ↳ Yes: on Windows it stats the path t…││                                             │
│                        ││         no file changes · 0m 38s · $0.06    ││                                             │
│                        ││                                             ││                                             │
│                        │├ ✎ next input ───────────────────────────────┤│                                             │
│                        ││ type the next input · Enter to send         ││                                             │
└────────────────────────┘└─────────────────────────────────────────────┘└─────────────────────────────────────────────┘
 claude-opus-5  input $0.52 · session $1.31 · 6e527e9 · arrows move · Enter read · type to write · Tab panes · Esc comma
```

Constraints `[Percentage(22), Percentage(40), Percentage(38)]`. Breakpoints,
set by `column_layout`:

| Width | Layout |
|---|---|
| ≥ 140 | three columns as above *(built)* |
| 100–139 | three columns, Sessions fixed at `Length(26)`, Segments and Changes splitting the rest 55 / 45 *(built)* |
| 60–99 | two columns, following the focus: Sessions + Segments when Sessions has it (Sessions widened to 40%, at least 26), otherwise Segments + Changes 55 / 45 *(built)* |
| < 60 | the focused column alone, full width *(built)* |

Segments is the column kept readable — it is what the screen is for, and it
wraps where the others cut — so as the terminal narrows, whether from a
smaller window or a larger font (the font is the terminal's, not ours),
Changes gives up width first, then whole columns go. A column off screen keeps
its number: `1`–`3`, `h`/`l` and `Tab` reach it as before, and it comes back
with the focus. A breadcrumb for the single-column width
(`session ▸ segment ▸ section`) is still planned.

The code pane is full-screen at every width, so it needs no breakpoints — one
reason to prefer it over a permanent fourth column.

**The composer lives at the bottom of the Segments column**, joined to the list
by a `├ ✎ next input ┤` rule. It is 3 rows when empty and grows with the draft
up to 8, taking height from the list above it. This replaces an earlier plan for
a separate bottom pane: typing the next segment next to the segments it follows
keeps the context on screen and makes the middle column read as a timeline —
the agent's segments, then yours.

Behaviour, all implemented in the skeleton and covered by `tests/keys.rs`:

- **Arriving in the Segments pane lands in the composer** the first time; after
  that it returns to whichever of list or composer you left, so coming back
  from Changes after pressing `Esc` does not silently put you back into typing.
- **`Enter` sends** the draft as the next input. It becomes the next segment
  of the session — four inputs, then a fifth — shown in the human-authored
  colour. While the agent of the session works, `Enter` sends the draft to
  that agent immediately, as Claude Code does (§5d). Only the sample view, which
  has no agent, shows `queued · runs when the agent is idle`.
  The new segment **becomes the selection**, so it is scrolled into view, and
  the Changes column says plainly that nothing has run yet rather than going
  blank. A queued input belongs to its session; switching sessions does not
  carry it along.
- **List scrolling** prefers showing the tail (where a new segment appears) but
  never scrolls the selected segment's first line off the top.
- Moving right from Changes when a queued segment is selected does not open an
  empty code pane.

### 5a. Starting another session

`n` in the Sessions column, or `Alt+n` from anywhere, adds a session and selects
it with the cursor already in its composer. The column carries a `+ n new
session` line under the list so the key is visible where it applies.

A session is one conversation, so a new one is a genuine separation and not a
filter over the same one — and the sessions run **side by side**: typing in one
while another is working sends the input straight out.

| Kept apart | Shared |
|---|---|
| the `claude` process and its conversation — the new session knows nothing of what was said in the others | the repository's history and object database: every session's snapshots are in the one repository, and `git worktree list` has them all |
| the working tree: every session after the first works in a worktree of its own (below) | the screen: one Sessions column, one Changes column, one code pane, showing whichever session is selected |
| the inputs, the queue, and therefore the Segments column — one conversation still serves one input at a time, so a second input typed *here* goes to the running input (§5d) | the draft: the composer is one text box, so what you had typed comes with you |
| the snapshot refs (`refs/because/<session>/…`) and the commits that attribute a line, so what another session wrote reads as *not changed in this session* in the code pane | the log (`.git/because/<run>.log`) and the cost line, which is per session |

The runtime creates everything lazily, on the session's first input: a session
you start and never type into costs no process, no id, no refs and no checkout.
The Sessions column marks every session the agent is working in with `⟳`, and
the status line reads `● live · N working elsewhere` when the work is not in the
session on screen.

**A working tree each.** Two agents editing one checkout would make the
before/after snapshots — `because`'s whole account of who changed what —
meaningless: each would record the other's edits as its own. So session 1 keeps
the repository you ran `cuz live` in, and every session after it gets a worktree
beside it, in `<repo>.sessions/<n>-<id>`, on a branch of its own
(`because/session-<n>-<id>`).

| | |
|---|---|
| it starts from | a snapshot of the main working tree, taken when the session's first input is sent — so edits that were never committed come with it |
| it does not start from | ignored files: a snapshot leaves them out, as `git add -A` would, so build output is not copied and the first build in a new session is cold |
| it costs | one checkout, on the first input only. This is the one slow step in the event loop |
| it is removed | never, by `cuz`. The paths are printed on the way out; `git worktree remove <path>` when the work in one is finished with |

The open question is how work done in a session worktree comes back — merge,
cherry-pick, or a review screen that shows two sessions' changes to one file
side by side. Until then the branch is the handle: the work is on it, and the
snapshots under `refs/because/` say which input produced what.

### 5b. History: earlier sessions

Sessions are kept (`01-data-model.md` §0), so `cuz live` opens with the ones
before it listed under this run's, newest first, below a `── earlier` rule:
sessions earlier runs of `cuz live` hosted, and sessions run in Claude Code
itself, read from its transcripts (`04-agent-host.md`, transcripts).

| | This run's sessions | History |
|---|---|---|
| where | top of the column; `n` adds one at the end of this block, so history moves down rather than new sessions landing under it | under `── earlier`, newest first |
| meta line | inputs · cost | date · inputs · cost, date first because the end of the line is what a narrow column cuts |
| Segments pane title | `Segments` | `Segments · earlier session` or `Segments · from Claude Code` |
| typing | sends | refused with a notice, the draft kept: its agent is gone. `c` tries to revive it (below); `n` starts a new session, which the draft carries over to |
| code | from the snapshots of this run | loaded from git when an input is selected (the store lists files; git has the code). The Changes column says `loading the code from git…` until it arrives, and the code pane waits for it |
| cost | per input | per input where it was recorded; `$—` where only a total covering several inputs was. The session total is shown when that was recorded |

An imported input's changes are the commits made while it ran, and the column
says so above the file list: *Matched by time … A commit made by hand meanwhile
would be counted too.* The meta line marks the commit `by time`, and the
reader's header `(by commit time)`. In this repository's own history that rule
tied 16 of 17 inputs to the right commits; the 17th picked up a commit the
developer made by hand while the agent was working.

**Reviving one.** `c` on an earlier session asks `claude --resume <id>` to
take the conversation up again, in the tree it ran in. Before starting it, the
runtime checks the two things a resume needs and would otherwise fail on less
clearly: that tree still exists and is a git repository, and Claude Code still
has the session's transcript (`<projects>/<slug of that tree>/<id>.jsonl`; it
deletes them after 30 days by default). Nothing moves while it starts. A
`claude` that is still running after four seconds has the conversation: the
session moves up to the end of this run's sessions, out of history, and takes
inputs again, with the lines its earlier inputs wrote still counted as its
own. One that exits sooner, or a failed check, leaves the session where it was,
read only, with the reason in the status line. Sending while it is still
starting is refused with a notice.

A revived session works in the tree it ran in, not a new worktree. When that is
a tree another of this run's sessions is already using — usually the
repository you started in, which session 1 always has — the notice says so,
since each will count the other's edits as its own.

### The allowance

At the foot of the Sessions column, in a live session only, is how much of the
Claude subscription's allowance is **left**: one bar per window the account is
metered over (the five-hour one, the weekly one, any others the endpoint
reports), coloured by what remains — red under 10%, amber under 25%. It is
pinned to the foot rather than following the list, so it stays put as
sessions are added. One poller serves the whole screen, since every session
spends the same allowance; it re-asks every two minutes (`04-agent-host.md`,
allowance). Before the first answer it reads `checking…`; if the query fails
it reads `unavailable`, the reason goes to the status line once and to the log
every time, and a bar that had a value keeps it under `usage left · stale`.
`cuz demo` shows no bar.

### 5c. Approval prompts

A tool call the session's permission mode does not allow (`04-agent-host.md`
§1, permission prompts) stops that session's agent and opens a modal over
whatever is on screen, titled `session N · <name> — approve?`. It shows the
tool, what it targets, and the call in full — a command line for line, not
reflowed, because that is what will run — then the answers:

| Key | Answer |
|---|---|
| `a` | allow once |
| `A` | allow calls like it for the rest of the session — shown only when Claude Code suggested a rule (e.g. `Bash(cargo test:*)`), and kept to the session, not written to settings |
| `m` | allow, and switch the session to `auto` mode, so Claude Code's classifier decides from then on |
| `d` | deny; the agent is told, and carries on without it |

The prompt takes the keyboard (§4), and ignores answers for the first 400 ms
it is up, with the keys dimmed: it opens over whatever was being typed, and a
keystroke meant for the draft must not answer it. Prompts from several
sessions queue, oldest first, with `N more waiting` on the frame. A request the
agent withdraws leaves the queue unanswered, and so does every request of a
session whose input finishes or whose agent exits; an answer that crosses a
withdrawal on the way is dropped, and the status line says so.

When the agent asks questions (`AskUserQuestion`), the modal shows one
question at a time, titled `session N · <name> — question 1 of 2`. It shows
the header, the question, and the options with their descriptions. The last
row, "Other", opens a text field for an answer that is not an option.

| Key | Answer |
|---|---|
| `↑`/`↓`, `k`/`j` | move the cursor |
| `1`–`9` | choose that option, or select it when the question allows more than one |
| `Space` | select or clear the option under the cursor |
| `Enter` | answer with the option under the cursor, or with the selected options |
| `Esc` | close the text field; with the text field closed, decline all the questions |

The answers go to the agent together, after the last question. The same grace
period, queue and withdrawal rules apply.

### What a session is called

A session is named after what was said in it, not after when it was started —
`live · repo · 2` told you nothing, and told you the same thing about every
session on screen. `cuz live` and `n` both open a session with no name, and the
first column names it in three stages, each in the colour of where the words
came from (§6, three provenance classes):

| | Shown | Colour |
|---|---|---|
| nothing typed yet | `new session` | neutral |
| the first input typed, queued or running | its first line, verbatim | human |
| that input's summary, once the summarizer answers | the summary | generated |

Only the *first* input names the session: a name that changed under you with
every input would be no name at all, and the data model agrees (`sessions.title`
is "generated after segment 1", `01-data-model.md`). A session that came with a
title — anything loaded from a fixture or the store — keeps it. An imported
session's title is the one Claude Code's model wrote for it, so it is shown in
the generated colour.

### What a segment shows: a summary, not the reply

The column is for scanning. A ~1,000-word reply, shown as it was, took the
selected segment's six lines and still said nothing about what the answer was;
there was nowhere to read the rest. So each segment is capped, and everything
it leaves out is in the reader:

| Part | Shown |
|---|---|
| the input | 2 lines, 4 when selected, ending in `…` when cut |
| the response | a generated summary, `↳` in the generated colour, 1 line (3 when selected); `↳ summarizing…` while one is on the way; otherwise the start of the reply, `❝` verbatim, 1 line (2 when selected) with Markdown markup removed |
| tool calls | under the selected input, the first 4, then `… and N more tool calls` |
| how to read it | under the selected input: `Enter reads all 1,025 words` (`r reads` when Enter would not) |

Replies of 30 words or fewer with no tool calls are shown as they are and not
summarized. How summaries are made: `02-summarization.md` §0.

### Reader

Full-screen, like the code pane: the input as typed, the summary with its
model, cost and `[generated]` tag, then the response verbatim — every text
block in order with tool calls in place, thinking where the model returned it,
and the files changed. From a real `cuz live` session in a pseudo-terminal at
150×40, trimmed and rewrapped to fit this page:

```
┌ input 1/1 · in full ──────────────────────────────────────────────────────────────────────┐
│ claude-sonnet-5-5 · 0m 14s · $0.03 · 588 words · de56bc6                                  │
│                                                                                           │
│ › Write a 600-word story about a lighthouse keeper, with a title as a Markdown heading    │
│   and one short bulleted list somewhere in it. Do not use any tools.                      │
│                                                                                           │
│ ↳ Agent wrote a 600-word story about a lighthouse keeper named Ansel who saves a fishing  │
│   boat during a storm using his signal lamp and foghorn.                                  │
│   [generated · claude-haiku-4-5-20251001 · $0.0013]                                       │
│                                                                                           │
│ ── ❝ response, verbatim ───────────────────────────────────────────────────────────────── │
│                                                                                           │
│ The Last Light at Marrow Point                                                            │
│                                                                                           │
│ Ansel Vore had kept the lamp at Marrow Point for thirty-one years, and in that time he    │
│ had learned that the sea never repeated itself, though it pretended to.                   │
│ …                                                                                         │
│ • Kerosene, enough for three nights                                                       │
│ • Matches, in a tin, dry                                                                  │
└────────────────────────────────────────────────────────────────────────────────────── 0% ┘
```

Keys: `j`/`k` or arrows scroll a line, `Space`/`b` or PageDown/PageUp a page,
`g`/`G` or Home/End the ends, `]`/`[` the next/previous input, `Esc`, `r`, `h`
or `←` back. Scrolling stops at both ends. At the end of a running input it
follows the text as it streams, like a terminal.

On the latest input of a session that takes new ones, the composer sits below
the reader, joined onto it, so the next input can be written having read the
reply. Opened from the composer (`Enter` on an empty draft) it keeps the
keyboard: keys type, ↑/↓ (while empty) and PageUp/PageDown scroll, `Enter`
sends and returns to the columns with the new input selected, and `Esc` returns
with the draft intact. Opened from the list it shows `i to type`, and the
command keys above apply until `i` moves into it.

The Markdown is rendered by a small renderer in `because-tui` (headings, lists,
quotes, fenced code, rules, `**bold**`, `*italic*`, `` `code` ``), not
`tui-markdown`. tui-markdown 0.3.10 is compatible (it depends on
`ratatui-core` 0.1, as ratatui 0.30 does; `default-features = false` drops
syntect), but it returns unwrapped text, and ratatui's own wrapping starts a
list item's continuation lines at the margin.

**Optional peek split** (v1.1, `z` to toggle): at ≥ 160 columns, compress the
three columns into the left 40% and put the code pane in the right 60%. Useful
for scanning a list of changes without a full view switch each time. Worth
having, not worth blocking v1 on.

## 6. Code pane

A window into the actual file **as of the selected input**, with that input's
lines marked and lines from earlier inputs of the same session labelled by input
number — not a diff view. The diff is one keystroke away (`u`), but the default
is the code as it really reads.

Rendered by the skeleton (`cuz render 104x24 "alt-up,alt-3,alt-right"`) — input 3
of the four-input session, viewing `pathutil.rs`:

```
input 3/4 › pathutil still has leftovers from before the refactor — tidy it up
The is_hidden pattern applied to the rest of the module: each pair of cfg-gated top-level … [generated]
line 86 ← input 3 · make the dot check platform-independent · collapse strip_prefix, is_file_name and f…
j/k move · W why · n/N this input · ]/[ any input · Esc back
┌ 4 crates/ignore/src/pathutil.rs ── as of input 3 · c55818f ──────────────────────────────────────────┐
│ 2┊   82 │ /// the path itself.                                                                       │
│ 2┊   83 │ ///                                                                                        │
│ 2┊   84 │ /// This has the same behavior on all platforms.                                           │
│ 2┊   85 │ fn is_hidden_path_only(path: &Path) -> bool {                                              │
│    ⌄ −37 ─ #[cfg(unix)]                                                                              │
│› ▌   86 │     if let Some(name) = file_name(path) {                                                  │
│  ▌   87 │         name.as_encoded_bytes().starts_with(b".")                                          │
│      88 │     } else {                                                                               │
│    ⌄ −1 ─ Some(&Path::new(OsStr::from_bytes(&path[prefix.l                                           │
│  ▌   89 │         false                                                                              │
│      90 │     }                                                                                      │
│      91 │ }                                                                                          │
│      92 │                                                                                            │
│      93 │ /// Strip `prefix` from the `path` and return the remainder.                               │
│      94 │ ///                                                                                        │
│      95 │ /// If `path` doesn't have a prefix `prefix`, then return `None`.                          │
```

### Which revision's content

Line numbers only mean something relative to a specific revision, so the pane is
explicit about which one it is showing:

| Source | When | Status line |
|---|---|---|
| post-image of the selected input's commit (`git show <sha>:<path>`) | default for committed changes — this is the state the summary describes | `as of input 3 · c55818f` |
| working tree | uncommitted changes; also `w` to toggle | `working tree` |
| pre-image | `p` to toggle — what the file looked like before | `da7b190^ (pre-image)` |
| `blob:<digest>` | the commit was amended or rebased away | `orphaned blob` |

If the file has changed since the commit, the default is still the post-image,
because that is what the summary was written about. `w` says so in the status
line when the working tree differs.

### Highlight tiers

Four visual levels, because "highlighted" has to answer *which input wrote this*:

| Tier | Rendering | Meaning |
|---|---|---|
| this input | full-width wash + ` ▌` gutter | written by the selected input |
| earlier input | faint wash + `N┊`, where N is the input number | written by input N of the same session and still present in this version |
| deleted | collapsed row `⌄ −37` between the surrounding lines | removed by the selected input; `Space` expands them inline |
| untouched | normal | older than the session |

**The earlier-input tier has to come from blame, not from hunks.** Each input's
hunks are numbered against *its own* version of the file. By input 3, lines
input 1 wrote have moved, and some are gone — reusing input 1's hunk line
numbers against input 3's version would mark the wrong lines. So each file
change carries `line_origins` (post-image line → originating commit and that
commit's line number), computed with `git blame` at the selected input's commit
and filtered to the session's commits. In the fixture: input 1 wrote 45 lines of
`pathutil.rs`; 33 survive input 2 and 25 survive input 3. This is the anchoring
mechanism from `07-provenance.md` §3 — needed at M1, not later, because any
conversation longer than one input produces it.

The deleted-lines case is the one that is easy to get wrong. Lines that were
removed cannot be highlighted in a post-image, because they are not there — so
they get a synthetic collapsed marker that expands on demand. The alternative is
`u`: render `git diff -U<large>` instead, a full-context diff where deletions
appear naturally at the cost of two ambiguous line-number columns. Both are
cheap; post-image is the default because it reads like real code, and the
full-context diff is the toggle for anyone who wants the deletions inline
permanently.

### Entry and in-pane navigation

On entry, scroll so the first highlighted region sits about a third from the top;
if a specific hunk was selected in the Changes column, center that hunk instead.

| Key | Action |
|---|---|
| `j` / `k` | move the cursor line; the view keeps it about a third of the way down |
| `n` / `N` | next / previous region written **by this input** (deletions included) |
| `]` / `[` | next / previous region written by **any input** in the session |
| `j` `k` `Ctrl+d` `Ctrl+u` `g` `G` | scroll |
| `Space` | expand / collapse the deletion marker under the cursor |
| `u` | toggle full-context diff rendering |
| `w` / `p` | working-tree / pre-image revision |
| `s` | side-by-side diff for the current hunk |
| `W` | why is this line here — rationale panel (`07-provenance.md` §7) |
| `o` | open in `$EDITOR` at the cursor line |
| `y` / `Y` | yank visible selection / `path:line` |
| `Esc` / `Alt+←` | back to the Changes column |

`o` is only meaningful on the working-tree revision — line numbers from a
historical post-image do not map to the file on disk. When the user presses it
on a historical revision, say that and offer to switch to the working tree
rather than opening the editor at a misleading line.

### Sticky header

The header is pinned and carries four lines: the selected input as typed
(`input 3/4 › …`), the file's generated summary, what the cursor line resolves
to (`line 86 ← input 3 · <section>`), and the key hints. The third line updates
as the cursor moves, so walking down the file reads off which input and which
section wrote each region without opening the rationale panel. This is not decoration:
the entire premise of the tool is that the prose explains the code, so the prose
has to stay on screen while the code is being read. Losing it at the moment of
maximum need would undo the point.

Below it, a breadcrumb of the enclosing symbol. At M1 this comes free from git's
own `@@ … @@` hunk headers (git computes function context per hunk via its
built-in language drivers and `diff.*.xfuncname`), which covers the highlighted
regions. It does *not* cover arbitrary scroll positions between hunks — for that
the breadcrumb needs a real parse, so tree-sitter goes in later, and only if the
breadcrumb turns out to earn it.

### Rationale panel

`W` resolves **the line under the cursor**, not the selected input — the line may
have been written by an earlier one. Viewing input 3, with the cursor on a line
input 2 wrote:

```
input 3/4 › pathutil still has leftovers from before the refactor — tidy it up
The is_hidden pattern applied to the rest of the module: each pair of cfg-gated top-level … [generated]
line 85 ← input 2 · share the leading-dot check
j/k move · W why · n/N this input · ]/[ any input · Esc back
┌ … code pane, cursor on line 85 …
┌ why is line 85 here? ────────────────────────────────────────────────────────────────────────────────┐
│input      › now add a variant that takes a plain Path. incremental checking won't have a DirEntry to…│
│summary      New is_hidden_path for callers without a DirEntry — on Windows it costs a                │
│             metadata() call per check. The leading-dot test is extracted into                        │
│             is_hidden_path_only and shared with is_hidden_entry. [generated]                         │
│section      share the leading-dot check                                                              │
│  ❝ said     Both functions need the same leading-dot test. Pulling it out as is_hidden_path_only and │
│             having is_hidden_entry call it, so there is one definition.                              │
│    ⟩ ran    edit_file crates/ignore/src/pathutil.rs                                                  │
└ via 6e527e9 · input 2 of 4 (an earlier input) · its line 85 ─────────────────────────────────────────┘
```

The resolution path is the one in `07-provenance.md` §3: `line_origins` gives the
originating commit and that commit's line number; the session maps the commit to
an input; that input's hunk containing the line maps to the sections that cite
it; the sections carry the narration.

When several sections cite one hunk, the panel shows all of them and says so.
It happens for real in the fixture: `c55818f` is a single hunk at `-U3` holding
three sections' work, so any line it wrote resolves to all three. A line older
than the session resolves to nothing, and the panel says that rather than
guessing.

Rendering rules:

- **Three provenance classes never look alike.** What the developer typed
  (`›`, warm colour), what a model generated (accent colour, `[generated]`), and
  the model's own words (`❝`, normal foreground).
- **The panel names the commit it resolved through** and whether that is this
  input or an earlier one, because blame can resolve to a move or a reformat
  rather than to the logic that put the line there.
- Blame is too slow to run inline (`07-provenance.md` §3 has the numbers); the
  panel opens immediately and fills from an `AppEvent`. The fixture carries
  precomputed origins, so the skeleton does not show this latency.

## 7. Diff and highlight rendering

- `imara-diff` for hunks (Histogram algorithm), fast enough to diff on demand
  rather than caching rendered output.
- `syntect` + `two-face` for highlighting, with one non-obvious constraint:
  **syntect's parse state is sequential**, so correct colours at line 18,000
  require parsing the 18,000 lines before it. Jumping to a hunk deep in a large
  file would stall. Mitigation: snapshot `ParseState` + `HighlightState` every
  512 lines into a `Vec` when a file is first opened, so any seek parses at most
  512 lines. Cache keyed by `(blob_digest, syntax, theme)`.
- Render only the visible window. Keep `first_visible_line` in the model and
  never build a `Text` for a whole file.
- Limits: over 100k lines or 5 MiB, drop syntax highlighting and render plain
  text — the gutter markers and summary still work, which is most of the value.
  Binary files show stats only.
- Optional intra-line word diff via `similar` for changed line pairs.

## 8. Performance targets

Testable, not aspirational:

- open the TUI on a repo with 10k commits in < 300 ms (index is pre-built);
- key-to-paint latency < 16 ms for navigation, including a column → code pane
  view switch;
- entering the code pane on a 20k-line file paints in < 100 ms (this is what the
  snapshot cache above is for);
- a 5,000-line patch scrolls without a visible hitch;
- `W` paints its panel in < 16 ms and fills the narration within 500 ms on a
  file with 10k commits of history (prefetch on pane entry is what makes this
  achievable; see `07-provenance.md` §3);
- steady-state CPU ~0% when idle with no agent segment running.

### 5d. Text sent while the agent works *(built)*

`Enter` while the agent of the session works writes the text to the stdin of
the agent immediately (`Cmd::SendNow`). The CLI reads it at its next step. The
CLI runs with `--replay-user-messages`, so it echoes each input when it reads
it (`AgentEvent::InputTaken`). The echo tells `cuz` where the text went:

| Echo arrives | Meaning | `cuz` |
|---|---|---|
| before `Finished` | the text joined the running input, at a tool boundary | adds the text to that input, under "Added while the agent worked:" |
| after `Finished` | the agent read the text after its last step, so the text starts a response of its own | opens a new running input for it, with no second send |

**Send now** (`Ctrl+Enter`, or `Ctrl+X` `Ctrl+S`) is Claude Code's key of the
same name. `cuz` sends the draft as above, then sends the `interrupt` request
(`Cmd::Stop`). The CLI ends the running input, then reads the text as a turn of
its own, so the second row of the table applies. The interrupted input is
`Done`, and it shows "Interrupted to read the input that you sent". Claude Code
moves running tools to the background. The CLI has no request for this, so in
`cuz` the tools stop. Only a terminal with keyboard enhancement reports
`Ctrl+Enter`. `Ctrl+X` `Ctrl+S` works in all terminals. After `Ctrl+X`, any
other key does its usual action. `Ctrl+X` shadows the cut key of the editor.

Text that the agent did not read before it exits is lost with the process.
`Ctrl+C` does not take back text that went out. If the agent did not read it
before the stop, it becomes the next input.

