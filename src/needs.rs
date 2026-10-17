//! Tasks-block view for `csm show`: render the board's `needs:` tails as a
//! dependency forest. The tails are the single dependency mapping, parsed by
//! [`crate::workspace::parse_needs_tail`] so this view, the snapshot's board
//! block, and the head all read one rule; this module reads them straight from
//! the parsed board - no second graph file, no cache - so the picture cannot
//! drift from the plan the agents read.
//!
//! The forest follows the `cargo tree` lineage: every live task (Open, then
//! Pending review, then Pending fix - board order) is a root; its needs hang
//! below as leaves. A live task is expanded only at its own root - under
//! another task it is a single leaf - so no subtree repeats and a cycle
//! cannot recurse. Status rides on glyphs (`○` `◐` `◑` `✓`); anomalies are
//! marked at the relation they break (`? ... (missing)` / `(unknown)`,
//! `! itself`, `! unparseable tail`, a `warning:` cycle line) - never
//! rendered as satisfied. `claimable` lists the Open / Pending fix tasks
//! whose refs all hold; a `[condition]` is never assumed met. Boards with
//! no live task keep the bare card (summary + done line only).

use crate::ui;
use crate::workspace::{
    entry_head, entry_id, parse_needs_tail, NeedRef, Status, TaskNeeds, TasksBoard,
};
use std::collections::{HashMap, HashSet};

// --- view building -----------------------------------------------------------

/// One board entry with its tail parsed and status attached.
struct Entry {
    id: String,
    label: String,
    status: Status,
    needs: TaskNeeds,
    /// Whether a view binds to this task. Stamped once here, so the binding is
    /// task data rather than something each render site re-derives.
    has_view: bool,
}

fn collect_entries(board: &TasksBoard, view_ids: &HashSet<String>) -> Vec<Entry> {
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
                has_view: view_ids.contains(id),
            });
        }
    }
    out
}

/// The rendered tasks block of the `csm show` card.
pub struct DepView {
    /// The live roots and their `needs:` leaves, grouped by board section in
    /// board order. Each group is its own `cargo tree`, rendered under its
    /// section's label; a section with no live task contributes no group.
    pub sections: Vec<SectionLines>,
    /// The board-level cycle `warning:` line when the tails loop, rendered once
    /// after the sections; `None` when no loop exists.
    pub warning_line: Option<String>,
    /// Comma-joined ids of claimable tasks (Open / Pending fix whose needs
    /// all hold). Empty when nothing is claimable or no live task carries a
    /// tail - then the row would only restate the Open roots.
    pub claimable_line: String,
}

/// One board section of the card's tasks block: the status it renders under,
/// and the forest lines for the tasks that sit in it.
pub struct SectionLines {
    pub status: Status,
    pub lines: Vec<String>,
}

/// cargo-tree connectors. Roots hang off the `tasks` label row; children of
/// the last root lose the rail. Exactly these four pieces, so the forest
/// cannot drift into a second shape.
const BRANCH: &str = "├── ";
const LAST_BRANCH: &str = "└── ";
const RAIL: &str = "│   ";
const NO_RAIL: &str = "    ";

fn branch(last: bool) -> &'static str {
    if last {
        LAST_BRANCH
    } else {
        BRANCH
    }
}

/// A dimmed connector: the structure recedes, the task nodes carry the eye.
fn dim_branch(last: bool) -> String {
    ui::paint(ui::DIM, branch(last))
}

/// A task's view marker: the painted glyph when the task has a view, nothing
/// otherwise. Presence only - absence needs no glyph, so the forest stays a
/// picture of the plan. The single emitter for the marker, so every place a
/// task's label renders separates it from the label the same way.
pub fn view_mark(has_view: bool) -> String {
    if has_view {
        format!(" {}", ui::paint(ui::CYAN_BOLD, ui::T_VIEW))
    } else {
        String::new()
    }
}

