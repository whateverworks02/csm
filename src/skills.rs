//! csm-shipped skills, distributed by `csm init`. Currently one: `/csm-plan`,
//! the authoring discipline for the coordinator's architect pass (grill ->
//! one-pager -> executor-grade tasks) - the variance-prone handoff in the
//! tiered pipeline (`notes/scout-plan-skills.md`).
//!
//! Mirrors `prompt.rs`: the const here is the single source of truth, versioned
//! with the tool and rendered at deploy time. Every target below is csm-owned -
//! `csm init` converges it to the const unconditionally (rendered, never
//! user-edited; unlike session files). The update loop is identical to the
//! prompt: edit `skills.rs` -> `cargo build` -> `csm init`.
//!
//! Render targets (both from [`PLAN_SKILL_MD`], so deployment duplication is
//! not meaning duplication):
//! - `~/.claude/skills/csm-plan/SKILL.md` - a real Claude skill: slash command
//!   plus auto-trigger via the frontmatter `description` (the always-loaded
//!   context pointer).
//! - `~/.csm/skills/plan.md` - vendor-neutral home: the human-readable copy,
//!   readable by any agent or human that goes looking.
//!
//! pi and codex get nothing: they have no skill mechanism, and a pointer line
//! in their always-loaded prompt block was judged not worth its context load -
//! the skill is Claude-reachable only for now.

use crate::inject::claude_dir;
use crate::store;
use crate::ui;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// The `/csm-plan` skill. The frontmatter `description` is the top-level
/// context pointer (writing-for-agents): front-loads the trigger branch
/// (creating/planning csm tasks), names the pass with leading words (grill,
/// one-pager, executor-grade). The body is steps that each end on a completion
/// criterion; the csm prompt owns the workflow (roles, board moves) and is not
/// restated.
pub const PLAN_SKILL_MD: &str = r#"---
name: csm-plan
description: Creating csm tasks for a new mission - grill the human, write the state.md one-pager, decompose into executor-grade tasks
---

# csm-plan

Architect pass over a csm session: `notes/` + the human's mission in; a rewritten `state.md` + executor-grade tasks out. The csm prompt owns the workflow (roles, board moves); this skill owns authoring quality. Inputs are `state.md`, `notes/`, and the mission - plan from notes, and read code only for a targeted spot-check of a load-bearing note claim marked uncertain.

## Steps

1. **Grill.** Enumerate every decision the notes leave open - the decisions the ACs will rest on. Ask the human in one batch: each question names the decision, the options the notes support, and what breaks under each option. The grill is the only fuse against scout gaps - stop only when every planned AC has an unambiguous basis (a note with evidence, or an answer).
2. **One-pager.** Rewrite `state.md` Context: mission in one line, the chosen approach, one line per rejected alternative with the why, current focus. Refresh Key links.
3. **Decompose.** One task per worker run. Each task file states its preconditions - which tasks and contracts it assumes - as its first Scope line; csm has no dependency graph, the precondition line is the mechanism.
4. **SOP for a weak executor.** Every step ends on a completion criterion the executor itself can check (a command output, a file state), never on judgment. Exact commands where a command exists, exact paths - `src/store.rs`, never "the relevant file". Guardrails as targets: which files to touch, which convention to match. Every AC verifiable by the reviewer without asking the worker.
5. **Fuse.** A decision with neither note basis nor human answer goes back to the human or out as a follow-up scout question. Inventing a basis is the one way this pass fails silently.
6. **Done when:** every task file has Scope/AC/SOP, every INDEX line sits under Open, `state.md` is the one-pager, and zero grill questions stand unanswered.
"#;

/// Claude render target: `~/.claude/skills/csm-plan/SKILL.md`.
pub fn claude_skill_path() -> Result<PathBuf> {
    Ok(claude_dir()?
        .join("skills")
        .join("csm-plan")
        .join("SKILL.md"))
}

/// Vendor-neutral render target: `~/.csm/skills/plan.md`.
pub fn vendor_neutral_path() -> Result<PathBuf> {
    Ok(store::csm_home()?.join("skills").join("plan.md"))
}

