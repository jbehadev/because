//! Claude Code's own transcripts (`~/.claude/projects/<dir>/<session>.jsonl`),
//! read as history: sessions run in Claude Code itself, before or beside
//! `because`.
//!
//! The format is not a published contract. Only a few record types are read —
//! the developer's inputs, the assistant's messages, the running cost and the
//! session's title — and everything else is skipped, so a record type added
//! later costs nothing. Checked against transcripts from Claude Code 2.1: an
//! interactive session (inputs carry `origin.kind = "human"`) and `claude -p`
//! sessions (no `origin`; inputs are plain strings).
//!
//! What a transcript cannot give: which files each input changed. Claude Code
//! records the edits its own tools made, but not what a shell command did, so
//! the importer ties inputs to the commits made while they ran instead
//! (`because-cli`'s import), and says that is how it knows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use because_core::{NarrationStep, Origin, SegStatus, Section, Segment, Session, StepKind, time};

use super::tool_target;

/// Where Claude Code keeps transcripts: `$CLAUDE_CONFIG_DIR/projects`, or
/// `~/.claude/projects`.
pub fn projects_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".claude")))
        .map(|d| d.join("projects"))
}

/// Claude Code's directory name for a project: the path with every character
/// that is not a letter or digit replaced by `-`.
pub fn slug(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// One transcript, parsed.
#[derive(Debug, Clone)]
pub struct Transcript {
    /// `origin: Imported`, keyed by Claude Code's session id. File changes and
    /// commits are left for the importer, which has the repository.
    pub session: Session,
    /// For each input, when it was sent and when the last thing that answered
    /// it was written (milliseconds since the epoch): the window a commit has
    /// to fall in to be counted as that input's.
    pub windows: Vec<(i64, i64)>,
}

/// `None` when the transcript holds no input a person typed.
pub fn parse(text: &str) -> Option<Transcript> {
    let mut s = Session { origin: Origin::Imported, ..Default::default() };
    let mut windows: Vec<(i64, i64)> = Vec::new();
    let mut cwd = String::new();
    // Running cost: Claude Code writes the session's total so far at some turn
    // ends, not one figure per input.
    let mut last_total = 0.0;
    let mut recorded: Option<f64> = None;
    let mut since_total: Vec<usize> = Vec::new();
    // tool_use id → its step in the latest input, to mark the ones that failed.
    let mut tool_steps: HashMap<String, usize> = HashMap::new();

    for line in text.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        let str_of = |k: &str| e[k].as_str().unwrap_or_default().to_string();
        match e["type"].as_str() {
            Some("ai-title") => {
                let t = str_of("aiTitle");
                if !t.trim().is_empty() {
                    s.title = t.trim().to_string();
                }
                continue;
            }
            Some("cost-state") => {
                let Some(total) = e["totalCostUSD"].as_f64() else { continue };
                // A total that went down means a new process started counting
                // again; what it counted is then all new.
                let delta = if total >= last_total { total - last_total } else { total };
                last_total = total;
                recorded = Some(recorded.unwrap_or(0.0) + delta);
                if let [only] = since_total[..] {
                    s.segments[only].cost_usd = delta;
                    s.segments[only].cost_unrecorded = false;
                }
                since_total.clear();
                continue;
            }
            Some("user" | "assistant") => {}
            _ => continue,
        }
        // Subagents' own conversations are not this session's inputs.
        if e["isSidechain"].as_bool().unwrap_or(false) {
            continue;
        }
        if s.id.is_empty() {
            s.id = str_of("sessionId");
        }
        if cwd.is_empty() {
            cwd = str_of("cwd");
            s.repo = cwd.clone();
        }
        if s.branch.is_empty() {
            s.branch = str_of("gitBranch");
        }
        let ts = str_of("timestamp");
        let ts_ms = time::parse_ms(&ts);
        if s.started_at.is_empty() {
            s.started_at = ts.clone();
        }
        let content = &e["message"]["content"];
        if e["type"] == "user" {
            if let Some(blocks) = content.as_array()
                && blocks.iter().any(|b| b["type"] == "tool_result")
            {
                if let Some(seg) = s.segments.last_mut() {
                    mark_failed_tools(seg, blocks, &tool_steps);
                    extend(&mut windows, ts_ms);
                }
                continue;
            }
            let Some(input) = human_input(&e) else { continue };
            s.segments.push(Segment {
                id: e["promptId"].as_str().or(e["uuid"].as_str()).unwrap_or_default().to_string(),
                seq: s.segments.len() as u32 + 1,
                input,
                started_at: ts,
                status: SegStatus::Done,
                cost_unrecorded: true,
                sections: vec![Section::whole_response(Vec::new())],
                ..Default::default()
            });
            since_total.push(s.segments.len() - 1);
            tool_steps.clear();
            windows.push((ts_ms.unwrap_or(0), ts_ms.unwrap_or(0)));
            continue;
        }
        // An assistant message, answering the latest input.
        let Some(seg) = s.segments.last_mut() else { continue };
        extend(&mut windows, ts_ms);
        let model = e["message"]["model"].as_str().unwrap_or_default();
        if seg.model.is_empty() && !model.is_empty() && model != "<synthetic>" {
            seg.model = model.to_string();
        }
        let steps = &mut seg.sections[0].narration;
        for b in content.as_array().into_iter().flatten() {
            match b["type"].as_str() {
                Some("text") => {
                    let t = b["text"].as_str().unwrap_or_default();
                    if !t.trim().is_empty() {
                        steps.push(NarrationStep { kind: StepKind::Text, body: t.to_string() });
                        seg.reply = t.to_string();
                    }
                }
                Some("thinking") => {
                    let t = b["thinking"].as_str().unwrap_or_default();
                    if !t.trim().is_empty() {
                        steps.push(NarrationStep { kind: StepKind::Thinking, body: t.to_string() });
                    }
                }
                Some("tool_use") => {
                    let name = b["name"].as_str().unwrap_or_default();
                    let target = tool_target(&b["input"]);
                    let target = target.strip_prefix(&format!("{cwd}/")).unwrap_or(&target).to_string();
                    let body = if target.is_empty() { name.to_string() } else { format!("{name} {target}") };
                    tool_steps.insert(b["id"].as_str().unwrap_or_default().to_string(), steps.len());
                    steps.push(NarrationStep { kind: StepKind::ToolCall, body });
                }
                _ => {}
            }
        }
    }
    if s.segments.is_empty() {
        return None;
    }
    for (seg, (start, end)) in s.segments.iter_mut().zip(&windows) {
        seg.duration_ms = (end - start).max(0) as u64;
        seg.elapsed = time::fmt_duration(seg.duration_ms);
        if seg.model.is_empty() {
            seg.model = "claude".into();
        }
    }
    s.recorded_cost = recorded;
    Some(Transcript { session: s, windows })
}

