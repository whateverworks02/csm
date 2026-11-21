# csm protocol evaluations

`csm-eval` runs fixed user turns through real Codex using the protocol from the
current checkout. Each case starts in a disposable Git repository and csm
session. Codex gets the seeded files, the protocol installed by `csm init`, and
the case’s user messages. The runner keeps the case’s provenance and expected
checks outside the fixture workspace.

## Run

Use a recent Codex CLI with `exec --json`, `exec resume`, and the
`codex sandbox -P :read-only` command. You also need Rust, Git, Python 3, and a
working Codex login with file-backed credentials, or `OPENAI_API_KEY` in the
environment. The runner uses a fresh Codex configuration and does not import
personal instructions, hooks, or plugins. It verifies the generated `csm hook`
configuration before allowing that hook to run unattended with Codex’s hook
trust flag. Codex workspace write restrictions still apply.

Run these commands from the root of the checkout.

```sh
cargo build --locked --bins
target/debug/csm-eval run
target/debug/csm-eval run execution-handoff
```

Use `--model MODEL` to override the Codex default or `--timeout SECONDS` to
change the 240-second per-turn limit. Later turns resume the same thread.
Exit status `0` means all automated checks passed, `1` means at least one
behavioral check failed, and `2` means the runner could not complete a case.
The suite continues through the remaining cases after a case error.

The runner prints the evidence directory under `evals/runs/`. Each case has a
`result.json` with the invocation, resolved model by turn, protocol reference,
checks, and any run error. `protocol.md` records the protocol with a placeholder
for the disposable csm directory. `AGENTS.md` records the deployed text. The
runner also saves the user messages, Codex JSONL events and stderr, final
replies, command output, and initial and per-turn repo and session snapshots. `HUMAN` points to a reply for manual
assessment. Filesystem checks compare snapshots at turn boundaries, so they can
miss changes that were reverted before the turn ended.

## Cases

| Case | Behavior |
| --- | --- |
| `active-session-boundary` | Diagnose a missing picker, then verify a separate cross-session dependency. Inspect both tool traces for the scope of reads. |
| `discussion-only` | Discuss implementation and verification while preserving the repository and session. |
| `execution-handoff` | Implement and verify the greeting, check that the tested file matches the committed file, record Progress, move the task to Pending review, and preserve Review. Answer the second turn without making changes. |
| `follow-up-question` | Answer a question about a possible follow-up without creating or moving tasks. |

The initial cases come from the earlier eval corpus. Each case records its
source and which inputs were reconstructed. The results describe the fixtures
and the recorded Codex invocation.

## Add a case

Create `cases/<name>/case.json` using an existing case as a template. Set
`provenance.source` and `provenance.reconstruction`, seed text files in the
`repo` and `session` maps keyed by relative path, and put the exact user
messages in `turns`. The optional `other_sessions` map seeds sibling sessions by name with their own relative file maps. Add the `checks` before running the case. Their `turn`
field defaults to `1`. The comparison `baseline` defaults to `initial` and
can be `previous` for turns after the first.

| Check `kind` | Fields and evidence |
| --- | --- |
| `repo_unchanged` | Compares repo files and Git state captured in the Codex read-only sandbox. |
| `session_unchanged` | Compares files in the csm session. |
| `file_unchanged` | Compares the session file at `path`. |
| `task_inventory_unchanged` | Compares the task Markdown filenames. |
| `board_move` | Checks `entry` appears once under `from` at baseline and once under `to` after the turn. |
| `section_updated` | Checks that `section` in session file `path` changed to a nonempty body containing every string in `contains`. |
| `section_unchanged` | Compares the body of `section` in session file `path`. |
| `command_output` | Executes `argv` from the fixture repo in the Codex read-only sandbox and requires exit 0 and exact `stdout`. |
| `other_sessions_unchanged` | Compares sibling session files against the baseline. |
| `human_trace` | Marks the full turn transcript for manual inspection against `expected`. |
| `human_reply` | Marks the saved reply for manual inspection. |

The local calibration test checks the three initial cases against seeded correct
and faulty outcomes. It needs the Codex CLI, Git, and Python, and makes no model
requests.

```sh
cargo test --bin csm-eval -- --include-ignored
```
