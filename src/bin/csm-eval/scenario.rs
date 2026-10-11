//! Scenario model: the unit of evaluation. One scenario = a fixture (session
//! files + a tiny repo) reconstructing a starting situation, plus a fixed
//! user-turn script that never branches on what the agent did.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// The contrast arm under test. The headline number is a delta between arms,
/// never an absolute score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    /// The claim under test: the working-tree [csm] block deployed by `csm init`.
    WorkingTree,
    /// Known-unbound control: no csm wiring at all (init is skipped), so the
    /// agent sees a bare harness.
    NoBlock,
}

impl Arm {
    pub fn parse(s: &str) -> Result<Arm> {
        match s {
            "working-tree" => Ok(Arm::WorkingTree),
            "no-block" => Ok(Arm::NoBlock),
            other => anyhow::bail!("unknown arm {other:?} (expected: working-tree, no-block)"),
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Arm::WorkingTree => "working-tree",
            Arm::NoBlock => "no-block",
        }
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
        Ok(scenario)
    }
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
        "session": { "state_md": "s", "tasks_index_md": "t" },
        "repo": { "files": {"README.md": "r"}, "initial_commit": "seed" },
        "script": ["go", "how did it go?"]
    }"#;

    #[test]
    fn arms_and_harnesses_parse() {
        assert_eq!(Arm::parse("working-tree").unwrap(), Arm::WorkingTree);
        assert_eq!(Arm::parse("no-block").unwrap(), Arm::NoBlock);
        assert!(Arm::parse("v0.6").is_err());
        assert_eq!(Harness::parse("claude").unwrap(), Harness::Claude);
        assert_eq!(Harness::parse("codex").unwrap(), Harness::Codex);
        assert!(Harness::parse("pi").is_err());
        assert_eq!(Arm::WorkingTree.id(), "working-tree");
    }

    #[test]
    fn scenario_loads_and_defaults_apply() {
        let (_dir, path) = tmp_scenario_dir("demo", VALID);
        let s = Scenario::load(&path).unwrap();
        assert_eq!(s.name, "demo");
        assert_eq!(s.script.len(), 2);
        assert_eq!(s.session.name, "eval-fixture"); // default session name
        assert_eq!(s.repo.files["README.md"], "r");
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
}
