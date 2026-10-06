//! Dependency view for `csm show`: render the board's `needs:` tails (task
//! 004) for the user. The tails are the single dependency mapping; this module
//! reads them straight from the parsed board - no second graph file, no
//! cache - so the picture cannot drift from the plan the agents read.
//!
//! The view is one line per live (non-Done) task that carries refs:
//! `004 e2e (Open) needs 002 api (Open), 003 frontend (Done)` - plus the
//! ready set (claimable tasks whose needs all hold). The line is the wait
//! reason: an unmet ref shows its real status, a `[condition]` is never
//! assumed met, and anomalies are marked at the relation they break
//! (`(missing)`, `(unknown)`, `needs itself`, an unparseable tail) - never
//! rendered as satisfied. A dependency cycle gets one explicit `warning:`
//! line. Sessions with no live `needs:` tail keep the bare card.

use crate::ui;
use crate::workspace::{entry_head, entry_id, Status, TasksBoard};
use std::collections::HashMap;

// --- needs-tail parsing (the board's dependency mapping, task 004) ----------

/// One `needs:` reference: `<task-id>` (same session) or `<session>/<task-id>`
/// (cross-session boundary). A trailing `[...]` is a delivery condition that
/// must be verified before the task counts as ready - never assumed met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedRef {
    pub session: Option<String>,
    pub task: String,
    pub condition: Option<String>,
}

/// Parsed `needs:` tail of one board entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskNeeds {
    pub refs: Vec<NeedRef>,
    /// The raw tail when it fails the grammar. Surfaced for checking; never
    /// treated as "no dependencies" (which would mark the task ready).
    pub unparseable: Option<String>,
}

