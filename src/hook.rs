//! SessionStart handler for Claude Code and Codex. Stdout is reserved for the JSON response.

use crate::store;
use crate::workspace;
use anyhow::Result;
use std::io::Write;

const STATE_CAP: usize = 6000;
const TASKS_CAP: usize = 4000;

pub fn run_hook() -> Result<()> {
    run_hook_to(&mut std::io::stdout())
}

fn run_hook_to<W: Write>(out: &mut W) -> Result<()> {
    let name = match std::env::var("CSM_SESSION") {
        Ok(n) if !n.is_empty() => n,
        _ => return Ok(()),
    };

    let meta = match store::touch_if_exists(&name)? {
        Some(m) => m,
        None => return Ok(()),
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

/// Render the session snapshot shared by the hook and launch-time injection.
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

fn read_capped(content: Option<String>, cap: usize, label: &str) -> String {
    let content = match content {
        Some(c) => c,
        None => return format!("({label} not found)"),
    };
    let Some((end, _)) = content.char_indices().nth(cap) else {
        return content;
    };
    let truncated = &content[..end];
    format!("{truncated}\n...({label} truncated; full file at the workspace directory)...")
}

fn read_state_capped(name: &str) -> String {
    read_capped(workspace::read_state_md(name), STATE_CAP, "state.md")
}

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

    fn seed_session(name: &str, state: &str) {
        crate::test_support::scaffold_session(name);
        let dir = store::session_dir(name).unwrap();
        fs::write(dir.join("state.md"), state).unwrap();
    }

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

    #[test]
    #[serial]
    fn build_context_renders_the_board_by_section() {
        with_csm_home(|_dir| {
            seed_session("ws", "state");
            crate::test_support::write_board(
                "ws",
                "## Open\n- 003 c\n- 004 d needs: 003\n\n\
                 ## Pending review\n- 005 e\n\n## Pending fix\n- 006 f\n\n\
                 ## Done\n- 002 b\n- 001 a\n",
            );
            let dir = store::session_dir("ws").unwrap();
            fs::write(dir.join("views/003-c.html"), "<html></html>").unwrap();

            let ctx = build_context("ws");
            let block = ctx.split("--- tasks/INDEX.md ---").nth(1).unwrap();
            assert_eq!(
                block,
                "\n## Open\n- 003 c\n- 004 d needs: 003\n\n## Pending review\n- 005 e\n\n\
                 ## Pending fix\n- 006 f\n\n## Done\n- 001 a\n- 002 b\n"
            );
            assert!(!block.contains(crate::ui::T_VIEW));
        });
    }

    #[test]
    #[serial]
    fn build_context_falls_back_when_files_missing() {
        with_csm_home(|_dir| {
            store::touch_session("ws", "/o").unwrap();
            std::fs::remove_dir_all(store::session_dir("ws").unwrap()).unwrap();
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
            let mut md = String::from("## Open\n");
            for i in 0..300 {
                md.push_str(&format!("- {i:03} {}\n", "t".repeat(40)));
            }
            fs::write(dir.join("tasks/INDEX.md"), &md).unwrap();
            let ctx = build_context("ws");
            assert!(ctx.contains(
                "...(tasks/INDEX.md truncated; full file at the workspace directory)..."
            ));
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
