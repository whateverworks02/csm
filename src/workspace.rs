//! Workspace directory scaffold, templates, and read helpers.

use crate::store::{now_iso, session_dir, SessionMeta};
use anyhow::Result;
use std::fs;

/// Ensure the workspace for `name` exists with all scaffolding. Idempotent:
/// existing files are never overwritten.
pub fn ensure_workspace(name: &str, meta: &SessionMeta) -> Result<()> {
    let dir = session_dir(name)?;
    let scripts = dir.join("scripts");
    fs::create_dir_all(&scripts)?;

    let state_md = dir.join("state.md");
    if !state_md.exists() {
        fs::write(&state_md, state_md_template(name))?;
    }

    let progress_md = dir.join("progress.md");
    if !progress_md.exists() {
        fs::write(&progress_md, progress_md_template(name, &meta.origin_pwd))?;
    }

    let index_md = scripts.join("INDEX.md");
    if !index_md.exists() {
        fs::write(&index_md, scripts_index_template(name))?;
    }

    Ok(())
}

pub fn read_state_md(name: &str) -> Option<String> {
    let path = session_dir(name).ok()?.join("state.md");
    fs::read_to_string(path).ok()
}

/// Return the last `max_lines` lines of progress.md.
pub fn read_progress_tail(name: &str, max_lines: usize) -> Option<String> {
    let path = session_dir(name).ok()?.join("progress.md");
    let content = fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    Some(lines[start..].join("\n"))
}

/// List script filenames under scripts/ (excluding INDEX.md), sorted.
pub fn list_scripts(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let dir = match session_dir(name) {
        Ok(d) => d.join("scripts"),
        Err(_) => return out,
    };
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let fname = e.file_name().to_string_lossy().to_string();
            if fname == "INDEX.md" {
                continue;
            }
            out.push(fname);
        }
    }
    out.sort();
    out
}

fn state_md_template(name: &str) -> String {
    format!(
        r#"# {name} - state

> Source of truth for this task. Keep it concise. Move settled detail into progress.md.

## Task
<!-- What and why. -->

## Acceptance criteria (AC)
<!-- - [ ] ... -->

## SOP
<!-- The protocol / steps to follow. -->

## Progress
<!-- One short paragraph: current status. -->

## Key links
<!-- PRs / issues / commits / docs. -->

## Open questions
<!-- - ... -->
"#
    )
}

fn progress_md_template(name: &str, origin_pwd: &str) -> String {
    let ts = now_iso();
    format!(
        r#"# {name} - progress log

> Append-only. One entry per meaningful change. Newest at the bottom.
> Entry format: `## YYYY-MM-DD HH:MM - <agent> - <summary>`

## {ts} - csm - session created
- Workspace initialized at `{origin_pwd}`.
"#
    )
}

fn scripts_index_template(name: &str) -> String {
    format!(
        r#"# {name} - scripts registry

> Registry of shared scripts under scripts/. Read this before writing a new script.
> Entry format: `### <name>` then purpose / args / example.

<!-- Add entries as you add scripts. -->
"#
    )
}