/// Parse the `needs:` tail of a board entry. Per protocol the tail ends the
/// line, so the last `needs:` occurrence is the tail. Grammar:
/// `needs: <ref>[ [condition]][, ...]` - refs comma separated, each carrying
/// at most one `[condition]`; commas inside a condition do not split. Any
/// chunk that fails the grammar makes the whole tail unparseable (surfaced,
/// not silently repaired).
pub fn parse_needs_tail(entry: &str) -> TaskNeeds {
    let Some(pos) = entry.rfind("needs:") else {
        return TaskNeeds::default();
    };
    let tail = entry[pos..].trim();
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
    // Split on commas at bracket depth 0 so a condition may contain commas.
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

/// Parse one comma-separated chunk: `<ref>` or `<ref> [condition]`.
fn parse_ref(chunk: &str) -> Option<NeedRef> {
    let chunk = chunk.trim();
    let (ref_part, condition) = match (chunk.find('['), chunk.rfind(']')) {
        (None, None) => (chunk, None),
        // The condition bracket must close the chunk; text after `]` fails.
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

/// Task-id token: ascii alphanumerics, dashes, underscores.
fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// --- view building -----------------------------------------------------------

/// One board entry with its tail parsed and status attached.
struct Entry {
    id: String,
    label: String,
    status: Status,
    needs: TaskNeeds,
}

fn collect_entries(board: &TasksBoard) -> Vec<Entry> {
    let mut out = Vec::new();
    for (raws, status) in board.sections() {
        for raw in raws {
            let Some(id) = entry_id(raw) else {
                continue; // entries without an id cannot be referenced
            };
            out.push(Entry {
                id: id.to_string(),
                label: entry_head(raw).to_string(),
                status,
                needs: parse_needs_tail(raw),
            });
        }
    }
    out
}

/// The rendered `needs` block of the `csm show` card.
pub struct DepView {
    /// One line per live task carrying refs, plus a cycle warning when the
    /// tails loop. Every edge and condition stays spelled out.
    pub needs_lines: Vec<String>,
    /// Comma-joined ids of claimable tasks (Open / Pending fix) whose needs
    /// all hold; empty when nothing is claimable right now.
    pub ready_line: String,
}

/// Build the dependency view for one session's board. `boundary_status`
/// resolves a cross-session ref against the target session's own board
/// (`None` = session or task not found, shown as `(unknown)`). Returns `None`
/// when no live entry carries a `needs:` tail - dependency-free sessions keep
/// the bare card. Read-only over the board: nothing here writes back.
pub fn build_view(
    board: &TasksBoard,
    session: &str,
    boundary_status: &mut dyn FnMut(&str, &str) -> Option<Status>,
) -> Option<DepView> {
    let entries = collect_entries(board);
    let by_id: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i))
        .collect();
    let has_live_needs = entries.iter().any(|e| {
        e.status.is_unfinished() && (!e.needs.refs.is_empty() || e.needs.unparseable.is_some())
    });
    if !has_live_needs {
        return None;
    }

    // One pass: each live task with refs yields its needs line and its
    // readiness; the line is the wait reason, so nothing is rendered twice.
    let mut needs_lines = Vec::new();
    let mut ready: Vec<&str> = Vec::new();
    for e in &entries {
        if !e.status.is_unfinished() {
            continue;
        }
        let claimable = matches!(e.status, Status::Open | Status::PendingFix);
        if let Some(raw) = &e.needs.unparseable {
            needs_lines.push(format!(
                "{} ({}) needs check: unparseable tail ({})",
                e.label,
                e.status.label(),
                raw
            ));
            continue;
        }
        if e.needs.refs.is_empty() {
            if claimable {
                ready.push(&e.id);
            }
            continue;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut all_met = true;
        for r in &e.needs.refs {
            let rv = ref_view(r, e, session, &entries, &by_id, boundary_status);
            all_met &= rv.satisfied;
            parts.push(rv.display);
        }
        if claimable && all_met {
            ready.push(&e.id);
        }
        // Leave the full relation intact; the terminal can wrap long lines.
        needs_lines.push(format!(
            "{} ({}) needs {}",
            e.label,
            e.status.label(),
            parts.join(", ")
        ));
    }

    if let Some(cycle) = find_cycle(&live_adjacency(&entries, &by_id, session)) {
        let names: Vec<&str> = cycle.iter().map(|&i| entries[i].label.as_str()).collect();
        needs_lines.push(format!(
            "{} cycle in needs: {}",
            ui::paint(ui::YELLOW, "warning:"),
            names.join(" -> ")
        ));
    }

    Some(DepView {
        needs_lines,
        ready_line: ready.join(", "),
    })
}

/// One ref rendered for a needs line, with whether it counts as met.
struct RefView {
    display: String,
    satisfied: bool,
}

/// Render one ref as `name (status)` with its `[condition]` when unmet. A ref
/// is met only when it resolves on its target board, is Done there, and
/// carries no condition - a condition is never assumed verified. Anomalies
/// are named inline: `itself`, `(missing)`, `(unknown)`.
fn ref_view(
    r: &NeedRef,
    consumer: &Entry,
    session: &str,
    entries: &[Entry],
    by_id: &HashMap<&str, usize>,
    boundary_status: &mut dyn FnMut(&str, &str) -> Option<Status>,
) -> RefView {
    // A `session/task` ref naming this session itself is local.
    let local = r.session.as_deref().is_none_or(|s| s == session);
    if local && r.task == consumer.id {
        return RefView {
            display: "itself".to_string(),
            satisfied: false,
        };
    }
    let cond = match &r.condition {
        Some(c) => format!(" [{c}]"),
        None => String::new(),
    };
    let (name, status, absent) = if local {
        match by_id.get(r.task.as_str()) {
            Some(&i) => (entries[i].label.clone(), Some(entries[i].status), None),
            None => (r.task.clone(), None, Some("missing")),
        }
    } else {
        let s = r.session.as_deref().unwrap();
        (
            format!("{s}/{}", r.task),
            boundary_status(s, &r.task),
            Some("unknown"),
        )
    };
    let display = match status {
        Some(Status::Done) if r.condition.is_none() => {
            return RefView {
                display: format!("{name} (Done)"),
                satisfied: true,
            };
        }
        Some(Status::Done) => format!("{name} (Done){cond} (unverified)"),
        Some(s) => format!("{name} ({}){cond}", s.label()),
        None => format!("{name} ({}){cond}", absent.unwrap()),
    };
    RefView {
        display,
        satisfied: false,
    }
}

/// Local edges among live entries, for cycle detection. Self-loops are
/// excluded (the line already says `needs itself`); missing refs have no
/// target. Done entries take no part: their tails are history, and a loop
/// through a Done task blocks nothing.
fn live_adjacency(
    entries: &[Entry],
    by_id: &HashMap<&str, usize>,
    session: &str,
) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); entries.len()];
    for (i, e) in entries.iter().enumerate() {
        if e.status == Status::Done {
            continue;
        }
        for r in &e.needs.refs {
            if r.session.as_deref().is_some_and(|s| s != session) {
                continue; // cross-session boundary: no local edge
            }
            let Some(&t) = by_id.get(r.task.as_str()) else {
                continue;
            };
            if t == i || entries[t].status == Status::Done {
                continue;
            }
            adj[i].push(t);
        }
    }
    adj
}

