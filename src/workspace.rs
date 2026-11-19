use crate::store::{session_dir, SessionMeta};
use anyhow::Result;
use std::collections::HashSet;
use std::fs;

struct Scaffold {
    rel: &'static str,
    content: fn(&str) -> String,
}

const SCAFFOLD: &[Scaffold] = &[
    Scaffold {
        rel: "state.md",
        content: state_md_template,
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
        rel: "views/INDEX.md",
        content: views_content,
    },
    Scaffold {
        rel: "tasks/INDEX.md",
        content: tasks_index_template,
    },
];

pub fn expected_files() -> impl Iterator<Item = &'static str> {
    SCAFFOLD.iter().map(|s| s.rel)
}

fn scripts_content(name: &str) -> String {
    index_template(
        name,
        "scripts",
        "shared scripts",
        "`### <name>` then purpose / args / example",
    )
}

fn notes_content(name: &str) -> String {
    index_template(
        name,
        "notes",
        "focused deep-dive articles",
        "`### <slug>` then a one-line gist",
    )
}

fn views_content(name: &str) -> String {
    index_template(
        name,
        "views",
        "human-facing HTML task views",
        "`### <id>-<slug>.html` then a one-line gist",
    )
}

/// Create missing workspace directories and files, keeping existing content.
pub fn ensure_workspace(name: &str, _meta: &SessionMeta) -> Result<()> {
    let dir = session_dir(name)?;
    fs::create_dir_all(&dir)?;

    for s in SCAFFOLD {
        let path = dir.join(s.rel);
        if path.exists() {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, (s.content)(name))?;
    }
    Ok(())
}

pub fn read_state_md(name: &str) -> Option<String> {
    let path = session_dir(name).ok()?.join("state.md");
    fs::read_to_string(path).ok()
}

pub fn read_tasks_index_md(name: &str) -> Option<String> {
    let path = session_dir(name).ok()?.join("tasks/INDEX.md");
    fs::read_to_string(path).ok()
}

#[derive(Default)]
pub struct TasksBoard {
    pub open: Vec<String>,
    pub pending_review: Vec<String>,
    pub pending_fix: Vec<String>,
    pub done: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Open,
    PendingReview,
    PendingFix,
    Done,
}

impl Status {
    pub const ALL: [Status; 4] = [
        Status::Open,
        Status::PendingReview,
        Status::PendingFix,
        Status::Done,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Status::Open => "Open",
            Status::PendingReview => "Pending review",
            Status::PendingFix => "Pending fix",
            Status::Done => "Done",
        }
    }

    pub fn from_label(title: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|s| s.label() == title)
    }

    pub fn is_unfinished(self) -> bool {
        !matches!(self, Status::Done)
    }
}

impl TasksBoard {
    pub fn section(&self, status: Status) -> &[String] {
        match status {
            Status::Open => &self.open,
            Status::PendingReview => &self.pending_review,
            Status::PendingFix => &self.pending_fix,
            Status::Done => &self.done,
        }
    }

    fn section_mut(&mut self, status: Status) -> &mut Vec<String> {
        match status {
            Status::Open => &mut self.open,
            Status::PendingReview => &mut self.pending_review,
            Status::PendingFix => &mut self.pending_fix,
            Status::Done => &mut self.done,
        }
    }

    pub fn sections(&self) -> [(&[String], Status); 4] {
        Status::ALL.map(|status| (self.section(status), status))
    }

    pub fn entry_count(&self) -> usize {
        self.sections()
            .iter()
            .map(|(entries, _)| entries.len())
            .sum()
    }

    pub fn unfinished_count(&self) -> usize {
        self.sections()
            .iter()
            .filter(|(_, status)| status.is_unfinished())
            .map(|(entries, _)| entries.len())
            .sum()
    }

    pub fn status_of(&self, id: &str) -> Option<Status> {
        for (entries, status) in self.sections() {
            if entries.iter().any(|e| entry_id(e) == Some(id)) {
                return Some(status);
            }
        }
        None
    }
}

fn split_needs_tail(entry: &str) -> Option<(&str, &str)> {
    let at = entry.rfind("needs:")?;
    Some((&entry[..at], entry[at..].trim()))
}

pub fn entry_head(entry: &str) -> &str {
    split_needs_tail(entry)
        .map_or(entry, |(head, _)| head)
        .trim()
}

