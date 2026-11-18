//! The summarizer on the developer's own Claude Code login: one `claude -p`
//! call for each input, so it needs no API key.
//!
//! The interim summarizer until M2's (`docs/design/02-summarization.md`).
//! Measured with claude-haiku-4-5 on a ~1,000-word reply: about $0.002 and
//! 1.3–2.2 s with thinking off; $0.005–0.006 and 7–9 s with it on, which is
//! why `run` turns it off.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::summary::{SYSTEM, Summarizer, Summary};

/// Read `claude -p --output-format json` output.
pub fn parse_result(stdout: &str) -> anyhow::Result<Summary> {
    let v: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| anyhow::anyhow!("unexpected output from claude ({e}): {}", first_line(stdout)))?;
    let text = v["result"].as_str().unwrap_or_default().trim().to_string();
    if v["is_error"].as_bool().unwrap_or(false) || v["subtype"] != "success" {
        let why = if text.is_empty() { v["subtype"].as_str().unwrap_or("error").to_string() } else { text };
        anyhow::bail!("claude reported an error: {}", first_line(&why));
    }
    if text.is_empty() {
        anyhow::bail!("claude returned an empty summary");
    }
    Ok(Summary {
        // One sentence was asked for; a model that adds a second line is cut
        // back to the first rather than trusted to be brief.
        text: first_line(&text).to_string(),
        model: v["modelUsage"].as_object().and_then(|m| m.keys().next().cloned()).unwrap_or_default(),
        cost_usd: v["total_cost_usd"].as_f64().unwrap_or(0.0),
        duration_ms: v["duration_ms"].as_u64().unwrap_or(0),
    })
}

fn first_line(s: &str) -> &str {
    s.trim().lines().next().unwrap_or_default()
}

pub fn args(model: &str) -> Vec<String> {
    [
        "-p",
        "--model",
        model,
        "--system-prompt",
        SYSTEM,
        // No tools: it only reads what it is given.
        "--tools",
        "",
        // Keeps these calls out of `claude --resume` and the transcript store.
        "--no-session-persistence",
        "--output-format",
        "json",
        // A guard, not an expectation: a call costs about a fifth of a cent.
        "--max-budget-usd",
        "0.05",
    ]
    .map(String::from)
    .to_vec()
}

/// Run one summary call. Blocking; call it from a worker thread.
///
/// It runs in the system temp directory so the repository's CLAUDE.md is not
/// loaded into a call that has no use for it. The prompt goes in on stdin.
pub fn run(program: &Path, model: &str, prompt: &str) -> anyhow::Result<Summary> {
    let mut child = Command::new(program)
        .args(args(model))
        .current_dir(std::env::temp_dir())
        .env("MAX_THINKING_TOKENS", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("could not start {}: {e}", program.display()))?;
    child.stdin.take().expect("piped stdin").write_all(prompt.as_bytes())?;
    let out = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("claude printed nothing ({}): {}", out.status, first_line(&err));
    }
    parse_result(&stdout)
}

/// `claude -p` with one model, as a `Summarizer`.
pub struct ClaudeSummarizer {
    pub program: PathBuf,
    pub model: String,
}

impl Summarizer for ClaudeSummarizer {
    fn summarize(&self, prompt: &str) -> anyhow::Result<Summary> {
        run(&self.program, &self.model, prompt)
    }
}
