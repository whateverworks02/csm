# csm

**Workspace memory for coding agents - cross-time, cross-repo, multi-agent.**

[![CI](https://github.com/whateverworks02/csm/actions/workflows/ci.yml/badge.svg)](https://github.com/whateverworks02/csm/actions/workflows/ci.yml)
[![latest release](https://img.shields.io/github/v/release/whateverworks02/csm)](https://github.com/whateverworks02/csm/releases)
[![license: MIT](https://img.shields.io/github/license/whateverworks02/csm)](LICENSE)
![platform: macOS arm64](https://img.shields.io/badge/platform-macOS%20arm64-lightgrey)

csm gives every task a durable, agent-neutral workspace-memory directory. Start a session with `csm <name>` and the workspace is injected into the agent on launch - and again on every `/clear`. The agent keeps the memory current; csm provides the directory, the prompt, and the hook.

**Workflow compatibility:** task status now identifies candidate work rather than authorizing execution. Agents carry out the user's current mandate and stop at phase boundaries. After upgrading, run `csm init` and update custom instructions that assumed automatic execution from the board. Existing task files need no migration.

## The workspace

Each session is a directory at `~/.csm/sessions/<name>/`:

```
<name>/
├── state.md
├── tasks/
│   ├── INDEX.md
│   └── <id>-<slug>.md
├── notes/
└── scripts/
```

- **`state.md`** - the session one-pager. Sections: `Context` (what this session is and why), `Key links` (repo, docs, related sessions). Read on every launch to recall what the session is about.
- **`tasks/INDEX.md`** - the task board. Sections are statuses - `Open` -> `Pending review` -> `Pending fix` -> `Done` - and a task's status is which section its line is in. The operational center: what's claimable, what's under review, what's done. `Done` means the local loop closed - executed and reviewed locally; PR creation, CI, approval, and merge are not part of that status. Later code changes alone do not reopen a historical `Done` task; a requested change follows the request table in [Task lifecycle](#task-lifecycle). Within a section, line order = execution order: ids are stable handles, so replanning re-slots lines (reorder/insert/split/drop) instead of renumbering, and the board - not chat - carries the plan. A task's preconditions ride on the same line as a `needs:` tail (see [Dependencies](#dependencies)) - the only dependency mapping, with no second graph file to maintain.
- **`tasks/<id>-<slug>.md`** - one file per task. Sections: `Scope` (what), `AC` (acceptance criteria), `SOP` (the procedure), `Open questions` (blockers - worker raises, coordinator answers), `Progress` (outcome records: what changed, where - files/PR - and what's left; never a timestamped diary), `Review` (coordinator feedback). The same sections serve every task, scaled to its size (see [Scaling the record](#scaling-the-record-to-the-task)). The coordinator writes `Scope` + `AC` + `SOP` at create and `Review` at review; the worker executes the `SOP` and appends to `Progress`. Internal names and background stay in the task file - they never enter code, commits, or PRs.
- **`notes/`** - focused deep-dive articles that outlive a single task; `notes/INDEX.md` is the registry.
- **`scripts/`** - shared utility scripts; `scripts/INDEX.md` is the registry.

### Dependencies

A task's preconditions live on its board line as a `needs:` tail - the single dependency mapping, which agents read straight from the board:

```text
## Open
- 002 api - implement the endpoint needs: 001
- 003 frontend - build the form needs: 001
- 004 integration - end-to-end test needs: 002, 003
- 005 docs - document the endpoint

## Done
- 001 contract - publish the v2 schema
```

`002` and `003` fork off `001`; `004` joins them. `005` has no preconditions, so it carries none.

- `needs: <task-id>` - a task in this session. `needs: <session>/<task-id>` - another session's board, for work that spans repos: `needs: api-contract/012 [interface PR merged]`.
- No condition = the upstream reached local `Done` on its board. A bracketed condition names what else the upstream owes; a local `Done` never implies it, so an unconfirmed delivery stays written as a condition rather than being assumed met.
- Several refs are comma-separated, and all must hold before the task is claimable.
- The tail stays refs and conditions; the evidence for a delivery is recorded in the consuming task's `Progress`.
- Line order within a section is execution order among the startable tasks - order alone is not a dependency.
- Replanning edits the same tails: reordering lines changes no relation, and dropping a task fixes or removes the refs pointing at it, so no dangling edge survives.
- Sessions written before this rule keep working as-is: no `needs:` means no preconditions, and nothing is migrated.

## Task lifecycle

Before acting, an agent identifies the work concerned, the requested action, and its endpoint. Three things stay separate: the **process** (the prompt defines the workflow; a conflicting rule in memory or habit is stale), the **mandate** (the user determines requirements and authorizes work; earlier authorization remains valid for unfinished work within its scope and phase), and the **records** (files preserve plans, user decisions, facts, and results; code and runs can correct recorded facts, but cannot change a user decision or grant permission).

The board identifies candidate work - `Open` and `Pending fix` - without authorizing execution. A request to execute the next eligible task permits selection from it, a request about a named task stays with that task, and an existing mandate resumes across a restart or interruption; authorization that cannot be established from the conversation or records is clarified before proceeding. Within the authorized scope and phase the agent investigates, implements, tests, fixes defects, and corrects facts without per-step approval; work beyond the boundary is proposed for the user to decide.

| Request | Action | Endpoint |
| --- | --- | --- |
| Discuss or confirm a plan | Investigate and propose the approach, task breakdown, and procedure; on confirmation, create new tasks in `Open` and revise existing tasks in place, preserving existing statuses unless the request calls for a transition in the table | Proposal delivered, or confirmed plan recorded |
| Ask for an explanation | Answer from the relevant task, verifying the facts the answer needs | Answer delivered; requirements and status unchanged |
| Execute agreed work | Implement and verify within scope; record outcomes and any blocker | Task records updated; line at `Pending review`, whether completed or stuck |
| Review submitted work | Check against `AC`; write `Review` and answer `Open questions`; approve to `Done` or return to `Pending fix` | Verdict recorded; reviewed board lines moved |
| Request a change to existing work | Amend that task's `Scope`/`AC`/`SOP` as needed; reopen submitted or `Done` work to `Pending fix`; implement and verify the requested change | Revised plan recorded if that was the limit; otherwise the execution endpoint |
| Authorize opening a PR after local review | Create the PR from the reviewed work; record its link in `Progress` | Link recorded |

Questions, reviews, and modifications belong to the task they concern. A new task requires the user's confirmation of a separate, independently reviewable deliverable. PR feedback is classified by what it requests; its source does not itself authorize a change. The same agent may serve different phases on the same task: roles describe record-maintenance duties - creating or reviewing a task is coordinator work, claiming or executing is worker work - they do not grant permission to enter a phase.

A normal continuation resumes the plan already on the board, within the mandate. When the record cannot support the current next step, the agent checks only the evidence that step needs, then says what is confirmed, what remains, what is unknown, and the next action. An interruption alone triggers no check. Claims leave no board mark: another worker's activity is unknowable, so a real ambiguity is a question for the user rather than a takeover, and work the agent did not do stays in place.

Recorded conclusions are checked when a task leans on them, not when they're read. Before a conclusion drives the current work, the agent checks the premise it rests on - only as far as needed for the current decision. The conclusion is used while the premise holds; when the premise has changed, the agent amends or explicitly supersedes the conclusion in its original record and retains the evidence; when it cannot be confirmed, the specific unknown and the evidence needed go to the consuming task's `Open questions`, and other authorized work continues. A changed file, a new commit, or an old note is a cue to look, never proof the premise no longer holds. When evidence contradicts a user requirement, the agent reports the discrepancy and retains the requirement until the user changes it; correcting a factual record does not authorize a lifecycle transition.

Planning (`/csm-plan`) starts on a real planning need - a new mission, or a decision that would change the plan and can't be settled from what's recorded - and scouting (`/csm-scout`) runs around a specific question whose answer needs the code.

1. **Create** (coordinator): write `tasks/<id>-<slug>.md` with `Scope` + `AC` + `SOP` sized to the task; add its line under `Open` at its execution position (next free id, wherever it slots) with its `needs:` tail when it has real preconditions.
2. **Claim & execute** (worker): under an execution mandate, pick from `Open` or `Pending fix` whose `needs:` hold; execute the `SOP`, recording outcomes in `Progress`.
3. **Submit** (worker): done or stuck - if stuck, add an `Open questions` bullet first; move the INDEX line to `Pending review`.
4. **Review** (coordinator, on the user's review request): approve -> `Done`; or write `Review` + answer `Open questions` -> `Pending fix`.

Plan, execute, review, and opening a PR are separate phases. At the endpoint in the table, or an earlier limit the user sets, the agent updates the records and stops for user input before the next phase. A conditional instruction whose condition fails ends with the failure reported - the failure is never repaired merely to activate the instruction - and the files are left sufficient to resume: results, remaining work, and the endpoint reached.

`state.md` and `tasks/INDEX.md` are the orientation surface - injected into the agent on launch. Per-task files carry the detail; `notes/` and `scripts/` carry reusable knowledge.

### Scaling the record to the task

Every task keeps the same six sections and the same lifecycle; only the depth changes. Every task has verifiable `AC` - verifiable in the local loop, never a PR, CI, or merge outcome; `Open questions` carries content only when there is something to say.

A simple fix - one behavior, no handoff - shown after review:

```markdown
# 006 - `csm list` panics on an empty index

## Scope
`csm list` panics when no sessions are indexed; return an empty listing instead.

## AC
- `csm list` on an empty index prints the empty-state line and exits 0.
- Listing behavior with sessions present is unchanged.

## SOP
Guard the empty case in `src/main.rs`; add a regression test.

## Open questions

## Progress
- Guarded the empty index in `src/main.rs`; regression test added; `cargo test` green.

## Review
- Approved - both AC verified.
```

Cross-repo work another agent may pick up keeps the same sections, with executor-grade steps (every step ends on a completion criterion the executor itself can check) and a `needs:` tail on its board line - also shown after review:

```markdown
# 007 - client batch retry against the v2 endpoint

## Scope
Send batch requests through the v2 endpoint once the server contract lands; keep a v1 fallback.

## AC
- Batch calls use v2 when the contract is available; a 404 falls back to v1.
- The integration test covers both paths.

## SOP
1. Add the v2 request builder in `src/api/batch.rs`; `cargo test api::batch` green.
2. Wire the 404 -> v1 fallback in `src/api/mod.rs`; the test covers the fallback path.
3. Run the integration suite against staging; record the result in `Progress`.

## Open questions

## Progress
- v2 builder and fallback wired; integration suite green against staging.

## Review
- Approved - the fallback path is covered by the test.
```

Board line: `- 007 batch-retry - v2 with v1 fallback needs: api-contract/012 [v2 PR merged]`.

## The csm skills

`csm init` ships two skills - the authoring discipline at the pipeline's two variance-prone handoffs:

- **csm-plan** (architect pass): for a new mission or a replan - resolve the decisions that affect the plan, investigating missing facts with targeted code reads and asking the user for the choices that need their judgment (grouped, with options and tradeoffs), update the `state.md` one-pager, and decompose into tasks with verifiable outcomes, `needs:` tails, and `SOP`s sized to the work (see [Scaling the record](#scaling-the-record-to-the-task)).
- **csm-scout** (scout pass): for a specific open question whose answer needs the code - frame the decision it informs, read the files that can supply the evidence rather than touring the tree, and record the findings in the relevant note (or a new one): the answer with `path:line` evidence and, for a conclusion a later task will reuse, the premise it rests on, observations distinguished from inferences, and specific unknowns with the evidence they need (material for the planning pass, not failure).

Claude gets both as real skills - `/csm-plan`, `/csm-scout`, auto-triggered - and opencode auto-loads that same `~/.claude/skills/` directory as external skills, so it reads them with no separate deployment. Vendor-neutral copies live at `~/.csm/skills/plan.md` and `~/.csm/skills/scout.md`. Update loop is the same as the prompt: upgrade csm, rerun `csm init`.

## Install

**macOS (Apple Silicon):**

```sh
curl -fsSL https://raw.githubusercontent.com/whateverworks02/csm/main/install.sh | bash
```

The installer puts the binary in `~/.local/bin` and runs `csm init` (the hook, the prompt, and the csm skills) and `csm doctor` for you. Add `~/.local/bin` to `PATH` if it says so, then `csm <name>`.

**From source** (any platform with Rust):

```sh
cargo install --path .
csm init
```

> Linux / Intel-macOS prebuilts aren't out yet - build from source for now.

## Quickstart

```sh
cd ~/proj/my-task
csm my-task                 # create/resume "my-task", launch claude (default)
csm my-task --agent pi      # same session, launch pi
csm my-task --agent codex   # same session, launch codex
csm my-task --agent opencode # same session, launch opencode
csm                         # pick a session for this directory, launch claude
csm -a pi                   # same picker, launch pi
```

Per-agent shell shortcuts pick when called bare (`csp my-task` starts pi; bare `csp` opens the picker):

```sh
# ~/.zshrc
csp() { csm "$@" -a pi }
csx() { csm "$@" -a codex }
cso() { csm "$@" -a opencode }
```

> codex: after `csm init`, run `/hooks` in your first codex session and trust the `csm hook` SessionStart entry - codex skips untrusted hooks. Once trusted, csm revives the workspace on `/clear` and compaction.

## Commands

| Command | What it does |
|---------|-------------|
| `csm <name>` | Start or resume a session, launch the agent (default `claude`; `--agent pi`/`codex`/`opencode`, before or after the name) |
| `csm [-a <agent>]` | Pick a session whose origin is the current directory, launch it with that agent (default `claude`) |
| `csm list` | List all sessions |
| `csm show [name]` | Compact card: context, tasks, needs, ready, scripts, notes |
| `csm detail [name]` | Full `state.md` + task board render |
| `csm init` | (Re)install the hook, the prompt, and the csm skills - rerun after upgrading |
| `csm pin <name>` / `csm unpin` | Protect from / allow garbage collection |
| `csm rename <old> <new>` | Rename and re-home to the current directory |
| `csm rm <name>` | Delete a session and its workspace |
| `csm gc [--older-than N]` | Garbage-collect unpinned sessions (sessions with unfinished board tasks are kept) |
| `csm doctor [--fix]` | Diagnose and repair consistency |

`show` and `detail` default to `$CSM_SESSION`, else open a picker. `csm init` (run by the installer) installs the hook, the prompt, and the csm skills - rerun it after upgrading csm.

### Reading the plan in `csm show`

The card reads the board's `needs:` tails directly, so the view is the plan the agents read - there is no second graph file to keep in sync, and a replan shows up on the next `show`. One line per live task with refs; `ready` lists what can be claimed now (`Open` and `Pending fix` whose refs all hold):

```text
  needs       002 api (Open) needs 001 contract (Done)
              003 frontend (Open) needs 001 contract (Done)
              004 e2e (Open) needs 002 api (Open), 003 frontend (Open)
  ready       002, 003
```

A line is also its wait reason: 004 is blocked because the refs it names are still `(Open)`. Long dependency lines wrap in the terminal so every ref, status, and condition remains visible. A `[condition]` is never assumed met: `needs: api-contract/012 [interface PR merged]` renders as `api-contract/012 (Done) [interface PR merged] (unverified)`. Retained conditions always require checking in this view; `show` does not read verification evidence from Progress. A ref the board can't resolve shows `(missing)` / `(unknown)` instead of satisfied. A dependency cycle is called out with a `warning:` line. Sessions with no `needs:` tails keep the plain card.

## License

MIT
