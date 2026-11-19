//! Delete unpinned, active sessions whose boards have no unfinished tasks.

use crate::store::{self, SessionMeta};
use crate::ui;
use crate::workspace;
use anyhow::Result;
use chrono::Local;

struct Candidate<'a> {
    name: &'a str,
    meta: &'a SessionMeta,
    unfinished: usize,
}

pub fn run(older_than: Option<u64>, yes: bool) -> Result<()> {
    let idx = store::load_index()?;
    let now = Local::now();

    let mut candidates: Vec<Candidate> = idx
        .sessions
        .iter()
        .filter(|(_, m)| !m.pinned && !m.is_archived())
        .filter(|(_, m)| {
            older_than.is_none_or(|d| {
                store::parse_time(&m.last_access)
                    .map(|t| (now - t).num_days() >= d as i64)
                    .unwrap_or(false)
            })
        })
        .map(|(k, v)| Candidate {
            name: k,
            meta: v,
            unfinished: unfinished_tasks(k),
        })
        .collect();
    candidates.sort_by(|a, b| b.meta.last_access.cmp(&a.meta.last_access));

    if candidates.is_empty() {
        eprintln!(
            "{}",
            ui::epaint(ui::DIM, "no unpinned sessions to garbage-collect.")
        );
        return Ok(());
    }

    let (kept, deletable): (Vec<Candidate>, Vec<Candidate>) =
        candidates.into_iter().partition(|c| c.unfinished > 0);

    if !kept.is_empty() {
        eprintln!(
            "{}",
            ui::epaint(
                ui::BOLD,
                &format!(
                    "kept - {} session(s) with unfinished tasks on the board:",
                    kept.len()
                )
            )
        );
        print_kept(&kept);
        ui::hint("delete one deliberately with: csm rm <name>");
    }

    if deletable.is_empty() {
        eprintln!("{}", ui::epaint(ui::DIM, "nothing to garbage-collect."));
        return Ok(());
    }

    let header = match older_than {
        Some(d) => format!("unpinned sessions not accessed in the last {d} day(s):"),
        None => "unpinned sessions:".to_string(),
    };
    eprintln!("{}", ui::epaint(ui::BOLD, &header));
    print_collectable(&deletable);

    let to_delete: Vec<String> = if older_than.is_some() {
        if yes
            || ui::confirm(&format!(
                "delete all {} listed session(s)?",
                deletable.len()
            ))?
        {
            names(&deletable)
        } else {
            eprintln!("{}", ui::epaint(ui::DIM, "aborted"));
            return Ok(());
        }
    } else {
        eprint!(
            "\n{} ",
            ui::epaint(
                ui::DIM,
                "select indices to delete (comma-separated), 'a' for all, 'q' to quit:"
            ),
        );
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let line = line.trim();
        if line.is_empty() || line.eq_ignore_ascii_case("q") {
            eprintln!("{}", ui::epaint(ui::DIM, "aborted"));
            return Ok(());
        }
        let selected = select_from_line(line, &names(&deletable));
        if selected.is_empty() {
            eprintln!("{}", ui::epaint(ui::DIM, "nothing selected"));
            return Ok(());
        }
        if !yes {
            eprintln!("{}", ui::epaint(ui::BOLD, "will delete:"));
            for n in &selected {
                eprintln!("  {}", ui::epaint(ui::CYAN_BOLD, n));
            }
            if !ui::confirm("proceed?")? {
                eprintln!("{}", ui::epaint(ui::DIM, "aborted"));
                return Ok(());
            }
        }
        selected
    };

    for name in &to_delete {
        match store::delete_session(name) {
            Ok(_) => ui::done("deleted", name),
            Err(e) => eprintln!("{} {name}: {e}", ui::epaint(ui::RED_BOLD, "error:")),
        }
    }
    Ok(())
}

fn unfinished_tasks(name: &str) -> usize {
    workspace::read_tasks_board(name)
        .map(|b| b.unfinished_count())
        .unwrap_or(0)
}

fn names(rows: &[Candidate]) -> Vec<String> {
    rows.iter().map(|c| c.name.to_owned()).collect()
}

/// Resolve selection indices against the collectable sessions only.
fn select_from_line(line: &str, rows: &[String]) -> Vec<String> {
    if line.eq_ignore_ascii_case("a") {
        return rows.to_vec();
    }
    line.split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .filter_map(|index| index.checked_sub(1).and_then(|index| rows.get(index)))
        .cloned()
        .collect()
}

fn print_collectable(rows: &[Candidate]) {
    for (i, c) in rows.iter().enumerate() {
        eprintln!(
            "{}",
            ui::session_row(
                Some(i + 1),
                c.name,
                &store::format_ts(&c.meta.last_access),
                &c.meta.origin_pwd,
                ""
            )
        );
    }
}