/// Status glyph + color for one task node. Shape carries the status without
/// color; color reinforces it (cyan open, yellow review, magenta fix, green
/// done).
fn status_mark(s: Status) -> String {
    let (g, style) = match s {
        Status::Open => (ui::T_OPEN, ui::CYAN),
        Status::PendingReview => (ui::T_REVIEW, ui::YELLOW),
        Status::PendingFix => (ui::T_FIX, ui::MAGENTA),
        Status::Done => (ui::T_DONE, ui::GREEN),
    };
    ui::paint(style, g)
}

/// Where a `needs:` ref points, resolved against this board or the boundary.
enum RefTarget {
    /// The consumer names itself.
    Itself,
    /// A task on this board (index into `entries`).
    Local(usize),
    /// A local id no board entry carries.
    Missing,
    /// A cross-session ref, with the target's status when the boundary
    /// resolves it (`None` = session or task not found).
    Foreign {
        name: String,
        status: Option<Status>,
    },
}

/// One rendered forest leaf, with whether the ref counts as met.
struct LeafView {
    line: String,
    satisfied: bool,
}

/// Render one ref as a leaf: a status-marked task when it resolves, `?`
/// when the boards cannot resolve it, `!` when the relation itself is
/// broken. The condition rides on every leaf, resolved or not. A ref is met
/// only when it resolves to a Done task and carries no condition - a
/// condition is never assumed verified.
fn leaf_view(
    r: &NeedRef,
    consumer: usize,
    session: &str,
    entries: &[Entry],
    by_id: &HashMap<&str, usize>,
    boundary_status: &mut dyn FnMut(&str, &str) -> Option<Status>,
) -> LeafView {
    // A `session/task` ref naming this session itself is local.
    let local = r.session.as_deref().is_none_or(|s| s == session);
    let target = if local && r.task == entries[consumer].id {
        RefTarget::Itself
    } else if local {
        match by_id.get(r.task.as_str()) {
            Some(&i) => RefTarget::Local(i),
            None => RefTarget::Missing,
        }
    } else {
        let s = r.session.as_deref().unwrap();
        RefTarget::Foreign {
            name: format!("{s}/{}", r.task),
            status: boundary_status(s, &r.task),
        }
    };
    let cond = match &r.condition {
        Some(c) => format!(" [{c}]"),
        None => String::new(),
    };
    match target {
        RefTarget::Itself => LeafView {
            line: format!("{} itself", ui::paint(ui::YELLOW, ui::T_ALERT)),
            satisfied: false,
        },
        RefTarget::Local(i) => {
            let e = &entries[i];
            // A Done ref with a condition looks satisfied but never is -
            // say so at the leaf.
            let unverified = if e.status == Status::Done && r.condition.is_some() {
                " (unverified)"
            } else {
                ""
            };
            LeafView {
                line: format!(
                    "{} {}{cond}{unverified}{}",
                    status_mark(e.status),
                    e.label,
                    view_mark(e.has_view)
                ),
                satisfied: e.status == Status::Done && r.condition.is_none(),
            }
        }
        RefTarget::Missing => LeafView {
            line: format!(
                "{} {}{cond} (missing)",
                ui::paint(ui::DIM, ui::T_UNKNOWN),
                r.task
            ),
            satisfied: false,
        },
        RefTarget::Foreign { name, status } => match status {
            Some(s) => {
                let unverified = if s == Status::Done && r.condition.is_some() {
                    " (unverified)"
                } else {
                    ""
                };
                LeafView {
                    line: format!("{} {name}{cond}{unverified}", status_mark(s)),
                    satisfied: s == Status::Done && r.condition.is_none(),
                }
            }
            None => LeafView {
                line: format!(
                    "{} {name}{cond} (unknown)",
                    ui::paint(ui::DIM, ui::T_UNKNOWN)
                ),
                satisfied: false,
            },
        },
    }
}

