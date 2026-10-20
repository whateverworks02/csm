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
        turn_trace(&self.path, index)
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

/// The trace of turn `index` under a run or rep dir. The writer and the
/// grader both come here, so the name has one home.
pub fn turn_trace(root: &Path, index: usize) -> PathBuf {
    root.join(format!("turn-{index:02}.jsonl"))
}

/// The artifact names of one checkpoint - the state a turn left behind.
/// Checkpoint 0 is the seeded fixture, before any turn; the last one is the
/// run's end state, which keeps the names the runner has always written.
pub struct Checkpoint {
    /// Directory holding the fixture session's record files.
    pub session: String,
    /// Directory holding the fixture repo's file tree.
    pub repo: String,
    pub status: String,
    pub diff: String,
    pub log: String,
}

impl Checkpoint {
    /// The repo-state texts this checkpoint records, as `(label, artifact
    /// name)`: the list of them lives here, once, for writer and reader alike.
    pub fn repo_texts(&self) -> [(&'static str, &str); 3] {
        [
            ("status", &self.status),
            ("diff", &self.diff),
            ("log", &self.log),
        ]
    }
}

/// The end-of-run checkpoint: `fixture-final/`, `repo-final/`, and the repo
/// texts at the run dir's root.
pub fn final_checkpoint() -> Checkpoint {
    Checkpoint {
        session: String::from("fixture-final"),
        repo: String::from("repo-final"),
        status: String::from("repo-status.txt"),
        diff: String::from("repo-diff.patch"),
        log: String::from("repo-log.txt"),
    }
}

/// The checkpoint recorded after turn `index` (0 = the seeded fixture).
pub fn turn_checkpoint(index: usize) -> Checkpoint {
    Checkpoint {
        session: format!("turn-{index:02}-session"),
        repo: format!("turn-{index:02}-repo"),
        status: format!("turn-{index:02}-repo-status.txt"),
        diff: format!("turn-{index:02}-repo-diff.patch"),
        log: format!("turn-{index:02}-repo-log.txt"),
    }
}

/// Copy the fixture session's record files (state.md, board, task files,
/// notes, scripts) into `<run>/<checkpoint.session>`.
pub fn snapshot_session(run: &RunDir, session_dir: &Path, cp: &Checkpoint) -> Result<()> {
    copy_tree(session_dir, &run.path.join(&cp.session), &[])
}

/// Record the fixture repo at a checkpoint: its file tree under
/// `<run>/<checkpoint.repo>` plus the uncommitted diff, status, and commit log
/// the run produced so far.
pub fn snapshot_repo(run: &RunDir, repo: &Path, cp: &Checkpoint) -> Result<()> {
    // `.git` is machinery, not repo state: it dwarfs the fixture tree and no
    // check reads it.
    copy_tree(repo, &run.path.join(&cp.repo), &[".git"])?;
    let queries = [
        (&["diff", "HEAD"][..], cp.diff.as_str()),
        (&["status", "--porcelain"][..], cp.status.as_str()),
        (&["log", "--oneline"][..], cp.log.as_str()),
    ];
    for (args, name) in queries {
        let text = git_text(repo, args).unwrap_or_default();
        std::fs::write(run.path.join(name), text).with_context(|| format!("writing {name}"))?;
    }
    Ok(())
}

/// Both halves of one checkpoint - the fixture session's records and the repo's
/// state - which are always taken together.
pub fn snapshot_checkpoint(
    run: &RunDir,
    session_dir: &Path,
    repo: &Path,
    cp: &Checkpoint,
) -> Result<()> {
    snapshot_session(run, session_dir, cp)?;
    snapshot_repo(run, repo, cp)
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

/// Recursively copy `src` into `dst` (files only; permissions not preserved),
/// skipping any entry named in `skip`.
fn copy_tree(src: &Path, dst: &Path, skip: &[&str]) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if skip.iter().any(|s| entry.file_name() == *s) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to, skip)?;
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
    /// Paths whose changes are concurrent-operator noise, not agent escapes.
    allowlist: Vec<PathBuf>,
}

/// Max depth: only the path itself (files), or recurse.
const DEEP: usize = usize::MAX;
const SHALLOW: usize = 1;

/// Directories under `~/.claude` that the harness itself churns. Every running
/// Claude Code process on the machine writes session transcripts, config
/// backups, plugin and todo state and caches here - the session driving the
/// eval among them - so a change in one cannot be attributed to the agent under
/// test. The paths that *are* the deployed wiring (`CLAUDE.md`, `settings.json`,
/// `skills/`) stay off this list and remain violations, which is the surface the
/// claim is about.
const HARNESS_NOISE_DIRS: [&str; 14] = [
    "backups",
    "cache",
    "debug",
    "downloads",
    "feedback",
    "file-history",
    "paste-cache",
    "plugins",
    "projects",
    "session-env",
    "sessions",
    "shell-snapshots",
    "statsig",
    "telemetry",
];

