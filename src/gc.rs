//! Garbage collection for unpinned sessions. Pinned sessions are never listed
//! or deleted by gc. Deletion is a hard delete (workspace dir + index entry).

use crate::store::{self, SessionMeta};
use crate::ui;
use anyhow::Result;
use chrono::Local;

pub fn run(older_than: Option<u64>, yes: bool) -> Result<()> {
    let idx = store::load_index()?;
    let now = Local::now();

    let mut candidates: Vec<(String, SessionMeta)> = idx
        .sessions
        .iter()
        .filter(|(_, m)| !m.pinned)
        .filter(|(_, m)| {
            older_than.is_none_or(|d| {
                store::parse_time(&m.last_access)
                    .map(|t| (now - t).num_days() >= d as i64)
                    .unwrap_or(false)
            })
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    candidates.sort_by(|a, b| b.1.last_access.cmp(&a.1.last_access));

    if candidates.is_empty() {
        eprintln!(
            "{}",
            ui::epaint(ui::DIM, "no unpinned sessions to garbage-collect.")
        );
        return Ok(());
    }

    let to_delete: Vec<String> = if let Some(d) = older_than {
        eprintln!(
            "{}",
            ui::epaint(
                ui::BOLD,
                &format!("unpinned sessions not accessed in the last {d} day(s):")
            ),
        );
        print_list(&candidates);
        if yes || ui::confirm("delete all of the above?")? {
            candidates.iter().map(|(k, _)| k.clone()).collect()
        } else {
            eprintln!("{}", ui::epaint(ui::DIM, "aborted"));
            return Ok(());
        }
    } else {
        eprintln!("{}", ui::epaint(ui::BOLD, "unpinned sessions:"));
        print_list(&candidates);
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
        let selected: Vec<String> = if line.eq_ignore_ascii_case("a") {
            candidates.iter().map(|(k, _)| k.clone()).collect()
        } else {
            let mut out = Vec::new();
            for part in line.split(',') {
                let part = part.trim();
                if let Ok(i) = part.parse::<usize>() {
                    if i >= 1 && i <= candidates.len() {
                        out.push(candidates[i - 1].0.clone());
                    }
                }
            }
            out
        };
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

fn print_list(rows: &[(String, SessionMeta)]) {
    for (i, (name, m)) in rows.iter().enumerate() {
        let last = store::format_ts(&m.last_access);
        eprintln!(
            "  {}  {}  {}  {}",
            ui::epaint(ui::DIM, &format!("{:>2}", i + 1)),
            ui::epaint(ui::CYAN_BOLD, &format!("{:<20}", name)),
            ui::epaint(ui::DIM, &format!("{:<16}", last)),
            ui::epaint(ui::DIM, &ui::abbrev_home(&m.origin_pwd)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scaffold_session, with_csm_home};
    use serial_test::serial;

    /// Set `name`'s `last_access` to `days_ago` days before now, as RFC3339
    /// (what `now_iso` writes and `parse_time` reads). `last_access` is `pub`.
    fn backdate(name: &str, days_ago: i64) {
        let mut idx = store::load_index().unwrap();
        let ts = (Local::now() - chrono::Duration::days(days_ago)).to_rfc3339();
        idx.sessions.get_mut(name).unwrap().last_access = ts;
        store::save_index(&idx).unwrap();
    }

    fn dir_exists(name: &str) -> bool {
        store::session_dir(name)
            .map(|d| d.exists())
            .unwrap_or(false)
    }

    /// `run(Some(d), true)` is the stdin-free path: `yes=true` short-circuits
    /// `ui::confirm`. (`run(None, _)` always reads stdin for index selection ->
    /// not testable without a stdin seam; out of scope.)
    #[test]
    #[serial]
    fn deletes_unpinned_older_than_threshold_and_keeps_recent() {
        with_csm_home(|_dir| {
            scaffold_session("old");
            backdate("old", 10);
            scaffold_session("recent"); // last_access = now
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
            // Only a pinned session -> no candidates -> early no-op.
            scaffold_session("kept");
            store::set_pinned("kept", true).unwrap();
            run(Some(1), true).unwrap();
            assert!(dir_exists("kept"));
            assert!(store::require_session("kept").is_ok());
        });
    }
}
