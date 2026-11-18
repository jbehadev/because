//! The agent host: the interface between `cuz` and a coding agent.
//!
//! `cuz` sees an agent only through `Agent` and `AgentSession`, and a summary
//! model only through `summary::Summarizer`. Each backend translates its own
//! protocol into `because_core::AgentEvent`. The first backend is `claude`, the
//! Claude Code CLI. See `docs/design/04-agent-host.md` §1.

use std::path::PathBuf;

use because_core::{AgentEvent, Decision, Usage};

pub mod claude;
pub mod summary;

/// What a session needs from `cuz`. The same for each backend.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub cwd: PathBuf,
    pub session_id: String,
    /// When a tool call needs approval. The values are the backend's own; a
    /// tool call the mode does not allow arrives as an
    /// `AgentEvent::PermissionRequest`, and waits for `AgentSession::answer`.
    pub permission_mode: String,
    pub model: Option<String>,
    /// Continue an existing session (after its process exited) instead of
    /// starting one with this id.
    pub resume: bool,
    /// Where to keep everything the agent prints, so a run that misbehaves
    /// can be diagnosed afterwards.
    pub log: Option<PathBuf>,
    /// Keep the conversation out of the agent's own history, for a throwaway
    /// one like `cuz live --check`'s — which would otherwise come back as an
    /// imported session.
    pub ephemeral: bool,
}

/// Receives the events of one session, from a thread of the backend's own.
pub type Emit = Box<dyn Fn(AgentEvent) + Send + 'static>;

/// A coding agent that `cuz` can host.
pub trait Agent: Send + Sync {
    /// A short name for messages, such as "claude".
    fn name(&self) -> &str;

    /// The agent's version, as it reports it.
    fn version(&self) -> &str;

    /// The command that a session runs, for `cuz live --check` and the log.
    fn describe(&self, cfg: &SessionConfig) -> String;

    /// Start a session, or resume one when `cfg.resume` is set. `emit` gets
    /// every event, and then `Exited` once when the session ends.
    fn start(&self, cfg: &SessionConfig, emit: Emit) -> anyhow::Result<Box<dyn AgentSession>>;

    /// Find what a resume of `cfg` needs, so that a session that cannot
    /// resume fails with a clear reason before it starts.
    fn check_resume(&self, _cfg: &SessionConfig) -> anyhow::Result<()> {
        Ok(())
    }

    /// How much of the account's allowance is left, and the raw reply for the
    /// log. `None` when the agent has no allowance to report. Blocks.
    fn allowance(&self) -> Option<anyhow::Result<(Usage, String)>> {
        None
    }
}

/// One running session of an `Agent`. Dropping it stops the agent.
pub trait AgentSession: Send {
    /// Send one input. The response arrives as events, and ends with
    /// `Finished`. An input sent while the agent works can join the input
    /// that runs; `InputTaken` tells which.
    fn send(&mut self, text: &str) -> std::io::Result<()>;

    /// Answer a permission request. `Ok(false)` when there is nothing to
    /// answer: the agent withdrew it, or it was answered already.
    fn answer(&mut self, request_id: &str, decision: Decision) -> std::io::Result<bool>;

    /// Stop the input that runs. Its `Finished` event arrives as usual.
    fn interrupt(&mut self) -> std::io::Result<()>;
}
