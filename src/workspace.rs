//! Workspace directory scaffold, templates, and read helpers.

use crate::store::{session_dir, SessionMeta};
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
        rel: "scripts/INDEX.md",
        content: scripts_content,
    },
    Scaffold {
        rel: "notes/INDEX.md",
        content: notes_content,
    },
    Scaffold {
        rel: "tasks/INDEX.md",
        content: tasks_content,
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

fn scripts_content(name: &str, _origin_pwd: &str) -> String {
    index_template(
        name,
        "scripts",
        "shared scripts",
        "`### <name>` then purpose / args / example",
    )
}

fn notes_content(name: &str, _origin_pwd: &str) -> String {
    index_template(
        name,
        "notes",
        "focused deep-dive articles",
        "`### <slug>` then a one-line gist",
    )
}

fn tasks_content(name: &str, _origin_pwd: &str) -> String {
    tasks_index_template(name)
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

/// Read the raw `tasks/INDEX.md` board. `None` if the file is absent (a
/// pre-tasks-model session, or a ghost with no workspace). Used by `csm detail`
/// (render the full board) and the hook snapshot.
pub fn read_tasks_index_md(name: &str) -> Option<String> {
    let path = session_dir(name).ok()?.join("tasks/INDEX.md");
    fs::read_to_string(path).ok()
}

/// Per-status task entries parsed from `tasks/INDEX.md`. The board is the source
/// of truth for what's claimable (Open / Pending fix) vs awaiting review
/// (Pending review) vs done. `csm show` lists the Open and Done entries (the
/// actionable and the accomplished) as a recognition aid; entries not listed in
/// INDEX are invisible here - keep INDEX current. Each entry is the text after
/// the leading `- ` (e.g. `001 fix-cookie-set - wire SameSite`); the card
/// formats it to `id slug`.
#[derive(Default)]
pub struct TasksBoard {
    pub open: Vec<String>,
    pub pending_review: Vec<String>,
    pub pending_fix: Vec<String>,
    pub done: Vec<String>,
}

/// Collect task entries per status from `tasks/INDEX.md` content. Layers on
/// [`crate::markdown::sections`] (the single `## Section` scanner shared with
/// `csm detail` / `csm show`) so the board and the readers agree on what a
/// section is. A task entry is a body line whose first non-space char is `-`
/// with non-empty content after it; comment-only lines are already dropped by
/// `sections`, and unknown sections are ignored (forward-compatible with
/// renamed statuses).
pub fn parse_tasks_board(content: &str) -> TasksBoard {
    let mut board = TasksBoard::default();
    for section in crate::markdown::sections(content) {
        let entries: &mut Vec<String> = match section.title.as_str() {
            "Open" => &mut board.open,
            "Pending review" => &mut board.pending_review,
            "Pending fix" => &mut board.pending_fix,
            "Done" => &mut board.done,
            _ => continue,
        };
        for line in section.body {
            if let Some(rest) = line.trim_start().strip_prefix('-') {
                let rest = rest.trim();
                if !rest.is_empty() {
                    entries.push(rest.to_string());
                }
            }
        }
    }
    board
}

/// Read and parse task entries from `tasks/INDEX.md` for a session. `None` if
/// the file is absent; `Some` of an empty board if it exists but has no entries
/// (a fresh scaffold). Used by `csm show` to list Open/Done entries.
pub fn read_tasks_board(name: &str) -> Option<TasksBoard> {
    read_tasks_index_md(name).map(|c| parse_tasks_board(&c))
}

/// First paragraph of the Context section in state.md (the lines under
/// `## Context` up to the first blank line), each with inline markdown stripped,
/// enough to recall what the session is about, not just the heading line.
/// Capped at `max_lines`. Falls back to a legacy `## Task` section if Context
/// is absent (pre-tasks-model sessions). Empty vec if neither section exists.
///
/// Layers on `markdown::sections` (the single `## Section` scanner shared with
/// `csm detail`) instead of re-scanning: find the Context (or legacy Task)
/// section, take its first paragraph, cap it. Each line is trimmed to match the
/// prior `strip_inline(line.trim())` behavior.
pub fn read_context_lines(name: &str, max_lines: usize) -> Vec<String> {
    let content = match read_state_md(name) {
        Some(c) => c,
        None => return Vec::new(),
    };
    let Some(section) = crate::markdown::sections(&content)
        .into_iter()
        .find(|s| s.title == "Context" || s.title == "Task")
    else {
        return Vec::new();
    };
    section
        .body
        .into_iter()
        .take_while(|l| !l.is_empty())
        .take(max_lines)
        .map(|l| l.trim().to_string())
        .collect()
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

## Context
<!-- What this session is + why + current focus. -->

## Key links
<!-- Repo / docs / related sessions / transcript. -->
"#
    )
}

fn index_template(name: &str, subdir: &str, description: &str, entry_format: &str) -> String {
    format!(
        r#"# {name} - {subdir} registry

> Registry of {description} under {subdir}/. Read this before writing a new one.
> Entry format: {entry_format}.

<!-- Add entries as you add {subdir}. -->
"#
    )
}

/// The task board. Sectioned by status (Open / Pending review / Pending fix /
/// Done) so an orienting agent reads only the actionable sections (Open and
/// Pending fix are worker-claimable; Pending review awaits the coordinator) and
/// skims Done. The header carries only the task-file pointer + status rule -
/// the section list and claim/submit/review flow live in the CLAUDE.md
/// prompt, and this file is injected verbatim on every launch (no
/// restatement).
fn tasks_index_template(name: &str) -> String {
    format!(
        r#"# {name} - tasks board

> Each task: tasks/<id>-<slug>.md. Status = section.

## Open
<!-- - 001 <slug> - <gist> -->

## Pending review

## Pending fix

## Done
"#
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_workspace, expected_files, parse_tasks_board, read_context_lines, read_tasks_board,
        read_tasks_index_md,
    };
    use crate::store::{session_dir, touch_session};
    use crate::test_support::with_csm_home;
    use serial_test::serial;
    use std::fs;
    use std::path::Path;

    /// Sum of entries across all statuses (test-only; prod has no use for a total).
    fn total(b: &super::TasksBoard) -> usize {
        b.open.len() + b.pending_review.len() + b.pending_fix.len() + b.done.len()
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
            assert!(session_dir("ws").unwrap().join("tasks/INDEX.md").exists());
        });
    }

    #[test]
    #[serial]
    fn read_context_lines_first_paragraph_capped() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            fs::write(
                session_dir("ws").unwrap().join("state.md"),
                "# ws - state\n\n> q\n\n## Context\nfirst context line.\nsecond context line.\n\n## SOP\ndone\n",
            )
            .unwrap();
            assert_eq!(
                read_context_lines("ws", 5),
                vec!["first context line.", "second context line."]
            );
            assert_eq!(read_context_lines("ws", 1), vec!["first context line."]);
        });
    }

    #[test]
    #[serial]
    fn read_context_lines_falls_back_to_legacy_task_section() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            // Legacy session: state.md has `## Task` (pre-tasks-model), no `## Context`.
            fs::write(
                session_dir("ws").unwrap().join("state.md"),
                "# ws - state\n\n## Task\nlegacy task line.\n",
            )
            .unwrap();
            assert_eq!(read_context_lines("ws", 5), vec!["legacy task line."]);
        });
    }

    #[test]
    #[serial]
    fn read_context_lines_empty_when_no_context_or_task() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            fs::write(
                session_dir("ws").unwrap().join("state.md"),
                "# ws - state\n\n## Other\nbody\n",
            )
            .unwrap();
            assert!(read_context_lines("ws", 5).is_empty());
        });
    }

    // --- tasks board ---

    #[test]
    fn parse_tasks_board_collects_entries_by_status() {
        let board = parse_tasks_board(
            "# ws - tasks board\n\n> q\n\n## Open\n- 001 refactor - slim state\n\
             - 002 prompt - review loop\n\n## Pending review\n- 003 doctor check\n\n\
             ## Pending fix\n\n## Done\n- 000 init - scaffold\n",
        );
        assert_eq!(board.open.len(), 2);
        assert_eq!(board.pending_review.len(), 1);
        assert_eq!(board.pending_fix.len(), 0);
        assert_eq!(board.done.len(), 1);
        assert_eq!(total(&board), 4);
        // Entry stored is the text after `- `.
        assert_eq!(board.open[0], "001 refactor - slim state");
        assert_eq!(board.done[0], "000 init - scaffold");
    }

    #[test]
    fn parse_tasks_board_drops_comments_and_unknown_sections() {
        // The scaffold template has a commented example under Open and empty
        // statuses - a fresh board should parse to all-empty.
        let fresh = "# ws - tasks board\n\n> Status board. ...\n\n## Open\n\
                     <!-- - 001 <slug> - <gist> -->\n\n## Pending review\n\n## Pending fix\n\n## Done\n";
        let board = parse_tasks_board(fresh);
        assert_eq!(total(&board), 0);
        // Unknown sections are ignored, not panic.
        let with_extra = "# ws\n\n## Open\n- 001 x\n\n## Archived\n- old\n\n## Done\n- 000 y\n";
        let board = parse_tasks_board(with_extra);
        assert_eq!(board.open.len(), 1);
        assert_eq!(board.done.len(), 1);
        assert_eq!(total(&board), 2);
    }

    #[test]
    fn parse_tasks_board_collects_bold_entries() {
        // sections() inline-strips, so a `**bold**` gist still counts as an entry.
        let board = parse_tasks_board("## Open\n- 001 **refactor** - slim\n");
        assert_eq!(board.open.len(), 1);
    }

    #[test]
    #[serial]
    fn read_tasks_index_md_none_when_no_workspace() {
        with_csm_home(|_dir| {
            touch_session("ws", "/o").unwrap();
            assert!(read_tasks_index_md("ws").is_none());
        });
    }

    #[test]
    #[serial]
    fn read_tasks_board_empty_for_fresh_scaffold() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            let board = read_tasks_board("ws").expect("tasks/INDEX.md is scaffolded");
            assert_eq!(total(&board), 0);
        });
    }

    #[test]
    #[serial]
    fn read_tasks_board_collects_real_entries() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            fs::write(
                session_dir("ws").unwrap().join("tasks/INDEX.md"),
                "# ws - tasks board\n\n## Open\n- 001 a\n\n## Pending review\n- 002 b\n\n\
                 ## Pending fix\n- 003 c\n\n## Done\n- 000 d\n",
            )
            .unwrap();
            let board = read_tasks_board("ws").expect("parsed");
            assert_eq!(board.open.len(), 1);
            assert_eq!(board.pending_review.len(), 1);
            assert_eq!(board.pending_fix.len(), 1);
            assert_eq!(board.done.len(), 1);
            assert_eq!(total(&board), 4);
        });
    }
}
