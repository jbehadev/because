//! `cuz` — walking skeleton.
//!
//! Modes:
//!   cuz [live options]
//!                    a live session (see `cuz live` below); the default
//!   cuz demo          run the TUI against the sample fixture, with no agent
//!                    (needs a terminal)
//!   cuz render <w>x<h> [script]
//!                    replay a key script and print the resulting frame, so the
//!                    UI can be reviewed in CI or a transcript with no TTY
//!   cuz keys          key probe: shows how each press arrives from this terminal
//!                    and what it does in each mode
//!   cuz live [--permission-mode MODE] [--model MODEL] [--claude PATH]
//!           [--summary-model MODEL | --no-summary] [--no-import]
//!                    a live session in the current git repository, with the
//!                    Claude Code CLI as the agent. Finished inputs are
//!                    summarized by a small model (default haiku, about a fifth
//!                    of a cent each) through the same CLI. Every input is kept
//!                    in the store, and earlier sessions — this tool's and
//!                    Claude Code's own — are listed as history. Plain `cuz`
//!                    with the same options does the same.
//!   cuz live --check  run the whole live chain once outside the TUI and report
//!                    each step (sends one tiny input and one summary call,
//!                    about two cents)
//!   cuz import-claude read this repository's Claude Code transcripts into the
//!                    store and report what was found (`cuz live` does the same
//!                    at start, quietly)
//!
//! A script is comma-separated steps. Keys: `tab`, `shift-tab`, `enter`, `esc`,
//! `up`, `down`, `left`, `right`, `space`, `pgup`, `pgdn`, `home`, `end`,
//! `ctrl-c`, `ctrl-j`, `ctrl-left`, and any of
//! those or a single character prefixed with `alt-` (e.g. `alt-2`, `alt-right`,
//! `alt-enter`). A bare single character is that key. `text:hello world` types
//! each character; `paste:a\nb` delivers a bracketed paste. Text cannot contain
//! commas.

use std::io::stdout;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use because_agent::summary::Summarizer;
use because_agent::{Agent, AgentSession, SessionConfig};
use because_core::Fixture;
use because_tui::{Model, Msg, dispatch, update, view};

mod import;

const FIXTURE: &str = "fixtures/ripgrep.json";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fixture = || Fixture::load(FIXTURE).with_context(|| format!("loading {FIXTURE}"));
    match args.first().map(String::as_str) {
        // A live session is the default: bare `cuz`, or `cuz` with live options.
        None => live(&[]),
        Some(a) if a.starts_with("--") => live(&args),
        Some("live") => live(&args[1..]),
        Some("import-claude") => import::command(),
        Some("demo") => run(fixture()?),
        Some("render") => render(fixture()?, &args[1..]),
        Some("keys") => keys(fixture()?),
        Some(other) => bail!("unknown command `{other}`: expected live, demo, render, keys or import-claude"),
    }
}

fn parse_key(name: &str) -> Result<KeyEvent> {
    let (mods, base) = if let Some(rest) = name.strip_prefix("alt-") {
        (KeyModifiers::ALT, rest)
    } else if let Some(rest) = name.strip_prefix("ctrl-") {
        (KeyModifiers::CONTROL, rest)
    } else if let Some(rest) = name.strip_prefix("shift-") {
        (KeyModifiers::SHIFT, rest)
    } else {
        (KeyModifiers::NONE, name)
    };
    let code = match base {
        "tab" if mods == KeyModifiers::SHIFT => return Ok(KeyEvent::new(KeyCode::BackTab, mods)),
        "tab" => KeyCode::Tab,
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "space" => KeyCode::Char(' '),
        "pgup" => KeyCode::PageUp,
        "pgdn" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        s if s.chars().count() == 1 => KeyCode::Char(s.chars().next().unwrap()),
        other => bail!("unknown key in script: {other}"),
    };
    Ok(KeyEvent::new(code, mods))
}

fn apply_script(m: &mut Model, script: &str) -> Result<()> {
    for step in script.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(text) = step.strip_prefix("text:") {
            for c in text.chars() {
                if let Some(msg) = dispatch(m, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) {
                    update(m, msg);
                }
            }
        } else if let Some(text) = step.strip_prefix("paste:") {
            update(m, Msg::Paste(text.replace("\\n", "\n")));
        } else {
            let k = parse_key(step)?;
            if let Some(msg) = dispatch(m, k) {
                update(m, msg);
            }
        }
    }
    Ok(())
}

fn render(fx: Fixture, args: &[String]) -> Result<()> {
    let (w, h) = args
        .first()
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((150u16, 44u16));
    let mut m = Model::new(fx);
    if let Some(script) = args.get(1) {
        apply_script(&mut m, script)?;
    }
    let mut term = Terminal::new(TestBackend::new(w, h))?;
    term.draw(|f| view(&m, f))?;
    // TestBackend's Display quotes every row; dump the cells directly instead.
    let buf = term.backend().buffer();
    let (w, h) = (buf.area.width as usize, buf.area.height as usize);
    for y in 0..h {
        let row: String = (0..w).map(|x| buf.content[y * w + x].symbol()).collect();
        println!("{}", row.trim_end());
    }
    Ok(())
}

fn run(fx: Fixture) -> Result<()> {
    enable_raw_mode()?;
    // Without bracketed paste, a pasted newline is indistinguishable from Enter
    // and a multi-line paste would submit on its first line.
    execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    // Where the terminal supports it, this makes Shift+Enter distinguishable
    // from Enter and modifiers unambiguous. Elsewhere Alt+Enter still works.
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    let result = event_loop(fx);
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableBracketedPaste, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    result
}

