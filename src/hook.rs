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
const TASKS_CAP: usize = 4000;

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
/// (capped), and the `tasks/INDEX.md` board (capped and rendered - the board's
/// sections, each task's head and `needs:` tail, never the file's gist or
/// progress prose). This is the lean orientation memory: the prompt tells the
/// agent these two files are the orientation surface and that the `[csm]` block
/// is "only a snapshot of these", so the snapshot must include both. The agent
/// discovers the `scripts/`, `notes/` and `views/` corpora through the
/// filesystem and their INDEXes (the working-mode prompt points at them), so
/// they aren't injected here. Used by both the SessionStart hook (`run_hook`)
/// and the pi launch adapter.
pub(crate) fn build_context(name: &str) -> String {
    let dir = store::session_dir(name)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let state = read_state_capped(name);
    let tasks = read_tasks_capped(name);

    format!(
        "[csm] Active workspace memory session: \"{name}\".
Workspace directory: {dir}

--- state.md ---
{state}

--- tasks/INDEX.md ---
{tasks}"
    )
}

/// Cap `content` at `cap` chars (by char count), appending a truncation note
/// naming `label` (a file name) when it overflows. `None` content yields a
/// "(label not found)" placeholder. Shared by the state.md and tasks/INDEX.md
/// snapshots so the cap policy can't drift between them.
fn read_capped(content: Option<String>, cap: usize, label: &str) -> String {
    let content = match content {
        Some(c) => c,
        None => return format!("({label} not found)"),
    };
    if content.chars().count() <= cap {
        return content;
    }
    let truncated: String = content.chars().take(cap).collect();
    format!("{truncated}\n...({label} truncated; full file at the workspace directory)...")
}

fn read_state_capped(name: &str) -> String {
    read_capped(workspace::read_state_md(name), STATE_CAP, "state.md")
}

/// The board's snapshot: [`workspace::render_board`]'s rendering, capped. The
/// block carries the board's sections with one head line per task; the file
/// keeps the full lines, so the cap falls on the plan's heads rather than on a
/// task's outcome prose.
fn read_tasks_capped(name: &str) -> String {
    let rendered = workspace::read_tasks_board(name).map(|board| workspace::render_board(&board));
    read_capped(rendered, TASKS_CAP, "tasks/INDEX.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{with_csm_home, with_env, without_env};
    use serial_test::serial;
    use std::fs;

    /// Create a session with a workspace and the given state.md body
    /// (overwriting the scaffolded file).
    fn seed_session(name: &str, state: &str) {
        crate::test_support::scaffold_session(name);
        let dir = store::session_dir(name).unwrap();
        fs::write(dir.join("state.md"), state).unwrap();
    }

    /// Run the hook into a buffer and assert it injected nothing (no output).
    fn assert_no_inject() {
        let mut buf = Vec::new();
        run_hook_to(&mut buf).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    #[serial]
    fn build_context_renders_state_tasks() {
        with_csm_home(|_dir| {
            seed_session("ws", "TASK BODY");
            let ctx = build_context("ws");
            assert!(ctx.contains("[csm] Active workspace memory session: \"ws\"."));
            assert!(ctx.contains("--- state.md ---"));
            assert!(ctx.contains("TASK BODY"));
            assert!(ctx.contains("--- tasks/INDEX.md ---"));
        });
    }

    /// The board reaches the agent as a rendering, not as the file: the board's
    /// sections in board order, one head line per task with its `needs:` tail,
    /// Done in numeric id order - and none of the prose a line carries.
    #[test]
    #[serial]
    fn build_context_renders_the_board_by_section() {
        with_csm_home(|_dir| {
            seed_session("ws", "state");
            crate::test_support::write_board(
                "ws",
                "## Open\n- 003 c - a gist the block must not carry\n- 004 d needs: 003\n\n\
                 ## Pending review\n- 005 e - g\n\n## Pending fix\n- 006 f - g\n\n\
                 ## Done\n- 002 b - g\n- 001 a - g\n",
            );
            // A view on a live task: views are human-facing, so no marker
            // reaches the snapshot.
            let dir = store::session_dir("ws").unwrap();
            fs::write(dir.join("views/003-c.html"), "<html></html>").unwrap();

            let ctx = build_context("ws");
            let block = ctx.split("--- tasks/INDEX.md ---").nth(1).unwrap();
            assert_eq!(
                block,
                "\n## Open\n- 003 c\n- 004 d needs: 003\n\n## Pending review\n- 005 e\n\n\
                 ## Pending fix\n- 006 f\n\n## Done\n- 001 a\n- 002 b\n"
            );
            assert!(!block.contains("a gist the block must not carry"));
            assert!(!block.contains(crate::ui::T_VIEW));
        });
    }

    /// A board line whose gist spells the tail delimiter - backticked in the
    /// file, so inline stripping leaves a bare word - contributes its head
    /// alone: no gist or progress prose reaches the snapshot, and a real tail
    /// after such a gist still rides its line.
    #[test]
    #[serial]
    fn build_context_renders_heads_for_a_gist_that_spells_the_delimiter() {
        with_csm_home(|_dir| {
            seed_session("ws", "state");
            crate::test_support::write_board(
                "ws",
                "## Open\n\
                 - 003 c - a gist that spells the `needs:` delimiter and then keeps talking\n\
                 - 004 d - the `needs:` forest, and how it renders needs: 003\n",
            );
            let ctx = build_context("ws");
            let block = ctx.split("--- tasks/INDEX.md ---").nth(1).unwrap();
            assert_eq!(block, "\n## Open\n- 003 c\n- 004 d needs: 003\n");
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
            assert!(ctx.contains("(tasks/INDEX.md not found)"));
        });
    }

    #[test]
    #[serial]
    fn build_context_caps_oversized_state() {
        with_csm_home(|_dir| {
            let big = "a".repeat(STATE_CAP + 1);
            seed_session("ws", &big);
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
    fn build_context_caps_oversized_tasks() {
        with_csm_home(|_dir| {
            seed_session("ws", "state");
            let dir = store::session_dir("ws").unwrap();
            // A board whose *rendered* block overflows: 300 heads of 40 chars.
            let mut md = String::from("## Open\n");
            for i in 0..300 {
                md.push_str(&format!("- {i:03} {}\n", "t".repeat(40)));
            }
            fs::write(dir.join("tasks/INDEX.md"), &md).unwrap();
            let ctx = build_context("ws");
            assert!(ctx.contains(
                "...(tasks/INDEX.md truncated; full file at the workspace directory)..."
            ));
            // Exactly TASKS_CAP chars of the rendered block survive; the last
            // task is cut, so the cap falls on heads and not on prose.
            let rendered = workspace::render_board(&workspace::parse_tasks_board(&md));
            assert!(rendered.chars().count() > TASKS_CAP);
            let kept: String = rendered.chars().take(TASKS_CAP).collect();
            assert!(ctx.contains(&kept));
            assert!(!ctx.contains(&format!("- 299 {}", "t".repeat(40))));
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
            seed_session("ws", "TASK BODY");
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
                assert!(ctx.contains("--- tasks/INDEX.md ---"));
            });
        });
    }
}
