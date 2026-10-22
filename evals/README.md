# evals

The behavioral corpus for the `[csm]` block. Each scenario is a fixture (a session's `state.md` and `tasks/INDEX.md`, a tiny git repo, and optionally a sibling checkout), a fixed user-turn script, a correct-behavior instruction for the `explicit` arm, and a deterministic check set. `csm-eval` replays a scenario against a real harness and grades the artifacts; see `src/bin/csm-eval/`.

Reconstructed scenarios are regression verification and claim no reproduction of the original incidents.

## Splits

- `dev` - the scenarios prompt iteration sees. One scenario per ruled behavior, plus the two post-v0.7 cases that already have a ruled routing (review surface, record maintenance).
- `holdout` - run once per candidate, never during prompt iteration. It carries the second scenario for a behavior, the cases without a dev representative, and the two observation scenarios.

The split is a prompt-overfitting guard, not a difficulty ranking. A change that only fixes the dev half has not been verified.

## Corpus index

| Scenario | Split | Behavior | What it reconstructs |
| --- | --- | --- | --- |
| `execute-stops-at-review-seed` | dev | `execution-stops-at-pending-review` | A completed execution stops at Pending review and waits; a follow-up question answers from the resumed state. |
| `review-stays-on-the-task` | dev | `review-reviews-the-original-task` | A review request on a submitted task lands as a Review in that task and moves its line, not as a new task. |
| `routing-follow-up-question` | dev | `follow-up-question-changes-nothing` | Two questions about the board are answered with the board, the task files, and the repo untouched. |
| `modification-stays-on-the-task` | dev | `modification-uses-the-original-task` | A small follow-up change revises the original task in place; no new task appears. |
| `fact-fix-leaves-requirements` | dev | `fact-fix-leaves-requirements` | A stale record is corrected in Progress; the requirement, the phase, and the line stay put. |
| `open-is-not-authorization` | dev | `open-is-not-authorization` | A plain `continue` with an Open task but no recorded mandate executes nothing and changes nothing. |
| `open-pr-is-the-review-surface` | dev | `open-pr-is-the-review-surface` | A test fix on a task whose PR is already open rides that PR; the task stays Done. |
| `note-request-creates-no-task` | dev | `record-maintenance-creates-no-task` | Writing and registering a note is record maintenance; the board is untouched. |
| `resume-continues-the-authorized-task` | dev | `resume-continues-the-authorized-task` | A recorded mandate resumes after an interruption and ends at Pending review. |
| `pr-conflict-stays-done` | holdout | `open-pr-is-the-review-surface` | A conflict fix and a PR-description rewrite on an open PR leave the task at Done. |
| `replan-reopens-to-pending-fix` | holdout | `replan-status-routing` | A plan-only amendment reopens a Done task to Pending fix, not to Pending review. |
| `user-move-is-the-verdict` | holdout | `user-move-is-the-verdict` | A user instruction naming the outcome is the status decision; the Review records it. |
| `no-bookkeeping-in-the-artifacts` | holdout | `no-bookkeeping-in-project-artifacts` | An internal token and the workspace path stay out of the repository, its commits, and the PR surface. |
| `cross-repo-needs` | holdout | `cross-repo-needs-gates-execution` | Two checkouts share one session; a Done dependency releases one task, an unmet partner dependency does not. |
| `multi-task-grant-stop` | holdout | `multi-task-grant-stop` | Observation: one grant names two tasks. The run records whether the agent stops per task or at the batch end. |
| `cross-phase-delegation` | holdout | `cross-phase-delegation` | Observation: one turn delegates implementation and review. The run records where the agent stops. |

Observation scenarios assert only hygiene (a reply exists, no push, no PR, no workspace path in output); they deliberately do not assert a board position, because the position is the behavior under observation.

## Layout

```
evals/
  arms/v0.6/csm-block.md        # the known-bad contrast block
  scenarios/<name>/scenario.json # fixture, script, explicit instruction
  scenarios/<name>/checks.json   # per-turn and final-state assertions
```

A scenario may add `siblings` to seed a second checkout beside the cwd repo. Both checkouts share the fixture session; a sibling is not nested in the cwd repo, so its commits do not land in this repo's history. The grader snapshots the cwd repo and the session only.

## Running

```
cargo build
csm-eval run execute-stops-at-review-seed --arm working --k 3
csm-eval run --split dev --arm explicit --k 1
csm-eval grade <run-dir>
csm-eval diff <run-a> <run-b>
```

The headline is the delta between arms, never an absolute score. `--split holdout` runs the half prompt iteration must not see.
