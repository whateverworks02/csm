//! Workspace directory scaffold, templates, and read helpers.

use crate::store::{now_iso, session_dir, SessionMeta};
use anyhow::Result;
use std::fs;

/// One scaffold artifact: a path relative to the session dir and a function
/// that renders its initial content (given the session name and origin pwd).
struct Scaffold {
    rel: &'static str,
    content: fn(&str, &str) -> String,
}

/// Single source of truth for the per-session scaffold. [`ensure_workspace`]
/// writes these; [`expected_files`] (used by `doctor`) lists their paths by
/// iterating the same list - so the writer and the checker cannot drift (a file
/// added here is both written and diagnosed; nothing to keep in sync by hand).
const SCAFFOLD: &[Scaffold] = &[
    Scaffold {
        rel: "state.md",
        content: state_content,
    },
    Scaffold {
        rel: "progress.md",
        content: progress_content,
    },
    Scaffold {
        rel: "scripts/INDEX.md",
        content: scripts_content,
    },
    Scaffold {
        rel: "notes/INDEX.md",
        content: notes_content,
    },
];

/// The scaffold paths (relative to the session dir), for `doctor`'s
/// incomplete-workspace check. Derived from [`SCAFFOLD`] so it can't drift.
pub fn expected_files() -> impl Iterator<Item = &'static str> {
    SCAFFOLD.iter().map(|s| s.rel)
}

fn state_content(name: &str, _origin_pwd: &str) -> String {
    state_md_template(name)
}

fn progress_content(name: &str, origin_pwd: &str) -> String {
    progress_md_template(name, origin_pwd)
}

fn scripts_content(name: &str, _origin_pwd: &str) -> String {
    index_template(name, "scripts", "shared scripts")
}

fn notes_content(name: &str, _origin_pwd: &str) -> String {
    index_template(name, "notes", "focused deep-dive articles")
}

