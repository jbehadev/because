//! The one keymap. Dispatch and the help text both read from here, so they
//! cannot drift. See `docs/design/03-tui.md` §4.
//!
//! While an approval prompt or a question prompt is up, it takes every key but
//! Ctrl+C, which stops the input of that prompt. Otherwise
//! three layers, checked in order:
//!
//! 1. **Global** — structural keys that work in every pane, *including while
//!    typing*. None is a plain printable key, and none is bound by the
//!    composer's editor (checked against ratatui-textarea 0.9.2's default map).
//! 2. **Composer** — while typing a new segment, a handful of keys have composer
//!    meanings (submit, newline, leave). Everything else is text editing and is
//!    handed to the editor untouched, so `Ctrl+←/→`, `Ctrl+W`, `Ctrl+A/E`, etc.
//!    behave the way they do in any text box.
//! 3. **Command** — outside the composer, plain keys are commands.
//!
//! Rule of thumb for users: *Alt moves and toggles from anywhere; plain keys
//! type when you're typing and command when you're not; Esc switches.*

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{Focus, Model, Msg, PickKey, View};

pub fn dispatch(m: &Model, k: KeyEvent) -> Option<Msg> {
    use KeyCode::*;
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    let plain = k.modifiers.is_empty();
    let alt_only = k.modifiers == KeyModifiers::ALT;
    let empty = m.draft_is_empty();

    // ── 0. approval ────────────────────────────────────────────────────────
    // An agent is stopped until it is answered, so the prompt has the keyboard
    // — every key but Ctrl+C, which stops that input (and then quits). Answers
    // are not taken in the first moments it is up: it opens over whatever was
    // being typed.
    if let Some(a) = m.approval() {
        if k.code == Char('c') && ctrl {
            return Some(Msg::ClearOrQuit);
        }
        if !m.approval_ready() || !(plain || k.modifiers == KeyModifiers::SHIFT) {
            return None;
        }
        // The agent asks questions: the keys pick an option or type an answer.
        if !a.req.questions.is_empty() {
            let typing = a.pick.other.is_some();
            return Some(Msg::Pick(match k.code {
                Enter => PickKey::Enter,
                Esc => PickKey::Back,
                Backspace if typing => PickKey::Erase,
                Char(c) if typing => PickKey::Type(c),
                Up | Char('k') => PickKey::Up,
                Down | Char('j') => PickKey::Down,
                Char(' ') => PickKey::Toggle,
                Char(c @ '1'..='9') => PickKey::Choose(c as usize - '1' as usize),
                _ => return None,
            }));
        }
        return match k.code {
            Char('a') => Some(Msg::Answer(because_core::Decision::AllowOnce)),
            Char('A') if a.req.always.is_some() => Some(Msg::Answer(because_core::Decision::AllowSession)),
            Char('m') => Some(Msg::Answer(because_core::Decision::AllowAuto)),
            Char('d') => Some(Msg::Answer(because_core::Decision::Deny)),
            _ => None,
        };
    }

    // ── 1. global ──────────────────────────────────────────────────────────
    // Ctrl+X Ctrl+S is send now. After Ctrl+X, any other key does what it
    // always does.
    if m.chord_x.replace(false) && k.code == Char('s') && ctrl {
        return Some(Msg::SendNow);
    }
    match k.code {
        // Send now, as in Claude Code. Only a terminal with keyboard
        // enhancement tells Ctrl+Enter from Enter; Ctrl+X Ctrl+S works in all.
        Enter if ctrl => return Some(Msg::SendNow),
        // Shadows the editor's cut; use terminal selection.
        Char('x') if ctrl => return Some(Msg::ChordX),
        Tab if !alt && !ctrl => return Some(Msg::FocusNext),
        BackTab => return Some(Msg::FocusPrev),
        Left if alt && !ctrl => return Some(Msg::FocusLeft),
        Right if alt && !ctrl => return Some(Msg::FocusRight),
        // Moves the selection in the focused list. In the composer that is the
        // segment list, so you can browse segments without losing the draft.
        Up if alt && !ctrl => return Some(Msg::Up),
        Down if alt && !ctrl => return Some(Msg::Down),
        Char(c @ '1'..='4') if alt => return Some(Msg::FocusAt(c as usize - '1' as usize)),
        // Alt+X does what plain X does, from anywhere. Alt+v shadows the
        // editor's page-up; PageUp itself still works in the composer.
        Char('v') if alt => return Some(Msg::ToggleVerbatim),
        Char('t') if alt => return Some(Msg::ToggleTranscript),
        Char('w') if alt => return Some(Msg::ToggleRationale),
        Char('r') if alt => return Some(Msg::Read),
        // The one Alt key that is not its plain key everywhere: plain `n` keeps
        // its code-pane meaning (next region), while Alt+n always starts a
        // session, as it opens a tab elsewhere. It shadows the editor's
        // down-a-paragraph; Alt+] and Ctrl+↓ still do that.
        Char('n') if alt => return Some(Msg::NewSession),
        // Clears a draft first (undoable with Ctrl+U), then stops the running
        // input of this session, and quits only when there is nothing to lose.
        // Shadows the editor's copy; use terminal selection.
        Char('c') if ctrl => return Some(Msg::ClearOrQuit),
        _ => {}
    }

    // ── 2. composer ────────────────────────────────────────────────────────
    if m.composing() {
        // Below the reader, the page keys scroll the text above; so do Home
        // and End while there is nothing typed for them to move through.
        if m.view == View::Reader {
            match k.code {
                PageUp => return Some(Msg::PageUp),
                PageDown => return Some(Msg::PageDown),
                Home if empty && plain => return Some(Msg::Top),
                End if empty && plain => return Some(Msg::Bottom),
                _ => {}
            }
        }
        return Some(match k.code {
            // Alt+Enter arrives as ESC CR on every terminal. Shift+Enter is only
            // distinguishable from Enter when keyboard enhancement is active.
            Enter if alt || shift => Msg::Newline,
            // In raw mode LF (Ctrl+J) is distinct from CR (Enter). The editor
            // would treat it as delete-to-line-head; newline is more useful.
            Char('j') if ctrl => Msg::Newline,
            // With nothing to send, Enter opens the selected input in full, as
            // it does in the list.
            Enter if empty => Msg::Read,
            Enter => Msg::Submit,
            Esc => Msg::LeaveComposer,
            // With nothing typed there is nothing for arrows to edit, so every
            // form of arrow navigates: ↑/↓ browse segments, ←/→ change pane.
            // Once there is text, they belong to the editor and Tab moves panes.
            Up if empty && plain => Msg::Up,
            Down if empty && plain => Msg::Down,
            Left if empty && plain => Msg::FocusLeft,
            Right if empty && plain => Msg::FocusRight,
            Char('b') if empty && alt_only => Msg::FocusLeft,
            Char('f') if empty && alt_only => Msg::FocusRight,
            // Otherwise Alt+b/Alt+f reach the editor as word movement — which is
            // what Option+←/→ means in every macOS text field.
            _ => Msg::Edit(k),
        });
    }

    // ── 3. command ─────────────────────────────────────────────────────────
    // macOS Terminal and iTerm2 send Option+←/→ as ESC b / ESC f (the readline
    // word-movement codes), which crossterm reports as Alt+b / Alt+f. Outside
    // the composer there is no text, so they move panes like Alt+←/→.
    match k.code {
        Char('b') if alt_only => return Some(Msg::FocusLeft),
        Char('f') if alt_only => return Some(Msg::FocusRight),
        _ => {}
    }
    if alt || ctrl {
        return None;
    }
    // The reader is a page of text: movement keys scroll it.
    if m.view == View::Reader {
        return Some(match k.code {
            Char('k') | Up => Msg::Up,
            Char('j') | Down => Msg::Down,
            Char(' ') | PageDown => Msg::PageDown,
            Char('b') | PageUp => Msg::PageUp,
            Char('g') | Home => Msg::Top,
            Char('G') | End => Msg::Bottom,
            Char(']') => Msg::NextInput,
            Char('[') => Msg::PrevInput,
            Char('r') | Char('h') | Char('q') | Left | Esc => Msg::Back,
            Char('l') | Right => Msg::FocusRight,
            Char(c @ '1'..='4') => Msg::FocusAt(c as usize - '1' as usize),
            Char('i') | Char('a') => Msg::Compose,
            Char('t') => Msg::ToggleTranscript,
            Char('Q') => Msg::Quit,
            _ => return None,
        });
    }
    Some(match k.code {
        Char(c @ '1'..='4') => Msg::FocusAt(c as usize - '1' as usize),
        Char('h') | Left => Msg::FocusLeft,
        Char('l') | Right => Msg::FocusRight,
        Char('k') | Up => Msg::Up,
        Char('j') | Down => Msg::Down,
        Char('i') | Char('a') => Msg::Compose,
        Enter => Msg::Enter,
        Esc | Char('q') => Msg::Back,
        Char('W') => Msg::ToggleRationale,
        Char('v') => Msg::ToggleVerbatim,
        Char('t') => Msg::ToggleTranscript,
        Char('r') => Msg::Read,
        Char('n') if m.focus == Focus::Sessions => Msg::NewSession,
        // Only an earlier session has anything to revive; elsewhere `c` is free.
        Char('c') if m.session().origin.is_history() => Msg::Revive,
        Char('n') if m.view == View::Code => Msg::NextRegion,
        Char('N') if m.view == View::Code => Msg::PrevRegion,
        Char(']') if m.view == View::Code => Msg::NextAny,
        Char('[') if m.view == View::Code => Msg::PrevAny,
        Char('Q') => Msg::Quit,
        _ => return None,
    })
}

