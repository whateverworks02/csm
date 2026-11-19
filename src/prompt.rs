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

### Turn decisions

Before acting, identify the work concerned, the action the user requested, and its endpoint.

1. **Authority.** Follow this workflow while the session is active and correct conflicting workflow rules in memory or habit. The user sets requirements and authorizes work. Use the conversation or a record of the user's authorization to establish the mandate, and resume it if unfinished after an interruption. Board status, facts, role, and newly discovered work confer no authority. Clarify if the authority you need is missing.
2. **Scope.** Identify the requested outcome and keep questions, reviews, and changes with the task they concern. Classify PR feedback by what it requests. Its source alone cannot authorize a change. A named-task request stays with that task. A request to execute the next eligible task lets you select an authorized Open or Pending fix line in board order. Create a new task only for a separate, independently reviewable deliverable the user has confirmed. Maintain in-scope records directly without a new board line.
3. **Phase.** Use the table to identify the authorized action. Plan, execute, review, and open a PR as separate phases, even if one agent serves all of them. Investigate, implement, test, fix defects, and correct facts as needed within the authorized scope and phase without per-step approval. Propose work beyond that boundary for a user decision.
4. **Prerequisites.** Check only evidence the next decision needs, including `needs:` before claiming. Skip a task with unmet or unverified dependencies only to select another authorized candidate. If none qualifies, report the wait. If a condition in the user's instruction fails, report it and stop that instruction. Do not repair the failure merely to activate it. Interruption alone triggers no check.

   Resume the recorded plan within the mandate. Before using a recorded conclusion, verify the relevant premise. Changes or age are cues to check, not proof of failure. If the premise holds, reuse the conclusion. If the premise changed, update the original record with the new evidence even when the conclusion still holds. Amend or explicitly supersede any affected conclusion and preserve unaffected findings.

   If a required premise is unverified, record the specific unknown and the evidence needed in the consuming task's Open questions. Pause dependent work and continue independent authorized work. When the record alone cannot support the next step, report what the evidence confirms, what remains unknown or undone, and the next action. Report any conflict with a user requirement and retain the requirement until the user changes it.
5. **Ownership.** Update only the relevant entries and distinguish proposals from user decisions. Coordinators maintain state.md, notes, scripts, and the board, create Scope, AC, and SOP, maintain dependencies, and write Review while normalizing Progress. Workers maintain their task file and board line, follow its SOP, and raise Open questions when stuck. Workers self-claim without assignment or tracking. Claims leave no board mark. Ask before claiming if there is real ownership ambiguity and leave others' work in place.
6. **Endpoint.** Write the current result, verification, remaining work, and endpoint reached, then stop at the table's endpoint or an earlier user limit. At handoff, wait for user input before entering the next phase. Preserve the scope and phase of interrupted work in Progress, or in Context under the pre-task exception below. Factual corrections alone do not change board status.

### Requests and actions

| Request | Action | Endpoint |
| --- | --- | --- |
| Discuss or confirm a plan | For discussion, investigate and propose the approach, task breakdown, and procedure. On confirmation, create new tasks in Open and revise existing tasks in place. Preserve existing statuses unless the request calls for a transition in this table | Proposal delivered, or confirmed plan recorded |
| Ask for an explanation | Answer from the relevant task and verify the facts needed for the answer. Propose a change if warranted | Answer delivered; requirements and status unchanged |
| Execute agreed work | Implement and verify within scope; record outcomes and any blocker | Task records updated; line at Pending review, whether completed or stuck |
| Review submitted work | Check against AC; write Review and answer Open questions; approve to Done or return to Pending fix. Report required fixes for the execution phase | Verdict recorded; reviewed board lines moved |
| Request a change to existing work | Amend that task's Scope, AC, and SOP as needed. A request limited to changing the plan reopens submitted or Done work to Pending fix and ends; otherwise implement and verify the requested change | Revised plan recorded if that was the limit; otherwise the execution endpoint |
| Authorize opening a PR after local review | Create the PR from the reviewed work and record its link in Progress | Link recorded |

Pending review and Pending fix are handoffs. Keep internal fixes and self-checks within execution. Submit unchecked work to Pending review. For conclusions, drafts, and record updates with nothing to check, or changes riding an already-open PR for a Done task, record the actual state instead: Done if complete, Pending fix or Open if execution remains. Follow the user's explicit status decision, which also serves as the verdict when the user is the sole reviewer.

