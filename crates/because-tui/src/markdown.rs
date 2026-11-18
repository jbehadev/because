//! Enough Markdown for the reader: headings, lists, quotes, fenced code, rules,
//! and inline `**bold**`, `*italic*` and `` `code` ``. Agents answer in
//! Markdown, and a 1,000-word answer is hard to read with its markup showing.
//!
//! It wraps as it renders, so a list item's continuation lines stay indented
//! under its text; ratatui's own wrapping starts them at the margin. Widths are
//! measured in terminal cells. Anything it does not recognise (tables, HTML,
//! links) is shown as written.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub struct Theme {
    pub text: Style,
    pub code: Style,
    pub dim: Style,
}

pub fn render(src: &str, width: usize, t: &Theme) -> Vec<Line<'static>> {
    let width = width.max(16);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut fence: Option<String> = None;
    for raw in src.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(marker) = &fence {
            if trimmed.starts_with(marker.as_str()) {
                fence = None;
            } else {
                for chunk in hard_wrap(line, width.saturating_sub(2)) {
                    out.push(Line::from(vec![Span::styled("▏ ", t.dim), Span::styled(chunk, t.code)]));
                }
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(trimmed[..3].to_string());
            let lang = trimmed[3..].trim();
            if !lang.is_empty() {
                out.push(Line::from(Span::styled(format!("▏ {lang}"), t.dim)));
            }
            continue;
        }
        if trimmed.is_empty() {
            if out.last().is_some_and(|l| l.width() > 0) {
                out.push(Line::default());
            }
            continue;
        }
        if is_rule(trimmed) {
            out.push(Line::from(Span::styled("─".repeat(width.min(40)), t.dim)));
            continue;
        }
        if let Some((level, text)) = heading(trimmed) {
            if out.last().is_some_and(|l| l.width() > 0) {
                out.push(Line::default());
            }
            let mut style = t.text.add_modifier(Modifier::BOLD);
            if level == 1 {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            out.extend(wrap(inline(text, style, t), width, Vec::new(), 0));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            let lead = vec![Span::styled("│ ", t.dim)];
            out.extend(wrap(inline(rest, t.text.add_modifier(Modifier::ITALIC), t), width, lead, 2));
            continue;
        }
        if trimmed.starts_with('|') {
            out.push(Line::from(Span::styled(ellipsize_cells(line, width), t.text)));
            continue;
        }
        let pad = indent.min(12);
        if let Some((bullet, text)) = list_item(trimmed) {
            let lead = vec![Span::raw(" ".repeat(pad)), Span::styled(format!("{bullet} "), t.dim)];
            let hang = pad + cells(&bullet) + 1;
            out.extend(wrap(inline(text, t.text, t), width, lead, hang));
            continue;
        }
        let lead = if pad > 0 { vec![Span::raw(" ".repeat(pad))] } else { Vec::new() };
        out.extend(wrap(inline(trimmed, t.text, t), width, lead, pad));
    }
    while out.last().is_some_and(|l| l.width() == 0) {
        out.pop();
    }
    out
}

fn is_rule(s: &str) -> bool {
    let c: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    c.len() >= 3 && (c.chars().all(|x| x == '-') || c.chars().all(|x| x == '*') || c.chars().all(|x| x == '_'))
}

fn heading(s: &str) -> Option<(usize, &str)> {
    let level = s.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&level) && s[level..].starts_with(' ') {
        Some((level, s[level..].trim().trim_end_matches('#').trim_end()))
    } else {
        None
    }
}

/// `- x`, `* x`, `+ x` → ("•", "x"); `12. x` or `12) x` → ("12.", "x").
fn list_item(s: &str) -> Option<(String, &str)> {
    for b in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(b) {
            return Some(("•".into(), rest.trim_start()));
        }
    }
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=3).contains(&digits) {
        let rest = &s[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((format!("{}.", &s[..digits]), r.trim_start()));
        }
    }
    None
}

/// Inline markup to styled spans. Unclosed markers are kept as text.
/// Underscores are left alone: in code-heavy prose they are identifiers far
/// more often than emphasis.
fn inline(s: &str, base: Style, t: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut rest = s;
    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>, style: Style| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), style));
        }
    };
    while let Some(c) = rest.chars().next() {
        let (marker, style) = if rest.starts_with('`') {
            ("`", t.code)
        } else if rest.starts_with("**") {
            ("**", base.add_modifier(Modifier::BOLD))
        } else if c == '*' && rest[1..].starts_with(|n: char| !n.is_whitespace() && n != '*') {
            ("*", base.add_modifier(Modifier::ITALIC))
        } else {
            buf.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        };
        let body = &rest[marker.len()..];
        match body.find(marker) {
            Some(end) if end > 0 => {
                flush(&mut buf, &mut spans, base);
                spans.push(Span::styled(body[..end].to_string(), style));
                rest = &body[end + marker.len()..];
            }
            _ => {
                buf.push_str(marker);
                rest = body;
            }
        }
    }
    flush(&mut buf, &mut spans, base);
    spans
}

