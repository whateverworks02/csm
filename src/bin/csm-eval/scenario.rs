//! Scenario model: the unit of evaluation. One scenario = a fixture (session
//! files + a tiny repo) reconstructing a starting situation, a fixed user-turn
//! script that never branches on what the agent did, and the check set that
//! grades the rep (`checks.json`, loaded by the grader).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// The check set beside `scenario.json`; every scenario carries one.
pub const CHECKS_FILE: &str = "checks.json";

/// The contrast arm under test. The headline number is a delta between arms,
/// never an absolute score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    /// The claim under test: the working-tree [csm] block deployed by `csm init`.
    Working,
    /// Known-unbound control: no csm wiring at all (init is skipped), so the
    /// agent sees a bare harness.
    NoBlock,
    /// Known-bad: the block pinned from the v0.6.0 tag, vendored under
    /// `evals/arms/v0.6/`, swapped in over the deployed block.
    V06,
    /// Known-good: the working-tree wiring, with the scenario's correct-behavior
    /// instruction appended to every scripted turn. Validates the grader.
    Explicit,
}

impl Arm {
    pub fn parse(s: &str) -> Result<Arm> {
        match s {
            "working" => Ok(Arm::Working),
            "no-block" => Ok(Arm::NoBlock),
            "v0.6" => Ok(Arm::V06),
            "explicit" => Ok(Arm::Explicit),
            other => {
                anyhow::bail!("unknown arm {other:?} (expected: working, no-block, v0.6, explicit)")
            }
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Arm::Working => "working",
            Arm::NoBlock => "no-block",
            Arm::V06 => "v0.6",
            Arm::Explicit => "explicit",
        }
    }

    /// The arm whose difference from `working` rides on the turn prompt rather
    /// than on the deployed block.
    pub fn appends_instruction(&self) -> bool {
        matches!(self, Arm::Explicit)
    }

    /// The arm whose block is swapped for a vendored historical one.
    pub fn vendored_block(&self) -> bool {
        matches!(self, Arm::V06)
    }
}

/// The coding-agent harness under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
}

impl Harness {
    pub fn parse(s: &str) -> Result<Harness> {
        match s {
            "claude" => Ok(Harness::Claude),
            "codex" => Ok(Harness::Codex),
            other => anyhow::bail!("unknown harness {other:?} (expected: claude, codex)"),
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
        }
    }
}

/// Which half of the corpus a scenario belongs to. Prompt iteration sees `dev`
/// only; `holdout` runs once per candidate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Split {
    #[default]
    Dev,
    Holdout,
}

impl Split {
    pub fn id(&self) -> &'static str {
        match self {
            Split::Dev => "dev",
            Split::Holdout => "holdout",
        }
    }
}

/// The `--split` selection: one half, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitSelect {
    Dev,
    Holdout,
    All,
}

impl SplitSelect {
    pub fn parse(s: &str) -> Result<SplitSelect> {
        match s {
            "dev" => Ok(SplitSelect::Dev),
            "holdout" => Ok(SplitSelect::Holdout),
            "all" => Ok(SplitSelect::All),
            other => anyhow::bail!("unknown split {other:?} (expected: dev, holdout, all)"),
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            SplitSelect::Dev => "dev",
            SplitSelect::Holdout => "holdout",
            SplitSelect::All => "all",
        }
    }

    pub fn accepts(&self, split: Split) -> bool {
        match self {
            SplitSelect::Dev => split == Split::Dev,
            SplitSelect::Holdout => split == Split::Holdout,
            SplitSelect::All => true,
        }
    }
}

/// The csm session the fixture reconstructs inside the sandbox.
#[derive(Deserialize, Debug)]
pub struct FixtureSession {
    /// Session name bound via `$CSM_SESSION` during the run.
    #[serde(default = "default_session_name")]
    pub name: String,
    /// Full `state.md` content.
    pub state_md: String,
    /// Full `tasks/INDEX.md` board content.
    pub tasks_index_md: String,
    /// Extra files under the session's `tasks/` dir (task bodies), keyed by
    /// relative path.
    #[serde(default)]
    pub task_files: BTreeMap<String, String>,
}

fn default_session_name() -> String {
    String::from("eval-fixture")
}

