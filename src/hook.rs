//! Claude Code SessionStart hook handler.
//!
//! stdin  : Claude pipes a SessionStart event JSON; we don't parse it.
//! stdout : { "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": "..." } }
//!
//! When no active csm session is bound ($CSM_SESSION unset or unknown), we
//! exit 0 with no output - inject nothing. stdout must contain *only* the JSON
//! object, so all diagnostics go to stderr.

use crate::store;
use crate::workspace;
use anyhow::Result;

const STATE_CAP: usize = 6000;

pub fn run_hook() -> Result<()> {
    let name = match std::env::var("CSM_SESSION") {
        Ok(n) if !n.is_empty() => n,
        _ => return Ok(()), // no active session - inject nothing
    };

    // Self-heal the workspace and refresh last_access. Unknown sessions are
    // ignored - the hook must not create sessions for stray `$CSM_SESSION`.
    let meta = match store::touch_if_exists(&name)? {
        Some(m) => m,
        None => return Ok(()), // unknown session - inject nothing
    };
    workspace::ensure_workspace(&name, &meta)?;

    let ctx = build_context(&name);
    let out = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": ctx,
        }
    });
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

/// Build the `[csm]` state snapshot for a session: workspace path, `state.md`
/// (capped), and a `progress.md` tail - the lean orientation memory. The agent
/// discovers scripts/notes via the filesystem (`csm show` lists them; the
/// working-mode prompt points at the INDEXes), so they aren't injected here.
/// Used by both the SessionStart hook (`run_hook`) and the pi launch adapter.
pub(crate) fn build_context(name: &str) -> String {
    let dir = store::session_dir(name)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let state = read_state_capped(name);
    let progress = workspace::read_progress_tail(name, 40)
        .unwrap_or_else(|| "(progress.md not found)".to_string());

    format!(
        "[csm] Active workspace memory session: \"{name}\".
Workspace directory: {dir}

--- state.md ---
{state}

--- progress.md (recent) ---
{progress}"
    )
}

fn read_state_capped(name: &str) -> String {
    let state =
        workspace::read_state_md(name).unwrap_or_else(|| "(state.md not found)".to_string());
    if state.chars().count() <= STATE_CAP {
        return state;
    }
    let truncated: String = state.chars().take(STATE_CAP).collect();
    format!("{truncated}\n...(state.md truncated; full file at the workspace directory)...")
}
