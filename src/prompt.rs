pub const CSM_MARK_BEGIN: &str = "<!-- csm:begin -->";
pub const CSM_MARK_END: &str = "<!-- csm:end -->";

/// Render the marked prompt block using the configured csm home.
pub fn csm_block(csm_home: &str) -> String {
    format!(
        "{begin}\n\
## csm workspace memory

A csm session is active iff `$CSM_SESSION` is set. Read `state.md` (Context) and `tasks/INDEX.md`, and skim `notes/INDEX.md` at `{csm_home}/sessions/$CSM_SESSION/`. A `[csm]` block, if present, is only a snapshot of these files. If `$CSM_SESSION` is unset, there is no csm session. **You maintain these files, not csm.**

Search, read, and update CSM records within the active session by default. If it has no matching task, continue within the requested phase using relevant project files. Read another session only as needed for an explicit user request for cross-session work or a specific cross-session reference in the active records. Updating another session requires explicit user direction.

### Turn decisions

Before acting, identify the work concerned, the action the user requested, and its endpoint.

1. **Authority.** Follow this workflow while the session is active and correct conflicting workflow rules in memory or habit. The user sets requirements and authorizes work. Read the current request in context and use the conversation or recorded user decisions to recover earlier authorization. The user's latest instruction about that work takes precedence wherever it conflicts with earlier instructions or records. Resume unfinished work under that mandate after an interruption or context compaction without asking for approval again. Board status, facts, role, urgency, and newly discovered work confer no authority. If the next action requires authorization you cannot establish, ask for the specific decision you need.
2. **Scope.** Identify the requested outcome and keep questions, reviews, and changes with the task they concern. Classify PR feedback by what it requests. Its source alone cannot authorize a change. A named-task request stays with that task. A request to execute the next eligible task lets you select an authorized Open or Pending fix line in board order. Create a new task only for a separate, independently reviewable deliverable the user has confirmed. Maintain in-scope records directly without a new board line.
3. **Phase.** Apply the table to the requested outcome in context. Plan, execute, review, and open a PR as separate phases, even if one agent serves all of them. When scope, phase, and the prerequisites for the next step are clear, proceed toward the endpoint. Within the mandate, make reasonable, reversible implementation choices, investigate, implement, test, fix defects, correct facts, and maintain records without per-step approval. Ask when the user's decision is needed to resolve scope or a required prerequisite. Propose work beyond the mandate for a user decision.
4. **Prerequisites.** Check only evidence the next decision needs, including `needs:` before claiming. Skip a task with unmet or unverified dependencies only to select another authorized candidate. If none qualifies, report the wait. If a condition in the user's instruction fails, report it and stop that instruction. Do not repair the failure merely to activate it. Interruption alone triggers no check.

   Resume the recorded plan within the mandate. Before using a recorded conclusion, verify the relevant premise with available evidence. If that evidence is insufficient or there is a concrete reason it may no longer apply, check only what would resolve the next decision. Reuse the conclusion when the premise holds. If the premise changed, update the original record with the new evidence even when the conclusion still holds. Amend or explicitly supersede any affected conclusion and preserve unaffected findings.

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
}
