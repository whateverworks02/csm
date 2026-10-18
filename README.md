# csm

**Workspace memory for coding agents - session-scoped, cross-repo, multi-agent.**

[![CI](https://github.com/whateverworks02/csm/actions/workflows/ci.yml/badge.svg)](https://github.com/whateverworks02/csm/actions/workflows/ci.yml)
[![latest release](https://img.shields.io/github/v/release/whateverworks02/csm)](https://github.com/whateverworks02/csm/releases)
[![license: MIT](https://img.shields.io/github/license/whateverworks02/csm)](LICENSE)
![platform: macOS arm64](https://img.shields.io/badge/platform-macOS%20arm64-lightgrey)

`csm <name>` gives each task a durable directory of plans, decisions, and results, injected into the agent on launch. The records, not the chat transcript, are the state.

- **Sessions as directories** - plain Markdown files on disk, readable and editable without csm.
- **Any agent, any time** - claude, pi, codex, and opencode read the same workspace; switch agents mid-task.
- **Cross-repo** - one session spans repositories, and a task can depend on another session's board.
- **A real task board** - a task's status is the section its line sits in, with dependencies on the same line.
- **Automatic recall** - injected on launch, and revived on `/clear` for claude and codex.
- **User-gated execution** - the board proposes work; your request authorizes it.

The authoritative rules live in the prompt `csm init` writes into each agent's instruction file (`~/.claude/CLAUDE.md` for claude); this page is the human-facing summary.

## Install

macOS (Apple Silicon):

```sh
curl -fsSL https://raw.githubusercontent.com/whateverworks02/csm/main/install.sh | bash
```

Installs to `~/.local/bin` (add it to `PATH` if prompted), then runs `csm init` and `csm doctor`.

From source (any platform with Rust):

```sh
cargo install --path .
csm init
```

Linux and Intel macOS builds aren't published yet.

## Quickstart

```sh
cd ~/proj/my-task
csm my-task                  # create or resume "my-task", launch claude
csm my-task -a codex         # same session, launch codex
csm                          # pick a session for this directory
```

Agents: `claude` (default), `pi`, `codex`, `opencode`. `-a` may lead or trail the name.

claude and codex revive the workspace on `/clear` and compaction, through the SessionStart hook `csm init` installs. pi and opencode are injected at launch only. codex skips untrusted hooks, so run `/hooks` in your first codex session and trust the `csm hook` entry.

Shell shortcuts, for when you call one agent bare:

```sh
# ~/.zshrc
csp() { csm "$@" -a pi; }
csx() { csm "$@" -a codex; }
cso() { csm "$@" -a opencode; }
```

## Commands

| Command | What it does |
| --- | --- |
| `csm <name>` | Start or resume a session and launch its agent (`claude` by default) |
| `csm [-a <agent>]` | Pick a session whose origin is this directory |
| `csm list` | List all sessions |
| `csm show [name]` | Compact card: origin, context, task board, what's claimable |
| `csm detail [name]` | Full `state.md` + task board render |
| `csm init` | (Re)install the SessionStart hooks, the prompt, and the skills |
| `csm pin <name>` / `csm unpin <name>` | Protect from / allow garbage collection |
| `csm rename <old> <new>` | Rename and re-home to the current directory |
| `csm rm <name>` | Delete a session and its workspace (`-f` for a pinned one) |
| `csm gc [--older-than N]` | Collect unpinned sessions (unfinished boards are kept) |
| `csm doctor [--fix]` | Diagnose consistency and agent wiring; `--fix` repairs consistency |

`show` and `detail` default to `$CSM_SESSION`, else open a picker.

## The workspace

Each session is a directory at `~/.csm/sessions/<name>/` (set `CSM_HOME` to move the data root):

```text
<name>/
├── state.md
├── tasks/
│   ├── INDEX.md
│   └── <id>-<slug>.md
├── notes/
├── views/
└── scripts/
```

| Path | Holds |
| --- | --- |
| `state.md` | The session one-pager: `Context` (what this session is and why) and `Key links`. |
| `tasks/INDEX.md` | The board. A task's status is the section its line is in: `Open` -> `Pending review` -> `Pending fix` -> `Done`. Line order within a section is execution order. |
| `tasks/<id>-<slug>.md` | One file per task: `Scope`, `AC`, `SOP`, `Open questions`, `Progress`, `Review` - sized to the work, not to the template. |
| `notes/` | Deep-dive articles that outlive a task, entered through `notes/INDEX.md`. |
| `views/` | One HTML page for a task whose logic a human must judge whole, entered through `views/INDEX.md`. |
| `scripts/` | Shared utility scripts, entered through `scripts/INDEX.md`. |

`state.md` and `tasks/INDEX.md` are injected into the agent on launch - they are the orientation surface. The rest is read on demand.

### Dependencies

A task's preconditions ride on its board line as a `needs:` tail, the only dependency mapping:

```text
## Open
- 002 api - implement the endpoint needs: 001
- 003 frontend - build the form needs: 001
- 004 integration - end-to-end test needs: 002, 003
- 005 docs - document the endpoint
```

`needs: <id>` names a task in this session; `needs: <session>/<id>` names another session's board, for work that spans repos. Several refs are comma-separated, and all must hold.

No condition means the upstream's task reached `Done` on its board. A bracketed condition - `needs: api-contract/012 [interface PR merged]` - names what else it owes, and is never assumed met.

## The task lifecycle

The board identifies candidate work; it does not authorize execution. The user's request sets the mandate and its endpoint, and the agent stops there for the next decision:

1. **Create** - write the task file with `Scope`, `AC`, and `SOP`; add its line to `Open` with any `needs:` tail.
2. **Claim and execute** - pick a startable task from `Open` or `Pending fix`, run the `SOP`, record outcomes in `Progress`.
3. **Submit** - move the line to `Pending review`, done or stuck.
4. **Review** - approve to `Done`, or write `Review` and return the task to `Pending fix`.

A plan request ends once the plan is recorded, execution ends at `Pending review`, review ends with a verdict, and opening a PR is a separate authorization after local review.

`Done` means the local loop closed - executed and reviewed locally. PR creation, CI, approval, and merge are not part of it, and a later code change alone does not reopen a historical `Done` task. `AC` is always verifiable in the local loop, never as a PR or CI outcome.

## Reading the plan

`csm show` renders the board by its own sections, in board order, with each task's `needs:` hanging below it - the plan on screen is the plan the agents read:

```text
  tasks       [███████░░░░░░░░░░░░░] 2/6 done
              Open
              ├── ○ 002 eval-grader
              │   └── ✓ 001 eval-runner
              ├── ○ 003 incident-corpus
              │   └── ○ 002 eval-grader
              ├── ○ 004 judge-layer ◆
              │   └── ○ 002 eval-grader
              └── ○ 005 baseline-and-gate
                  ├── ○ 003 incident-corpus
                  └── ○ 004 judge-layer ◆
              Done
              001 eval-runner
              006 prompt-surface-patch
  claimable   002
```

Marks: `○` open, `◐` pending review, `◑` pending fix, `✓` done, `◆` the task has a view, `(unverified)` a `[condition]` whose evidence isn't recorded, `(missing)` an unresolvable ref. `claimable` lists what can be started now; `csm detail` renders the whole board where `show` caps the `Done` list.

## Skills

`csm init` also installs two skills, at the handoffs where authoring discipline matters most:

- **`/csm-plan`** - for a new mission or a replan: resolve the decisions that shape the plan, update `state.md`, and decompose into tasks with verifiable outcomes and `needs:` tails.
- **`/csm-scout`** - for one open question whose answer needs the code: read the files that can supply the evidence, and record the findings with `path:line` citations in the relevant note.

Claude and opencode load them from `~/.claude/skills/`; vendor-neutral copies live at `~/.csm/skills/plan.md` and `scout.md`.

## Upgrading

The board no longer authorizes execution (see [The task lifecycle](#the-task-lifecycle)). Run `csm init` to replace the shared prompt, and update any personal instructions that assumed execution followed the board. Existing task files need no migration.

## License

MIT