/// Build the forest view for one session's board. Roots are grouped by the
/// board's own sections - the reader's mental model - and each group is its own
/// tree, so the last root of a section drops the rail. `boundary_status`
/// resolves a cross-session ref against the target session's own board (`None` =
/// session or task not found, shown as `(unknown)`). `view_ids` are the task
/// ids that have a view, marked on their task's label wherever it renders.
/// Returns `None` when no task is live - a fully-done (or empty) board keeps
/// the bare card (summary + done section only). Read-only over the board:
/// nothing here writes back.
pub fn build_view(
    board: &TasksBoard,
    session: &str,
    boundary_status: &mut dyn FnMut(&str, &str) -> Option<Status>,
    view_ids: &HashSet<String>,
) -> Option<DepView> {
    let entries = collect_entries(board, view_ids);
    let by_id: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i))
        .collect();
    // Roots: every live task, in board order (collect_entries already walks
    // Open, then Pending review, then Pending fix).
    let live: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.status.is_unfinished())
        .map(|(i, _)| i)
        .collect();
    if live.is_empty() {
        return None;
    }
    // The claimable row earns its place only when a tail exists to gate
    // anything; without tails it would restate the Open roots.
    let has_live_tail = live.iter().any(|&i| {
        let n = &entries[i].needs;
        !n.refs.is_empty() || n.unparseable.is_some()
    });

    // `live` is in board order, so each status's live entries are one run of
    // it; the group is that run, rendered as its own tree.
    let mut sections: Vec<SectionLines> = Vec::new();
    let mut claimable: Vec<&str> = Vec::new();
    for status in Status::ALL {
        let group: Vec<usize> = live
            .iter()
            .copied()
            .filter(|&i| entries[i].status == status)
            .collect();
        if group.is_empty() {
            continue;
        }
        let mut lines = Vec::new();
        for (pos, &i) in group.iter().enumerate() {
            let e = &entries[i];
            let last_root = pos + 1 == group.len();
            lines.push(format!(
                "{}{} {}{}",
                dim_branch(last_root),
                status_mark(e.status),
                e.label,
                view_mark(e.has_view)
            ));
            // Children of the last root drop the rail (cargo tree).
            let rail = ui::paint(ui::DIM, if last_root { NO_RAIL } else { RAIL });
            let mut all_met = true;
            if let Some(raw) = &e.needs.unparseable {
                lines.push(format!(
                    "{rail}{}{} unparseable tail ({raw})",
                    dim_branch(true),
                    ui::paint(ui::YELLOW, ui::T_ALERT)
                ));
                all_met = false;
            } else {
                for (j, r) in e.needs.refs.iter().enumerate() {
                    let lv = leaf_view(r, i, session, &entries, &by_id, boundary_status);
                    all_met &= lv.satisfied;
                    let leaf_last = j + 1 == e.needs.refs.len();
                    lines.push(format!("{rail}{}{}", dim_branch(leaf_last), lv.line));
                }
            }
            let claimable_status = matches!(e.status, Status::Open | Status::PendingFix);
            if claimable_status && all_met {
                claimable.push(e.id.as_str());
            }
        }
        sections.push(SectionLines { status, lines });
    }

    // A loop spans sections, so its warning is the block's, not a section's.
    let warning_line = find_cycle(&live_adjacency(&entries, &by_id, session)).map(|cycle| {
        let names: Vec<&str> = cycle.iter().map(|&i| entries[i].label.as_str()).collect();
        format!(
            "{} cycle in needs: {}",
            ui::paint(ui::YELLOW, "warning:"),
            names.join(" -> ")
        )
    });

    Some(DepView {
        sections,
        warning_line,
        claimable_line: if has_live_tail {
            claimable.join(", ")
        } else {
            String::new()
        },
    })
}