pub fn entry_id(entry: &str) -> Option<&str> {
    entry_head(entry).split_whitespace().next()
}

/// One `needs:` reference: `<task-id>` (same session) or `<session>/<task-id>`
/// (cross-session boundary). A trailing `[...]` is a delivery condition that
/// must be verified before the task counts as ready - never assumed met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedRef {
    pub session: Option<String>,
    pub task: String,
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskNeeds {
    pub refs: Vec<NeedRef>,
    pub unparseable: Option<String>,
}

/// Parse the `needs:` tail of a board entry. Grammar:
/// `needs: <ref>[ [condition]][, ...]` - refs comma separated, each carrying
/// at most one `[condition]`; commas inside a condition do not split. Any
/// chunk that fails the grammar makes the whole tail unparseable (surfaced,
/// not silently repaired).
pub fn parse_needs_tail(entry: &str) -> TaskNeeds {
    match split_needs_tail(entry) {
        Some((_, tail)) => parse_tail_text(tail),
        None => TaskNeeds::default(),
    }
}

fn parse_tail_text(tail: &str) -> TaskNeeds {
    let fail = || TaskNeeds {
        refs: Vec::new(),
        unparseable: Some(tail.to_string()),
    };
    let Some(rest) = tail.strip_prefix("needs:") else {
        return fail();
    };
    let refs_txt = rest.trim();
    if refs_txt.is_empty() {
        return fail();
    }
    // Split only outside brackets so conditions may contain commas.
    let mut chunks: Vec<&str> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in refs_txt.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                chunks.push(&refs_txt[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    chunks.push(&refs_txt[start..]);
    let mut refs = Vec::new();
    for chunk in chunks {
        match parse_ref(chunk) {
            Some(r) => refs.push(r),
            None => return fail(),
        }
    }
    TaskNeeds {
        refs,
        unparseable: None,
    }
}

fn parse_ref(chunk: &str) -> Option<NeedRef> {
    let chunk = chunk.trim();
    let (ref_part, condition) = match (chunk.find('['), chunk.rfind(']')) {
        (None, None) => (chunk, None),
        (Some(open), Some(close)) if open < close && close == chunk.len() - 1 => {
            let cond = chunk[open + 1..close].trim();
            if cond.is_empty() {
                return None;
            }
            (&chunk[..open], Some(cond.to_string()))
        }
        _ => return None,
    };
    let ref_part = ref_part.trim();
    if ref_part.is_empty() || ref_part.chars().any(char::is_whitespace) {
        return None;
    }
    let (session, task) = match ref_part.split_once('/') {
        None if is_token(ref_part) => (None, ref_part.to_string()),
        Some((s, t)) if is_session_component(s) && is_token(t) => {
            (Some(s.to_string()), t.to_string())
        }
        _ => return None,
    };
    Some(NeedRef {
        session,
        task,
        condition,
    })
}

/// Session names may include dots and Unicode. Keep the reference within one
/// directory component and exclude the needs-tail delimiters.
fn is_session_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '[' | ']' | ',')
        })
}

fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn entry_needs_tail(entry: &str) -> Option<&str> {
    let (_, tail) = split_needs_tail(entry)?;
    parse_tail_text(tail).unparseable.is_none().then_some(tail)
}

/// Sort nonnumeric IDs by text, then numeric IDs by value with text as a tie breaker.
pub fn entry_id_key(entry: &str) -> (Option<u64>, &str) {
    let id = entry_id(entry).unwrap_or("");
    (id.parse().ok(), id)
}

/// Render task heads and valid dependency tails for the session snapshot.
pub fn render_board(board: &TasksBoard) -> String {
    let mut out = String::new();
    for (entries, status) in board.sections() {
        if entries.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("## ");
        out.push_str(status.label());
        out.push('\n');
        let mut ordered: Vec<&str> = entries.iter().map(String::as_str).collect();
        if status == Status::Done {
            ordered.sort_by_key(|&entry| entry_id_key(entry));
        }
        for entry in ordered {
            out.push_str("- ");
            out.push_str(entry_head(entry));
            if let Some(tail) = entry_needs_tail(entry) {
                out.push(' ');
                out.push_str(tail);
            }
            out.push('\n');
        }
    }
    out
}

