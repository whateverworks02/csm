# csm

**Workspace memory for coding agents - cross-time, cross-repo, multi-agent.**

[![CI](https://github.com/whateverworks02/csm/actions/workflows/ci.yml/badge.svg)](https://github.com/whateverworks02/csm/actions/workflows/ci.yml)
[![latest release](https://img.shields.io/github/v/release/whateverworks02/csm)](https://github.com/whateverworks02/csm/releases)
[![license: MIT](https://img.shields.io/github/license/whateverworks02/csm)](LICENSE)
![platform: macOS arm64](https://img.shields.io/badge/platform-macOS%20arm64-lightgrey)

csm gives every task a durable, agent-neutral workspace-memory directory. Start
a session with `csm <name>` and it injects the session's `state.md` into the
agent on launch - and again on every `/clear`. The agent keeps the memory
current; **csm just provides the directory, the prompt, and the hook.**

```text
$ csm list
NAME                  PIN   LAST ACCESS          ORIGIN
fix-login-bug         *     2026-07-31 14:30     ~/projects/web
refactor-checkout           2026-07-30 11:20     ~/projects/web

$ csm show fix-login-bug
fix-login-bug
  origin      ~/projects/web
  last access 2026-07-31 14:30
  pinned      yes
  task        Fix the silent auth failure on Safari 17. Token refresh returns 200 but the
              session cookie isn't set when the request comes from a cross-origin iframe.
  last        2026-07-31 14:30  root-caused to SameSite=None; Secure requirement
  scripts     repro-safari.sh (1)
  notes       safari-cookie-trap.md (1)
```

## Highlights

- Persists agent state across `/clear`, new sessions, and repos - orientation is one file read, not starting from scratch.
- Plain markdown - diffable, greppable, editable, agent-neutral.
- Multi-agent: Claude Code (default), `pi`, and `codex`.
- No repo pollution - the prompt lives in global `~/.claude/CLAUDE.md`; `csm <name>` never touches repo files.
- Survives `/clear` - Claude Code fires the `SessionStart` hook again, the workspace comes back.

## Install

**macOS (Apple Silicon):**

```sh
curl -fsSL https://raw.githubusercontent.com/whateverworks02/csm/main/install.sh | bash
```

The installer puts the binary in `~/.local/bin` and runs `csm init` (the hook + the prompt) and `csm doctor` for you. Add `~/.local/bin` to `PATH` if it says so, then `csm <name>`.

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

> codex: after `csm init`, run `/hooks` in your first codex session and trust
> the `csm hook` SessionStart entry - codex skips untrusted hooks. Once trusted,
> csm revives the workspace on `/clear` and compaction.

## How it works

Three pieces: a kv **index** of sessions, a per-session **workspace** directory, and a **working-mode prompt** csm injects into `~/.claude/CLAUDE.md` plus a `SessionStart` hook that feeds the active session into your agent.

The index lives at `~/.csm/index.json` (name → metadata). Each workspace lives at `~/.csm/sessions/<name>/` — `state.md` is the source of truth, `progress.md` is an append-only log, and `notes/` and `scripts/` have their own INDEX files.

The "magic" is the prompt. When `$CSM_SESSION` is set, the agent knows to orient on `state.md`, append to `progress.md`, write `notes/` for deep dives, and leave a handoff line before stopping. Without a session, the prompt does nothing — so it's safe in the global `CLAUDE.md`. **csm never writes the memory beyond the initial scaffold — the agent keeps it current.**

The prompt lives in `CLAUDE.md` (not the hook) because Claude Code treats hook-injected context as factual data, and imperative instructions there can trip prompt-injection defenses. So the hook only injects *data* (`state.md` + a `progress.md` tail), `CLAUDE.md` carries the *instructions*, and on `/clear` the still-running process re-fires the hook and re-injects — the workspace comes back without restarting the session.

## License

MIT
