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
- **`tasks/INDEX.md`** - the task board. Sections are statuses - `Open` -> `Pending review` -> `Pending fix` -> `Done` - and a task's status is which section its line is in. The operational center: what's claimable, what's under review, what's done.
- **`tasks/<id>-<slug>.md`** - one file per task. Sections: `Scope` (what), `AC` (acceptance criteria), `SOP` (the procedure), `Open questions` (blockers - worker raises, coordinator answers), `Progress` (outcome records: what changed, where - files/PR - and what's left; never a timestamped diary), `Review` (coordinator feedback). The coordinator writes `Scope` + `AC` + `SOP` at create and `Review` at review; the worker executes the `SOP` and appends to `Progress`.
- **`notes/`** - focused deep-dive articles that outlive a single task; `notes/INDEX.md` is the registry.
- **`scripts/`** - shared utility scripts; `scripts/INDEX.md` is the registry.

## Task lifecycle

Roles are action-derived, not assigned: creating or reviewing a task is a coordinator action; claiming or executing one is a worker action. One agent can do both in a session.

1. **Create** (coordinator): write `tasks/<id>-<slug>.md` with `Scope` + `AC` + `SOP`; add a line under `Open` in `tasks/INDEX.md`.
2. **Claim & execute** (worker): pick from `Open` or `Pending fix`; execute the `SOP`, recording outcomes in `Progress`.
3. **Submit** (worker): done or stuck - if stuck, add an `Open questions` bullet first; move the INDEX line to `Pending review`.
4. **Review** (coordinator): approve -> `Done`; or write `Review` + answer `Open questions` -> `Pending fix`.

`state.md` and `tasks/INDEX.md` are the orientation surface - injected into the agent on launch. Per-task files carry the detail; `notes/` and `scripts/` carry reusable knowledge.

## The csm skills

`csm init` ships two skills - the authoring discipline at the pipeline's two variance-prone handoffs:

- **csm-plan** (architect pass): grill the human in one batch (every question names the decision, the options, and what breaks under each), write the `state.md` one-pager, decompose into tasks whose SOPs a weak executor can run (every step ends on a completion criterion the executor itself can check).
- **csm-scout** (scout pass): explore to answer questions, not to tour files - one note per question with `path:line` evidence, claims marked read vs inferred, unknowns listed as `open:` (they are grill material, not failure), options reported but never picked.

Claude gets both as real skills - `/csm-plan`, `/csm-scout`, auto-triggered - and vendor-neutral copies live at `~/.csm/skills/plan.md` and `~/.csm/skills/scout.md`. Update loop is the same as the prompt: upgrade csm, rerun `csm init`.

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
```

> codex: after `csm init`, run `/hooks` in your first codex session and trust the `csm hook` SessionStart entry - codex skips untrusted hooks. Once trusted, csm revives the workspace on `/clear` and compaction.

## Commands

| Command | What it does |
|---------|-------------|
| `csm <name>` | Start or resume a session, launch the agent (default `claude`; `--agent pi`/`codex`) |
| `csm` | Pick a session whose origin is the current directory |
| `csm list` | List all sessions |
| `csm show [name]` | Compact card: context, open/done tasks, scripts, notes |
| `csm detail [name]` | Full `state.md` + task board render |
| `csm init` | (Re)install the hook, the prompt, and the csm skills - rerun after upgrading |
| `csm pin <name>` / `csm unpin` | Protect from / allow garbage collection |
| `csm rename <old> <new>` | Rename and re-home to the current directory |
| `csm rm <name>` | Delete a session and its workspace |
| `csm gc [--older-than N]` | Garbage-collect unpinned sessions |
| `csm doctor [--fix]` | Diagnose and repair consistency |

`show` and `detail` default to `$CSM_SESSION`, else open a picker. `csm init` (run by the installer) installs the hook, the prompt, and the csm skills - rerun it after upgrading csm.

## License

MIT