/// Contextual hint for the status line. Generated from the same state that
/// drives dispatch, so the hint is always true for the current mode.
pub fn hint(m: &Model) -> String {
    let mut h = base_hint(m).to_string();
    // A draft that starts with `!` is a shell command.
    if m.composing() && m.draft().trim_start().starts_with('!') {
        h = h.replace("Enter send", "Enter runs the command");
    }
    // Ctrl+C stops before it quits; a draft it clears comes first.
    if m.stoppable().is_some() {
        h.replace("Enter send", "Enter send · Ctrl+Enter send now")
            .replace("Ctrl+C quits", "Ctrl+C stops")
            .replace(" · Q quit", " · Ctrl+C stops · Q quit")
    } else {
        h
    }
}

fn base_hint(m: &Model) -> &'static str {
    if let Some(a) = m.approval() {
        let multi = a.req.questions.get(a.pick.at).is_some_and(|q| q.multi);
        if a.pick.other.is_some() {
            "type your answer · Enter answers · Esc options · Ctrl+C quits"
        } else if multi {
            "↑/↓ move · Space or 1-9 select · Enter done · Esc decline · Ctrl+C quits"
        } else if !a.req.questions.is_empty() {
            "↑/↓ move · Enter or 1-9 choose · Esc decline · Ctrl+C quits"
        } else if a.req.always.is_some() {
            "a allow once · A allow for this session · m auto mode · d deny · Ctrl+C quits"
        } else {
            "a allow once · m auto mode · d deny · Ctrl+C quits"
        }
    } else if m.composing() && m.view == View::Reader {
        if m.draft_is_empty() {
            "↑/↓ scroll · PgUp/PgDn page · type to write · Enter/Esc back · Ctrl+C quits"
        } else {
            "typing · Enter send · Alt+Enter newline · PgUp/PgDn scroll · Esc back · Ctrl+C clear"
        }
    } else if m.composing() {
        if m.draft_is_empty() {
            "arrows move · Enter read · type to write · Tab panes · Esc commands · Ctrl+C quits"
        } else {
            "typing · Enter send · Alt+Enter newline · Tab panes · Esc commands · Ctrl+C clear"
        }
    } else if m.reader_has_composer() {
        "j/k scroll · Space/b page · g/G ends · [ inputs · i type · Esc/q back · Q quit"
    } else if m.view == View::Reader {
        "j/k scroll · Space/b page · g/G ends · ]/[ inputs · Esc/q back · Q quit"
    } else if m.view == View::Code {
        "W why · n/N this input · ]/[ any input · i next input · Alt+← back · Q quit"
    } else if m.session().origin.is_history() && m.view == View::Columns {
        "c revive this session · Alt+n new session · Enter open · j/k move · 1-4 or Alt+←/→ panes · Q quit"
    } else if m.focus == Focus::Sessions {
        "n new session · j/k sessions · Enter its inputs · 1-4 or Alt+←/→ panes · Q quit"
    } else if m.focus == Focus::Segments {
        "Enter read · i type · j/k inputs · v verbatim · t transcript · 1-4 or Alt+←/→ panes · Q quit"
    } else {
        "i next input · j/k move · Enter open · 1-4 or Alt+←/→ panes · Q quit"
    }
}