/// The tiny git repo the agent works in, seeded with fixed files and one
/// initial commit, with a local bare remote so `git push` succeeds harmlessly
/// while staying visible in the trace.
#[derive(Deserialize, Debug)]
pub struct FixtureRepo {
    /// Repo files, keyed by relative path.
    pub files: BTreeMap<String, String>,
    /// The initial commit message.
    pub initial_commit: String,
}

/// A loaded scenario.
#[derive(Deserialize, Debug)]
pub struct Scenario {
    pub name: String,
    /// Behavior name the scenario seeds (grader-facing label).
    pub behavior: String,
    pub description: String,
    /// Which corpus half the scenario belongs to.
    #[serde(default)]
    pub split: Split,
    /// Free-form labels `--filter` selects on.
    #[serde(default)]
    pub tags: Vec<String>,
    /// The correct behavior stated as a direct instruction, appended to every
    /// scripted turn by the `explicit` arm - the known-good control that shows
    /// the checks can be satisfied. Required: without it the corpus cannot be
    /// graded against its own answer key.
    #[serde(default)]
    pub explicit_instruction: String,
    pub session: FixtureSession,
    pub repo: FixtureRepo,
    /// Fixed user turns, executed verbatim in order.
    pub script: Vec<String>,
}

impl Scenario {
    /// Load `<dir>/scenario.json`; the dir name must match the scenario's
    /// `name` field so the id used on the command line is unambiguous.
    pub fn load(dir: &Path) -> Result<Scenario> {
        let path = dir.join("scenario.json");
        let raw =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let scenario: Scenario =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        let dir_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if scenario.name != dir_name {
            anyhow::bail!(
                "scenario name {:?} does not match its directory name {:?}",
                scenario.name,
                dir_name
            );
        }
        if scenario.script.is_empty() {
            anyhow::bail!("scenario {:?} has an empty script", scenario.name);
        }
        if scenario.explicit_instruction.trim().is_empty() {
            anyhow::bail!(
                "scenario {:?} states no explicit_instruction (the explicit arm's input)",
                scenario.name
            );
        }
        if scenario.tags.iter().any(|t| t.trim().is_empty()) {
            anyhow::bail!("scenario {:?} carries an empty tag", scenario.name);
        }
        Ok(scenario)
    }

    /// The scenario's check set, which must sit beside `scenario.json`.
    pub fn checks_path(dir: &Path) -> std::path::PathBuf {
        dir.join(CHECKS_FILE)
    }
}

