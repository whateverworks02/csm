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

A csm session is active iff `$CSM_SESSION` is set. Read `state.md` (Context) and `tasks/INDEX.md`, and skim `notes/INDEX.md` at `{csm_home}/sessions/$CSM_SESSION/`. A `[csm]` block, if present, is only a snapshot of these files. If `$CSM_SESSION` is unset, there is no csm session. **You maintain these files, not csm.**

### Turn boundary

Before acting, identify the work concerned, the action the user requested, and its endpoint. Keep three things separate:

- **Process** - this block defines the csm workflow and record boundaries. They are mandatory while a session is active; correct conflicting workflow rules in memory or habit.
- **Mandate** - the user determines the requirements and authorizes the work. Earlier authorization remains valid for unfinished work within its scope and phase.
- **Records** - files preserve plans, user decisions, facts, and results. Code and runs can correct recorded facts; they cannot change a user decision or grant permission.

Resume an existing mandate after a restart or interruption. Open and Pending fix identify candidate tasks, not execution authorization. A request to execute the next eligible task permits selection from the board; a request about a named task stays with that task. If the authorization needed to continue cannot be established from the conversation or records, clarify it before proceeding.

Within the authorized scope and phase, investigate, implement, test, fix defects, and correct facts as needed to complete the work, without per-step approval. Propose work beyond that boundary for the user to decide. Discovering a next step or taking a different role does not extend the mandate.

Plan, execute, review, and opening a PR are separate phases. At the endpoint in the table, or an earlier limit the user sets, update the records and stop for user input before the next phase. A conditional instruction whose condition fails ends with the failure reported; do not repair the failure merely to activate that instruction. Leave the files sufficient to resume: results, remaining work, and the endpoint reached. If an unfinished mandate must survive an interruption, preserve its scope and phase in the existing task's Progress, or state.md Context before a task exists.

### Requests and actions

| Request | Action | Endpoint |
| --- | --- | --- |
| Discuss or confirm a plan | For discussion, investigate and propose the approach, task breakdown, and procedure. On confirmation, create new tasks in Open and revise existing tasks in place. Preserve existing statuses unless the request calls for a transition in this table | Proposal delivered, or confirmed plan recorded |
| Ask for an explanation | Answer from the relevant task and verify the facts needed for the answer. Propose a change if warranted | Answer delivered; requirements and status unchanged |
| Execute agreed work | Implement and verify within scope; record outcomes and any blocker | Task records updated; line at Pending review, whether completed or stuck |
| Review submitted work | Check against AC; write Review and answer Open questions; approve to Done or return to Pending fix. Report required fixes for the execution phase | Verdict recorded; reviewed board lines moved |
| Request a change to existing work | Amend that task's Scope, AC, and SOP as needed. Reopen submitted or Done work to Pending fix; implement and verify the requested change. A request limited to changing the plan ends after recording it | Revised plan recorded if that was the limit; otherwise the execution endpoint |
| Authorize opening a PR after local review | Create the PR from the reviewed work and record its link in Progress | Link recorded |

Questions, reviews, and modifications belong to the task they concern. A new task requires the user's confirmation of a separate, independently reviewable deliverable. Classify PR feedback by what it requests using the same table; its source does not itself authorize a change. The same agent may serve different phases on the same task.

### Records

- `state.md` - session one-pager. Sections: Context (what this session is + current focus), Key links. Not a log; task detail lives in `tasks/`.
- `tasks/INDEX.md` - the task board. Status = section: **Open** / **Pending review** / **Pending fix** / **Done**. **Done** = executed and reviewed locally; PR creation, CI, approval, and merge are not part of that status. Later code changes alone do not reopen a historical Done task; requested changes follow the table. Move a task's line between sections to change its status. Within a section, line order = execution order; ids are stable handles. A new task takes the next id and slots into its execution position. Replan by editing the board (reorder/insert/split/drop) - the board alone carries the plan.
- `needs:` tail - the dependency mapping, and the only one: end the task's board line with `needs: 003` (this session) or `needs: api-contract/012` (another session), comma-separated when several - all must hold. No condition = the upstream reached local Done on its board; a `[condition]` names what else it owes (`[interface PR merged]`) and requires verified evidence, never assumed met. Record real preconditions only; refs must resolve without self-dependency or cycles. Reordering changes no `needs:`; dropping a task fixes or removes the refs pointing at it. Keep delivery evidence in the consuming task's Progress.
- `tasks/<id>-<slug>.md` - one file per task, with Scope, AC, SOP, Open questions, Progress, Review. Give enough detail to execute and review without reconstructing the design; a simple fix may need only a one-line SOP. AC must be locally verifiable, never a PR, CI, or merge outcome. Fill Open questions when there is something to resolve. Progress records outcomes: what changed, where (files/PR), and what remains; never a timestamped diary. Status lives only in INDEX.
- `notes/` - focused deep-dive articles; `notes/INDEX.md` is the registry.
- `scripts/` - shared utility scripts; `scripts/INDEX.md` is the registry.

