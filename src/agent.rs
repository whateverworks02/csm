//! Per-agent adapter (strategy). Each impl normalizes two things for a
//! coding-agent CLI: launch, and (idempotent) global install. The rest of csm
//! talks only to `dyn Agent`, never a concrete agent - so adding an agent is
//! "implement this trait + add a match arm".
//!
//! Why `dyn` (not an `enum`): the agent is selected at runtime from `--agent`,
//! and the set is open (codex etc. may follow). The trait is object-safe -
//! `&self` methods only, no generics, no `Self` returns - so `Box<dyn Agent>`
//! works.
//!
//! Semantic note: both agents carry the working-mode prompt in a persistent
//! global file that they auto-discover - Claude in `~/.claude/CLAUDE.md`, pi in
//! `~/.pi/agent/CLAUDE.md` (both written by `csm init`). At runtime only the
//! per-session state snapshot is injected: Claude via its SessionStart hook
//! (revives on `/clear`); pi at launch via `--append-system-prompt` (no
//! in-process revival - resume the pi session instead).

use crate::hook;
use crate::inject;
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
    /// Default no-op; Claude installs a SessionStart hook + CLAUDE.md, pi
    /// installs CLAUDE.md.
    fn install(&self) -> Result<()> {
        Ok(())
    }
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

/// Install global wiring for every known agent. Idempotent. Used by `csm init`
/// so one command sets up every supported agent.
pub fn install_all() -> Result<()> {
    for id in ["claude", "pi"] {
        agent_for(id)?.install()?;
    }
    Ok(())
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
        // revives the workspace memory in place. The state snapshot itself is
        // emitted by the hook at runtime, not passed here.
        let mut cmd = Command::new("claude");
        cmd.env("CSM_SESSION", name);
        cmd
    }

    fn install(&self) -> Result<()> {
        inject::install_claude()
    }
}

// --- pi --------------------------------------------------------------------

struct PiAgent;

impl Agent for PiAgent {
    fn id(&self) -> &'static str {
        "pi"
    }

    fn launch(&self, name: &str, meta: &crate::store::SessionMeta) -> Command {
        // pi auto-discovers `~/.pi/agent/CLAUDE.md` (installed by `install()`)
        // for the working-mode prompt, so only the per-session state snapshot is
        // injected here, at launch, via `--append-system-prompt`. Sessions are
        // co-located under the workspace to feed the later capture step.
        let ctx = hook::context_for_session(name, meta);
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

    fn install(&self) -> Result<()> {
        inject::install_pi()
    }
}

/// Co-located pi session storage for `name`: `<workspace>/.pi-sessions`.
fn pi_session_dir(name: &str) -> PathBuf {
    store::session_dir(name)
        .map(|d| d.join(".pi-sessions"))
        .unwrap_or_else(|_| PathBuf::from(format!(".pi-sessions-{name}")))
}