/// Every scenario under `dir`, sorted by name - the corpus `--split` and
/// `--filter` select from.
pub fn discover(dir: &Path) -> Result<Vec<(String, Scenario)>> {
    let mut out = Vec::new();
    let entries = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if !path.join("scenario.json").is_file() {
            continue;
        }
        let scenario = Scenario::load(&path)?;
        out.push((scenario.name.clone(), scenario));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_scenario_dir(name: &str, json: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let scenario_dir = dir.path().join(name);
        std::fs::create_dir_all(&scenario_dir).unwrap();
        std::fs::write(scenario_dir.join("scenario.json"), json).unwrap();
        (dir, scenario_dir)
    }

    const VALID: &str = r#"{
        "name": "demo",
        "behavior": "b",
        "description": "d",
        "explicit_instruction": "stop at review",
        "session": { "state_md": "s", "tasks_index_md": "t" },
        "repo": { "files": {"README.md": "r"}, "initial_commit": "seed" },
        "script": ["go", "how did it go?"]
    }"#;

    #[test]
    fn arms_and_harnesses_parse() {
        assert_eq!(Arm::parse("working").unwrap(), Arm::Working);
        assert_eq!(Arm::parse("no-block").unwrap(), Arm::NoBlock);
        assert_eq!(Arm::parse("v0.6").unwrap(), Arm::V06);
        assert_eq!(Arm::parse("explicit").unwrap(), Arm::Explicit);
        assert!(Arm::parse("working-tree").is_err());
        assert!(Arm::Explicit.appends_instruction());
        assert!(!Arm::Working.appends_instruction());
        assert!(Arm::V06.vendored_block());
        assert_eq!(Harness::parse("claude").unwrap(), Harness::Claude);
        assert_eq!(Harness::parse("codex").unwrap(), Harness::Codex);
        assert!(Harness::parse("pi").is_err());
        assert_eq!(Arm::Working.id(), "working");
        assert_eq!(Arm::V06.id(), "v0.6");
    }

    #[test]
    fn split_selects_the_corpus_halves() {
        assert_eq!(SplitSelect::parse("dev").unwrap(), SplitSelect::Dev);
        assert_eq!(SplitSelect::parse("all").unwrap(), SplitSelect::All);
        assert!(SplitSelect::parse("both").is_err());
        assert!(SplitSelect::Dev.accepts(Split::Dev));
        assert!(!SplitSelect::Dev.accepts(Split::Holdout));
        assert!(SplitSelect::All.accepts(Split::Holdout));
        assert!(SplitSelect::Holdout.accepts(Split::Holdout));
    }

    #[test]
    fn scenario_loads_and_defaults_apply() {
        let (_dir, path) = tmp_scenario_dir("demo", VALID);
        let s = Scenario::load(&path).unwrap();
        assert_eq!(s.name, "demo");
        assert_eq!(s.script.len(), 2);
        assert_eq!(s.session.name, "eval-fixture"); // default session name
        assert_eq!(s.repo.files["README.md"], "r");
        assert_eq!(s.split, Split::Dev); // default split
        assert!(s.tags.is_empty());
    }

    #[test]
    fn scenario_dir_name_must_match() {
        let (_dir, path) = tmp_scenario_dir("other", VALID);
        let err = Scenario::load(&path).unwrap_err().to_string();
        assert!(err.contains("does not match"), "{err}");
    }

    #[test]
    fn scenario_rejects_empty_script() {
        let empty = VALID.replace(r#""script": ["go", "how did it go?"]"#, r#""script": []"#);
        let (_dir, path) = tmp_scenario_dir("demo", &empty);
        let err = Scenario::load(&path).unwrap_err().to_string();
        assert!(err.contains("empty script"), "{err}");
    }

    #[test]
    fn scenario_requires_the_explicit_instruction() {
        let bare = VALID.replace(r#""explicit_instruction": "stop at review","#, "");
        let (_dir, path) = tmp_scenario_dir("demo", &bare);
        let err = Scenario::load(&path).unwrap_err().to_string();
        assert!(err.contains("explicit_instruction"), "{err}");
        let blank = VALID.replace("stop at review", "  ");
        let (_dir, path) = tmp_scenario_dir("demo", &blank);
        let err = Scenario::load(&path).unwrap_err().to_string();
        assert!(err.contains("explicit_instruction"), "{err}");
    }

    /// The shipped corpus, read from the repo: every scenario carries a check
    /// set, and every check loads under the loader's validation.
    #[test]
    fn the_shipped_corpus_loads_with_its_checks() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("evals")
            .join("scenarios");
        let found = discover(&root).unwrap();
        assert!(!found.is_empty(), "no scenarios under {}", root.display());
        for (name, scenario) in &found {
            let path = Scenario::checks_path(&root.join(name));
            assert!(path.is_file(), "{name} carries no {CHECKS_FILE}");
            let checks =
                crate::checks::ChecksFile::load(&path).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert!(!checks.checks.is_empty(), "{name} carries no checks");
            let behaviors: Vec<&str> = checks.checks.iter().map(|c| c.behavior.as_str()).collect();
            assert!(
                behaviors.contains(&scenario.behavior.as_str()),
                "{name}: no check names the scenario's behavior {:?}",
                scenario.behavior
            );
        }
    }

    #[test]
    fn discover_reads_the_corpus_sorted_and_skips_non_scenarios() {
        let dir = tempfile::TempDir::new().unwrap();
        for name in ["zeta", "alpha"] {
            let sub = dir.path().join(name);
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(
                sub.join("scenario.json"),
                VALID.replace(r#""name": "demo""#, &format!(r#""name": "{name}""#)),
            )
            .unwrap();
        }
        std::fs::create_dir_all(dir.path().join("not-a-scenario")).unwrap();
        let found = discover(dir.path()).unwrap();
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }
}
