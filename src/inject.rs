//! Idempotent injection of the csm working-mode prompt into the global
//! `~/.claude/CLAUDE.md` (via `csm init`). The block is wrapped in marker
//! comments so re-running refreshes it in place.

use crate::prompt::{csm_block, CSM_MARK_BEGIN, CSM_MARK_END};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Path to the global user claude config dir (`~/.claude`).
pub fn claude_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".claude"))
}

/// Path to the global user CLAUDE.md (`~/.claude/CLAUDE.md`).
pub fn claude_md_path() -> Result<PathBuf> {
    Ok(claude_dir()?.join("CLAUDE.md"))
}

/// Inject (or refresh) the csm block into `path`. Creates the file and parent
/// dirs if missing. Idempotent. Returns the path.
pub fn inject_file(path: &Path) -> Result<PathBuf> {
    let block = csm_block();
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let new_content = replace_or_prepend(&existing, &block);
    if new_content != existing {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &new_content)?;
    }
    Ok(path.to_path_buf())
}

fn replace_or_prepend(existing: &str, block: &str) -> String {
    let begin_idx = existing.find(CSM_MARK_BEGIN);
    let end_idx = existing.find(CSM_MARK_END);
    match (begin_idx, end_idx) {
        (Some(b), Some(e)) if e >= b => {
            let mut s = String::with_capacity(existing.len() + block.len());
            s.push_str(&existing[..b]);
            s.push_str(block);
            s.push_str(&existing[e + CSM_MARK_END.len()..]);
            s
        }
        _ => {
            if existing.trim().is_empty() {
                format!("{}\n", block)
            } else {
                format!("{}\n\n{}", block, existing)
            }
        }
    }
}
