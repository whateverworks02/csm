# csm

**Workspace memory for coding agents - cross-time, cross-repo, multi-agent.**

[![CI](https://github.com/whateverworks02/csm/actions/workflows/ci.yml/badge.svg)](https://github.com/whateverworks02/csm/actions/workflows/ci.yml)
[![latest release](https://img.shields.io/github/v/release/whateverworks02/csm)](https://github.com/whateverworks02/csm/releases)
[![license: MIT](https://img.shields.io/github/license/whateverworks02/csm)](LICENSE)
![platform: macOS arm64](https://img.shields.io/badge/platform-macOS%20arm64-lightgrey)

csm gives every task a durable, agent-neutral workspace-memory directory. Start a session with `csm <name>` and the workspace is injected into the agent on launch - and again on every `/clear`. The agent keeps the memory current; csm provides the directory, the prompt, and the hook.

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
- **`tasks/INDEX.md`** - the task board. Sections are statuses - `Open` -> `Pending review` -> `Pending fix` -> `Done` - and a task's status is which section its line is in. The operational center: what's claimable, what's under review, what's done. `Done` means the local loop closed - executed, reviewed locally; a PR, if any, is raised on the user's explicit go-ahead after the review; CI/approval/merge are outside csm. Within a section, line order = execution order: ids are stable handles, so replanning re-slots lines (reorder/insert/split/drop) instead of renumbering, and the board - not chat - carries the plan. A task's preconditions ride on the same line as a `needs:` tail (see [Dependencies](#dependencies)) - the only dependency mapping, with no second graph file to maintain.
- **`tasks/<id>-<slug>.md`** - one file per task. Sections: `Scope` (what), `AC` (acceptance criteria), `SOP` (the procedure), `Open questions` (blockers - worker raises, coordinator answers), `Progress` (outcome records: what changed, where - files/PR - and what's left; never a timestamped diary), `Review` (coordinator feedback). The same sections serve every task, scaled to its size (see [Scaling the record](#scaling-the-record-to-the-task)). The coordinator writes `Scope` + `AC` + `SOP` at create and `Review` at review; the worker executes the `SOP` and appends to `Progress`.
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

Roles are action-derived, not assigned: creating or reviewing a task is a coordinator action; claiming or executing one is a worker action. One agent can serve both roles on different tasks; a worker submits its own work for coordinator review.

A normal continuation resumes the plan already on the board: planning (`/csm-plan`) starts on a real planning need - a new mission, or a decision that would change the plan and can't be settled from what's recorded - and scouting (`/csm-scout`) runs around a specific question whose answer needs the code.

1. **Create** (coordinator): write `tasks/<id>-<slug>.md` with `Scope` + `AC` + `SOP` sized to the task; add its line under `Open` at its execution position (next free id, wherever it slots) with its `needs:` tail when it has real preconditions.
2. **Claim & execute** (worker): pick from `Open` or `Pending fix` whose `needs:` hold; execute the `SOP`, recording outcomes in `Progress`.
3. **Submit** (worker): done or stuck - if stuck, add an `Open questions` bullet first; move the INDEX line to `Pending review`. The worker's responsibility for this task ends here; self-checks or checks by other agents do not authorize the worker to write its `Review` or move it to `Done`.
4. **Review** (coordinator): approve -> `Done`; or write `Review` + answer `Open questions` -> `Pending fix`.

`state.md` and `tasks/INDEX.md` are the orientation surface - injected into the agent on launch. Per-task files carry the detail; `notes/` and `scripts/` carry reusable knowledge.

### Scaling the record to the task

Every task keeps the same six sections and the same lifecycle; only the depth changes. Every task has verifiable `AC`; `Open questions` carries content only when there is something to say.

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

- **csm-plan** (architect pass): for a new mission or a replan - grill the human in one batch on the questions that would change the plan (each names the decision, the options, and what breaks under each; a pass with none asks none), write the `state.md` one-pager, decompose into tasks whose `SOP`s are sized to the work (see [Scaling the record](#scaling-the-record-to-the-task)).
- **csm-scout** (scout pass): for a specific open question whose answer needs the code - explore to answer questions, not to tour files - one note per question with `path:line` evidence, claims marked read vs inferred, unknowns listed as `open:` (they are grill material, not failure), options reported but never picked.

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
| `csm gc [--older-than N]` | Garbage-collect unpinned sessions |
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
