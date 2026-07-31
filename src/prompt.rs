//! The csm working-mode prompt, injected into `~/.claude/CLAUDE.md` via
//! `csm init`.
//!
//! Style: terse, action-first. No tool introduction - just tell the agent what
//! to do when a session is active. No hard-wrapping (each unit on one line).
//! Dormant unless a csm session is active, so safe in the global CLAUDE.md.

pub const CSM_MARK_BEGIN: &str = "<!-- csm:begin -->";
pub const CSM_MARK_END: &str = "<!-- csm:end -->";

/// The full marked block to inject. `csm_home` is rendered into the prompt's
/// path so a relocated `$CSM_HOME` is reflected - an agent follows the actual
/// home, not a hardcoded `~/.csm`.
pub fn csm_block(csm_home: &str) -> String {
    format!(
        "{begin}\n\
## csm workspace memory

A csm session is active iff `$CSM_SESSION` is set. Orient on `state.md` + `progress.md` at `{csm_home}/sessions/$CSM_SESSION/` (a `[csm]` block, if present, is only a snapshot of these). If `$CSM_SESSION` is unset, there is no csm session. **Keeping `state.md` / `progress.md` current is your job, not csm's.**

- `state.md` - source of truth. Sections: Task, Acceptance criteria, SOP, Progress, Key links, Open questions.
- `progress.md` - append-only timestamped log.
- `notes/` - focused deep-dive articles; `notes/INDEX.md` is the registry.
- `scripts/` - shared utility scripts; `scripts/INDEX.md` is the registry.

### Working mode

1. **Orient first.** Read `state.md` fully; skim the `progress.md` tail and `notes/INDEX.md`.
2. **Keep `state.md` tight and authoritative.** Move settled detail to `progress.md`; move deep dives to `notes/`.
3. **Append `progress.md` after each meaningful change** (subtask done, decision, blocker, handoff). Entry: `## YYYY-MM-DD HH:MM - <agent> - <summary>` plus 1-3 bullets. Append only. Never rewrite history.
4. **Maintain `scripts/INDEX.md` and `notes/INDEX.md`.** Add an entry per new script/note; update on rename/remove. Read the index before writing a new one.
5. **Before you stop: update `state.md`** (Progress + Open questions current) **and append a `progress.md` handoff line** stating where to resume. Mandatory - the next agent's orientation depends on it.
6. **Cross-repo:** the same session name in each repo shares one `state.md`. Reference the name in commits/PRs.
{end}",
        begin = CSM_MARK_BEGIN,
        end = CSM_MARK_END,
        csm_home = csm_home,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csm_block_renders_home_into_path() {
        let block = csm_block("/data/csm");
        assert!(block.contains("/data/csm/sessions/$CSM_SESSION/"));
        assert!(!block.contains("~/.csm/sessions"));
        assert!(block.starts_with(CSM_MARK_BEGIN));
        assert!(block.ends_with(CSM_MARK_END));
    }

    #[test]
    fn csm_block_default_home_path() {
        let block = csm_block("/home/user/.csm");
        assert!(block.contains("/home/user/.csm/sessions/$CSM_SESSION/"));
    }
}
