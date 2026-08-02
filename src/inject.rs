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

/// Path to the global user codex config dir (`~/.codex`), honoring `$CODEX_HOME`
/// (codex's own override, mirroring `$CSM_HOME` -> `~/.csm`). codex
/// auto-discovers `AGENTS.md` and `hooks.json` here - the direct analogs of
/// Claude's `~/.claude/CLAUDE.md` and `settings.json`.
pub fn codex_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CODEX_HOME") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".codex"))
}

/// codex's global instructions precedence: `AGENTS.override.md` (if present and
/// non-empty) else `AGENTS.md` - codex loads only the first non-empty file at
/// the global scope. Mirroring [`pi_context_target`], csm must write into
/// whichever file codex will actually load, else its block is silently ignored.
pub fn codex_agents_target() -> Result<PathBuf> {
    let dir = codex_dir()?;
    let override_md = dir.join("AGENTS.override.md");
    let override_loads = std::fs::read_to_string(&override_md).is_ok_and(|s| !s.trim().is_empty());
    if override_loads {
        return Ok(override_md);
    }
    Ok(dir.join("AGENTS.md"))
}

/// Path to the global user codex hooks file (`~/.codex/hooks.json`) - the
/// analog of Claude's `~/.claude/settings.json` hooks. codex loads hooks from
/// `hooks.json` or inline `[hooks]` in `config.toml`; csm uses the standalone
/// `hooks.json` so it never touches the user's `config.toml` (model/provider
/// settings live there).
pub fn codex_hooks_path() -> Result<PathBuf> {
    Ok(codex_dir()?.join("hooks.json"))
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

/// Read a JSON file as a `serde_json::Value`, returning `{}` if the file is
/// missing or empty. Shared by the agent hook installs (Claude `settings.json`,
/// codex `hooks.json`) so both tolerate an absent/blank file identically.
fn read_json_or_default(path: &Path) -> Result<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let data =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if data.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))
}

/// Ensure the `csm hook` SessionStart entry is present in the JSON file at
/// `path` (Claude `settings.json` or codex `hooks.json` - same hooks shape, so
/// [`ensure_sessionstart_hook`] works for both). Creates the file if missing;
/// leaves all other entries untouched. Prints a `wrote`/`already present`
/// status line. Returns whether the hook was newly written - codex uses this to
/// decide whether to print its trust hint.
fn ensure_sessionstart_hook_in_file(path: &Path) -> Result<bool> {
    let mut root = read_json_or_default(path)?;
    let written = ensure_sessionstart_hook(&mut root);
    if written {
        std::fs::write(path, serde_json::to_string_pretty(&root)?)?;
        ui::step(
            "wrote",
            &format!("SessionStart hook to {}", ui::abbrev_path(path)),
        );
    } else {
        eprintln!(
            "{} {}",
            ui::epaint(ui::DIM, "SessionStart hook already present at"),
            ui::epaint(ui::DIM, &ui::abbrev_path(path)),
        );
    }
    Ok(written)
}

/// Inject the csm working-mode prompt block into `path` and print an
/// `injected`/`already present` status line. Thin status wrapper around
/// [`inject_file`] shared by every agent install.
fn inject_prompt_block(path: &Path) -> Result<()> {
    let (_, modified) = inject_file(path)?;
    if modified {
        ui::step(
            "injected",
            &format!("prompt into {}", ui::abbrev_path(path)),
        );
    } else {
        eprintln!(
            "{} {}",
            ui::epaint(ui::DIM, "prompt already present at"),
            ui::epaint(ui::DIM, &ui::abbrev_path(path)),
        );
    }
    Ok(())
}

