//! Run artifacts: the per-run directory layout, the manifest, final fixture
//! snapshots, the fixture repo diff, and the hermeticity assertion
//! (filesystem-scoped: nothing outside the sandbox may change during a run).

use anyhow::{Context, Result};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The per-run artifact directory under `evals/runs/<run-id>/`.
pub struct RunDir {
    pub path: PathBuf,
}

impl RunDir {
    pub fn create(runs_root: &Path, run_id: &str) -> Result<RunDir> {
        let path = runs_root.join(run_id);
        std::fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
        Ok(RunDir { path })
    }

    pub fn turn_trace(&self, index: usize) -> PathBuf {
        self.path.join(format!("turn-{index:02}.jsonl"))
    }

    pub fn turn_stderr(&self, index: usize) -> PathBuf {
        self.path.join(format!("turn-{index:02}.stderr.txt"))
    }

    /// Append a line to the runner log (diagnostics alongside the artifacts).
    pub fn log(&self, line: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.join("runner.log"))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn write_manifest(&self, manifest: &serde_json::Value) -> Result<()> {
        std::fs::write(
            self.path.join("manifest.json"),
            serde_json::to_string_pretty(manifest)?,
        )
        .context("writing manifest.json")
    }
}

/// Copy the final fixture session files (state.md, board, task files, notes,
/// scripts) into the run dir - the record state the run left behind.
pub fn snapshot_fixture_session(run: &RunDir, session_dir: &Path) -> Result<()> {
    copy_tree(session_dir, &run.path.join("fixture-final"))
}

/// The fixture repo's end state: uncommitted diff, status, and the commit log
/// the run produced.
pub fn snapshot_repo(run: &RunDir, repo: &Path) -> Result<()> {
    let diff = git_text(repo, &["diff", "HEAD"]).unwrap_or_default();
    std::fs::write(run.path.join("repo-diff.patch"), diff).context("writing repo-diff.patch")?;
    let status = git_text(repo, &["status", "--porcelain"]).unwrap_or_default();
    std::fs::write(run.path.join("repo-status.txt"), status).context("writing repo-status.txt")?;
    let log = git_text(repo, &["log", "--oneline"]).unwrap_or_default();
    std::fs::write(run.path.join("repo-log.txt"), log).context("writing repo-log.txt")?;
    Ok(())
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    // --no-optional-locks: read-only queries must not refresh the index
    // (a write the hermeticity watch would flag on the repo under test).
    let out = std::process::Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.stderr.is_empty() {
        text.push_str(&String::from_utf8_lossy(&out.stderr));
    }
    Ok(text)
}

/// Recursively copy `src` into `dst` (files only; permissions not preserved).
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

// --- hermeticity assertion ----------------------------------------------------

/// One watched filesystem entry's signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EntrySig {
    is_dir: bool,
    len: u64,
    mtime: Option<SystemTime>,
}

fn sig_of(meta: &std::fs::Metadata) -> EntrySig {
    // Dirs carry a constant signature: a dir's mtime churns whenever a child
    // changes (including allowlisted operator writes), and the file-level
    // added/removed/changed entries already carry the real signal.
    let is_dir = meta.is_dir();
    EntrySig {
        is_dir,
        len: if is_dir { 0 } else { meta.len() },
        mtime: if is_dir { None } else { meta.modified().ok() },
    }
}

/// A filesystem-scoped snapshot of the watch set: everything outside the
/// sandbox the run could plausibly touch. The real home is watched at depth 1
/// (its direct entries - new dotfiles and dotdirs surface as additions), the
/// config stores (`~/.claude`, `~/.codex`, `~/.csm`, `~/.config`, `~/.ssh`)
/// recursively, and the repo recursively (minus `target/` and `evals/runs/`,
/// which the runner itself writes). Changes inside unwatched subtrees (e.g.
/// `~/Library`) are outside the scope of the claim.
pub struct Watch {
    roots: Vec<(PathBuf, usize)>,
    /// Paths whose changes are concurrent-operator noise, not agent escapes:
    /// the session driving the eval writes its own transcript and history
    /// while the run is in flight.
    allowlist: Vec<PathBuf>,
}

/// Max depth: only the path itself (files), or recurse.
const DEEP: usize = usize::MAX;
const SHALLOW: usize = 1;

impl Watch {
    pub fn new(home: &Path, repo_root: &Path, operator_project: Option<PathBuf>) -> Watch {
        let mut roots: Vec<(PathBuf, usize)> = vec![(home.to_path_buf(), SHALLOW)];
        for store in [".claude", ".csm", ".config", ".ssh"] {
            roots.push((home.join(store), DEEP));
        }
        // ~/.codex is watched narrowly: the session store plus the csm-managed
        // files. A tree-wide watch false-positives on the user's own codex
        // daemon, which churns logs and sqlite WALs outside any eval run.
        roots.push((home.join(".codex").join("sessions"), DEEP));
        for file in ["auth.json", "config.toml", "AGENTS.md", "hooks.json"] {
            roots.push((home.join(".codex").join(file), DEEP));
        }
        roots.push((repo_root.to_path_buf(), DEEP));
        let mut allowlist = vec![
            home.join(".claude").join("history.jsonl"),
            home.join(".claude").join(".last-update-result.json"),
            home.join(".claude").join("statsig"),
            home.join(".claude.json"),
        ];
        if let Some(p) = operator_project {
            allowlist.push(p);
        }
        Watch { roots, allowlist }
    }

