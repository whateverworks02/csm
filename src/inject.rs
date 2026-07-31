//! Idempotent injection of the csm working-mode prompt into the global
//! `~/.claude/CLAUDE.md` (via `csm init`). The block is wrapped in marker
//! comments so re-running refreshes it in place.

use crate::prompt::{csm_block, CSM_MARK_BEGIN, CSM_MARK_END};
use crate::ui;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the global user claude config dir (`~/.claude`).
pub fn claude_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".claude"))
}

/// Path to the global user CLAUDE.md (`~/.claude/CLAUDE.md`).
pub fn claude_md_path() -> Result<PathBuf> {
    Ok(claude_dir()?.join("CLAUDE.md"))
}

/// Path to the pi global agent dir (`~/.pi/agent`) - the direct analog of
/// Claude's `~/.claude/CLAUDE.md`. See [`pi_context_target`] for the
/// precedence we mirror when picking which file to write.
pub fn pi_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".pi").join("agent"))
}

/// pi's context-file precedence, mirroring
/// `@earendil-works/pi-coding-agent`'s `dist/core/resource-loader.js`. pi reads
/// the first match and ignores the rest, so csm must write into whichever file
/// pi will actually load.
const PI_CONTEXT_CANDIDATES: [&str; 4] = ["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// Pick the file pi will actually load from `~/.pi/agent/`. First-match-wins
/// over [`PI_CONTEXT_CANDIDATES`]; if none exists yet, default to `CLAUDE.md`
/// (fresh-install behavior) - otherwise writing into `CLAUDE.md` when
/// `AGENTS.md` exists would be silently ignored.
pub fn pi_context_target() -> Result<PathBuf> {
    let dir = pi_dir()?;
    for name in PI_CONTEXT_CANDIDATES {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Ok(dir.join("CLAUDE.md"))
}

/// Inject (or refresh) the csm block into `path`. Creates the file and parent
/// dirs if missing. Idempotent. Returns (path, modified).
pub fn inject_file(path: &Path) -> Result<(PathBuf, bool)> {
    let home = crate::store::csm_home()?.display().to_string();
    let block = csm_block(&home);
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

/// Find the csm block's marker bounds in `content`: the byte offsets of
/// [`CSM_MARK_BEGIN`] and [`CSM_MARK_END`] when both are present and in order
/// (begin before end). Single source of truth for "is the block present?" -
/// shared by [`replace_or_prepend`] (the write path: rewrite vs prepend) and
/// [`prompt_block_present`] (the read path `doctor` uses), so install and
/// diagnose agree on what "present" means.
fn block_bounds(content: &str) -> Option<(usize, usize)> {
    let begin = content.find(CSM_MARK_BEGIN)?;
    let end = content.find(CSM_MARK_END)?;
    (end >= begin).then_some((begin, end))
}

/// Read-only check: does `path` contain the csm prompt block? Used by `doctor`;
/// mirrors the presence [`inject_file`] maintains (via [`block_bounds`]).
pub fn prompt_block_present(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| block_bounds(&s).is_some())
        .unwrap_or(false)
}

fn replace_or_prepend(existing: &str, block: &str) -> String {
    match block_bounds(existing) {
        Some((b, e)) => {
            let mut s = String::with_capacity(existing.len() + block.len());
            s.push_str(&existing[..b]);
            s.push_str(block);
            s.push_str(&existing[e + CSM_MARK_END.len()..]);
            s
        }
        None => {
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

/// Install pi's state-injection wiring: the csm working-mode block in pi's
/// global context file (`~/.pi/agent/AGENTS.md` or `CLAUDE.md` - whichever pi
/// loads first; see [`pi_context_target`]). pi discovers this file at launch
/// (no hook needed), so the per-session state snapshot is all that's passed at
/// launch time. Idempotent. This is `PiAgent::install`.
pub fn install_pi() -> Result<()> {
    let pi_md = pi_context_target()?;
    let (_, modified) = inject_file(&pi_md)?;
    if modified {
        ui::step(
            "injected",
            &format!("prompt into {}", ui::abbrev_path(&pi_md)),
        );
    } else {
        eprintln!(
            "{} {}",
            ui::epaint(ui::DIM, "prompt already present at"),
            ui::epaint(ui::DIM, &ui::abbrev_path(&pi_md)),
        );
    }
    Ok(())
}

/// Read-only check: is the `csm hook` SessionStart entry already wired into
/// `root`? Shared by [`ensure_sessionstart_hook`] (install) and `doctor`
/// (diagnose), so install and diagnose agree on what "present" means.
pub fn sessionstart_hook_present(root: &serde_json::Value) -> bool {
    const CMD: &str = "csm hook";
    root.get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(|s| s.as_array())
        .is_some_and(|groups| {
            groups.iter().any(|g| {
                g.get("matcher") == Some(&serde_json::json!(""))
                    && g.get("hooks").and_then(|h| h.as_array()).is_some_and(|hs| {
                        hs.iter().any(|h| {
                            h.get("type") == Some(&serde_json::json!("command"))
                                && h.get("command") == Some(&serde_json::json!(CMD))
                        })
                    })
            })
        })
}

/// Add a SessionStart hook (`csm hook`) to the settings if not already present.
/// Returns true if the settings were modified.
fn ensure_sessionstart_hook(root: &mut serde_json::Value) -> bool {
    const CMD: &str = "csm hook";
    if sessionstart_hook_present(root) {
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

/// Read-only check: is the `csm` binary resolvable as a bare command on PATH
/// (so the hook's `csm hook` will resolve)? Shared by `csm init` (install-time
/// warning) and `doctor` (diagnose), like the other wiring presence checks.
/// Uses `which csm` rather than `current_exe()`: the hook invokes csm by bare
/// name, so only a PATH lookup reflects what the hook will actually see.
pub fn which_csm() -> Option<PathBuf> {
    Command::new("which")
        .arg("csm")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod block_bounds {
        use super::*;

        #[test]
        fn ordered_markers_found() {
            let content = format!("pre\n{CSM_MARK_BEGIN}\nbody\n{CSM_MARK_END}\nsuf");
            let (b, e) = block_bounds(&content).expect("present and ordered");
            assert!(b < e);
            assert!(content[b..].starts_with(CSM_MARK_BEGIN));
            assert!(content[e..].starts_with(CSM_MARK_END));
        }

        #[test]
        fn end_before_begin_is_none() {
            // Both markers present but reversed: must not match.
            let content = format!("{CSM_MARK_END} {CSM_MARK_BEGIN}");
            assert!(block_bounds(&content).is_none());
        }

        #[test]
        fn missing_either_marker_is_none() {
            assert!(block_bounds(CSM_MARK_BEGIN).is_none());
            assert!(block_bounds(CSM_MARK_END).is_none());
            assert!(block_bounds("no markers here").is_none());
        }
    }

    mod replace_or_prepend {
        use super::*;

        #[test]
        fn replaces_existing_block_in_place() {
            let existing = format!("before\n{CSM_MARK_BEGIN}\nOLD\n{CSM_MARK_END}\nafter");
            let block = csm_block("/data/csm");
            let out = replace_or_prepend(&existing, &block);
            assert!(out.starts_with(&format!("before\n{CSM_MARK_BEGIN}")));
            assert!(out.ends_with(&format!("{CSM_MARK_END}\nafter")));
            assert!(!out.contains("OLD"));
            assert!(out.contains("/data/csm"));
        }

        #[test]
        fn empty_or_whitespace_prepends_block_only() {
            let block = csm_block("/h");
            assert_eq!(replace_or_prepend("", &block), format!("{block}\n"));
            assert_eq!(replace_or_prepend("   \n  ", &block), format!("{block}\n"));
        }

        #[test]
        fn non_empty_without_block_prepends_with_blank_separator() {
            let block = csm_block("/h");
            let out = replace_or_prepend("existing content", &block);
            assert_eq!(out, format!("{block}\n\nexisting content"));
        }

        #[test]
        fn replacing_current_block_is_idempotent() {
            let block = csm_block("/h");
            // A file already holding this exact block is unchanged by another
            // replace_or_prepend pass.
            assert_eq!(replace_or_prepend(&block, &block), block);
        }
    }

    mod sessionstart_hook {
        use super::*;

        fn wired() -> serde_json::Value {
            serde_json::json!({
                "hooks": {
                    "SessionStart": [
                        { "matcher": "", "hooks": [{ "type": "command", "command": "csm hook" }] }
                    ]
                }
            })
        }

        #[test]
        fn present_detected() {
            assert!(sessionstart_hook_present(&wired()));
        }

        #[test]
        fn absent_when_no_hooks_key() {
            assert!(!sessionstart_hook_present(&serde_json::json!({})));
        }

        #[test]
        fn wrong_command_not_detected() {
            let v = serde_json::json!({
                "hooks": { "SessionStart": [
                    { "matcher": "", "hooks": [{ "type": "command", "command": "other" }] }
                ] }
            });
            assert!(!sessionstart_hook_present(&v));
        }

        #[test]
        fn wrong_matcher_not_detected() {
            let v = serde_json::json!({
                "hooks": { "SessionStart": [
                    { "matcher": "Editor", "hooks": [{ "type": "command", "command": "csm hook" }] }
                ] }
            });
            assert!(!sessionstart_hook_present(&v));
        }

        #[test]
        fn sessionstart_not_array_not_detected() {
            let v = serde_json::json!({ "hooks": { "SessionStart": "nope" } });
            assert!(!sessionstart_hook_present(&v));
        }

        #[test]
        fn matches_within_multiple_groups() {
            let v = serde_json::json!({
                "hooks": { "SessionStart": [
                    { "matcher": "Editor", "hooks": [{ "type": "command", "command": "x" }] },
                    { "matcher": "", "hooks": [{ "type": "command", "command": "csm hook" }] }
                ] }
            });
            assert!(sessionstart_hook_present(&v));
        }

        #[test]
        fn ensure_adds_when_absent() {
            let mut root = serde_json::json!({});
            assert!(ensure_sessionstart_hook(&mut root));
            assert!(sessionstart_hook_present(&root));
        }

        #[test]
        fn ensure_is_noop_when_present() {
            let mut root = wired();
            let before = root.clone();
            assert!(!ensure_sessionstart_hook(&mut root));
            assert_eq!(root, before);
        }

        #[test]
        fn ensure_adds_into_existing_hooks_object() {
            // hooks present but no SessionStart array -> added, other hooks kept.
            let mut root = serde_json::json!({ "hooks": { "Stop": [] } });
            assert!(ensure_sessionstart_hook(&mut root));
            assert!(sessionstart_hook_present(&root));
            assert!(root["hooks"]["Stop"].is_array());
        }
    }
}