/// Ensure the workspace for `name` exists with all scaffolding. Idempotent:
/// existing files are never overwritten.
pub fn ensure_workspace(name: &str, meta: &SessionMeta) -> Result<()> {
    let dir = session_dir(name)?;
    // Create the session directory first; the per-entry writes below need a
    // parent to exist (each entry's own parent is created on demand, but the
    // session dir itself is the parent of the top-level files).
    fs::create_dir_all(&dir)?;

    for s in SCAFFOLD {
        let path = dir.join(s.rel);
        if path.exists() {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, (s.content)(name, &meta.origin_pwd))?;
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

/// List filenames under `<session-dir>/<subdir>/` (excluding INDEX.md), sorted.
fn list_files_in(name: &str, subdir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let dir = match session_dir(name) {
        Ok(d) => d.join(subdir),
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

/// List script filenames under scripts/ (excluding INDEX.md), sorted.
pub fn list_scripts(name: &str) -> Vec<String> {
    list_files_in(name, "scripts")
}

/// List note filenames under notes/ (excluding INDEX.md), sorted. Filesystem-
/// based, so awareness (show card / hook snapshot) does not depend on INDEX.md
/// being kept current - an unregistered note file is still counted.
pub fn list_notes(name: &str) -> Vec<String> {
    list_files_in(name, "notes")
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

fn index_template(name: &str, subdir: &str, description: &str) -> String {
    format!(
        r#"# {name} - {subdir} registry

> Registry of {description} under {subdir}/. Read this before writing a new one.
> Entry format: `### <slug>` then a one-line gist.

<!-- Add entries as you add {subdir}. -->
"#
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_workspace, expected_files, parse_progress_header, read_last_activity,
        read_progress_tail, read_task_lines,
    };
    use crate::store::{session_dir, touch_session};
    use crate::test_support::with_csm_home;
    use serial_test::serial;
    use std::fs;
    use std::path::Path;

    #[test]
    fn three_parts_drops_agent() {
        let (ts, summary) = parse_progress_header("2026-07-30 15:04 - claude - did thing");
        assert_eq!(ts, "2026-07-30 15:04");
        assert_eq!(summary, "did thing");
    }

    #[test]
    fn two_parts_ts_and_summary() {
        let (ts, summary) = parse_progress_header("2026-07-30 15:04 - did thing");
        assert_eq!(ts, "2026-07-30 15:04");
        assert_eq!(summary, "did thing");
    }

    #[test]
    fn one_part_becomes_summary() {
        let (ts, summary) = parse_progress_header("only");
        assert_eq!(ts, "");
        assert_eq!(summary, "only");
    }

    #[test]
    fn empty_header() {
        let (ts, summary) = parse_progress_header("");
        assert_eq!(ts, "");
        assert_eq!(summary, "");
    }

    #[test]
    fn segments_are_trimmed() {
        let (ts, summary) = parse_progress_header("  ts  -  agent  -  summary  ");
        assert_eq!(ts, "ts");
        assert_eq!(summary, "summary");
    }

    #[test]
    fn extra_separators_kept_in_summary() {
        // splitn(3, " - "): only the first two splits count; the rest stays in
        // the summary verbatim.
        let (ts, summary) = parse_progress_header("ts - agent - a - b - c");
        assert_eq!(ts, "ts");
        assert_eq!(summary, "a - b - c");
    }

    // --- workspace integration (isolated $CSM_HOME) ---

    /// Recursively collect file paths relative to `dir`, sorted.
    fn rel_files(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                for sub in rel_files(&path) {
                    out.push(format!("{name}/{sub}"));
                }
            } else {
                out.push(name);
            }
        }
        out.sort();
        out
    }

    #[test]
    #[serial]
    fn ensure_workspace_scaffolds_exactly_expected_files() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            let mut expected: Vec<String> = expected_files().map(String::from).collect();
            expected.sort();
            assert_eq!(rel_files(&session_dir("ws").unwrap()), expected);
        });
    }

    #[test]
    #[serial]
    fn ensure_workspace_idempotent_never_overwrites() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            // User customizes state.md.
            let state_path = session_dir("ws").unwrap().join("state.md");
            fs::write(&state_path, "# my custom state\n").unwrap();
            // Re-run: must not overwrite existing files, but still creates any
            // missing ones.
            ensure_workspace("ws", &meta).unwrap();
            assert_eq!(
                fs::read_to_string(&state_path).unwrap(),
                "# my custom state\n"
            );
            assert!(session_dir("ws").unwrap().join("progress.md").exists());
        });
    }

    #[test]
    #[serial]
    fn read_progress_tail_returns_last_n_lines() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            fs::write(
                session_dir("ws").unwrap().join("progress.md"),
                "line1\nline2\nline3\nline4\n",
            )
            .unwrap();
            assert_eq!(read_progress_tail("ws", 2).unwrap(), "line3\nline4");
        });
    }

    #[test]
    #[serial]
    fn read_task_lines_first_paragraph_capped() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            fs::write(
                session_dir("ws").unwrap().join("state.md"),
                "# ws - state\n\n> q\n\n## Task\nfirst task line.\nsecond task line.\n\n## Progress\ndone\n",
            )
            .unwrap();
            assert_eq!(
                read_task_lines("ws", 5),
                vec!["first task line.", "second task line."]
            );
            assert_eq!(read_task_lines("ws", 1), vec!["first task line."]);
        });
    }

    #[test]
    #[serial]
    fn read_task_lines_empty_when_no_task_section() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            fs::write(
                session_dir("ws").unwrap().join("state.md"),
                "# ws - state\n\n## Other\nbody\n",
            )
            .unwrap();
            assert!(read_task_lines("ws", 5).is_empty());
        });
    }

    #[test]
    #[serial]
    fn read_last_activity_picks_newest_entry() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            fs::write(
                session_dir("ws").unwrap().join("progress.md"),
                "# ws - progress log\n\n> q\n\n## 2026-07-29 10:00 - csm - session created\n- init.\n\n## 2026-07-30 15:04 - claude - did thing\n- bullet\n",
            )
            .unwrap();
            let act = read_last_activity("ws").expect("some activity");
            assert_eq!(act.ts, "2026-07-30 15:04");
            assert_eq!(act.summary, "did thing");
        });
    }

    #[test]
    #[serial]
    fn read_last_activity_none_when_no_progress() {
        with_csm_home(|_dir| {
            // Session in the index but no workspace dir / progress.md.
            touch_session("ws", "/o").unwrap();
            assert!(read_last_activity("ws").is_none());
        });
    }
}
