//! The csm working-mode prompt, injected into `~/.claude/CLAUDE.md` (via
//! `csm init`) and optionally a repo's `AGENTS.md` (via `csm <name> --agents-md`).
//!
//! Style: terse, action-first. No tool introduction - just tell the agent what
//! to do when a session is active. No hard-wrapping (each unit on one line).
//! Dormant unless a csm session is active, so safe in the global CLAUDE.md.

pub const CSM_MARK_BEGIN: &str = "<!-- csm:begin -->";
pub const CSM_MARK_END: &str = "<!-- csm:end -->";

/// The full marked block to inject.
pub fn agents_md_block() -> String {
    format!("{}\n{}\n{}", CSM_MARK_BEGIN, AGENTS_MD_BODY, CSM_MARK_END)
}

const AGENTS_MD_BODY: &str = r#"## csm workspace memory

When a `[csm]` block appears in your context, a csm workspace-memory session is active. The block hands you the **workspace directory** and the current `state.md` - use that path directly; do not look up the session name via env vars or files. csm only delivers state at session start. **Keeping `state.md` / `progress.md` current is your job, not csm's.**

- `state.md` - source of truth. Sections: Task, Acceptance criteria, SOP, Progress, Key links, Open questions.
- `progress.md` - append-only timestamped log.
- `scripts/` - shared utility scripts; `scripts/INDEX.md` is the registry.

### Working mode

1. **Orient first.** Read `state.md` fully; skim the `progress.md` tail from the `[csm]` block.
2. **Keep `state.md` tight and authoritative.** Move settled detail to `progress.md`.
3. **Append `progress.md` after each meaningful change** (subtask done, decision, blocker, handoff). Entry: `## YYYY-MM-DD HH:MM - <agent> - <summary>` plus 1-3 bullets. Append only. Never rewrite history.
4. **Maintain `scripts/INDEX.md`.** Add an entry per new script (name, purpose, args, example); update on rename/remove. Read the index before writing a new script.
5. **Before you stop: update `state.md`** (Progress + Open questions current) **and append a `progress.md` handoff line** stating where to resume. Mandatory - the next agent's orientation depends on it.
6. **Cross-repo:** the same session name in each repo shares one `state.md`. Reference the name in commits/PRs.
"#;
