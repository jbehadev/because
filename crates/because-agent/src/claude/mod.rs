//! The first backend: the Claude Code CLI in streaming JSON mode.
//!
//! One `claude` process serves a whole session. Each input is written to its
//! stdin as a JSON line; it streams events back on stdout, and a response ends
//! with a `result` event. See `docs/design/04-agent-host.md` §1.
//!
//! The event shapes here were taken from a recorded real session
//! (`fixtures/claude-stream/two-inputs.jsonl`), not from documentation — the
//! stream format is not a published contract, so unknown events are ignored
//! rather than treated as errors.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, bail};
use serde_json::{Value, json};
use because_core::{AgentEvent, Decision, PermissionRequest, Question, QuestionOption, Usage};

use crate::{Agent, AgentSession, Emit, SessionConfig};

pub mod summary;
pub mod transcript;
pub mod usage;

/// Claude Code, as an `Agent`.
pub struct Claude {
    pub program: PathBuf,
    version: String,
}

impl Claude {
    /// Find the CLI and ask it for its version. `explicit` is a path given
    /// by the developer, which is used as it is.
    pub fn find(explicit: Option<String>) -> anyhow::Result<Self> {
        let program = find_program(explicit)?;
        let out = Command::new(&program)
            .arg("--version")
            .output()
            .with_context(|| format!("could not run `{} --version`", program.display()))?;
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(Self { program, version })
    }
}