/// Local edges among live entries, for cycle detection. Self-loops are
/// excluded (the leaf already says `! itself`); missing refs have no
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

    /// A view-id set from literals: the shape the marker tests assert.
    fn view_ids(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn view_or_none(md: &str) -> Option<DepView> {
        let board = parse_tasks_board(md);
        build_view(&board, "csm", &mut no_boundary, &HashSet::new())
    }

    fn view(md: &str) -> DepView {
        view_or_none(md).expect("board has a live needs tail")
    }

    fn view_with(md: &str, ids: &[&str]) -> DepView {
        let board = parse_tasks_board(md);
        build_view(&board, "csm", &mut no_boundary, &view_ids(ids))
            .expect("board has a live needs tail")
    }

    /// Every live line in board order with the section grouping flattened - the
    /// forest as one sequence, which is the shape most of these tests assert on.
    fn lines(v: &DepView) -> Vec<String> {
        v.sections
            .iter()
            .flat_map(|s| s.lines.iter().cloned())
            .collect()
    }

    /// A view marker follows the task it belongs to, everywhere that task's
    /// name renders: its own line, and the `needs:` leaf that references it.
    /// A task with no file carries nothing.
    #[test]
    fn view_marker_follows_the_task_through_the_forest() {
        let md = "## Open\n- 001 a - x\n- 002 b - x needs: 001\n";
        assert_eq!(
            lines(&view_with(md, &["001"])),
            vec!["├── ○ 001 a ◆", "└── ○ 002 b", "    └── ○ 001 a ◆"]
        );
        assert_eq!(
            lines(&view_with(md, &["002"])),
            vec!["├── ○ 001 a", "└── ○ 002 b ◆", "    └── ○ 001 a"]
        );
    }

    /// The marker on a leaf follows the ref's own annotations, so the
    /// condition and `(unverified)` stay attached to the label.
    #[test]
    fn view_marker_follows_leaf_annotations() {
        let md = "## Open\n- 002 api - x needs: 001 [pr merged]\n\n## Done\n- 001 contract - x\n";
        assert_eq!(
            lines(&view_with(md, &["001"])),
            vec![
                "└── ○ 002 api",
                "    └── ✓ 001 contract [pr merged] (unverified) ◆",
            ]
        );
    }

    /// Fork/join: 001 -> 002, 003 -> 004.
    const DIAMOND: &str = "# b\n\n## Open\n\
        - 002 api - x needs: 001\n\
        - 003 frontend - x needs: 001\n\
        - 004 e2e - x needs: 002, 003\n\n\
        ## Done\n- 001 contract - x\n";

    // --- the forest ---

    #[test]
    fn diamond_renders_each_edge_and_the_claimable_set() {
        let v = view(DIAMOND);
        // The fork (001 needed by both) and the join (004 needs both) are
        // spelled out; a live task is a single leaf under another root (002
        // under 004), expanded only at its own root - no subtree repeats.
        assert_eq!(
            lines(&v),
            vec![
                "├── ○ 002 api",
                "│   └── ✓ 001 contract",
                "├── ○ 003 frontend",
                "│   └── ✓ 001 contract",
                "└── ○ 004 e2e",
                "    ├── ○ 002 api",
                "    └── ○ 003 frontend",
            ]
        );
        assert_eq!(v.claimable_line, "002, 003");
    }

    #[test]
    fn chain_marks_each_status_and_pending_review_is_not_claimable() {
        let md = "# b\n\n## Open\n- 004 e2e - x needs: 002\n\n\
                  ## Pending review\n- 002 api - x needs: 001\n\n\
                  ## Pending fix\n- 003 fix - x needs: 001\n\n\
                  ## Done\n- 001 contract - x\n";
        let v = view(md);
        // One group per board section, in board order: Open, then Pending
        // review (◐), then Pending fix (◑). Each group is its own tree, so
        // every group's last root drops the rail and its children lose it.
        assert_eq!(
            v.sections.iter().map(|s| s.status).collect::<Vec<_>>(),
            vec![Status::Open, Status::PendingReview, Status::PendingFix]
        );
        assert_eq!(
            v.sections
                .iter()
                .map(|s| s.lines.clone())
                .collect::<Vec<_>>(),
            vec![
                vec!["└── ○ 004 e2e".to_string(), "    └── ◐ 002 api".to_string()],
                vec![
                    "└── ◐ 002 api".to_string(),
                    "    └── ✓ 001 contract".to_string()
                ],
                vec![
                    "└── ◑ 003 fix".to_string(),
                    "    └── ✓ 001 contract".to_string()
                ],
            ]
        );
        assert_eq!(
            lines(&v),
            vec![
                "└── ○ 004 e2e",
                "    └── ◐ 002 api",
                "└── ◐ 002 api",
                "    └── ✓ 001 contract",
                "└── ◑ 003 fix",
                "    └── ✓ 001 contract",
            ]
        );
        // 002 awaits the coordinator, so it is not claimable; 003 is.
        assert_eq!(v.claimable_line, "003");
    }

    #[test]
    fn gistless_entry_label_drops_the_tail() {
        let v = view("# b\n\n## Open\n- 002 api check needs: 001\n\n## Done\n- 001 contract\n");
        assert_eq!(
            lines(&v),
            vec!["└── ○ 002 api check", "    └── ✓ 001 contract",]
        );
        assert_eq!(v.claimable_line, "002");
    }

    #[test]
    fn cjk_labels_render_inline() {
        let v = view("# b\n\n## Open\n- 002 前端 - x needs: 001\n\n## Done\n- 001 契约 - x\n");
        assert_eq!(lines(&v), vec!["└── ○ 002 前端", "    └── ✓ 001 契约"]);
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
        assert!(lines(&v)[1].contains(&format!("✓ 001 {}", "contract".repeat(20))));
        assert_eq!(lines(&v)[2], "│   └── ○ 002 blocker [interface PR merged]");
        assert_eq!(v.claimable_line, "002");
    }

    // --- claimable ---

    #[test]
    fn conditioned_ref_is_never_satisfied() {
        let md = "# b\n\n## Open\n- 002 api - x needs: 001 [interface PR merged]\n\n\
                  ## Done\n- 001 contract - x\n";
        let v = view(md);
        assert_eq!(v.claimable_line, "");
        // The leaf looks Done but says so itself: the condition is unverified.
        assert_eq!(
            lines(&v),
            vec![
                "└── ○ 002 api",
                "    └── ✓ 001 contract [interface PR merged] (unverified)",
            ]
        );
    }

    #[test]
    fn known_done_boundary_satisfies() {
        let board = parse_tasks_board("# b\n\n## Open\n- 002 api - x needs: api-contract/012\n");
        let mut boundary =
            |s: &str, t: &str| (s == "api-contract" && t == "012").then_some(Status::Done);
        let v = build_view(&board, "csm", &mut boundary, &HashSet::new()).unwrap();
        assert_eq!(v.claimable_line, "002");
        assert_eq!(
            lines(&v),
            vec!["└── ○ 002 api", "    └── ✓ api-contract/012"]
        );
    }

    #[test]
    fn dotted_and_unicode_session_names_resolve_boundaries() {
        for session in ["api.v2", "接口.v2"] {
            let board =
                parse_tasks_board(&format!("## Open\n- 002 api - x needs: {session}/001\n"));
            let mut boundary =
                |s: &str, t: &str| (s == session && t == "001").then_some(Status::Done);
            let v = build_view(&board, "csm", &mut boundary, &HashSet::new()).unwrap();
            assert_eq!(v.claimable_line, "002");
            assert_eq!(
                lines(&v),
                vec![
                    "└── ○ 002 api".to_string(),
                    format!("    └── ✓ {session}/001")
                ]
            );
        }
    }

    #[test]
    fn own_session_prefix_resolves_locally() {
        let md = "# b\n\n## Open\n- 002 api - x needs: csm/001\n\n## Done\n- 001 contract - x\n";
        let v = view(md);
        assert_eq!(v.claimable_line, "002");
        assert_eq!(lines(&v), vec!["└── ○ 002 api", "    └── ✓ 001 contract"]);
    }

    // --- anomalies: marked at the leaf, never satisfied ---

    #[test]
    fn unknown_boundary_is_flagged_not_satisfied() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: api-contract/012\n");
        assert_eq!(v.claimable_line, "");
        assert_eq!(
            lines(&v),
            vec!["└── ○ 002 api", "    └── ? api-contract/012 (unknown)"]
        );
    }

    #[test]
    fn missing_local_ref_is_flagged_inline() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: 099\n");
        assert_eq!(v.claimable_line, "");
        assert_eq!(lines(&v), vec!["└── ○ 002 api", "    └── ? 099 (missing)"]);
    }

    #[test]
    fn unresolved_leaves_keep_their_condition() {
        // The condition matters most exactly where the ref cannot resolve -
        // it is what must be verified once the target appears.
        let v = view(
            "# b\n\n## Open\n- 002 api - x needs: 099 [interface PR merged], \
             api-contract/012 [deployed]\n",
        );
        assert_eq!(
            lines(&v),
            vec![
                "└── ○ 002 api",
                "    ├── ? 099 [interface PR merged] (missing)",
                "    └── ? api-contract/012 [deployed] (unknown)",
            ]
        );
        assert_eq!(v.claimable_line, "");
    }

    #[test]
    fn a_flagged_ref_does_not_hide_the_resolvable_ones() {
        let md = "# b\n\n## Open\n- 003 solo - x needs: 001, 099\n\n## Done\n- 001 contract - x\n";
        let v = view(md);
        assert_eq!(
            lines(&v),
            vec![
                "└── ○ 003 solo",
                "    ├── ✓ 001 contract",
                "    └── ? 099 (missing)",
            ]
        );
        assert_eq!(v.claimable_line, "");
    }

    #[test]
    fn self_need_is_named_itself_and_never_satisfied() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: 002\n");
        assert_eq!(lines(&v), vec!["└── ○ 002 api", "    └── ! itself"]);
        assert_eq!(v.claimable_line, "");
        // A self-loop is already named by the leaf; no cycle warning on top.
        assert!(v.warning_line.is_none());
    }

    #[test]
    fn cycle_keeps_every_edge_and_names_the_loop_once() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: 003\n- 003 frontend - x needs: 002\n");
        // Expand-once: each side is a single leaf under the other, so the
        // loop renders without recursing; the warning names it once, at the
        // block's level rather than under a section.
        assert_eq!(
            lines(&v),
            vec![
                "├── ○ 002 api",
                "│   └── ○ 003 frontend",
                "└── ○ 003 frontend",
                "    └── ○ 002 api",
            ]
        );
        assert_eq!(
            v.warning_line.as_deref(),
            Some("warning: cycle in needs: 002 api -> 003 frontend -> 002 api")
        );
        assert_eq!(v.claimable_line, "");
    }

    #[test]
    fn unparseable_tail_is_surfaced_inline() {
        let v = view("# b\n\n## Open\n- 002 api - x needs: ???\n");
        assert_eq!(
            lines(&v),
            vec!["└── ○ 002 api", "    └── ! unparseable tail (needs: ???)"]
        );
        assert_eq!(v.claimable_line, "");
    }

    // --- boards without live tails ---

    #[test]
    fn dependency_free_board_renders_roots_without_claimable() {
        // No tails to gate anything: the forest is the plain root list, and
        // a claimable row would only restate it.
        let v = view("# b\n\n## Open\n- 001 a - x\n- 002 b - x\n");
        assert_eq!(lines(&v), vec!["├── ○ 001 a", "└── ○ 002 b"]);
        assert_eq!(v.claimable_line, "");
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