fn extend(windows: &mut [(i64, i64)], ts: Option<i64>) {
    if let (Some(w), Some(t)) = (windows.last_mut(), ts) {
        w.1 = w.1.max(t);
    }
}

/// The text of a record if it is something a person typed: not a tool result,
/// not text Claude Code adds itself (skill bodies, compaction summaries, slash
/// command output, interruption notices).
fn human_input(e: &Value) -> Option<String> {
    if e["isMeta"].as_bool().unwrap_or(false)
        || e["isCompactSummary"].as_bool().unwrap_or(false)
        || e["isVisibleInTranscriptOnly"].as_bool().unwrap_or(false)
        || !e["toolUseResult"].is_null()
    {
        return None;
    }
    // Where Claude Code says who wrote it, believe that.
    if let Some(kind) = e["origin"]["kind"].as_str()
        && kind != "human"
    {
        return None;
    }
    let content = &e["message"]["content"];
    let text = match content {
        Value::String(t) => t.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| match b["type"].as_str() {
                Some("text") => b["text"].as_str().map(str::to_string),
                Some("image") => Some("[image]".to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let t = text.trim();
    const NOT_TYPED: [&str; 7] = [
        "<command-name>",
        "<command-message>",
        "<local-command-",
        "<bash-input>",
        "<bash-stdout>",
        "[Request interrupted",
        "Caveat: The messages below",
    ];
    if t.is_empty() || NOT_TYPED.iter().any(|p| t.starts_with(p)) {
        return None;
    }
    Some(t.to_string())
}

fn mark_failed_tools(seg: &mut Segment, blocks: &[Value], tool_steps: &HashMap<String, usize>) {
    for b in blocks.iter().filter(|b| b["is_error"].as_bool().unwrap_or(false)) {
        let Some(&at) = b["tool_use_id"].as_str().and_then(|id| tool_steps.get(id)) else { continue };
        let why = match &b["content"] {
            Value::String(t) => t.clone(),
            Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(" "),
            _ => String::new(),
        };
        let why: String = why.lines().next().unwrap_or_default().chars().take(140).collect();
        if let Some(step) = seg.sections[0].narration.get_mut(at) {
            step.body = format!("{} — failed: {why}", step.body);
        }
    }
}
