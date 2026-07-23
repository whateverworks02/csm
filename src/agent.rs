//! Per-agent adapter (strategy). Each impl normalizes three things for a
//! coding-agent CLI: launch, state-injection, and (eventually) trajectory
//! emission. The rest of csm talks only to `dyn Agent`, never a concrete
//! agent - so adding an agent is "implement this trait + add a match arm".
//!
//! Why `dyn` (not an `enum`): the agent is selected at runtime from `--agent`,
//! and the set is open (codex etc. may follow). The trait is object-safe -
//! `&self` methods only, no generics, no `Self` returns - so `Box<dyn Agent>`
//! works.
//!
//! Semantic note: Claude Code *separates* instructions (persistent `CLAUDE.md`)
//! from state data (event-injected by the SessionStart hook, revives on
//! `/clear`). pi *collapses* both into launch-time `--append-system-prompt` and
//! has no in-process revival (you resume a session file instead). The trait
//! exposes `inject_state` as "the string to inject now"; each agent decides
//! *when* it is delivered.

use crate::hook;
use crate::inject;
use crate::prompt;
use crate::store;
use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

pub trait Agent {
    /// Stable id ("claude", "pi"). Used by `--agent` and launch logs.
    fn id(&self) -> &'static str;

    /// Build the launch command for session `name`. Owns its args/env.
    fn launch(&self, name: &str, meta: &crate::store::SessionMeta) -> Command;

    /// Idempotent: wire state-injection into the agent's global config.
    /// Default no-op (pi has nothing global); Claude installs a hook + CLAUDE.md.
    fn install(&self) -> Result<()> {
        Ok(())
    }

    /// The state snapshot to inject *now*. The agent decides when this is
    /// actually delivered: Claude via its SessionStart hook at runtime; pi at
    /// launch via `--append-system-prompt`.
    fn inject_state(&self, name: &str, meta: &crate::store::SessionMeta) -> String;

    /// Where this agent's trajectory lives. Declared now; consumed by the
    /// capture step later.
    #[allow(dead_code)] // forward-declared seam; consumed by the capture step
    fn trajectory(&self, name: &str, meta: &crate::store::SessionMeta) -> TrajectorySource;
}

/// Where an agent's trajectory lives. Forward-declared seam for the capture
/// step (roadmap direction 1); not yet read by anything.
#[allow(dead_code)]
pub enum TrajectorySource {
    /// A transcript file the agent already writes (Claude's `.jsonl`).
    Transcript(PathBuf),
    /// Structured JSON on stdout when launched with a flag (stream-json / --mode json).
    StreamJson,
    /// Session files under a dir (pi `--session-dir`).
    SessionDir(PathBuf),
}

/// Pick an agent by id. The "context" in strategy terms: it holds the chosen
/// strategy and the rest of csm talks only to `dyn Agent`.
pub fn agent_for(id: &str) -> Result<Box<dyn Agent>> {
    match id {
        "claude" => Ok(Box::new(ClaudeAgent)),
        "pi" => Ok(Box::new(PiAgent)),
        other => anyhow::bail!("unknown agent {other:?} (expected: claude, pi)"),
    }
}

// --- Claude Code -----------------------------------------------------------

struct ClaudeAgent;

impl Agent for ClaudeAgent {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn launch(&self, name: &str, _meta: &crate::store::SessionMeta) -> Command {
        // `$CSM_SESSION` is the per-terminal binding the SessionStart hook reads
        // (hook.rs). On `/clear` it is still set (same process), so the hook
        // revives the workspace memory in place.
        let mut cmd = Command::new("claude");
        cmd.env("CSM_SESSION", name);
        cmd
    }

    fn install(&self) -> Result<()> {
        inject::install_claude()
    }

    fn inject_state(&self, name: &str, meta: &crate::store::SessionMeta) -> String {
        // Claude carries the working-mode prompt in its global CLAUDE.md, so the
        // hook injects only the state snapshot.
        hook::context_for_session(name, meta)
    }

    fn trajectory(&self, _name: &str, _meta: &crate::store::SessionMeta) -> TrajectorySource {
        // Claude writes ~/.claude/projects/<proj>/<uuid>.jsonl; the path arrives
        // in the hook stdin as `transcript_path`. Resolving it here is left for
        // the capture step.
        TrajectorySource::Transcript(PathBuf::new())
    }
}

// --- pi --------------------------------------------------------------------

struct PiAgent;

impl Agent for PiAgent {
    fn id(&self) -> &'static str {
        "pi"
    }

    fn launch(&self, name: &str, meta: &crate::store::SessionMeta) -> Command {
        // pi has no SessionStart hook and no global-instructions file, so the
        // working-mode prompt AND the state snapshot are injected at launch via
        // `--append-system-prompt`. The value is inline text (not an existing
        // file path), so pi treats it as literal text. Sessions are co-located
        // under the workspace to feed the later capture step.
        let ctx = self.inject_state(name, meta);
        let session_dir = pi_session_dir(name);
        let mut cmd = Command::new("pi");
        cmd.env("CSM_SESSION", name)
            .arg("--append-system-prompt")
            .arg(&ctx)
            .arg("--session-dir")
            .arg(&session_dir)
            .arg("--name")
            .arg(name);
        cmd
    }

    // install(): default no-op - pi has no global config to wire.

    fn inject_state(&self, name: &str, meta: &crate::store::SessionMeta) -> String {
        // pi has no persistent CLAUDE.md equivalent, so carry the working-mode
        // prompt alongside the state snapshot.
        format!(
            "{}\n\n---\n\n{}",
            prompt::PROMPT_BODY,
            hook::context_for_session(name, meta)
        )
    }

    fn trajectory(&self, name: &str, _meta: &crate::store::SessionMeta) -> TrajectorySource {
        TrajectorySource::SessionDir(pi_session_dir(name))
    }
}

/// Co-located pi session storage for `name`: `<workspace>/.pi-sessions`.
fn pi_session_dir(name: &str) -> PathBuf {
    store::session_dir(name)
        .map(|d| d.join(".pi-sessions"))
        .unwrap_or_else(|_| PathBuf::from(format!(".pi-sessions-{name}")))
}
