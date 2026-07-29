//! Claude Code SessionStart hook handler.
//!
//! stdin  : { session_id, transcript_path, cwd, hook_event_name, source: "startup"|"resume"|"clear"|"compact" }
//! stdout : { "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": "..." } }
//!
//! When no active csm session is bound ($CSM_SESSION unset or unknown), we
//! exit 0 with no output - inject nothing. stdout must contain *only* the JSON
//! object, so all diagnostics go to stderr.

use crate::store;
use crate::workspace;
use anyhow::Result;
use serde_json::Value;
use std::io::Read;

const STATE_CAP: usize = 6000;

pub fn run_hook() -> Result<()> {
    // Best-effort stdin parse. Never fail the session on bad input.
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let source = serde_json::from_str::<Value>(&input)
        .ok()
        .and_then(|v| v.get("source").and_then(|s| s.as_str()).map(String::from))
        .unwrap_or_default();

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

    let ctx = build_context(&name, &meta.origin_pwd, &source, meta.pinned);
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
/// (capped), a `progress.md` tail, the scripts list, and the notes list.
pub(crate) fn build_context(name: &str, origin_pwd: &str, source: &str, pinned: bool) -> String {
    let dir = store::session_dir(name)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let state = read_state_capped(name);
    let progress = workspace::read_progress_tail(name, 40)
        .unwrap_or_else(|| "(progress.md not found)".to_string());
    let scripts = workspace::list_scripts(name);
    let notes = workspace::list_notes(name);
    let src = if source.is_empty() { "startup" } else { source };

    format!(
        "[csm] Active workspace memory session: \"{name}\" (started from `{origin_pwd}`, source={src}, pinned={pinned}).
Workspace directory: {dir}

--- state.md ---
{state}

--- progress.md (recent) ---
{progress}

--- scripts/ (see scripts/INDEX.md) ---
{scripts}

--- notes/ (see notes/INDEX.md) ---
{notes}",
        scripts = join_or_none(&scripts),
        notes = join_or_none(&notes),
    )
}

fn join_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "(none yet)".to_string()
    } else {
        items.join(", ")
    }
}

/// Build the snapshot for a session looked up by name (source defaults to
/// "startup"). Used by agents that inject at launch (pi) and anywhere the
/// hook's event `source` isn't available.
pub(crate) fn context_for_session(name: &str, meta: &store::SessionMeta) -> String {
    build_context(name, &meta.origin_pwd, "startup", meta.pinned)
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