pub fn parse_tasks_board(content: &str) -> TasksBoard {
    let mut board = TasksBoard::default();
    for section in crate::markdown::sections(content) {
        let Some(status) = Status::from_label(&section.title) else {
            continue;
        };
        let entries = board.section_mut(status);
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

pub fn read_tasks_board(name: &str) -> Option<TasksBoard> {
    read_tasks_index_md(name).map(|c| parse_tasks_board(&c))
}

/// Read the first paragraph of the first `Context` or legacy `Task` section.
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

/// List visible HTML views in task-ID order.
pub fn list_views(name: &str) -> Vec<String> {
    let Ok(dir) = session_dir(name) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir.join("views")) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|fname| !fname.starts_with('.') && fname.ends_with(".html"))
        .collect();
    out.sort_by(|a, b| view_id_key(a).cmp(&view_id_key(b)));
    out
}

fn view_id_key(fname: &str) -> (Option<u64>, &str) {
    (view_task_id(fname).parse().ok(), fname)
}

/// The task id a view file belongs to: the `<id>` of `<id>-<slug>.html`. The
/// filename carries the binding, so the card can place a view on its task's
/// line without reading the task file or trusting a registry.
pub fn view_task_id(fname: &str) -> &str {
    fname.split_once('-').map_or(fname, |(id, _)| id)
}

pub fn view_task_ids(name: &str) -> HashSet<String> {
    list_views(name)
        .iter()
        .map(|fname| view_task_id(fname).to_string())
        .collect()
}

