//! csm-shipped planning and investigation skills, distributed by `csm init`.
//! The working protocol owns the workflow; these skills own authoring quality.
//!
//! Mirrors `prompt.rs`: each const here is the single source of truth, versioned
//! with the tool and rendered at deploy time. Every target below is csm-owned -
//! `csm init` converges it to the const unconditionally (rendered, never
//! user-edited; unlike session files). The update loop is identical to the
//! prompt: edit `skills.rs` -> `cargo build` -> `csm init`.
//!
//! Per skill, two render targets (both from the same const, so deployment
//! duplication is not meaning duplication):
//! - `~/.claude/skills/<id>/SKILL.md` - a real Claude skill: slash command
//!   plus auto-trigger via the frontmatter `description` (the always-loaded
//!   context pointer).
//! - `~/.csm/skills/<file>` - vendor-neutral home: the human-readable copy,
//!   readable by any agent or human that goes looking.
//!
//! pi and codex get nothing: they have no skill mechanism, and a pointer line
//! in their always-loaded prompt block was judged not worth its context load.
//! opencode needs nothing either, for the opposite reason: it auto-loads
//! `~/.claude/skills/` as external skills (verified via `opencode debug skill`),
//! so it reads the Claude-deployed copies as-is.

use crate::inject::claude_dir;
use crate::store;
use crate::ui;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Planning authoring guidance, triggered by a new mission or replan.
pub const PLAN_SKILL_MD: &str = r#"---
name: csm-plan
description: Creating csm tasks for a new mission or a replan - resolve decisions and write executable tasks
---

# csm-plan

Use the mission, `state.md`, the board, and relevant notes to plan the work. The csm prompt's working protocol owns record structure, dependencies, and lifecycle; this skill owns authoring quality.

## Steps

1. **Resolve decisions.** Identify decisions that affect the plan. Use available evidence; investigate missing facts with targeted code reads. Ask the user for unresolved choices requiring their judgment, grouping questions with options and tradeoffs. Continue when the proposed work has a supported scope; record unresolved prerequisites for any deferred work.
2. **Decompose.** Create or revise tasks around independently verifiable outcomes. Record each task's real preconditions in the board's `needs:` tail according to the working protocol. Each task has a bounded Scope and AC a reviewer can verify.
3. **Write the SOP.** Apply the working protocol's record depth. Name the files, operations, and conventions needed to execute the task, with exact commands where useful. Each step ends with an observable completion criterion; the procedure covers the task's AC.
4. **Update context.** Update `state.md` Context and Key links where the mission, approach, or current focus changed. Retain rationale needed to guide subsequent work; the resulting one-pager describes the current plan.
5. **Done when:** tasks created or revised in this pass have supported Scope, verifiable AC, executable SOP, and dependencies recorded under the working protocol. New tasks enter Open; existing tasks retain their status unless an actual lifecycle transition occurred. Unresolved prerequisites are explicit, and context reflects the plan.

"#;

/// Investigation authoring guidance, triggered by specific open questions.
pub const SCOUT_SKILL_MD: &str = r#"---
name: csm-scout
description: Exploring a codebase to answer specific open questions for a csm planning pass - write the notes an architect will plan from
---

# csm-scout

Investigate specific open questions and record evidence the planning pass can use.

## Steps

1. **Frame the question.** Identify the decision the requested investigation informs and the evidence needed to answer it. Read the files that can supply that evidence; finish when the question is answered or the missing evidence is identified.
2. **Record the findings.** Update the relevant note, or create one when the question has no existing home. Give the answer, locatable evidence (`path:line` for code claims), and relevant alternatives and tradeoffs. Distinguish observations from inferences; state specific unknowns and the evidence needed to resolve them. The reader can trace each consequential claim to its basis and distinguish options from chosen decisions.
3. **Register.** Update the note's one-line gist in `notes/INDEX.md` so the findings can be located.
4. **Done when:** each question in this investigation is answered with evidence or has an explicit evidence gap, and the findings are registered.

"#;

/// A csm-shipped skill: one const (the single source of truth), rendered by
/// `csm init` to both targets. Adding a skill = one const + one table row; the
/// deploy and doctor code paths iterate [`SKILLS`] unchanged.
pub struct SkillSpec {
    /// Claude skill id - the directory under `~/.claude/skills/` and the
    /// slash-command name (`/csm-plan`).
    pub id: &'static str,
    /// Vendor-neutral filename under `~/.csm/skills/`.
    pub vendor_file: &'static str,
    /// The skill body, frontmatter included.
    pub md: &'static str,
}