    /// Root list for the report (self-documenting hermeticity.json).
    pub(crate) fn root_list(&self) -> Vec<String> {
        self.roots
            .iter()
            .map(|(p, _)| p.display().to_string())
            .collect()
    }

    pub(crate) fn snapshot(&self) -> Result<BTreeMap<PathBuf, EntrySig>> {
        let mut out = BTreeMap::new();
        for (root, depth) in &self.roots {
            walk(root, *depth, &mut out)?;
        }
        Ok(out)
    }

    /// Assert the run changed nothing outside the sandbox. Returns the
    /// (violations, concurrent_noise) split so the report is actionable
    /// without the operator's own session activity failing every run.
    fn diff(&self, before: BTreeMap<PathBuf, EntrySig>) -> Result<(Vec<String>, Vec<String>)> {
        let after = self.snapshot()?;
        let mut violations = Vec::new();
        let mut noise = Vec::new();
        for (path, sig) in &after {
            match before.get(path) {
                None => Self::record(path, "added", self, &mut noise, &mut violations),
                Some(old) if old != sig => {
                    Self::record(path, "changed", self, &mut noise, &mut violations)
                }
                Some(_) => {}
            }
        }
        for path in before.keys() {
            if !after.contains_key(path) {
                Self::record(path, "removed", self, &mut noise, &mut violations);
            }
        }
        Ok((violations, noise))
    }

    fn record(
        path: &Path,
        kind: &str,
        watch: &Watch,
        noise: &mut Vec<String>,
        violations: &mut Vec<String>,
    ) {
        let allowed = watch
            .allowlist
            .iter()
            .any(|a| path.starts_with(a) || path == a);
        let line = format!("{kind} {}", path.display());
        if allowed {
            noise.push(line);
        } else {
            violations.push(line);
        }
    }
}

/// Record `dir`'s signature (when it exists) and, while depth remains,
/// recurse into its children.
fn walk(dir: &Path, depth: usize, out: &mut BTreeMap<PathBuf, EntrySig>) -> Result<()> {
    let Ok(meta) = std::fs::metadata(dir) else {
        return Ok(()); // absent root: fine, its appearance is the signal
    };
    if !meta.is_dir() {
        out.insert(dir.to_path_buf(), sig_of(&meta));
        return Ok(());
    }
    if depth == 0 {
        out.insert(dir.to_path_buf(), sig_of(&meta));
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // The runner's own artifact dir is not part of the claim.
        if path
            .components()
            .any(|c| c.as_os_str() == "target" || c.as_os_str() == "runs")
        {
            continue;
        }
        let child_meta = entry.metadata()?;
        let is_dir = child_meta.is_dir();
        out.insert(path.clone(), sig_of(&child_meta));
        if is_dir {
            walk(&path, depth.saturating_sub(1), out)?;
        }
    }
    Ok(())
}