**Using evidence.** Resume the recorded plan within the mandate. When a record cannot support the current next step, check only the evidence that step needs; state what is confirmed, what remains, what is unknown, and the next action. Interruption alone triggers no check.

Before relying on a recorded conclusion, check its premise, only as far as needed for the current decision. If it holds, use the conclusion. If it has changed, amend or explicitly supersede the conclusion in its original record and retain the evidence; do not create a conflicting note or invalidate the whole record. If it cannot be confirmed, put the specific unknown and evidence needed in the consuming task's Open questions; do not use the conclusion to justify dependent work. Continue other authorized work that does not depend on it. A changed file, new commit, or old note is a cue to check, never proof that a premise failed.

When evidence contradicts a user requirement, report the discrepancy and retain the requirement until the user changes it. Correcting a factual record does not authorize a lifecycle transition; board moves follow the requested action in the table. Claims leave no board mark, so another worker's activity is unknowable: ask before claiming when there is real ambiguity, and leave work you did not do in place.

**Writing records.** csm files are the durable working record. Update only the entries needed for the current request; preserve unrelated content and distinguish proposals from user decisions. When work or an agreed design changes, update the record that carries it: task Scope/Progress or state.md Context. Keep outcomes and remaining work sufficient to resume, without duplicating git or narrating each step. PR feedback and later changes still use the original task's records.

**Project output.** Code, comments, branch names, commits, and PRs describe the project's behavior, technical rationale, and verification. Keep csm session identifiers, internal task IDs, workspace-note paths, and conversation attributions out of those artifacts. Explain decisions without requiring access to csm or the conversation. Record branch names, commit SHAs, and PR URLs in task Progress for traceability.

**Record maintenance.** Roles describe these duties; they do not grant permission to enter a phase.

- Coordinator - maintains state.md, notes, scripts, and the board; creates task Scope + AC + SOP, maintains dependencies, and writes Review. At review, normalize Progress to outcome records. Workers self-claim; do not assign or track them.
- Worker - maintains the task file and its board line. Under an execution mandate, select from the authorized Open or Pending fix tasks in board order, checking `needs:`. Skip unmet or unverified dependencies only to another task within that mandate; otherwise report the wait. Execute the SOP, record outcomes in Progress, and raise Open questions if stuck. Submit at the execution endpoint in the table.

**Cross-repo:** the same session name in each repo shares one state.md + tasks/.