fn event_loop(fx: Fixture) -> Result<()> {
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut m = Model::new(fx);
    while !m.quit {
        term.draw(|f| view(&m, f))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let msg = match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => dispatch(&m, k),
            Event::Paste(text) => Some(Msg::Paste(text)),
            _ => None,
        };
        if let Some(msg) = msg {
            update(&mut m, msg);
        }
    }
    Ok(())
}

/// `cuz keys`: print how each key press arrives and what it does in each mode.
///
/// Runs under the same terminal settings as the TUI — raw mode, and keyboard
/// enhancement where the terminal supports it — so what it reports is exactly
/// what the TUI receives. This is the answer to "the shortcut does nothing on
/// my machine": the terminal, not the app, decides what a key sends.
fn keys(fx: Fixture) -> Result<()> {
    enable_raw_mode()?;
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    let result = probe(fx, enhanced);
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    result
}

fn probe(fx: Fixture, enhanced: bool) -> Result<()> {
    use std::io::Write;
    // The three contexts that decide what a key means.
    let empty = Model::new(fx.clone());
    let mut typing = Model::new(fx.clone());
    typing.composer.insert_str("x");
    let mut other = Model::new(fx);
    update(&mut other, Msg::FocusAt(2));

    let mut out = stdout();
    write!(out, "ts keys: press keys to see how this terminal sends them. Ctrl+C twice quits.\r\n")?;
    write!(
        out,
        "keyboard enhancement: {}\r\n\r\n",
        if enhanced { "on" } else { "not supported by this terminal (Shift+Enter looks like Enter)" }
    )?;
    write!(out, "{:<22} {:<18} {:<18} other panes\r\n", "arrives as", "empty composer", "while typing")?;
    let mut prev_ctrl_c = false;
    loop {
        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                let act = |m: &Model| action(dispatch(m, k));
                write!(out, "{:<22} {:<18} {:<18} {}\r\n", describe(k), act(&empty), act(&typing), act(&other))?;
                if let Some(note) = note(k) {
                    write!(out, "{:<22} ↳ {note}\r\n", "")?;
                }
                let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl_c && prev_ctrl_c {
                    break;
                }
                prev_ctrl_c = ctrl_c;
            }
            Event::Paste(text) => write!(out, "bracketed paste, {} chars\r\n", text.chars().count())?,
            _ => {}
        }
        out.flush()?;
    }
    Ok(())
}

fn describe(k: KeyEvent) -> String {
    let mut s = String::new();
    for (m, name) in [(KeyModifiers::CONTROL, "Ctrl+"), (KeyModifiers::ALT, "Alt+"), (KeyModifiers::SHIFT, "Shift+")] {
        if k.modifiers.contains(m) {
            s.push_str(name);
        }
    }
    match k.code {
        KeyCode::Char(' ') => s.push_str("Space"),
        KeyCode::Char(c) => s.push(c),
        other => s.push_str(&format!("{other:?}")),
    }
    s
}

fn action(msg: Option<Msg>) -> String {
    match msg {
        None => "(nothing)".into(),
        Some(Msg::Edit(_)) => "edit text".into(),
        Some(Msg::FocusLeft) => "pane ←".into(),
        Some(Msg::FocusRight) => "pane →".into(),
        Some(Msg::FocusNext) => "next pane".into(),
        Some(Msg::FocusPrev) => "previous pane".into(),
        Some(Msg::FocusAt(i)) => format!("pane {}", i + 1),
        Some(m) => format!("{m:?}"),
    }
}

/// Explain the encodings that trip people up, macOS first.
fn note(k: KeyEvent) -> Option<&'static str> {
    let alt = k.modifiers == KeyModifiers::ALT;
    match k.code {
        KeyCode::Char('b') if alt => Some("macOS Terminal and iTerm2 send Option+← like this (ESC b)"),
        KeyCode::Char('f') if alt => Some("macOS Terminal and iTerm2 send Option+→ like this (ESC f)"),
        // Option+v, t, w, 1-4 on a US layout when Option composes characters
        KeyCode::Char('√' | '†' | '∑' | '¡' | '™' | '£' | '¢') => Some(
            "Option is typing symbols instead of sending Alt. Terminal: Settings › Profiles › Keyboard › \
             Use Option as Meta key. iTerm2: Profiles › Keys › Left Option key › Esc+. \
             Ghostty: macos-option-as-alt = true",
        ),
        KeyCode::Esc => Some("if you pressed an Alt/Option+arrow, this terminal sends ESC ESC [..., which is read as Esc — use Tab"),
        _ => None,
    }
}

// ── live sessions ─────────────────────────────────────────────────────────