fn cells(s: &str) -> usize {
    Span::raw(s).width()
}

/// Greedy word wrap over styled spans. `lead` starts the first line; later
/// lines are indented by `hang` cells. A word wider than a line is split.
fn wrap(spans: Vec<Span<'static>>, width: usize, lead: Vec<Span<'static>>, hang: usize) -> Vec<Line<'static>> {
    // (word, style, preceded by whitespace)
    let mut words: Vec<(String, Style, bool)> = Vec::new();
    let mut space = false;
    for sp in &spans {
        let mut cur = String::new();
        for ch in sp.content.chars() {
            if ch.is_whitespace() {
                if !cur.is_empty() {
                    words.push((std::mem::take(&mut cur), sp.style, space));
                }
                space = true;
            } else {
                cur.push(ch);
            }
        }
        if !cur.is_empty() {
            words.push((cur, sp.style, space));
            space = false;
        }
    }
    let lead_w: usize = lead.iter().map(|s| s.width()).sum();
    let mut lines = Vec::new();
    let mut line: Vec<Span<'static>> = lead;
    let mut used = lead_w;
    let mut fresh = true; // nothing but the lead/indent on this line yet
    let avail = |first: bool| if first { width.saturating_sub(lead_w) } else { width.saturating_sub(hang) };
    let mut first = true;
    for (word, style, sp) in words {
        let w = cells(&word);
        let mut gap = usize::from(sp && !fresh);
        if !fresh && used + gap + w > width {
            gap = 0;
            lines.push(Line::from(std::mem::take(&mut line)));
            line = vec![Span::raw(" ".repeat(hang))];
            used = hang;
            fresh = true;
            first = false;
        }
        if fresh && w > avail(first) {
            // Too wide for any line: split it across lines.
            for piece in hard_wrap(&word, avail(first).max(1)) {
                if !fresh {
                    lines.push(Line::from(std::mem::take(&mut line)));
                    line = vec![Span::raw(" ".repeat(hang))];
                    used = hang;
                    first = false;
                }
                used += cells(&piece);
                line.push(Span::styled(piece, style));
                fresh = false;
            }
            continue;
        }
        if gap == 1 {
            line.push(Span::raw(" "));
            used += 1;
        }
        used += w;
        line.push(Span::styled(word, style));
        fresh = false;
    }
    if !fresh || lines.is_empty() {
        lines.push(Line::from(line));
    }
    lines
}

/// Split by cells without regard to words: for code, which must not reflow.
fn hard_wrap(s: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut used = 0;
    for ch in s.chars() {
        let w = cells(ch.encode_utf8(&mut [0; 4]));
        if used + w > width && used > 0 {
            out.push(String::new());
            used = 0;
        }
        out.last_mut().unwrap().push(ch);
        used += w;
    }
    out
}

fn ellipsize_cells(s: &str, width: usize) -> String {
    if cells(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        if cells(&out) + cells(ch.encode_utf8(&mut [0; 4])) + 1 > width {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme { text: Style::default(), code: Style::default(), dim: Style::default() }
    }
    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn list_items_hang_under_their_text() {
        let out = render("- alpha beta gamma delta epsilon zeta eta theta", 20, &theme());
        assert_eq!(text(&out), ["• alpha beta gamma", "  delta epsilon zeta", "  eta theta"]);
    }

    #[test]
    fn markup_is_rendered_not_shown() {
        let out = render("# Title\n\nSome **bold** and `code` and *it*.", 40, &theme());
        assert_eq!(text(&out), ["Title", "", "Some bold and code and it."]);
        let bold = out[2].spans.iter().find(|s| s.content == "bold").unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn code_blocks_keep_their_lines() {
        let out = render("```rust\nfn  main() {}\n```\nafter", 40, &theme());
        assert_eq!(text(&out), ["▏ rust", "▏ fn  main() {}", "after"]);
    }

    #[test]
    fn unclosed_markers_and_snake_case_stay_as_written() {
        let out = render("a * b and some_name_here and **open", 60, &theme());
        assert_eq!(text(&out), ["a * b and some_name_here and **open"]);
    }

    #[test]
    fn numbered_items_and_long_words() {
        let out = render("12. abcdefghijklmnopqrstuvwxyz", 12, &theme());
        // width is raised to the 16-cell floor
        assert_eq!(text(&out), ["12. abcdefghijkl", "    mnopqrstuvwx", "    yz"]);
    }
}
