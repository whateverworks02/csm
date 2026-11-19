//! Agent launch and global configuration.

use crate::hook;
use crate::inject;
use crate::store;
use crate::ui;
use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

pub trait Agent {
    fn id(&self) -> &'static str;

    fn launch(&self, name: &str) -> Command;

    /// Install global configuration. The default leaves it untouched.
    fn install(&self) -> Result<()> {
        Ok(())
    }
}

const KNOWN_AGENTS: &[&str] = &["claude", "pi", "codex", "opencode"];

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

pub fn install_all() -> Result<()> {
    for id in KNOWN_AGENTS {
        agent_for(id)?.install()?;
    }
    Ok(())
}

fn session_command(program: &str, name: &str) -> Command {
    let mut command = Command::new(program);
    command.env("CSM_SESSION", name);
    command
}

struct ClaudeAgent;

impl Agent for ClaudeAgent {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn launch(&self, name: &str) -> Command {
        session_command(self.id(), name)
    }

    fn install(&self) -> Result<()> {
        inject::install_claude()
    }
}

struct PiAgent;

impl Agent for PiAgent {
    fn id(&self) -> &'static str {
        "pi"
    }

    fn launch(&self, name: &str) -> Command {
        let ctx = hook::build_context(name);
        let session_dir = pi_session_dir(name);
        let mut cmd = session_command(self.id(), name);
        cmd.arg("--append-system-prompt")
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

fn pi_session_dir(name: &str) -> PathBuf {
    store::session_dir(name)
        .map(|d| d.join(".pi-sessions"))
        .unwrap_or_else(|_| PathBuf::from(format!(".pi-sessions-{name}")))
}

struct CodexAgent;

impl Agent for CodexAgent {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn launch(&self, name: &str) -> Command {
        session_command(self.id(), name)
    }

    fn install(&self) -> Result<()> {
        inject::install_codex()
    }
}

struct OpenCodeAgent;

impl Agent for OpenCodeAgent {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn launch(&self, name: &str) -> Command {
        // OpenCode loads the snapshot at launch from a file listed in its inline config.
        let mut cmd = session_command(self.id(), name);
        match write_context_snapshot(name) {
            Ok(path) => {
                cmd.env(
                    "OPENCODE_CONFIG_CONTENT",
                    serde_json::json!({ "instructions": [path.display().to_string()] }).to_string(),
                );
            }
            // A snapshot failure should still allow OpenCode to start.
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

fn write_context_snapshot(name: &str) -> Result<PathBuf> {
    let path = store::session_dir(name)?.join(".opencode-context.md");
    std::fs::write(&path, hook::build_context(name))?;
    Ok(path)
}