### Records

- `state.md` - session one-pager with exactly two sections: `Context` and `Key links` (replace the scaffold comments when you write). `Context` states the session's mission and current problem focus. Make its opening paragraph useful on its own for readers of compact summaries such as `csm show`. Add short paragraphs when they help explain the mission or focus. Replace superseded text in place. A focus change changes the problem or scope being addressed; progress within that scope belongs in task records.

  The coordinator maintains this session description. During execution of an existing task, record any needed correction to the description in that task's `Progress` for the coordinator to reconcile; leave `state.md` unchanged. Before the work has a task, `Context` may also hold one sentence preserving an unfinished mandate's scope and authorized phase; move that sentence to the task's `Progress` when the task is created.

  `Key links` holds durable session pointers (repo, docs, related sessions, transcript). Task status and order belong in `tasks/INDEX.md`; outcomes and branch/commit/PR identifiers belong in task `Progress`; rulings, rationale, and measurements belong in `notes/` or the relevant task. `state.md` is not a task history or investigation/debugging log.
- `tasks/INDEX.md` - the task board. Status = section: **Open** / **Pending review** / **Pending fix** / **Done**. **Done** = complete under the endpoint rules above, including local review where required. PR creation, CI, approval, and merge are not prerequisites. Later code changes alone do not reopen a historical Done task; requested changes follow the table. Move a task's line between sections to change its status. Within a section, line order = execution order; ids are stable handles. A new task takes the next id and slots into its execution position. Replan by editing the board (reorder/insert/split/drop) - the board alone carries the plan.
- `needs:` tail - the dependency mapping, and the only one: end the task's board line with `needs: 003` (this session) or `needs: api-contract/012` (another session), comma-separated when several - all must hold. No condition = the upstream reached local Done on its board; a `[condition]` names what else it owes (`[interface PR merged]`) and requires verified evidence, never assumed met. Record real preconditions only; refs must resolve without self-dependency or cycles. Reordering changes no `needs:`; dropping a task fixes or removes the refs pointing at it. Keep delivery evidence in the consuming task's Progress.
- A board line - `<id> <slug>`, with the `needs:` tail last. The `[csm]` block renders the board by section, each task's head and `needs:` tail only; what a task is about lives in `tasks/<id>-<slug>.md`.
- `tasks/<id>-<slug>.md` - one file per task, with Scope, AC, SOP, Open questions, Progress, Review. Give enough detail to execute and review without reconstructing the design; a simple fix may need only a one-line SOP. AC must be locally verifiable, never a PR, CI, or merge outcome. Fill Open questions when there is something to resolve. `Progress` is the task's current outcome state, rewritten in place as the task advances: what changed, where (files, branch, commit, PR), the verification result, and what remains - without duplicating git or logging investigation, test-fixing, or CI-debugging steps. Status lives only in INDEX.
- `References:` / `View:` lines - optional header lines under the task file's title (omitted when unused): `References: <slug>[, <slug>]` names the `notes/` articles the task rests on (bare slugs resolving to `notes/<slug>.md`, the reading list for whoever executes or reviews), and `View: <id>-<slug>.html` points to an existing `views/<id>-<slug>.html` file. `References:` carries the why (design rationale, provenance, adjudications); the what stays in Scope/AC/SOP. Read-side only: a note you plan to write is prose in SOP. The coordinator sets references at task creation; a worker adds a reference they write or discover. Neither line gates claiming - the `needs:` tail owns gating.
- `notes/` - focused deep-dive articles; `notes/INDEX.md` is the registry, one gist line per article describing the article alone. Which tasks rest on a note lives in those tasks' `References:`, so the registry never enumerates task ids.
- `views/` - optional HTML pages for human readers. Create a view if the user requests one. Otherwise, use one at your discretion when it helps the reader understand the task. Planning, execution, and review can proceed without an optional view or an explanation for omitting it, including for logic changes. When creating one, use a self-contained `views/<id>-<slug>.html` file and add the task's `View:` header. Register the page in `views/INDEX.md` with a `### <id>-<slug>.html` heading and a one-line gist. If a shareable page is published, record its link in Progress alongside the file. Derive the page from the task and its notes. Notes remain authoritative where a view differs, and existing records can supply the facts without duplication. An existing view carries no routine maintenance duty. When using one for a current decision, check the claims that matter. Refresh any outdated claims or clearly mark the view as historical and base the decision on current records.
- `scripts/` - shared utility scripts; `scripts/INDEX.md` is the registry.