impl Agent for Claude {
    fn name(&self) -> &str {
        "claude"
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn describe(&self, cfg: &SessionConfig) -> String {
        format!("{} {}", self.program.display(), args(cfg).join(" "))
    }

    fn start(&self, cfg: &SessionConfig, emit: Emit) -> anyhow::Result<Box<dyn AgentSession>> {
        Ok(Box::new(ClaudeCli::spawn(&self.program, cfg, emit)?))
    }

    /// `claude --resume` needs Claude Code's transcript of the session, which
    /// it looks up by the path of the tree that the session ran in.
    fn check_resume(&self, cfg: &SessionConfig) -> anyhow::Result<()> {
        let Some(projects) = transcript::projects_dir() else {
            bail!("neither CLAUDE_CONFIG_DIR nor HOME is set, so there is no transcript to resume from");
        };
        let path = projects.join(transcript::slug(&cfg.cwd)).join(format!("{}.jsonl", cfg.session_id));
        if !path.is_file() {
            bail!(
                "Claude Code has no transcript of it at {} (it deletes them after 30 days unless cleanupPeriodDays is raised)",
                path.display()
            );
        }
        Ok(())
    }

    fn allowance(&self) -> Option<anyhow::Result<(Usage, String)>> {
        Some(usage::fetch_raw())
    }
}

/// Find the Claude Code CLI. A shell alias is invisible to other programs, so
/// besides PATH this looks where the installers put it.
fn find_program(explicit: Option<String>) -> anyhow::Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("claude")).collect::<Vec<_>>())
        .unwrap_or_default();
    let fallbacks = [
        home.join(".claude/local/claude"),
        home.join(".local/bin/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ];
    for c in on_path.iter().chain(fallbacks.iter()) {
        if is_executable(c) {
            return Ok(c.clone());
        }
    }
    bail!(
        "could not find `claude`. Looked on PATH and in ~/.claude/local, ~/.local/bin, /opt/homebrew/bin \
         and /usr/local/bin. If you start it through a shell alias, `type claude` shows the real path; \
         pass it with `cuz --claude /that/path`."
    )
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Turn one line of `claude --output-format stream-json` into events.
///
/// Pure, so it can be tested against recordings. Events from subagents
/// (`parent_tool_use_id` set) are skipped: they are the subagent's own
/// conversation, not this session's.
pub fn parse_line(line: &str) -> Vec<AgentEvent> {
    let Ok(e) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    if !e["parent_tool_use_id"].is_null() {
        return Vec::new();
    }
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    match e["type"].as_str() {
        Some("system") if e["subtype"] == "init" => {
            vec![AgentEvent::Started { model: s(&e["model"]), session_id: s(&e["session_id"]) }]
        }
        // When the agent is idle, a response of its own follows. While it
        // works, the agent reads the notice at its next step.
        Some("system") if e["subtype"] == "task_notification" => vec![AgentEvent::TaskEnded(s(&e["summary"]))],
        Some("stream_event") => {
            let ev = &e["event"];
            if ev["type"] == "content_block_delta" && ev["delta"]["type"] == "text_delta" {
                vec![AgentEvent::TextDelta(s(&ev["delta"]["text"]))]
            } else if ev["type"] == "content_block_start" {
                let b = &ev["content_block"];
                match b["type"].as_str() {
                    Some("thinking") => vec![AgentEvent::Activity("thinking".into())],
                    Some("text") => vec![AgentEvent::Activity("writing".into())],
                    Some("tool_use") => vec![AgentEvent::Activity(format!("running {}", s(&b["name"])))],
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            }
        }
        Some("assistant") => e["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| match b["type"].as_str()? {
                "text" => Some(AgentEvent::Text(s(&b["text"]))),
                // Thinking text is often empty (only a signature is returned);
                // there is nothing to show or keep in that case.
                "thinking" => {
                    let t = s(&b["thinking"]);
                    (!t.trim().is_empty()).then_some(AgentEvent::Thinking(t))
                }
                "tool_use" => Some(AgentEvent::ToolUse {
                    id: s(&b["id"]),
                    name: s(&b["name"]),
                    target: tool_target(&b["input"]),
                }),
                _ => None,
            })
            .collect(),
        // With `--replay-user-messages`, the CLI echoes each input when it
        // reads it. Its content is a string, or text blocks.
        Some("user") if e["isReplay"] == true => {
            let c = &e["message"]["content"];
            let text = c.as_str().map(String::from).unwrap_or_else(|| result_text(c));
            vec![AgentEvent::InputTaken(text)]
        }
        Some("user") => e["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "tool_result")
            .map(|b| AgentEvent::ToolResult {
                id: s(&b["tool_use_id"]),
                is_error: b["is_error"].as_bool().unwrap_or(false),
                text: result_text(&b["content"]),
            })
            .collect(),
        Some("result") => vec![AgentEvent::Finished {
            reply: s(&e["result"]),
            cost_total_usd: e["total_cost_usd"].as_f64().unwrap_or(0.0),
            duration_ms: e["duration_ms"].as_u64().unwrap_or(0),
            is_error: e["is_error"].as_bool().unwrap_or(false),
            permission_denials: e["permission_denials"].as_array().map_or(0, |a| a.len()),
        }],
        // With `--permission-prompt-tool stdio`, a tool call that needs
        // approval arrives as a request on stdout and waits for an answer on
        // stdin. Subagents ask this way too, and they block the same, so these
        // are not skipped like the subagent's own messages.
        Some("control_request") if e["request"]["subtype"] == "can_use_tool" => {
            let r = &e["request"];
            let tool = s(&r["tool_name"]);
            let questions = if tool == "AskUserQuestion" { questions(&r["input"]) } else { Vec::new() };
            vec![AgentEvent::PermissionRequest(PermissionRequest {
                request_id: s(&e["request_id"]),
                target: questions.first().map_or_else(|| tool_target(&r["input"]), |q| q.question.clone()),
                detail: permission_detail(&tool, &r["input"]),
                always: always_label(&r["permission_suggestions"]),
                tool,
                questions,
            })]
        }
        Some("control_cancel_request") => {
            vec![AgentEvent::PermissionCancelled { request_id: s(&e["request_id"]) }]
        }
        _ => Vec::new(),
    }
}

/// The questions of an `AskUserQuestion` call.
fn questions(input: &Value) -> Vec<Question> {
    input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|q| Question {
            question: text(&q["question"]),
            header: text(&q["header"]),
            options: q["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| QuestionOption { label: text(&o["label"]), description: text(&o["description"]) })
                .collect(),
            multi: q["multiSelect"].as_bool().unwrap_or(false),
        })
        .collect()
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// Lines of a tool call shown in the approval prompt before the rest is cut.
const DETAIL_LINES: usize = 24;

/// A tool call as the developer needs to see it to approve it: a command in
/// full, a file and what is written to it, or — for a tool this does not know —
/// its input as it was sent.
pub fn permission_detail(tool: &str, input: &Value) -> String {
    let str_of = |k: &str| input[k].as_str().unwrap_or_default();
    let mut out = match tool {
        "Bash" => {
            let mut t = str_of("command").to_string();
            if !str_of("description").is_empty() {
                t = format!("{t}\n\n{}", str_of("description"));
            }
            t
        }
        "Write" => format!("{}\n\n{}", str_of("file_path"), str_of("content")),
        "Edit" => format!("{}\n\n− {}\n+ {}", str_of("file_path"), str_of("old_string"), str_of("new_string")),
        "WebFetch" => format!("{}\n\n{}", str_of("url"), str_of("prompt")),
        _ => serde_json::to_string_pretty(input).unwrap_or_default(),
    };
    let lines: Vec<&str> = out.lines().collect();
    if lines.len() > DETAIL_LINES {
        out = format!("{}\n… {} more lines", lines[..DETAIL_LINES].join("\n"), lines.len() - DETAIL_LINES);
    }
    out
}

/// What "allow for the session" would allow, from the agent's suggestions:
/// the rules it would add, as Claude Code writes them in settings.
fn always_label(suggestions: &Value) -> Option<String> {
    let mut parts = Vec::new();
    for s in suggestions.as_array().into_iter().flatten() {
        match s["type"].as_str() {
            Some("addRules") if s["behavior"] == "allow" => {
                for r in s["rules"].as_array().into_iter().flatten() {
                    let tool = r["toolName"].as_str().unwrap_or_default();
                    parts.push(match r["ruleContent"].as_str() {
                        Some(c) => format!("{tool}({c})"),
                        None => tool.to_string(),
                    });
                }
            }
            Some("setMode") => parts.push(format!("{} mode", s["mode"].as_str().unwrap_or_default())),
            Some("addDirectories") => {
                for d in s["directories"].as_array().into_iter().flatten() {
                    parts.push(format!("access to {}", d.as_str().unwrap_or_default()));
                }
            }
            _ => {}
        }
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The line to write to the CLI's stdin to answer a permission request.
/// `request` is the request object as it arrived.
///
/// Allowing passes the input back unchanged: the CLI runs whatever input the
/// answer carries. Allowing for the session adds the agent's own suggestions,
/// kept to this session rather than written to the settings file some of them
/// name — a rule saved there would outlive the session it was approved in.
/// Switching to auto mode is kept to the session the same way.
///
/// The answers to an `AskUserQuestion` call go in `updatedInput.answers`, by
/// question text. Without them, the CLI tells the agent that the developer
/// did not answer.
pub fn answer_line(request_id: &str, request: &Value, decision: Decision) -> String {
    let response = match decision {
        Decision::AllowOnce => json!({ "behavior": "allow", "updatedInput": request["input"] }),
        Decision::AllowSession => {
            let updates: Vec<Value> = request["permission_suggestions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| {
                    let mut s = s.clone();
                    s["destination"] = json!("session");
                    s
                })
                .collect();
            json!({ "behavior": "allow", "updatedInput": request["input"], "updatedPermissions": updates })
        }
        Decision::AllowAuto => {
            let update = json!({ "type": "setMode", "mode": "auto", "destination": "session" });
            json!({ "behavior": "allow", "updatedInput": request["input"], "updatedPermissions": [update] })
        }
        Decision::Deny if request["tool_name"] == "AskUserQuestion" => {
            json!({ "behavior": "deny", "message": "The developer declined to answer the questions." })
        }
        Decision::Deny => json!({ "behavior": "deny", "message": "The developer declined this tool call." }),
        Decision::Answers(answers) => {
            let mut input = request["input"].clone();
            let by_question: serde_json::Map<String, Value> = input["questions"]
                .as_array()
                .into_iter()
                .flatten()
                .zip(answers)
                .map(|(q, a)| (text(&q["question"]), json!(a)))
                .collect();
            input["answers"] = Value::Object(by_question);
            json!({ "behavior": "allow", "updatedInput": input })
        }
    };
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response },
    })
    .to_string()
}

/// The line to write to the CLI's stdin to stop the input that runs. The CLI
/// withdraws a permission request that waits (`control_cancel_request`), then
/// ends the input with a `result` that has `is_error` set. The process stays,
/// and it accepts the next input.
pub fn interrupt_line(request_id: &str) -> String {
    json!({ "type": "control_request", "request_id": request_id, "request": { "subtype": "interrupt" } }).to_string()
}

/// The answer to a control request this host does not handle, so the CLI is
/// not left waiting on it.
fn unsupported_line(request_id: &str, subtype: &str) -> String {
    json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request_id, "error": format!("`because` does not handle {subtype}") },
    })
    .to_string()
}