/// Read-only check: is the skill at `path` deployed and current (content ==
/// [`PLAN_SKILL_MD`])? Shared by [`deploy`] (install) and `doctor`'s wiring
/// check (diagnose), so the writer and the checker cannot drift - the same
/// contract as `inject::prompt_block_present`.
pub fn skill_current(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|c| c == PLAN_SKILL_MD)
}

/// Write `PLAN_SKILL_MD` to `path`, converging unconditionally: a content-equal
/// run writes nothing; a stale (post-upgrade) or user-edited copy is
/// overwritten - the `csm-` namespace is rendered, never hand-maintained.
/// Prints a `wrote`/`current` status line.
fn deploy(path: &Path) -> Result<()> {
    if skill_current(path) {
        eprintln!(
            "{} {}",
            ui::epaint(ui::DIM, "csm-plan skill already current at"),
            ui::epaint(ui::DIM, &ui::abbrev_path(path)),
        );
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, PLAN_SKILL_MD)?;
    ui::step(
        "wrote",
        &format!("csm-plan skill to {}", ui::abbrev_path(path)),
    );
    Ok(())
}

/// Deploy the Claude skill surface (`ClaudeAgent::install` calls this).
pub fn install_claude() -> Result<()> {
    deploy(&claude_skill_path()?)
}

/// Deploy the vendor-neutral skill home - the human-readable copy
/// (`agent::install_all` calls this once per `csm init`).
pub fn install_vendor_neutral() -> Result<()> {
    deploy(&vendor_neutral_path()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_isolated_home;
    use serial_test::serial;
    use std::fs;

    mod plan_skill_md {
        use super::*;

        #[test]
        fn frontmatter_name_and_frontloaded_description() {
            assert!(PLAN_SKILL_MD.starts_with("---\n"));
            let frontmatter = PLAN_SKILL_MD
                .strip_prefix("---\n")
                .and_then(|s| s.split_once("\n---\n"))
                .map(|(fm, _)| fm)
                .expect("closed frontmatter block");
            assert!(frontmatter.contains("name: csm-plan"));
            // The description is the always-loaded trigger pointer: it must
            // front-load the trigger branch.
            let desc = frontmatter
                .lines()
                .find(|l| l.starts_with("description:"))
                .expect("a description");
            assert!(
                desc.starts_with("description: Creating csm tasks"),
                "description must front-load the trigger, got: {desc}"
            );
        }

        #[test]
        fn body_is_steps_with_completion_criteria() {
            // Vendor-neutral: paths are session-relative, so no absolute home.
            assert!(!PLAN_SKILL_MD.contains("~/.csm"));
            assert!(!PLAN_SKILL_MD.contains("$CSM_HOME"));
            for step in [
                "## Steps",
                "**Grill.**",
                "**One-pager.**",
                "**Decompose.**",
                "**Fuse.**",
                "**Done when:**",
            ] {
                assert!(PLAN_SKILL_MD.contains(step), "missing {step}");
            }
            // The workflow (roles, board moves) belongs to the csm prompt;
            // the skill must not restate it.
            assert!(!PLAN_SKILL_MD.contains("coordinator"));
            assert!(!PLAN_SKILL_MD.contains("Pending review"));
        }
    }

    #[test]
    #[serial]
    fn install_claude_writes_converges_and_restores() {
        with_isolated_home(|_home| {
            // Fresh: writes the skill.
            install_claude().unwrap();
            let path = claude_skill_path().unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), PLAN_SKILL_MD);

            // Stale/user-edited copy: converged back to the const.
            fs::write(&path, "user edits\n").unwrap();
            install_claude().unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), PLAN_SKILL_MD);
        });
    }

    #[test]
    #[serial]
    fn install_vendor_neutral_writes_under_csm_home() {
        with_isolated_home(|home| {
            install_vendor_neutral().unwrap();
            assert_eq!(
                fs::read_to_string(home.join("skills").join("plan.md")).unwrap(),
                PLAN_SKILL_MD
            );
        });
    }

    #[test]
    fn skill_current_is_exact_content_equality() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("SKILL.md");
        // Missing.
        assert!(!skill_current(&path));
        // Present but not the const (stale or user-edited).
        fs::write(&path, "old").unwrap();
        assert!(!skill_current(&path));
        // Exact match - the same predicate deploy and doctor share.
        fs::write(&path, PLAN_SKILL_MD).unwrap();
        assert!(skill_current(&path));
    }
}
