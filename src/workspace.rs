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

fn progress_content(name: &str, origin_pwd: &str) -> String {
    progress_md_template(name, origin_pwd)
}

fn scripts_content(name: &str, _origin_pwd: &str) -> String {
    index_template(name, "scripts", "shared scripts")
}

fn notes_content(name: &str, _origin_pwd: &str) -> String {
    index_template(name, "notes", "focused deep-dive articles")
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

/// Read the raw `tasks/INDEX.md` board. `None` if the file is absent (a
/// pre-tasks-model session, or a ghost with no workspace). Used by `csm detail`
/// (render the full board) and the hook snapshot.
pub fn read_tasks_index_md(name: &str) -> Option<String> {
    let path = session_dir(name).ok()?.join("tasks/INDEX.md");
    fs::read_to_string(path).ok()
}

/// Per-status task counts parsed from `tasks/INDEX.md`. The board is the source
/// of truth for what's claimable (Open / Pending fix) vs awaiting review
/// (Pending review) vs done. `csm show` renders these as a one-line status
/// breakdown; an entry not listed in INDEX is invisible here - keep INDEX
/// current.
#[derive(Default)]
pub struct TasksBoard {
    pub open: usize,
    pub pending_review: usize,
    pub pending_fix: usize,
    pub done: usize,
}

impl TasksBoard {
    /// Total task entries across all statuses.
    pub fn total(&self) -> usize {
        self.open + self.pending_review + self.pending_fix + self.done
    }
}

/// Count task entries per status in `tasks/INDEX.md` content. Layers on
/// [`crate::render::sections`] (the single `## Section` scanner shared with
/// `csm detail` / `csm show`) so the board and the readers agree on what a
/// section is. A task entry is a body line whose first non-space char is `-`
/// with non-empty content after it; comment-only lines are already dropped by
/// `sections`, and unknown sections are ignored (forward-compatible with
/// renamed statuses).
pub fn parse_tasks_board(content: &str) -> TasksBoard {
    let mut board = TasksBoard::default();
    for section in crate::render::sections(content) {
        let count: &mut usize = match section.title.as_str() {
            "Open" => &mut board.open,
            "Pending review" => &mut board.pending_review,
            "Pending fix" => &mut board.pending_fix,
            "Done" => &mut board.done,
            _ => continue,
        };
        for line in section.body {
            if let Some(rest) = line.trim_start().strip_prefix('-') {
                if !rest.trim().is_empty() {
                    *count += 1;
                }
            }
        }
    }
    board
}

/// Read and count tasks in `tasks/INDEX.md` for a session. `None` if the file
/// is absent; `Some` of an all-zero board if it exists but has no task entries
/// (a fresh scaffold). Used by `csm show` for the status-counts row.
pub fn read_tasks_board(name: &str) -> Option<TasksBoard> {
    read_tasks_index_md(name).map(|c| parse_tasks_board(&c))
}

/// First paragraph of the Context section in state.md (the lines under
/// `## Context` up to the first blank line), each with inline markdown stripped,
/// enough to recall what the session is about, not just the heading line.
/// Capped at `max_lines`. Falls back to a legacy `## Task` section if Context
/// is absent (pre-tasks-model sessions). Empty vec if neither section exists.
///
/// Layers on `render::sections` (the single `## Section` scanner shared with
/// `csm detail`) instead of re-scanning: find the Context (or legacy Task)
/// section, take its first paragraph, cap it. Each line is trimmed to match the
/// prior `strip_inline(line.trim())` behavior.
pub fn read_context_lines(name: &str, max_lines: usize) -> Vec<String> {
    let content = match read_state_md(name) {
        Some(c) => c,
        None => return Vec::new(),
    };
    let Some(section) = crate::render::sections(&content)
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

> Session one-pager. Not a log. Task detail in tasks/<id>-<slug>.md;
> cross-task events in progress.md. (Coordinator-owned.)

## Context
<!-- What this session is + why + current focus. -->

## Key links
<!-- Repo / docs / related sessions / transcript. -->
"#
    )
}

fn progress_md_template(name: &str, origin_pwd: &str) -> String {
    let ts = now_iso();
    format!(
        r#"# {name} - progress log

> Thin cross-task event log: dispatch / handoff / decisions / milestones.
> Per-task progress in the task file, NOT here. Coordinator-only, append-only.
> Entry: `## YYYY-MM-DD HH:MM - <agent> - <summary>` + 1-3 bullets.

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

/// The task board. Sectioned by status (Open / Pending review / Pending fix /
/// Done) so an orienting agent reads only the actionable sections (Open and
/// Pending fix are worker-claimable; Pending review awaits the coordinator) and
/// skims Done. No owner column - the coordinator doesn't track workers.
/// Coordinator-owned; updated on create / submit / review only.
fn tasks_index_template(name: &str) -> String {
    format!(
        r#"# {name} - tasks board

> Status board. Each task: tasks/<id>-<slug>.md (scope+AC+SOP+open-questions+
> progress+Review). Status = section. Worker claims Open/Pending fix, submits
> to Pending review; coordinator reviews -> Pending fix or Done.

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
        ensure_workspace, expected_files, parse_progress_header, parse_tasks_board,
        read_context_lines, read_last_activity, read_progress_tail, read_tasks_board,
        read_tasks_index_md,
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

    // --- tasks board ---

    #[test]
    fn parse_tasks_board_counts_entries_by_status() {
        let board = parse_tasks_board(
            "# ws - tasks board\n\n> q\n\n## Open\n- 001 refactor - slim state\n\
             - 002 prompt - review loop\n\n## Pending review\n- 003 doctor check\n\n\
             ## Pending fix\n\n## Done\n- 000 init - scaffold\n",
        );
        assert_eq!(board.open, 2);
        assert_eq!(board.pending_review, 1);
        assert_eq!(board.pending_fix, 0);
        assert_eq!(board.done, 1);
        assert_eq!(board.total(), 4);
    }

    #[test]
    fn parse_tasks_board_drops_comments_and_unknown_sections() {
        // The scaffold template has a commented example under Open and empty
        // statuses - a fresh board should parse to all-zero.
        let fresh = "# ws - tasks board\n\n> Status board. ...\n\n## Open\n\
                     <!-- - 001 <slug> - <gist> -->\n\n## Pending review\n\n## Pending fix\n\n## Done\n";
        let board = parse_tasks_board(fresh);
        assert_eq!(board.total(), 0);
        // Unknown sections are ignored, not panic.
        let with_extra = "# ws\n\n## Open\n- 001 x\n\n## Archived\n- old\n\n## Done\n- 000 y\n";
        let board = parse_tasks_board(with_extra);
        assert_eq!(board.open, 1);
        assert_eq!(board.done, 1);
        assert_eq!(board.total(), 2);
    }

    #[test]
    fn parse_tasks_board_counts_bold_entries() {
        // sections() inline-strips, so a `**bold**` gist still counts as an entry.
        let board = parse_tasks_board("## Open\n- 001 **refactor** - slim\n");
        assert_eq!(board.open, 1);
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
            assert_eq!(board.total(), 0);
        });
    }

    #[test]
    #[serial]
    fn read_tasks_board_counts_real_entries() {
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
            assert_eq!(board.open, 1);
            assert_eq!(board.pending_review, 1);
            assert_eq!(board.pending_fix, 1);
            assert_eq!(board.done, 1);
            assert_eq!(board.total(), 4);
        });
    }
}