**Record language.** Use English by default for new agent-authored csm prose and follow the user's language preference where it applies. Preserve existing record language during routine edits, keep quotations in the original, and retain proper names and identifiers such as task IDs, slugs, and filenames. Keep original wording where translation would lose meaning. Follow the user's language in replies. Language alone never blocks execution.

**Project output.** Code, comments, branch names, commits, and PRs describe the project's behavior, technical rationale, and verification. Keep csm session identifiers, internal task IDs, workspace-note paths, and conversation attributions out of those artifacts. Explain decisions without requiring access to csm or the conversation. Record branch names, commit SHAs, and PR URLs in task Progress for traceability.

**Cross-repo:** the same session name in each repo shares one state.md + tasks/.

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
    fn csm_block_orders_turn_decisions_and_separates_authority_from_scope() {
        let block = csm_block("/home/user/.csm");
        let mut remainder = block
            .split_once("### Turn decisions")
            .expect("turn procedure")
            .1
            .split_once("### Requests and actions")
            .expect("request table")
            .0;
        for step in [
            "1. **Authority.**",
            "2. **Scope.**",
            "3. **Phase.**",
            "4. **Prerequisites.**",
            "5. **Ownership.**",
            "6. **Endpoint.**",
        ] {
            remainder = remainder.split_once(step).expect(step).1;
        }

        assert!(block.contains("The user sets requirements and authorizes work."));
        assert!(block.contains(
            "Use the conversation or a record of the user's authorization to establish the mandate, and resume it if unfinished after an interruption."
        ));
        assert!(block
            .contains("Board status, facts, role, and newly discovered work confer no authority."));
        assert!(block.contains("Clarify if the authority you need is missing."));
        assert!(block.contains("A named-task request stays with that task."));
        assert!(block.contains(
            "A request to execute the next eligible task lets you select an authorized Open or Pending fix line in board order."
        ));
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
        // Review lands in the original task. A plan-only change reopens it,
        // and an implemented change reaches the execution endpoint.
        assert!(block.contains("approve to Done or return to Pending fix"));
        assert!(block.contains(
            "A request limited to changing the plan reopens submitted or Done work to Pending fix and ends"
        ));
        assert!(block.contains(
            "Revised plan recorded if that was the limit; otherwise the execution endpoint |"
        ));
        assert!(block.contains("record its link in Progress"));
        assert!(block.contains("keep questions, reviews, and changes with the task they concern"));
        assert!(block.contains(
            "Classify PR feedback by what it requests. Its source alone cannot authorize a change."
        ));
    }

    #[test]
    fn csm_block_defines_handoffs_and_the_task_gate() {
        let block = csm_block("/home/user/.csm");
        assert!(block.contains("Keep internal fixes and self-checks within execution."));
        assert!(block.contains("Submit unchecked work to Pending review."));
        assert!(block.contains(
            "For conclusions, drafts, and record updates with nothing to check, or changes riding an already-open PR for a Done task, record the actual state instead: Done if complete, Pending fix or Open if execution remains."
        ));
        assert!(block.contains(
            "Follow the user's explicit status decision, which also serves as the verdict when the user is the sole reviewer."
        ));
        assert!(block.contains(
            "Create a new task only for a separate, independently reviewable deliverable the user has confirmed."
        ));
        assert!(block.contains("Maintain in-scope records directly without a new board line."));
    }

    #[test]
    fn csm_block_stops_phases_at_user_gates() {
        let block = csm_block("/home/user/.csm");
        assert!(block.contains(
            "Plan, execute, review, and open a PR as separate phases, even if one agent serves all of them."
        ));
        assert!(block.contains(
            "Investigate, implement, test, fix defects, and correct facts as needed within the authorized scope and phase without per-step approval."
        ));
        assert!(block.contains("Propose work beyond that boundary for a user decision."));
        assert!(block.contains(
            "Write the current result, verification, remaining work, and endpoint reached, then stop at the table's endpoint or an earlier user limit."
        ));
        assert!(block.contains("At handoff, wait for user input before entering the next phase."));
        assert!(block.contains(
            "Preserve the scope and phase of interrupted work in Progress, or in Context under the pre-task exception below."
        ));
        assert!(block.contains("Factual corrections alone do not change board status."));
    }

    #[test]
    fn csm_block_done_is_the_local_loop_and_the_board_carries_the_plan() {
        let block = csm_block("/home/user/.csm");
        // Done closes the local loop; PR/CI/approval/merge sit outside it.
        assert!(block.contains(
            "**Done** = complete under the endpoint rules above, including local review where required. PR creation, CI, approval, and merge are not prerequisites."
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
            "`Progress` is the task's current outcome state, rewritten in place as the task advances: what changed, where (files, branch, commit, PR), the verification result, and what remains"
        ));
        assert!(block.contains(
            "without duplicating git or logging investigation, test-fixing, or CI-debugging steps."
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
        // state.md keeps two sections and a self-contained opening summary.
        assert!(block.contains("exactly two sections: `Context` and `Key links`"));
        assert!(block.contains("Make its opening paragraph useful on its own"));
        assert!(block.contains("Add short paragraphs when they help explain the mission or focus"));
        assert!(block.contains(
            "A focus change changes the problem or scope being addressed; progress within that scope belongs in task records."
        ));
        assert!(block.contains("`Key links` holds durable session pointers"));
        assert!(block.contains(
            "Task status and order belong in `tasks/INDEX.md`; outcomes and branch/commit/PR identifiers belong in task `Progress`; rulings, rationale, and measurements belong in `notes/` or the relevant task. `state.md` is not a task history or investigation/debugging log."
        ));
        assert!(block.contains("**You maintain these files, not csm.**"));
    }

    #[test]
    fn csm_block_defines_references_and_views_edges() {
        let block = csm_block("/home/user/.csm");
        // The optional task headers still point to the same record paths.
        assert!(block.contains(
            "- `References:` / `View:` lines - optional header lines under the task file's title (omitted when unused):"
        ));
        assert!(block.contains(
            "bare slugs resolving to `notes/<slug>.md`, the reading list for whoever executes or reviews"
        ));
        assert!(block.contains(
            "`View: <id>-<slug>.html` points to an existing `views/<id>-<slug>.html` file"
        ));
        // Reference maintenance remains tied to the notes a task relies on.
        assert!(block.contains(
            "`References:` carries the why (design rationale, provenance, adjudications); the what stays in Scope/AC/SOP."
        ));
        assert!(block.contains("Read-side only: a note you plan to write is prose in SOP."));
        assert!(block.contains(
            "The coordinator sets references at task creation; a worker adds a reference they write or discover."
        ));
        assert!(block.contains("Neither line gates claiming - the `needs:` tail owns gating."));
        // Single-sided: the consumer declares the edge, so the registry stays
        // about the article and never enumerates the tasks that read it.
        assert!(block.contains(
            "`notes/INDEX.md` is the registry, one gist line per article describing the article alone."
        ));
        assert!(block.contains(
            "Which tasks rest on a note lives in those tasks' `References:`, so the registry never enumerates task ids."
        ));
        // Preserve artifact discovery and keep notes authoritative.
        assert!(block.contains(
            "use a self-contained `views/<id>-<slug>.html` file and add the task's `View:` header."
        ));
        assert!(block.contains(
            "Register the page in `views/INDEX.md` with a `### <id>-<slug>.html` heading and a one-line gist."
        ));
        assert!(block.contains(
            "Notes remain authoritative where a view differs, and existing records can supply the facts without duplication."
        ));
        assert!(block.contains(
            "If a shareable page is published, record its link in Progress alongside the file."
        ));
        // A view can be omitted, requested, left alone, or checked for current use.
        assert!(block.contains(
            "Planning, execution, and review can proceed without an optional view or an explanation for omitting it, including for logic changes."
        ));
        assert!(block.contains(
            "Create a view if the user requests one. Otherwise, use one at your discretion when it helps the reader understand the task."
        ));
        assert!(block.contains(
            "An existing view carries no routine maintenance duty. When using one for a current decision, check the claims that matter."
        ));
        assert!(block.contains(
            "Refresh any outdated claims or clearly mark the view as historical and base the decision on current records."
        ));
        // Records-list order: the edges follow the task file they annotate, and
        // `views/` sits between `notes/` and `scripts/`.
        let at = |needle: &str| {
            block
                .find(needle)
                .unwrap_or_else(|| panic!("missing records bullet: {needle}"))
        };
        let task_file = at("- `tasks/<id>-<slug>.md` -");
        let edges = at("- `References:` / `View:` lines -");
        let notes = at("- `notes/` -");
        let views = at("- `views/` -");
        let scripts = at("- `scripts/` -");
        assert!(task_file < edges && edges < notes && notes < views && views < scripts);
    }

    #[test]
    fn csm_block_gates_evidence_checks_on_need() {
        let block = csm_block("/home/user/.csm");
        assert!(block.contains("Check only evidence the next decision needs"));
        assert!(block.contains(
            "If a condition in the user's instruction fails, report it and stop that instruction. Do not repair the failure merely to activate it."
        ));
        assert!(block.contains("Interruption alone triggers no check."));
        assert!(block.contains("Resume the recorded plan within the mandate. Before using a recorded conclusion, verify the relevant premise."));
        assert!(block.contains("Changes or age are cues to check, not proof of failure."));
        assert!(block.contains(
            "If the premise holds, reuse the conclusion. If the premise changed, update the original record with the new evidence even when the conclusion still holds. Amend or explicitly supersede any affected conclusion and preserve unaffected findings."
        ));
        assert!(block.contains(
            "If a required premise is unverified, record the specific unknown and the evidence needed in the consuming task's Open questions. Pause dependent work and continue independent authorized work."
        ));
        assert!(block.contains(
            "When the record alone cannot support the next step, report what the evidence confirms, what remains unknown or undone, and the next action."
        ));
        assert!(block.contains(
            "Report any conflict with a user requirement and retain the requirement until the user changes it."
        ));
    }

    #[test]
    fn csm_block_assigns_record_ownership() {
        let block = csm_block("/home/user/.csm");
        assert!(block.contains(
            "Update only the relevant entries and distinguish proposals from user decisions."
        ));
        assert!(block.contains(
            "Coordinators maintain state.md, notes, scripts, and the board, create Scope, AC, and SOP, maintain dependencies, and write Review while normalizing Progress."
        ));
        assert!(block.contains("The coordinator maintains this session description."));
        assert!(block.contains(
            "During execution of an existing task, record any needed correction to the description in that task's `Progress` for the coordinator to reconcile; leave `state.md` unchanged."
        ));
        assert!(block.contains("Replace superseded text in place."));
        assert!(block.contains(
            "Workers maintain their task file and board line, follow its SOP, and raise Open questions when stuck. Workers self-claim without assignment or tracking."
        ));
        assert!(block.contains(
            "Claims leave no board mark. Ask before claiming if there is real ownership ambiguity and leave others' work in place."
        ));
        assert!(block.contains(
            "**Cross-repo:** the same session name in each repo shares one state.md + tasks/."
        ));
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
        assert!(block.contains(
            "Check only evidence the next decision needs, including `needs:` before claiming."
        ));
        assert!(block.contains(
            "Skip a task with unmet or unverified dependencies only to select another authorized candidate. If none qualifies, report the wait."
        ));
        // The mapping remains valid when creating and replanning.
        assert!(block.contains("refs must resolve without self-dependency or cycles"));
        assert!(block.contains("requires verified evidence"));
        assert!(block.contains("Reordering changes no `needs:`"));
        assert!(block.contains("dropping a task fixes or removes the refs pointing at it"));
    }

    #[test]
    fn csm_block_carries_the_board_rendering() {
        let block = csm_block("/home/user/.csm");
        // The line's own shape, and where a task's content lives.
        assert!(block.contains("- A board line - `<id> <slug>`, with the `needs:` tail last."));
        // The snapshot renders heads by section, so a task's content is read
        // from its file.
        assert!(block.contains("The `[csm]` block renders the board by section, each task's head and `needs:` tail only"));
        assert!(block.contains("what a task is about lives in `tasks/<id>-<slug>.md`"));
    }
}
