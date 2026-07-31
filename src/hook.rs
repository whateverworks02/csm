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
use std::io::Write;

const STATE_CAP: usize = 6000;

pub fn run_hook() -> Result<()> {
    run_hook_to(&mut std::io::stdout())
}

/// SessionStart hook body, writing the JSON result to `out` instead of stdout
/// directly so tests can capture it. `run_hook` is the prod entry point
/// (stdout); tests pass a buffer to assert the no-session-no-inject and
/// emit-context paths without spawning a process.
fn run_hook_to<W: Write>(out: &mut W) -> Result<()> {
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
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": ctx,
        }
    });
    writeln!(out, "{}", serde_json::to_string(&payload)?)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{with_csm_home, with_env, without_env};
    use serial_test::serial;
    use std::fs;

    /// Create a session with a workspace and the given state.md / progress.md
    /// bodies (overwriting the scaffolded files).
    fn seed_session(name: &str, state: &str, progress: &str) {
        let meta = store::touch_session(name, "/o").unwrap();
        workspace::ensure_workspace(name, &meta).unwrap();
        let dir = store::session_dir(name).unwrap();
        fs::write(dir.join("state.md"), state).unwrap();
        fs::write(dir.join("progress.md"), progress).unwrap();
    }

    /// Run the hook into a buffer and assert it injected nothing (no output).
    fn assert_no_inject() {
        let mut buf = Vec::new();
        run_hook_to(&mut buf).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    #[serial]
    fn build_context_renders_state_and_progress() {
        with_csm_home(|_dir| {
            seed_session("ws", "TASK BODY", "P1\nP2\n");
            let ctx = build_context("ws");
            assert!(ctx.contains("[csm] Active workspace memory session: \"ws\"."));
            assert!(ctx.contains("--- state.md ---"));
            assert!(ctx.contains("TASK BODY"));
            assert!(ctx.contains("--- progress.md (recent) ---"));
            assert!(ctx.contains("P1"));
            assert!(ctx.contains("P2"));
        });
    }

    #[test]
    #[serial]
    fn build_context_falls_back_when_files_missing() {
        with_csm_home(|_dir| {
            // In the index but no workspace dir / files.
            store::touch_session("ws", "/o").unwrap();
            let ctx = build_context("ws");
            assert!(ctx.contains("(state.md not found)"));
            assert!(ctx.contains("(progress.md not found)"));
        });
    }

    #[test]
    #[serial]
    fn build_context_caps_oversized_state() {
        with_csm_home(|_dir| {
            let big = "a".repeat(STATE_CAP + 1);
            seed_session("ws", &big, "tail\n");
            let ctx = build_context("ws");
            assert!(
                ctx.contains("...(state.md truncated; full file at the workspace directory)...")
            );
            // Exactly STATE_CAP 'a's survive (contiguous); the +1th does not.
            assert!(ctx.contains(&"a".repeat(STATE_CAP)));
            assert!(!ctx.contains(&"a".repeat(STATE_CAP + 1)));
        });
    }

    #[test]
    #[serial]
    fn run_hook_no_inject_when_csm_session_unset() {
        with_csm_home(|_dir| {
            without_env("CSM_SESSION", assert_no_inject);
        });
    }

    #[test]
    #[serial]
    fn run_hook_no_inject_when_csm_session_empty() {
        with_csm_home(|_dir| {
            with_env("CSM_SESSION", "", assert_no_inject);
        });
    }

    #[test]
    #[serial]
    fn run_hook_no_inject_for_unknown_session() {
        with_csm_home(|_dir| {
            with_env("CSM_SESSION", "ghost", || {
                assert_no_inject();
                // Unknown sessions are not created.
                assert!(store::require_session("ghost").is_err());
                assert!(!store::session_dir("ghost").unwrap().exists());
            });
        });
    }

    #[test]
    #[serial]
    fn run_hook_emits_context_for_known_session() {
        with_csm_home(|_dir| {
            seed_session("ws", "TASK BODY", "P1\n");
            with_env("CSM_SESSION", "ws", || {
                let mut buf = Vec::new();
                run_hook_to(&mut buf).unwrap();
                let v: serde_json::Value =
                    serde_json::from_str(String::from_utf8(buf).unwrap().trim()).unwrap();
                assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
                let ctx = v["hookSpecificOutput"]["additionalContext"]
                    .as_str()
                    .unwrap();
                assert!(ctx.contains("\"ws\""));
                assert!(ctx.contains("TASK BODY"));
            });
        });
    }
}
