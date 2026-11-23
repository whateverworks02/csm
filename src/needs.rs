//! Render live tasks and their immediate dependencies for `csm show`.

use crate::ui;
use crate::workspace::{
    entry_head, entry_id, parse_needs_tail, NeedRef, Status, TaskNeeds, TasksBoard,
};
use std::collections::{HashMap, HashSet};

struct Entry {
    id: String,
    label: String,
    status: Status,
    needs: TaskNeeds,
    has_view: bool,
}

fn collect_entries(board: &TasksBoard, view_ids: &HashSet<String>) -> Vec<Entry> {
    let mut out = Vec::new();
    for (raws, status) in board.sections() {
        for raw in raws {
            let Some(id) = entry_id(raw) else {
                continue;
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

pub struct DepView {
    pub sections: Vec<SectionLines>,
    pub warning_line: Option<String>,
    pub claimable_line: String,
}

pub struct SectionLines {
    pub status: Status,
    pub lines: Vec<String>,
}

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

fn dim_branch(last: bool) -> String {
    ui::paint(ui::DIM, branch(last))
}

pub fn view_mark(has_view: bool) -> String {
    if has_view {
        format!(" {}", ui::paint(ui::CYAN_BOLD, ui::T_VIEW))
    } else {
        String::new()
    }
}

fn status_mark(s: Status) -> String {
    let (g, style) = match s {
        Status::Open => (ui::T_OPEN, ui::CYAN),
        Status::PendingReview => (ui::T_REVIEW, ui::YELLOW),
        Status::PendingFix => (ui::T_FIX, ui::MAGENTA),
        Status::Done => (ui::T_DONE, ui::GREEN),
    };
    ui::paint(style, g)
}

enum RefTarget {
    Itself,
    Local(usize),
    Missing,
}

struct LeafView {
    line: String,
    satisfied: bool,
}

fn leaf_view(
    r: &NeedRef,
    consumer: usize,
    entries: &[Entry],
    by_id: &HashMap<&str, usize>,
) -> LeafView {
    let target = match by_id.get(r.task.as_str()) {
        Some(_) if r.task == entries[consumer].id => RefTarget::Itself,
        Some(&i) => RefTarget::Local(i),
        None => RefTarget::Missing,
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
            // Conditions need external evidence even when the referenced task is Done.
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
    }
}

pub fn build_view(board: &TasksBoard, view_ids: &HashSet<String>) -> Option<DepView> {
    let entries = collect_entries(board, view_ids);
    let by_id: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i))
        .collect();
    let live: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.status.is_unfinished())
        .map(|(i, _)| i)
        .collect();
    if live.is_empty() {
        return None;
    }
    let has_live_tail = live.iter().any(|&i| {
        let n = &entries[i].needs;
        !n.refs.is_empty() || n.unparseable.is_some()
    });

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
                    let lv = leaf_view(r, i, &entries, &by_id);
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

    let warning_line = find_cycle(&live_adjacency(&entries, &by_id)).map(|cycle| {
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

/// Local live-task edges. Self-references are reported separately on their leaves.
fn live_adjacency(entries: &[Entry], by_id: &HashMap<&str, usize>) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); entries.len()];
    for (i, e) in entries.iter().enumerate() {
        if e.status == Status::Done {
            continue;
        }
        for r in &e.needs.refs {
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

/// Return the first cycle as a closed path, such as `a -> b -> a`.
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

    fn view_ids(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn view_or_none(md: &str) -> Option<DepView> {
        let board = parse_tasks_board(md);
        build_view(&board, &HashSet::new())
    }

    fn view(md: &str) -> DepView {
        view_or_none(md).expect("board has a live needs tail")
    }

    fn view_with(md: &str, ids: &[&str]) -> DepView {
        let board = parse_tasks_board(md);
        build_view(&board, &view_ids(ids)).expect("board has a live needs tail")
    }

    fn lines(v: &DepView) -> Vec<String> {
        v.sections
            .iter()
            .flat_map(|s| s.lines.iter().cloned())
            .collect()
    }

    #[test]
    fn view_marker_follows_the_task_through_the_forest() {
        let md = "## Open\n- 001 a\n- 002 b needs: 001\n";
        assert_eq!(
            lines(&view_with(md, &["001"])),
            vec!["├── ○ 001 a ◆", "└── ○ 002 b", "    └── ○ 001 a ◆"]
        );
        assert_eq!(
            lines(&view_with(md, &["002"])),
            vec!["├── ○ 001 a", "└── ○ 002 b ◆", "    └── ○ 001 a"]
        );
    }

    #[test]
    fn view_marker_follows_leaf_annotations() {
        let md = "## Open\n- 002 api needs: 001 [pr merged]\n\n## Done\n- 001 contract\n";
        assert_eq!(
            lines(&view_with(md, &["001"])),
            vec![
                "└── ○ 002 api",
                "    └── ✓ 001 contract [pr merged] (unverified) ◆",
            ]
        );
    }

    const DIAMOND: &str = "# b\n\n## Open\n\
        - 002 api needs: 001\n\
        - 003 frontend needs: 001\n\
        - 004 e2e needs: 002, 003\n\n\
        ## Done\n- 001 contract\n";

    #[test]
    fn diamond_renders_each_edge_and_the_claimable_set() {
        let v = view(DIAMOND);
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
        let md = "# b\n\n## Open\n- 004 e2e needs: 002\n\n\
                  ## Pending review\n- 002 api needs: 001\n\n\
                  ## Pending fix\n- 003 fix needs: 001\n\n\
                  ## Done\n- 001 contract\n";
        let v = view(md);
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
        assert_eq!(v.claimable_line, "003");
    }

    #[test]
    fn entry_label_drops_the_needs_tail() {
        let v = view("# b\n\n## Open\n- 002 api check needs: 001\n\n## Done\n- 001 contract\n");
        assert_eq!(
            lines(&v),
            vec!["└── ○ 002 api check", "    └── ✓ 001 contract",]
        );
        assert_eq!(v.claimable_line, "002");
    }

    #[test]
    fn non_ascii_labels_render_inline() {
        let v = view("# b\n\n## Open\n- 002 café needs: 001\n\n## Done\n- 001 naïve\n");
        assert_eq!(lines(&v), vec!["└── ○ 002 café", "    └── ✓ 001 naïve"]);
    }

    #[test]
    fn long_labels_preserve_all_dependencies_and_conditions() {
        let md = format!(
            "## Open\n- 003 {} needs: 001, 002 [interface PR merged]\n\
             - 002 blocker\n## Done\n- 001 {}\n",
            "integration".repeat(15),
            "contract".repeat(20)
        );
        let v = view(&md);
        assert!(lines(&v)[1].contains(&format!("✓ 001 {}", "contract".repeat(20))));
        assert_eq!(lines(&v)[2], "│   └── ○ 002 blocker [interface PR merged]");
        assert_eq!(v.claimable_line, "002");
    }

    #[test]
    fn conditioned_ref_is_never_satisfied() {
        let md = "# b\n\n## Open\n- 002 api needs: 001 [interface PR merged]\n\n\
                  ## Done\n- 001 contract\n";
        let v = view(md);
        assert_eq!(v.claimable_line, "");
        assert_eq!(
            lines(&v),
            vec![
                "└── ○ 002 api",
                "    └── ✓ 001 contract [interface PR merged] (unverified)",
            ]
        );
    }

    #[test]
    fn session_qualified_ref_blocks_claiming_even_if_local_id_is_done() {
        let v = view("## Open\n- 002 api needs: api-contract/012\n\n## Done\n- 012 contract\n");
        assert_eq!(v.claimable_line, "");
        assert_eq!(
            lines(&v),
            vec![
                "└── ○ 002 api",
                "    └── ! unparseable tail (needs: api-contract/012)",
            ]
        );
    }

    #[test]
    fn missing_local_ref_is_flagged_inline() {
        let v = view("# b\n\n## Open\n- 002 api needs: 099\n");
        assert_eq!(v.claimable_line, "");
        assert_eq!(lines(&v), vec!["└── ○ 002 api", "    └── ? 099 (missing)"]);
    }

    #[test]
    fn unresolved_leaves_keep_their_condition() {
        let v = view(
            "# b\n\n## Open\n- 002 api needs: 099 [interface PR merged], \
             012 [deployed]\n",
        );
        assert_eq!(
            lines(&v),
            vec![
                "└── ○ 002 api",
                "    ├── ? 099 [interface PR merged] (missing)",
                "    └── ? 012 [deployed] (missing)",
            ]
        );
        assert_eq!(v.claimable_line, "");
    }

    #[test]
    fn a_flagged_ref_does_not_hide_the_resolvable_ones() {
        let md = "# b\n\n## Open\n- 003 solo needs: 001, 099\n\n## Done\n- 001 contract\n";
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
        let v = view("# b\n\n## Open\n- 002 api needs: 002\n");
        assert_eq!(lines(&v), vec!["└── ○ 002 api", "    └── ! itself"]);
        assert_eq!(v.claimable_line, "");
        assert!(v.warning_line.is_none());
    }

    #[test]
    fn cycle_keeps_every_edge_and_names_the_loop_once() {
        let v = view("# b\n\n## Open\n- 002 api needs: 003\n- 003 frontend needs: 002\n");
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
        let v = view("# b\n\n## Open\n- 002 api needs: ???\n");
        assert_eq!(
            lines(&v),
            vec!["└── ○ 002 api", "    └── ! unparseable tail (needs: ???)"]
        );
        assert_eq!(v.claimable_line, "");
    }

    #[test]
    fn dependency_free_board_renders_roots_without_claimable() {
        let v = view("# b\n\n## Open\n- 001 a\n- 002 b\n");
        assert_eq!(lines(&v), vec!["├── ○ 001 a", "└── ○ 002 b"]);
        assert_eq!(v.claimable_line, "");
    }

    #[test]
    fn fully_done_board_has_no_view() {
        assert!(view_or_none("# b\n\n## Done\n- 001 a needs: 000\n- 000 z\n").is_none());
    }

    #[test]
    fn empty_board_has_no_view() {
        assert!(view_or_none("# b\n\n## Open\n").is_none());
    }
}
