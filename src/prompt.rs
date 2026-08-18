//! The csm working-mode prompt, injected into `~/.claude/CLAUDE.md` via
//! `csm init`.
//!
//! Style: terse, action-first. No tool introduction - just tell the agent what
//! to do when a session is active. No hard-wrapping (each unit on one line).
//! Dormant unless a csm session is active, so safe in the global CLAUDE.md.

pub const CSM_MARK_BEGIN: &str = "<!-- csm:begin -->";
pub const CSM_MARK_END: &str = "<!-- csm:end -->";

/// The full marked block to inject. `csm_home` is rendered into the prompt's
/// path so a relocated `$CSM_HOME` is reflected - an agent follows the actual
/// home, not a hardcoded `~/.csm`.
pub fn csm_block(csm_home: &str) -> String {
    format!(
        "{begin}\n\
## csm workspace memory

A csm session is active iff `$CSM_SESSION` is set. Orient on `state.md` + `tasks/INDEX.md` at `{csm_home}/sessions/$CSM_SESSION/` (a `[csm]` block, if present, is only a snapshot of these). If `$CSM_SESSION` is unset, there is no csm session. **You maintain these files, not csm.**

- `state.md` - session one-pager. Sections: Context (what this session is + current focus), Key links. Not a log; task detail lives in `tasks/`.
- `tasks/INDEX.md` - the task board. Status = section: **Open** / **Pending review** / **Pending fix** / **Done**. Move a task's line between sections to change its status.
- `tasks/<id>-<slug>.md` - one file per task. Sections: Scope, AC, SOP, Open questions, Progress, Review. Progress = outcome records (what changed, where - files/PR - and what's left), never a timestamped diary. No status/owner here (those live in INDEX).
- `notes/` - focused deep-dive articles; `notes/INDEX.md` is the registry.
- `scripts/` - shared utility scripts; `scripts/INDEX.md` is the registry.

### Working mode

1. **Orient.** Read `state.md` (Context), `tasks/INDEX.md` (Open + Pending fix are claimable; Pending review awaits the coordinator; Done is skimmable); skim `notes/INDEX.md`.
2. **Role follows action.** Creating or reviewing a task = coordinator (touch `state.md`, `notes/`, `scripts/`, the board). Claiming or executing a task = worker (touch only that task's file + your own INDEX line). One agent can do both - do whichever the current step needs.
   - **Coordinator actions**: maintain `state.md`, `notes/`, `scripts/`. Create tasks in Open (write Scope + AC + SOP in the task file - the procedure is part of the design). Review Pending review -> approve to Done, or write Review + answer Open questions -> Pending fix; at review, normalize Progress to outcome records (strip timestamped/narrative lines). Workers self-claim - don't assign or track them.
   - **Worker actions**: claim one task from Open or Pending fix (no INDEX mark). Execute its SOP, recording outcomes in the task file's Progress section; raise Open questions if stuck. Submit (done or stuck) by moving your own INDEX line to Pending review.
3. **Write discipline.** csm files orient the next agent - don't duplicate what git already records. Default to not writing; before writing, ask: \"will the next agent need this to orient, claim, or review?\" If not, skip it.
4. **Before you stop:** leave the files pick-up-ready - worker: task file complete + INDEX line at Pending review; coordinator: reviewed INDEX lines moved.
5. **Cross-repo:** the same session name in each repo shares one `state.md` + `tasks/`. Reference the name in commits/PRs.
6. **Legacy.** If `state.md` has `## Task` (no `## Context`), it's a pre-tasks-model session - maintain it the old way; don't force `tasks/` on old work.
{end}",
        begin = CSM_MARK_BEGIN,
        end = CSM_MARK_END,
        csm_home = csm_home,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csm_block_renders_home_into_path() {
        let block = csm_block("/data/csm");
        assert!(block.contains("/data/csm/sessions/$CSM_SESSION/"));
        assert!(!block.contains("~/.csm/sessions"));
        assert!(block.starts_with(CSM_MARK_BEGIN));
        assert!(block.ends_with(CSM_MARK_END));
    }

    #[test]
    fn csm_block_default_home_path() {
        let block = csm_block("/home/user/.csm");
        assert!(block.contains("/home/user/.csm/sessions/$CSM_SESSION/"));
    }

    #[test]
    fn csm_block_encodes_review_loop_model_and_retires_append_mandate() {
        let block = csm_block("/home/user/.csm");
        // Board + review-loop status flow.
        assert!(block.contains("tasks/INDEX.md"));
        assert!(block.contains("Pending review"));
        assert!(block.contains("Pending fix"));
        // Role split is coordinator/worker (not orchestrator).
        assert!(block.contains("Coordinator"));
        assert!(block.contains("Worker"));
        assert!(!block.contains("Orchestrator"));
        // Role is action-derived, not a fixed identity.
        assert!(block.contains("Role follows action"));
        assert!(!block.contains("Know your role"));
        // Coordinator creates (Scope+AC+SOP) + reviews; worker executes the SOP + submits.
        assert!(block.contains("Create tasks in Open"));
        assert!(block.contains("Scope + AC + SOP"));
        assert!(block.contains("Execute its SOP"));
        assert!(block.contains("approve to Done"));
        assert!(block.contains("self-claim"));
        // state.md slimmed to Context + Key links.
        assert!(block.contains("## Context"));
        // The `>>`-not-Edit mandate is retired (single-writer; Edit is the path).
        assert!(!block.contains("not Edit"));
        // progress.md is gone - board + task files absorbed it.
        assert!(!block.contains("progress.md"));
    }
}