/// What a tool call acts on, for a one-line display.
pub(crate) fn tool_target(input: &Value) -> String {
    for key in ["file_path", "notebook_path", "command", "pattern", "url", "description", "query"] {
        if let Some(v) = input[key].as_str() {
            return v.lines().next().unwrap_or_default().to_string();
        }
    }
    String::new()
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(t) => t.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The CLI's arguments for a session.
pub fn args(cfg: &SessionConfig) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        // stream-json output requires --verbose in print mode
        "--verbose",
        "--include-partial-messages",
        // Echo each input when the agent reads it. An input sent while the
        // agent works then shows if it joined the running input.
        "--replay-user-messages",
        if cfg.resume { "--resume" } else { "--session-id" },
        &cfg.session_id,
        "--permission-mode",
        &cfg.permission_mode,
        // Ask on stdout and wait for the answer on stdin, instead of
        // refusing whatever the mode does not allow.
        "--permission-prompt-tool",
        "stdio",
    ]
    .map(String::from)
    .to_vec();
    if let Some(m) = &cfg.model {
        a.extend(["--model".into(), m.clone()]);
    }
    if cfg.ephemeral {
        a.push("--no-session-persistence".into());
    }
    a
}

/// A running `claude` process for one session.
pub struct ClaudeCli {
    child: Child,
    /// Shared with the reader thread, which answers control requests this host
    /// does not handle without waiting for the UI.
    stdin: Arc<Mutex<ChildStdin>>,
    /// Permission requests not answered yet, as they arrived, by request id.
    /// The answer echoes the request's input, which the UI never sees whole.
    asks: Arc<Mutex<HashMap<String, Value>>>,
    /// The number of interrupts sent, to give each request its own id.
    interrupts: u32,
}