fn live(args: &[String]) -> Result<()> {
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let cwd = std::env::current_dir()?;
    let repo = because_git::Repo::discover(&cwd).context("`cuz` needs to run inside a git repository")?;
    // The only backend so far. Another agent is one more `Agent` here.
    let claude = because_agent::claude::Claude::find(opt("--claude"))?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::create_dir_all(repo.data_dir());
    let log = repo.data_dir().join(format!("{session_id}.log"));
    let fx = Fixture {
        sessions: vec![because_core::Session {
            id: session_id.clone(),
            // Unnamed: the first column names it after the first input typed.
            title: String::new(),
            repo: repo.root.display().to_string(),
            branch: repo.branch(),
            segments: Vec::new(),
            ..Default::default()
        }],
    };
    let mut m = Model::new(fx);
    m.live = true;
    m.log_path = Some(log.display().to_string());
    m.summarize = !args.iter().any(|a| a == "--no-summary");
    let summary_model = opt("--summary-model").unwrap_or_else(|| "haiku".into());
    let summarizer: Arc<dyn Summarizer> = Arc::new(because_agent::claude::summary::ClaudeSummarizer {
        program: claude.program.clone(),
        model: summary_model.clone(),
    });
    let agent: Arc<dyn Agent> = Arc::new(claude);
    let version = agent.version().to_string();
    let cfg = SessionConfig {
        cwd: repo.root.clone(),
        session_id: session_id.clone(),
        permission_mode: opt("--permission-mode").unwrap_or_else(|| "acceptEdits".into()),
        model: opt("--model"),
        resume: false,
        log: Some(log.clone()),
        ephemeral: false,
    };
    if args.iter().any(|a| a == "--check") {
        let summary = m.summarize.then_some((summary_model.as_str(), summarizer.as_ref()));
        return check(&repo, agent.as_ref(), SessionConfig { ephemeral: true, ..cfg }, summary);
    }

    // History: earlier sessions from the store, after reading any Claude Code
    // transcripts for this repository that are new or have grown since.
    let store_path = repo.store_path();
    let mut store = match because_store::Store::open(&store_path) {
        Ok(s) => Some(s),
        Err(e) => {
            m.notice = Some(format!("No history this run — the store would not open: {e}"));
            None
        }
    };
    if let Some(st) = store.as_mut() {
        if !args.iter().any(|a| a == "--no-import") {
            let report = import::run(&repo, st, |line| println!("{line}"));
            if let Some(n) = report.notice() {
                m.notice = Some(n);
            }
        }
        m.persist = true;
        match st.load_sessions() {
            Ok(history) => m.fx.sessions.extend(history),
            Err(e) => m.notice = Some(format!("Could not read history from the store: {e}")),
        }
    }

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    let result = live_loop(m, repo.clone(), agent.clone(), cfg, summarizer, store);
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableBracketedPaste, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    println!("{version}, first session {session_id}");
    println!("  Everything {0} printed is in {1} — one `# {0} …` line per session.", agent.name(), log.display());
    println!("  Claude Code keeps a transcript for each session under ~/.claude/projects/.");
    println!("  Snapshots of the working tree are under refs/because/<session>/.");
    if let Ok(trees) = &result
        && !trees.is_empty()
    {
        println!("  Sessions after the first worked in trees of their own, left as they are:");
        for t in trees {
            println!("    {t}");
        }
        println!("  Each is on a branch of its own; `git worktree remove <path>` when you are done with it.");
    }
    println!("  Inputs, responses and summaries are kept in {}.", store_path.display());
    // Last, so it is what is on screen: work that is only in a working tree is
    // lost with it.
    let dirty: Vec<(String, usize)> = repo.uncommitted().unwrap_or_default().into_iter().filter(|(_, n)| *n > 0).collect();
    if !dirty.is_empty() {
        println!("  Not committed yet:");
        for (dir, n) in dirty {
            println!("    {dir}  ({n} file{})", if n == 1 { "" } else { "s" });
        }
    }
    result.map(|_| ())
}

/// One session's half of the runtime. A session is one conversation, so each
/// needs its own agent process, its own working tree and its own snapshots.
/// They run at the same time: nothing here is shared between two of them.
struct Live {
    /// The tree this session's agent works in. The first session has the
    /// repository you started in; every later one has a worktree of its own,
    /// because two agents editing one checkout would make the before/after
    /// snapshots — the account of who changed what — meaningless.
    repo: because_git::Repo,
    cfg: SessionConfig,
    agent: Option<Box<dyn AgentSession>>,
    started_once: bool,
    /// The snapshot taken before each input, by segment index.
    before: std::collections::HashMap<usize, String>,
    /// Short shas of this session's "after" snapshots: the commits whose lines
    /// count as written by one of its inputs.
    keep: std::collections::HashSet<String>,
    /// Parent for this session's next snapshot, so its snapshots form one
    /// chain. Per session, since the sessions advance independently.
    last: Option<String>,
}

impl Live {
    fn new(repo: because_git::Repo, cfg: SessionConfig) -> Self {
        Self { repo, cfg, agent: None, started_once: false, before: Default::default(), keep: Default::default(), last: None }
    }

    /// Snapshot the tree before an input, so that whatever the agent does is
    /// measured against the tree as it was when the input started.
    fn snapshot_before(&mut self, seg: usize) -> anyhow::Result<()> {
        let r = format!("refs/because/{}/{}-before", self.cfg.session_id, seg + 1);
        let sha = self.repo.snapshot(self.last.as_deref(), &format!("before input {}", seg + 1), &r)?;
        self.last = Some(sha.clone());
        self.before.insert(seg, sha);
        Ok(())
    }
}

