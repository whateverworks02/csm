//! Idempotent injection of the csm working-mode prompt into the global
//! `~/.claude/CLAUDE.md` (via `csm init`). The block is wrapped in marker
//! comments so re-running refreshes it in place.

use crate::prompt::{csm_block, CSM_MARK_BEGIN, CSM_MARK_END};
use crate::ui;
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
/// dirs if missing. Idempotent. Returns (path, modified).
pub fn inject_file(path: &Path) -> Result<(PathBuf, bool)> {
    let block = csm_block();
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let new_content = replace_or_prepend(&existing, &block);
    let modified = new_content != existing;
    if modified {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &new_content)?;
    }
    Ok((path.to_path_buf(), modified))
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

/// Install Claude Code's state-injection wiring: the `SessionStart` hook in
/// `~/.claude/settings.json` and the csm working-mode block in
/// `~/.claude/CLAUDE.md`. Idempotent - leaves all other settings/content
/// untouched. This is `ClaudeAgent::install`.
pub fn install_claude() -> Result<()> {
    let claude_dir = claude_dir()?;
    std::fs::create_dir_all(&claude_dir)?;
    let settings_path = claude_dir.join("settings.json");

    // 1. Install the SessionStart hook (idempotent).
    let mut root: serde_json::Value = if settings_path.exists() {
        let data = std::fs::read_to_string(&settings_path)
            .with_context(|| format!("reading {}", settings_path.display()))?;
        if data.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&data)
                .with_context(|| format!("parsing {}", settings_path.display()))?
        }
    } else {
        serde_json::json!({})
    };
    if ensure_sessionstart_hook(&mut root) {
        std::fs::write(&settings_path, serde_json::to_string_pretty(&root)?)?;
        ui::step(
            "wrote",
            &format!("SessionStart hook to {}", ui::abbrev_path(&settings_path)),
        );
    } else {
        eprintln!(
            "{} {}",
            ui::epaint(ui::DIM, "SessionStart hook already present at"),
            ui::epaint(ui::DIM, &ui::abbrev_path(&settings_path)),
        );
    }

    // 2. Inject the csm working-mode prompt into the global CLAUDE.md.
    let claude_md = claude_md_path()?;
    let (_, modified) = inject_file(&claude_md)?;
    if modified {
        ui::step(
            "injected",
            &format!("prompt into {}", ui::abbrev_path(&claude_md)),
        );
    } else {
        eprintln!(
            "{} {}",
            ui::epaint(ui::DIM, "prompt already present at"),
            ui::epaint(ui::DIM, &ui::abbrev_path(&claude_md)),
        );
    }
    Ok(())
}

/// Add a SessionStart hook (`csm hook`) to the settings if not already present.
/// Returns true if the settings were modified.
fn ensure_sessionstart_hook(root: &mut serde_json::Value) -> bool {
    const CMD: &str = "csm hook";

    let already = root
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(|s| s.as_array())
        .is_some_and(|groups| {
            groups.iter().any(|g| {
                g.get("matcher") == Some(&serde_json::json!(""))
                    && g
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .is_some_and(|hs| {
                            hs.iter().any(|h| {
                                h.get("type") == Some(&serde_json::json!("command"))
                                    && h.get("command") == Some(&serde_json::json!(CMD))
                            })
                        })
            })
        });
    if already {
        return false;
    }

    if root.get("hooks").is_none() {
        root["hooks"] = serde_json::json!({});
    }
    if !root["hooks"]["SessionStart"].is_array() {
        root["hooks"]["SessionStart"] = serde_json::json!([]);
    }
    let arr = root["hooks"]["SessionStart"]
        .as_array_mut()
        .expect("SessionStart is an array");
    arr.push(serde_json::json!({
        "matcher": "",
        "hooks": [{ "type": "command", "command": CMD }]
    }));
    true
}
