//! csm-shipped skills, distributed by `csm init`: `/csm-plan`, the authoring
//! discipline for the coordinator's architect pass (grill -> one-pager ->
//! executor-grade tasks), and `/csm-scout`, the exploration discipline for the
//! scout pass that precedes it (notes an architect can plan from) - the two
//! variance-prone handoffs in the tiered pipeline (`notes/scout-plan-skills.md`).
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
//! in their always-loaded prompt block was judged not worth its context load -
//! the skills are Claude-reachable only for now.

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

/// The `/csm-scout` skill. The frontmatter `description` front-loads the
/// trigger branch (exploring to feed a planning pass); the body is steps that
/// each end on a completion criterion. Leading words: question-first (explore
/// to answer, not to tour), read vs inferred (the trust mark on every claim),
/// `open:` (an unknown is output, not failure), report-don't-decide (the
/// architect picks). Like csm-plan, it owns discipline only - the csm prompt
/// owns the workflow, and csm-plan's own content is not restated.
pub const SCOUT_SKILL_MD: &str = r#"---
name: csm-scout
description: Exploring a codebase to feed a csm planning pass - write the notes an architect will plan from
---

# csm-scout

Scout pass over the codebase: the mission's open decisions in; `notes/` out. The notes are the architect's only eyes on the code - the planning pass reads notes, not code - so every decision the plan makes rests on what this pass writes.

## Steps

1. **Question list first.** Before walking any file, derive from the mission the decisions the plan must make - schema? API shape? which layer? what exists already? Explore to answer those questions; a file read without a question it serves is a tour.
2. **One note per question.** Title = the question. Body: the answer, evidence (`path:line` for every non-trivial claim), alternatives considered + tradeoffs, unknowns. Mark each claim **read** (you saw it) or **inferred** (you concluded it) - the architect must know which claims to trust blindly.
3. **Unknowns are output.** A note ending in `open: X` is correct - it is grill material for the planning pass. A guess dressed as an answer is the one failure this pass can fail silently.
4. **Report, don't decide.** Present the options + tradeoffs and stop; the architect picks. A note that concludes "so we should do X" has overstepped.
5. **Register.** One-line gist per note in `notes/INDEX.md`.
6. **Done when:** every decision the plan must make is either answered with evidence or explicitly listed as open.
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