/// Open a session that has not run before: a tree of its own, branched from
/// what the repository looks like now, and a conversation of its own.
///
/// The base is a snapshot rather than HEAD so that work in progress — edits
/// that were never committed — comes with it. Ignored files do not: a snapshot
/// leaves them out, so build output is not copied and the session's first build
/// starts cold.
fn open_session(main: &because_git::Repo, n: usize, cfg: &SessionConfig) -> Result<(Live, String)> {
    let id = uuid::Uuid::new_v4().to_string();
    let tag = format!("{}-{}", n + 1, &id[..6]);
    let base = main.snapshot(None, &format!("session {} starts here", n + 1), &format!("refs/because/{id}/base"))?;
    let repo = main
        .worktree(&tag, &base, &format!("because/session-{tag}"))
        .with_context(|| format!("could not make a working tree for session {}", n + 1))?;
    let root = repo.root.display().to_string();
    let mut o = cfg.clone();
    o.session_id = id;
    o.cwd = repo.root.clone();
    Ok((Live::new(repo, o), root))
}

/// How long a resumed agent has to stay up before its session counts as
/// revived. One that cannot resume the conversation says so and exits on
/// start, before any input is sent.
const REVIVE_GRACE: Duration = Duration::from_secs(4);

/// An earlier session whose resumed agent is running but has not yet
/// lasted `REVIVE_GRACE`. It is not in `live` until then: its place among the
/// sessions is only known once it is moved there.
struct Probe {
    live: Live,
    /// Where the process's events go: `None` while on probation (they arrive
    /// as `In::Probe`), the session's index once revived.
    route: std::sync::Arc<std::sync::Mutex<Option<usize>>>,
    since: std::time::Instant,
}

/// Start resuming an earlier session, after checking what a resume needs and
/// would otherwise fail on less clearly: the tree it ran in, and what the agent
/// itself needs (for Claude Code, its transcript of the session).
fn revive(
    id: &str,
    dir: &str,
    agent: &dyn Agent,
    cfg: &SessionConfig,
    store: Option<&because_store::Store>,
    tx: &std::sync::mpsc::Sender<In>,
) -> Result<Probe> {
    use std::sync::{Arc, Mutex};
    let path = std::path::Path::new(dir);
    if !path.is_dir() {
        bail!("the tree it ran in, {dir}, is gone");
    }
    let repo = because_git::Repo::discover(path).with_context(|| format!("{dir} is no longer a git repository"))?;
    let mut o = cfg.clone();
    o.session_id = id.to_string();
    o.cwd = path.to_path_buf();
    o.resume = true;
    agent.check_resume(&o)?;
    let route: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let (route2, tx2, id2) = (route.clone(), tx.clone(), id.to_string());
    let session = agent.start(
        &o,
        Box::new(move |ev| {
            let _ = match *route2.lock().unwrap() {
                Some(i) => tx2.send(In::Agent(i, ev)),
                None => tx2.send(In::Probe(id2.clone(), ev)),
            };
        }),
    )?;
    let mut live = Live::new(repo, o);
    live.agent = Some(session);
    live.started_once = true;
    // Lines its earlier inputs wrote still count as this session's.
    if let Some(st) = store {
        live.keep = st.session_commits(id).unwrap_or_default().into_iter().map(|(sha, _)| sha).collect();
    }
    Ok(Probe { live, route, since: std::time::Instant::now() })
}

/// What arrives from a worker thread. Agent events are tagged with the session
/// whose process produced them: several sessions are served at once, and an
/// event only means something against the conversation it came from.
enum In {
    Agent(usize, because_core::AgentEvent),
    /// From an earlier session's resumed process that is still on probation,
    /// by session id.
    Probe(String, because_core::AgentEvent),
    Msg(Msg),
}