impl ClaudeCli {
    /// Start the CLI. `emit` is called from a reader thread for every event,
    /// and once with `Exited` when the process's output ends.
    pub fn spawn(program: &Path, cfg: &SessionConfig, emit: impl Fn(AgentEvent) + Send + 'static) -> anyhow::Result<Self> {
        let mut child = Command::new(program)
            .args(args(cfg))
            .current_dir(&cfg.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("could not start {}: {e}", program.display()))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("piped stdin")));
        let asks: Arc<Mutex<HashMap<String, Value>>> = Default::default();
        let (stdin2, asks2) = (stdin.clone(), asks.clone());
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let log = cfg.log.as_ref().and_then(|p| {
            std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
        });
        let log = std::sync::Arc::new(std::sync::Mutex::new(log));
        if let Some(f) = log.lock().unwrap().as_mut() {
            let _ = writeln!(f, "# {} {}", program.display(), args(cfg).join(" "));
        }
        let errbuf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let (errbuf2, log2) = (errbuf.clone(), log.clone());
        thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            if let Some(f) = log2.lock().unwrap().as_mut() {
                for l in s.lines() {
                    let _ = writeln!(f, "# stderr: {l}");
                }
            }
            *errbuf2.lock().unwrap() = s;
        });
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(f) = log.lock().unwrap().as_mut() {
                    let _ = writeln!(f, "{line}");
                }
                if line.contains("\"control_") {
                    keep_request(&line, &asks2, &stdin2, &log);
                }
                for ev in parse_line(&line) {
                    emit(ev);
                }
            }
            // Give the stderr reader a moment to finish so the reason is included.
            thread::sleep(std::time::Duration::from_millis(100));
            let err = errbuf.lock().unwrap().trim().to_string();
            let detail = if err.is_empty() { "the claude process exited".into() } else { err };
            emit(AgentEvent::Exited { detail });
        });
        Ok(Self { child, stdin, asks, interrupts: 0 })
    }
}

impl AgentSession for ClaudeCli {
    fn send(&mut self, text: &str) -> std::io::Result<()> {
        let msg = json!({ "type": "user", "message": { "role": "user", "content": text } });
        write_line(&self.stdin, &msg.to_string())
    }

    fn answer(&mut self, request_id: &str, decision: Decision) -> std::io::Result<bool> {
        let Some(request) = self.asks.lock().unwrap().remove(request_id) else { return Ok(false) };
        write_line(&self.stdin, &answer_line(request_id, &request, decision))?;
        Ok(true)
    }

    fn interrupt(&mut self) -> std::io::Result<()> {
        self.interrupts += 1;
        write_line(&self.stdin, &interrupt_line(&format!("because-interrupt-{}", self.interrupts)))
    }
}

fn write_line(stdin: &Mutex<ChildStdin>, line: &str) -> std::io::Result<()> {
    let mut w = stdin.lock().unwrap();
    writeln!(w, "{line}")?;
    w.flush()
}

type Log = Arc<Mutex<Option<std::fs::File>>>;

/// Keep a permission request until it is answered, drop one the agent
/// withdrew, and refuse any other control request on the spot.
fn keep_request(line: &str, asks: &Mutex<HashMap<String, Value>>, stdin: &Mutex<ChildStdin>, log: &Log) {
    let Ok(e) = serde_json::from_str::<Value>(line) else { return };
    let id = e["request_id"].as_str().unwrap_or_default().to_string();
    match e["type"].as_str() {
        Some("control_request") if e["request"]["subtype"] == "can_use_tool" => {
            asks.lock().unwrap().insert(id, e["request"].clone());
        }
        Some("control_request") => {
            let reply = unsupported_line(&id, e["request"]["subtype"].as_str().unwrap_or("?"));
            if let Some(f) = log.lock().unwrap().as_mut() {
                let _ = writeln!(f, "# sent: {reply}");
            }
            let _ = write_line(stdin, &reply);
        }
        Some("control_cancel_request") => {
            asks.lock().unwrap().remove(&id);
        }
        _ => {}
    }
}

impl Drop for ClaudeCli {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
