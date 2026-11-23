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
cargo install --path . --locked --bin csm
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

Use a single directory name for each session, such as `search` or `api-review`.

## What carries over

Each session lives in `~/.csm/sessions/<name>/` by default. On launch, the
agent gets a snapshot of `state.md` and the board in `tasks/INDEX.md`.
Agents update the Markdown files as they work. Detailed task records live
in `tasks/`, and decisions that span tasks can go in `notes/`.

Ask the agent to record a plan, execute an agreed task, or review submitted
work. Your request determines which step it takes. Use `needs: 003` on a board
line to depend on task 003 in the same session.

## License

[MIT](LICENSE)