/// Install Claude Code's state-injection wiring: the `SessionStart` hook in
/// `~/.claude/settings.json` and the csm working-mode block in
/// `~/.claude/CLAUDE.md`. Idempotent - leaves all other settings/content
/// untouched. This is `ClaudeAgent::install`.
pub fn install_claude() -> Result<()> {
    let claude_dir = claude_dir()?;
    std::fs::create_dir_all(&claude_dir)?;
    ensure_sessionstart_hook_in_file(&claude_dir.join("settings.json"))?;
    inject_prompt_block(&claude_md_path()?)?;
    Ok(())
}

/// Install pi's state-injection wiring: the csm working-mode block in pi's
/// global context file (`~/.pi/agent/AGENTS.md` or `CLAUDE.md` - whichever pi
/// loads first; see [`pi_context_target`]). pi discovers this file at launch
/// (no hook needed), so the per-session state snapshot is all that's passed at
/// launch time. Idempotent. This is `PiAgent::install`.
pub fn install_pi() -> Result<()> {
    inject_prompt_block(&pi_context_target()?)?;
    Ok(())
}

/// Install codex's state-injection wiring: a `SessionStart` hook in
/// `~/.codex/hooks.json` and the csm working-mode block in `~/.codex/AGENTS.md`
/// (or `AGENTS.override.md` if codex loads that - see [`codex_agents_target`]).
/// Idempotent - leaves all other hooks/content untouched. This is
/// `CodexAgent::install`.
///
/// Unlike Claude, codex requires non-managed command hooks to be reviewed and
/// trusted (against the hook's hash) via `/hooks` before they run; when the
/// hook is newly written `csm init` prints a trust hint so a fresh install
/// isn't silently inert. The `csm hook` handler is reused unchanged - codex's
/// SessionStart accepts the same JSON Claude does.
pub fn install_codex() -> Result<()> {
    let codex_dir = codex_dir()?;
    std::fs::create_dir_all(&codex_dir)?;
    let hook_written = ensure_sessionstart_hook_in_file(&codex_hooks_path()?)?;
    inject_prompt_block(&codex_agents_target()?)?;

    // codex skips non-managed command hooks until they are reviewed/trusted
    // via `/hooks`. Only hint when the hook was just written (re-runs don't
    // re-trigger trust review); if an existing hook is untrusted, codex itself
    // warns at startup.
    if hook_written {
        ui::warn(
            "codex requires hooks to be trusted: in a codex session run `/hooks` \
             and trust the `csm hook` SessionStart entry, or it will be skipped.",
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

    mod inject_file {
        use super::*;
        use crate::test_support::with_csm_home;
        use serial_test::serial;
        use std::fs;
        use tempfile::TempDir;

        /// Fresh temp `CLAUDE.md` target. The returned `TempDir` must stay
        /// bound for the test's scope - it owns the dir and cleans it on drop.
        fn temp_target() -> (TempDir, PathBuf) {
            let dir = TempDir::new().unwrap();
            let target = dir.path().join("CLAUDE.md");
            (dir, target)
        }

        #[test]
        #[serial]
        fn creates_and_prepends_block() {
            with_csm_home(|home| {
                let (_dir, target) = temp_target();
                let (_, modified) = inject_file(&target).unwrap();
                assert!(modified);
                let content = fs::read_to_string(&target).unwrap();
                assert!(content.contains(CSM_MARK_BEGIN));
                assert!(content.contains(CSM_MARK_END));
                // The block reflects the isolated $CSM_HOME (path dynamization).
                assert!(content.contains(&home.display().to_string()));
            });
        }

        #[test]
        #[serial]
        fn replaces_stale_block_no_duplicate() {
            with_csm_home(|_home| {
                let (_dir, target) = temp_target();
                let stale = format!("header\n{CSM_MARK_BEGIN}\nSTALE\n{CSM_MARK_END}\nfooter\n");
                fs::write(&target, &stale).unwrap();
                let (_, modified) = inject_file(&target).unwrap();
                assert!(modified);
                let content = fs::read_to_string(&target).unwrap();
                assert_eq!(content.matches(CSM_MARK_BEGIN).count(), 1);
                assert_eq!(content.matches(CSM_MARK_END).count(), 1);
                assert!(!content.contains("STALE"));
                assert!(content.contains("header"));
                assert!(content.contains("footer"));
            });
        }

        #[test]
        #[serial]
        fn idempotent_when_block_current() {
            with_csm_home(|_home| {
                let (_dir, target) = temp_target();
                inject_file(&target).unwrap();
                let after_first = fs::read_to_string(&target).unwrap();
                let (_, modified) = inject_file(&target).unwrap();
                assert!(!modified);
                assert_eq!(fs::read_to_string(&target).unwrap(), after_first);
            });
        }

        #[test]
        #[serial]
        fn prepends_before_existing_content() {
            with_csm_home(|_home| {
                let (_dir, target) = temp_target();
                fs::write(&target, "user content\n").unwrap();
                inject_file(&target).unwrap();
                let content = fs::read_to_string(&target).unwrap();
                assert!(content.starts_with(CSM_MARK_BEGIN));
                assert!(content.contains("user content"));
            });
        }
    }

    mod install_claude {
        use super::*;
        use crate::test_support::with_isolated_home;
        use serial_test::serial;
        use std::fs;

        #[test]
        #[serial]
        fn writes_hook_and_block() {
            with_isolated_home(|home| {
                install_claude().unwrap();
                let claude_dir = home.join(".claude");
                let root: serde_json::Value = serde_json::from_str(
                    &fs::read_to_string(claude_dir.join("settings.json")).unwrap(),
                )
                .unwrap();
                assert!(sessionstart_hook_present(&root));
                let md = fs::read_to_string(claude_dir.join("CLAUDE.md")).unwrap();
                assert!(md.contains(CSM_MARK_BEGIN));
                assert!(md.contains(CSM_MARK_END));
            });
        }

        #[test]
        #[serial]
        fn idempotent_no_duplicate_hook_or_block() {
            with_isolated_home(|home| {
                install_claude().unwrap();
                let s_before =
                    fs::read_to_string(home.join(".claude").join("settings.json")).unwrap();
                let m_before = fs::read_to_string(home.join(".claude").join("CLAUDE.md")).unwrap();
                // Second run must not duplicate the hook or the block.
                install_claude().unwrap();
                let s_after =
                    fs::read_to_string(home.join(".claude").join("settings.json")).unwrap();
                let m_after = fs::read_to_string(home.join(".claude").join("CLAUDE.md")).unwrap();
                assert_eq!(s_after, s_before);
                assert_eq!(m_after, m_before);
                assert_eq!(m_after.matches(CSM_MARK_BEGIN).count(), 1);
                let root: serde_json::Value = serde_json::from_str(&s_after).unwrap();
                assert_eq!(root["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
            });
        }
    }

    mod pi_context_target {
        use super::*;
        use crate::test_support::with_home;
        use serial_test::serial;
        use std::fs;

        #[test]
        #[serial]
        fn defaults_to_claude_md_when_empty() {
            with_home(|home| {
                let target = pi_context_target().unwrap();
                assert_eq!(target, home.join(".pi").join("agent").join("CLAUDE.md"));
            });
        }

        #[test]
        #[serial]
        fn prefers_agents_md() {
            with_home(|home| {
                let dir = home.join(".pi").join("agent");
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("AGENTS.md"), "x").unwrap();
                assert_eq!(pi_context_target().unwrap(), dir.join("AGENTS.md"));
            });
        }

        #[test]
        #[serial]
        fn first_match_wins_in_precedence_order() {
            with_home(|home| {
                let dir = home.join(".pi").join("agent");
                fs::create_dir_all(&dir).unwrap();
                // AGENTS.md (candidate 0) beats CLAUDE.md (candidate 2).
                fs::write(dir.join("CLAUDE.md"), "x").unwrap();
                fs::write(dir.join("AGENTS.md"), "x").unwrap();
                assert_eq!(pi_context_target().unwrap(), dir.join("AGENTS.md"));
            });
        }

        // The uppercase case-variant candidates (AGENTS.MD, CLAUDE.MD) exist for
        // pi compatibility but aren't portably testable: on a case-insensitive
        // FS (macOS default) CLAUDE.MD and CLAUDE.md are the same file, so
        // candidate 2 always shadows candidate 3. Different-basename precedence
        // (AGENTS.* > CLAUDE.*) is covered above and is FS-independent.
    }

    mod codex_dir {
        use super::*;
        use crate::test_support::{with_env, with_home, without_env};
        use serial_test::serial;

        #[test]
        #[serial]
        fn honors_codex_home() {
            with_home(|home| {
                with_env("CODEX_HOME", "/custom/codex", || {
                    assert_eq!(codex_dir().unwrap(), PathBuf::from("/custom/codex"));
                });
                without_env("CODEX_HOME", || {
                    assert_eq!(codex_dir().unwrap(), home.join(".codex"));
                });
            });
        }
    }

    mod codex_agents_target {
        use super::*;
        use crate::test_support::with_home;
        use serial_test::serial;
        use std::fs;

        #[test]
        #[serial]
        fn defaults_to_agents_md_when_empty() {
            with_home(|home| {
                let target = codex_agents_target().unwrap();
                assert_eq!(target, home.join(".codex").join("AGENTS.md"));
            });
        }

        #[test]
        #[serial]
        fn prefers_nonempty_override() {
            with_home(|home| {
                let dir = home.join(".codex");
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("AGENTS.override.md"), "x").unwrap();
                assert_eq!(
                    codex_agents_target().unwrap(),
                    dir.join("AGENTS.override.md")
                );
            });
        }

        #[test]
        #[serial]
        fn ignores_empty_override() {
            with_home(|home| {
                let dir = home.join(".codex");
                fs::create_dir_all(&dir).unwrap();
                // An empty override is skipped by Codex, so csm targets AGENTS.md.
                fs::write(dir.join("AGENTS.override.md"), "   \n  ").unwrap();
                assert_eq!(codex_agents_target().unwrap(), dir.join("AGENTS.md"));
            });
        }
    }

    mod install_codex {
        use super::*;
        use crate::test_support::with_isolated_home;
        use serial_test::serial;
        use std::fs;

        #[test]
        #[serial]
        fn writes_hook_and_block() {
            with_isolated_home(|home| {
                install_codex().unwrap();
                let codex_dir = home.join(".codex");
                let root: serde_json::Value = serde_json::from_str(
                    &fs::read_to_string(codex_dir.join("hooks.json")).unwrap(),
                )
                .unwrap();
                assert!(sessionstart_hook_present(&root));
                let md = fs::read_to_string(codex_dir.join("AGENTS.md")).unwrap();
                assert!(md.contains(CSM_MARK_BEGIN));
                assert!(md.contains(CSM_MARK_END));
            });
        }

        #[test]
        #[serial]
        fn idempotent_no_duplicate_hook_or_block() {
            with_isolated_home(|home| {
                install_codex().unwrap();
                let h_before = fs::read_to_string(home.join(".codex").join("hooks.json")).unwrap();
                let m_before = fs::read_to_string(home.join(".codex").join("AGENTS.md")).unwrap();
                // Second run must not duplicate the hook or the block.
                install_codex().unwrap();
                let h_after = fs::read_to_string(home.join(".codex").join("hooks.json")).unwrap();
                let m_after = fs::read_to_string(home.join(".codex").join("AGENTS.md")).unwrap();
                assert_eq!(h_after, h_before);
                assert_eq!(m_after, m_before);
                assert_eq!(m_after.matches(CSM_MARK_BEGIN).count(), 1);
                let root: serde_json::Value = serde_json::from_str(&h_after).unwrap();
                assert_eq!(root["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
            });
        }
    }
}
