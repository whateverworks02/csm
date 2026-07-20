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

When a `[csm]` workspace memory block appears in your context, a csm session is active. Follow this mode. Workspace: `~/.csm/sessions/<name>/` (name from the block, `$CSM_SESSION`, or `~/.csm/current`).

- `state.md` - source of truth. Sections: Task, Acceptance criteria, SOP, Progress, Key links, Open questions.
- `progress.md` - append-only timestamped log.
- `scripts/` - shared utility scripts; `scripts/INDEX.md` is the registry.

### Working mode

1. Orient first. Read `state.md` fully, skim `progress.md` tail.
2. Keep `state.md` tight and authoritative. Move settled detail to `progress.md`.
3. Log `progress.md` after each meaningful change (subtask done, decision, blocker, handoff). Entry: `## YYYY-MM-DD HH:MM - <agent> - <summary>` plus 1-3 bullets. Append only. Never rewrite history.
4. Maintain `scripts/INDEX.md`. Add entry on new script (name, purpose, args, example). Update on rename/remove. Read index before writing a new script.
5. Prepare handoff before stopping. Make `state.md` Progress and Open questions current. Add `progress.md` entry stating where to resume.
6. Cross-repo: same session name in each repo shares one `state.md`. Reference the name in commits/PRs.
"#;
