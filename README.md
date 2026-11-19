# csm

csm gives coding agents a shared memory for a project. Start a named session,
then pick up its plan, task board, and notes in another agent or repository.
The workspace is plain Markdown on disk.

## Install

The [latest release](https://github.com/whateverworks02/csm/releases/latest)
has a prebuilt binary for Apple Silicon Macs.

```sh
curl -fsSL https://raw.githubusercontent.com/whateverworks02/csm/main/install.sh | bash
```

The installer sets up agent instructions and hooks. Add `~/.local/bin` to
your `PATH` if prompted.

You can also build from a local clone with Cargo.

```sh
cargo install --path . --locked
csm init
```

## Quick start

Run csm from your project directory with your agent’s CLI on `PATH`.

```sh
csm search                   # Start or reopen “search” in Claude Code
csm search -a codex          # Open the same workspace in Codex
csm show search              # See the session context and task board
csm                          # Pick a session, including from another repo
```

Use `-a` to select `claude` (the default), `codex`, `pi`, or `opencode`.
In Codex, run `/hooks` and trust the `csm hook` entry to load session context.
Run `csm --help` for all commands.

## What carries over

Each session lives in `~/.csm/sessions/<name>/` by default. On launch, the
agent gets a snapshot of `state.md` and the board in `tasks/INDEX.md`.
Agents update the Markdown files as they work. Detailed task records live
in `tasks/`, and decisions that span tasks can go in `notes/`.

Ask the agent to record a plan, execute an agreed task, or review submitted
work. Your request determines which step it takes. The board can track
dependencies between tasks in the same session or across sessions.

## Upgrading

Run `csm init` to refresh agent instructions and hooks. If your own
instructions tell agents to start tasks automatically from board status,
update them to require an explicit execution request.

If an older version installed `csm-plan` and `csm-scout`, remove their
files with the commands below. Set `csm_legacy_home` to the previous csm
data directory if it has moved.

```sh
csm_legacy_home=${CSM_HOME:-"$HOME/.csm"}
rm -f "$HOME/.claude/skills/csm-plan/SKILL.md" \
      "$HOME/.claude/skills/csm-scout/SKILL.md"
rm -f "$csm_legacy_home/skills/plan.md" "$csm_legacy_home/skills/scout.md"
```

## License

[MIT](LICENSE)