/// Run the before/after assertion and write `hermeticity.json`. Returns
/// whether the run passed (no violations).
pub fn assert_hermeticity(
    run: &RunDir,
    watch: &Watch,
    before: BTreeMap<PathBuf, EntrySig>,
) -> Result<bool> {
    let (violations, noise) = watch.diff(before)?;
    let passed = violations.is_empty();
    let report = json!({
        "scope": "filesystem (model-provider network calls out of scope)",
        "watched_roots": watch.root_list(),
        "passed": passed,
        "violations": violations,
        "concurrent_operator_noise": noise,
    });
    std::fs::write(
        run.path.join("hermeticity.json"),
        serde_json::to_string_pretty(&report)?,
    )
    .context("writing hermeticity.json")?;
    Ok(passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_dir_layout_and_fixture_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let run = RunDir::create(tmp.path(), "demo.working-tree.claude.1").unwrap();
        assert_eq!(run.turn_trace(1).file_name().unwrap(), "turn-01.jsonl");
        assert_eq!(
            run.turn_stderr(2).file_name().unwrap(),
            "turn-02.stderr.txt"
        );

        let session = tmp.path().join("session-src");
        std::fs::create_dir_all(session.join("tasks")).unwrap();
        std::fs::write(session.join("state.md"), "state").unwrap();
        std::fs::write(session.join("tasks").join("INDEX.md"), "board").unwrap();
        snapshot_fixture_session(&run, &session).unwrap();
        assert_eq!(
            std::fs::read_to_string(run.path.join("fixture-final").join("state.md")).unwrap(),
            "state"
        );
        assert_eq!(
            std::fs::read_to_string(
                run.path
                    .join("fixture-final")
                    .join("tasks")
                    .join("INDEX.md")
            )
            .unwrap(),
            "board"
        );
    }

    #[test]
    fn repo_snapshot_writes_diff_status_log() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "t"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
        }
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["add", "-A"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["commit", "-m", "one"])
            .output()
            .unwrap();

        let run = RunDir::create(tmp.path(), "demo.snap").unwrap();
        // Uncommitted change -> the diff is non-empty, the log has the commit.
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();
        snapshot_repo(&run, &repo).unwrap();
        let diff = std::fs::read_to_string(run.path.join("repo-diff.patch")).unwrap();
        assert!(diff.contains("-one"), "{diff}");
        assert!(diff.contains("+two"), "{diff}");
        let log = std::fs::read_to_string(run.path.join("repo-log.txt")).unwrap();
        assert!(log.contains("one"), "{log}");
        let status = std::fs::read_to_string(run.path.join("repo-status.txt")).unwrap();
        assert!(status.contains("M a.txt"), "{status}");
    }

    /// Build a fake home + repo with the expected watch structure and take a
    /// pre-snapshot; the caller mutates and diffs.
    fn seed_watch(home: &Path, repo: &Path) -> BTreeMap<PathBuf, EntrySig> {
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(home.join(".csm")).unwrap();
        std::fs::create_dir_all(repo).unwrap();
        std::fs::write(home.join(".claude").join("history.jsonl"), "h1\n").unwrap();
        std::fs::write(home.join(".codex").join("config.toml"), "cfg1\n").unwrap();
        std::fs::write(home.join(".codex").join("logs_2.sqlite-wal"), "wal1\n").unwrap();
        std::fs::write(home.join(".csm").join("index.json"), "{}\n").unwrap();
        std::fs::write(home.join(".zsh_history"), "old\n").unwrap();
        std::fs::write(repo.join("code.txt"), "rev1\n").unwrap();
        let watch = Watch::new(home, repo, None);
        watch.snapshot().unwrap()
    }

    #[test]
    fn hermeticity_diff_classifies_violations_and_operator_noise() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        let before = seed_watch(&home, &repo);

        // Concurrent-operator writes: allowlisted.
        std::fs::write(home.join(".claude").join("history.jsonl"), "h1\nh2\n").unwrap();
        // The user's codex daemon churns top-level logs/sqlite outside any
        // eval run; the narrowed watch must not flag it.
        std::fs::write(home.join(".codex").join("logs_2.sqlite-wal"), "wal2\n").unwrap();
        // Agent-escape writes: not allowlisted.
        std::fs::write(home.join(".codex").join("config.toml"), "cfg2\n").unwrap();
        std::fs::write(home.join(".csm").join("index.json"), "{\"sessions\":{}}\n").unwrap();
        std::fs::write(home.join(".csm").join("escaped.json"), "{}\n").unwrap();
        std::fs::write(home.join(".zsh_history"), "old\nnew\n").unwrap();
        std::fs::write(repo.join("code.txt"), "rev2\n").unwrap();

        let watch = Watch::new(&home, &repo, None);
        let (violations, noise) = watch.diff(before).unwrap();
        let joins = |v: &[String]| v.join("\n");
        let v = joins(&violations);
        let n = joins(&noise);
        assert!(v.contains("changed"), "{v}");
        assert!(v.contains(".codex/config.toml"), "{v}");
        assert!(v.contains(".csm/index.json"), "{v}");
        assert!(
            v.contains("added") && v.contains(".csm/escaped.json"),
            "{v}"
        );
        assert!(v.contains(".zsh_history"), "{v}");
        assert!(v.contains("repo"), "{v}");
        assert!(n.contains("history.jsonl"), "{n}");
        // Daemon churn at ~/.codex top level (outside sessions/ and the
        // csm-managed files) is out of watch scope.
        assert!(!violations.iter().any(|s| s.contains("sqlite-wal")), "{v}");
        assert!(
            !violations.iter().any(|s| s.contains("history.jsonl")),
            "{v}"
        );
    }

    #[test]
    fn hermeticity_operator_project_dir_is_noise() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        // `~/.claude/projects` exists before the run; only the operator
        // session's own subdir appears during it.
        std::fs::create_dir_all(home.join(".claude").join("projects")).unwrap();
        let before = seed_watch(&home, &repo);
        let operator_project = home.join(".claude").join("projects").join("-Users-x-csm");
        std::fs::create_dir_all(&operator_project).unwrap();
        std::fs::write(operator_project.join("s.jsonl"), "a\n").unwrap();

        let watch = Watch::new(&home, &repo, Some(operator_project));
        let (violations, noise) = watch.diff(before).unwrap();
        assert!(violations.is_empty(), "{violations:?}");
        assert!(noise.iter().any(|s| s.contains("s.jsonl")), "{noise:?}");
    }
}