/// Returns the worktrees it made, so the caller can say where they are.
fn live_loop(
    mut m: Model,
    repo: because_git::Repo,
    agent: Arc<dyn Agent>,
    cfg: SessionConfig,
    summarizer: Arc<dyn Summarizer>,
    mut store: Option<because_store::Store>,
) -> Result<Vec<String>> {
    use std::collections::{BTreeMap, HashSet};
    use std::sync::mpsc;
    use because_core::{AgentEvent, Commit};
    use because_tui::Cmd;

    let (tx, rx) = mpsc::channel::<In>();
    // Created on a session's first input, so a session you never type into
    // costs no process, no conversation and no working tree. Session 0 already
    // has its id, and works in the repository you started `cuz live` in.
    let mut live: BTreeMap<usize, Live> = BTreeMap::new();
    live.insert(0, Live::new(repo.clone(), cfg.clone()));
    // The worktrees made for later sessions, printed on the way out so the work
    // in them is not lost behind a path nobody was told.
    let mut worktrees: Vec<String> = Vec::new();
    // Earlier inputs whose code has been asked of git, so each is asked once.
    let mut code_asked: HashSet<(String, usize)> = HashSet::new();
    // The process group of the shell command (`!`) that runs in each session,
    // so that Ctrl+C can stop it.
    let mut shells: BTreeMap<usize, u32> = BTreeMap::new();
    // Earlier sessions being revived, by id, until they have lasted the grace.
    let mut probes: std::collections::HashMap<String, Probe> = Default::default();
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    // The import may have printed before the alternate screen opened; start
    // from a known-blank one rather than trusting the terminal to.
    term.clear()?;
    poll_usage(tx.clone(), agent.clone(), cfg.log.clone());
    poll_uncommitted(tx.clone(), repo.clone());

    while !m.quit {
        term.draw(|f| view(&m, f))?;
        let wait = if m.running.is_empty() { 250 } else { 50 };
        if event::poll(Duration::from_millis(wait))? {
            let msg = match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => dispatch(&m, k),
                Event::Paste(text) => Some(Msg::Paste(text)),
                _ => None,
            };
            if let Some(msg) = msg {
                update(&mut m, msg);
            }
        }
        while let Ok(inc) = rx.try_recv() {
            // A probe's event that was on its way when it was revived belongs
            // to the session it became.
            let inc = match inc {
                In::Probe(id, ev) if !probes.contains_key(&id) => {
                    match m.fx.sessions.iter().position(|s| s.id == id && !s.origin.is_history()) {
                        Some(i) => In::Agent(i, ev),
                        None => continue,
                    }
                }
                other => other,
            };
            match inc {
                In::Msg(msg) => {
                    if let Msg::ShellDone { session, .. } = msg {
                        shells.remove(&session);
                    }
                    update(&mut m, msg)
                }
                In::Probe(id, AgentEvent::Exited { detail }) => {
                    probes.remove(&id);
                    update(&mut m, Msg::ReviveFailed { session_id: id, error: detail });
                }
                In::Probe(..) => {}
                In::Agent(s, ev) => {
                    if matches!(ev, AgentEvent::Exited { .. })
                        && let Some(l) = live.get_mut(&s)
                    {
                        l.agent = None; // its next input starts it again, resuming the conversation
                    }
                    // Tagged with the session it came from, so an event from a
                    // session you are not looking at lands on its own input
                    // rather than on whatever is on screen.
                    update(&mut m, Msg::Agent(s, ev));
                }
            }
        }
        update(&mut m, Msg::Tick(std::time::Instant::now()));

        // A resumed agent that is still up after the grace has the
        // conversation: its session moves up among this run's and takes inputs.
        let ready: Vec<String> = probes.iter().filter(|(_, p)| p.since.elapsed() >= REVIVE_GRACE).map(|(id, _)| id.clone()).collect();
        for id in ready {
            let p = probes.remove(&id).expect("listed above");
            update(&mut m, Msg::Revived { session_id: id.clone() });
            match m.fx.sessions.iter().position(|s| s.id == id && !s.origin.is_history()) {
                Some(i) => {
                    *p.route.lock().unwrap() = Some(i);
                    live.insert(i, p.live);
                }
                // Gone from the screen while it started: nothing to serve.
                None => drop(p),
            }
        }

        for cmd in std::mem::take(&mut m.outbox) {
            match cmd {
                Cmd::Send { session, seg, text } => {
                    // The wall clock lives here, not in the model: `update`
                    // only ever sees an Instant.
                    let now = because_core::time::now_rfc3339();
                    let s = &mut m.fx.sessions[session];
                    if s.started_at.is_empty() {
                        s.started_at = now.clone();
                    }
                    if let Some(x) = s.segments.get_mut(seg) {
                        x.started_at = now;
                    }
                    // A session the agent has not served yet gets a working
                    // tree and a conversation of its own, under an id of its
                    // own, in the same log. This is the one slow step in the
                    // loop — a checkout — and it happens once per session.
                    if let Err(e) = open_live(&mut live, &mut m, &repo, session, &cfg, &mut worktrees) {
                        update(&mut m, Msg::Agent(session, AgentEvent::Exited { detail: e.to_string() }));
                        continue;
                    }
                    let l = live.get_mut(&session).expect("opened above");
                    if let Err(e) = l.snapshot_before(seg) {
                        m.notice = Some(format!("Snapshot failed, changes will not be shown: {e}"));
                    }
                    if l.agent.is_none() {
                        l.cfg.resume = l.started_once;
                        let tx2 = tx.clone();
                        match agent.start(
                            &l.cfg,
                            Box::new(move |ev| {
                                let _ = tx2.send(In::Agent(session, ev));
                            }),
                        ) {
                            Ok(a) => {
                                l.agent = Some(a);
                                l.started_once = true;
                            }
                            Err(e) => {
                                update(&mut m, Msg::Agent(session, AgentEvent::Exited { detail: e.to_string() }));
                                continue;
                            }
                        }
                    }
                    if let Some(Err(e)) = l.agent.as_mut().map(|a| a.send(&text)) {
                        l.agent = None;
                        update(
                            &mut m,
                            Msg::Agent(session, AgentEvent::Exited { detail: format!("could not send to {}: {e}", agent.name()) }),
                        );
                    }
                }
                Cmd::Begin { session, seg } => {
                    let now = because_core::time::now_rfc3339();
                    if let Some(x) = m.fx.sessions[session].segments.get_mut(seg) {
                        x.started_at = now;
                    }
                    // The first step of the response makes this command, so
                    // the snapshot comes before the tools of the response.
                    // A very fast first tool can be earlier than the snapshot.
                    if let Some(l) = live.get_mut(&session)
                        && let Err(e) = l.snapshot_before(seg)
                    {
                        m.notice = Some(format!("Snapshot failed, changes will not be shown: {e}"));
                    }
                }
                Cmd::Finish { session, seg } => {
                    let Some(l) = live.get_mut(&session) else { continue };
                    let Some(b) = l.before.get(&seg).cloned() else {
                        let _ = tx.send(In::Msg(Msg::ChangesFailed {
                            session,
                            seg,
                            error: "no snapshot before this input".into(),
                        }));
                        continue;
                    };
                    let r = format!("refs/because/{}/{}-after", l.cfg.session_id, seg + 1);
                    match l.repo.snapshot(Some(&b), &format!("after input {}", seg + 1), &r) {
                        Ok(a) => {
                            l.last = Some(a.clone());
                            l.keep.insert(a[..7].to_string());
                            // Only this session's snapshots attribute a line, so
                            // what an earlier session wrote reads as untouched.
                            let (repo2, keep2, tx2) = (l.repo.clone(), l.keep.clone(), tx.clone());
                            std::thread::spawn(move || {
                                let msg = match repo2.changes(&b, &a, &keep2) {
                                    Ok(changes) => Msg::Changes {
                                        session,
                                        seg,
                                        commit: Commit {
                                            short: a[..7].to_string(),
                                            sha: a,
                                            subject: format!("after input {}", seg + 1),
                                            author: "because".into(),
                                            committed_at: because_core::time::now_rfc3339(),
                                            parent: b.clone(),
                                        },
                                        changes,
                                    },
                                    Err(e) => Msg::ChangesFailed { session, seg, error: e.to_string() },
                                };
                                let _ = tx2.send(In::Msg(msg));
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(In::Msg(Msg::ChangesFailed { session, seg, error: e.to_string() }));
                        }
                    }
                }
                Cmd::Summarize { session, seg } => {
                    let s = &m.fx.sessions[session].segments[seg];
                    let tools: Vec<&str> = s.tool_calls().collect();
                    let prompt = because_agent::summary::prompt(&s.input, &tools, &s.said());
                    let (summarizer, log, tx2) = (summarizer.clone(), cfg.log.clone(), tx.clone());
                    std::thread::spawn(move || {
                        let result = summarizer.summarize(&prompt);
                        if let Some(f) = log.and_then(|p| std::fs::OpenOptions::new().append(true).open(p).ok()) {
                            use std::io::Write;
                            let line = match &result {
                                Ok(r) => format!("ok, {} · ${:.4} · {}ms", r.model, r.cost_usd, r.duration_ms),
                                Err(e) => format!("failed: {e}"),
                            };
                            let _ = writeln!(&f, "# summary of input {}: {line}", seg + 1);
                        }
                        let _ = tx2.send(In::Msg(match result {
                            Ok(r) => Msg::Summary { session, seg, text: r.text, model: r.model, cost_usd: r.cost_usd },
                            Err(e) => Msg::SummaryFailed { session, seg, error: e.to_string() },
                        }));
                    });
                }
                Cmd::Answer { session, request_id, decision } => {
                    let Some(a) = live.get_mut(&session).and_then(|l| l.agent.as_mut()) else {
                        m.notice = Some(format!("Session {}'s agent has gone; there is nothing to answer.", session + 1));
                        continue;
                    };
                    match a.answer(&request_id, decision) {
                        Ok(true) => {}
                        Ok(false) => m.notice = Some("The agent had already withdrawn that request.".into()),
                        Err(e) => m.notice = Some(format!("Could not answer the agent in session {}: {e}", session + 1)),
                    }
                }
                Cmd::SendMidTurn { session, text } => {
                    let sent = live.get_mut(&session).and_then(|l| l.agent.as_mut()).map(|a| a.send(&text));
                    if !matches!(sent, Some(Ok(()))) {
                        m.notice = Some(format!("Could not send to the agent in session {}. Its input still runs.", session + 1));
                    }
                }
                Cmd::Stop { session } if shells.contains_key(&session) => {
                    if let Err(e) = stop_shell(shells[&session]) {
                        m.notice = Some(format!("Could not stop the command in session {}: {e}", session + 1));
                    }
                }
                Cmd::Shell { session, seg, command } => {
                    let now = because_core::time::now_rfc3339();
                    let s = &mut m.fx.sessions[session];
                    if s.started_at.is_empty() {
                        s.started_at = now.clone();
                    }
                    if let Some(x) = s.segments.get_mut(seg) {
                        x.started_at = now;
                    }
                    let started = open_live(&mut live, &mut m, &repo, session, &cfg, &mut worktrees).and_then(|()| {
                        let l = live.get_mut(&session).expect("opened above");
                        if let Err(e) = l.snapshot_before(seg) {
                            m.notice = Some(format!("Snapshot failed, changes will not be shown: {e}"));
                        }
                        Ok(run_shell(&l.repo.root, &command, session, seg, tx.clone())?)
                    });
                    match started {
                        Ok(pgid) => {
                            shells.insert(session, pgid);
                        }
                        Err(e) => {
                            let error = Some(e.to_string());
                            update(&mut m, Msg::ShellDone { session, seg, output: String::new(), code: None, error, duration_ms: 0 });
                        }
                    }
                }
                Cmd::Stop { session } => {
                    let Some(a) = live.get_mut(&session).and_then(|l| l.agent.as_mut()) else {
                        m.notice = Some(format!("Session {}'s agent has gone; there is nothing to stop.", session + 1));
                        continue;
                    };
                    if let Err(e) = a.interrupt() {
                        m.notice = Some(format!("Could not stop the agent in session {}: {e}", session + 1));
                    }
                }
                Cmd::Persist { session, seg } => {
                    let Some(st) = store.as_mut() else { continue };
                    let s = &m.fx.sessions[session];
                    // History is written by the importer, from its source, and
                    // never by a run that only showed it.
                    if s.origin.is_history() || seg >= s.segments.len() {
                        continue;
                    }
                    let saved = st.save_session(s, None).and_then(|_| st.save_segment(&s.id, &s.segments[seg]));
                    if let Err(e) = saved {
                        m.notice = Some(format!("Could not save input {} to the store: {e}", seg + 1));
                    }
                }
                Cmd::Revive { session_id, repo: dir } => match revive(&session_id, &dir, agent.as_ref(), &cfg, store.as_ref(), &tx) {
                    Ok(p) => {
                        probes.insert(session_id, p);
                    }
                    Err(e) => update(&mut m, Msg::ReviveFailed { session_id, error: e.to_string() }),
                },
            }
        }

        // An earlier input's code, when it is selected: the store listed its
        // files; git has their content, between the commits the store named.
        if let (Some((s, seg)), Some(st)) = (m.wants_code(), store.as_ref())
            && code_asked.insert((s.id.clone(), seg))
        {
            let session_id = s.id.clone();
            match s.segments[seg].commit.clone() {
                None => {
                    update(&mut m, Msg::CodeFailed { session_id, seg, error: "no commit is recorded for it".into() });
                }
                Some(c) => {
                    // Blame counts a line as this session's only when one of its
                    // inputs wrote it. An input matched to several commits
                    // answers for each of them under its own.
                    let commits = st.session_commits(&session_id).unwrap_or_default();
                    let keep: HashSet<String> = commits.iter().map(|(sha, _)| sha.clone()).collect();
                    let alias: std::collections::HashMap<String, String> =
                        commits.into_iter().filter(|(sha, own)| !own.is_empty() && sha != own).collect();
                    let (repo2, tx2) = (repo.clone(), tx.clone());
                    std::thread::spawn(move || {
                        let msg = match repo2.changes(&c.parent, &c.sha, &keep) {
                            Ok(mut changes) => {
                                for ch in &mut changes {
                                    for (short, _) in ch.line_origins.values_mut() {
                                        if let Some(own) = alias.get(short) {
                                            *short = own.clone();
                                        }
                                    }
                                }
                                Msg::Code { session_id, seg, changes }
                            }
                            Err(e) => Msg::CodeFailed { session_id, seg, error: e.to_string() },
                        };
                        let _ = tx2.send(In::Msg(msg));
                    });
                }
            }
        }
    }
    // A command that still runs has nobody to show its output to.
    for pgid in shells.into_values() {
        let _ = stop_shell(pgid);
    }
    Ok(worktrees)
}

/// Give a session that has not run before a tree and a conversation of its
/// own, on its first input. A session that has them already keeps them.
fn open_live(
    live: &mut std::collections::BTreeMap<usize, Live>,
    m: &mut Model,
    repo: &because_git::Repo,
    session: usize,
    cfg: &SessionConfig,
    worktrees: &mut Vec<String>,
) -> Result<()> {
    if live.contains_key(&session) {
        return Ok(());
    }
    let (l, root) = open_session(repo, session, cfg)?;
    m.fx.sessions[session].id = l.cfg.session_id.clone();
    m.fx.sessions[session].repo = root.clone();
    m.fx.sessions[session].branch = l.repo.branch();
    m.notice = Some(format!("Session {} works in {root}", session + 1));
    worktrees.push(root);
    live.insert(session, l);
    Ok(())
}

/// Run a shell command (`!`) in a tree, with the shell of the developer, and
/// send `Msg::ShellDone` when it stops. Stdout and stderr go to one pipe, so
/// that the output keeps its order. Returns the process group of the command.
fn run_shell(
    dir: &std::path::Path,
    command: &str,
    session: usize,
    seg: usize,
    tx: std::sync::mpsc::Sender<In>,
) -> std::io::Result<u32> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
    let (mut out, w) = std::io::pipe()?;
    let started = std::time::Instant::now();
    // The command has no terminal: the TUI has it. A command that reads its
    // input gets end of file.
    let mut child = Command::new(shell)
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(w.try_clone()?)
        .stderr(w)
        // A group of its own, so that a stop also stops the processes that
        // the command starts.
        .process_group(0)
        .spawn()?;
    let pgid = child.id();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = out.read_to_end(&mut bytes);
        let status = child.wait();
        let error = match (read, &status) {
            (Err(e), _) => Some(e.to_string()),
            (_, Err(e)) => Some(e.to_string()),
            _ => None,
        };
        let _ = tx.send(In::Msg(Msg::ShellDone {
            session,
            seg,
            output: String::from_utf8_lossy(&bytes).into_owned(),
            code: status.ok().and_then(|s| s.code()),
            error,
            duration_ms: started.elapsed().as_millis() as u64,
        }));
    });
    Ok(pgid)
}

