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

pub fn read_progress_md(name: &str) -> Option<String> {
    let path = session_dir(name).ok()?.join("progress.md");
    fs::read_to_string(path).ok()
}

/// Return the last `max_lines` lines of progress.md.
pub fn read_progress_tail(name: &str, max_lines: usize) -> Option<String> {
    let content = read_progress_md(name)?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    Some(lines[start..].join("\n"))
}

/// First paragraph of the Task section in state.md (the lines under `## Task`
/// up to the first blank line), each with inline markdown stripped - enough
/// substance to recall what the session is about, not just the heading line.
/// Capped at `max_lines`. Empty vec if no Task section.
///
/// Layers on `render::sections` (the single `## Section` scanner shared with
/// `csm detail`) instead of re-scanning: find the Task section, take its first
/// paragraph, cap it. Each line is trimmed to match the prior
/// `strip_inline(line.trim())` behavior.
pub fn read_task_lines(name: &str, max_lines: usize) -> Vec<String> {
    let content = match read_state_md(name) {
        Some(c) => c,
        None => return Vec::new(),
    };
    let Some(task) = crate::render::sections(&content)
        .into_iter()
        .find(|s| s.title == "Task")
    else {
        return Vec::new();
    };
    task.body
        .into_iter()
        .take_while(|l| !l.is_empty())
        .take(max_lines)
        .map(|l| l.trim().to_string())
        .collect()
}

/// Most recent progress entry, parsed into a one-line card gist.
/// `ts`/`summary` come from the last `## <ts> - <agent> - <summary>` header.
pub fn read_last_activity(name: &str) -> Option<LastActivity> {
    let content = read_progress_md(name)?;
    let lines: Vec<&str> = content.lines().collect();
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("## ") {
            start = Some(i);
        }
    }
    let start = start?;
    let header = lines[start]
        .strip_prefix("## ")
        .unwrap_or(lines[start])
        .trim();
    let (ts, summary) = parse_progress_header(header);
    Some(LastActivity { ts, summary })
}

/// A parsed progress entry header, for the `last` line of the show card.
pub struct LastActivity {
    pub ts: String,
    pub summary: String,
}

/// Split `<ts> - <agent> - <summary>` (or fewer parts) into (ts, summary).
/// The agent segment is dropped - the card shows when + what, not who.
fn parse_progress_header(header: &str) -> (String, String) {
    let parts: Vec<&str> = header.splitn(3, " - ").collect();
    match parts.as_slice() {
        [ts, _agent, summary] => (ts.trim().to_string(), summary.trim().to_string()),
        [ts, summary] => (ts.trim().to_string(), summary.trim().to_string()),
        [only] => (String::new(), only.trim().to_string()),
        _ => (String::new(), String::new()),
    }
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