impl SkillSpec {
    /// Status-line label, shared by [`deploy`] and `doctor`'s check.
    pub fn label(&self) -> String {
        format!("{} skill", self.id)
    }
}

/// Every csm-shipped skill - one deploy code path, N skills.
pub const SKILLS: &[SkillSpec] = &[
    SkillSpec {
        id: "csm-plan",
        vendor_file: "plan.md",
        md: PLAN_SKILL_MD,
    },
    SkillSpec {
        id: "csm-scout",
        vendor_file: "scout.md",
        md: SCOUT_SKILL_MD,
    },
];

/// Claude render target: `~/.claude/skills/<id>/SKILL.md`.
pub fn claude_skill_path(id: &str) -> Result<PathBuf> {
    Ok(claude_dir()?.join("skills").join(id).join("SKILL.md"))
}

/// Vendor-neutral render target: `~/.csm/skills/<vendor_file>`.
pub fn vendor_neutral_path(vendor_file: &str) -> Result<PathBuf> {
    Ok(store::csm_home()?.join("skills").join(vendor_file))
}

/// Read-only check: is the skill at `path` deployed and current (content ==
/// its const)? Shared by [`deploy`] (install) and `doctor`'s wiring check
/// (diagnose), so the writer and the checker cannot drift - the same contract
/// as `inject::prompt_block_present`.
pub fn skill_current(path: &Path, md: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|c| c == md)
}

/// Write `md` to `path`, converging unconditionally: a content-equal run
/// writes nothing; a stale (post-upgrade) or user-edited copy is overwritten -
/// the `csm-` namespace is rendered, never hand-maintained. Prints a
/// `wrote`/`current` status line.
fn deploy(path: &Path, md: &str, label: &str) -> Result<()> {
    if skill_current(path, md) {
        eprintln!(
            "{} {}",
            ui::epaint(ui::DIM, &format!("{label} already current at")),
            ui::epaint(ui::DIM, &ui::abbrev_path(path)),
        );
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, md)?;
    ui::step("wrote", &format!("{label} to {}", ui::abbrev_path(path)));
    Ok(())
}

/// Deploy every skill to the surface `path_for` picks - the one install code
/// path both surfaces call. A failed deploy aborts the pass with the rest
/// undeployed; the converge-to-const contract makes the partial state
/// self-healing on the next `csm init`.
fn install_all_at(path_for: impl Fn(&SkillSpec) -> Result<PathBuf>) -> Result<()> {
    for skill in SKILLS {
        deploy(&path_for(skill)?, skill.md, &skill.label())?;
    }
    Ok(())
}

/// Deploy every skill's Claude surface (`ClaudeAgent::install` calls this).
pub fn install_claude() -> Result<()> {
    install_all_at(|skill| claude_skill_path(skill.id))
}