fn state_md_template(name: &str) -> String {
    format!(
        r#"# {name} - state

## Context
<!-- One short paragraph: what this session is + current focus. Rewrite in place when focus shifts; no history, no log. -->

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

fn tasks_index_template(name: &str) -> String {
    format!(
        r#"# {name} - tasks board

> Each task: tasks/<id>-<slug>.md. Status = section.

## Open
<!-- - 001 <slug> -->

## Pending review

## Pending fix

## Done
"#
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_workspace, expected_files, list_views, parse_tasks_board, read_context_lines,
        read_tasks_board, read_tasks_index_md, render_board,
    };
    use crate::store::{session_dir, touch_session};
    use crate::test_support::{with_csm_home, write_board};
    use serial_test::serial;
    use std::fs;
    use std::path::Path;

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
            let state_path = session_dir("ws").unwrap().join("state.md");
            fs::write(&state_path, "# my custom state\n").unwrap();
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
    fn views_registry_scaffolded() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            let content =
                fs::read_to_string(session_dir("ws").unwrap().join("views/INDEX.md")).unwrap();
            assert!(content.starts_with("# ws - views registry\n"));
            assert!(content.contains("`### <id>-<slug>.html` then a one-line gist"));
            assert!(list_views("ws").is_empty());
        });
    }

    #[test]
    #[serial]
    fn list_views_keeps_only_html_files_in_task_id_order() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            let dir = session_dir("ws").unwrap().join("views");
            for f in ["10-ten.html", "9-nine.html", "001-padded.html"] {
                fs::write(dir.join(f), "<!-- -->").unwrap();
            }
            fs::write(dir.join(".DS_Store"), "").unwrap();
            fs::write(dir.join(".draft.html"), "").unwrap();
            fs::write(dir.join("notes.md"), "").unwrap();
            fs::create_dir(dir.join("assets")).unwrap();
            assert_eq!(
                list_views("ws"),
                vec![
                    "001-padded.html".to_string(),
                    "9-nine.html".to_string(),
                    "10-ten.html".to_string(),
                ]
            );
        });
    }

    #[test]
    fn view_task_id_is_the_leading_id_token() {
        use super::view_task_id;
        assert_eq!(view_task_id("001-views.html"), "001");
        assert_eq!(view_task_id("10-ten-slug.html"), "10");
        assert_eq!(view_task_id("draft.html"), "draft.html");
    }

    #[test]
    #[serial]
    fn view_task_ids_bound_each_view_to_its_task() {
        use super::view_task_ids;
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            let dir = session_dir("ws").unwrap().join("views");
            for f in ["001-views.html", "9-nine.html", "draft.html"] {
                fs::write(dir.join(f), "<!-- -->").unwrap();
            }
            assert_eq!(
                view_task_ids("ws"),
                ["001".to_string(), "9".to_string(), "draft.html".to_string()]
                    .into_iter()
                    .collect::<std::collections::HashSet<_>>()
            );
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
    fn parse_tasks_board_collects_entries_by_status() {
        let board = parse_tasks_board(
            "# ws - tasks board\n\n> q\n\n## Open\n- 001 refactor\n\
             - 002 prompt\n\n## Pending review\n- 003 doctor check\n\n\
             ## Pending fix\n\n## Done\n- 000 init\n",
        );
        assert_eq!(board.open.len(), 2);
        assert_eq!(board.pending_review.len(), 1);
        assert_eq!(board.pending_fix.len(), 0);
        assert_eq!(board.done.len(), 1);
        assert_eq!(board.entry_count(), 4);
        assert_eq!(board.open[0], "001 refactor");
        assert_eq!(board.done[0], "000 init");
    }

    #[test]
    fn parse_tasks_board_drops_comments_and_unknown_sections() {
        let board = parse_tasks_board(&super::tasks_index_template("ws"));
        assert_eq!(board.entry_count(), 0);
        let with_extra = "# ws\n\n## Open\n- 001 x\n\n## Archived\n- old\n\n## Done\n- 000 y\n";
        let board = parse_tasks_board(with_extra);
        assert_eq!(board.open.len(), 1);
        assert_eq!(board.done.len(), 1);
        assert_eq!(board.entry_count(), 2);
    }

    #[test]
    fn parse_tasks_board_tolerates_needs_tail() {
        let board = parse_tasks_board(
            "## Open\n\
             - 004 integration needs: api-contract/012 [interface PR merged]\n\
             - 005 docs needs: 002, 003\n\n\
             ## Done\n- 002 api\n",
        );
        assert_eq!(board.open.len(), 2);
        assert_eq!(board.done.len(), 1);
        assert_eq!(board.entry_count(), 3);
        assert_eq!(
            board.open[0],
            "004 integration needs: api-contract/012 [interface PR merged]"
        );
        assert_eq!(board.open[1], "005 docs needs: 002, 003");
    }

    #[test]
    fn parse_tasks_board_collects_bold_entries() {
        let board = parse_tasks_board("## Open\n- 001 **refactor**\n");
        assert_eq!(board.open.len(), 1);
    }

    #[test]
    fn entry_head_takes_the_text_before_the_needs_tail() {
        use super::entry_head;
        assert_eq!(
            entry_head("003 doctor check needs: 001"),
            "003 doctor check"
        );
        assert_eq!(entry_head("004 e2e needs: 002, 003 [gate]"), "004 e2e");
        assert_eq!(entry_head("005 bare"), "005 bare");
    }

    #[test]
    fn parse_needs_tail_grammar() {
        use super::{parse_needs_tail, NeedRef, TaskNeeds};
        assert_eq!(parse_needs_tail("001 x"), TaskNeeds::default());

        let t = parse_needs_tail("002 api needs: 001");
        assert_eq!(t.unparseable, None);
        assert_eq!(
            t.refs,
            vec![NeedRef {
                session: None,
                task: "001".into(),
                condition: None
            }]
        );

        let t = parse_needs_tail("004 e2e needs: api-contract/012 [interface PR merged], 003");
        assert_eq!(t.refs.len(), 2);
        assert_eq!(t.refs[0].session.as_deref(), Some("api-contract"));
        assert_eq!(t.refs[0].task, "012");
        assert_eq!(t.refs[0].condition.as_deref(), Some("interface PR merged"));
        assert_eq!(t.refs[1].task, "003");
        assert_eq!(t.refs[1].condition, None);

        let t = parse_needs_tail("005 x needs: 001 [a, b]");
        assert_eq!(t.refs.len(), 1);
        assert_eq!(t.refs[0].condition.as_deref(), Some("a, b"));
    }

    #[test]
    fn parse_needs_tail_rejects_bad_grammar() {
        use super::parse_needs_tail;
        for bad in [
            "001 x needs:",
            "001 x needs: 002 003",
            "001 x needs: ???",
            "001 x needs: 002 [unclosed",
            "001 x needs: 002 []",
            "001 x needs: 002 trailing",
            "001 x needs: a//b",
            "001 x needs: /001",
            "001 x needs: ./001",
            "001 x needs: ../001",
            "001 x needs: api/../001",
            "001 x needs: api\\v2/001",
            "001 x needs: 002, ",
        ] {
            let t = parse_needs_tail(bad);
            assert!(t.unparseable.is_some(), "should be unparseable: {bad}");
            assert!(t.refs.is_empty(), "no refs from {bad}");
        }
    }

    #[test]
    fn entry_needs_tail_rides_the_line_only_when_it_parses() {
        use super::entry_needs_tail;
        assert_eq!(
            entry_needs_tail("004 e2e needs: 002, 003"),
            Some("needs: 002, 003")
        );
        assert_eq!(entry_needs_tail("012 bad needs: ???"), None);
        assert_eq!(entry_needs_tail("005 bare"), None);
    }

    #[test]
    fn entry_needs_tail_takes_the_line_s_last_occurrence() {
        use super::entry_needs_tail;
        assert_eq!(
            entry_needs_tail("010 graph - document the needs: forest needs: 001"),
            Some("needs: 001")
        );
    }

    #[test]
    fn unfinished_count_sums_every_live_section() {
        let board = parse_tasks_board(
            "## Open\n- 001 a\n\n## Pending review\n- 002 b\n\n\
             ## Pending fix\n- 003 c\n- 004 d\n\n## Done\n- 000 e\n",
        );
        assert_eq!(board.unfinished_count(), 4);
        assert_eq!(
            parse_tasks_board("## Done\n- 000 e\n").unfinished_count(),
            0
        );
        assert_eq!(parse_tasks_board("## Open\n").unfinished_count(), 0);
    }

    #[test]
    fn status_of_finds_entries_across_sections() {
        use super::Status;
        let board = parse_tasks_board("## Open\n- 001 a needs: 002\n\n## Done\n- 002 b\n");
        assert_eq!(board.status_of("001"), Some(Status::Open));
        assert_eq!(board.status_of("002"), Some(Status::Done));
        assert_eq!(board.status_of("003"), None);
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
            assert_eq!(board.entry_count(), 0);
        });
    }

    #[test]
    #[serial]
    fn read_tasks_board_collects_real_entries() {
        with_csm_home(|_dir| {
            let meta = touch_session("ws", "/o").unwrap();
            ensure_workspace("ws", &meta).unwrap();
            write_board(
                "ws",
                "## Open\n- 001 a\n\n## Pending review\n- 002 b\n\n## Pending fix\n- 003 c\n\n\
                 ## Done\n- 000 d\n",
            );
            let board = read_tasks_board("ws").expect("parsed");
            assert_eq!(board.open.len(), 1);
            assert_eq!(board.pending_review.len(), 1);
            assert_eq!(board.pending_fix.len(), 1);
            assert_eq!(board.done.len(), 1);
            assert_eq!(board.entry_count(), 4);
        });
    }

    #[test]
    fn render_board_heads_by_section_in_board_order() {
        let board = parse_tasks_board(
            "## Open\n- 003 c\n- 004 d needs: 003\n\n\
             ## Pending review\n- 005 e\n\n## Done\n- 002 b\n- 001 a\n",
        );
        assert_eq!(
            render_board(&board),
            "## Open\n- 003 c\n- 004 d needs: 003\n\n\
             ## Pending review\n- 005 e\n\n## Done\n- 001 a\n- 002 b\n"
        );
    }

    #[test]
    fn render_board_keeps_the_tail_verbatim_and_no_view_marker() {
        let board = parse_tasks_board(
            "## Open\n- 004 e2e needs: api-contract/012 [interface PR merged], 003\n",
        );
        let rendered = render_board(&board);
        assert_eq!(
            rendered,
            "## Open\n- 004 e2e needs: api-contract/012 [interface PR merged], 003\n"
        );
        assert!(!rendered.contains(crate::ui::T_VIEW));
    }

    #[test]
    fn render_board_drops_a_tail_the_grammar_rejects() {
        let board = parse_tasks_board("## Open\n- 012 bad needs: ???\n");
        assert_eq!(render_board(&board), "## Open\n- 012 bad\n");
    }

    #[test]
    fn render_board_omits_empty_sections() {
        assert_eq!(render_board(&parse_tasks_board("## Open\n\n## Done\n")), "");
        assert_eq!(
            render_board(&parse_tasks_board(
                "## Open\n- 001 a\n\n## Pending review\n"
            )),
            "## Open\n- 001 a\n"
        );
    }
}