/// Files Finder rewrites whenever a watched directory has an open window; the
/// change carries no information about the run.
const FINDER_NOISE: [&str; 1] = [".DS_Store"];

impl Watch {
    pub fn new(home: &Path, repo_root: &Path) -> Watch {
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
            home.join(".claude.json"),
        ];
        for dir in HARNESS_NOISE_DIRS {
            allowlist.push(home.join(".claude").join(dir));
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
            .any(|a| path.starts_with(a) || path == a)
            || path
                .file_name()
                .map(|n| FINDER_NOISE.iter().any(|f| n == *f))
                .unwrap_or(false);
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
        snapshot_session(&run, &session, &final_checkpoint()).unwrap();
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
        snapshot_repo(&run, &repo, &final_checkpoint()).unwrap();
        let diff = std::fs::read_to_string(run.path.join("repo-diff.patch")).unwrap();
        assert!(diff.contains("-one"), "{diff}");
        assert!(diff.contains("+two"), "{diff}");
        let log = std::fs::read_to_string(run.path.join("repo-log.txt")).unwrap();
        assert!(log.contains("one"), "{log}");
        let status = std::fs::read_to_string(run.path.join("repo-status.txt")).unwrap();
        assert!(status.contains("M a.txt"), "{status}");
        // The file tree is recorded, minus git's own machinery.
        assert_eq!(
            std::fs::read_to_string(run.path.join("repo-final").join("a.txt")).unwrap(),
            "two\n"
        );
        assert!(!run.path.join("repo-final").join(".git").exists());
    }

    #[test]
    fn per_turn_checkpoints_get_their_own_names() {
        let cp = turn_checkpoint(3);
        assert_eq!(cp.session, "turn-03-session");
        assert_eq!(cp.repo, "turn-03-repo");
        assert_eq!(cp.status, "turn-03-repo-status.txt");
        assert_eq!(cp.diff, "turn-03-repo-diff.patch");
        assert_eq!(cp.log, "turn-03-repo-log.txt");
        // Checkpoint 0 is the seeded fixture, before any turn ran.
        assert_eq!(turn_checkpoint(0).session, "turn-00-session");
        let fin = final_checkpoint();
        assert_eq!(fin.session, "fixture-final");
        assert_eq!(fin.status, "repo-status.txt");
    }

    #[test]
    fn a_turn_checkpoint_records_both_halves() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session = tmp.path().join("session");
        std::fs::create_dir_all(session.join("tasks")).unwrap();
        std::fs::write(session.join("tasks").join("INDEX.md"), "seed board\n").unwrap();
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

        let run = RunDir::create(tmp.path(), "demo.turn").unwrap();
        let cp = turn_checkpoint(0);
        snapshot_session(&run, &session, &cp).unwrap();
        snapshot_repo(&run, &repo, &cp).unwrap();
        assert_eq!(
            std::fs::read_to_string(run.path.join(&cp.session).join("tasks").join("INDEX.md"))
                .unwrap(),
            "seed board\n"
        );
        assert_eq!(
            std::fs::read_to_string(run.path.join(&cp.status)).unwrap(),
            ""
        );
        assert!(std::fs::read_to_string(run.path.join(&cp.log))
            .unwrap()
            .contains("one"));
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
        let watch = Watch::new(home, repo);
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

        let watch = Watch::new(&home, &repo);
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

    /// The harness's own bookkeeping churns under `~/.claude` for every session
    /// on the machine, while the deployed wiring - the block, the settings, the
    /// skills - stays a violation.
    #[test]
    fn hermeticity_separates_harness_bookkeeping_from_the_deployed_wiring() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        let before = seed_watch(&home, &repo);

        for (dir, file) in [
            ("projects", "-Users-x-csm/abc.jsonl"),
            ("backups", ".claude.json.backup.1"),
            ("sessions", "64250.json"),
            ("plugins/store", "p.json"),
        ] {
            let path = home.join(".claude").join(dir).join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "churn\n").unwrap();
        }
        std::fs::write(home.join(".DS_Store"), "finder\n").unwrap();
        // The wiring the claim is about.
        std::fs::write(home.join(".claude").join("CLAUDE.md"), "block\n").unwrap();
        std::fs::write(home.join(".claude").join("settings.json"), "{}\n").unwrap();

        let watch = Watch::new(&home, &repo);
        let (violations, noise) = watch.diff(before).unwrap();
        assert!(
            violations.iter().any(|s| s.contains("CLAUDE.md")),
            "{violations:?}"
        );
        assert!(
            violations.iter().any(|s| s.contains("settings.json")),
            "{violations:?}"
        );
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(noise.iter().any(|s| s.contains("abc.jsonl")), "{noise:?}");
        assert!(noise.iter().any(|s| s.contains(".DS_Store")), "{noise:?}");
    }
}
