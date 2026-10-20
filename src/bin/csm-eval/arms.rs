//! The vendored historical blocks the contrast arms deploy. An arm that pins a
//! past prompt must not depend on that tag still being reachable, or on its
//! source still compiling, so the block text is checked in under
//! `evals/arms/<arm>/csm-block.md` and rendered here.

use crate::scenario::Arm;
use anyhow::{Context, Result};
use csm::prompt::{CSM_MARK_BEGIN, CSM_MARK_END};
use std::path::{Path, PathBuf};

/// Where the block text goes inside the CSM_HOME path the prompt renders.
const HOME_PLACEHOLDER: &str = "{{CSM_HOME}}";

/// Where the block an arm pins is vendored, if it pins one: a directory named
/// for the arm under `evals/arms/`. The arm's own `vendored_block` decides
/// whether it pins one, so a missing file stays an error rather than an arm
/// that silently deploys the working-tree block instead.
pub fn block_path(repo_root: &Path, arm: Arm) -> Option<PathBuf> {
    arm.vendored_block().then(|| {
        repo_root
            .join("evals")
            .join("arms")
            .join(arm.id())
            .join("csm-block.md")
    })
}

/// Render the vendored block for `arm` against the sandbox's CSM_HOME.
pub fn vendored_block(repo_root: &Path, arm: Arm, csm_home: &str) -> Result<Option<String>> {
    let Some(path) = block_path(repo_root, arm) else {
        return Ok(None);
    };
    let raw = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "reading the vendored {} block at {}",
            arm.id(),
            path.display()
        )
    })?;
    Ok(Some(render(&raw, csm_home).with_context(|| {
        format!("rendering the vendored {} block", arm.id())
    })?))
}

/// Substitute the home path and assert the result is a well-formed marked
/// block: a vendored file that lost its markers would silently deploy as prose
/// outside the region the deploy check reads.
fn render(template: &str, csm_home: &str) -> Result<String> {
    let text = template
        .trim_end_matches('\n')
        .replace(HOME_PLACEHOLDER, csm_home);
    anyhow::ensure!(
        !text.contains("{{"),
        "an unsubstituted placeholder survived rendering"
    );
    anyhow::ensure!(
        text.starts_with(CSM_MARK_BEGIN) && text.ends_with(CSM_MARK_END),
        "the vendored block is not bounded by {CSM_MARK_BEGIN} .. {CSM_MARK_END}"
    );
    anyhow::ensure!(
        text.matches(CSM_MARK_BEGIN).count() == 1 && text.matches(CSM_MARK_END).count() == 1,
        "the vendored block repeats its markers"
    );
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repo's own arms dir - the vendored files are checked in, so the
    /// tests read the real corpus rather than a fixture.
    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn only_pinned_arms_have_a_block() {
        assert!(block_path(&repo_root(), Arm::Working).is_none());
        assert!(block_path(&repo_root(), Arm::NoBlock).is_none());
        assert!(block_path(&repo_root(), Arm::Explicit).is_none());
        let v06 = block_path(&repo_root(), Arm::V06).unwrap();
        assert!(v06.is_file(), "{} is not vendored", v06.display());
        assert_eq!(
            v06.strip_prefix(repo_root()).unwrap(),
            Path::new("evals/arms/v0.6/csm-block.md")
        );
    }

    #[test]
    fn the_vendored_v06_block_renders_into_a_marked_block() {
        let block = vendored_block(&repo_root(), Arm::V06, "/sandbox/csm")
            .unwrap()
            .unwrap();
        assert!(block.starts_with(CSM_MARK_BEGIN));
        assert!(block.ends_with(CSM_MARK_END));
        assert!(
            !block.ends_with('\n'),
            "the trailing newline is not part of it"
        );
        assert!(block.contains("/sandbox/csm/sessions/$CSM_SESSION/"));
        assert!(!block.contains("{{"));
        // The v0.6 block is the pre-revamp prose: it carries no task-creation
        // gate and no review-surface rule, which is what makes it the
        // known-bad arm.
        assert!(!block.contains("Review submitted work"));
    }

    #[test]
    fn rendering_rejects_a_block_without_markers() {
        let err = render("## csm workspace memory\nno markers\n", "/h").unwrap_err();
        assert!(err.to_string().contains("not bounded"), "{err}");
    }

    #[test]
    fn rendering_rejects_a_leftover_placeholder() {
        let template =
            format!("{CSM_MARK_BEGIN}\n{HOME_PLACEHOLDER}\n{{{{OTHER}}}}\n{CSM_MARK_END}");
        let err = render(&template, "/h").unwrap_err();
        assert!(err.to_string().contains("placeholder"), "{err}");
    }
}