/// Deploy every skill's vendor-neutral home - the human-readable copy
/// (`agent::install_all` calls this once per `csm init`).
pub fn install_vendor_neutral() -> Result<()> {
    install_all_at(|skill| vendor_neutral_path(skill.vendor_file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_isolated_home;
    use serial_test::serial;
    use std::fs;

    mod skill_md {
        use super::*;

        #[test]
        fn frontmatter_name_and_frontloaded_description() {
            for skill in SKILLS {
                assert!(skill.md.starts_with("---\n"), "{}: frontmatter", skill.id);
                let frontmatter = skill
                    .md
                    .strip_prefix("---\n")
                    .and_then(|s| s.split_once("\n---\n"))
                    .map(|(fm, _)| fm)
                    .unwrap_or_else(|| panic!("{}: closed frontmatter block", skill.id));
                assert!(
                    frontmatter.contains(&format!("name: {}", skill.id)),
                    "{}: name",
                    skill.id
                );
                // The description is the always-loaded trigger pointer: it must
                // front-load the trigger branch.
                let desc = frontmatter
                    .lines()
                    .find(|l| l.starts_with("description:"))
                    .unwrap_or_else(|| panic!("{}: a description", skill.id));
                let trigger = match skill.id {
                    "csm-plan" => "description: Creating csm tasks",
                    "csm-scout" => "description: Exploring a codebase",
                    other => panic!("no trigger assertion for skill {other}"),
                };
                assert!(
                    desc.starts_with(trigger),
                    "{}: description must front-load the trigger, got: {desc}",
                    skill.id
                );
                // The branch itself: plan fires for a new mission or replan,
                // not a small task added to a live board; scout fires around
                // specific open questions, not routine exploration.
                let branch = match skill.id {
                    "csm-plan" => "new mission or a replan",
                    "csm-scout" => "specific open questions",
                    other => panic!("no branch assertion for skill {other}"),
                };
                assert!(
                    desc.contains(branch),
                    "{}: description must carry the trigger branch, got: {desc}",
                    skill.id
                );
            }
        }

        #[test]
        fn body_is_steps_with_completion_criteria() {
            for skill in SKILLS {
                // Vendor-neutral: paths are session-relative, so no absolute home.
                assert!(!skill.md.contains("~/.csm"), "{}: vendor-neutral", skill.id);
                assert!(
                    !skill.md.contains("$CSM_HOME"),
                    "{}: vendor-neutral",
                    skill.id
                );
                assert!(skill.md.contains("## Steps"), "{}: steps", skill.id);
                assert!(
                    skill.md.contains("**Done when:**"),
                    "{}: completion criterion",
                    skill.id
                );
                // The workflow (roles, board moves) belongs to the csm prompt;
                // the skill must not restate it.
                assert!(
                    !skill.md.contains("coordinator"),
                    "{}: prompt overlap",
                    skill.id
                );
                assert!(
                    !skill.md.contains("Pending review"),
                    "{}: prompt overlap",
                    skill.id
                );
            }
        }

        #[test]
        fn plan_skill_resolves_decisions_and_limits_replanning_scope() {
            assert!(PLAN_SKILL_MD.contains("investigate missing facts with targeted code reads"));
            assert!(PLAN_SKILL_MD.contains("choices requiring their judgment"));
            assert!(PLAN_SKILL_MD.contains("Apply the working protocol's record depth"));
            assert!(PLAN_SKILL_MD.contains("existing tasks retain their status"));
            assert!(!PLAN_SKILL_MD.contains("every INDEX line sits under Open"));
            assert!(!PLAN_SKILL_MD.contains("marked uncertain"));
            assert!(!SCOUT_SKILL_MD.contains("trust blindly"));
            assert!(SCOUT_SKILL_MD.contains("each question in this investigation"));
        }

        #[test]
        fn plan_skill_points_at_the_board_dependency_mapping() {
            // The mapping is the board's `needs:` tail; the skill records real
            // preconditions there and points at the working protocol...
            assert!(PLAN_SKILL_MD
                .contains("Record each task's real preconditions in the board's `needs:` tail"));
            assert!(PLAN_SKILL_MD.contains("working protocol"));
            // ...without restating a second rule set that can drift: no format,
            // no conditions, no graph invariants of its own.
            assert!(!PLAN_SKILL_MD.contains("local Done"));
            assert!(!PLAN_SKILL_MD.contains("comma-separated"));
            assert!(!PLAN_SKILL_MD.contains("Every ref must resolve"));
            assert!(!PLAN_SKILL_MD.contains("acyclic"));
        }
    }

    #[test]
    #[serial]
    fn install_claude_writes_converges_and_restores() {
        with_isolated_home(|_home| {
            // Fresh: writes every skill.
            install_claude().unwrap();
            for skill in SKILLS {
                let path = claude_skill_path(skill.id).unwrap();
                assert_eq!(fs::read_to_string(&path).unwrap(), skill.md);
            }

            // Stale/user-edited copy: converged back to the const.
            let path = claude_skill_path("csm-scout").unwrap();
            fs::write(&path, "user edits\n").unwrap();
            install_claude().unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), SCOUT_SKILL_MD);
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
            assert_eq!(
                fs::read_to_string(home.join("skills").join("scout.md")).unwrap(),
                SCOUT_SKILL_MD
            );
        });
    }

    #[test]
    fn skill_current_is_exact_content_equality() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("SKILL.md");
        // Missing.
        assert!(!skill_current(&path, SCOUT_SKILL_MD));
        // Present but not the const (stale or user-edited).
        fs::write(&path, "old").unwrap();
        assert!(!skill_current(&path, SCOUT_SKILL_MD));
        // Exact match - the same predicate deploy and doctor share.
        fs::write(&path, SCOUT_SKILL_MD).unwrap();
        assert!(skill_current(&path, SCOUT_SKILL_MD));
    }
}