/// Stop a shell command and the processes that it started.
fn stop_shell(pgid: u32) -> std::io::Result<()> {
    let status = std::process::Command::new("kill")
        .args(["-TERM", "--", &format!("-{pgid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() { Ok(()) } else { Err(std::io::Error::other(format!("kill exited with {status}"))) }
}

/// How often the subscription's allowance is re-asked. It only moves as work is
/// done, and the answer costs a request against the account, so this is slow on
/// purpose.
const USAGE_EVERY: Duration = Duration::from_secs(120);

/// Keep the Sessions column's allowance bar up to date.
///
/// One poller for the whole screen, not one per session: the allowance belongs
/// to the account, and every session on screen spends the same one. It outlives
/// nothing — when the UI drops the receiver the next send fails and it stops.
/// An agent with no allowance to report is told to the UI once.
fn poll_usage(tx: std::sync::mpsc::Sender<In>, agent: Arc<dyn Agent>, log: Option<std::path::PathBuf>) {
    std::thread::spawn(move || {
        loop {
            let (msg, line) = match agent.allowance() {
                Some(Ok((u, raw))) => (Msg::Usage(u), format!("# usage: {}", raw.trim())),
                Some(Err(e)) => (Msg::UsageFailed(e.to_string()), format!("# usage failed: {e}")),
                None => {
                    let _ = tx.send(In::Msg(Msg::UsageFailed(format!("{} has no allowance to report", agent.name()))));
                    return;
                }
            };
            if let Some(f) = log.as_ref().and_then(|p| std::fs::OpenOptions::new().append(true).open(p).ok()) {
                use std::io::Write;
                let _ = writeln!(&f, "{line}");
            }
            if tx.send(In::Msg(msg)).is_err() {
                return;
            }
            std::thread::sleep(USAGE_EVERY);
        }
    });
}

/// How often every working tree is asked for changes not yet committed. A
/// `git status` per tree, run off the UI thread; the agents commit and edit
/// all the time, so the mark should not lag them by much.
const UNCOMMITTED_EVERY: Duration = Duration::from_secs(5);

/// Keep the Sessions column's uncommitted-work marks up to date, for every
/// tree of the repository — this run's sessions, earlier ones', and the
/// repository's own checkout. Stops when the UI drops the receiver.
fn poll_uncommitted(tx: std::sync::mpsc::Sender<In>, repo: because_git::Repo) {
    std::thread::spawn(move || {
        loop {
            if let Ok(n) = repo.uncommitted()
                && tx.send(In::Msg(Msg::Uncommitted(n))).is_err()
            {
                return;
            }
            std::thread::sleep(UNCOMMITTED_EVERY);
        }
    });
}

/// `cuz live --check`: every step `cuz live` depends on, outside the TUI, so a
/// failure is printed instead of hidden behind the interface.
fn check(repo: &because_git::Repo, agent: &dyn Agent, cfg: SessionConfig, summary: Option<(&str, &dyn Summarizer)>) -> Result<()> {
    use std::sync::mpsc;
    use std::time::Instant;
    use because_core::AgentEvent;

    let step = |name: &str, detail: &str| println!("  {name:<11} {detail}");
    println!("ts live --check");
    step("repository", &format!("{} ({})", repo.root.display(), repo.branch()));
    step("agent", &format!("{} · {}", agent.name(), agent.version()));
    // Not fatal: `cuz live` runs fine with the allowance bar saying "unavailable".
    match agent.allowance() {
        None => step("usage", &format!("{} has no allowance to report", agent.name())),
        Some(Ok((u, _))) => {
            let each: Vec<String> =
                u.windows.iter().map(|w| format!("{} {:.0}% left", w.label, w.left_pct())).collect();
            step("usage", &each.join(" · "));
        }
        Some(Err(e)) => step("usage", &format!("unavailable: {e}")),
    }
    let r = "refs/because/check/before";
    match repo.snapshot(None, "ts live --check", r) {
        Ok(sha) => {
            step("snapshot", &format!("ok ({}), removed again", &sha[..7]));
            let _ = repo.delete_ref(r);
        }
        Err(e) => {
            step("snapshot", &format!("FAILED: {e}"));
            bail!("git snapshots do not work in this repository");
        }
    }
    step("command", &agent.describe(&cfg));
    let (tx, rx) = mpsc::channel();
    let t0 = Instant::now();
    let mut cli = agent.start(
        &cfg,
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    )?;
    step("started", "sending \"Reply with exactly: ok\" (one tiny input; about two cents, mostly caching the system prompt)");
    cli.send("Reply with exactly: ok")?;
    let secs = || format!("{:.1}s", t0.elapsed().as_secs_f32());
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(90)) {
            Ok(AgentEvent::Started { model, .. }) => step("responding", &format!("model {model} · {}", secs())),
            Ok(AgentEvent::Finished { reply, cost_total_usd, is_error, .. }) => {
                step("finished", &format!("reply {reply:?} · ${cost_total_usd:.4} · {}", secs()));
                if is_error {
                    let log = cfg.log.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                    bail!("{} reported an error; see {log}", agent.name());
                }
                let Some((model, summarizer)) = summary else {
                    println!("\nEverything `cuz` needs works here. Run `cuz --no-summary` in this repository.");
                    return Ok(());
                };
                step("summary", &format!("asking {model} for a one-sentence summary (about a fifth of a cent)"));
                let prompt = because_agent::summary::prompt("Reply with exactly: ok", &[], "ok");
                match summarizer.summarize(&prompt) {
                    Ok(r) => step(
                        "summarized",
                        &format!("{:?} · {} · ${:.4} · {:.1}s", r.text, r.model, r.cost_usd, r.duration_ms as f32 / 1000.0),
                    ),
                    Err(e) => {
                        step("SUMMARY", &format!("failed: {e}"));
                        println!("\nThe agent works, but summaries do not. `cuz` still runs; inputs show the start");
                        println!("of the reply instead of a summary. `--no-summary` stops it trying.");
                        return Ok(());
                    }
                }
                println!("\nEverything `cuz` needs works here. Run `cuz` in this repository.");
                return Ok(());
            }
            Ok(AgentEvent::Exited { detail }) => {
                step("EXITED", &format!("{} stopped before replying:", agent.name()));
                for l in detail.lines().take(12) {
                    println!("              {l}");
                }
                bail!("{} exited early — the lines above are what it printed", agent.name());
            }
            Ok(_) => {}
            Err(_) => {
                step("TIMEOUT", &format!("no reply after {}", secs()));
                bail!(
                    "{} started but did not answer; its raw output is in {}",
                    agent.name(),
                    cfg.log.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
                );
            }
        }
    }
}