fn print_kept(rows: &[Candidate]) {
    for c in rows {
        let marker = format!(
            "  {}",
            ui::epaint(ui::YELLOW, &format!("({} unfinished)", c.unfinished))
        );
        eprintln!(
            "{}",
            ui::session_row(
                None,
                c.name,
                &store::format_ts(&c.meta.last_access),
                &c.meta.origin_pwd,
                &marker
            )
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scaffold_session, with_csm_home, write_board};
    use serial_test::serial;

    fn backdate(name: &str, days_ago: i64) {
        let ts = (Local::now() - chrono::Duration::days(days_ago)).to_rfc3339();
        store::set_last_access(name, ts).unwrap();
    }

    fn dir_exists(name: &str) -> bool {
        store::session_dir(name)
            .map(|d| d.exists())
            .unwrap_or(false)
    }

    #[test]
    #[serial]
    fn deletes_unpinned_older_than_threshold_and_keeps_recent() {
        with_csm_home(|_dir| {
            scaffold_session("old");
            backdate("old", 10);
            scaffold_session("recent");
            run(Some(5), true).unwrap();
            assert!(!dir_exists("old"), "old unpinned session should be gc'd");
            assert!(dir_exists("recent"), "recent session should be kept");
            assert!(
                store::require_session("old").is_err(),
                "index entry removed"
            );
            assert!(store::require_session("recent").is_ok(), "index entry kept");
        });
    }

    #[test]
    #[serial]
    fn skips_pinned_sessions() {
        with_csm_home(|_dir| {
            scaffold_session("pinned");
            backdate("pinned", 10);
            store::set_pinned("pinned", true).unwrap();
            run(Some(5), true).unwrap();
            assert!(dir_exists("pinned"), "pinned session must never be gc'd");
            assert!(store::require_session("pinned").is_ok(), "index entry kept");
        });
    }

    #[test]
    #[serial]
    fn skips_archived_sessions() {
        with_csm_home(|_dir| {
            scaffold_session("archived");
            write_board("archived", "## Done\n- 001 x\n");
            store::set_archived("archived", true).unwrap();
            backdate("archived", 10);
            run(Some(5), true).unwrap();
            assert!(
                dir_exists("archived"),
                "archived session must never be gc'd"
            );
            let meta = store::require_session("archived").expect("index entry kept");
            assert!(meta.is_archived());
        });
    }

    #[test]
    #[serial]
    fn threshold_is_inclusive_days() {
        with_csm_home(|_dir| {
            scaffold_session("edge");
            backdate("edge", 5);
            run(Some(5), true).unwrap();
            assert!(
                !dir_exists("edge"),
                "exactly N days old meets the `>= N` threshold"
            );
        });
    }

    #[test]
    #[serial]
    fn no_unpinned_candidates_is_noop() {
        with_csm_home(|_dir| {
            scaffold_session("kept");
            store::set_pinned("kept", true).unwrap();
            run(Some(1), true).unwrap();
            assert!(dir_exists("kept"));
            assert!(store::require_session("kept").is_ok());
        });
    }

    #[test]
    #[serial]
    fn keeps_sessions_with_unfinished_tasks_under_yes() {
        with_csm_home(|_dir| {
            scaffold_session("busy");
            write_board(
                "busy",
                "## Open\n- 001 live\n\n## Pending review\n- 002 live\n\n\
                 ## Done\n- 000 old\n",
            );
            backdate("busy", 10);
            scaffold_session("idle");
            backdate("idle", 10);
            run(Some(5), true).unwrap();
            assert!(dir_exists("busy"), "unfinished tasks keep the session");
            assert!(store::require_session("busy").is_ok(), "index entry kept");
            assert!(!dir_exists("idle"), "an empty board is still collected");
        });
    }

    #[test]
    #[serial]
    fn pending_review_and_pending_fix_count_as_unfinished() {
        with_csm_home(|_dir| {
            for (name, section) in [("review", "Pending review"), ("fix", "Pending fix")] {
                scaffold_session(name);
                write_board(name, &format!("## {section}\n- 001 live\n"));
                backdate(name, 10);
            }
            run(Some(5), true).unwrap();
            for name in ["review", "fix"] {
                assert!(dir_exists(name), "{name}: not-Done section keeps it");
                assert!(store::require_session(name).is_ok());
            }
        });
    }

    #[test]
    #[serial]
    fn all_done_board_is_collected() {
        with_csm_home(|_dir| {
            scaffold_session("finished");
            write_board("finished", "## Done\n- 001 x\n- 002 y\n");
            backdate("finished", 10);
            run(Some(5), true).unwrap();
            assert!(
                !dir_exists("finished"),
                "an all-Done board is history, not live work"
            );
            assert!(store::require_session("finished").is_err());
        });
    }

    #[test]
    #[serial]
    fn boardless_session_is_collected() {
        with_csm_home(|_dir| {
            store::touch_session("ghost", "/o").unwrap();
            backdate("ghost", 10);
            run(Some(5), true).unwrap();
            assert!(store::require_session("ghost").is_err());
        });
    }

    #[test]
    fn selection_resolves_only_the_given_rows() {
        let rows = vec!["a".to_string(), "b".to_string()];
        assert_eq!(select_from_line("a", &rows), vec!["a", "b"]);
        assert_eq!(select_from_line("A", &rows), vec!["a", "b"]);
        assert_eq!(select_from_line("2, 1", &rows), vec!["b", "a"]);
        assert!(select_from_line("3", &rows).is_empty(), "out of range");
        assert!(select_from_line("x", &rows).is_empty(), "not a selection");
        assert!(select_from_line("a", &[]).is_empty());
    }
}
