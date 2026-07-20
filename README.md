# csm

**Workspace memory for coding agents** - cross-agent, cross-time, cross-repo.

csm gives every task a durable, agent-neutral workspace memory directory. Start
a session with `csm <name>`; from then on Claude Code automatically injects the
session's `state.md` on session start and `/clear`, and the global `CLAUDE.md`
tells the agent how to maintain it.

```
~/.csm/
  index.json              # kv: name -> {origin_pwd, created_at, last_access, pinned}
  current                 # last-started session name (discoverability hint)
  sessions/<name>/
    state.md              # source of truth: Task, AC, SOP, Progress, Key links, Open questions
    progress.md           # append-only, timestamped log
    scripts/
      INDEX.md            # registry of shared scripts (tool discovery)
      *.py                # shared data-washing / utility scripts
```

## The three pillars

1. **A kv index** (`~/.csm/index.json`) of sessions. Key = session name
   (`csm <name>`). Value = `{origin_pwd, created_at, last_access, pinned}`.
2. **A per-session workspace memory directory** (`~/.csm/sessions/<name>/`) -
   just an address and a shared working area.
3. **A carefully maintained working-mode prompt** injected into the global
   `~/.claude/CLAUDE.md` (by `csm init`), plus a `SessionStart` hook that
   auto-injects the active session's `state.md`.

The "magic" is the prompt: it specifies a disciplined working mode - orient on
`state.md`, append to `progress.md`, maintain `scripts/INDEX.md`, prepare
handoffs. The prompt is framed to stay dormant unless a csm session is active,
so it's safe in the global CLAUDE.md. **Writing `state.md` / `progress.md` is
entirely the agent's job**; csm only provides the directory, the prompt, and the
hook.

Why the prompt lives in `CLAUDE.md` (not in the hook's injected context): Claude
Code treats hook-injected `additionalContext` as factual context; imperative
instructions there can trigger prompt-injection defenses. `CLAUDE.md` is a
normal context file where instructions are followed. The hook therefore injects
only factual data (the workspace path + `state.md`), and `CLAUDE.md` carries the
instructions.

## Install

```sh
cargo install --path .     # puts `csm` on ~/.cargo/bin (ensure it's on PATH)
csm init                   # installs the SessionStart hook + injects the prompt into ~/.claude/CLAUDE.md
```

`csm init` is idempotent - it adds a single `SessionStart` hook (`csm hook`) to
`~/.claude/settings.json` and a marked block to `~/.claude/CLAUDE.md`, leaving
all your other settings/content untouched.

## Quickstart

```sh
cd ~/proj/my-task
csm my-task          # create/refresh session "my-task", launch claude
```

What happens on `csm <name>`:
- creates the session in `index.json` (recording `origin_pwd`) if new,
  refreshes `last_access` otherwise;
- scaffolds the workspace (`state.md`, `progress.md`, `scripts/INDEX.md`) if
  missing;
- writes `~/.csm/current` (a discoverability hint);
- runs `claude` with `CSM_SESSION=<name>` exported.

It does **not** modify any file in your repo (the working-mode prompt is global,
in `~/.claude/CLAUDE.md`). Claude Code's `SessionStart` hook then fires, reads
`CSM_SESSION`, and injects the session's `state.md` (+ a `progress.md` tail +
scripts list) into context. The agent reads the working mode from `CLAUDE.md`
and the current state from the hook injection.

## `/clear` revival

`/clear` does **not** restart the `claude` process, so `CSM_SESSION` is still
set. Claude Code fires `SessionStart` again with `source=clear`; the csm hook
re-reads `CSM_SESSION` and re-injects `state.md`. The workspace memory is
revived in place - no need to exit and re-run `csm`.

(`csm <name>` unconditionally rebinds `CSM_SESSION`; the env var is the
per-terminal binding, used only for this in-process revival.)

## Commands

