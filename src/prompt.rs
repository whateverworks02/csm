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
- `tasks/INDEX.md` - the task board. Status = section: **Open** / **Pending review** / **Pending fix** / **Done**. **Done** = executed, reviewed locally - a PR, if any, is raised on the user's explicit go-ahead after the review; CI/approval/merge are outside csm. Move a task's line between sections to change its status; within a section, line order = execution order. Ids are stable handles: a new task takes the next id and slots into its execution position. Replan by editing the board (reorder/insert/split/drop) - the board, and only the board, carries the plan.
- `needs:` tail - the dependency mapping, and the only one: end the task's board line with `needs: 003` (this session) or `needs: api-contract/012` (another session), comma-separated when several - all must hold. No condition = the upstream reached local Done on its board; a `[condition]` names what else it owes (`[interface PR merged]`) and requires verified evidence, never assumed met. Record real preconditions only; refs must resolve without self-dependency or cycles. Reordering changes no `needs:`; dropping a task fixes or removes the refs pointing at it. Keep delivery evidence in the consuming task's Progress.
- `tasks/<id>-<slug>.md` - one file per task. Same sections for every task - Scope, AC, SOP, Open questions, Progress, Review - with enough detail for another agent to execute and review without reconstructing the design; a simple fix may need only a one-line SOP. Every task has verifiable AC; Open questions is filled when there is something to say. Progress = outcome records (what changed, where - files/PR - and what's left), never a timestamped diary. No status/owner here (those live in INDEX).
- `notes/` - focused deep-dive articles; `notes/INDEX.md` is the registry.
- `scripts/` - shared utility scripts; `scripts/INDEX.md` is the registry.

### Working mode

1. **Orient.** Read `state.md` (Context), `tasks/INDEX.md` (Open + Pending fix are claimable; Pending review awaits the coordinator; Done is skimmable); skim `notes/INDEX.md`. Resume the recorded plan; when the record can't support the next step of the task at hand - a task still Open while the repo already shows its work, Progress recording a result nothing verifies - check just the evidence that step depends on, then state what is confirmed, what remains, what is unknown, and the next action. Interruption alone triggers no check. Claims leave no board mark, so another worker's activity is unknowable: on real ambiguity ask before claiming, and leave work you did not do in place. Plan when the mission or approach needs to change; investigate when a specific unanswered question requires code evidence.
2. **Role follows action.** Creating or reviewing a task = coordinator (touch `state.md`, `notes/`, `scripts/`, the board). Claiming or executing a task = worker (touch only that task's file + your own INDEX line). One agent can serve both roles on different tasks; a worker submits its own work for coordinator review.
   - **Coordinator actions**: maintain `state.md`, `notes/`, `scripts/`. Create tasks in Open (write Scope + AC + SOP in the task file - the procedure is part of the design) with their `needs:` tails. Review Pending review -> approve to Done, or write Review + answer Open questions -> Pending fix; at review, normalize Progress to outcome records (strip timestamped/narrative lines). Workers self-claim - don't assign or track them.
   - **Worker actions**: Check `needs:` in board order and claim one eligible task from Open or Pending fix; when a dependency is unmet or unverified, name the wait and take the next eligible line. Execute its SOP, recording outcomes in the task file's Progress section; raise Open questions if stuck. Submit (done or stuck) by moving your own INDEX line to Pending review. Your responsibility for this task ends at submission; self-checks or checks by other agents do not authorize you to write its Review or move it to Done.
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
        // Roles can vary across tasks; executing a task does not authorize its review.
        assert!(block.contains("Role follows action"));
        assert!(block.contains("One agent can serve both roles on different tasks"));
        assert!(block.contains("Your responsibility for this task ends at submission"));
        assert!(block.contains("do not authorize you to write its Review or move it to Done"));
        assert!(!block.contains("do whichever the current step needs"));
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

    #[test]
    fn csm_block_done_is_the_local_loop_and_the_board_carries_the_plan() {
        let block = csm_block("/home/user/.csm");
        // Done closes the local loop; upstream status never gates it.
        // The PR is the hand-off, not loop work: user-gated, after the local review.
        assert!(block.contains("**Done** = executed, reviewed locally"));
        assert!(block
            .contains("a PR, if any, is raised on the user's explicit go-ahead after the review"));
        assert!(block.contains("CI/approval/merge are outside csm"));
        // The PR never re-enters the Done equation as worker work.
        assert!(!block.contains("reviewed locally, PR raised"));
        // Execution order lives in board line order, not in ids or chat.
        assert!(block.contains("line order = execution order"));
        assert!(block.contains("Ids are stable handles"));
        assert!(block.contains("Replan by editing the board"));
        // The board is a replan surface, not an append-only log.
        assert!(block.contains("reorder/insert/split/drop"));
    }

    #[test]
    fn csm_block_scales_records_and_gates_planning() {
        let block = csm_block("/home/user/.csm");
        // One section set for every task; depth scales with the work.
        assert!(block.contains("Same sections for every task"));
        assert!(block.contains("a simple fix may need only a one-line SOP"));
        assert!(block.contains("execute and review without reconstructing the design"));
        // The floor that never scales away.
        assert!(block.contains("Every task has verifiable AC"));
        assert!(!block.contains("the local review are never skipped"));
        assert!(block.contains("Open questions is filled when there is something to say"));
        // Orientation resumes the existing plan; planning and scouting are
        // on-demand, not a continuation ritual.
        assert!(block.contains("Resume the recorded plan"));
        assert!(block.contains("Plan when the mission or approach needs to change"));
        assert!(block
            .contains("investigate when a specific unanswered question requires code evidence"));
    }

    #[test]
    fn csm_block_gates_continuation_on_evidence() {
        let block = csm_block("/home/user/.csm");
        // The trigger is a record that can't support the next step, not the
        // interruption itself.
        assert!(block.contains("when the record can't support the next step"));
        assert!(block.contains("Interruption alone triggers no check"));
        // The check is bounded to what the next step depends on, and the
        // report separates confirmed from unknown.
        assert!(block.contains("check just the evidence that step depends on"));
        assert!(block.contains("state what is confirmed, what remains, what is unknown"));
        // Claims leave no trace, so a live worker is unknowable: ask, don't
        // take over, and don't touch work that isn't yours.
        assert!(block.contains("Claims leave no board mark"));
        assert!(block.contains("on real ambiguity ask before claiming"));
        assert!(block.contains("leave work you did not do in place"));
        // The worker bullet's "(no INDEX mark)" is retired into the line above.
        assert!(!block.contains("no INDEX mark"));
    }

    #[test]
    fn csm_block_carries_the_needs_dependency_mapping() {
        let block = csm_block("/home/user/.csm");
        // One mapping, on the board line - no second file, no second graph.
        assert!(block.contains("`needs:` tail - the dependency mapping, and the only one"));
        // Reference forms: same session, and cross-session for cross-repo work.
        assert!(block.contains("needs: 003` (this session)"));
        assert!(block.contains("needs: api-contract/012` (another session)"));
        // Several refs all must hold.
        assert!(block.contains("comma-separated when several - all must hold"));
        // Default condition is the upstream's local Done; a bracketed condition
        // is extra and never assumed satisfied.
        assert!(block.contains("No condition = the upstream reached local Done on its board"));
        assert!(block.contains("never assumed met"));
        // No implicit serialization: order is not a dependency.
        assert!(block.contains("Record real preconditions only"));
        // The tail stays a mapping; the evidence lives in the consuming task.
        assert!(block.contains("Keep delivery evidence in the consuming task's Progress"));
        // Claim gate and the local-Done vs external-delivery distinction.
        assert!(block.contains("Check `needs:` in board order"));
        assert!(block.contains("when a dependency is unmet or unverified"));
        // The mapping remains valid when creating and replanning.
        assert!(block.contains("refs must resolve without self-dependency or cycles"));
        assert!(block.contains("requires verified evidence"));
        assert!(block.contains("Reordering changes no `needs:`"));
        assert!(block.contains("dropping a task fixes or removes the refs pointing at it"));
    }
}
