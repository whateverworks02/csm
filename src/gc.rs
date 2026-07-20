//! Garbage collection for unpinned sessions. Pinned sessions are never listed
//! or deleted by gc. Deletion is a hard delete (workspace dir + index entry).

use crate::store::{self, SessionMeta};
use anyhow::Result;
use chrono::Local;
use std::io::Write;

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
        println!("no unpinned sessions to garbage-collect.");
        return Ok(());
    }

    let to_delete: Vec<String> = if let Some(d) = older_than {
        println!(
            "unpinned sessions not accessed in the last {} day(s):",
            d
        );
        print_list(&candidates);
        if yes || confirm("delete all of the above?")? {
            candidates.iter().map(|(k, _)| k.clone()).collect()
        } else {
            println!("aborted");
            return Ok(());
        }
    } else {
        println!("unpinned sessions:");
        print_list(&candidates);
        print!("\nselect indices to delete (comma-separated), 'a' for all, 'q' to quit: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let line = line.trim();
        if line.is_empty() || line.eq_ignore_ascii_case("q") {
            println!("aborted");
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
            println!("nothing selected");
            return Ok(());
        }
        if !yes {
            println!("\nwill delete:");
            for n in &selected {
                println!("  - {}", n);
            }
            if !confirm("proceed?")? {
                println!("aborted");
                return Ok(());
            }
        }
        selected
    };

    for name in &to_delete {
        match store::delete_session(name) {
            Ok(_) => println!("deleted: {}", name),
            Err(e) => eprintln!("failed to delete {}: {}", name, e),
        }
    }
    Ok(())
}

fn print_list(rows: &[(String, SessionMeta)]) {
    for (i, (name, m)) in rows.iter().enumerate() {
        let last = store::format_last_access(&m.last_access);
        println!("  [{}] {:<20} {:<20} {}", i + 1, name, last, m.origin_pwd);
    }
}

pub fn confirm(msg: &str) -> Result<bool> {
    print!("{} [y/N] ", msg);
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().eq_ignore_ascii_case("y"))
}
