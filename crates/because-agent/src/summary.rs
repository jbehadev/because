//! One-sentence summaries of a finished input, written by a small model.
//!
//! The prompt is the same for each model. A `Summarizer` sends it to one
//! model; `claude::summary` has the first one.

/// Without the length limit restated at the end of the input (see `prompt`),
/// Haiku wrote about 60 words; with it, 17.
pub const SYSTEM: &str = "You write one-line summaries of a coding agent's work for a review tool. \
Output a single plain sentence of at most 25 words saying what the agent did or answered. \
No preamble, no markdown.";

const INPUT_MAX: usize = 4_000;
const SAID_HEAD: usize = 12_000;
const SAID_TAIL: usize = 3_000;
const TOOLS_MAX: usize = 30;

/// What the summarizer is shown: the input, the tool calls, and what the agent
/// said. Long text keeps its beginning and end; the middle is cut.
pub fn prompt(input: &str, tools: &[&str], said: &str) -> String {
    let tools = if tools.is_empty() {
        "(none)".to_string()
    } else {
        let mut t: Vec<String> = tools.iter().take(TOOLS_MAX).map(|s| format!("- {s}")).collect();
        if tools.len() > TOOLS_MAX {
            t.push(format!("- … and {} more", tools.len() - TOOLS_MAX));
        }
        format!("\n{}", t.join("\n"))
    };
    format!(
        "Input the developer typed:\n{}\n\nTools the agent ran: {tools}\n\nThe agent's reply:\n{}\n\n---\nSummarize the exchange above in ONE sentence of at most 25 words.",
        head(input, INPUT_MAX),
        head_tail(said, SAID_HEAD, SAID_TAIL),
    )
}

fn head(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}\n[…]", &s[..i]),
        None => s.to_string(),
    }
}

fn head_tail(s: &str, h: usize, t: usize) -> String {
    let n = s.chars().count();
    if n <= h + t {
        return s.to_string();
    }
    let start: String = s.chars().take(h).collect();
    let end: String = s.chars().skip(n - t).collect();
    format!("{start}\n[… {} characters cut …]\n{end}", n - h - t)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub text: String,
    /// The model that answered, as its agent reports it.
    pub model: String,
    pub cost_usd: f64,
    pub duration_ms: u64,
}

/// A small model that writes the summaries. Each call blocks, so call it
/// from a worker thread.
pub trait Summarizer: Send + Sync {
    /// Send a prompt from `prompt`, with `SYSTEM` as the system prompt.
    fn summarize(&self, prompt: &str) -> anyhow::Result<Summary>;
}