/// First cycle found by DFS, as a closed path `a -> b -> a`; `None` if acyclic.
fn find_cycle(adj: &[Vec<usize>]) -> Option<Vec<usize>> {
    #[derive(Clone, Copy, PartialEq)]
    enum S {
        White,
        Gray,
        Black,
    }
    fn visit(
        u: usize,
        adj: &[Vec<usize>],
        state: &mut Vec<S>,
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        state[u] = S::Gray;
        path.push(u);
        for &v in &adj[u] {
            match state[v] {
                S::Gray => {
                    let start = path.iter().position(|&x| x == v).unwrap_or(0);
                    let mut cyc: Vec<usize> = path[start..].to_vec();
                    cyc.push(v);
                    return Some(cyc);
                }
                S::White => {
                    if let Some(c) = visit(v, adj, state, path) {
                        return Some(c);
                    }
                }
                S::Black => {}
            }
        }
        path.pop();
        state[u] = S::Black;
        None
    }
    let mut state = vec![S::White; adj.len()];
    let mut path: Vec<usize> = Vec::new();
    for i in 0..adj.len() {
        if state[i] == S::White {
            if let Some(c) = visit(i, adj, &mut state, &mut path) {
                return Some(c);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::parse_tasks_board;

    /// A boundary resolver that knows nothing: every cross-session ref is
    /// unresolved, which is the case the view must flag rather than assume.
    fn no_boundary(_: &str, _: &str) -> Option<Status> {
        None
    }

    fn view_or_none(md: &str) -> Option<DepView> {
        let board = parse_tasks_board(md);
        build_view(&board, "csm", &mut no_boundary)
    }

    fn view(md: &str) -> DepView {
        view_or_none(md).expect("board has a live needs tail")
    }

    /// Fork/join: 001 -> 002, 003 -> 004.
    const DIAMOND: &str = "# b\n\n## Open\n\
        - 002 api - x needs: 001\n\
        - 003 frontend - x needs: 001\n\
        - 004 e2e - x needs: 002, 003\n\n\
        ## Done\n- 001 contract - x\n";

    // --- tail grammar ---

    #[test]
    fn parse_needs_tail_grammar() {
        assert_eq!(parse_needs_tail("001 x - gist"), TaskNeeds::default());

        let t = parse_needs_tail("002 api - x needs: 001");
        assert_eq!(t.unparseable, None);
        assert_eq!(
            t.refs,
            vec![NeedRef {
                session: None,
                task: "001".into(),
                condition: None
            }]
        );

        let t = parse_needs_tail("004 e2e - x needs: api-contract/012 [interface PR merged], 003");
        assert_eq!(t.refs.len(), 2);
        assert_eq!(t.refs[0].session.as_deref(), Some("api-contract"));
        assert_eq!(t.refs[0].task, "012");
        assert_eq!(t.refs[0].condition.as_deref(), Some("interface PR merged"));
        assert_eq!(t.refs[1].task, "003");
        assert_eq!(t.refs[1].condition, None);

        // A comma inside a condition does not split the ref.
        let t = parse_needs_tail("005 x - y needs: 001 [a, b]");
        assert_eq!(t.refs.len(), 1);
        assert_eq!(t.refs[0].condition.as_deref(), Some("a, b"));
    }

    #[test]
    fn parse_needs_tail_rejects_bad_grammar() {
        for bad in [
            "001 x - y needs:",
            "001 x - y needs: 002 003",
            "001 x - y needs: ???",
            "001 x - y needs: 002 [unclosed",
            "001 x - y needs: 002 []",
            "001 x - y needs: 002 trailing",
            "001 x - y needs: a//b",
            "001 x - y needs: /001",
            "001 x - y needs: ./001",
            "001 x - y needs: ../001",
            "001 x - y needs: api/../001",
            "001 x - y needs: api\\v2/001",
            "001 x - y needs: 002, ",
        ] {
            let t = parse_needs_tail(bad);
            assert!(t.unparseable.is_some(), "should be unparseable: {bad}");
            assert!(t.refs.is_empty(), "no refs from {bad}");
        }
    }

    // --- the needs list ---

    #[test]
    fn diamond_lists_every_edge_and_the_ready_set() {
        let v = view(DIAMOND);
        // The fork (001 needed by both) and the join (004 needs both) are
        // spelled out - no multi-prerequisite relation is dropped.
        assert_eq!(
            v.needs_lines,
            vec![
                "002 api (Open) needs 001 contract (Done)",
                "003 frontend (Open) needs 001 contract (Done)",
                "004 e2e (Open) needs 002 api (Open), 003 frontend (Open)",
            ]
        );
        assert_eq!(v.ready_line, "002, 003");
    }

    #[test]
    fn chain_carries_each_status_and_pending_review_is_not_ready() {
        let md = "# b\n\n## Open\n- 004 e2e - x needs: 002\n\n\
                  ## Pending review\n- 002 api - x needs: 001\n\n\
                  ## Pending fix\n- 003 fix - x needs: 001\n\n\
                  ## Done\n- 001 contract - x\n";
        let v = view(md);
        assert_eq!(
            v.needs_lines,
            vec![
                "004 e2e (Open) needs 002 api (Pending review)",
                "002 api (Pending review) needs 001 contract (Done)",
                "003 fix (Pending fix) needs 001 contract (Done)",
            ]
        );
        // 002 awaits the coordinator, so it is not claimable; 003 is.
        assert_eq!(v.ready_line, "003");
    }

    #[test]
    fn gistless_entry_label_drops_the_tail() {
        let v = view("# b\n\n## Open\n- 002 api check needs: 001\n\n## Done\n- 001 contract\n");
        assert_eq!(
            v.needs_lines,
            vec!["002 api check (Open) needs 001 contract (Done)"]
        );
        assert_eq!(v.ready_line, "002");
    }

    #[test]
    fn cjk_labels_render_inline() {
        let v = view("# b\n\n## Open\n- 002 前端 - x needs: 001\n\n## Done\n- 001 契约 - x\n");
        assert_eq!(v.needs_lines, vec!["002 前端 (Open) needs 001 契约 (Done)"]);
    }

    #[test]
    fn long_labels_preserve_all_dependencies_and_conditions() {
        let md = format!(
            "## Open\n- 003 {} - x needs: 001, 002 [interface PR merged]\n\
             - 002 blocker - x\n## Done\n- 001 {} - x\n",
            "integration".repeat(15),
            "contract".repeat(20)
        );
        let v = view(&md);
        assert!(v.needs_lines[0].contains(&format!("001 {} (Done)", "contract".repeat(20))));
        assert!(v.needs_lines[0].ends_with("002 blocker (Open) [interface PR merged]"));
        assert_eq!(v.ready_line, "002");
    }

    // --- ready ---

    #[test]
    fn conditioned_ref_is_never_satisfied() {
        let md = "# b\n\n## Open\n- 002 api - x needs: 001 [interface PR merged]\n\n\
                  ## Done\n- 001 contract - x\n";
        let v = view(md);
        assert!(v.ready_line.is_empty());
        assert_eq!(
            v.needs_lines,
            vec!["002 api (Open) needs 001 contract (Done) [interface PR merged] (unverified)"]
        );
    }

    #[test]
    fn known_done_boundary_satisfies() {
        let board = parse_tasks_board("# b\n\n## Open\n- 002 api - x needs: api-contract/012\n");
        let mut boundary =
            |s: &str, t: &str| (s == "api-contract" && t == "012").then_some(Status::Done);
        let v = build_view(&board, "csm", &mut boundary).unwrap();
        assert_eq!(v.ready_line, "002");
        assert_eq!(
            v.needs_lines,
            vec!["002 api (Open) needs api-contract/012 (Done)"]
        );
    }

    #[test]
    fn dotted_and_unicode_session_names_resolve_boundaries() {
        for session in ["api.v2", "接口.v2"] {
            let board =
                parse_tasks_board(&format!("## Open\n- 002 api - x needs: {session}/001\n"));
            let mut boundary =
                |s: &str, t: &str| (s == session && t == "001").then_some(Status::Done);
            let v = build_view(&board, "csm", &mut boundary).unwrap();
            assert_eq!(v.ready_line, "002");
            assert_eq!(
                v.needs_lines,
                vec![format!("002 api (Open) needs {session}/001 (Done)")]
            );
        }
    }

    #[test]
    fn own_session_prefix_resolves_locally() {
        let md = "# b\n\n## Open\n- 002 api - x needs: csm/001\n\n## Done\n- 001 contract - x\n";
        let v = view(md);
        assert_eq!(v.ready_line, "002");
        assert_eq!(
            v.needs_lines,
            vec!["002 api (Open) needs 001 contract (Done)"]
        );
    }

    // --- anomalies: marked inline, never satisfied ---

    #[test]
    fn unknown_boundary_is_flagged_not_satisfied() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: api-contract/012\n");
        assert!(v.ready_line.is_empty());
        assert_eq!(
            v.needs_lines,
            vec!["002 api (Open) needs api-contract/012 (unknown)"]
        );
    }

    #[test]
    fn missing_local_ref_is_flagged_inline() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: 099\n");
        assert!(v.ready_line.is_empty());
        assert_eq!(v.needs_lines, vec!["002 api (Open) needs 099 (missing)"]);
    }

    #[test]
    fn a_flagged_ref_does_not_hide_the_resolvable_ones() {
        let md = "# b\n\n## Open\n- 003 solo - x needs: 001, 099\n\n## Done\n- 001 contract - x\n";
        let v = view(md);
        assert_eq!(
            v.needs_lines,
            vec!["003 solo (Open) needs 001 contract (Done), 099 (missing)"]
        );
        assert!(v.ready_line.is_empty());
    }

    #[test]
    fn self_need_is_named_itself_and_never_satisfied() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: 002\n");
        assert_eq!(v.needs_lines, vec!["002 api (Open) needs itself"]);
        assert!(v.ready_line.is_empty());
        // A self-loop is already named by the line; no cycle warning on top.
        assert!(!v.needs_lines.iter().any(|l| l.contains("cycle")));
    }

    #[test]
    fn cycle_keeps_every_edge_and_names_the_loop_once() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: 003\n- 003 frontend - x needs: 002\n");
        assert_eq!(
            v.needs_lines,
            vec![
                "002 api (Open) needs 003 frontend (Open)",
                "003 frontend (Open) needs 002 api (Open)",
                "warning: cycle in needs: 002 api -> 003 frontend -> 002 api",
            ]
        );
        assert!(v.ready_line.is_empty());
    }

    #[test]
    fn unparseable_tail_is_surfaced_inline() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: ???\n");
        assert_eq!(
            v.needs_lines,
            vec!["002 api (Open) needs check: unparseable tail (needs: ???)"]
        );
        assert!(v.ready_line.is_empty());
    }

    // --- dependency-free sessions keep the bare card ---

    #[test]
    fn dependency_free_board_has_no_view() {
        assert!(view_or_none("# b\n\n## Open\n- 001 a - x\n- 002 b - x\n").is_none());
    }

    #[test]
    fn fully_done_board_has_no_view() {
        // A tail on a Done task is history, not a live plan.
        assert!(view_or_none("# b\n\n## Done\n- 001 a - x needs: 000\n- 000 z - x\n").is_none());
    }

    #[test]
    fn empty_board_has_no_view() {
        assert!(view_or_none("# b\n\n## Open\n").is_none());
    }
}