| Command | Description |
| --- | --- |
| `csm <name>` | Start/resume session `<name>` and launch Claude Code. |
| `csm <name> --no-launch` | Set up the session but don't launch `claude` (for other agents). Prints `export CSM_SESSION=<name>`. |
| `csm <name> --agents-md` | Also inject the csm prompt into this repo's `AGENTS.md` (for cross-agent support with Cursor/Codex). |
| `csm start <name>` | Same as `csm <name>`, explicit form (also takes `--no-launch`, `--agents-md`). |
| `csm list` | List sessions (sorted by last access; `*` = pinned). |
| `csm pin <name>` / `csm unpin <name>` | Pin / unpin (pinned sessions are never GC'd). |
| `csm show [name]` | Print a session's workspace path, metadata, and `state.md`. Defaults to `$CSM_SESSION` / `~/.csm/current`. |
| `csm rm <name>` | Hard-delete a session (workspace dir + index entry). `--force` required for pinned; `--yes` skips confirm. |
| `csm gc` | Interactive picker - delete unpinned sessions by index. |
| `csm gc --older-than Nd` | Delete unpinned sessions not accessed in the last N days. (`--yes` skips confirm.) |
| `csm init` | Install the `SessionStart` hook + inject the prompt into `~/.claude/CLAUDE.md`. |
| `csm hook` | Internal - the `SessionStart` hook handler (reads stdin JSON). |

**GC is a hard delete.** Pinned sessions are never listed or deleted by `gc`.

## How the hook works

`csm init` adds to `~/.claude/settings.json`:

```json
{ "hooks": { "SessionStart": [
  { "matcher": "", "hooks": [{ "type": "command", "command": "csm hook" }] }
] } }
```

`csm hook` (the `SessionStart` handler):
- reads the event JSON from stdin (`source` ∈ `startup | resume | clear | compact`);
- reads `CSM_SESSION` from the environment (inherited from the `claude` process
  that `csm <name>` launched);
- if set and known: self-heals the workspace, refreshes `last_access`, and
  prints `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"…"}}`
  containing the workspace path, `state.md`, a `progress.md` tail, and the
  scripts list;
- otherwise: exits 0 with no output (injects nothing).

stdout contains **only** the JSON object; all diagnostics go to stderr.

## Cross-agent / cross-repo

The workspace is just markdown + scripts, so any agent can use it:
- **Claude Code**: the global `CLAUDE.md` (injected by `csm init`) carries the
  working mode; the hook auto-injects `state.md` on start / `/clear`.
- **Other agents** (Cursor, Codex, ...): run `csm <name> --no-launch`, then
  `export CSM_SESSION=<name>` (or point the agent at the path from
  `csm show <name>`). For agents that read `AGENTS.md` (Cursor/Codex), also run
  `csm <name> --agents-md` in that repo to inject the working-mode block.
- **Cross-repo**: the session **name** is the shared handle. Run `csm my-task`
  in both the frontend and backend repos; the same
  `~/.csm/sessions/my-task/state.md` is the shared task memory. Reference the
  session name in commits/PRs.

## Design notes

- **No `resume` handling needed.** csm's "state" (workspace memory) is distinct
  from Claude Code's session state; they don't conflict. If `claude --resume`
  fires `SessionStart` with `source=resume`, the hook just re-injects - harmless
  and helpful.
- **Single-user, single-machine.** Concurrency is not addressed in this version;
  the kv index is a plain JSON file.
- **Agents own the memory.** csm never writes `state.md` / `progress.md` beyond
  the initial scaffold. All updates are the agent's responsibility, guided by
  the `CLAUDE.md` prompt.
- **No repo pollution by default.** The working-mode prompt lives in the global
  `~/.claude/CLAUDE.md`; `csm <name>` does not touch repo files. AGENTS.md
  injection is opt-in (`--agents-md`) for cross-agent repos.

## Uninstall

```sh
# remove the SessionStart hook entry from ~/.claude/settings.json by hand, or edit it
# remove the <!-- csm:begin -->..<!-- csm:end --> block from ~/.claude/CLAUDE.md
rm -rf ~/.csm
cargo uninstall csm
```
