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
//! Semantic note: every agent carries the working-mode prompt in a persistent
//! global file that it auto-discovers - Claude in `~/.claude/CLAUDE.md`, pi in
//! `~/.pi/agent/CLAUDE.md`, codex in `~/.codex/AGENTS.md`, opencode in
//! `~/.config/opencode/AGENTS.md` (all written by `csm init`). At runtime only
//! the per-session state snapshot is injected: Claude and codex via a
//! SessionStart hook (revives on `/clear`; codex also on `compact`); pi at
//! launch via `--append-system-prompt`; opencode at launch via
//! `$OPENCODE_CONFIG_CONTENT` + an `instructions` path (no in-process revival
//! for either launch-injection agent - start a fresh `csm <name>` launch
//! instead).

use crate::hook;
use crate::inject;
use crate::skills;
use crate::store;
use crate::ui;
use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

pub trait Agent {
    /// Stable id ("claude", "pi", "codex"). Used by `--agent` and launch logs.
    fn id(&self) -> &'static str;

    /// Build the launch command for session `name`. Owns its args/env.
    fn launch(&self, name: &str) -> Command;

    /// Idempotent: wire state-injection into the agent's global config.
    /// Default no-op; Claude installs a SessionStart hook + CLAUDE.md, pi
    /// installs CLAUDE.md.
    fn install(&self) -> Result<()> {
        Ok(())
    }
}

/// Every agent csm supports, in canonical order. Single source of truth for
/// the agent set: `install_all` iterates it and `agent_for`'s error message
/// lists it. The `agent_for` match arms stay explicit (each dispatches to a
/// different struct), so adding an agent still means touching them.
const KNOWN_AGENTS: &[&str] = &["claude", "pi", "codex", "opencode"];

/// Pick an agent by id. The "context" in strategy terms: it holds the chosen
/// strategy and the rest of csm talks only to `dyn Agent`.
pub fn agent_for(id: &str) -> Result<Box<dyn Agent>> {
    match id {
        "claude" => Ok(Box::new(ClaudeAgent)),
        "pi" => Ok(Box::new(PiAgent)),
        "codex" => Ok(Box::new(CodexAgent)),
        "opencode" => Ok(Box::new(OpenCodeAgent)),
        other => anyhow::bail!(
            "unknown agent {other:?} (expected: {})",
            KNOWN_AGENTS.join(", ")
        ),
    }
}

/// Install global wiring for every known agent. Idempotent. Used by `csm init`
/// so one command sets up every supported agent. Also deploys the
/// vendor-neutral skill homes (`skills::install_vendor_neutral`) - the
/// human-readable copies, agent-independent so they deploy once.
pub fn install_all() -> Result<()> {
    for id in KNOWN_AGENTS {
        agent_for(id)?.install()?;
    }
    skills::install_vendor_neutral()?;
    Ok(())
}

// --- Claude Code -----------------------------------------------------------

struct ClaudeAgent;

impl Agent for ClaudeAgent {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn launch(&self, name: &str) -> Command {
        // `$CSM_SESSION` is the per-terminal binding the SessionStart hook reads
        // (hook.rs). On `/clear` it is still set (same process), so the hook
        // revives the workspace memory in place. The state snapshot itself is
        // emitted by the hook at runtime, not passed here.
        let mut cmd = Command::new("claude");
        cmd.env("CSM_SESSION", name);
        cmd
    }

    fn install(&self) -> Result<()> {
        inject::install_claude()?;
        // Claude reaches the csm skills as real skills (slash command +
        // auto-trigger) - no pointer line in its block.
        skills::install_claude()
    }
}

// --- pi --------------------------------------------------------------------

struct PiAgent;

impl Agent for PiAgent {
    fn id(&self) -> &'static str {
        "pi"
    }

    fn launch(&self, name: &str) -> Command {
        // pi auto-discovers `~/.pi/agent/CLAUDE.md` (installed by `install()`)
        // for the working-mode prompt, so only the per-session state snapshot is
        // injected here, at launch, via `--append-system-prompt`. Sessions are
        // co-located under the workspace to feed the later capture step.
        let ctx = hook::build_context(name);
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

// --- codex -----------------------------------------------------------------

struct CodexAgent;

impl Agent for CodexAgent {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn launch(&self, name: &str) -> Command {
        // Like Claude: `$CSM_SESSION` binds the session, and codex's
        // SessionStart hook (in `~/.codex/hooks.json`, installed by
        // `install()`) reads it to inject the state snapshot. codex fires
        // SessionStart on startup, resume, `/clear`, and compact - so the
        // workspace memory revives on all of them (broader than Claude, which
        // only revives on `/clear`). The state snapshot is emitted by the hook
        // at runtime, not passed here - so `csm hook` is reused unchanged.
        let mut cmd = Command::new("codex");
        cmd.env("CSM_SESSION", name);
        cmd
    }

    fn install(&self) -> Result<()> {
        inject::install_codex()
    }
}

// --- opencode ----------------------------------------------------------------

struct OpenCodeAgent;

impl Agent for OpenCodeAgent {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn launch(&self, name: &str) -> Command {
        // Like pi: launch-time injection. opencode has no hook envelope csm can
        // reuse, but it merges `$OPENCODE_CONFIG_CONTENT` (inline JSON) into
        // its config and loads the files listed under `instructions`, so the
        // per-session snapshot - same `build_context` as pi - is written into
        // the workspace and passed by path. Config is read once per launch, so
        // the snapshot is frozen for that session: start a fresh `csm <name>`
        // launch to re-inject, don't continue the opencode session. `$CSM_SESSION`
        // is still set (the working-mode prompt tells the agent to check it).
        // csm owns this env var for the launch - a pre-set value is overridden.
        let mut cmd = Command::new("opencode");
        cmd.env("CSM_SESSION", name);
        match write_context_snapshot(name) {
            Ok(path) => {
                cmd.env(
                    "OPENCODE_CONFIG_CONTENT",
                    serde_json::json!({ "instructions": [path.display().to_string()] }).to_string(),
                );
            }
            // Snapshot write failed (e.g. unwritable workspace). Launch anyway -
            // degraded (no injected state), but a broken csm shouldn't block
            // the agent entirely; `csm doctor` covers workspace health.
            Err(e) => ui::warn(&format!(
                "could not write the opencode context snapshot: {e}"
            )),
        }
        cmd
    }

    fn install(&self) -> Result<()> {
        inject::install_opencode()
    }
}

/// Write the `[csm]` snapshot for opencode into session `name`'s workspace and
/// return its path - the file `$OPENCODE_CONFIG_CONTENT`'s `instructions`
/// points at. Rewritten on every `csm <name> --agent opencode`; a dotfile so
/// it stays out of the orientation surface (the live `state.md` /
/// `tasks/INDEX.md` remain the files to maintain).
fn write_context_snapshot(name: &str) -> Result<PathBuf> {
    let path = store::session_dir(name)?.join(".opencode-context.md");
    std::fs::write(&path, hook::build_context(name))?;
    Ok(path)
}