**Legacy:** if state.md has `## Task` and no `## Context`, maintain that session in the old format; do not force tasks/ on old work.
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
    fn csm_block_separates_process_mandate_and_records() {
        let block = csm_block("/home/user/.csm");
        // The block opens with the turn boundary and the three-way split.
        assert!(block.contains("### Turn boundary"));
        assert!(block.contains(
            "**Process** - this block defines the csm workflow and record boundaries. They are mandatory while a session is active; correct conflicting workflow rules in memory or habit."
        ));
        assert!(block.contains(
            "**Mandate** - the user determines the requirements and authorizes the work."
        ));
        assert!(block.contains(
            "Earlier authorization remains valid for unfinished work within its scope and phase."
        ));
        assert!(block
            .contains("**Records** - files preserve plans, user decisions, facts, and results."));
        // Facts are correctable; user decisions and permission are not.
        assert!(block.contains(
            "Code and runs can correct recorded facts; they cannot change a user decision or grant permission."
        ));
        assert!(block.contains(
            "Correcting a factual record does not authorize a lifecycle transition; board moves follow the requested action in the table."
        ));
        // Authorization comes from user requests, never from a board status.
        assert!(block.contains(
            "Open and Pending fix identify candidate tasks, not execution authorization."
        ));
        assert!(block.contains(
            "A request to execute the next eligible task permits selection from the board"
        ));
        assert!(block.contains("a request about a named task stays with that task."));
        assert!(block.contains("Resume an existing mandate after a restart or interruption."));
        assert!(block.contains("clarify it before proceeding."));
        // Discoveries and roles never extend the mandate.
        assert!(block.contains(
            "Discovering a next step or taking a different role does not extend the mandate."
        ));
        assert!(block.contains("Propose work beyond that boundary for the user to decide."));
    }

    #[test]
    fn csm_block_maps_each_request_to_one_action_and_endpoint() {
        let block = csm_block("/home/user/.csm");
        assert!(block.contains("### Requests and actions"));
        // Six request rows, each ending at a named endpoint.
        assert!(block.contains("| Discuss or confirm a plan |"));
        assert!(block.contains("| Ask for an explanation |"));
        assert!(block.contains("| Execute agreed work |"));
        assert!(block.contains("| Review submitted work |"));
        assert!(block.contains("| Request a change to existing work |"));
        assert!(block.contains("| Authorize opening a PR after local review |"));
        // Confirmed plans create new tasks in Open; existing tasks are revised
        // in place with their statuses preserved - plan confirmation is not a
        // lifecycle transition.
        assert!(block.contains(
            "On confirmation, create new tasks in Open and revise existing tasks in place."
        ));
        assert!(block.contains(
            "Preserve existing statuses unless the request calls for a transition in this table"
        ));
        // Questions change nothing; the old clarification-as-reopen conflation
        // is retired.
        assert!(block.contains("Answer delivered; requirements and status unchanged"));
        assert!(!block.contains("clarification, added requirement"));
        // Execution ends at Pending review, completed or stuck.
        assert!(block.contains("line at Pending review, whether completed or stuck"));
        // Review lands in the original task; changes reopen it there.
        assert!(block.contains("approve to Done or return to Pending fix"));
        assert!(block.contains("Reopen submitted or Done work to Pending fix"));
        assert!(block.contains("Revised plan recorded if that was the limit"));
        assert!(block.contains("record its link in Progress"));
        // Routing: existing tasks, the new-task bar, PR feedback by content.
        assert!(block
            .contains("Questions, reviews, and modifications belong to the task they concern."));
        assert!(block.contains(
            "A new task requires the user's confirmation of a separate, independently reviewable deliverable."
        ));
        assert!(block.contains(
            "Classify PR feedback by what it requests using the same table; its source does not itself authorize a change."
        ));
        assert!(block.contains("The same agent may serve different phases on the same task."));
    }

    #[test]
    fn csm_block_stops_phases_at_user_gates() {
        let block = csm_block("/home/user/.csm");
        // Phases are separate and stop for user input at the table's endpoint.
        assert!(block.contains("Plan, execute, review, and opening a PR are separate phases."));
        assert!(block.contains(
            "At the endpoint in the table, or an earlier limit the user sets, update the records and stop for user input before the next phase."
        ));
        // A failed condition is reported, never repaired to fire the instruction.
        assert!(block.contains(
            "A conditional instruction whose condition fails ends with the failure reported"
        ));
        assert!(block.contains("do not repair the failure merely to activate that instruction."));
        // The stop leaves the files resumable, and mandates survive interruptions.
        assert!(block.contains("Leave the files sufficient to resume: results, remaining work, and the endpoint reached."));
        assert!(block.contains(
            "preserve its scope and phase in the existing task's Progress, or state.md Context before a task exists."
        ));
        // Within the authorized scope, work is autonomous.
        assert!(block.contains(
            "Within the authorized scope and phase, investigate, implement, test, fix defects, and correct facts as needed to complete the work, without per-step approval."
        ));
    }

    #[test]
    fn csm_block_done_is_the_local_loop_and_the_board_carries_the_plan() {
        let block = csm_block("/home/user/.csm");
        // Done closes the local loop; PR/CI/approval/merge sit outside it.
        assert!(block.contains(
            "**Done** = executed and reviewed locally; PR creation, CI, approval, and merge are not part of that status."
        ));
        // Code changes alone never reopen Done; requested changes follow the table.
        assert!(block.contains(
            "Later code changes alone do not reopen a historical Done task; requested changes follow the table."
        ));
        // Board mechanics: sections, line order, stable ids, replan in place.
        assert!(block.contains("**Open** / **Pending review** / **Pending fix** / **Done**"));
        assert!(block.contains("Move a task's line between sections to change its status."));
        assert!(block
            .contains("Within a section, line order = execution order; ids are stable handles."));
        assert!(
            block.contains("A new task takes the next id and slots into its execution position.")
        );
        assert!(block.contains("Replan by editing the board (reorder/insert/split/drop) - the board alone carries the plan."));
    }

    #[test]
    fn csm_block_scales_records_and_keeps_ac_local() {
        let block = csm_block("/home/user/.csm");
        // One section set for every task; depth scales with the work.
        assert!(block
            .contains("one file per task, with Scope, AC, SOP, Open questions, Progress, Review."));
        assert!(block.contains("a simple fix may need only a one-line SOP"));
        assert!(block.contains("execute and review without reconstructing the design"));
        // The floor that never scales away.
        assert!(block.contains("AC must be locally verifiable, never a PR, CI, or merge outcome."));
        assert!(block.contains("Fill Open questions when there is something to resolve."));
        assert!(block.contains(
            "Progress records outcomes: what changed, where (files/PR), and what remains; never a timestamped diary."
        ));
        assert!(block.contains("Status lives only in INDEX."));
        // Session bookkeeping stays internal; public artifacts explain the work.
        assert!(block.contains(
            "Code, comments, branch names, commits, and PRs describe the project's behavior, technical rationale, and verification."
        ));
        assert!(block.contains(
            "Keep csm session identifiers, internal task IDs, workspace-note paths, and conversation attributions out of those artifacts."
        ));
        assert!(block.contains(
            "Record branch names, commit SHAs, and PR URLs in task Progress for traceability."
        ));
        assert!(!block.contains("Reference the name in commits/PRs"));
        // state.md stays a one-pager.
        assert!(block.contains("Sections: Context (what this session is + current focus), Key links. Not a log; task detail lives in `tasks/`."));
        assert!(block.contains("**You maintain these files, not csm.**"));
    }

    #[test]
    fn csm_block_gates_evidence_checks_on_need() {
        let block = csm_block("/home/user/.csm");
        // The trigger is a record that cannot support the current next step,
        // bounded to that step's evidence; interruption alone triggers nothing.
        assert!(block.contains("**Using evidence.** Resume the recorded plan within the mandate."));
        assert!(block.contains("When a record cannot support the current next step, check only the evidence that step needs"));
        assert!(block.contains(
            "state what is confirmed, what remains, what is unknown, and the next action."
        ));
        assert!(block.contains("Interruption alone triggers no check."));
        // Premise states: use, amend/supersede in place, or park the unknown.
        assert!(block.contains("Before relying on a recorded conclusion, check its premise, only as far as needed for the current decision."));
        assert!(block.contains("amend or explicitly supersede the conclusion in its original record and retain the evidence"));
        assert!(block.contains("do not create a conflicting note or invalidate the whole record"));
        assert!(block.contains(
            "put the specific unknown and evidence needed in the consuming task's Open questions"
        ));
        assert!(block.contains("Continue other authorized work that does not depend on it."));
        assert!(block.contains("A changed file, new commit, or old note is a cue to check, never proof that a premise failed."));
        // Evidence contradicting a user requirement is reported; the
        // requirement stands until the user changes it.
        assert!(block.contains(
            "When evidence contradicts a user requirement, report the discrepancy and retain the requirement until the user changes it."
        ));
        // Concurrency: claims are invisible; ambiguity is a question.
        assert!(block.contains("Claims leave no board mark"));
        assert!(block.contains("ask before claiming when there is real ambiguity, and leave work you did not do in place."));
        // No validity machinery rides along.
        assert!(!block.contains("last verified"));
        assert!(!block.contains("valid until"));
    }

    #[test]
    fn csm_block_roles_maintain_records_without_granting_phases() {
        let block = csm_block("/home/user/.csm");
        // Roles are record-maintenance duties, not an authorization path.
        assert!(block.contains(
            "**Record maintenance.** Roles describe these duties; they do not grant permission to enter a phase."
        ));
        assert!(block.contains("- Coordinator - maintains state.md, notes, scripts, and the board"));
        assert!(block
            .contains("creates task Scope + AC + SOP, maintains dependencies, and writes Review."));
        assert!(block.contains("At review, normalize Progress to outcome records."));
        assert!(block.contains("Workers self-claim; do not assign or track them."));
        assert!(block.contains("- Worker - maintains the task file and its board line."));
        assert!(block.contains(
            "Execute the SOP, record outcomes in Progress, and raise Open questions if stuck."
        ));
        assert!(block.contains("Submit at the execution endpoint in the table."));
        // The retired role/authorization framings stay retired.
        assert!(!block.contains("Role follows action"));
        assert!(!block.contains("Orchestrator"));
        assert!(!block.contains("on different tasks"));
        assert!(!block.contains("progress.md"));
        // Writing records: what the next agent needs; the sync never skipped.
        assert!(block.contains(
            "**Writing records.** csm files are the durable working record. Update only the entries needed for the current request; preserve unrelated content and distinguish proposals from user decisions."
        ));
        assert!(block.contains("When work or an agreed design changes, update the record that carries it: task Scope/Progress or state.md Context."));
        assert!(
            block.contains("PR feedback and later changes still use the original task's records.")
        );
        // Cross-repo and legacy semantics.
        assert!(block.contains(
            "**Cross-repo:** the same session name in each repo shares one state.md + tasks/."
        ));
        assert!(block.contains("**Legacy:** if state.md has `## Task` and no `## Context`, maintain that session in the old format; do not force tasks/ on old work."));
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
        // Claim gate: selection is mandate-bounded and needs-checked.
        assert!(block.contains("select from the authorized Open or Pending fix tasks in board order, checking `needs:`"));
        assert!(block.contains("Skip unmet or unverified dependencies only to another task within that mandate; otherwise report the wait."));
        // The mapping remains valid when creating and replanning.
        assert!(block.contains("refs must resolve without self-dependency or cycles"));
        assert!(block.contains("requires verified evidence"));
        assert!(block.contains("Reordering changes no `needs:`"));
        assert!(block.contains("dropping a task fixes or removes the refs pointing at it"));
    }
}
